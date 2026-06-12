package pi

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"sync"

	"github.com/unarbos/arbos/internal/agent"
	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/mind"
	"github.com/unarbos/arbos/internal/outbox"
	"github.com/unarbos/arbos/internal/plan"
	"github.com/unarbos/arbos/internal/ports"
	"github.com/unarbos/arbos/internal/tool"
	"github.com/unarbos/arbos/internal/tool/coding"
	"github.com/unarbos/arbos/internal/tool/codingspec"
)

// Options configures the pi coding agent assembly. Provider, NewStore, Clock,
// Cwd, and Model are required; the rest take sensible defaults.
type Options struct {
	Provider       ports.LLMProvider
	NewStore       func() ports.SessionStore
	Clock          ports.Clock
	Cwd            string
	AgentDir       string // user-scope config dir for skills/prompts; "" disables user scope
	Model          string
	Models         *ModelRegistry
	Reasoning      core.ReasoningLevel
	CacheRetention core.CacheRetention
	// DistillModel, when set, runs the background distillers — the compaction
	// Summarizer here and the Mind curator (wired by the host) — on a cheaper,
	// faster model than the main agent. Distillation is template extraction,
	// so it never inherits the agent's reasoning level. Empty falls back to
	// Model.
	DistillModel string
	// SubagentModel, when set, names the model delegated children run on,
	// read per delegation. It is a hook rather than a string so the host can
	// back it with a live settings store — a change in the Settings tab
	// applies to the next delegate call without rebuilding the agent. Nil or
	// returning "" falls back to Model.
	SubagentModel func() string
	// Approval is nil by default, which the engine treats as allow-all — matching
	// pi's full-privileges behavior. A user opts into gating writes/shell by
	// supplying an ApprovalPolicy here.
	Approval      ports.ApprovalPolicy
	Observer      ports.Observer
	MaxIterations int
	// Mind, when set, is the agent's long-term memory, and it is the SELF's:
	// recall (read) is injected at turn start, curation distills the turn at
	// end, and the remember tool (write) is offered. Set only on the top
	// engine (which the scheduler's wakes reuse) — not on delegated children:
	// recall anchors on the session's own event log, which a child running on
	// an ephemeral store does not have in the durable atom store, and a child
	// writing global atoms is a memory-poisoning surface. Children contribute
	// findings by returning results up to the self, which decides what to
	// remember. Atoms remain one global set; only the read/write *operations*
	// live at the self.
	Mind *mind.Mind
	// Plan, when set, is the agent's durable intent (internal/plan): the plan
	// tool joins the toolset and the forest is injected at each turn start.
	// Unlike Mind it threads into delegated children too — the forest is
	// shared, so a child sees and advances the same intent as its parent.
	Plan plan.Store
	// Outbox, when set, is the agent's voice to the user outside any open
	// conversation (internal/outbox): the notify tool joins the toolset.
	// Threads into children and scheduler-spawned executors alike — any
	// session may need to reach the user.
	Outbox outbox.Store
	// ExtraRuntimes are merged into the top-level toolset only (e.g. MCP servers).
	ExtraRuntimes []ports.ToolRuntime
	// ExtraTools, when set, is composed alongside the coding toolset for the
	// top-level session only (via NewEngine) — this is how the primary pi agent
	// gets the delegation tool (delegate) so it can spawn sub-agents. Delegated
	// children (NewAgent) get the pure coding toolset, which bounds delegation
	// depth.
	ExtraTools ports.ToolRuntime
}

// projectContextFiles is the set of AGENTS.md-style files auto-loaded into the
// system prompt, matching pi's default context files.
var projectContextFiles = []string{"AGENTS.md", "CLAUDE.md"}

func (o Options) models() *ModelRegistry {
	if o.Models != nil {
		return o.Models
	}
	return DefaultModelRegistry()
}

// DistillerModel is the model background distillation runs on: DistillModel
// when set, otherwise the main model. It is the one home for that fallback;
// hosts wiring other distillers (the Mind curator) call it rather than
// restating the rule.
func (o Options) DistillerModel() string {
	if o.DistillModel != "" {
		return o.DistillModel
	}
	return o.Model
}

// buildForRoot assembles the coding config and tool registry rooted at a
// specific working directory. The tools' path base, the prompt's cwd, and the
// auto-loaded context files/skills all derive from root, so a delegated child
// granted a different repo (Grant.Env.Path) genuinely operates there.
func (o Options) buildForRoot(root string) (engine.Config, ports.ToolRuntime, error) {
	var rt ports.ToolRuntime
	reg, err := coding.NewRuntime(root)
	if err != nil {
		return engine.Config{}, nil, err
	}
	rt = reg
	infos := codingspec.PromptInfos()
	// Subsystem tools (plan, notify) join one extras registry so the toolset
	// is composed once however many subsystems are wired.
	extras := tool.New()
	if o.Plan != nil {
		if err := plan.RegisterTool(extras, o.Plan); err != nil {
			return engine.Config{}, nil, err
		}
		infos = append(infos, plan.PromptInfo())
	}
	if o.Outbox != nil {
		if err := outbox.RegisterTool(extras, o.Outbox); err != nil {
			return engine.Config{}, nil, err
		}
		infos = append(infos, outbox.PromptInfo())
	}
	if o.Mind != nil {
		// The explicit write half of memory — universal across sessions
		// (atoms are one global set), so a delegated child can remember too.
		if err := mind.RegisterRememberTool(extras, o.Mind); err != nil {
			return engine.Config{}, nil, err
		}
		infos = append(infos, mind.RememberPromptInfo())
	}
	if len(extras.Schemas()) > 0 {
		rt = tool.Multi(reg, extras)
	}
	prompt := BuildSystemPrompt(PromptOptions{
		Cwd:          root,
		Tools:        infos,
		ContextFiles: loadContextFiles(root),
		Skills:       LoadSkills(root, o.AgentDir),
	})
	maxIter := o.MaxIterations
	if maxIter == 0 {
		maxIter = 50
	}
	cfg := engine.Config{
		Model:          o.Model,
		SystemPrompt:   prompt,
		MaxIterations:  maxIter,
		Reasoning:      o.Reasoning,
		CacheRetention: o.CacheRetention,
		// Slash commands expand at projection, never in the log: every host
		// (CLI, TUI, serve, web) gets them by construction, and the persisted
		// transcript keeps exactly what the user typed.
		ExpandUser: TemplateExpander(root, o.AgentDir),
	}
	return cfg, rt, nil
}

// engineOptions returns the engine options for sessions rooted at root and
// running model (the compaction policy budgets against that model's window):
// compaction policy, summarizer, approval, observer, and the turn-start
// context injectors — memory recall when a Mind is attached, the plan forest
// when a plan store is attached, and the workspace's background-job table
// always (jobs are part of the coding toolset, so every session that can
// start one can see them).
func (o Options) engineOptions(root, model string) []engine.Option {
	mi := o.models().Get(model)
	policy := CompactionPolicy{ContextWindow: mi.ContextWindow}
	summ := Summarizer{Provider: o.Provider, Model: o.DistillerModel()}
	opts := []engine.Option{engine.WithContextPolicy(policy, summ)}
	if o.Mind != nil {
		// Recall at turn start, curate at turn end — the self's memory loop.
		// o.Mind is set only on the self (top engine + the wakes that reuse
		// it), so this never wires for a child, whose recall could not anchor
		// (its events live in an ephemeral store).
		opts = append(opts, engine.WithContextInjector(o.Mind.Recall), engine.WithTurnEnd(o.Mind.Curate))
	}
	if o.Plan != nil {
		opts = append(opts, engine.WithContextInjector(plan.Injector(o.Plan)))
	}
	opts = append(opts, engine.WithContextInjector(jobsInjector(root)))
	if o.Approval != nil {
		opts = append(opts, engine.WithApproval(o.Approval))
	}
	if o.Observer != nil {
		opts = append(opts, engine.WithObserver(o.Observer))
	}
	return opts
}

// jobsInjector surfaces the workspace's background-job table at the start of
// each turn, so the model re-grounds on running and recently-finished jobs
// without spending a tool call — including jobs that survived an arbos
// restart, since the table is derived from the shared job directories
// (ADR-0032).
//
// It implements ADR-0015's only-append-when-changed discipline: a session is
// re-injected only when its rendered table differs from what it was last
// shown, and a session that has never had jobs is never told "(no background
// jobs)" — the empty table is injected only to supersede a previous table.
func jobsInjector(root string) func(context.Context, core.SessionID) ([]core.Segment, error) {
	var mu sync.Mutex
	last := map[core.SessionID]string{}
	return func(_ context.Context, sid core.SessionID) ([]core.Segment, error) {
		content := codingspec.JobsContext(root)
		mu.Lock()
		defer mu.Unlock()
		prev, seen := last[sid]
		if content == prev || (!seen && content == codingspec.NoJobsContext) {
			return nil, nil
		}
		last[sid] = content
		return []core.Segment{{Source: core.SourceJobs, Content: content}}, nil
	}
}

// NewEngine builds a coding-configured engine for the top-level pi session: the
// coding toolset, the assembled prompt, project context, skills, the pi
// compaction policy and summarizer, reasoning and cache hints, and nil approval
// (full-privileges; opt into gating via Options.Approval).
func NewEngine(o Options) (*engine.Engine, error) {
	cfg, reg, err := o.buildForRoot(o.Cwd)
	if err != nil {
		return nil, err
	}
	rt := ports.ToolRuntime(reg)
	if len(o.ExtraRuntimes) > 0 {
		rts := append([]ports.ToolRuntime{reg}, o.ExtraRuntimes...)
		rt = tool.Multi(rts...)
	}
	if o.ExtraTools != nil {
		// Compose delegation tools onto the coding (+ MCP/extension) toolset.
		rt = tool.Multi(rt, o.ExtraTools)
	}
	return engine.New(o.Provider, rt, o.NewStore(), o.Clock, cfg, o.engineOptions(o.Cwd, cfg.Model)...), nil
}

// NewAgent builds the pi coding agent as an ArbosAgent. Each delegation gets its
// own engine sharing the prepared coding registry and prompt; a grant's tool
// allowlist filters the child's toolset and its iteration budget overrides
// MaxIterations.
func NewAgent(o Options, newID func() core.SessionID) (*agent.ArbosAgent, error) {
	// Validate the build-time root up front so a misconfigured host fails at
	// registration, not on first delegation.
	if _, _, err := o.buildForRoot(o.Cwd); err != nil {
		return nil, err
	}
	factory := func(g agent.Grant) (*engine.Engine, error) {
		// Honor Grant.Env.Path: a child granted a repo runs rooted there (its own
		// path base, prompt cwd, context files), not in the parent's cwd. Empty
		// falls back to the build-time cwd.
		root := o.Cwd
		if g.Env.Path != "" {
			abs, err := validateGrantRoot(o.Cwd, g.Env.Path)
			if err != nil {
				return nil, err
			}
			root = abs
		}
		childCfg, childReg, err := o.buildForRoot(root)
		if err != nil {
			return nil, err
		}
		// The subagent model, read per delegation so a settings change made
		// since assembly applies to this child. Empty keeps the main model.
		if o.SubagentModel != nil {
			if m := o.SubagentModel(); m != "" {
				childCfg.Model = m
			}
		}
		if len(g.Tools) > 0 {
			known := map[string]bool{}
			for _, s := range childReg.Schemas() {
				known[s.Name] = true
			}
			for _, name := range g.Tools {
				if !known[name] {
					return nil, fmt.Errorf("grant tool %q is not available to delegated agents", name)
				}
			}
		}
		if g.Budget.MaxIterations > 0 {
			childCfg.MaxIterations = g.Budget.MaxIterations
		}
		childReg = tool.Filter(childReg, g.Tools)
		// Options are per-root: a child granted a different repo gets that
		// repo's background-job table injected, matching where its tools run.
		// Per-model too: the compaction policy budgets against the child's
		// actual model, which may differ from the parent's.
		return engine.New(o.Provider, childReg, o.NewStore(), o.Clock, childCfg, o.engineOptions(root, childCfg.Model)...), nil
	}
	return agent.NewArbosAgent(factory, newID), nil
}

// validateGrantRoot resolves a delegated repo path against the host cwd (the
// same tool.Resolve the file tools use: relative joins to the host root, an
// absolute path is taken as given) and confirms it is an existing directory. It
// does not confine the child to the host tree — a child may be granted any repo
// on the machine, matching arbos's unrestricted default (see tool.Resolve). The
// existence check stays so a typo'd or missing grant fails loudly at build time
// rather than running the child in the wrong place.
func validateGrantRoot(hostRoot, path string) (string, error) {
	abs, err := tool.Resolve(hostRoot, path)
	if err != nil {
		return "", fmt.Errorf("repo path %q: %w", path, err)
	}
	info, err := os.Stat(abs)
	if err != nil {
		return "", fmt.Errorf("repo path %q: %w", path, err)
	}
	if !info.IsDir() {
		return "", fmt.Errorf("repo path %q is not a directory", path)
	}
	return abs, nil
}

// Register adds the pi coding agent to a router under the backend name "pi", so
// it is reachable via delegate.
func Register(router *agent.Router, o Options, newID func() core.SessionID) error {
	ag, err := NewAgent(o, newID)
	if err != nil {
		return err
	}
	router.Register("pi", ag)
	return nil
}

func loadContextFiles(cwd string) []ContextFile {
	var out []ContextFile
	for _, name := range projectContextFiles {
		if b, err := os.ReadFile(filepath.Join(cwd, name)); err == nil {
			out = append(out, ContextFile{Path: name, Content: string(b)})
		}
	}
	return out
}
