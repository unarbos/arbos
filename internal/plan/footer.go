package plan

import (
	"fmt"
	"strings"
	"time"
)

// Footer renders the always-visible status strip the interactive terminal pins
// beneath its prompt: standing obligations with their next fire, the soonest
// timed node, and a count of open work and questions waiting on the human. It
// is the persistent glance kept *while working*, distinct from Brief (the
// arriving moment) and Handoff (the leaving moment) — so the schedule stops
// being state that is reprinted at every door and becomes state you look at.
// At most a handful of lines; empty when the forest has nothing to show.
func Footer(nodes []Node, now time.Time) []string {
	hasChildren := map[NodeID]bool{}
	for _, n := range nodes {
		hasChildren[n.Parent] = true
	}

	var standing []Node
	var nextTimed *Node
	open, human := 0, 0
	for i := range nodes {
		n := nodes[i]
		switch {
		case Terminal(n.Status):
		case n.Recurring():
			standing = append(standing, n)
		case n.Assignee == AssigneeHuman:
			human++
		case n.Status == StatusPending && !n.After.IsZero() && now.Before(n.After):
			if nextTimed == nil || n.After.Before(nextTimed.After) {
				nextTimed = &nodes[i]
			}
		case n.Parent == 0 && hasChildren[n.ID]:
			// A decomposed root is a heading; its children carry the state.
		case n.Status == StatusActive || Ready(n, GatedBySibling(nodes, n), now):
			open++
		}
	}

	var lines []string
	for i, n := range standing {
		if i == footerMaxStanding {
			lines = append(lines, fmt.Sprintf("  ↻ +%d more standing", len(standing)-footerMaxStanding))
			break
		}
		lines = append(lines, fmt.Sprintf("  ↻ #%d %s%s", n.ID, clipText(n.Goal, footerGoalWidth), dueSuffix(n, now)))
	}
	if nextTimed != nil {
		lines = append(lines, fmt.Sprintf("  ⧗ #%d %s · fires in ~%s",
			nextTimed.ID, clipText(nextTimed.Goal, footerGoalWidth), humanize(nextTimed.After.Sub(now))))
	}
	if summary := footerSummary(open, human); summary != "" {
		lines = append(lines, "  "+summary)
	}
	return lines
}

const (
	footerMaxStanding = 2
	footerGoalWidth   = 48
)

func footerSummary(open, human int) string {
	var parts []string
	if open > 0 {
		parts = append(parts, fmt.Sprintf("%d open", open))
	}
	if human > 0 {
		parts = append(parts, fmt.Sprintf("%d waiting on you", human))
	}
	return strings.Join(parts, " · ")
}
