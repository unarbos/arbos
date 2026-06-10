package sqlite_test

import (
	"context"
	"path/filepath"
	"testing"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/ports"
	"github.com/unarbos/arbos/internal/ports/porttest"
	"github.com/unarbos/arbos/internal/sqlite"
)

// openTemp returns a fresh store backed by a unique temp file. t.TempDir gives a
// new directory per call, so every store is isolated and cleaned up by the test
// framework.
func openTemp(t *testing.T) *sqlite.Store {
	t.Helper()
	s, err := sqlite.Open(filepath.Join(t.TempDir(), "store.db"))
	if err != nil {
		t.Fatalf("open: %v", err)
	}
	t.Cleanup(func() { _ = s.Close() })
	return s
}

// TestSQLiteStoreContract is the whole point of the porttest scaffold: the
// durable store must satisfy the exact behavioral contract the in-memory fake
// does, proving they are interchangeable behind ports.SessionStore.
func TestSQLiteStoreContract(t *testing.T) {
	porttest.RunSessionStoreContract(t, func() ports.SessionStore {
		return openTemp(t)
	})
}

// TestAddAtomThenRecall proves the explicit memory write (the remember tool's
// store call) lands and is findable by recall — the read/write symmetry the
// agent needs to deliberately use memory.
func TestAddAtomThenRecall(t *testing.T) {
	s := openTemp(t)
	ctx := context.Background()
	if err := s.AddAtom(ctx, "the user's name is Const"); err != nil {
		t.Fatalf("add atom: %v", err)
	}
	hits, err := s.RecallAtoms(ctx, "Const", 5)
	if err != nil {
		t.Fatalf("recall: %v", err)
	}
	found := false
	for _, a := range hits {
		if a.Content == "the user's name is Const" {
			found = true
		}
	}
	if !found {
		t.Fatalf("remembered atom not recalled; got %d hits: %+v", len(hits), hits)
	}
}

// TestPersistsAcrossReopen proves durability: events written by one handle are
// readable after closing and reopening the same file — the property the
// in-memory fake cannot have and the reason the SQLite store exists.
func TestPersistsAcrossReopen(t *testing.T) {
	ctx := context.Background()
	now := time.Unix(0, 0).UTC()
	path := filepath.Join(t.TempDir(), "persist.db")

	s, err := sqlite.Open(path)
	if err != nil {
		t.Fatal(err)
	}
	if err := s.CreateSession(ctx, core.Session{ID: "s1", Status: core.SessionActive, Model: "m", CreatedAt: now, UpdatedAt: now}); err != nil {
		t.Fatal(err)
	}
	if err := s.AppendEvent(ctx, core.NewMessageEvent("s1", core.Message{Role: core.RoleUser, Content: "remember me"}, now)); err != nil {
		t.Fatal(err)
	}
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}

	reopened, err := sqlite.Open(path)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = reopened.Close() }()

	evs, err := reopened.Events(ctx, "s1")
	if err != nil {
		t.Fatal(err)
	}
	if len(evs) != 1 {
		t.Fatalf("want 1 event after reopen, got %d", len(evs))
	}
	mp, ok := evs[0].Payload.(core.MessagePayload)
	if !ok || mp.Message.Content != "remember me" {
		t.Fatalf("payload lost across reopen: %#v", evs[0].Payload)
	}
	if evs[0].Version != core.CurrentEventVersion {
		t.Fatalf("version not preserved: %d", evs[0].Version)
	}
}

// TestSearchFindsMessageContent exercises the FTS5 index that makes session
// search a single-binary win (no external search engine).
func TestSearchFindsMessageContent(t *testing.T) {
	ctx := context.Background()
	now := time.Unix(0, 0).UTC()
	s := openTemp(t)
	if err := s.CreateSession(ctx, core.Session{ID: "s1", Status: core.SessionActive, CreatedAt: now, UpdatedAt: now}); err != nil {
		t.Fatal(err)
	}
	mustAppend(t, s, core.NewMessageEvent("s1", core.Message{Role: core.RoleUser, Content: "the quick brown fox"}, now))
	mustAppend(t, s, core.NewMessageEvent("s1", core.Message{Role: core.RoleAssistant, Content: "a lazy dog sleeps"}, now))
	// Usage events carry no prose and must not pollute the index.
	mustAppend(t, s, core.NewUsageEvent("s1", core.Usage{TotalTokens: 5}, now))

	hits, err := s.Search(ctx, "s1", "fox", 10)
	if err != nil {
		t.Fatal(err)
	}
	if len(hits) != 1 {
		t.Fatalf("want 1 hit for 'fox', got %d (%+v)", len(hits), hits)
	}
	if hits[0].SessionID != "s1" {
		t.Fatalf("unexpected hit session: %q", hits[0].SessionID)
	}

	none, err := s.Search(ctx, "s1", "elephant", 10)
	if err != nil {
		t.Fatal(err)
	}
	if len(none) != 0 {
		t.Fatalf("want 0 hits for 'elephant', got %d", len(none))
	}
}

func mustAppend(t *testing.T, s *sqlite.Store, ev *core.Event) {
	t.Helper()
	if err := s.AppendEvent(context.Background(), ev); err != nil {
		t.Fatalf("append: %v", err)
	}
}
