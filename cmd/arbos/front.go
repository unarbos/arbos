package main

import (
	"context"
	"fmt"
	"io"
	"time"

	"github.com/unarbos/arbos/internal/plan"
	"github.com/unarbos/arbos/internal/sqlite"
)

// The front door (the arriving moment) and the handoff (the leaving moment).
// Both render from SQLite reads and the on-disk job table alone — never a
// model call — so the greeting is instant and the exit never blocks. Empty
// states print nothing: a fresh install greets with silence, not a frame.

// printBrief renders the front-door retrospective on a bare interactive
// `arbos`: the missions that finished since the human was last seen. Standing
// obligations are announced separately once at session start (and again only
// when a new one appears). Best-effort: any store error just skips the brief.
func printBrief(ctx context.Context, store *sqlite.Store, w io.Writer) {
	lastSeen, err := store.LastHumanSeen(ctx)
	if err != nil {
		return
	}
	recent, _ := store.PlanNodesUpdatedSince(ctx, lastSeen)

	if out := plan.Brief(recent); out != "" {
		// The brief embeds model-authored goals and outcomes — the same
		// prompt-injection-reachable text the outbox sanitizes — so strip
		// control bytes before it reaches the terminal.
		_, _ = fmt.Fprintln(w, sanitizeNotice(out))
		_, _ = fmt.Fprintln(w)
	}
}

// collectStandingLines returns formatted standing-obligation lines. When
// onlyNew is set, nodes already in known are skipped — for mid-session
// announcements of newly created recurring work.
func collectStandingLines(open []plan.Node, now time.Time, known map[plan.NodeID]bool, onlyNew bool) []string {
	var lines []string
	for _, n := range plan.Standing(open, now) {
		if onlyNew && known[n.ID] {
			continue
		}
		known[n.ID] = true
		lines = append(lines, sanitizeNotice(plan.FormatStanding(n, now)))
	}
	return lines
}
