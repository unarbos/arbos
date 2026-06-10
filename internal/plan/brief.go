package plan

import (
	"fmt"
	"strings"
)

// This file is the HUMAN projection of the forest, as render.go is the
// model's. Two views over the same rows: the model gets the working tree it
// must advance; the human gets the two moments that build trust — arriving
// ("what happened while I was gone, what waits on me") and leaving ("what
// stays open"). Both are pure functions over already-fetched data, so the
// front door renders from SQLite reads alone: the greeting never waits on a
// model.

// Brief renders the front-door retrospective: the one-shot missions that
// reached a terminal state since the human was last seen, as self-explanatory
// ✓/✗/- lines. Recurring activity is deliberately not summarized — the
// standing ↻ announcement already names what keeps firing. Empty when nothing
// flipped while away — the caller prints nothing rather than an empty frame.
func Brief(recent []Node) string {
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
	return strings.Join(flips, "\n")
}

// briefGoalWidth caps goal and outcome text in the human views: the greeting
// is a glance, not a document — the ids are there for anyone who wants depth.
const briefGoalWidth = 64
