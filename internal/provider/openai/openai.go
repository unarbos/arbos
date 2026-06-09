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
		return nil, fmt.Errorf("openai: request: %w", err)
	}
	if resp.StatusCode != http.StatusOK {
		defer func() { _ = resp.Body.Close() }()
		return nil, fmt.Errorf("openai: status %d: %s", resp.StatusCode, providerkit.ReadSnippet(resp.Body))
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
				acc.Add(idx, f.ID, f.Function.Name, f.Function.Arguments)
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
	providerkit.Send(ctx, out, core.LLMChunk{Done: true, Usage: usage, ToolCalls: acc.Calls(), FinishReason: finishReason})
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
	for _, m := range req.Messages {
		w.Messages = append(w.Messages, toWireMessage(m))
	}
	for _, ts := range req.Tools {
		w.Tools = append(w.Tools, wireTool{
			Type: "function",
			Function: wireToolFn{
				Name:        ts.Name,
				Description: ts.Description,
				Parameters:  ts.Parameters,
			},
		})
	}
	return w
}

func toWireMessage(m core.Message) wireMessage {
	wm := wireMessage{Role: string(m.Role), Content: m.Content}
	// Multimodal: when the message carries non-text Parts, send a content array
	// (text block first, then images as data: URLs) instead of a plain string.
	// A tool-role message is the exception: OpenAI rejects image_url on
	// role:"tool" with a 400, and the bad result would re-project every later
	// turn and permanently wedge the session — so images there downgrade to a
	// text placeholder (ADR-0022).
	if len(m.Parts) > 0 {
		if m.Role == core.RoleTool {
			wm.Content = providerkit.ToolResultText(m)
		} else {
			parts := make([]wireContentPart, 0, len(m.Parts)+1)
			if m.Content != "" {
				parts = append(parts, wireContentPart{Type: "text", Text: m.Content})
			}
			for _, b := range m.Parts {
				switch b.Type {
				case core.BlockImage:
					if b.Image != nil {
						parts = append(parts, wireContentPart{
							Type:     "image_url",
							ImageURL: &wireImageURL{URL: "data:" + b.Image.MimeType + ";base64," + b.Image.Data},
						})
					}
				case core.BlockText:
					parts = append(parts, wireContentPart{Type: "text", Text: b.Text})
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

// Ensure the broker satisfies Authorizer at compile time (documentation of the
// intended production wiring).
var _ Authorizer = (*secret.Broker)(nil)
