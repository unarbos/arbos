package engine_test

import (
	"context"
	"errors"
	"sync"
	"testing"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/fake"
	"github.com/unarbos/arbos/internal/ports"
)

// flakyProvider is a configurable ports.LLMProvider for exercising the engine's
// in-turn retry and model-fallback loop. It records the model of every Stream
// call (so a test can assert the retry/fallback order) and fails on a policy:
// a per-model failure set, a countdown of leading failures, or a permanent
// (non-retryable) error. A successful call streams a one-token answer.
type flakyProvider struct {
	mu sync.Mutex
	// calls is the model of every Stream invocation, in order.
	calls []string
	// failModels, when non-nil, fails every call whose model is in it.
	failModels map[string]bool
	// failsLeft fails the first N calls regardless of model, then succeeds.
	failsLeft int
	// permanent makes the failures non-retryable (a 4xx), so the loop must
	// stop without retrying or falling back.
	permanent bool
}

func (p *flakyProvider) Name() string                     { return "flaky" }
func (p *flakyProvider) Capabilities() ports.Capabilities { return ports.Capabilities{Tools: true} }

func (p *flakyProvider) Stream(ctx context.Context, req core.LLMRequest) (<-chan core.LLMChunk, error) {
	p.mu.Lock()
	p.calls = append(p.calls, req.Model)
	fail := false
	if p.failsLeft > 0 {
		fail = true
		p.failsLeft--
	}
	if p.failModels[req.Model] {
		fail = true
	}
	p.mu.Unlock()

	if fail {
		status := 503
		if p.permanent {
			status = 400
		}
		return nil, &ports.ProviderError{StatusCode: status, Err: errors.New("flaky provider failure")}
	}
	out := make(chan core.LLMChunk, 4)
	go func() {
		defer close(out)
		out <- core.LLMChunk{ContentDelta: "ok"}
		out <- core.LLMChunk{Done: true, Usage: &core.Usage{TotalTokens: 1}}
	}()
	return out, nil
}

func (p *flakyProvider) modelCalls() []string {
	p.mu.Lock()
	defer p.mu.Unlock()
	return append([]string(nil), p.calls...)
}

// fastRetry keeps the backoff sub-millisecond so the retry tests don't sleep.
func fastRetry(attempts int) engine.RetryPolicy {
	return engine.RetryPolicy{MaxAttempts: attempts, BaseBackoff: time.Microsecond, MaxBackoff: time.Microsecond}
}

func retryEngine(p ports.LLMProvider, cfg engine.Config) *engine.Engine {
	cfg.Model = "primary"
	cfg.MaxIterations = 4
	return engine.New(p, fake.Tools{}, fake.NewStore(), fake.NewClock(), cfg)
}

func runTurn(t *testing.T, eng *engine.Engine, id core.SessionID) transcript {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	conv, err := eng.StartSession(ctx, id)
	if err != nil {
		t.Fatal(err)
	}
	conv.Send(core.PromptIntent{Text: "hello"})
	return drain(t, conv)
}

func lastKind(tr transcript) string {
	if len(tr.kinds) == 0 {
		return ""
	}
	return tr.kinds[len(tr.kinds)-1]
}

// A transient failure that clears within the per-model attempt budget is
// retried on the SAME model and the turn completes — the ephemeral-break case.
func TestRetrySucceedsAfterTransientFailures(t *testing.T) {
	p := &flakyProvider{failsLeft: 2}
	eng := retryEngine(p, engine.Config{Retry: fastRetry(3)})
	tr := runTurn(t, eng, "s-retry")

	if lastKind(tr) != "turn_complete" {
		t.Fatalf("expected turn_complete after retries, got kinds %v", tr.kinds)
	}
	if got := p.modelCalls(); len(got) != 3 {
		t.Fatalf("expected 3 calls (2 fail + 1 success) on primary, got %v", got)
	}
}

// When a model exhausts its retries, the turn falls back to the next model and
// completes there — the model-outage case.
func TestRetryFallsBackToNextModel(t *testing.T) {
	p := &flakyProvider{failModels: map[string]bool{"primary": true}}
	eng := retryEngine(p, engine.Config{Retry: fastRetry(2), FallbackModels: []string{"backup"}})
	tr := runTurn(t, eng, "s-fallback")

	if lastKind(tr) != "turn_complete" {
		t.Fatalf("expected turn_complete on fallback model, got kinds %v", tr.kinds)
	}
	got := p.modelCalls()
	want := []string{"primary", "primary", "backup"}
	if len(got) != len(want) {
		t.Fatalf("expected calls %v, got %v", want, got)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Fatalf("call %d: expected %q, got %q (all: %v)", i, want[i], got[i], got)
		}
	}
}

// A permanent (non-retryable) failure stops at once: no retry, no fallback —
// retrying a bad request just burns the turn's deadline.
func TestPermanentErrorDoesNotRetryOrFallBack(t *testing.T) {
	p := &flakyProvider{failModels: map[string]bool{"primary": true}, permanent: true}
	eng := retryEngine(p, engine.Config{Retry: fastRetry(3), FallbackModels: []string{"backup"}})
	tr := runTurn(t, eng, "s-permanent")

	if lastKind(tr) != "error" {
		t.Fatalf("expected error on permanent failure, got kinds %v", tr.kinds)
	}
	if got := p.modelCalls(); len(got) != 1 || got[0] != "primary" {
		t.Fatalf("expected a single primary call, got %v", got)
	}
}

// When every model and attempt is exhausted, the turn emits the terminal
// provider error — and only after the whole chain was tried.
func TestRetryExhaustionEmitsTerminalError(t *testing.T) {
	p := &flakyProvider{failModels: map[string]bool{"primary": true, "backup": true}}
	eng := retryEngine(p, engine.Config{Retry: fastRetry(2), FallbackModels: []string{"backup"}})
	tr := runTurn(t, eng, "s-exhausted")

	if lastKind(tr) != "error" {
		t.Fatalf("expected terminal error after exhaustion, got kinds %v", tr.kinds)
	}
	if got := p.modelCalls(); len(got) != 4 {
		t.Fatalf("expected 4 calls (2 per model x 2 models), got %v", got)
	}
}
