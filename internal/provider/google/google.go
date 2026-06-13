// Package google is a native ports.LLMProvider for Google's Gemini
// generateContent streaming API. It speaks the streamGenerateContent SSE shape:
// contents with text / functionCall / functionResponse parts, a systemInstruction,
// and usageMetadata. Auth goes through the same secret.Broker seam (configured
// with an x-goog-api-key header injector by the host).
//
// Faithful to the provider API pi targets. Documented gaps (no live key here):
// image parts and OAuth/Vertex auth flows are deferred; the broker abstracts
// credential injection so those are credential-acquisition concerns, not adapter
// changes.
package google

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

const defaultBaseURL = "https://generativelanguage.googleapis.com"

// Authorizer, Option, and the With* options are the shared providerkit ones,
// re-exported so callers keep using google.WithAuth etc.
type (
	Authorizer = providerkit.Authorizer
	Option     = providerkit.Option
)

var (
	WithName       = providerkit.WithName
	WithHTTPClient = providerkit.WithHTTPClient
	WithAuth       = providerkit.WithAuth
)

// Provider implements ports.LLMProvider against the Gemini generateContent API.
type Provider struct {
	base    providerkit.Base
	baseURL string
}

var _ ports.LLMProvider = (*Provider)(nil)

func New(baseURL string, opts ...Option) *Provider {
	if baseURL == "" {
		baseURL = defaultBaseURL
	}
	return &Provider{
		base:    providerkit.NewBase("google", ports.Capabilities{Tools: true, Reasoning: true, Vision: true}, opts...),
		baseURL: strings.TrimRight(baseURL, "/"),
	}
}

func (p *Provider) Name() string                     { return p.base.Name }
func (p *Provider) Capabilities() ports.Capabilities { return p.base.Caps }

func (p *Provider) Stream(ctx context.Context, req core.LLMRequest) (<-chan core.LLMChunk, error) {
	body, err := json.Marshal(p.buildRequest(req))
	if err != nil {
		return nil, fmt.Errorf("google: marshal request: %w", err)
	}
	url := fmt.Sprintf("%s/v1beta/models/%s:streamGenerateContent?alt=sse", p.baseURL, req.Model)
	httpReq, err := http.NewRequestWithContext(ctx, http.MethodPost, url, bytes.NewReader(body))
	if err != nil {
		return nil, fmt.Errorf("google: build request: %w", err)
	}
	httpReq.Header.Set("Content-Type", "application/json")
	httpReq.Header.Set("Accept", "text/event-stream")
	if err := p.base.Authorize(ctx, httpReq); err != nil {
		return nil, fmt.Errorf("google: authorize: %w", err)
	}
	resp, err := p.base.HTTPClient.Do(httpReq)
	if err != nil {
		return nil, providerkit.TransportError("google", err)
	}
	if resp.StatusCode != http.StatusOK {
		defer func() { _ = resp.Body.Close() }()
		return nil, providerkit.NonOKError("google", resp)
	}
	out := make(chan core.LLMChunk, 16)
	go p.consume(ctx, resp, out)
	return out, nil
}

// consume parses the streamGenerateContent SSE: each data line is a full
// GenerateContentResponse. Text parts stream as ContentDelta; functionCall parts
// (sent whole by Gemini) accumulate into tool calls; usageMetadata maps to Usage.
func (p *Provider) consume(ctx context.Context, resp *http.Response, out chan<- core.LLMChunk) {
	defer close(out)
	defer func() { _ = resp.Body.Close() }()

	acc := providerkit.NewToolAccumulator()
	var usage core.Usage
	n := 0

	err := providerkit.ScanSSE(ctx, resp.Body, func(data []byte) bool {
		var ev wireResponse
		if json.Unmarshal(data, &ev) != nil {
			return true
		}
		if ev.UsageMetadata != nil {
			usage = core.Usage{
				PromptTokens:     ev.UsageMetadata.PromptTokenCount,
				CompletionTokens: ev.UsageMetadata.CandidatesTokenCount,
				TotalTokens:      ev.UsageMetadata.TotalTokenCount,
			}
		}
		for _, cand := range ev.Candidates {
			for _, part := range cand.Content.Parts {
				if part.Text != "" && !providerkit.Send(ctx, out, core.LLMChunk{ContentDelta: part.Text}) {
					return false
				}
				if part.FunctionCall != nil {
					// Gemini sends each functionCall whole; index by encounter
					// order and synthesize a stable id (it provides none). Empty
					// args are normalized by the accumulator.
					n++
					acc.Add(n, fmt.Sprintf("call_%s_%d", part.FunctionCall.Name, n), part.FunctionCall.Name, string(part.FunctionCall.Args))
				}
			}
		}
		return true
	})
	if err != nil {
		providerkit.Send(ctx, out, core.LLMChunk{Done: true, Err: fmt.Errorf("google: stream read: %w", err)})
		return
	}
	final := core.LLMChunk{Done: true}
	if usage.TotalTokens != 0 || usage.PromptTokens != 0 {
		final.Usage = &usage
	}
	final.ToolCalls = acc.Calls()
	providerkit.Send(ctx, out, final)
}
