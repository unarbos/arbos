package engine_test

import (
	"context"
	"sync"
	"testing"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/fake"
	"github.com/unarbos/arbos/internal/ports"
)

// drainLast reads to the first terminal event and returns its kind. Safe to call
// from a goroutine (it never touches *testing.T).
func drainLast(conv *engine.Conversation) string {
	for env := range conv.Events() {
		switch env.Event.(type) {
		case core.TurnComplete:
			return "turn_complete"
		case core.Interrupted:
			return "interrupted"
		case core.ErrorEvent:
			return "error"
		}
	}
	return ""
}

// TestInterruptUnwindsUnderBackpressure is the empirical proof for P0-A: a slow
// consumer fills the events buffer so the turn blocks on emit; an interrupt must
// still unwind the actor. With a non-cancellation-aware emit this deadlocks and
// the test times out.
func TestInterruptUnwindsUnderBackpressure(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	eng := engine.New(floodProvider{}, fake.Tools{}, fake.NewStore(), fake.NewClock(),
		engine.Config{Model: "x", MaxIterations: 5})
	conv, err := eng.StartSession(ctx, "s-flood")
	if err != nil {
		t.Fatal(err)
	}

	conv.Send(core.PromptIntent{Text: "go"})
	// Let the events buffer (cap 32) fill while nobody reads, so the turn
	// goroutine is parked on a send.
	time.Sleep(100 * time.Millisecond)
	conv.Send(core.InterruptIntent{})

	result := make(chan string, 1)
	go func() { result <- drainLast(conv) }()

	select {
	case last := <-result:
		if last != "interrupted" {
			t.Fatalf("expected interrupted, got %q", last)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("deadlock: interrupt did not unwind under backpressure (P0-A)")
	}
}

// TestInterruptLeavesReplayableLog is the empirical proof for P0-C: interrupting
// a turn mid-tool-dispatch must not leave an assistant tool_call without a
// matching tool result, or a real provider would 400 on resume. Every tool_call
// must end up answered (real result for the dispatched call, synthetic for the
// rest) and the interrupt must be recorded.
func TestInterruptLeavesReplayableLog(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	started := make(chan struct{})
	store := fake.NewStore()
	eng := engine.New(twoToolProvider{}, &gatedTools{started: started}, store, fake.NewClock(),
		engine.Config{Model: "x", MaxIterations: 5})
	conv, err := eng.StartSession(ctx, "s-replay")
	if err != nil {
		t.Fatal(err)
	}

	conv.Send(core.PromptIntent{Text: "please use the tool"})
	<-started // first tool is mid-dispatch
	conv.Send(core.InterruptIntent{})

	if last := awaitTerminal(t, conv); last != "interrupted" {
		t.Fatalf("expected interrupted, got %q", last)
	}

	events, err := store.Events(ctx, "s-replay")
	if err != nil {
		t.Fatal(err)
	}

	var toolCalls, toolResults int
	var sawInterrupt bool
	for _, ev := range events {
		switch p := ev.Payload.(type) {
		case core.MessagePayload:
			if p.Message.Role == core.RoleAssistant {
				toolCalls += len(p.Message.ToolCalls)
			}
		case core.ToolResultPayload:
			toolResults++
		case core.InterruptPayload:
			sawInterrupt = true
		}
	}
	if toolCalls != 2 {
		t.Fatalf("want 2 tool calls in log, got %d", toolCalls)
	}
	if toolResults != 2 {
		t.Fatalf("want 2 tool results (1 real + 1 backfilled), got %d", toolResults)
	}
	if !sawInterrupt {
		t.Fatal("interrupt was not recorded in the log")
	}

	// The projection a real provider would receive must answer every tool_call.
	assertEveryToolCallAnswered(t, core.Project(events, ""))
}

// TestConcurrentSessionsAreRaceFree is the empirical proof for P1-G: two session
// actors share one engine clock. Run under `go test -race` this fails if Clock
// mutates without synchronization.
func TestConcurrentSessionsAreRaceFree(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	eng := newEngine(fake.NewStore()) // one engine => one shared clock
	var wg sync.WaitGroup
	for _, id := range []core.SessionID{"race-a", "race-b", "race-c"} {
		conv, err := eng.StartSession(ctx, id)
		if err != nil {
			t.Fatal(err)
		}
		conv.Send(core.PromptIntent{Text: "please use the tool"})
		wg.Add(1)
		go func(c *engine.Conversation) {
			defer wg.Done()
			drainLast(c)
		}(conv)
	}
	wg.Wait()
}

func awaitTerminal(t *testing.T, conv *engine.Conversation) string {
	t.Helper()
	result := make(chan string, 1)
	go func() { result <- drainLast(conv) }()
	select {
	case last := <-result:
		return last
	case <-time.After(2 * time.Second):
		t.Fatal("turn did not reach a terminal event in time")
		return ""
	}
}

func assertEveryToolCallAnswered(t *testing.T, msgs []core.Message) {
	t.Helper()
	answered := map[string]bool{}
	for _, m := range msgs {
		if m.Role == core.RoleTool {
			answered[m.ToolCallID] = true
		}
	}
	for _, m := range msgs {
		if m.Role != core.RoleAssistant {
			continue
		}
		for _, call := range m.ToolCalls {
			if !answered[call.ID] {
				t.Fatalf("tool_call %q has no matching tool result in projection (would 400)", call.ID)
			}
		}
	}
}

// floodProvider streams deltas forever until ctx is cancelled.
type floodProvider struct{}

func (floodProvider) Name() string                     { return "flood" }
func (floodProvider) Capabilities() ports.Capabilities { return ports.Capabilities{} }

func (floodProvider) Stream(ctx context.Context, _ core.LLMRequest) (<-chan core.LLMChunk, error) {
	out := make(chan core.LLMChunk)
	go func() {
		defer close(out)
		for {
			select {
			case <-ctx.Done():
				return
			case out <- core.LLMChunk{ContentDelta: "x"}:
			}
		}
	}()
	return out, nil
}

// twoToolProvider requests two tool calls on the first turn, then answers.
type twoToolProvider struct{}

func (twoToolProvider) Name() string                     { return "twotool" }
func (twoToolProvider) Capabilities() ports.Capabilities { return ports.Capabilities{Tools: true} }

func (twoToolProvider) Stream(_ context.Context, req core.LLMRequest) (<-chan core.LLMChunk, error) {
	out := make(chan core.LLMChunk, 4)
	go func() {
		defer close(out)
		for _, m := range req.Messages {
			if m.Role == core.RoleTool {
				out <- core.LLMChunk{ContentDelta: "done", Done: true}
				return
			}
		}
		out <- core.LLMChunk{ToolCalls: []core.ToolCall{
			{ID: "c1", Name: "t1"},
			{ID: "c2", Name: "t2"},
		}}
		out <- core.LLMChunk{Done: true}
	}()
	return out, nil
}

// gatedTools blocks on the first call until ctx is cancelled (signalling once it
// is mid-dispatch), so the test can interrupt deterministically.
type gatedTools struct {
	started chan struct{}
	once    sync.Once
}

func (*gatedTools) Schemas() []core.ToolSchema {
	return []core.ToolSchema{{Name: "t1"}, {Name: "t2"}}
}

func (g *gatedTools) Dispatch(ctx context.Context, call core.ToolCall) core.ToolResult {
	if call.Name == "t1" {
		g.once.Do(func() { close(g.started) })
		<-ctx.Done()
		return core.ToolResult{CallID: call.ID, IsError: true, Content: "cancelled"}
	}
	return core.ToolResult{CallID: call.ID, Content: "t2 ran"}
}
