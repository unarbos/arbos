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
