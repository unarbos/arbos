package engine_test

import (
	"context"
	"errors"
	"testing"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/fake"
	"github.com/unarbos/arbos/internal/ports"
)

// errProvider fails every Stream call. The engine retries a transient failure
// (the in-turn resilience loop), then ends the turn with a RETRYABLE provider
// ErrorEvent once the chain is exhausted. It exists to prove Drive returns on
// that terminal error rather than blocking forever once the turn ends.
type errProvider struct{}

func (errProvider) Name() string                     { return "err" }
func (errProvider) Capabilities() ports.Capabilities { return ports.Capabilities{} }
func (errProvider) Stream(context.Context, core.LLMRequest) (<-chan core.LLMChunk, error) {
	return nil, errors.New("rate limited")
}

func TestDrive_ReturnsOnTurnComplete(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	eng := engine.New(fake.Provider{}, fake.Tools{}, fake.NewStore(), fake.NewClock(),
		engine.Config{Model: "fake", MaxIterations: 5})
	conv, err := eng.StartSession(ctx, "drive-ok")
	if err != nil {
		t.Fatal(err)
	}
	conv.Send(core.PromptIntent{Text: "hello"})
	tc, err := engine.Drive(ctx, conv, nil)
	if err != nil {
		t.Fatalf("Drive returned error on a normal turn: %v", err)
	}
	if tc.StopReason == "" {
		t.Fatalf("Drive returned an empty TurnComplete: %+v", tc)
	}
}

// TestDrive_ReturnsOnRetryableError is the regression guard: once a turn ends
// on a retryable provider error (after the in-turn retry chain is exhausted),
// Drive must return that error promptly, not block until ctx cancels (the hang
// the unification briefly introduced). A sub-millisecond backoff keeps the
// retries from making the test slow.
func TestDrive_ReturnsOnRetryableError(t *testing.T) {
	// A short timeout that is still far longer than a prompt return: if Drive
	// hangs, the test fails by deadline; if it returns, it returns fast.
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	eng := engine.New(errProvider{}, fake.Tools{}, fake.NewStore(), fake.NewClock(),
		engine.Config{Model: "err", MaxIterations: 5,
			Retry: engine.RetryPolicy{MaxAttempts: 2, BaseBackoff: time.Microsecond, MaxBackoff: time.Microsecond}})
	conv, err := eng.StartSession(ctx, "drive-err")
	if err != nil {
		t.Fatal(err)
	}
	conv.Send(core.PromptIntent{Text: "hello"})

	done := make(chan error, 1)
	go func() {
		_, derr := engine.Drive(ctx, conv, nil)
		done <- derr
	}()
	select {
	case derr := <-done:
		if derr == nil {
			t.Fatal("Drive returned nil on a failed turn; want the provider error")
		}
		if ctx.Err() != nil {
			t.Fatal("Drive only returned because ctx expired — it hung on the retryable error")
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Drive hung on a retryable error instead of returning")
	}
}
