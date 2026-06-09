package secret

import (
	"context"
	"fmt"
	"os"

	"github.com/unarbos/arbos/internal/core"
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
