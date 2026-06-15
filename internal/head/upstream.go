package head

import (
	"bytes"
	"context"
	"net"
	"net/http"
	"strings"
	"time"

	"golang.org/x/net/http2"
)

// pooledTransport builds the shared HTTP transport for upstream calls. The
// gateway sends every request to a single host (OpenRouter, behind Cloudflare),
// which speaks HTTP/2 — so one persistent connection already multiplexes all
// concurrent completions and the TLS handshake is amortized, not per-request.
// The explicit transport pins that intent and adds two things the defaults lack:
// a generous per-host idle pool (a no-op under h2, but it keeps a future
// HTTP/1.1 upstream — Chutes, a direct provider — from churning TLS), and h2
// health pings so a dead idle connection (Cloudflare reaps them) is detected and
// replaced instead of carrying the next request into a sporadic failure.
//
// No client-level timeout is set by callers on the streaming path: a long stream
// must not be cut mid-flight; the request context bounds the call. The dial and
// TLS-handshake timeouts here only bound connection setup, never an open stream.
func pooledTransport() *http.Transport {
	t := &http.Transport{
		Proxy:                 http.ProxyFromEnvironment,
		DialContext:           (&net.Dialer{Timeout: 10 * time.Second, KeepAlive: 30 * time.Second}).DialContext,
		ForceAttemptHTTP2:     true,
		MaxIdleConns:          256,
		MaxIdleConnsPerHost:   256,
		IdleConnTimeout:       90 * time.Second,
		TLSHandshakeTimeout:   10 * time.Second,
		ExpectContinueTimeout: 1 * time.Second,
	}
	// ConfigureTransports wires HTTP/2 onto t and hands back the h2 transport so
	// we can set keepalive pings; on error we simply fall back to t's built-in
	// h2 (without pings), which is still correct.
	if h2t, err := http2.ConfigureTransports(t); err == nil && h2t != nil {
		h2t.ReadIdleTimeout = 30 * time.Second
		h2t.PingTimeout = 15 * time.Second
	}
	return t
}

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
		client: &http.Client{Transport: pooledTransport()},
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
