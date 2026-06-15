// Package secret turns SecretRefs into outbound credentials at the trusted
// boundary and binds each secret to the destination hosts it may reach. This is
// the anti-exfiltration control: a prompt-injected agent cannot send a key to an
// arbitrary host, because the broker refuses to resolve a secret for a host that
// is not in its binding — and the agent never holds the value in the first place
// (it only ever sees SecretRefs). See ADR-0016.
package secret

import (
	"context"
	"fmt"
	"net/http"
	"strings"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/ports"
)

// Injector applies a resolved secret to an outbound request. The raw value is
// only ever seen here, at the boundary, and only for the duration of the call.
type Injector func(req *http.Request, value []byte)

// BearerInjector sets "Authorization: Bearer <value>".
func BearerInjector(req *http.Request, value []byte) {
	req.Header.Set("Authorization", "Bearer "+string(value))
}

// HeaderInjector sets an arbitrary header to the raw value (e.g. "x-api-key").
func HeaderInjector(header string) Injector {
	return func(req *http.Request, value []byte) {
		req.Header.Set(header, string(value))
	}
}

// SchemeInjector sets header to prefix+value — the general form HeaderInjector
// (empty prefix) and BearerInjector (Authorization, "Bearer ") specialize. It
// lets a stored secret carry its own auth dialect (e.g. "Token <value>" or a
// "prefix.secret" key sent raw) without the storage layer hand-rolling the
// header write.
func SchemeInjector(header, prefix string) Injector {
	return func(req *http.Request, value []byte) {
		req.Header.Set(header, prefix+string(value))
	}
}

// Binding ties a secret to the hosts it may be sent to and how it is applied.
// Hosts are matched exactly (case-insensitive) against the request hostname.
type Binding struct {
	Ref    core.SecretRef
	Hosts  []string
	Inject Injector
}

// Broker is the single component that touches raw secret material at the network
// boundary. It resolves a ref via the provider only after the destination host
// passes the binding allowlist.
type Broker struct {
	provider ports.SecretProvider
	bindings map[string]Binding
}

func NewBroker(p ports.SecretProvider, bindings ...Binding) *Broker {
	m := make(map[string]Binding, len(bindings))
	for _, b := range bindings {
		m[b.Ref.Name] = b
	}
	return &Broker{provider: p, bindings: m}
}

// Apply attaches the secret named by ref to req — but only if req's host is in
// the binding's allowlist. On a missing binding or host mismatch it refuses and
// the secret is NOT resolved (the value never enters memory). On success the
// value is exposed to the injector and immediately destroyed.
func (br *Broker) Apply(ctx context.Context, ref core.SecretRef, req *http.Request) error {
	b, ok := br.bindings[ref.Name]
	if !ok {
		return fmt.Errorf("secret %q has no binding; refusing to attach", ref.Name)
	}
	host := req.URL.Hostname()
	if !hostAllowed(host, b.Hosts) {
		return fmt.Errorf("secret %q not permitted for host %q (allowed: %v)", ref.Name, host, b.Hosts)
	}
	val, err := br.provider.Resolve(ctx, ref)
	if err != nil {
		return fmt.Errorf("resolve secret %q: %w", ref.Name, err)
	}
	defer val.Destroy()
	b.Inject(req, val.Expose())
	return nil
}

func hostAllowed(host string, allowed []string) bool {
	for _, h := range allowed {
		if strings.EqualFold(host, h) {
			return true
		}
	}
	return false
}
