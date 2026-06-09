// Package plan is the agent's intent: the durable forest of goals it is
// pursuing, and the operations over it. It completes the trinity:
//
//	STREAM  the event log (what happened)   — owned by the engine
//	ATOMS   what the agent knows            — internal/mind
//	PLAN    what the agent intends          — here
//
// Nodes are goals, not tasks: a node states what done looks like (and, when
// possible, how to verify it); executing one is an Attempt, recorded
// separately. The split is load-bearing — retries, recurring obligations,
// best-of-N, and regression all hang off attempt history, while the tree of
// goals stays small and legible.
//
// Two goal kinds, after the BDI distinction: achieve nodes reach a state and
// terminate; maintain nodes are standing obligations ("commit hourly") that
// recur while their parent lives and never complete. Deferral (After) and
// recurrence (Every) make readiness time-aware; sibling order is the only
// dependency relation. Like the rest of arbos, the store is the durable
// entity: intent survives compaction, restarts, and the death of any session,
// because it is projected into each turn (see Injector) rather than living in
// the context window.
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

// Kind distinguishes goals that terminate from standing obligations.
type Kind string

const (
	// KindAchieve is a goal that reaches a state and terminates.
	KindAchieve Kind = "achieve"
	// KindMaintain is a standing obligation: it recurs (Every) while its
	// parent is live and has no terminal success — it ends by cancellation
	// when its scope (the parent) ends.
	KindMaintain Kind = "maintain"
)

// Status is a node's execution state. For achieve nodes, done is a cached
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

	Kind  Kind
	Goal  string
	Check string // how to verify done, when stateable ("" = self-report)
	// Cmd is the node's mechanical payload: a shell command that IS the work.
	// A ready node with Cmd set is kernel-executable — the scheduler runs it
	// as a job and maps the exit code to the verdict, no model turn spent.
	// Judgment was exercised once, when the command was written; the model is
	// woken again only if it fails. "" = a judgment node (model-executed).
	Cmd      string
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

	// Par is an assignment-time directive, not persisted: append this node at
	// the SAME Seq as the one before it, so the two run alongside each other
	// (GatedBySibling only gates on strictly-lower Seq). Equal-Seq groups are
	// the fan-out/join shape: 1,1,1,2 is "three in parallel, then the fourth".
	Par bool

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
	// AddPlanNodes appends nodes in order. parent == 0 starts a new plan: the
	// first node becomes the root and the rest its children. It assigns IDs,
	// Plan, and Seq, returning the IDs in input order.
	AddPlanNodes(ctx context.Context, parent NodeID, nodes []Node) ([]NodeID, error)
	// PlanNode fetches one node.
	PlanNode(ctx context.Context, id NodeID) (Node, error)
	// UpdatePlanNode persists a node's mutable fields (status, outcome, owner,
	// goal/check, timing) by ID.
	UpdatePlanNode(ctx context.Context, n Node) error
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

// CanTransition validates a status change against the node's kind and the
// transition table. It is the deterministic floor under the model: illegal
// state drift is rejected here, not discouraged in the prompt.
func CanTransition(n Node, to Status) error {
	if to == n.Status {
		return fmt.Errorf("node %d is already %s", n.ID, to)
	}
	if n.Kind == KindMaintain && (to == StatusDone || to == StatusFailed) {
		return fmt.Errorf("node %d is a maintain node: it has no terminal success or failure — record recurrences with an outcome-only update, and cancel it when its scope ends", n.ID)
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
	if n.Kind == KindMaintain {
		return !n.NextDue.IsZero() && !now.Before(n.NextDue)
	}
	return true
}

// GatedBySibling reports whether an earlier sibling (lower Seq, same parent,
// achieve kind) is still open, which gates n in the implicit sequential
// dependency of sibling order. Maintain siblings never gate: standing
// obligations run alongside the work, not before it.
func GatedBySibling(siblings []Node, n Node) bool {
	for _, s := range siblings {
		if s.Parent == n.Parent && s.Seq < n.Seq && s.Kind == KindAchieve && !Terminal(s.Status) {
			return true
		}
	}
	return false
}
