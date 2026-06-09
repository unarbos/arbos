package secret_test

import (
	"context"
	"net/http"
	"testing"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/fake"
	"github.com/unarbos/arbos/internal/secret"
)

func newBroker(p *fake.MapSecretProvider) *secret.Broker {
	return secret.NewBroker(p, secret.Binding{
		Ref:    core.SecretRef{Name: "openai_api_key"},
		Hosts:  []string{"api.openai.com"},
		Inject: secret.BearerInjector,
	})
}

func TestBrokerInjectsForAllowedHost(t *testing.T) {
	p := &fake.MapSecretProvider{Values: map[string]string{"openai_api_key": "sk-live"}}
	br := newBroker(p)

	req, _ := http.NewRequest(http.MethodPost, "https://api.openai.com/v1/responses", nil)
	if err := br.Apply(context.Background(), core.SecretRef{Name: "openai_api_key"}, req); err != nil {
		t.Fatalf("Apply: %v", err)
	}
	if got := req.Header.Get("Authorization"); got != "Bearer sk-live" {
		t.Fatalf("Authorization = %q, want Bearer sk-live", got)
	}
}

// The anti-exfiltration guarantee: a request to a non-allowlisted host is
// refused AND the secret is never even resolved (provider.Resolve not called),
// so the value never enters memory. This is the prompt-injection defense.
func TestBrokerRefusesDisallowedHostWithoutResolving(t *testing.T) {
	p := &fake.MapSecretProvider{Values: map[string]string{"openai_api_key": "sk-live"}}
	br := newBroker(p)

	req, _ := http.NewRequest(http.MethodPost, "https://evil.example.com/steal", nil)
	err := br.Apply(context.Background(), core.SecretRef{Name: "openai_api_key"}, req)
	if err == nil {
		t.Fatal("expected refusal for disallowed host")
	}
	if req.Header.Get("Authorization") != "" {
		t.Fatal("no credential should be attached to a disallowed host")
	}
	if p.Calls != 0 {
		t.Fatalf("secret was resolved %d times for a disallowed host; must be 0", p.Calls)
	}
}

func TestBrokerRefusesUnboundSecret(t *testing.T) {
	p := &fake.MapSecretProvider{Values: map[string]string{"other": "x"}}
	br := newBroker(p)

	req, _ := http.NewRequest(http.MethodGet, "https://api.openai.com/", nil)
	if err := br.Apply(context.Background(), core.SecretRef{Name: "other"}, req); err == nil {
		t.Fatal("expected refusal for a secret with no binding")
	}
	if p.Calls != 0 {
		t.Fatalf("unbound secret was resolved %d times; must be 0", p.Calls)
	}
}
