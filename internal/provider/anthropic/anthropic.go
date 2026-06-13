// Package anthropic is a native ports.LLMProvider for Anthropic's Messages API
// (the second real provider beyond the OpenAI-compatible one). It speaks the
// Messages streaming SSE shape directly: system prompt hoisted out of messages,
// tool_use / tool_result blocks, thinking blocks, and message_delta usage. Auth
// goes through the same secret.Broker seam as the OpenAI adapter (configured
// with an x-api-key injector by the host), so the provider holds only a
// SecretRef.
//
// Faithful to the provider API pi targets. Documented gaps (no live key in this
// environment to exercise them): image content blocks are not yet sent, the
// thinking-signature multi-turn round-trip is not replayed (it needs an opaque
// per-message channel and a live key to verify — see ADR-0003, superseded), and
// OAuth login is deferred pending client credentials — the broker already
// abstracts credential injection, so adding OAuth is a credential-acquisition
// flow, not an adapter change.
package anthropic

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
)

const (
	defaultBaseURL     = "https://api.anthropic.com"
	anthropicVersion   = "2023-06-01"
	defaultMaxTokens   = 4096
	defaultThinkBudget = 4096
)

// Authorizer, Option, and the With* options are the shared providerkit ones,
// re-exported so callers keep using anthropic.WithAuth etc.
type (
	Authorizer = providerkit.Authorizer
	Option     = providerkit.Option
)

var (
	WithName       = providerkit.WithName
	WithHTTPClient = providerkit.WithHTTPClient
	WithAuth       = providerkit.WithAuth
)

// Provider implements ports.LLMProvider against the Anthropic Messages API.
type Provider struct {
	base    providerkit.Base
	baseURL string
}

var _ ports.LLMProvider = (*Provider)(nil)

// New builds a provider for the given base URL (default api.anthropic.com). The
// shared kit supplies the redirect-refusing client (anti-exfiltration) and the
// name/auth/capabilities plumbing.
func New(baseURL string, opts ...Option) *Provider {
	if baseURL == "" {
		baseURL = defaultBaseURL
	}
	return &Provider{
		base:    providerkit.NewBase("anthropic", ports.Capabilities{Tools: true, Reasoning: true, Vision: true}, opts...),
		baseURL: strings.TrimRight(baseURL, "/"),
	}
}

func (p *Provider) Name() string                     { return p.base.Name }
func (p *Provider) Capabilities() ports.Capabilities { return p.base.Caps }

func (p *Provider) Stream(ctx context.Context, req core.LLMRequest) (<-chan core.LLMChunk, error) {
	body, err := json.Marshal(p.buildRequest(req))
	if err != nil {
		return nil, fmt.Errorf("anthropic: marshal request: %w", err)
	}
	httpReq, err := http.NewRequestWithContext(ctx, http.MethodPost, p.baseURL+"/v1/messages", bytes.NewReader(body))
	if err != nil {
		return nil, fmt.Errorf("anthropic: build request: %w", err)
	}
	httpReq.Header.Set("Content-Type", "application/json")
	httpReq.Header.Set("Accept", "text/event-stream")
	httpReq.Header.Set("anthropic-version", anthropicVersion)
	if err := p.base.Authorize(ctx, httpReq); err != nil {
		return nil, fmt.Errorf("anthropic: authorize: %w", err)
	}
	resp, err := p.base.HTTPClient.Do(httpReq)
	if err != nil {
		return nil, providerkit.TransportError("anthropic", err)
	}
	if resp.StatusCode != http.StatusOK {
		defer func() { _ = resp.Body.Close() }()
		return nil, providerkit.NonOKError("anthropic", resp)
	}
	out := make(chan core.LLMChunk, 16)
	go p.consume(ctx, resp, out)
	return out, nil
}

// consume parses the Messages SSE stream. text_delta -> ContentDelta,
// thinking_delta -> ReasoningDelta, tool_use blocks accumulate input_json_delta
// by index and emit fully-formed in the final chunk with usage.
func (p *Provider) consume(ctx context.Context, resp *http.Response, out chan<- core.LLMChunk) {
	defer close(out)
	defer func() { _ = resp.Body.Close() }()

	acc := providerkit.NewToolAccumulator()
	var usage core.Usage

	err := providerkit.ScanSSE(ctx, resp.Body, func(data []byte) bool {
		var ev wireEvent
		if json.Unmarshal(data, &ev) != nil {
			return true
		}
		switch ev.Type {
		case "message_start":
			if ev.Message != nil && ev.Message.Usage != nil {
				usage.PromptTokens = ev.Message.Usage.InputTokens
			}
		case "content_block_start":
			if ev.ContentBlock != nil && ev.ContentBlock.Type == "tool_use" {
				// Announce the call the instant it opens (zero bytes yet) so the
				// composing card appears before any arguments stream.
				if !providerkit.EmitToolProgress(ctx, out, acc, ev.Index, ev.ContentBlock.ID, ev.ContentBlock.Name, "") {
					return false
				}
			}
		case "content_block_delta":
			if ev.Delta == nil {
				return true
			}
			switch ev.Delta.Type {
			case "text_delta":
				if !providerkit.Send(ctx, out, core.LLMChunk{ContentDelta: ev.Delta.Text}) {
					return false
				}
			case "thinking_delta":
				if !providerkit.Send(ctx, out, core.LLMChunk{ReasoningDelta: ev.Delta.Thinking}) {
					return false
				}
			case "input_json_delta":
				if !providerkit.EmitToolProgress(ctx, out, acc, ev.Index, "", "", ev.Delta.PartialJSON) {
					return false
				}
			}
		case "message_delta":
			if ev.Usage != nil {
				usage.CompletionTokens = ev.Usage.OutputTokens
			}
		}
		return true
	})
	if err != nil {
		providerkit.Send(ctx, out, core.LLMChunk{Done: true, Err: fmt.Errorf("anthropic: stream read: %w", err)})
		return
	}
	usage.TotalTokens = usage.PromptTokens + usage.CompletionTokens
	final := core.LLMChunk{Done: true, Usage: &usage}
	final.ToolCalls = acc.Calls()
	providerkit.Send(ctx, out, final)
}
