package fake

import (
	"context"
	"fmt"
	"os"

	"github.com/unarbos/arbos/internal/core"
)

// EnvSecretProvider resolves secrets from environment variables. DEV ONLY and
// explicitly insecure: env is visible to child processes and via /proc, which
// is exactly the exfiltration surface we avoid in production (keychain /
// 1Password / Doppler / age-file providers). Its Name advertises the risk.
type EnvSecretProvider struct{}

func (EnvSecretProvider) Name() string { return "env(dev-insecure)" }

func (EnvSecretProvider) Resolve(ctx context.Context, ref core.SecretRef) (core.SecretValue, error) {
	v, ok := os.LookupEnv(ref.Name)
	if !ok {
		return core.SecretValue{}, fmt.Errorf("env secret %q not set", ref.Name)
	}
	return core.NewSecretValue([]byte(v)), nil
}

// MapSecretProvider is a deterministic in-memory provider for tests. It also
// counts Resolve calls so tests can assert that a refused binding never reaches
// the provider (i.e. the value is never even fetched).
type MapSecretProvider struct {
	Values map[string]string
	Calls  int
}

func (m *MapSecretProvider) Name() string { return "map(test)" }

func (m *MapSecretProvider) Resolve(ctx context.Context, ref core.SecretRef) (core.SecretValue, error) {
	m.Calls++
	v, ok := m.Values[ref.Name]
	if !ok {
		return core.SecretValue{}, fmt.Errorf("secret %q not found", ref.Name)
	}
	return core.NewSecretValue([]byte(v)), nil
}
