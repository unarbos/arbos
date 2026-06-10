package plan

import (
	"fmt"
	"time"
)

// Standing returns open recurring plan nodes — the "alive" background
// obligations the interactive terminal announces once at session start and
// again only when a new one appears mid-session.
func Standing(nodes []Node, _ time.Time) []Node {
	var out []Node
	for _, n := range nodes {
		if !Terminal(n.Status) && n.Recurring() {
			out = append(out, n)
		}
	}
	return out
}

// FormatStanding renders one standing obligation for the human terminal.
func FormatStanding(n Node, now time.Time) string {
	return fmt.Sprintf("↻ #%d %s%s", n.ID, clipText(n.Goal, standingGoalWidth), dueSuffix(n, now))
}

const standingGoalWidth = 48
