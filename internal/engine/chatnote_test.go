package engine_test

import (
	"context"
	"testing"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/fake"
	"github.com/unarbos/arbos/internal/ports"
)

// heldProvider streams one delta, then blocks until release is closed before
// finishing — so a test can act while a turn is reliably in flight.
type heldProvider struct{ release chan struct{} }

func (heldProvider) Name() string                     { return "held" }
func (heldProvider) Capabilities() ports.Capabilities { return ports.Capabilities{Tools: true} }

func (p heldProvider) Stream(ctx context.Context, req core.LLMRequest) (<-chan core.LLMChunk, error) {
	out := make(chan core.LLMChunk, 4)
	go func() {
		defer close(out)
		out <- core.LLMChunk{ContentDelta: "working"}
		select {
		case <-p.release:
		case <-ctx.Done():
			return
		}
		out <- core.LLMChunk{Done: true, Usage: &core.Usage{TotalTokens: 1}}
	}()
	return out, nil
}

// collectChatKinds reads events for a window, tagging the kinds relevant to the
// side-chat behavior. It never blocks past the deadline.
func collectChatKinds(conv *engine.Conversation, d time.Duration) map[string]int {
	got := map[string]int{}
	deadline := time.After(d)
	for {
		select {
		case env, ok := <-conv.Events():
			if !ok {
				return got
			}
			switch env.Event.(type) {
			case core.ChatNote:
				got["chat_note"]++
			case core.Queued:
				got["queued"]++
			case core.TurnComplete:
				got["turn_complete"]++
			case core.ErrorEvent:
				got["error"]++
			case core.MessageDelta:
				got["delta"]++
			}
		case <-deadline:
			return got
		}
	}
}

// An idle chat note broadcasts a ChatNote and starts NO turn — no Queued, no
// TurnComplete, no ErrorEvent.
func TestChatNoteIdleNoTurn(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	eng := engine.New(fake.Provider{}, fake.Tools{}, fake.NewStore(), fake.NewClock(),
		engine.Config{Model: "x", MaxIterations: 5})
	conv, err := eng.StartSession(ctx, "s-note-idle")
	if err != nil {
		t.Fatal(err)
	}
	conv.Send(core.ChatNoteIntent{Text: "look at the terminal tab", Author: "Alice"})

	got := collectChatKinds(conv, 500*time.Millisecond)
	if got["chat_note"] != 1 {
		t.Errorf("want exactly 1 chat_note, got %d (all: %v)", got["chat_note"], got)
	}
	for _, forbidden := range []string{"turn_complete", "queued", "error", "delta"} {
		if got[forbidden] != 0 {
			t.Errorf("idle chat note must not produce %q, got %d", forbidden, got[forbidden])
		}
	}
}

// THE headline regression: a chat note posted MID-TURN logs+broadcasts without
// ending the turn — no ErrorEvent, the turn still completes normally.
func TestChatNoteMidTurnDoesNotBreakTurn(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	p := heldProvider{release: make(chan struct{})}
	eng := engine.New(p, fake.Tools{}, fake.NewStore(), fake.NewClock(),
		engine.Config{Model: "x", MaxIterations: 5})
	conv, err := eng.StartSession(ctx, "s-note-midturn")
	if err != nil {
		t.Fatal(err)
	}

	// Start a turn and wait until it is in flight (the first delta arrived).
	conv.Send(core.PromptIntent{Text: "do the thing"})
	waitFirstDelta(t, conv)

	// Post a side-chat line WHILE the agent works, then let the turn finish.
	conv.Send(core.ChatNoteIntent{Text: "fyi I'm watching", Author: "Bob"})
	time.Sleep(50 * time.Millisecond) // let the actor process the note
	close(p.release)

	got := collectChatKinds(conv, time.Second)
	if got["error"] != 0 {
		t.Errorf("mid-turn chat note emitted an ErrorEvent (turn-terminal) — turn isolation broken")
	}
	if got["turn_complete"] != 1 {
		t.Errorf("turn did not complete normally after a mid-turn note: %v", got)
	}
	if got["chat_note"] != 1 {
		t.Errorf("want the mid-turn chat note broadcast once, got %d", got["chat_note"])
	}
}

// waitFirstDelta drains until the first MessageDelta so the turn is provably in
// flight, failing if none arrives.
func waitFirstDelta(t *testing.T, conv *engine.Conversation) {
	t.Helper()
	deadline := time.After(2 * time.Second)
	for {
		select {
		case env, ok := <-conv.Events():
			if !ok {
				t.Fatal("events closed before first delta")
			}
			if _, isDelta := env.Event.(core.MessageDelta); isDelta {
				return
			}
		case <-deadline:
			t.Fatal("no delta — turn never started")
		}
	}
}
