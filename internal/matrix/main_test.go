package matrix

import (
	"context"
	"fmt"
	"os"
	"testing"

	"github.com/unarbos/arbos/internal/matrix/hs"
)

// testHS is a single embedded homeserver shared by every test in this package.
// It must be a singleton: Dendrite registers process-global Prometheus
// collectors on startup, so a second hs.Start in the same test binary panics
// with "duplicate metrics collector registration". One homeserver, many
// sessions/rooms — each test scopes itself with its own session ids and clients.
var testHS *hs.Homeserver

func TestMain(m *testing.M) {
	dir, err := os.MkdirTemp("", "arbos-matrix-test-hs")
	if err != nil {
		fmt.Fprintln(os.Stderr, "matrix test: temp dir:", err)
		os.Exit(1)
	}
	h, err := hs.Start(context.Background(), dir, "localhost", "127.0.0.1:18101")
	if err != nil {
		_ = os.RemoveAll(dir)
		fmt.Fprintln(os.Stderr, "matrix test: hs.Start:", err)
		os.Exit(1)
	}
	testHS = h
	code := m.Run()
	h.Shutdown()
	_ = os.RemoveAll(dir)
	os.Exit(code)
}
