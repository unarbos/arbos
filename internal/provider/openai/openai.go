// Package openai is the reference LLMProvider: an adapter for any
// OpenAI-compatible Chat Completions endpoint (OpenAI, OpenRouter, vLLM, Ollama,
// most aggregators). It is the one real provider the kernel ships; native
// adapters (Anthropic, Gemini, Bedrock) are separate packages behind the same
// ports.LLMProvider, added later.
//
// Auth goes through the secret.Broker (ADR-0016): the provider holds only a
// core.SecretRef, never the key, and the broker refuses to attach it unless the
// request host is on the binding allowlist — so a prompt-injected agent cannot
// exfiltrate the key to an arbitrary base_url.
package openai

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"strings"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/ports"
	"github.com/unarbos/arbos/internal/provider/internal/providerkit"
	"github.com/unarbos/arbos/internal/secret"
)

// Authorizer, Option, and the With* options are the shared providerkit ones; the
// package re-exports them so callers keep using openai.WithAuth etc.
type (
	Authorizer = providerkit.Authorizer
	Option     = providerkit.Option
)

var (
	WithName         = providerkit.WithName
	WithHTTPClient   = providerkit.WithHTTPClient
	WithAuth         = providerkit.WithAuth
	WithCapabilities = providerkit.WithCapabilities
	WithImageModel   = providerkit.WithImageModel
)

// Provider implements ports.LLMProvider against a Chat Completions endpoint.
type Provider struct {
	base    providerkit.Base
	baseURL string
}

var _ ports.LLMProvider = (*Provider)(nil)

// New builds a provider for the given base URL (e.g. "https://api.openai.com/v1").
// The shared kit supplies the redirect-refusing client (anti-exfiltration) and
// the name/auth/capabilities plumbing. Default capabilities: tools only.
func New(baseURL string, opts ...Option) *Provider {
	return &Provider{
		base:    providerkit.NewBase("openai", ports.Capabilities{Tools: true}, opts...),
		baseURL: strings.TrimRight(baseURL, "/"),
	}
}

func (p *Provider) Name() string                     { return p.base.Name }
func (p *Provider) Capabilities() ports.Capabilities { return p.base.Caps }

// Stream sends the request and returns a channel of chunks. The HTTP request is
// issued synchronously so config/transport errors surface immediately as a
// returned error; the body is consumed on a goroutine that closes the channel
// when the stream ends or ctx is cancelled.
func (p *Provider) Stream(ctx context.Context, req core.LLMRequest) (<-chan core.LLMChunk, error) {
	body, err := json.Marshal(p.buildRequest(req))
	if err != nil {
		return nil, fmt.Errorf("openai: marshal request: %w", err)
	}
	httpReq, err := http.NewRequestWithContext(ctx, http.MethodPost, p.baseURL+"/chat/completions", bytes.NewReader(body))
	if err != nil {
		return nil, fmt.Errorf("openai: build request: %w", err)
	}
	httpReq.Header.Set("Content-Type", "application/json")
	httpReq.Header.Set("Accept", "text/event-stream")
	// Session-affinity caching: when a session id is present and caching is not
	// disabled, send the affinity headers providers use to key a prompt cache.
	// They are ignored by endpoints that do not use them, so this stays safe on a
	// generic OpenAI-compatible server; body-level cache_control markers belong to
	// native adapters added later (ADR-0023).
	if req.SessionID != "" && req.CacheRetention != core.CacheNone {
		httpReq.Header.Set("session_id", req.SessionID)
		httpReq.Header.Set("x-session-affinity", req.SessionID)
	}
	if err := p.base.Authorize(ctx, httpReq); err != nil {
		return nil, fmt.Errorf("openai: authorize: %w", err)
	}

	resp, err := p.base.HTTPClient.Do(httpReq)
	if err != nil {
		return nil, providerkit.TransportError("openai", err)
	}
	if resp.StatusCode != http.StatusOK {
		defer func() { _ = resp.Body.Close() }()
		return nil, providerkit.NonOKError("openai", resp)
	}

	out := make(chan core.LLMChunk, 16)
	go p.consume(ctx, resp, out)
	return out, nil
}

// consume parses the SSE stream into chunks. Content/reasoning deltas are
// forwarded as they arrive; tool-call fragments are accumulated by index and
// emitted, fully-formed, in a final chunk alongside usage (ADR contract: the
// adapter accumulates partials, the engine never sees fragments).
func (p *Provider) consume(ctx context.Context, resp *http.Response, out chan<- core.LLMChunk) {
	defer close(out)
	defer func() { _ = resp.Body.Close() }()

	acc := providerkit.NewToolAccumulator()
	var usage *core.Usage
	var finishReason string
	var citations []core.Citation
	var images []core.ContentBlock

	// A malformed line is skipped (return true); a cancelled send stops the loop
	// (return false). ScanSSE handles buffering, data: framing, [DONE], and
	// surfaces a truncated-stream error.
	err := providerkit.ScanSSE(ctx, resp.Body, func(data []byte) bool {
		var evt wireChunk
		if json.Unmarshal(data, &evt) != nil {
			return true
		}
		if evt.Usage != nil {
			usage = &core.Usage{
				PromptTokens:     evt.Usage.PromptTokens,
				CompletionTokens: evt.Usage.CompletionTokens,
				TotalTokens:      evt.Usage.TotalTokens,
			}
		}
		for _, choice := range evt.Choices {
			if choice.FinishReason != "" {
				finishReason = choice.FinishReason
			}
			d := choice.Delta
			if d.Content != "" && !providerkit.Send(ctx, out, core.LLMChunk{ContentDelta: d.Content}) {
				return false
			}
			if d.Reasoning != "" && !providerkit.Send(ctx, out, core.LLMChunk{ReasoningDelta: d.Reasoning}) {
				return false
			}
			for _, f := range d.ToolCalls {
				idx := 0
				if f.Index != nil {
					idx = *f.Index
				}
				if !providerkit.EmitToolProgress(ctx, out, acc, idx, f.ID, f.Function.Name, f.Function.Arguments) {
					return false
				}
			}
			for _, a := range d.Annotations {
				if a.Type != "url_citation" || a.URLCitation.URL == "" {
					continue
				}
				citations = append(citations, core.Citation{
					URL:        a.URLCitation.URL,
					Title:      a.URLCitation.Title,
					Content:    a.URLCitation.Content,
					StartIndex: a.URLCitation.StartIndex,
					EndIndex:   a.URLCitation.EndIndex,
				})
			}
			// Generated images ride the delta as base64 data: URLs. Like
			// citations they are accumulated and delivered on the final chunk,
			// fully-formed (the engine never sees fragments).
			for _, img := range d.Images {
				if data, ok := imageFromDataURL(img.ImageURL.URL); ok {
					images = append(images, core.ContentBlock{Type: core.BlockImage, Image: &data})
				}
			}
		}
		return true
	})
	if err != nil {
		providerkit.Send(ctx, out, core.LLMChunk{Done: true, Err: fmt.Errorf("openai: stream read: %w", err)})
		return
	}

	// acc.Calls() is nil when no tool calls streamed, so assign directly (same
	// shape as the anthropic/google adapters).
	providerkit.Send(ctx, out, core.LLMChunk{Done: true, Usage: usage, ToolCalls: acc.Calls(), Citations: citations, Images: images, FinishReason: finishReason})
}

// reasoningEffort maps the neutral reasoning level to the Chat Completions
// reasoning_effort value. "" and "off" send nothing (provider default / no
// reasoning). OpenAI has no "xhigh", so it clamps to "high"; a native adapter
// with a richer ladder maps it exactly via model metadata (ADR-0023).
func reasoningEffort(r core.ReasoningLevel) string {
	switch r {
	case core.ReasoningMinimal:
		return "minimal"
	case core.ReasoningLow:
		return "low"
	case core.ReasoningMedium:
		return "medium"
	case core.ReasoningHigh, core.ReasoningXHigh:
		return "high"
	default: // "" or off
		return ""
	}
}

// buildRequest translates the neutral request into the Chat Completions body.
func (p *Provider) buildRequest(req core.LLMRequest) wireRequest {
	w := wireRequest{Model: req.Model, Stream: req.Stream}
	if req.Stream {
		w.StreamOptions = &wireStreamOptions{IncludeUsage: true}
	}
	if req.Temperature != nil {
		w.Temperature = req.Temperature
	}
	if req.MaxTokens > 0 {
		w.MaxTokens = req.MaxTokens
	}
	w.ReasoningEffort = reasoningEffort(req.Reasoning)
	// OpenAI's tool role rejects image_url (a 400 that would wedge the session on
	// every re-projection), so a tool result's image/file parts can't ride the
	// tool message. They are promoted to a user message emitted AFTER the run of
	// tool results — one user turn per tool batch — which OpenAI renders to
	// vision. Buffering until a non-tool message (or the end) keeps every tool
	// reply adjacent to its assistant tool_calls, as the API requires.
	var pendingVision []wireContentPart
	flushVision := func() {
		if len(pendingVision) > 0 {
			w.Messages = append(w.Messages, wireMessage{Role: "user", Content: pendingVision})
			pendingVision = nil
		}
	}
	for _, m := range req.Messages {
		if m.Role != core.RoleTool {
			flushVision()
		}
		w.Messages = append(w.Messages, toWireMessage(m))
		if m.Role == core.RoleTool {
			for _, b := range providerkit.ToolResultVisionParts(m) {
				if p, ok := visionContentPart(b); ok {
					pendingVision = append(pendingVision, p)
				}
			}
		}
	}
	flushVision()
	for _, ts := range req.Tools {
		w.Tools = append(w.Tools, wireTool{
			Type: "function",
			Function: &wireToolFn{
				Name:        ts.Name,
				Description: ts.Description,
				Parameters:  ts.Parameters,
			},
		})
	}
	// Web search rides as OpenRouter's web_search server tool: it sits in the
	// same tools array but the model invokes it and OpenRouter runs it
	// server-side, returning sources as message annotations (no client
	// dispatch). The legacy `:online` suffix and `web` plugin are deprecated.
	// On a non-OpenRouter OpenAI-compatible endpoint this entry is simply
	// ignored, matching the neutral "drop it when unsupported" contract.
	if req.WebSearch {
		w.Tools = append(w.Tools, wireTool{Type: webSearchToolType})
	}
	// Web fetch is the sibling server tool: the model calls it with a URL and
	// OpenRouter extracts the page content server-side. Independent of search,
	// same no-client-dispatch contract.
	if req.WebFetch {
		w.Tools = append(w.Tools, wireTool{Type: webFetchToolType})
	}
	// Image generation is the third server tool: the model calls it with a
	// prompt and OpenRouter generates the image server-side. Same
	// no-client-dispatch, drop-when-unsupported contract. The optional
	// parameters pick the backing image model (the endpoint's default
	// otherwise); the generated image reaches the model as a hosted URL it
	// embeds in its reply, and image-output chat models additionally attach
	// images to the response itself (parsed in consume).
	if req.ImageGen {
		t := wireTool{Type: imageGenToolType}
		if p.base.ImageModel != "" {
			params, _ := json.Marshal(struct {
				Model string `json:"model"`
			}{p.base.ImageModel})
			t.Parameters = params
		}
		w.Tools = append(w.Tools, t)
	}
	return w
}

// webSearchToolType / webFetchToolType are OpenRouter's identifiers for the
// server-side web tools. These provider-native strings are confined to this
// adapter; the kernel only knows the neutral LLMRequest.WebSearch / WebFetch
// bools.
const (
	webSearchToolType = "openrouter:web_search"
	webFetchToolType  = "openrouter:web_fetch"
	imageGenToolType  = "openrouter:image_generation"
)

// imageFromDataURL parses a base64 data: URL ("data:image/png;base64,...")
// into ImageData. The second return is false for any other URL shape — a
// remote URL is not inlined here (nothing in the OpenRouter image path emits
// one today, and silently fetching it would be an exfiltration vector).
func imageFromDataURL(url string) (core.ImageData, bool) {
	rest, ok := strings.CutPrefix(url, "data:")
	if !ok {
		return core.ImageData{}, false
	}
	mime, data, ok := strings.Cut(rest, ";base64,")
	if !ok || mime == "" || data == "" {
		return core.ImageData{}, false
	}
	return core.ImageData{MimeType: mime, Data: data}, true
}

func toWireMessage(m core.Message) wireMessage {
	wm := wireMessage{Role: string(m.Role), Content: m.Content}
	// Multimodal: when the message carries non-text Parts, send a content array
	// (text block first, then images as data: URLs) instead of a plain string.
	// A tool-role message is the exception: OpenAI rejects image_url on
	// role:"tool", so it stays text-only here and its image/file parts are
	// promoted to a following user message by buildRequest (ADR-0022).
	if len(m.Parts) > 0 {
		if m.Role == core.RoleTool {
			wm.Content = providerkit.ToolResultText(m)
		} else {
			parts := make([]wireContentPart, 0, len(m.Parts)+1)
			if m.Content != "" {
				parts = append(parts, wireContentPart{Type: "text", Text: m.Content})
			}
			for _, b := range m.Parts {
				if b.Type == core.BlockText {
					parts = append(parts, wireContentPart{Type: "text", Text: b.Text})
					continue
				}
				if p, ok := visionContentPart(b); ok {
					parts = append(parts, p)
				}
			}
			wm.Content = parts
		}
	}
	if m.Role == core.RoleTool {
		wm.ToolCallID = m.ToolCallID
	}
	for _, tc := range m.ToolCalls {
		wm.ToolCalls = append(wm.ToolCalls, wireToolCall{
			ID:   tc.ID,
			Type: "function",
			Function: wireToolCallFn{
				Name:      tc.Name,
				Arguments: string(tc.Args),
			},
		})
	}
	return wm
}

// visionContentPart renders an image or file content block as an OpenAI content
// part (image_url / file, data URL). The second return is false for a block that
// is not media or whose payload is missing.
func visionContentPart(b core.ContentBlock) (wireContentPart, bool) {
	switch b.Type {
	case core.BlockImage:
		if b.Image != nil {
			return wireContentPart{
				Type:     "image_url",
				ImageURL: &wireImageURL{URL: "data:" + b.Image.MimeType + ";base64," + b.Image.Data},
			}, true
		}
	case core.BlockFile:
		if b.File != nil {
			return wireContentPart{
				Type: "file",
				File: &wireFile{
					Filename: b.File.Name,
					FileData: "data:" + b.File.MimeType + ";base64," + b.File.Data,
				},
			}, true
		}
	default:
		// Non-media blocks (e.g. text) carry no vision content part.
	}
	return wireContentPart{}, false
}

// Ensure the broker satisfies Authorizer at compile time (documentation of the
// intended production wiring).
var _ Authorizer = (*secret.Broker)(nil)
