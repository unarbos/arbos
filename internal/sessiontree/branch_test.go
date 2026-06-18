package sessiontree

import (
	"context"
	"testing"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/fake"
)

// seedParent builds a parent session with a few message turns and returns its
// id plus the store. Seqs are store-assigned 0..n-1.
func seedParent(t *testing.T, store *fake.Store) core.SessionID {
	t.Helper()
	ctx := context.Background()
	now := time.Unix(0, 0)
	const pid core.SessionID = "parent"
	if err := store.CreateSession(ctx, core.Session{ID: pid, Status: core.SessionActive, Model: "m", CreatedAt: now, UpdatedAt: now}); err != nil {
		t.Fatalf("create parent: %v", err)
	}
	for _, m := range []core.Message{
		{Role: core.RoleUser, Content: "explain the plan"},
		{Role: core.RoleAssistant, Content: "the plan has three regions, the second being the conversation"},
		{Role: core.RoleUser, Content: "and after that?"},
	} {
		if err := store.AppendEvent(ctx, core.NewMessageEvent(pid, m, now)); err != nil {
			t.Fatalf("append: %v", err)
		}
	}
	return pid
}

func TestBranchForksAtAnchorAndSeeds(t *testing.T) {
	ctx := context.Background()
	store := fake.NewStore()
	pid := seedParent(t, store)
	now := time.Unix(0, 0)

	const child core.SessionID = "child-1"
	anchor := core.BranchAnchorPayload{Seq: 1, Start: 4, End: 8, Quote: "plan has three regions"}
	if err := Branch(ctx, store, pid, child, anchor, now); err != nil {
		t.Fatalf("Branch: %v", err)
	}

	// Child is a session whose parent is the source.
	cs, err := store.Get(ctx, child)
	if err != nil {
		t.Fatalf("get child: %v", err)
	}
	if cs.ParentID != pid {
		t.Errorf("child ParentID = %q, want %q", cs.ParentID, pid)
	}

	// Child sees parent history up to and including the anchored event (seq 1),
	// not the later seq-2 user turn, plus the seeded anchor context segment.
	cev, _ := store.Events(ctx, child)
	var msgs, anchorSegs int
	for _, e := range cev {
		switch p := e.Payload.(type) {
		case core.MessagePayload:
			msgs++
			if p.Message.Content == "and after that?" {
				t.Error("child contains the post-anchor parent turn — branch should fork at the anchor")
			}
		case core.ContextPayload:
			for _, s := range p.Segments {
				if s.Source == SourceBranchAnchor {
					anchorSegs++
				}
			}
		}
	}
	if msgs != 2 {
		t.Errorf("child has %d message events, want 2 (seq 0,1)", msgs)
	}
	if anchorSegs != 1 {
		t.Errorf("child has %d branch-anchor segments, want 1", anchorSegs)
	}

	// Parent has an open anchor pointing at the child.
	pev, _ := store.Events(ctx, pid)
	got, ok := core.LatestBranchAnchor(pev, child)
	if !ok {
		t.Fatal("parent has no anchor for child")
	}
	if got.Status != core.BranchOpen || got.Seq != 1 || got.Quote != anchor.Quote {
		t.Errorf("parent anchor = %+v, want open/seq1/quote", got)
	}
}

func TestAcceptMergesSummaryAndMarksAccepted(t *testing.T) {
	ctx := context.Background()
	store := fake.NewStore()
	pid := seedParent(t, store)
	now := time.Unix(0, 0)
	const child core.SessionID = "child-1"
	if err := Branch(ctx, store, pid, child, core.BranchAnchorPayload{Seq: 1, Quote: "three regions"}, now); err != nil {
		t.Fatalf("Branch: %v", err)
	}

	if err := Accept(ctx, store, pid, child, "the third region is injected context, rendered last", now); err != nil {
		t.Fatalf("Accept: %v", err)
	}

	// Parent projects the curated summary as a fenced branch segment, and the
	// raw child transcript does NOT bleed into the parent model context.
	pev, _ := store.Events(ctx, pid)
	ctxMsgs := core.ProjectContext(pev)
	var found bool
	for _, m := range ctxMsgs {
		if want := "<<" + core.BranchSegmentSource(child) + ">>"; len(m.Content) >= len(want) && m.Content[:len(want)] == want {
			found = true
			if !contains(m.Content, "third region is injected context") {
				t.Errorf("branch segment missing summary text: %q", m.Content)
			}
		}
	}
	if !found {
		t.Error("parent context has no fenced branch segment after accept")
	}

	got, ok := core.LatestBranchAnchor(pev, child)
	if !ok || got.Status != core.BranchAccepted {
		t.Errorf("anchor after accept = %+v ok=%v, want accepted", got, ok)
	}
	if got.Summary == "" {
		t.Error("accepted anchor carries no summary")
	}
}

func TestReacceptSupersedes(t *testing.T) {
	ctx := context.Background()
	store := fake.NewStore()
	pid := seedParent(t, store)
	now := time.Unix(0, 0)
	const child core.SessionID = "child-1"
	if err := Branch(ctx, store, pid, child, core.BranchAnchorPayload{Seq: 1, Quote: "q"}, now); err != nil {
		t.Fatalf("Branch: %v", err)
	}
	if err := Accept(ctx, store, pid, child, "first take", now); err != nil {
		t.Fatalf("accept 1: %v", err)
	}
	if err := Accept(ctx, store, pid, child, "second take", now); err != nil {
		t.Fatalf("accept 2: %v", err)
	}
	pev, _ := store.Events(ctx, pid)
	// Latest-per-source: exactly one branch segment, the second take.
	var n int
	var content string
	for _, m := range core.ProjectContext(pev) {
		if want := "<<" + core.BranchSegmentSource(child) + ">>"; len(m.Content) >= len(want) && m.Content[:len(want)] == want {
			n++
			content = m.Content
		}
	}
	if n != 1 {
		t.Fatalf("got %d branch segments, want 1 (supersede)", n)
	}
	if !contains(content, "second take") || contains(content, "first take") {
		t.Errorf("re-accept did not supersede: %q", content)
	}
}

func TestDiscardMarksDiscardedAndInjectsNothing(t *testing.T) {
	ctx := context.Background()
	store := fake.NewStore()
	pid := seedParent(t, store)
	now := time.Unix(0, 0)
	const child core.SessionID = "child-1"
	if err := Branch(ctx, store, pid, child, core.BranchAnchorPayload{Seq: 1, Quote: "q"}, now); err != nil {
		t.Fatalf("Branch: %v", err)
	}
	if err := Discard(ctx, store, pid, child, now); err != nil {
		t.Fatalf("Discard: %v", err)
	}
	pev, _ := store.Events(ctx, pid)
	got, _ := core.LatestBranchAnchor(pev, child)
	if got.Status != core.BranchDiscarded {
		t.Errorf("anchor after discard = %s, want discarded", got.Status)
	}
	for _, m := range core.ProjectContext(pev) {
		if want := "<<" + core.BranchSegmentSource(child) + ">>"; len(m.Content) >= len(want) && m.Content[:len(want)] == want {
			t.Error("discard injected a branch segment into the parent context")
		}
	}
}

func contains(s, sub string) bool {
	for i := 0; i+len(sub) <= len(s); i++ {
		if s[i:i+len(sub)] == sub {
			return true
		}
	}
	return false
}
