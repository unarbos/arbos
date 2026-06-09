package fake_test

import (
	"testing"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/fake"
	"github.com/unarbos/arbos/internal/ports"
	"github.com/unarbos/arbos/internal/ports/porttest"
)

// These prove the reference (fake) adapters satisfy the port contracts. They
// are also the template every real adapter copies: e.g. the Phase-2 SQLite
// store gets a one-liner TestSQLiteStoreContract that calls the same suite.

func TestFakeStoreContract(t *testing.T) {
	porttest.RunSessionStoreContract(t, func() ports.SessionStore { return fake.NewStore() })
}

func TestFakeProviderContract(t *testing.T) {
	porttest.RunLLMProviderContract(t, func() ports.LLMProvider { return fake.Provider{} })
}

func TestFakeToolsContract(t *testing.T) {
	porttest.RunToolRuntimeContract(t, func() ports.ToolRuntime { return fake.Tools{} })
}

func TestFakeSecretProviderContract(t *testing.T) {
	porttest.RunSecretProviderContract(t,
		func() ports.SecretProvider {
			return &fake.MapSecretProvider{Values: map[string]string{"api_key": "s3cr3t-value"}}
		},
		core.SecretRef{Name: "api_key"}, "s3cr3t-value")
}
