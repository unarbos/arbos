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

// Brief renders the front-door greeting: what finished or ran since the human
// was last seen, what waits on them, and what is open now. open is the working
// forest; recent is nodes updated since lastSeen from ANY plan — a closed
// mission is exactly the headline — and attempts are those since lastSeen.
// Empty when there is nothing worth saying — the caller prints nothing rather
// than an empty frame.
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

	writeOpenSections(&b, open, counts, now)
	return strings.TrimRight(b.String(), "\n")
}

// briefGoalWidth caps goal and outcome text in the human views: the greeting
// is a glance, not a document — the ids are there for anyone who wants depth.
const briefGoalWidth = 64

// Handoff renders the leaving summary: what stays open when the human walks
// away. Empty when nothing is open.
func Handoff(nodes []Node, now time.Time) string {
	var b strings.Builder
	writeOpenSections(&b, nodes, nil, now)
	if b.Len() == 0 {
		return ""
	}
	out := "left open:\n" + b.String()
	if hasArmed(nodes) {
		out += "(scheduled items fire while `arbos -serve` is running)\n"
	}
	return strings.TrimRight(out, "\n")
}

// writeOpenSections renders the shared trunk of both moments: open work,
// timed work that has not fired yet, standing obligations, and what waits on
// the human. Deferred nodes render with their countdown — the user who just
// said "in 20 seconds…" must see that promise acknowledged at every door,
// especially the one they are walking out of. counts annotates standing lines
// with their recent activity (nil for the handoff, where nothing has run
// "since" anything).
func writeOpenSections(b *strings.Builder, nodes []Node, counts map[NodeID]int, now time.Time) {
	hasChildren := map[NodeID]bool{}
	for _, n := range nodes {
		hasChildren[n.Parent] = true
	}
	var open, timed, standing, human []string
	for _, n := range nodes {
		switch {
		case Terminal(n.Status):
		case n.Recurring():
			line := fmt.Sprintf("  ▸ #%d %s", n.ID, clipText(n.Goal, briefGoalWidth))
			if c := counts[n.ID]; c > 0 {
				line += fmt.Sprintf(" · ran %d×", c)
			}
			standing = append(standing, line+dueSuffix(n, now))
		case n.Assignee == AssigneeHuman:
			human = append(human, fmt.Sprintf("  ? #%d %s", n.ID, clipText(n.Goal, briefGoalWidth)))
		case n.Status == StatusPending && !n.After.IsZero() && now.Before(n.After):
			timed = append(timed, fmt.Sprintf("  ▸ #%d %s · fires in ~%s", n.ID, clipText(n.Goal, briefGoalWidth), humanize(n.After.Sub(now))))
		case n.Parent == 0 && hasChildren[n.ID]:
			// A decomposed root is a heading; its children carry the state. A
			// childless root is itself the work and falls through below.
		case n.Status == StatusActive || Ready(n, GatedBySibling(nodes, n), now):
			open = append(open, fmt.Sprintf("  ▸ #%d [%s] %s", n.ID, statusWord(n, now), clipText(n.Goal, briefGoalWidth)))
		}
	}
	if len(human) > 0 {
		b.WriteString("waiting on you:\n" + strings.Join(human, "\n") + "\n")
	}
	if len(open) > 0 {
		b.WriteString("open:\n" + strings.Join(open, "\n") + "\n")
	}
	if len(timed) > 0 {
		b.WriteString("timed:\n" + strings.Join(timed, "\n") + "\n")
	}
	if len(standing) > 0 {
		b.WriteString("standing:\n" + strings.Join(standing, "\n") + "\n")
	}
}

func statusWord(n Node, now time.Time) string {
	if n.Status == StatusActive {
		return "active"
	}
	if !n.After.IsZero() && !now.Before(n.After) {
		return "due"
	}
	return "ready"
}

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

// hasArmed reports whether any open node carries a scheduler-owned trigger, so
// the handoff can note that those items only fire while `arbos -serve` runs.
func hasArmed(nodes []Node) bool {
	for _, n := range nodes {
		if !Terminal(n.Status) && n.Armed() {
			return true
		}
	}
	return false
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
