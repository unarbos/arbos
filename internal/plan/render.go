package plan

import (
	"fmt"
	"strings"
	"time"
)

// renderBudgetChars caps the injected plan block (~600 tokens). The projection
// is peripheral vision, not the tree: open work in full, finished work as
// counts, and a hard ceiling so a sprawling mission cannot flood the prompt.
const renderBudgetChars = 2400

// NoPlanContext is the rendering of an empty forest. Like
// codingspec.NoJobsContext, it exists so the injector can supersede a stale
// table without ever teaching a plan-less session about plans.
const NoPlanContext = "(no active plans)"

// Render projects the working forest into the per-turn context segment. Shape,
// per open plan: the mission line with done counts, then open nodes
// depth-first (done/cancelled children are folded into the parent's count),
// then standing (maintain) obligations, then nodes waiting on the human. A
// [ready] marker means the node can be worked on now; [due] means a deferred
// or recurring node's time has arrived.
func Render(nodes []Node, now time.Time) string {
	if len(nodes) == 0 {
		return NoPlanContext
	}
	children := map[NodeID][]Node{}
	var roots []Node
	for _, n := range nodes {
		if n.Parent == 0 {
			roots = append(roots, n)
			continue
		}
		children[n.Parent] = append(children[n.Parent], n)
	}

	var b strings.Builder
	for _, root := range roots {
		renderPlan(&b, root, children, now)
		if b.Len() > renderBudgetChars {
			b.WriteString("… (plan truncated; use the plan tool with op:show for the rest)\n")
			break
		}
	}
	return strings.TrimRight(b.String(), "\n")
}

func renderPlan(b *strings.Builder, root Node, children map[NodeID][]Node, now time.Time) {
	done, total := countAchieve(root.ID, children)
	fmt.Fprintf(b, "#%d %s", root.ID, root.Goal)
	if total > 0 {
		fmt.Fprintf(b, "  (%d/%d done)", done, total)
	}
	b.WriteString("\n")

	var standing, human []Node
	renderOpen(b, root.ID, children, now, 1, &standing, &human)

	if len(standing) > 0 {
		b.WriteString("  standing:\n")
		for _, n := range standing {
			fmt.Fprintf(b, "    #%d %s%s\n", n.ID, n.Goal, dueSuffix(n, now))
		}
	}
	if len(human) > 0 {
		b.WriteString("  waiting on the user:\n")
		for _, n := range human {
			fmt.Fprintf(b, "    #%d %s\n", n.ID, n.Goal)
		}
	}
}

// renderOpen walks a subtree depth-first, printing open achieve nodes and
// collecting maintain and human-assigned nodes for their own sections.
// Terminal nodes are folded into their parent's done count and not printed.
func renderOpen(b *strings.Builder, parent NodeID, children map[NodeID][]Node, now time.Time, depth int, standing, human *[]Node) {
	siblings := children[parent]
	for _, n := range siblings {
		switch {
		case n.Kind == KindMaintain:
			if !Terminal(n.Status) {
				*standing = append(*standing, n)
			}
			continue
		case n.Assignee == AssigneeHuman:
			if !Terminal(n.Status) {
				*human = append(*human, n)
			}
			continue
		case Terminal(n.Status):
			continue
		}
		marker := string(n.Status)
		if Ready(n, GatedBySibling(siblings, n), now) {
			marker = "ready"
			if !n.After.IsZero() {
				marker = "due"
			}
		}
		fmt.Fprintf(b, "%s#%d [%s] %s", strings.Repeat("  ", depth), n.ID, marker, n.Goal)
		// A deferred node's timer is part of its state: without it the model
		// cannot answer "what is scheduled?". Absolute time, not a countdown,
		// so the rendering is stable between state changes (ADR-0015 churn).
		if n.Status == StatusPending && !n.After.IsZero() && now.Before(n.After) {
			fmt.Fprintf(b, "  (fires ~%s)", n.After.Format("15:04:05"))
		}
		if n.Cmd != "" {
			fmt.Fprintf(b, "  (cmd: %s)", clipText(n.Cmd, 48))
		}
		if n.Check != "" {
			fmt.Fprintf(b, "  (check: %s)", n.Check)
		}
		if n.Status == StatusBlocked && n.Outcome != "" {
			fmt.Fprintf(b, "  — %s", n.Outcome)
		}
		b.WriteString("\n")
		renderOpen(b, n.ID, children, now, depth+1, standing, human)
	}
}

// countAchieve counts terminal-done and total achieve nodes in a subtree, for
// the mission line's progress fraction.
func countAchieve(parent NodeID, children map[NodeID][]Node) (done, total int) {
	for _, n := range children[parent] {
		if n.Kind == KindAchieve && n.Assignee == AssigneeAgent {
			total++
			if n.Status == StatusDone {
				done++
			}
		}
		d, t := countAchieve(n.ID, children)
		done, total = done+d, total+t
	}
	return done, total
}

// clipText truncates to n runes with an ellipsis, single-line.
func clipText(s string, n int) string {
	if i := strings.IndexByte(s, '\n'); i >= 0 {
		s = s[:i] + " …"
	}
	runes := []rune(s)
	if len(runes) <= n {
		return s
	}
	return string(runes[:n-1]) + "…"
}

func dueSuffix(n Node, now time.Time) string {
	if n.NextDue.IsZero() {
		return ""
	}
	if !now.Before(n.NextDue) {
		return "  [due now]"
	}
	return fmt.Sprintf("  (next due %s)", n.NextDue.Format("15:04"))
}
