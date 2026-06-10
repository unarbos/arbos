package plan

import (
	"fmt"
	"strings"
	"time"
)

// This file is the HUMAN projection of the forest, as render.go is the
// model's. Two views over the same rows: the model gets the working tree it
// must advance; the human gets the two moments that build trust — arriving
// ("what happened while I was gone, what waits on me") and leaving ("what
// stays open"). Both are pure functions over already-fetched data, so the
// front door renders from SQLite reads alone: the greeting never waits on a
// model.

// Brief renders the front-door retrospective: what finished or ran since the
// human was last seen. The *current* state of the forest — open work, standing
// obligations, timed nodes, questions waiting on the human — is owned by the
// persistent footer (see Footer), which is pinned for the whole session, so the
// greeting no longer reprints it. open seeds the recurrence id lookup; recent is
// nodes updated since lastSeen from ANY plan (a closed mission is exactly the
// headline) and attempts are those since lastSeen. Empty when nothing happened
// while away — the caller prints nothing rather than an empty frame.
func Brief(open, recent []Node, attempts []Attempt, lastSeen, now time.Time) string {
	byID := make(map[NodeID]Node, len(open)+len(recent))
	for _, n := range open {
		byID[n.ID] = n
	}
	for _, n := range recent {
		byID[n.ID] = n
	}

	var b strings.Builder

	// Since you left, tally-style: one header line carrying the counts, then
	// only the terminal flips as lines (recurrence activity folds onto the
	// standing lines below — a goal is never printed twice).
	var flips []string
	for _, n := range recent {
		if !Terminal(n.Status) || n.Recurring() {
			continue
		}
		mark := map[Status]string{StatusDone: "✓", StatusFailed: "✗", StatusCancelled: "-"}[n.Status]
		line := fmt.Sprintf("  %s #%d %s", mark, n.ID, clipText(n.Goal, briefGoalWidth))
		if n.Outcome != "" {
			line += " — " + clipText(n.Outcome, briefGoalWidth)
		}
		flips = append(flips, line)
	}
	counts := recurrenceCounts(attempts, byID)
	fired, failed := 0, 0
	for _, c := range counts {
		fired += c
	}
	for _, a := range attempts {
		if a.Verdict == VerdictFail {
			failed++
		}
	}
	if len(flips) > 0 || fired > 0 {
		header := "while away"
		if !lastSeen.IsZero() {
			header = fmt.Sprintf("since you left (%s)", humanize(now.Sub(lastSeen)))
		}
		if fired > 0 {
			header += fmt.Sprintf(" · fired %d×", fired)
		}
		if failed > 0 {
			header += fmt.Sprintf(" · %d failed", failed)
		}
		b.WriteString(header + "\n")
		if len(flips) > 0 {
			b.WriteString(strings.Join(flips, "\n") + "\n")
		}
	}

	return strings.TrimRight(b.String(), "\n")
}

// briefGoalWidth caps goal and outcome text in the human views: the greeting
// is a glance, not a document — the ids are there for anyone who wants depth.
const briefGoalWidth = 64

// recurrenceCounts tallies attempts per maintain node — "commit hourly ran
// 3×" — ignoring attempts on achieve nodes, whose terminal flip already tells
// the story.
func recurrenceCounts(attempts []Attempt, byID map[NodeID]Node) map[NodeID]int {
	counts := map[NodeID]int{}
	for _, a := range attempts {
		if n, ok := byID[a.Node]; ok && n.Recurring() {
			counts[a.Node]++
		}
	}
	return counts
}

func humanize(d time.Duration) string {
	switch {
	case d < time.Minute:
		return fmt.Sprintf("%ds", max(int(d.Seconds()), 1))
	case d < time.Hour:
		return fmt.Sprintf("%dm", int(d.Minutes()))
	case d < 48*time.Hour:
		return fmt.Sprintf("%dh", int(d.Hours()))
	}
	return fmt.Sprintf("%dd", int(d.Hours()/24))
}
