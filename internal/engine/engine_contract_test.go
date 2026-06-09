package engine_test

import (
	"context"
	"path/filepath"
	"testing"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/fake"
	"github.com/unarbos/arbos/internal/ports"
	"github.com/unarbos/arbos/internal/sqlite"
)

// storeFactories enumerates every SessionStore implementation the engine
// contract must hold for. The durable SQLite store runs the entire golden suite
// below unchanged — that is what makes it a provable drop-in for the fake rather
// than a hopeful one.
func storeFactories(t *testing.T) map[string]func() ports.SessionStore {
	t.Helper()
	return map[string]func() ports.SessionStore{
		"fake": func() ports.SessionStore { return fake.NewStore() },
		"sqlite": func() ports.SessionStore {
			s, err := sqlite.Open(filepath.Join(t.TempDir(), "engine.db"))
			if err != nil {
				t.Fatalf("open sqlite: %v", err)
			}
			t.Cleanup(func() { _ = s.Close() })
			return s
		},
	}
}

// TestEventLogFullFidelity locks the entire persisted event list for a tool
// turn — kind, Seq, Version, and full payload contents — not just the kind
// sequence. The deterministic fake clock makes CreatedAt assertable as
// monotonic. This is the golden the SQLite store must reproduce byte-for-byte.
func TestEventLogFullFidelity(t *testing.T) {
	for name, mk := range storeFactories(t) {
		t.Run(name, func(t *testing.T) {
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()

			store := mk()
			conv, err := newEngine(store).StartSession(ctx, "s-fidelity")
			if err != nil {
				t.Fatal(err)
			}
			conv.Send(core.PromptIntent{Text: "please use the tool"})
			drain(t, conv)

			events, err := store.Events(ctx, "s-fidelity")
			if err != nil {
				t.Fatal(err)
			}

			wantKinds := []core.EventKind{
				core.EventUserMessage,
				core.EventAssistantMessage, // requests the tool
				core.EventToolResult,
				core.EventAssistantMessage, // final answer
				core.EventUsage,
			}
			if len(events) != len(wantKinds) {
				t.Fatalf("want %d events, got %d", len(wantKinds), len(events))
			}

			var lastTime int64 = -1
			for i, want := range wantKinds {
				ev := events[i]
				if ev.Payload.Kind() != want {
					t.Fatalf("event %d: want kind %q got %q", i, want, ev.Payload.Kind())
				}
				if ev.Seq != int64(i) {
					t.Fatalf("event %d: want Seq %d got %d", i, i, ev.Seq)
				}
				if ev.Version != core.CurrentEventVersion {
					t.Fatalf("event %d: want version %d got %d", i, core.CurrentEventVersion, ev.Version)
				}
				if ev.CreatedAt.IsZero() {
					t.Fatalf("event %d: CreatedAt is zero", i)
				}
				if unix := ev.CreatedAt.UnixNano(); unix < lastTime {
					t.Fatalf("event %d: CreatedAt went backwards", i)
				} else {
					lastTime = unix
				}
			}

			// Full payload fidelity, not just kinds.
			if mp, ok := events[0].Payload.(core.MessagePayload); !ok || mp.Message.Content != "please use the tool" {
				t.Fatalf("event 0 payload: %#v", events[0].Payload)
			}
			if mp, ok := events[1].Payload.(core.MessagePayload); !ok || len(mp.Message.ToolCalls) != 1 || mp.Message.ToolCalls[0].Name != "ls" {
				t.Fatalf("event 1 must carry the ls tool call: %#v", events[1].Payload)
			}
			if tp, ok := events[2].Payload.(core.ToolResultPayload); !ok || tp.Result.Content != ".\n" {
				t.Fatalf("event 2 tool result: %#v", events[2].Payload)
			}
			if mp, ok := events[3].Payload.(core.MessagePayload); !ok || mp.Message.Content != "This is a deterministic fake response." {
				t.Fatalf("event 3 final answer: %#v", events[3].Payload)
			}
			if up, ok := events[4].Payload.(core.UsagePayload); !ok || up.Usage.TotalTokens != 42 {
				t.Fatalf("event 4 usage: %#v", events[4].Payload)
			}
		})
	}
}

// TestReloadReplayRoundTrip simulates a restart/resume: read the persisted log
// back, push every payload through the core codec (Encode -> Decode, exactly
// what a byte-backed store like SQLite does), then Project. This proves codec
// and projection agree and that a reloaded session reconstructs the conversation
// the model saw — the central deterministic-replay guarantee.
func TestReloadReplayRoundTrip(t *testing.T) {
	for name, mk := range storeFactories(t) {
		t.Run(name, func(t *testing.T) {
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()

			store := mk()
			conv, err := newEngine(store).StartSession(ctx, "s-replay")
			if err != nil {
				t.Fatal(err)
			}
			conv.Send(core.PromptIntent{Text: "please use the tool"})
			drain(t, conv)

			events, err := store.Events(ctx, "s-replay")
			if err != nil {
				t.Fatal(err)
			}
			roundTripped := make([]core.Event, len(events))
			for i, ev := range events {
				data, err := core.EncodePayload(ev.Payload)
				if err != nil {
					t.Fatalf("encode event %d: %v", i, err)
				}
				p, err := core.DecodePayload(ev.Payload.Kind(), data)
				if err != nil {
					t.Fatalf("decode event %d: %v", i, err)
				}
				ev.Payload = p
				roundTripped[i] = ev
			}

			msgs := core.Project(roundTripped, "sys")
			wantRoles := []core.Role{core.RoleSystem, core.RoleUser, core.RoleAssistant, core.RoleTool, core.RoleAssistant}
			if len(msgs) != len(wantRoles) {
				t.Fatalf("want %d messages, got %d (%+v)", len(wantRoles), len(msgs), msgs)
			}
			for i, r := range wantRoles {
				if msgs[i].Role != r {
					t.Fatalf("msg %d: want role %q got %q", i, r, msgs[i].Role)
				}
			}
			if msgs[len(msgs)-1].Content != "This is a deterministic fake response." {
				t.Fatalf("final assistant content lost on reload: %q", msgs[len(msgs)-1].Content)
			}
		})
	}
}

// TestStoreRejectsInvalidEvents is the port-contract test for Event.Validate:
// every SessionStore must refuse a malformed event so "the log is the source of
// truth" cannot be undermined by an incidental check (or its absence) in one
// store. SQLite must reject identically.
func TestStoreRejectsInvalidEvents(t *testing.T) {
	for name, mk := range storeFactories(t) {
		t.Run(name, func(t *testing.T) {
			ctx := context.Background()
			store := mk()
			if err := store.CreateSession(ctx, core.Session{ID: "s", Status: core.SessionActive}); err != nil {
				t.Fatal(err)
			}

			bad := []struct {
				name string
				ev   *core.Event
			}{
				{"nil payload", &core.Event{SessionID: "s", Version: core.CurrentEventVersion}},
				{"zero version", &core.Event{SessionID: "s", Payload: core.MessagePayload{Message: core.Message{Role: core.RoleUser}}}},
				{"empty session", &core.Event{Version: core.CurrentEventVersion, Payload: core.MessagePayload{Message: core.Message{Role: core.RoleUser}}}},
				{"non-persistable role", &core.Event{SessionID: "s", Version: core.CurrentEventVersion, Payload: core.MessagePayload{Message: core.Message{Role: core.RoleTool}}}},
			}
			for _, b := range bad {
				if err := store.AppendEvent(ctx, b.ev); err == nil {
					t.Fatalf("%s: expected AppendEvent to reject, got nil", b.name)
				}
			}
		})
	}
}
