package engine_test

import (
	"context"
	"strings"
	"testing"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/fake"
	"github.com/unarbos/arbos/internal/ports"
)

// transcript is a semantic capture of a turn: the sequence of event kinds (with
// runs of deltas collapsed) plus the reconstructed assistant text and any tool
// results. It asserts behavior, not the fake's canned token count.
type transcript struct {
	kinds       []string
	finalText   string
	toolResults []string
}

func drain(t *testing.T, conv *engine.Conversation) transcript {
	t.Helper()
	var tr transcript
	var text strings.Builder
	lastDelta := false
	for env := range conv.Events() {
		switch e := env.Event.(type) {
		case core.MessageDelta:
			text.WriteString(e.Text)
			if !lastDelta {
				tr.kinds = append(tr.kinds, "delta")
				lastDelta = true
			}
			continue
		case core.ReasoningDelta:
			tr.kinds = append(tr.kinds, "reasoning")
		case core.ToolStarted:
			tr.kinds = append(tr.kinds, "tool_started")
		case core.ToolFinished:
			tr.kinds = append(tr.kinds, "tool_finished")
			tr.toolResults = append(tr.toolResults, e.Result.Content)
		case core.TurnComplete:
			tr.kinds = append(tr.kinds, "turn_complete")
			tr.finalText = e.FinalResponse
			return tr
		case core.Interrupted:
			tr.kinds = append(tr.kinds, "interrupted")
			return tr
		case core.ErrorEvent:
			tr.kinds = append(tr.kinds, "error")
			return tr
		}
		lastDelta = false
	}
	return tr
}

func newEngine(store ports.SessionStore) *engine.Engine {
	return engine.New(fake.Provider{}, fake.Tools{}, store, fake.NewClock(), engine.Config{Model: "fake", MaxIterations: 10})
}

func TestPlainTurnReconstructsFinalText(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	conv, err := newEngine(fake.NewStore()).StartSession(ctx, "s-plain")
	if err != nil {
		t.Fatal(err)
	}
	conv.Send(core.PromptIntent{Text: "hello there"})

	tr := drain(t, conv)
	if got := tr.kinds[len(tr.kinds)-1]; got != "turn_complete" {
		t.Fatalf("expected turn_complete, got %q (%v)", got, tr.kinds)
	}
	if tr.finalText != "This is a deterministic fake response." {
		t.Fatalf("reconstructed final text mismatch: %q", tr.finalText)
	}
}

func TestToolTurnDispatchesAndCompletes(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	conv, err := newEngine(fake.NewStore()).StartSession(ctx, "s-tool")
	if err != nil {
		t.Fatal(err)
	}
	conv.Send(core.PromptIntent{Text: "please use the tool"})

	tr := drain(t, conv)
	wantKinds := []string{"tool_started", "tool_finished", "delta", "turn_complete"}
	if !equal(tr.kinds, wantKinds) {
		t.Fatalf("kind sequence mismatch:\n got: %v\nwant: %v", tr.kinds, wantKinds)
	}
	if len(tr.toolResults) != 1 || tr.toolResults[0] != ".\n" {
		t.Fatalf("tool result mismatch: %v", tr.toolResults)
	}
	if tr.finalText != "This is a deterministic fake response." {
		t.Fatalf("final text mismatch: %q", tr.finalText)
	}
}

// TestEventLogIsTheSourceOfTruth asserts the persisted payloads, not just kinds:
// the tool turn must persist user -> assistant(tool call) -> tool result ->
// assistant(final), plus a usage event from the fake's final response.
func TestEventLogIsTheSourceOfTruth(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	store := fake.NewStore()
	conv, err := newEngine(store).StartSession(ctx, "s-log")
	if err != nil {
		t.Fatal(err)
	}
	conv.Send(core.PromptIntent{Text: "please use the tool"})
	drain(t, conv)

	events, err := store.Events(ctx, "s-log")
	if err != nil {
		t.Fatal(err)
	}

	wantKinds := []core.EventKind{
		core.EventUserMessage,
		core.EventAssistantMessage, // requests the tool
		core.EventToolResult,
		core.EventAssistantMessage, // final answer
		core.EventUsage,            // usage from the final response
	}
	if len(events) != len(wantKinds) {
		t.Fatalf("expected %d events, got %d", len(wantKinds), len(events))
	}
	for i, want := range wantKinds {
		if events[i].Payload.Kind() != want {
			t.Fatalf("event %d: want %q got %q", i, want, events[i].Payload.Kind())
		}
		if events[i].Seq != int64(i) {
			t.Fatalf("event %d: want Seq %d got %d", i, i, events[i].Seq)
		}
		if events[i].Version != core.CurrentEventVersion {
			t.Fatalf("event %d: missing schema version", i)
		}
	}

	// Tool result payload content must round-trip.
	tr, ok := events[2].Payload.(core.ToolResultPayload)
	if !ok || tr.Result.Content != ".\n" {
		t.Fatalf("tool result payload mismatch: %#v", events[2].Payload)
	}
}

// TestEventsAreGroupedByTurnID asserts the observability foundation: every
// engine-produced event carries a nonzero, monotonic TurnID, and successive
// prompts form distinct turn groups. This is the join key a debugging agent uses
// to isolate one turn and the unit of deterministic replay.
func TestEventsAreGroupedByTurnID(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	store := fake.NewStore()
	conv, err := newEngine(store).StartSession(ctx, "s-turns")
	if err != nil {
		t.Fatal(err)
	}
	conv.Send(core.PromptIntent{Text: "hello there"})
	drain(t, conv)
	conv.Send(core.PromptIntent{Text: "hello again"})
	drain(t, conv)

	events, err := store.Events(ctx, "s-turns")
	if err != nil {
		t.Fatal(err)
	}

	seen := map[int64]int{}
	var prev int64
	for i, ev := range events {
		if ev.TurnID == 0 {
			t.Fatalf("event %d (%s) has zero TurnID", i, ev.Payload.Kind())
		}
		if ev.TurnID < prev {
			t.Fatalf("TurnID went backwards at event %d: %d after %d", i, ev.TurnID, prev)
		}
		prev = ev.TurnID
		seen[ev.TurnID]++
	}
	if len(seen) != 2 || seen[1] == 0 || seen[2] == 0 {
		t.Fatalf("expected two turns {1,2}, got %v", seen)
	}
}

// TestInterruptCancelsTurn exercises the actor's defining feature: an interrupt
// mid-turn cancels the turn's context. A blocking provider holds the turn open
// until cancellation so the test is deterministic, not timing-dependent.
func TestInterruptCancelsTurn(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	started := make(chan struct{})
	eng := engine.New(&blockingProvider{started: started}, fake.Tools{}, fake.NewStore(), fake.NewClock(), engine.Config{Model: "x", MaxIterations: 5})
	conv, err := eng.StartSession(ctx, "s-int")
	if err != nil {
		t.Fatal(err)
	}
	conv.Send(core.PromptIntent{Text: "go"})
	<-started // the turn is now in flight and blocked
	conv.Send(core.InterruptIntent{})

	tr := drain(t, conv)
	if got := tr.kinds[len(tr.kinds)-1]; got != "interrupted" {
		t.Fatalf("expected interrupted, got %q (%v)", got, tr.kinds)
	}
}

// blockingProvider emits one delta, signals, then blocks until ctx is cancelled.
type blockingProvider struct{ started chan struct{} }

func (*blockingProvider) Name() string                     { return "blocking" }
func (*blockingProvider) Capabilities() ports.Capabilities { return ports.Capabilities{} }

func (p *blockingProvider) Stream(ctx context.Context, _ core.LLMRequest) (<-chan core.LLMChunk, error) {
	out := make(chan core.LLMChunk)
	go func() {
		defer close(out)
		select {
		case out <- core.LLMChunk{ContentDelta: "working"}:
		case <-ctx.Done():
			return
		}
		close(p.started)
		<-ctx.Done()
	}()
	return out, nil
}

func equal(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}
