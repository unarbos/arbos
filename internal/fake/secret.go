package fake

import (
	"context"
	"fmt"

	"github.com/unarbos/arbos/internal/core"
)

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
