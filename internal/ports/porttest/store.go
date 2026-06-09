// Package porttest provides reusable contract test suites for the kernel ports.
//
// The architecture is ports + adapters, so the load-bearing test scaffold is
// "every adapter behaves identically behind its port". These suites encode each
// port's behavioral contract once; the fake passes them today and every future
// adapter (the SQLite store, real providers, transport agents) must pass the
// same suite. That makes "the fake and the real thing are interchangeable" a
// tested guarantee, not a hope — and makes TDD for a new adapter literal: wire
// it into the suite, watch it fail, implement until green.
//
// Call from a normal *_test.go in the adapter's package:
//
//	func TestSQLiteStoreContract(t *testing.T) {
//	    porttest.RunSessionStoreContract(t, func() ports.SessionStore { return sqlite.New(...) })
//	}
package porttest

import (
	"context"
	"errors"
	"sync"
	"testing"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/ports"
)

// RunSessionStoreContract asserts the behavioral contract every SessionStore
// must satisfy. newStore must return a fresh, empty store on each call.
func RunSessionStoreContract(t *testing.T, newStore func() ports.SessionStore) {
	t.Helper()
	ctx := context.Background()
	now := time.Unix(0, 0).UTC()

	t.Run("create_get_roundtrip", func(t *testing.T) {
		s := newStore()
		want := core.Session{ID: "s1", Status: core.SessionActive, Model: "m", CreatedAt: now, UpdatedAt: now}
		if err := s.CreateSession(ctx, want); err != nil {
			t.Fatalf("CreateSession: %v", err)
		}
		got, err := s.Get(ctx, "s1")
		if err != nil {
			t.Fatalf("Get: %v", err)
		}
		if got.ID != "s1" || got.Model != "m" || got.Status != core.SessionActive {
			t.Fatalf("round-trip mismatch: %+v", got)
		}
	})

	t.Run("get_unknown_errors", func(t *testing.T) {
		s := newStore()
		// The contract is the sentinel, not merely "an error": callers gate on
		// errors.Is(err, ErrSessionNotFound) to distinguish "no such session"
		// from a transient store failure (ports.go). Every adapter must agree.
		if _, err := s.Get(ctx, "nope"); !errors.Is(err, ports.ErrSessionNotFound) {
			t.Fatalf("expected ErrSessionNotFound, got %v", err)
		}
	})

	t.Run("create_duplicate_errors", func(t *testing.T) {
		s := newStore()
		mustCreate(t, s, "s1", now)
		if err := s.CreateSession(ctx, core.Session{ID: "s1", Status: core.SessionActive, CreatedAt: now, UpdatedAt: now}); err == nil {
			t.Fatal("expected error for duplicate session id")
		}
	})

	t.Run("append_assigns_monotonic_seq_and_id", func(t *testing.T) {
		s := newStore()
		mustCreate(t, s, "s1", now)
		for i := 0; i < 3; i++ {
			ev := core.NewMessageEvent("s1", core.Message{Role: core.RoleUser, Content: "x"}, now)
			if err := s.AppendEvent(ctx, ev); err != nil {
				t.Fatalf("AppendEvent: %v", err)
			}
			if ev.Seq != int64(i) {
				t.Fatalf("event %d: want Seq %d, got %d", i, i, ev.Seq)
			}
			if ev.ID == 0 {
				t.Fatalf("event %d: store did not assign an ID", i)
			}
		}
	})

	t.Run("append_unknown_session_errors", func(t *testing.T) {
		s := newStore()
		ev := core.NewMessageEvent("ghost", core.Message{Role: core.RoleUser}, now)
		if err := s.AppendEvent(ctx, ev); !errors.Is(err, ports.ErrSessionNotFound) {
			t.Fatalf("expected ErrSessionNotFound appending to unknown session, got %v", err)
		}
	})

	t.Run("append_rejects_invalid_event", func(t *testing.T) {
		s := newStore()
		mustCreate(t, s, "s1", now)
		// Built outside the constructors: nil payload + zero version. The port
		// contract requires AppendEvent to enforce core.Event.Validate.
		if err := s.AppendEvent(ctx, &core.Event{SessionID: "s1"}); err == nil {
			t.Fatal("expected AppendEvent to reject an event that fails Validate")
		}
	})

	t.Run("events_returned_in_order", func(t *testing.T) {
		s := newStore()
		mustCreate(t, s, "s1", now)
		mustAppend(t, s, core.NewMessageEvent("s1", core.Message{Role: core.RoleUser, Content: "a"}, now))
		mustAppend(t, s, core.NewMessageEvent("s1", core.Message{Role: core.RoleAssistant, Content: "b"}, now))
		evs, err := s.Events(ctx, "s1")
		if err != nil {
			t.Fatalf("Events: %v", err)
		}
		if len(evs) != 2 || evs[0].Seq != 0 || evs[1].Seq != 1 {
			t.Fatalf("unexpected events: %+v", evs)
		}
	})

	t.Run("events_returns_a_copy", func(t *testing.T) {
		s := newStore()
		mustCreate(t, s, "s1", now)
		mustAppend(t, s, core.NewMessageEvent("s1", core.Message{Role: core.RoleUser, Content: "a"}, now))
		evs, _ := s.Events(ctx, "s1")
		evs[0].Seq = 999 // mutate the caller's slice
		again, _ := s.Events(ctx, "s1")
		if again[0].Seq != 0 {
			t.Fatal("Events must return a copy; a caller mutation leaked into the store")
		}
	})

	t.Run("events_unknown_session_is_empty_not_panic", func(t *testing.T) {
		s := newStore()
		evs, err := s.Events(ctx, "nope")
		if err == nil && len(evs) != 0 {
			t.Fatalf("unknown session should yield no events, got %d", len(evs))
		}
	})

	t.Run("update_persists", func(t *testing.T) {
		s := newStore()
		mustCreate(t, s, "s1", now)
		sess, _ := s.Get(ctx, "s1")
		sess.TokenCount = 123
		sess.Status = core.SessionEnded
		if err := s.UpdateSession(ctx, sess); err != nil {
			t.Fatalf("UpdateSession: %v", err)
		}
		got, _ := s.Get(ctx, "s1")
		if got.TokenCount != 123 || got.Status != core.SessionEnded {
			t.Fatalf("update did not persist: %+v", got)
		}
	})

	t.Run("update_unknown_errors", func(t *testing.T) {
		s := newStore()
		if err := s.UpdateSession(ctx, core.Session{ID: "nope"}); !errors.Is(err, ports.ErrSessionNotFound) {
			t.Fatalf("expected ErrSessionNotFound updating unknown session, got %v", err)
		}
	})

	t.Run("concurrent_appends_across_sessions", func(t *testing.T) {
		s := newStore()
		ids := []core.SessionID{"a", "b", "c"}
		for _, id := range ids {
			mustCreate(t, s, id, now)
		}
		var wg sync.WaitGroup
		for _, id := range ids {
			id := id
			wg.Add(1)
			go func() {
				defer wg.Done()
				for i := 0; i < 25; i++ {
					_ = s.AppendEvent(ctx, core.NewMessageEvent(id, core.Message{Role: core.RoleUser, Content: "x"}, now))
				}
			}()
		}
		wg.Wait()
		for _, id := range ids {
			evs, _ := s.Events(ctx, id)
			if len(evs) != 25 {
				t.Fatalf("session %q: want 25 events, got %d", id, len(evs))
			}
		}
	})
}

func mustCreate(t *testing.T, s ports.SessionStore, id core.SessionID, now time.Time) {
	t.Helper()
	if err := s.CreateSession(context.Background(), core.Session{ID: id, Status: core.SessionActive, CreatedAt: now, UpdatedAt: now}); err != nil {
		t.Fatalf("CreateSession(%q): %v", id, err)
	}
}

func mustAppend(t *testing.T, s ports.SessionStore, ev *core.Event) {
	t.Helper()
	if err := s.AppendEvent(context.Background(), ev); err != nil {
		t.Fatalf("AppendEvent: %v", err)
	}
}
