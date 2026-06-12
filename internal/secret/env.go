package secret

import (
	"context"
	"fmt"
	"os"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/ports"
)

// EnvProvider resolves secrets from environment variables. It is the default
// production backend for a local single-user host, with a known caveat: env is
// visible to child processes and via /proc, a surface that keychain / Doppler /
// age-file providers close. Its Name advertises the tradeoff so logs show
// which backend resolved a credential.
type EnvProvider struct{}

func (EnvProvider) Name() string { return "env" }

func (EnvProvider) Resolve(_ context.Context, ref core.SecretRef) (core.SecretValue, error) {
	v, ok := os.LookupEnv(ref.Name)
	if !ok {
		return core.SecretValue{}, fmt.Errorf("env secret %q not set", ref.Name)
	}
	return core.NewSecretValue([]byte(v)), nil
}

// Chain resolves from the first provider that succeeds, in order. It is the
// env-then-vault composition the host's own credential uses: an exported
// OPENROUTER_API_KEY keeps working, and a key pasted into the Settings tab
// (vault) backs it without either side knowing about the other. Nil entries
// are skipped so a host without a vault chains cleanly.
type Chain []ports.SecretProvider

func (c Chain) Name() string { return "chain" }

func (c Chain) Resolve(ctx context.Context, ref core.SecretRef) (core.SecretValue, error) {
	var lastErr error
	for _, p := range c {
		if p == nil {
			continue
		}
		v, err := p.Resolve(ctx, ref)
		if err == nil {
			return v, nil
		}
		lastErr = err
	}
	if lastErr == nil {
		lastErr = fmt.Errorf("secret %q: no providers in chain", ref.Name)
	}
	return core.SecretValue{}, lastErr
}
