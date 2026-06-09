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

// NewNode is the model-facing shape for creating one goal.
type NewNode struct {
	Goal     string `json:"goal" desc:"What done looks like, stated falsifiably."`
	Check    string `json:"check,omitempty" desc:"How to verify it (a command, a test, a criterion). Omit only when no check is stateable."`
	Cmd      string `json:"cmd,omitempty" desc:"Shell command that IS this node's work: when the node becomes ready the kernel runs it as a job in the workspace and maps exit 0/non-zero to done/failed — no model turn, you are woken only on failure. Use for mechanical steps (build, test, commit, push)."`
	Par      bool   `json:"par,omitempty" desc:"Run alongside the previous node (same order slot) instead of after it. Consecutive par nodes form a parallel group; the next non-par node joins after the whole group."`
	Kind     string `json:"kind,omitempty" desc:"achieve (default) reaches a state and terminates; maintain is a standing obligation that recurs and never completes."`
	After    string `json:"after,omitempty" desc:"Defer readiness by this duration from now (e.g. \"30m\")."`
	Every    string `json:"every,omitempty" desc:"Recurrence period for maintain nodes (e.g. \"1h\"). Required for maintain."`
	Assignee string `json:"assignee,omitempty" desc:"agent (default), or human for a question/decision only the user can resolve."`
}

// Args are the arguments to the plan tool.
type Args struct {
	Op string `json:"op" desc:"add, update, or show."`
	// add
	Parent int64     `json:"parent,omitempty" desc:"add: parent node id. 0 starts a new plan — the first node becomes the mission root, the rest its children."`
	Nodes  []NewNode `json:"nodes,omitempty" desc:"add: nodes to append, in execution order."`
	// update
	Node    int64  `json:"node,omitempty" desc:"update: target node id."`
	Status  string `json:"status,omitempty" desc:"update: active (claim it), done, failed, blocked, cancelled, or pending (reopen). Omit on a maintain node to record one recurrence."`
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
		"Maintain your durable plan: the forest of goals you are pursuing. It survives restarts, compaction, and sessions — trust it over conversation memory. op:add decomposes work into goal nodes (state a check whenever one is stateable); op:update moves one node through its life (claim, finish with an outcome, fail, block, cancel); op:show renders the forest.",
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
	for i, in := range a.Nodes {
		n, err := buildNode(in, now)
		if err != nil {
			return "", fmt.Errorf("plan add: nodes[%d]: %w", i, err)
		}
		nodes[i] = n
	}
	ids, err := store.AddPlanNodes(ctx, NodeID(a.Parent), nodes)
	if err != nil {
		return "", err
	}
	strs := make([]string, len(ids))
	for i, id := range ids {
		strs[i] = fmt.Sprintf("#%d", id)
	}
	return result(ctx, store, now, fmt.Sprintf("Added %s.", strings.Join(strs, ", ")))
}

func buildNode(in NewNode, now time.Time) (Node, error) {
	n := Node{
		Kind:     KindAchieve,
		Goal:     strings.TrimSpace(in.Goal),
		Check:    strings.TrimSpace(in.Check),
		Cmd:      strings.TrimSpace(in.Cmd),
		Par:      in.Par,
		Status:   StatusPending,
		Assignee: AssigneeAgent,
	}
	if n.Goal == "" {
		return Node{}, fmt.Errorf("goal must not be empty")
	}
	switch in.Kind {
	case "", string(KindAchieve):
	case string(KindMaintain):
		n.Kind = KindMaintain
	default:
		return Node{}, fmt.Errorf("kind must be achieve or maintain, got %q", in.Kind)
	}
	switch in.Assignee {
	case "", AssigneeAgent:
	case AssigneeHuman:
		n.Assignee = AssigneeHuman
	default:
		return Node{}, fmt.Errorf("assignee must be agent or human, got %q", in.Assignee)
	}
	if in.After != "" {
		d, err := time.ParseDuration(in.After)
		if err != nil || d <= 0 {
			return Node{}, fmt.Errorf("after must be a positive duration like \"30m\", got %q", in.After)
		}
		n.After = now.Add(d)
	}
	if in.Every != "" {
		d, err := time.ParseDuration(in.Every)
		if err != nil || d <= 0 {
			return Node{}, fmt.Errorf("every must be a positive duration like \"1h\", got %q", in.Every)
		}
		n.Every = d
	}
	if n.Kind == KindMaintain {
		if n.Every == 0 {
			return Node{}, fmt.Errorf("a maintain node needs every (its recurrence period)")
		}
		n.NextDue = now.Add(n.Every)
	} else if n.Every != 0 {
		return Node{}, fmt.Errorf("every is for maintain nodes; use a maintain node for recurring work")
	}
	if n.Cmd != "" && n.Assignee == AssigneeHuman {
		return Node{}, fmt.Errorf("a cmd node cannot be assigned to the human — the kernel runs cmds")
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
	// attempt with the outcome, and the next due time advanced.
	if a.Status == "" {
		if n.Kind != KindMaintain {
			return "", fmt.Errorf("plan update: node %d: status is required for achieve nodes", n.ID)
		}
		if outcome == "" {
			return "", fmt.Errorf("plan update: node %d: a recurrence needs an outcome", n.ID)
		}
		if err := store.AddPlanAttempt(ctx, Attempt{Node: n.ID, Session: sid, Verdict: VerdictSuccess, Outcome: outcome, VerifiedBy: "self"}); err != nil {
			return "", err
		}
		n.NextDue = now.Add(n.Every)
		if err := store.UpdatePlanNode(ctx, n); err != nil {
			return "", err
		}
		return result(ctx, store, now, fmt.Sprintf("Recorded recurrence of #%d; next due %s.", n.ID, n.NextDue.Format("15:04")))
	}

	to := Status(a.Status)
	if err := CanTransition(n, to); err != nil {
		return "", fmt.Errorf("plan update: %w", err)
	}
	if (to == StatusDone || to == StatusFailed || to == StatusBlocked) && outcome == "" {
		return "", fmt.Errorf("plan update: node %d: %s needs an outcome — say what happened or what is needed", n.ID, to)
	}
	if to == StatusActive {
		n.Owner = sid
	}
	if outcome != "" {
		n.Outcome = outcome
	}
	n.Status = to
	if err := store.UpdatePlanNode(ctx, n); err != nil {
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
			"Timed requests — \"in 30 minutes, …\", \"message me in an hour\", \"at 3pm do X\" — are deferred plan nodes (op:add with after, assigned to the agent), and the turn ends immediately: the host clock fires the node and notify carries the result to the user. Never hold a turn open with sleep or await just to reach a moment in time.",
			"For work with three or more steps, decompose it with the plan tool before starting, give each node a check when one is stateable, and keep exactly one node active while you work on it.",
			"Compile pipelines instead of babysitting them: mechanical steps (build, test, commit, push) become sibling nodes with cmd — the kernel runs them in sibling order with zero model turns, par:true nodes run in parallel groups, and you are woken only if a command fails. Never await a sequence you can express as cmd nodes; create them and end the turn.",
			"Update the plan as facts arrive, not in batches: claim a node before working it (status active), and finish it the moment it is verified (status done, with a one-line outcome of what changed or was learned).",
			"Never end a turn with a node still active unless you are mid-task and continuing next turn; if you are stopped by something, mark it blocked with what is needed.",
			"Recurring obligations that need judgment (commit good progress, triage new failures) are maintain nodes with every; record each recurrence with an outcome-only update. The plan clock resolves to ~1 minute and each firing runs a model turn — purely mechanical commands on a finer cadence belong in a background loop job instead. Questions only the user can answer are nodes with assignee human — park them and continue other work.",
			"The <<plan>> block in your context is the live forest — trust it over your memory of earlier turns, and use node ids (#7) when updating.",
			"When the user asks what is running, scheduled, or happening in the background, answer from BOTH the <<plan>> block (deferred and standing nodes are scheduled work) and the jobs table (processes). A deferred node that has not fired yet is a background task.",
		},
	}
}
