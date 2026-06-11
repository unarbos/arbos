package plan

import (
	"testing"
	"time"
)

// The plan package's policy is pure by design (Ready, GatedBySibling,
// Fireable, CanTransition, AssignSeqs) precisely so it can be tested without a
// store or a clock. These tests pin the firing semantics — most importantly
// that Fireable stamps the wake reason from un-mutated fields, the regression
// that previously made every deferred task fire with the callback prompt.

func node(id NodeID, mod func(*Node)) Node {
	n := Node{ID: id, Plan: 1, Status: StatusPending, Assignee: AssigneeAgent}
	if mod != nil {
		mod(&n)
	}
	return n
}

func TestFireable_ClassifiesReasonBeforeMutation(t *testing.T) {
	now := time.Date(2026, 6, 9, 12, 0, 0, 0, time.UTC)
	past := now.Add(-time.Minute)

	deferred := node(2, func(n *Node) { n.After = past }) // timer elapsed
	maintain := node(3, func(n *Node) { n.Every = time.Hour; n.NextDue = past })
	callback := node(4, func(n *Node) { n.WakeOnReady = true }) // gates clear, no timer
	cmd := node(5, func(n *Node) { n.Cmd = "echo hi" })
	future := node(6, func(n *Node) { n.After = now.Add(time.Hour) }) // not yet due

	cmds, _, wakes := Fireable([]Node{deferred, maintain, callback, cmd, future}, now)

	if len(cmds) != 1 || cmds[0].ID != 5 {
		t.Fatalf("cmds = %+v, want just #5", cmds)
	}
	got := map[NodeID]WakeReason{}
	for _, w := range wakes {
		got[w.Node.ID] = w.Reason
	}
	want := map[NodeID]WakeReason{2: WakeDue, 3: WakeDue, 4: WakeReady}
	if len(got) != len(want) {
		t.Fatalf("wakes = %+v, want %+v", got, want)
	}
	for id, r := range want {
		if got[id] != r {
			t.Fatalf("node #%d reason = %q, want %q", id, got[id], r)
		}
	}
}

func TestFireable_DueOrdering(t *testing.T) {
	now := time.Date(2026, 6, 9, 12, 0, 0, 0, time.UTC)
	a := node(2, func(n *Node) { n.After = now.Add(-3 * time.Minute) })
	b := node(3, func(n *Node) { n.After = now.Add(-1 * time.Minute) })
	c := node(4, func(n *Node) { n.After = now.Add(-2 * time.Minute) })
	_, _, wakes := Fireable([]Node{b, c, a}, now)
	order := []NodeID{}
	for _, w := range wakes {
		order = append(order, w.Node.ID)
	}
	if len(order) != 3 || order[0] != 2 || order[1] != 4 || order[2] != 3 {
		t.Fatalf("due order = %v, want [2 4 3] (oldest-due first)", order)
	}
}

func TestReady_GatesAndAssignment(t *testing.T) {
	now := time.Date(2026, 6, 9, 12, 0, 0, 0, time.UTC)
	cases := []struct {
		name  string
		n     Node
		gated bool
		want  bool
	}{
		{"plain pending", node(1, nil), false, true},
		{"gated by sibling", node(1, nil), true, false},
		{"human-assigned parks", node(1, func(n *Node) { n.Assignee = AssigneeHuman }), false, false},
		{"active is not ready", node(1, func(n *Node) { n.Status = StatusActive }), false, false},
		{"deferred not yet", node(1, func(n *Node) { n.After = now.Add(time.Hour) }), false, false},
		{"deferred elapsed", node(1, func(n *Node) { n.After = now.Add(-time.Hour) }), false, true},
		{"maintain due", node(1, func(n *Node) { n.Every = time.Hour; n.NextDue = now.Add(-time.Minute) }), false, true},
		{"maintain not due", node(1, func(n *Node) { n.Every = time.Hour; n.NextDue = now.Add(time.Minute) }), false, false},
	}
	for _, c := range cases {
		if got := Ready(c.n, c.gated, now); got != c.want {
			t.Errorf("%s: Ready = %v, want %v", c.name, got, c.want)
		}
	}
}

func TestGatedBySibling_ParallelGroup(t *testing.T) {
	// Seq 1,1,2: the two at seq 1 do not gate each other; seq 2 is gated until
	// both finish. Maintain siblings never gate.
	a := Node{ID: 1, Parent: 9, Seq: 1, Status: StatusActive}
	b := Node{ID: 2, Parent: 9, Seq: 1, Status: StatusPending}
	join := Node{ID: 3, Parent: 9, Seq: 2, Status: StatusPending}
	sibs := []Node{a, b, join}
	if GatedBySibling(sibs, b) {
		t.Error("equal-seq sibling should not gate")
	}
	if !GatedBySibling(sibs, join) {
		t.Error("join should be gated while seq-1 group is open")
	}
	a.Status, b.Status = StatusDone, StatusDone
	if GatedBySibling([]Node{a, b, join}, join) {
		t.Error("join should be ungated once the group is done")
	}
}

func TestCanTransition(t *testing.T) {
	// recurring is expressed by Every (no separate kind).
	mk := func(s Status, every time.Duration) Node { return Node{ID: 1, Status: s, Every: every} }
	ok := func(from Status, to Status, every time.Duration) {
		if err := CanTransition(mk(from, every), to); err != nil {
			t.Errorf("%s->%s (every=%v) should be allowed: %v", from, to, every, err)
		}
	}
	bad := func(from Status, to Status, every time.Duration) {
		if err := CanTransition(mk(from, every), to); err == nil {
			t.Errorf("%s->%s (every=%v) should be rejected", from, to, every)
		}
	}
	ok(StatusPending, StatusActive, 0)
	ok(StatusActive, StatusDone, 0)
	ok(StatusDone, StatusPending, 0) // regression reopens
	ok(StatusFailed, StatusActive, 0)
	bad(StatusCancelled, StatusActive, 0)    // cancelled is final
	bad(StatusActive, StatusActive, 0)       // no-op
	bad(StatusActive, StatusDone, time.Hour) // recurring has no terminal success
	bad(StatusActive, StatusFailed, time.Hour)
}

func TestAssignSeqs(t *testing.T) {
	cases := []struct {
		name        string
		existingMax int
		par         []bool
		want        []int
	}{
		{"fresh sequential", -1, []bool{false, false, false}, []int{0, 1, 2}},
		{"fresh fan-out join", -1, []bool{false, true, true, false}, []int{0, 0, 0, 1}},
		{"append after existing", 4, []bool{false, false}, []int{5, 6}},
		{"par on first of batch joins last existing", 4, []bool{true, false}, []int{4, 5}},
		{"par on first with no existing is ignored", -1, []bool{true, false}, []int{0, 1}},
		{"all par", 2, []bool{true, true, true}, []int{2, 2, 2}},
	}
	for _, c := range cases {
		got := AssignSeqs(c.existingMax, c.par)
		if len(got) != len(c.want) {
			t.Fatalf("%s: len %d != %d", c.name, len(got), len(c.want))
		}
		for i := range got {
			if got[i] != c.want[i] {
				t.Fatalf("%s: got %v, want %v", c.name, got, c.want)
			}
		}
	}
}

func TestBuildNode_MinRecurrence(t *testing.T) {
	now := time.Now()
	if _, err := buildNode(NewNode{Goal: "tick", When: When{Every: "1s"}}, now); err == nil {
		t.Fatal("every below MinRecurrence should be rejected")
	}
	if _, err := buildNode(NewNode{Goal: "tick", When: When{Every: "1m"}}, now); err != nil {
		t.Fatalf("every above MinRecurrence should be accepted: %v", err)
	}
}

func TestBuildNode_AxisValidation(t *testing.T) {
	now := time.Now()
	bad := func(name string, in NewNode) {
		if _, err := buildNode(in, now); err == nil {
			t.Errorf("%s should be rejected", name)
		}
	}
	bad("two executors", NewNode{Goal: "x", Do: Do{Shell: "true", Notify: "hi"}})
	bad("shell+ask", NewNode{Goal: "x", Do: Do{Shell: "true", Ask: true}})
	bad("two triggers", NewNode{Goal: "x", When: When{After: "1m", Every: "1h"}})
	bad("onDeps on shell", NewNode{Goal: "x", When: When{OnDeps: true}, Do: Do{Shell: "true"}})
	bad("onDeps on notify", NewNode{Goal: "x", When: When{OnDeps: true}, Do: Do{Notify: "hi"}})
}

func TestExecutorClassification(t *testing.T) {
	cases := []struct {
		name string
		n    Node
		want ExecutorKind
		mech bool
	}{
		{"shell", Node{Cmd: "go build"}, ExecShell, true},
		{"notify", Node{Notify: "hi"}, ExecNotify, true},
		{"human", Node{Assignee: AssigneeHuman}, ExecAsk, false},
		{"agent default", Node{}, ExecAgent, false},
		{"shell beats notify", Node{Cmd: "x", Notify: "y"}, ExecShell, true},
		{"notify beats human", Node{Notify: "y", Assignee: AssigneeHuman}, ExecNotify, true},
	}
	for _, c := range cases {
		if got := c.n.Executor(); got != c.want {
			t.Errorf("%s: Executor = %q, want %q", c.name, got, c.want)
		}
		if got := c.n.Mechanical(); got != c.mech {
			t.Errorf("%s: Mechanical = %v, want %v", c.name, got, c.mech)
		}
	}
}

func TestFireable_NotifyIsMechanical(t *testing.T) {
	now := time.Now()
	// A deferred notify node is mechanical — it belongs in mech, not wakes, so
	// it fires with no model turn.
	n := node(2, func(n *Node) { n.Notify = "ping"; n.After = now.Add(-time.Minute) })
	mech, _, wakes := Fireable([]Node{n}, now)
	if len(mech) != 1 || mech[0].ID != 2 {
		t.Fatalf("notify node should be mechanical, got mech=%v wakes=%v", mech, wakes)
	}
	if len(wakes) != 0 {
		t.Fatalf("notify node should not wake a model: %v", wakes)
	}
}

func TestBuildNode_TwoAxesCompose(t *testing.T) {
	now := time.Now()
	// A deferred reminder: when:{after} × do:{notify} — the headline combo.
	n, err := buildNode(NewNode{Goal: "remind", When: When{After: "30m"}, Do: Do{Notify: "stand up"}}, now)
	if err != nil {
		t.Fatalf("deferred notify should be valid: %v", err)
	}
	if n.Notify != "stand up" || n.After.IsZero() || n.Executor() != ExecNotify {
		t.Fatalf("mapped wrong: %+v exec=%s", n, n.Executor())
	}
	// A recurring shell command: a no-model cron.
	n, err = buildNode(NewNode{Goal: "backup", When: When{Every: "1h"}, Do: Do{Shell: "backup.sh"}}, now)
	if err != nil {
		t.Fatalf("recurring shell should be valid: %v", err)
	}
	if !n.Recurring() || n.Cmd != "backup.sh" || n.Executor() != ExecShell {
		t.Fatalf("mapped wrong: %+v", n)
	}
	// A callback: when:{onDeps} × agent (do omitted).
	n, err = buildNode(NewNode{Goal: "report done", When: When{OnDeps: true}}, now)
	if err != nil {
		t.Fatalf("onDeps agent callback should be valid: %v", err)
	}
	if !n.WakeOnReady || n.Executor() != ExecAgent {
		t.Fatalf("callback mapped wrong: %+v", n)
	}
}

func TestArmed(t *testing.T) {
	if node(1, nil).Armed() {
		t.Error("plain pending node is not armed")
	}
	if !node(1, func(n *Node) { n.After = time.Now() }).Armed() {
		t.Error("deferred node is armed")
	}
	if !node(1, func(n *Node) { n.WakeOnReady = true }).Armed() {
		t.Error("callback node is armed")
	}
	if !node(1, func(n *Node) { n.Every = time.Hour }).Armed() {
		t.Error("recurring node is armed")
	}
}
