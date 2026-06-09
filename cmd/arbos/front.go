package main

import (
	"context"
	"fmt"
	"io"
	"time"

	"github.com/unarbos/arbos/internal/plan"
	"github.com/unarbos/arbos/internal/sqlite"
	"github.com/unarbos/arbos/internal/tool/codingspec"
)

// The front door (the arriving moment) and the handoff (the leaving moment).
// Both render from SQLite reads and the on-disk job table alone — never a
// model call — so the greeting is instant and the exit never blocks. Empty
// states print nothing: a fresh install greets with silence, not a frame.

// printBrief renders the since-you-left / waiting-on-you / open greeting on a
// bare interactive `arbos`. Best-effort: any store error just skips the brief.
func printBrief(ctx context.Context, store *sqlite.Store, cwd string, w io.Writer) {
	now := time.Now()
	lastSeen, err := store.LastHumanSeen(ctx)
	if err != nil {
		return
	}
	open, err := store.OpenPlanNodes(ctx)
	if err != nil {
		return
	}
	recent, _ := store.PlanNodesUpdatedSince(ctx, lastSeen)
	attempts, _ := store.PlanAttemptsSince(ctx, lastSeen)

	out := plan.Brief(open, recent, attempts, lastSeen, now)
	if jobs := codingspec.JobsBrief(cwd); jobs != "" {
		if out != "" {
			out += "\n"
		}
		out += jobs
	}
	if out != "" {
		_, _ = fmt.Fprintln(w, out)
		_, _ = fmt.Fprintln(w)
	}
}

// printHandoff renders what stays open as the human leaves. Read fresh: the
// session that just ended usually changed the forest.
func printHandoff(ctx context.Context, store *sqlite.Store, w io.Writer) {
	open, err := store.OpenPlanNodes(ctx)
	if err != nil {
		return
	}
	if out := plan.Handoff(open, time.Now()); out != "" {
		_, _ = fmt.Fprintln(w, "\n"+out)
	}
}
