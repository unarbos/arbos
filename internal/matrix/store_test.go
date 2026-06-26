package matrix

import (
	"testing"

	"github.com/unarbos/arbos/internal/fake"
	"github.com/unarbos/arbos/internal/ports"
	"github.com/unarbos/arbos/internal/ports/porttest"
)

// TestStoreContract proves matrix.Store is a faithful ports.SessionStore: in
// local-only mode (nil mirror — the offline/solo path) it must pass the same
// adapter-agnostic contract every store passes. The Matrix mirror is a
// side-effect that never changes the authoritative local result, so the
// contract holds with or without it; the mirror itself is exercised in
// store_mirror_test.go against a live embedded homeserver.
func TestStoreContract(t *testing.T) {
	porttest.RunSessionStoreContract(t, func() ports.SessionStore {
		return NewStore(fake.NewStore(), nil)
	})
}
