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

// printBrief renders the front-door retrospective on a bare interactive
// `arbos`: what finished or ran since the human was last seen. The current
// state of the forest (open work, standing obligations, timed nodes, running
// jobs) is owned by the persistent footer, which stays pinned for the whole
// session, so the greeting no longer reprints it. Best-effort: any store error
// just skips the brief.
func printBrief(ctx context.Context, store *sqlite.Store, w io.Writer) {
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

	if out := plan.Brief(open, recent, attempts, lastSeen, now); out != "" {
		// The brief embeds model-authored goals and outcomes — the same
		// prompt-injection-reachable text the outbox sanitizes — so strip
		// control bytes before it reaches the terminal.
		_, _ = fmt.Fprintln(w, sanitizeNotice(out))
		_, _ = fmt.Fprintln(w)
	}
}

// footerLines builds the persistent status strip the live terminal pins
// beneath its prompt: the plan forest's standing/timed/open glance plus a
// count of running background jobs. Pure store and on-disk reads, like the
// brief and handoff — never a model call — so refreshing it is instant. Nil
// when there is nothing to show (a fresh tree pins no footer).
func footerLines(ctx context.Context, store *sqlite.Store, cwd string) []string {
	if store == nil {
		return nil
	}
	open, err := store.OpenPlanNodes(ctx)
	if err != nil {
		return nil
	}
	lines := plan.Footer(open, time.Now())
	if jobs := codingspec.JobsRunningCount(cwd); jobs > 0 {
		noun := "job"
		if jobs > 1 {
			noun = "jobs"
		}
		lines = append(lines, fmt.Sprintf("  ⚙ %d running %s", jobs, noun))
	}
	// The footer can embed model-authored goal text; strip control bytes for
	// the same reason the outbox and brief do.
	for i, ln := range lines {
		lines[i] = sanitizeNotice(ln)
	}
	return lines
}
