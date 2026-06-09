package openai_test

import (
	"context"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/fake"
	"github.com/unarbos/arbos/internal/ports"
	"github.com/unarbos/arbos/internal/ports/porttest"
	"github.com/unarbos/arbos/internal/provider/openai"
	"github.com/unarbos/arbos/internal/secret"
)

// sseServer returns an httptest server that streams the given SSE lines and
// records the last Authorization header it saw.
func sseServer(t *testing.T, lines []string, gotAuth *string) *httptest.Server {
	t.Helper()
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if gotAuth != nil {
			*gotAuth = r.Header.Get("Authorization")
		}
		w.Header().Set("Content-Type", "text/event-stream")
		fl, _ := w.(http.Flusher)
		for _, l := range lines {
			_, _ = fmt.Fprintf(w, "data: %s\n\n", l)
			if fl != nil {
				fl.Flush()
			}
		}
	}))
	t.Cleanup(srv.Close)
	return srv
}

func collect(t *testing.T, ch <-chan core.LLMChunk) (text string, calls []core.ToolCall, usage *core.Usage) {
	t.Helper()
	var b strings.Builder
	deadline := time.After(3 * time.Second)
	for {
		select {
		case ck, ok := <-ch:
			if !ok {
				return b.String(), calls, usage
			}
			b.WriteString(ck.ContentDelta)
			calls = append(calls, ck.ToolCalls...)
			if ck.Usage != nil {
				usage = ck.Usage
			}
		case <-deadline:
			t.Fatal("provider did not close channel in time")
		}
	}
}

func TestStreamsContentAndUsage(t *testing.T) {
	lines := []string{
		`{"choices":[{"delta":{"content":"Hel"}}]}`,
		`{"choices":[{"delta":{"content":"lo"}}]}`,
		`{"choices":[{"delta":{}}],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}`,
		`[DONE]`,
	}
	srv := sseServer(t, lines, nil)
	p := openai.New(srv.URL)

	ch, err := p.Stream(context.Background(), core.LLMRequest{Model: "m", Stream: true, Messages: []core.Message{{Role: core.RoleUser, Content: "hi"}}})
	if err != nil {
		t.Fatal(err)
	}
	text, calls, usage := collect(t, ch)
	if text != "Hello" {
		t.Fatalf("content: got %q", text)
	}
	if len(calls) != 0 {
		t.Fatalf("unexpected tool calls: %+v", calls)
	}
	if usage == nil || usage.TotalTokens != 5 {
		t.Fatalf("usage: %+v", usage)
	}
}

// TestAccumulatesToolCallFragments is the core SSE-specific behavior the Python
// SDK gave us for free: tool calls arrive split across chunks and must be
// reassembled, in order, before reaching the engine.
func TestAccumulatesToolCallFragments(t *testing.T) {
	// OpenAI sends the function name whole in the first fragment; only the
	// arguments string is chunked across deltas.
	lines := []string{
		`{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"get_weather","arguments":"{\"q\""}}]}}]}`,
		`{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":":\"sf\"}"}}]}}]}`,
		`[DONE]`,
	}
	srv := sseServer(t, lines, nil)
	p := openai.New(srv.URL)

	ch, err := p.Stream(context.Background(), core.LLMRequest{Model: "m", Stream: true})
	if err != nil {
		t.Fatal(err)
	}
	_, calls, _ := collect(t, ch)
	if len(calls) != 1 {
		t.Fatalf("want 1 reassembled call, got %d (%+v)", len(calls), calls)
	}
	if calls[0].ID != "call_1" || calls[0].Name != "get_weather" {
		t.Fatalf("fragments not joined: %+v", calls[0])
	}
	if string(calls[0].Args) != `{"q":"sf"}` {
		t.Fatalf("argument fragments not joined: %s", calls[0].Args)
	}
}

// TestBrokerAuthorizesAllowedHost proves the secret-broker wiring: the key is
// attached on the allowed host and never held by the provider.
func TestBrokerAuthorizesAllowedHost(t *testing.T) {
	var gotAuth string
	srv := sseServer(t, []string{`{"choices":[{"delta":{"content":"ok"}}]}`, `[DONE]`}, &gotAuth)

	host := mustHost(t, srv.URL)
	sp := &fake.MapSecretProvider{Values: map[string]string{"openai_key": "sk-test-123"}}
	broker := secret.NewBroker(sp, secret.Binding{
		Ref:    core.SecretRef{Name: "openai_key"},
		Hosts:  []string{host},
		Inject: secret.BearerInjector,
	})
	p := openai.New(srv.URL, openai.WithAuth(broker, core.SecretRef{Name: "openai_key"}))

	ch, err := p.Stream(context.Background(), core.LLMRequest{Model: "m", Stream: true})
	if err != nil {
		t.Fatal(err)
	}
	collect(t, ch)
	if gotAuth != "Bearer sk-test-123" {
		t.Fatalf("broker did not attach the key: %q", gotAuth)
	}
}

// TestBrokerRefusesDisallowedHost proves anti-exfiltration: a base_url whose
// host is not in the binding gets no key and the request is refused before it
// leaves.
func TestBrokerRefusesDisallowedHost(t *testing.T) {
	srv := sseServer(t, []string{`[DONE]`}, nil)
	sp := &fake.MapSecretProvider{Values: map[string]string{"openai_key": "sk-secret"}}
	broker := secret.NewBroker(sp, secret.Binding{
		Ref:    core.SecretRef{Name: "openai_key"},
		Hosts:  []string{"api.openai.com"}, // NOT the test server's host
		Inject: secret.BearerInjector,
	})
	p := openai.New(srv.URL, openai.WithAuth(broker, core.SecretRef{Name: "openai_key"}))

	_, err := p.Stream(context.Background(), core.LLMRequest{Model: "m", Stream: true})
	if err == nil {
		t.Fatal("expected Stream to refuse: host not on allowlist")
	}
	if sp.Calls != 0 {
		t.Fatalf("secret was resolved despite host mismatch (calls=%d)", sp.Calls)
	}
}

// TestProviderContract runs the shared port contract against a provider pointed
// at a canned SSE server.
func TestProviderContract(t *testing.T) {
	porttest.RunLLMProviderContract(t, func() ports.LLMProvider {
		srv := sseServer(t, []string{`{"choices":[{"delta":{"content":"hi"}}]}`, `[DONE]`}, nil)
		return openai.New(srv.URL)
	})
}

// TestTruncatedStreamSurfacesError proves a stream that doesn't end cleanly (an
// over-long line blows the scanner buffer) is reported as an error chunk, not a
// silent clean completion.
func TestTruncatedStreamSurfacesError(t *testing.T) {
	huge := strings.Repeat("x", 5*1024*1024) // exceeds the 4 MiB scanner buffer
	srv := sseServer(t, []string{`{"choices":[{"delta":{"content":"` + huge + `"}}]}`}, nil)
	p := openai.New(srv.URL)

	ch, err := p.Stream(context.Background(), core.LLMRequest{Model: "m", Stream: true})
	if err != nil {
		t.Fatal(err)
	}
	var sawErr bool
	deadline := time.After(3 * time.Second)
	for {
		select {
		case c, ok := <-ch:
			if !ok {
				if !sawErr {
					t.Fatal("truncated stream did not surface an error chunk")
				}
				return
			}
			if c.Err != nil {
				sawErr = true
			}
		case <-deadline:
			t.Fatal("channel never closed")
		}
	}
}

// TestRedirectIsRefused proves the default client does not follow a redirect
// (which would leak a custom-header secret to the redirect target); a 3xx
// becomes a non-200 error.
func TestRedirectIsRefused(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.Redirect(w, r, "https://evil.example.com/v1/chat/completions", http.StatusFound)
	}))
	t.Cleanup(srv.Close)

	p := openai.New(srv.URL)
	if _, err := p.Stream(context.Background(), core.LLMRequest{Model: "m", Stream: true}); err == nil {
		t.Fatal("expected a redirect to be refused as a non-200 status")
	}
}

func mustHost(t *testing.T, raw string) string {
	t.Helper()
	// httptest URLs look like http://127.0.0.1:PORT
	rest := strings.TrimPrefix(raw, "http://")
	if i := strings.IndexByte(rest, ':'); i >= 0 {
		return rest[:i]
	}
	return rest
}
