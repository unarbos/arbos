package codingspec

import (
	"context"
	"net/http"
	"sync"
)

// Managed-secret injection (ADR-0016): a host can register a source of extra
// environment entries — the user's labeled, vault-stored secrets flagged for
// tool use — that every bash job inherits at spawn. It is a process-wide hook
// rather than a threaded option because the source is the same for the
// top-level session and every delegated child, and the job table it feeds is
// already a process-shared resource (see job.go). The value never enters the
// arbos parent environment: it is resolved from the vault and handed only to
// the specific child being spawned.
//
// The secret applier below is the same hook for the HTTP boundary: fetch
// passes a secret *name* and the outbound request, and the host's broker
// decides whether the destination host is allowed before the value ever
// leaves the vault. codingspec deliberately sees only this function shape,
// not the secret package — the binding policy belongs to the host.
var (
	envMu     sync.RWMutex
	envSource func() []string
	applier   func(ctx context.Context, name string, req *http.Request) error
)

// SetEnvSource registers the source of extra "KEY=value" entries injected into
// every bash job's environment. Called once at host assembly; a nil fn (the
// default) injects nothing, so a host that wires no vault behaves exactly as
// before.
func SetEnvSource(fn func() []string) {
	envMu.Lock()
	defer envMu.Unlock()
	envSource = fn
}

// extraEnv returns the currently registered injection entries, or nil.
func extraEnv() []string {
	envMu.RLock()
	fn := envSource
	envMu.RUnlock()
	if fn == nil {
		return nil
	}
	return fn()
}

// SetSecretApplier registers the host's broker for the fetch tool: given a
// managed secret's name and an outbound request, attach the value iff the
// destination host is on that secret's allowlist. Called once at host
// assembly, alongside SetEnvSource.
func SetSecretApplier(fn func(ctx context.Context, name string, req *http.Request) error) {
	envMu.Lock()
	defer envMu.Unlock()
	applier = fn
}

// applySecret attaches the named secret to req via the registered applier.
func applySecret(ctx context.Context, name string, req *http.Request) error {
	envMu.RLock()
	fn := applier
	envMu.RUnlock()
	if fn == nil {
		return errNoVault
	}
	return fn(ctx, name, req)
}
