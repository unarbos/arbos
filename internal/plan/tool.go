package plan

import (
	"context"
	"fmt"
	"reflect"
	"strings"
	"sync"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/obs"
	"github.com/unarbos/arbos/internal/tool"
	"github.com/unarbos/arbos/internal/tool/codingspec"
	"github.com/unarbos/arbos/internal/tool/jsonschema"
)

// MinRecurrence is the floor on a maintain node's period. The scheduler scans
// at a coarse interval, so a finer period cannot actually fire faster — and an
// unbounded-cadence recurrence is a runaway spend loop (each firing of a
// judgment node is a model turn). Mechanical sub-minute cadence belongs in a
// background loop job, which costs no model turns.
const MinRecurrence = 30 * time.Second

// NewNode is the model-facing shape for one goal. A node is two orthogonal
// choices — WHEN it becomes runnable (When) and WHAT discharges it (Do) — plus
// its goal and where it sits among siblings. Omitting When means "ready" (as
// soon as earlier siblings finish); omitting Do means "an agent task" (a model
// turn). Almost every When×Do combination is meaningful, so there is little to
// get wrong.
type NewNode struct {
	Goal  string `json:"goal" desc:"What done looks like, stated falsifiably. For an ask, this is the question."`
	Check string `json:"check,omitempty" desc:"How to verify it (a command, a test, a criterion). Omit only when no check is stateable."`
	When  When   `json:"when,omitempty" desc:"When the node becomes runnable. Omit for 'ready' (as soon as earlier siblings finish)."`
	Do    Do     `json:"do,omitempty" desc:"What discharges the node. Omit for an agent task (a model turn)."`
	Par   bool   `json:"par,omitempty" desc:"Run alongside the previous node (same order slot) instead of after it. Consecutive par nodes form a parallel group; the next non-par node joins after the whole group."`
}

// When is the trigger axis: at most one of after/every/onDeps (empty = ready),
// plus an optional condition gate that composes with every.
type When struct {
	After  string `json:"after,omitempty" desc:"Defer readiness by this duration from now (e.g. \"30m\"). One-shot."`
	Every  string `json:"every,omitempty" desc:"Recur on this period (e.g. \"1h\"). A standing obligation that never terminates — no separate 'kind'."`
	OnDeps bool   `json:"onDeps,omitempty" desc:"Fire a model turn the moment earlier siblings finish — the callback (\"tell me when the pipeline is done\"). Without it a ready agent task waits to be picked up by a working turn; only meaningful for agent tasks."`
	// Condition is a gate, not a trigger: it pairs with every (the poll
	// cadence) and fires the node's Do only when the predicate holds.
	Condition string `json:"condition,omitempty" desc:"A shell predicate (exit 0 = holds) the kernel checks every poll period; the node's do fires only when it holds. Pair with every (the poll cadence) and an agent or notify do. The cheap \"watch then act\" trigger — only the predicate runs each period, never the model, until it holds: when:{condition:\"test $(curl -s …) -lt 60000\", every:\"1m\"} with an agent do (wake and act) or a notify do (alert, no model turn)."`
}

// Do is the executor axis: at most one of its fields. Empty = an agent task.
type Do struct {
	Shell  string `json:"shell,omitempty" desc:"A shell command the kernel runs as a job — no model turn; verdict from exit 0/non-zero; you are woken only on failure. For mechanical steps (build, test, commit, push)."`
	Notify string `json:"notify,omitempty" desc:"A message the kernel delivers to the user — no model turn. For reminders and completion pings (when:{after:\"30m\"} do:{notify:\"stand up\"})."`
	Ask    bool   `json:"ask,omitempty" desc:"This node is a question only the user can resolve (the goal is the question); it parks until they answer instead of being worked by the agent."`
}

// Args are the arguments to the plan tool.
type Args struct {
	Op string `json:"op" desc:"add, update, or show."`
	// add
	Parent int64     `json:"parent,omitempty" desc:"add: parent node id. 0 starts a new plan — the first node becomes the mission root, the rest its children."`
	Nodes  []NewNode `json:"nodes,omitempty" desc:"add: nodes to append, in execution order."`
	// update
	Node    int64  `json:"node,omitempty" desc:"update: target node id."`
	Status  string `json:"status,omitempty" desc:"update: active (claim it), done, failed, blocked, cancelled, or pending (reopen). Omit on a recurring node to record one recurrence."`
	Outcome string `json:"outcome,omitempty" desc:"update: what happened or was learned — required when finishing, failing, or blocking."`
}

// RegisterTool adds the plan tool to a registry. Like delegate, it is the
// documented carve-out from ADR-0004's codegen: the tool is registered
// dynamically by whoever wires a Store, so its schema is reflected once at
// registration with the same reflector the generator uses.
func RegisterTool(reg *tool.Registry, store Store) error {
	schema, err := jsonschema.Reflect(reflect.TypeOf(Args{}))
	if err != nil {
		return fmt.Errorf("plan schema: %w", err)
	}
	spec := tool.NewSpec("plan",
		"Maintain your durable plan: the forest of goals you are pursuing. It survives restarts, compaction, and sessions — trust it over conversation memory. op:add decomposes work into nodes, each a (when × do) pair — when it runs and what runs it; op:update moves one node through its life (claim, finish with an outcome, fail, block, cancel); op:show renders the forest.",
		false,
		func(ctx context.Context, a Args) (string, error) {
			now := time.Now()
			switch a.Op {
			case "add":
				return addOp(ctx, store, a, now)
			case "update":
				return updateOp(ctx, store, a, now)
			case "show":
				return showOp(ctx, store, now)
			}
			return "", fmt.Errorf("plan: unknown op %q (use add, update, or show)", a.Op)
		})
	// The plan is shared state across every session, so the tool advertises an
	// unbounded footprint and never runs concurrently with same-batch writers.
	spec = tool.WithAccess(spec, func(Args) core.AccessSet {
		return core.AccessSet{Unknown: true}
	})
	return reg.Register(spec, schema)
}

func addOp(ctx context.Context, store Store, a Args, now time.Time) (string, error) {
	if len(a.Nodes) == 0 {
		return "", fmt.Errorf("plan add: nodes must not be empty")
	}
	nodes := make([]Node, len(a.Nodes))
	par := make([]bool, len(a.Nodes))
	origin := sessionFrom(ctx)
	for i, in := range a.Nodes {
		n, err := buildNode(in, now)
		if err != nil {
			return "", fmt.Errorf("plan add: nodes[%d]: %w", i, err)
		}
		// The forest is global, the voice is not: remember which session
		// created the node so its firings speak into that chat alone.
		n.Origin = origin
		nodes[i] = n
		par[i] = in.Par
	}
	ids, err := store.AddPlanNodes(ctx, NodeID(a.Parent), nodes, par)
	if err != nil {
		return "", err
	}
	strs := make([]string, len(ids))
	for i, id := range ids {
		strs[i] = fmt.Sprintf("#%d", id)
	}
	return result(ctx, store, now, fmt.Sprintf("Added %s.", strings.Join(strs, ", ")))
}

// buildNode maps the two model-facing axes (When, Do) onto the flat node. Each
// axis is at-most-one choice, so the only invalid inputs are picking two on one
// axis or an onDeps trigger on a non-agent executor (mechanical nodes already
// fire when ready) — the entire validation surface, down from the lattice the
// nine flat flags required.
func buildNode(in NewNode, now time.Time) (Node, error) {
	n := Node{
		Goal:     strings.TrimSpace(in.Goal),
		Check:    strings.TrimSpace(in.Check),
		Status:   StatusPending,
		Assignee: AssigneeAgent,
	}
	if n.Goal == "" {
		return Node{}, fmt.Errorf("goal must not be empty")
	}

	// Do: at most one executor (empty = agent task).
	do, exec := in.Do, 0
	if s := strings.TrimSpace(do.Shell); s != "" {
		n.Cmd, exec = s, exec+1
	}
	if s := strings.TrimSpace(do.Notify); s != "" {
		n.Notify, exec = s, exec+1
	}
	if do.Ask {
		n.Assignee, exec = AssigneeHuman, exec+1
	}
	if exec > 1 {
		return Node{}, fmt.Errorf("do: choose one of shell, notify, ask (or omit for an agent task)")
	}

	// When: at most one trigger (empty = ready).
	w, trig := in.When, 0
	if s := strings.TrimSpace(w.After); s != "" {
		d, err := time.ParseDuration(s)
		if err != nil || d <= 0 {
			return Node{}, fmt.Errorf("when.after must be a positive duration like \"30m\", got %q", s)
		}
		n.After, trig = now.Add(d), trig+1
	}
	if s := strings.TrimSpace(w.Every); s != "" {
		d, err := time.ParseDuration(s)
		if err != nil || d <= 0 {
			return Node{}, fmt.Errorf("when.every must be a positive duration like \"1h\", got %q", s)
		}
		if d < MinRecurrence {
			return Node{}, fmt.Errorf("when.every must be at least %s (the scheduler's resolution); for a finer mechanical cadence use a background loop job", MinRecurrence)
		}
		n.Every, n.NextDue, trig = d, now.Add(d), trig+1
	}
	if w.OnDeps {
		n.WakeOnReady, trig = true, trig+1
	}
	if trig > 1 {
		return Node{}, fmt.Errorf("when: choose one of after, every, onDeps (or omit for ready)")
	}
	if n.WakeOnReady && n.Executor() != ExecAgent {
		return Node{}, fmt.Errorf("when.onDeps is for agent tasks — a %s node already fires when ready", n.Executor())
	}
	// Condition is a gate on the cadence, not a standalone trigger: it needs a
	// poll period to be re-checked, and it discharges the node's Do (agent
	// wake or notify) — never a shell node, whose own Cmd would race the
	// predicate for the meaning of "the work".
	if c := strings.TrimSpace(w.Condition); c != "" {
		if n.Every == 0 {
			return Node{}, fmt.Errorf("when.condition needs when.every (the poll period to re-check it on)")
		}
		if n.Executor() != ExecAgent && n.Executor() != ExecNotify {
			return Node{}, fmt.Errorf("when.condition fires an agent or notify do, not a %s node", n.Executor())
		}
		n.Cond = c
	}
	return n, nil
}

func updateOp(ctx context.Context, store Store, a Args, now time.Time) (string, error) {
	if a.Node == 0 {
		return "", fmt.Errorf("plan update: node is required")
	}
	n, err := store.PlanNode(ctx, NodeID(a.Node))
	if err != nil {
		return "", err
	}
	sid := sessionFrom(ctx)
	outcome := strings.TrimSpace(a.Outcome)

	// A status-less update on a maintain node records one recurrence: an
	// attempt with the outcome. It does NOT advance the timer — the scheduler
	// owns next_due (it re-armed at disarm time), so writing it here would
	// double-advance and is exactly the trigger column a lifecycle write must
	// not touch.
	if a.Status == "" {
		if !n.Recurring() {
			return "", fmt.Errorf("plan update: node %d: status is required for one-shot nodes", n.ID)
		}
		if outcome == "" {
			return "", fmt.Errorf("plan update: node %d: a recurrence needs an outcome", n.ID)
		}
		if err := store.AddPlanAttempt(ctx, Attempt{Node: n.ID, Session: sid, Verdict: VerdictSuccess, Outcome: outcome, VerifiedBy: "self"}); err != nil {
			return "", err
		}
		if err := store.SetPlanNodeStatus(ctx, n.ID, StatusPending, outcome, n.Owner); err != nil {
			return "", err
		}
		return result(ctx, store, now, fmt.Sprintf("Recorded recurrence of #%d.", n.ID))
	}

	to := Status(a.Status)
	if err := CanTransition(n, to); err != nil {
		return "", fmt.Errorf("plan update: %w", err)
	}
	if (to == StatusDone || to == StatusFailed || to == StatusBlocked) && outcome == "" {
		return "", fmt.Errorf("plan update: node %d: %s needs an outcome — say what happened or what is needed", n.ID, to)
	}
	owner := n.Owner
	if to == StatusActive {
		owner = sid
	}
	if outcome == "" {
		outcome = n.Outcome // a status-only transition (e.g. claim) keeps the prior outcome
	}
	// Claiming a node active is compare-and-set against the status we read, so
	// two sessions cannot both claim the same node and double-execute it. Other
	// transitions are last-writer-wins (the terminal outcome is recorded as an
	// attempt regardless).
	if to == StatusActive {
		won, err := store.SetPlanNodeStatusIf(ctx, n.ID, n.Status, StatusActive, outcome, owner)
		if err != nil {
			return "", err
		}
		if !won {
			return "", fmt.Errorf("plan update: node %d is no longer %s — another session may have claimed it; re-read the plan", n.ID, n.Status)
		}
	} else if err := store.SetPlanNodeStatus(ctx, n.ID, to, outcome, owner); err != nil {
		return "", err
	}
	// Terminal work transitions are execution history: record the attempt.
	if to == StatusDone || to == StatusFailed {
		verdict := VerdictSuccess
		if to == StatusFailed {
			verdict = VerdictFail
		}
		if err := store.AddPlanAttempt(ctx, Attempt{Node: n.ID, Session: sid, Verdict: verdict, Outcome: outcome, VerifiedBy: "self"}); err != nil {
			return "", err
		}
	}
	return result(ctx, store, now, fmt.Sprintf("#%d -> %s.", n.ID, to))
}

func showOp(ctx context.Context, store Store, now time.Time) (string, error) {
	return renderForest(ctx, store, now)
}

// renderForest loads the open nodes and their most recent attempts and renders
// the plan forest. It is the single place that fetches both halves Render needs.
func renderForest(ctx context.Context, store Store, now time.Time) (string, error) {
	nodes, err := store.OpenPlanNodes(ctx)
	if err != nil {
		return "", err
	}
	last, err := store.LastPlanAttempts(ctx)
	if err != nil {
		return "", err
	}
	return Render(nodes, last, now), nil
}

// result appends the refreshed forest to a mutation's acknowledgement — the
// same verification-loop philosophy as edit's updated-region snippet: the
// model confirms the change from the result instead of issuing a show.
func result(ctx context.Context, store Store, now time.Time, ack string) (string, error) {
	forest, err := renderForest(ctx, store, now)
	if err != nil {
		return ack, nil // the mutation succeeded; the echo is best-effort
	}
	return ack + "\n\n" + forest, nil
}

func sessionFrom(ctx context.Context) string {
	c, _ := obs.From(ctx)
	return c.SessionID
}

// Injector surfaces the plan forest at the start of each turn so the model
// re-grounds on its intent without spending a tool call — including after
// compaction, a restart, or a session handoff, since the forest lives in the
// store, not the context window.
//
// It follows ADR-0015's only-append-when-changed discipline (the jobsInjector
// pattern): a session is re-injected only when the rendered forest differs
// from what it was last shown, and a session that has never had a plan is
// never told "(no active plans)" — the empty rendering is injected only to
// supersede a previous one.
func Injector(store Store) func(context.Context, core.SessionID) ([]core.Segment, error) {
	var mu sync.Mutex
	last := map[core.SessionID]string{}
	return func(ctx context.Context, sid core.SessionID) ([]core.Segment, error) {
		nodes, err := store.OpenPlanNodes(ctx)
		if err != nil {
			return nil, nil // intent is best-effort context, never a turn-breaker
		}
		// Attempt history is best-effort: the forest still renders without it.
		lastAttempts, _ := store.LastPlanAttempts(ctx)
		content := Render(nodes, lastAttempts, time.Now())
		mu.Lock()
		defer mu.Unlock()
		prev, seen := last[sid]
		if content == prev || (!seen && content == NoPlanContext) {
			return nil, nil
		}
		last[sid] = content
		return []core.Segment{{Source: core.SourcePlan, Content: content}}, nil
	}
}

// PromptInfo is the plan tool's system-prompt metadata, appended to the coding
// toolset's infos when a plan store is wired.
func PromptInfo() codingspec.ToolPromptInfo {
	return codingspec.ToolPromptInfo{
		Name:    "plan",
		Snippet: "Maintain your durable plan: goals, standing obligations, and questions for the user — survives restarts and compaction",
		Guidelines: []string{
			"Every node is two choices: WHEN it runs (when: omit for ready, {after} to defer, {every} to recur, {onDeps} to fire when earlier siblings finish) and WHAT runs it (do: omit for an agent task, {shell} for a kernel-run command, {notify} for a message to the user, {ask} for a question only the user can answer). Compose them; almost every combination is meaningful.",
			"Timed requests — \"in 30 minutes…\", \"message me in an hour\" — are when:{after:\"30m\"} nodes, and the turn ends immediately; the host clock fires them. Never hold a turn open with sleep or await to reach a moment in time.",
			"do:{shell} and do:{notify} cost no model turn — the kernel runs them. A reminder or completion ping is do:{notify} (e.g. when:{after:\"30m\"} do:{notify:\"stand up\"}); mechanical steps (build, test, commit, push) are do:{shell}. Reserve an agent task (do omitted) for firings that genuinely need judgment.",
			"For work with three or more steps, decompose it before starting, give each node a check when one is stateable, and keep exactly one node active while you work on it.",
			"Compile pipelines instead of babysitting them: mechanical steps become sibling do:{shell} nodes — the kernel runs them in sibling order with zero model turns, par nodes run as a parallel group, and you are woken only if one fails. Never await a sequence you can express as shell nodes; create them and end the turn.",
			"Success is silent unless asked: to hear about completion, end the pipeline with a final node with when:{onDeps} — the kernel summons it the moment its prerequisites finish (use do:{notify} for a fixed message, or an agent task to compose one). A ready node with no trigger never fires on its own; never promise a completion ping without when:{onDeps}.",
			"Update the plan as facts arrive, not in batches: claim a node before working it (status active), and finish it the moment it is verified (status done, with a one-line outcome of what changed or was learned).",
			"Never end a turn with a node still active unless you are mid-task and continuing next turn; if you are stopped by something, mark it blocked with what is needed.",
			"Recurring obligations that need judgment (commit good progress, triage new failures) are when:{every} agent tasks; record each recurrence with an outcome-only update. The clock resolves to ~1 minute and each firing runs a model turn — purely mechanical commands on a finer cadence belong in a background loop job. A when:{every} do:{shell} node is a no-model recurring command.",
			"The <<plan>> block in your context is the live forest — trust it over your memory of earlier turns, and use node ids (#7) when updating.",
			"Each open node shows its last attempt outcome (\"last: …\") — that is your working memory across firings, retries, and sessions, and it is authoritative for task state: when a <<memory>> atom disagrees with a last: line, the last: line wins (atoms are distilled in the background and can lag). Write outcomes as messages to whoever continues the work: include the values, readings, or conclusions the next attempt must compare against or build on. Never store task state in ad-hoc files when the outcome can carry it.",
			"When the user asks what is running, scheduled, or happening in the background, answer from BOTH the <<plan>> block (deferred and standing nodes are scheduled work) and the jobs table (processes). A deferred node that has not fired yet is a background task.",
		},
	}
}
