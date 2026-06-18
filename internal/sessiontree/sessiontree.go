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
	"strings"
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

// Branch opens an anchored sub-discussion scoped to a highlighted FRAGMENT and
// its containing message — NOT a fork of the whole conversation. The child is a
// fresh session (no parent turns copied in), seeded with a single ContextPayload
// segment that quotes the message the highlight came from and calls out the
// highlighted span within it. So the agent discusses the fragment with its own
// message as the surrounding context, and sees nothing else from the thread —
// no earlier turns, no later turns. It writes two records:
//
//   - on the PARENT: a BranchAnchorPayload (Status open) marking where the
//     highlight is and which child it opened, so the parent UI can render the
//     anchor and later resolve it on accept;
//   - on the CHILD: a ContextPayload segment (SourceBranchAnchor) carrying the
//     containing message + the highlighted fragment, injected (not a visible
//     message) so it reaches the model as fenced context.
//
// anchor.Branch must equal newID; anchor.Quote is the highlighted text and
// anchor.Message the full containing message (the caller, which has the rendered
// transcript, supplies both). anchor.Seq locates the anchor on the parent for
// the marker. Failures after CreateSession leave the child in place (the caller
// decides whether to retry or abandon it).
func Branch(ctx context.Context, store ports.SessionStore, source, newID core.SessionID, anchor core.BranchAnchorPayload, now time.Time) error {
	src, err := store.Get(ctx, source)
	if err != nil {
		return err
	}
	if err := store.CreateSession(ctx, core.Session{
		ID:        newID,
		ParentID:  source,
		Status:    core.SessionActive,
		Model:     src.Model,
		WebSearch: src.WebSearch,
		WebFetch:  src.WebFetch,
		ImageGen:  src.ImageGen,
		CreatedAt: now,
		UpdatedAt: now,
	}); err != nil {
		return err
	}
	anchor.Branch = newID
	anchor.Status = core.BranchOpen
	anchor.Summary = ""
	pa := core.NewBranchAnchorEvent(source, anchor, now)
	if err := store.AppendEvent(ctx, pa); err != nil {
		return err
	}
	seg := core.NewContextEvent(newID, []core.Segment{{Source: SourceBranchAnchor, Content: branchSeed(anchor)}}, now)
	return store.AppendEvent(ctx, seg)
}

// branchSeed renders the child's framing context: the containing message as the
// scope, the highlighted fragment called out within it, and the merge-back
// contract. When the containing message is unavailable (Message empty) it falls
// back to the fragment alone.
func branchSeed(a core.BranchAnchorPayload) string {
	var b strings.Builder
	b.WriteString("The user opened a focused side-discussion about a highlighted fragment of an earlier message. ")
	b.WriteString("Discuss ONLY this fragment; the surrounding conversation is intentionally not included.\n\n")
	b.WriteString("Highlighted fragment:\n\u201c")
	b.WriteString(a.Quote)
	b.WriteString("\u201d\n")
	if msg := strings.TrimSpace(a.Message); msg != "" && msg != strings.TrimSpace(a.Quote) {
		b.WriteString("\nIt appeared in this message (for context only):\n\u201c")
		b.WriteString(msg)
		b.WriteString("\u201d\n")
	}
	b.WriteString("\nWhen the user accepts, only a short curated conclusion is carried back to the main thread.")
	return b.String()
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
