// Package sessiontree implements pi's session-tree fork over any
// ports.SessionStore. arbos models pi's in-session entry tree as a set of
// parent-linked sessions: a branch is an independent, complete session log. This
// needs no event-log schema change because core.Session already reserves
// ParentID for fork lineage. It lives in its own package so both the engine
// (for the control-seam RPC) and the agent layer can reach it without an import
// cycle. A whole-log copy is just Fork with a negative throughSeq (what the
// control seam's `fork` frame does when through_seq is omitted), so no separate
// clone operation is needed.
package sessiontree

import (
	"context"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/ports"
)

// Fork creates newID as a branch of source, copying source's events with
// seq <= throughSeq (a negative throughSeq copies the whole log) and recording
// source as the parent. Copied events keep their payload, turn id, version, and
// timestamp; the store reassigns fresh per-branch seq/id on append.
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
		ID:        newID,
		ParentID:  source,
		Status:    core.SessionActive,
		Model:     src.Model,
		CreatedAt: now,
		UpdatedAt: now,
	}); err != nil {
		return err
	}
	for i := range events {
		ev := events[i]
		if throughSeq >= 0 && ev.Seq > throughSeq {
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
