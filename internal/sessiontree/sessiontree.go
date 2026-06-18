// Package sessiontree implements pi's session-tree fork over any
// ports.SessionStore. arbos models pi's in-session entry tree as a set of
// parent-linked sessions: a branch is an independent, complete session log. This
// needs no event-log schema change because core.Session already reserves
// ParentID for fork lineage. It lives in its own package so both the engine
// (for the control-seam RPC) and the agent layer can reach it without an import
// cycle. A whole-log copy is just Fork with a throughSeq at or past the last
// event's seq (what the control seam's `fork` frame does when through_seq is
// omitted), so no separate clone operation is needed.
package sessiontree

import (
	"context"
	"fmt"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/ports"
)

// Fork creates newID as a branch of source, copying source's events with
// seq <= throughSeq (a negative throughSeq copies nothing — an empty branch,
// the rewind-before-first-message case) and recording source as the parent.
// Copied events keep their payload, turn id, version, and timestamp; the store
// reassigns fresh per-branch seq/id on append.
func Fork(ctx context.Context, store ports.SessionStore, source, newID core.SessionID, throughSeq int64, now time.Time) error {
	src, err := store.Get(ctx, source)
	if err != nil {
		return err
	}
	events, err := store.Events(ctx, source)
	if err != nil {
		return err
	}
	if err := store.CreateSession(ctx, core.Session{
		ID:       newID,
		ParentID: source,
		Status:   core.SessionActive,
		Model:    src.Model,
		// The provider toggles travel with the conversation like the model:
		// a rewind-and-edit fork that silently disarmed web search would
		// contradict the toggle the user still sees armed.
		WebSearch: src.WebSearch,
		WebFetch:  src.WebFetch,
		ImageGen:  src.ImageGen,
		CreatedAt: now,
		UpdatedAt: now,
	}); err != nil {
		return err
	}
	for i := range events {
		ev := events[i]
		if ev.Seq > throughSeq {
			break
		}
		cp := core.Event{
			SessionID: newID,
			TurnID:    ev.TurnID,
			Version:   ev.Version,
			CreatedAt: ev.CreatedAt,
			Payload:   ev.Payload,
		}
		if err := store.AppendEvent(ctx, &cp); err != nil {
			return err
		}
	}
	return nil
}

// SourceBranchAnchor is the Segment.Source seeded into a freshly opened branch
// so the child agent knows it is in a focused side-discussion about a specific
// highlighted fragment, without that framing polluting the visible message
// stream. Like every injected segment it is fenced and superseded per source.
const SourceBranchAnchor = "branch_anchor"

// Branch opens an anchored sub-discussion about a highlighted span of source.
// It is Fork(throughSeq = anchor.Seq) — the child sees the parent's history up
// to and including the event the highlight lives in, but nothing after, so the
// side-thread is a genuine "step aside at this point" — followed by two records:
//
//   - on the PARENT: a BranchAnchorPayload (Status open) marking where the
//     highlight is and which child it opened, so the parent UI can render the
//     anchor and later resolve it on accept;
//   - on the CHILD: a ContextPayload segment (SourceBranchAnchor) framing the
//     side-discussion around the quoted fragment, injected (not a visible
//     message) so it reaches the model as fenced context.
//
// anchor.Branch must equal newID and anchor.Seq must be a valid source seq; the
// caller (engine) sets these. Failures after the fork leave the child session
// in place (the caller decides whether to retry or abandon it).
func Branch(ctx context.Context, store ports.SessionStore, source, newID core.SessionID, anchor core.BranchAnchorPayload, now time.Time) error {
	if err := Fork(ctx, store, source, newID, anchor.Seq, now); err != nil {
		return err
	}
	anchor.Branch = newID
	anchor.Status = core.BranchOpen
	anchor.Summary = ""
	pa := core.NewBranchAnchorEvent(source, anchor, now)
	if err := store.AppendEvent(ctx, pa); err != nil {
		return err
	}
	seed := "You are in a focused side-discussion the user opened about this highlighted fragment of the conversation:\n\n\u201c" + anchor.Quote + "\u201d\n\nDiscuss it in depth. When the user accepts, only a short curated conclusion is carried back to the main thread."
	seg := core.NewContextEvent(newID, []core.Segment{{Source: SourceBranchAnchor, Content: seed}}, now)
	return store.AppendEvent(ctx, seg)
}

// Accept resolves an open branch: it appends the curated summary to the PARENT
// as a ContextPayload segment under the branch's unique source (so it projects
// as a fenced block and a re-accept supersedes rather than accumulates), and
// records the resolution as a BranchAnchorPayload (Status accepted) carrying the
// summary. The full child transcript persists untouched — only the conclusion
// crosses back, the delegation contract made durable and user-curated.
//
// It reads the latest anchor for branch from the parent log to recover Seq and
// Quote, so the caller need only supply parent, branch, and the (possibly
// user-edited) summary.
func Accept(ctx context.Context, store ports.SessionStore, parent, branch core.SessionID, summary string, now time.Time) error {
	events, err := store.Events(ctx, parent)
	if err != nil {
		return err
	}
	anchor, ok := core.LatestBranchAnchor(events, branch)
	if !ok {
		return fmt.Errorf("accept branch %q: no anchor on parent %q", branch, parent)
	}
	seg := core.NewContextEvent(parent, []core.Segment{{
		Source:  core.BranchSegmentSource(branch),
		Content: "Resolved side-discussion about \u201c" + anchor.Quote + "\u201d:\n" + summary,
	}}, now)
	if err := store.AppendEvent(ctx, seg); err != nil {
		return err
	}
	anchor.Status = core.BranchAccepted
	anchor.Summary = summary
	return store.AppendEvent(ctx, core.NewBranchAnchorEvent(parent, anchor, now))
}

// Discard resolves an open branch without merging anything back: it records a
// BranchAnchorPayload (Status discarded) on the parent so the UI can mark the
// anchor closed. The child transcript persists (reopenable); no context segment
// is injected, so the parent's model context is unchanged.
func Discard(ctx context.Context, store ports.SessionStore, parent, branch core.SessionID, now time.Time) error {
	events, err := store.Events(ctx, parent)
	if err != nil {
		return err
	}
	anchor, ok := core.LatestBranchAnchor(events, branch)
	if !ok {
		return fmt.Errorf("discard branch %q: no anchor on parent %q", branch, parent)
	}
	anchor.Status = core.BranchDiscarded
	return store.AppendEvent(ctx, core.NewBranchAnchorEvent(parent, anchor, now))
}
