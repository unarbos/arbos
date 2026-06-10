// Package plan is the agent's intent: the durable forest of goals it is
// pursuing, and the operations over it. It completes the memory model:
//
//	STREAM  what happened (one turn's transcript)  — engine, ephemeral
//	ATOMS   what the agent knows (across all work) — internal/mind, semantic
//	PLAN    what the agent intends + how each step went — here, task state
//	OUTBOX  what the user must hear                 — internal/outbox
//
// How memory and state interact through the core agent design: the agent is
// woken in short, fresh sessions at decision points (a trigger fires, a step
// finishes, a command fails). Each wake is the convergence of the four —
// the kernel assembles the woken session's context from the PLAN slice
// (intent + the last attempt's outcome under each open node, which is the
// working memory across wakes) and recalled ATOMS (knowledge), the session
// produces a STREAM that mind curates back into atoms at turn end, and the
// agent's durable effects land in the PLAN (progress) and the OUTBOX (voice).
// The stream is disposable; intent and knowledge are not. A node's Outcome is
// therefore written as a message to whoever continues the work — it, not the
// vanished transcript, is what the next wake reads.
//
// Two axes describe every node: a Trigger (when it becomes runnable — ready,
// deferred, recurring, or a callback) and an Executor (what discharges it —
// see ExecutorKind). The mechanical executors (shell, notify) the kernel runs
// itself with no model turn; the agent executor wakes a session. This is the
// kernel/judgment split at the orchestration layer: the brain is summoned
// only where judgment is irreducible, and authors more of the graph when it
// is.
//
// Nodes are goals, not tasks: a node states what done looks like (and, when
// possible, how to verify it); executing one is an Attempt, recorded
// separately. The split is load-bearing — retries, recurring obligations,
// best-of-N, and regression all hang off attempt history, while the tree of
// goals stays small and legible.
//
// There is no separate "kind" of goal: a node recurs iff it carries a
// recurrence (Every) — a standing obligation ("commit hourly") that never
// terminates and is ended by cancellation when its scope does. A one-shot
// goal terminates. Deferral (After) and recurrence (Every) make readiness
// time-aware; sibling order is the only dependency relation. Like the rest of
// arbos, the store is the durable entity: intent survives compaction,
// restarts, and the death of any session, because it is projected into each
// turn (see Injector) rather than living in the context window.
//
// The kernel/judgment split: this package validates transitions and computes
// readiness deterministically; deciding what to want, when to decompose, and
// whether work is good remains the model's, via the plan tool.
package plan

import (
	"context"
	"fmt"
	"time"
)

// NodeID identifies a plan node. IDs are small integers so the model can
// reference them tersely (“node 7”).
type NodeID int64

// Status is a node's execution state. For one-shot nodes, done is a cached
// claim — a later regression may legitimately reopen it (done → pending).
type Status string

const (
	StatusPending   Status = "pending"
	StatusActive    Status = "active"
	StatusBlocked   Status = "blocked"
	StatusDone      Status = "done"
	StatusCancelled Status = "cancelled"
	StatusFailed    Status = "failed"
)

// Assignees. A human-assigned node is a question or decision waiting on the
// user; it parks without blocking sibling branches.
const (
	AssigneeAgent = "agent"
	AssigneeHuman = "human"
)

// Verdict grades one attempt.
type Verdict string

const (
	VerdictSuccess      Verdict = "success"
	VerdictFail         Verdict = "fail"
	VerdictInconclusive Verdict = "inconclusive"
)

// Node is one goal in the forest.
type Node struct {
	ID     NodeID
	Plan   NodeID // the root's id; a root's Plan is its own ID
	Parent NodeID // 0 for roots
	Seq    int    // order among siblings; earlier siblings gate later ones

	Goal  string
	Check string // how to verify done, when stateable ("" = self-report)
	// Cmd is the node's mechanical payload: a shell command that IS the work.
	// A ready node with Cmd set is kernel-executable — the scheduler runs it
	// as a job and maps the exit code to the verdict, no model turn spent.
	// Judgment was exercised once, when the command was written; the model is
	// woken again only if it fails. "" = not a shell node (see Executor).
	Cmd string
	// Notify is the node's message-to-the-user payload: a node with it set is
	// discharged by emitting the message to the outbox, no model turn. It is
	// the mechanical effect for "remind me at 5pm" / "tell me when the build
	// finishes" — work the kernel does without waking a brain.
	Notify   string
	Status   Status
	Outcome  string // what happened / was learned; set at terminal statuses
	Assignee string // AssigneeAgent or AssigneeHuman
	Owner    string // who claimed it: a session id, or "job:<id>" for kernel runs

	// WakeOnReady is the dependency trigger — the callback node. When set, the
	// scheduler summons a model turn the moment this node becomes ready
	// (prerequisites finished), then clears the flag (one-shot). It is what
	// "notify me when the pipeline completes" compiles to: cmd nodes fire by
	// themselves, failures wake by themselves, and this is the third firing
	// rule — success wanted a voice. Opt-in per node, deliberately: waking on
	// EVERY ready judgment node would be the full driver.
	WakeOnReady bool

	After   time.Time     // not ready before this instant; zero = no deferral
	Every   time.Duration // recurrence period; 0 = one-shot
	NextDue time.Time     // next firing for recurring nodes; zero = none

	UpdatedAt time.Time
}

// Attempt is one execution of a node. Append-only: history is the most
// valuable artifact the system produces (failed attempts are knowledge), so
// attempts are never updated or deleted.
type Attempt struct {
	Node       NodeID
	Session    string
	Verdict    Verdict
	Outcome    string
	VerifiedBy string    // "self" until kernel-run checks and judge sessions land
	Workspace  string    // attempt-isolated worktree; "" = the shared workspace
	At         time.Time // when it was recorded (set by the store)
}

// Store is the storage the plan needs, satisfied by *sqlite.Store. Narrow on
// purpose (the mind.Store pattern): the self can move or shard later without
// touching the kernel, tool, or projection.
type Store interface {
	// AddPlanNodes appends nodes in order, each with its parallel-group flag.
	// parent == 0 starts a new plan: the first node becomes the root and the
	// rest its children. It assigns IDs, Plan, and Seq (via AssignSeqs),
	// returning the IDs in input order. par must be the same length as nodes.
	AddPlanNodes(ctx context.Context, parent NodeID, nodes []Node, par []bool) ([]NodeID, error)
	// PlanNode fetches one node.
	PlanNode(ctx context.Context, id NodeID) (Node, error)
	// SetPlanNodeStatus persists only a node's lifecycle columns — status,
	// outcome, owner. The trigger columns (after, next_due, wake) are owned
	// exclusively by DisarmPlanNode, so a lifecycle write can never resurrect
	// a trigger the scheduler just disarmed (the read-modify-write race a
	// full-row update opened).
	SetPlanNodeStatus(ctx context.Context, id NodeID, status Status, outcome, owner string) error
	// SetPlanNodeStatusIf is the compare-and-set form: it applies the
	// transition only if the node is still in the expected status, reporting
	// whether this caller won. Claiming a node active goes through here so two
	// sessions racing to claim the same node cannot both succeed (the atomic
	// guard the scheduler's ClaimPlanNode already had, now shared by the tool).
	SetPlanNodeStatusIf(ctx context.Context, id NodeID, from, to Status, outcome, owner string) (bool, error)
	// ReclaimStaleKernelNodes resets to pending any node left active and owned
	// by the kernel past olderThan — an orphan from a host that died between
	// claiming a cmd node and recording its result. Scoped to kernel-owned
	// nodes (whose runtime is bounded by the cmd job timeout) so it never
	// reclaims a node a human or model session is legitimately holding active.
	// Returns the number reclaimed.
	ReclaimStaleKernelNodes(ctx context.Context, olderThan time.Time) (int, error)
	// OpenPlanNodes returns every node of every plan whose root is not
	// terminal, ordered by (plan, parent, seq) — the working forest.
	OpenPlanNodes(ctx context.Context) ([]Node, error)
	// AddPlanAttempt appends one attempt record.
	AddPlanAttempt(ctx context.Context, a Attempt) error
	// LastPlanAttempts returns the most recent attempt per node for nodes of
	// open plans — the working memory the projection surfaces: each execution
	// (a recurrence, a retry, a fresh executor) reads its predecessor's
	// outcome from here instead of needing files or recall.
	LastPlanAttempts(ctx context.Context) (map[NodeID]Attempt, error)
	// ClaimPlanNode atomically moves a pending node to active for owner,
	// reporting whether THIS caller won. The compare-and-claim is what lets
	// several scheduler processes share one store without double-running a
	// node (interactive sessions and the serve host all carry clocks).
	ClaimPlanNode(ctx context.Context, id NodeID, owner string) (bool, error)
	// DisarmPlanNode atomically replaces a node's triggers — time arming and
	// the one-shot WakeOnReady flag — but only if they still match what the
	// caller read — the same race story as ClaimPlanNode, for wake firings.
	DisarmPlanNode(ctx context.Context, n Node, after, nextDue time.Time, wakeOnReady bool) (bool, error)
}

// Terminal reports whether a status admits no further work.
func Terminal(s Status) bool {
	return s == StatusDone || s == StatusCancelled || s == StatusFailed
}

// Armed reports whether the scheduler (not pull mode) is responsible for
// firing this node: it carries a trigger the clock owns — a recurrence, a
// deferral, or a one-shot callback. This is the single predicate every
// trigger consumer shares (firing, disarming, the handoff hint), so "what
// fires this node" is answered in one place instead of re-derived from raw
// fields at each call site.
func (n Node) Armed() bool {
	return n.Recurring() || !n.After.IsZero() || n.WakeOnReady
}

// Recurring reports whether the node is a standing obligation rather than a
// one-shot goal — derived from the recurrence itself (Every set), not a
// separate "kind" field. A recurring node never terminates on success; it
// re-arms and is ended only by cancellation.
func (n Node) Recurring() bool { return n.Every != 0 }

// ExecutorKind names what discharges a node — the second axis of the design
// (the first being the trigger, "when"). It is derived from the node's fields
// rather than stored, the same "explicit value over flat columns" pattern as
// Armed(): one home for "what runs this" instead of the inference scattered
// across the scheduler. Two are mechanical (the kernel runs them, no model
// turn); one wakes a model session; one waits on the user.
type ExecutorKind string

const (
	ExecShell  ExecutorKind = "shell"  // run Cmd as a job, verdict from exit code
	ExecNotify ExecutorKind = "notify" // emit Notify to the outbox
	ExecAgent  ExecutorKind = "agent"  // wake a model session (judgment)
	ExecAsk    ExecutorKind = "ask"    // a question parked for the user
)

// Executor classifies how a node is discharged. Precedence is deliberate: a
// shell command is the work whatever else is set; then a notify message; an
// agent node assigned to the human is a question; otherwise the model decides.
func (n Node) Executor() ExecutorKind {
	switch {
	case n.Cmd != "":
		return ExecShell
	case n.Notify != "":
		return ExecNotify
	case n.Assignee == AssigneeHuman:
		return ExecAsk
	default:
		return ExecAgent
	}
}

// Mechanical reports whether the kernel can discharge this node itself, with
// no model turn — the executors the scheduler runs inline (shell, notify), as
// opposed to the agent wake. This is the partition the drain loop runs on.
func (n Node) Mechanical() bool {
	k := n.Executor()
	return k == ExecShell || k == ExecNotify
}

// AssignSeqs computes the sibling-order slot for each node appended under a
// parent, given the parent's current max seq (existingMax, or -1 when the
// parent has no children yet). A node with par[i] shares the previous node's
// slot — the parallel group — so a fan-out of three-then-join reads
// 1,1,1,2; everything else advances by one. Pure so the dependency/parallel
// policy is tested here, not buried in a SQL transaction.
func AssignSeqs(existingMax int, par []bool) []int {
	seqs := make([]int, len(par))
	next := existingMax + 1
	prev := existingMax // -1 when no existing siblings: first par is ignored
	for i, p := range par {
		seq := next
		if p && prev >= 0 {
			seq = prev
		}
		seqs[i] = seq
		prev = seq
		next = seq + 1
	}
	return seqs
}

// transitions is the legal status graph, declared whole so reading it answers
// "what can happen to a node". Notable edges: done -> pending (done is a
// cache — a detected regression reopens the goal) and nothing out of
// cancelled (final; re-adding a fresh node keeps history honest).
var transitions = map[Status]map[Status]bool{
	StatusPending:   {StatusActive: true, StatusBlocked: true, StatusDone: true, StatusFailed: true, StatusCancelled: true},
	StatusActive:    {StatusDone: true, StatusFailed: true, StatusBlocked: true, StatusPending: true, StatusCancelled: true},
	StatusBlocked:   {StatusPending: true, StatusActive: true, StatusCancelled: true},
	StatusDone:      {StatusPending: true},
	StatusFailed:    {StatusPending: true, StatusActive: true, StatusCancelled: true},
	StatusCancelled: {},
}

// CanTransition validates a status change against the transition table and
// the node's recurrence. It is the deterministic floor under the model:
// illegal state drift is rejected here, not discouraged in the prompt.
func CanTransition(n Node, to Status) error {
	if to == n.Status {
		return fmt.Errorf("node %d is already %s", n.ID, to)
	}
	if n.Recurring() && (to == StatusDone || to == StatusFailed) {
		return fmt.Errorf("node %d recurs (every set): it has no terminal success or failure — record recurrences with an outcome-only update, and cancel it when its scope ends", n.ID)
	}
	allowed, known := transitions[n.Status]
	if !known || !allowed[to] {
		return fmt.Errorf("node %d cannot go %s -> %s", n.ID, n.Status, to)
	}
	return nil
}

// Ready reports whether a node can be worked on now: pending,
// agent-assigned, past its deferral, due if recurring, and not gated by an
// unfinished earlier sibling. Advisory in pull mode (the projection marks it),
// authoritative for the future driver — one predicate, every consumer.
func Ready(n Node, gatedBySibling bool, now time.Time) bool {
	if n.Status != StatusPending || n.Assignee != AssigneeAgent || gatedBySibling {
		return false
	}
	if !n.After.IsZero() && now.Before(n.After) {
		return false
	}
	if n.Recurring() {
		return !n.NextDue.IsZero() && !now.Before(n.NextDue)
	}
	return true
}

// GatedBySibling reports whether an earlier one-shot sibling (lower Seq, same
// parent) is still open, which gates n in the implicit sequential dependency
// of sibling order. Recurring siblings never gate: standing obligations run
// alongside the work, not before it.
func GatedBySibling(siblings []Node, n Node) bool {
	for _, s := range siblings {
		if s.Parent == n.Parent && s.Seq < n.Seq && !s.Recurring() && !Terminal(s.Status) {
			return true
		}
	}
	return false
}
