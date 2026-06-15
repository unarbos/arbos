package head

import (
	"bytes"
	"context"
	"net/http"
	"strings"
)

// Upstream is one inference provider the gateway can forward to. Today there is
// exactly one (OpenRouter); Chutes and direct-provider upstreams satisfy the
// same shape later and a router picks among them per model. The gateway depends
// on this capability, not the concrete forwarder, and Config can inject a fake
// for tests (see Head.New).
type Upstream interface {
	// Forward issues the (already OpenAI-shaped) request body to the upstream
	// and returns the raw response for the gateway to stream and meter. stream
	// selects SSE vs a single JSON body.
	Forward(ctx context.Context, body []byte, stream bool) (*http.Response, error)
	// Models returns the upstream's raw model-catalog response (GET /models),
	// proxied to agents so they can discover what to ask for.
	Models(ctx context.Context) (*http.Response, error)
	// Name identifies the upstream in usage records.
	Name() string
}

var _ Upstream = (*openRouter)(nil)

// openRouter forwards verbatim to OpenRouter, attaching the head's own key
// server-side. The agent never sees this key: it authenticates to the head
// with its arbos key, and the head swaps in the OpenRouter credential — the
// same indirection the secret broker gives direct providers (ADR-0016).
type openRouter struct {
	baseURL string
	apiKey  string
	client  *http.Client
}

// newOpenRouter builds the forwarder. baseURL defaults to the public endpoint;
// apiKey is the head operator's OpenRouter key (sourced from the environment,
// e.g. via `doppler run`).
func newOpenRouter(baseURL, apiKey string) *openRouter {
	if baseURL == "" {
		baseURL = "https://openrouter.ai/api/v1"
	}
	return &openRouter{
		baseURL: strings.TrimRight(baseURL, "/"),
		apiKey:  apiKey,
		// No client-side timeout: a long stream must not be cut mid-flight.
		// The request context (the client connection) bounds the call.
		client: &http.Client{},
	}
}

func (o *openRouter) Name() string { return "openrouter" }

func (o *openRouter) Forward(ctx context.Context, body []byte, stream bool) (*http.Response, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, o.baseURL+"/chat/completions", bytes.NewReader(body))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Authorization", "Bearer "+o.apiKey)
	// Attribution headers OpenRouter surfaces in its dashboard; harmless if the
	// endpoint ignores them.
	req.Header.Set("HTTP-Referer", "https://arbos.life")
	req.Header.Set("X-Title", "arbos")
	if stream {
		req.Header.Set("Accept", "text/event-stream")
	}
	return o.client.Do(req)
}

// Models proxies the upstream model list (GET /models) so an agent can
// discover what it can ask for through the head. The gateway caches the body
// briefly; the list changes rarely.
func (o *openRouter) Models(ctx context.Context) (*http.Response, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, o.baseURL+"/models", nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Authorization", "Bearer "+o.apiKey)
	return o.client.Do(req)
}
