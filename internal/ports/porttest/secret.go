package porttest

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"testing"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/ports"
)

// RunSecretProviderContract asserts the contract every SecretProvider must
// satisfy. newProvider must return a provider that resolves knownRef to
// wantValue; the suite also checks that an unknown ref errors and that resolved
// values never stringify or marshal their material (the leak guarantee).
func RunSecretProviderContract(t *testing.T, newProvider func() ports.SecretProvider, knownRef core.SecretRef, wantValue string) {
	t.Helper()
	ctx := context.Background()

	t.Run("name_non_empty", func(t *testing.T) {
		if newProvider().Name() == "" {
			t.Fatal("SecretProvider Name() must be non-empty")
		}
	})

	t.Run("resolves_known_ref", func(t *testing.T) {
		v, err := newProvider().Resolve(ctx, knownRef)
		if err != nil {
			t.Fatalf("Resolve(%v): %v", knownRef, err)
		}
		if string(v.Expose()) != wantValue {
			t.Fatalf("resolved value mismatch")
		}
	})

	t.Run("unknown_ref_errors", func(t *testing.T) {
		if _, err := newProvider().Resolve(ctx, core.SecretRef{Name: "__definitely_absent__"}); err == nil {
			t.Fatal("expected an error resolving an unknown secret ref")
		}
	})

	t.Run("value_never_leaks_via_string_or_json", func(t *testing.T) {
		v, err := newProvider().Resolve(ctx, knownRef)
		if err != nil {
			t.Fatal(err)
		}
		if s := fmt.Sprintf("%v", v); strings.Contains(s, wantValue) {
			t.Fatalf("Stringer leaked the secret: %q", s)
		}
		b, _ := json.Marshal(v)
		if strings.Contains(string(b), wantValue) {
			t.Fatalf("JSON marshal leaked the secret: %s", b)
		}
	})
}
