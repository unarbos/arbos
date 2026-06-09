package porttest

import (
	"context"
	"testing"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/ports"
)

// RunLLMProviderContract asserts the provider-agnostic invariants every
// LLMProvider must satisfy. It deliberately does NOT assert response *content*
// (that is provider-specific); it asserts the lifecycle guarantees the engine
// relies on: a named provider, a stream that always closes, and honored
// cancellation (so a turn interrupt can never leak a goroutine or hang).
//
// newProvider must return a usable provider. A provider that genuinely needs
// external config may return an error from Stream; the suite treats that as
// "skip the streaming checks" rather than a failure, so it stays usable both
// for the fake and for real providers in a unit context.
func RunLLMProviderContract(t *testing.T, newProvider func() ports.LLMProvider) {
	t.Helper()

	req := core.LLMRequest{
		Model:    "contract",
		Messages: []core.Message{{Role: core.RoleUser, Content: "hello"}},
		Stream:   true,
	}

	t.Run("name_non_empty", func(t *testing.T) {
		if newProvider().Name() == "" {
			t.Fatal("provider Name() must be non-empty")
		}
	})

	t.Run("stream_channel_closes", func(t *testing.T) {
		p := newProvider()
		ch, err := p.Stream(context.Background(), req)
		if err != nil {
			t.Skipf("provider not usable without config: %v", err)
		}
		drainWithin(t, ch, 3*time.Second)
	})

	t.Run("honors_cancellation", func(t *testing.T) {
		p := newProvider()
		ctx, cancel := context.WithCancel(context.Background())
		ch, err := p.Stream(ctx, req)
		if err != nil {
			cancel()
			t.Skipf("provider not usable without config: %v", err)
		}
		cancel()
		// Even cancelled mid-flight, the provider must unwind and close the
		// channel — never block forever.
		drainWithin(t, ch, 3*time.Second)
	})
}

// drainWithin reads until the channel closes, failing if that does not happen
// within d (a leaked goroutine or an ignored ctx).
func drainWithin(t *testing.T, ch <-chan core.LLMChunk, d time.Duration) {
	t.Helper()
	deadline := time.After(d)
	for {
		select {
		case _, ok := <-ch:
			if !ok {
				return
			}
		case <-deadline:
			t.Fatalf("provider channel did not close within %s (leak or ignored ctx)", d)
		}
	}
}
