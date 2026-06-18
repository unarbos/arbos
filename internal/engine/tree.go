package engine

import (
	"context"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/sessiontree"
)

// ForkSession branches source into newID, copying its events with seq <=
// throughSeq (negative = an empty branch) and recording the lineage. It is a
// control-plane operation over the store (no running turn) — the arbos mapping
// of pi's RPC fork. The caller is responsible for not forking a session that is
// being actively written by a live actor.
func (e *Engine) ForkSession(ctx context.Context, source, newID core.SessionID, throughSeq int64) error {
	return sessiontree.Fork(ctx, e.store, source, newID, throughSeq, e.clock.Now())
}

// BranchSession opens an anchored sub-discussion scoped to the highlighted
// fragment and its containing message (anchor.Quote / anchor.Message): it
// creates a FRESH child (no parent turns copied), records the anchor on the
// parent, and seeds the child with that fragment as its context. The child is a
// live session the caller binds on a separate door (a sibling tab), so the
// parent stays open beside it. A control-plane store operation over an idle
// source.
func (e *Engine) BranchSession(ctx context.Context, source, newID core.SessionID, anchor core.BranchAnchorPayload) error {
	return sessiontree.Branch(ctx, e.store, source, newID, anchor, e.clock.Now())
}

// AcceptBranch resolves an open branch by merging summary back into parent as a
// fenced context segment and marking the anchor accepted. The full child
// transcript is left intact.
func (e *Engine) AcceptBranch(ctx context.Context, parent, branch core.SessionID, summary string) error {
	return sessiontree.Accept(ctx, e.store, parent, branch, summary, e.clock.Now())
}

// DiscardBranch resolves an open branch without merging anything back, marking
// the anchor discarded so the parent UI can close it.
func (e *Engine) DiscardBranch(ctx context.Context, parent, branch core.SessionID) error {
	return sessiontree.Discard(ctx, e.store, parent, branch, e.clock.Now())
}
