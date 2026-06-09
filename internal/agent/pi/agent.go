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
	// Approval is nil by default, which the engine treats as allow-all — matching
	// pi's full-privileges behavior. A user opts into gating writes/shell by
	// supplying an ApprovalPolicy here.
	Approval      ports.ApprovalPolicy
	Observer      ports.Observer
	MaxIterations int
	// Mind, when set, is the agent's long-term memory: it recalls relevant atoms
	// at turn start and curates the finished turn into atoms at turn end. Attach
	// it only to the top-level session — delegated children share the same global
	// atoms through it, so they need no Mind of their own.
	Mind *mind.Mind
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
// specific workspace directory. The tool sandbox, the prompt's cwd, and the
// auto-loaded context files/skills all derive from root, so a delegated child
// granted a different repo (Grant.Env.Path) genuinely operates there.
func (o Options) buildForRoot(root string) (engine.Config, ports.ToolRuntime, error) {
	reg, err := coding.NewRuntime(root)
	if err != nil {
		return engine.Config{}, nil, err
	}
	prompt := BuildSystemPrompt(PromptOptions{
		Cwd:          root,
		Tools:        codingspec.PromptInfos(),
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
	}
	return cfg, reg, nil
}

// engineOptions returns the engine options for sessions rooted at root:
// compaction policy, summarizer, approval, observer, and the turn-start
// context injectors — memory recall when a Mind is attached, and the
// workspace's background-job table always (jobs are part of the coding
// toolset, so every session that can start one can see them).
func (o Options) engineOptions(root string) []engine.Option {
	mi := o.models().Get(o.Model)
	policy := CompactionPolicy{ContextWindow: mi.ContextWindow}
	summ := Summarizer{Provider: o.Provider, Model: o.DistillerModel()}
	opts := []engine.Option{engine.WithContextPolicy(policy, summ)}
	if o.Mind != nil {
		// Recall at turn start, curate at turn end — the symmetric memory seam.
		opts = append(opts, engine.WithContextInjector(o.Mind.Recall), engine.WithTurnEnd(o.Mind.Curate))
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
	return engine.New(o.Provider, rt, o.NewStore(), o.Clock, cfg, o.engineOptions(o.Cwd)...), nil
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
		// sandbox, prompt cwd, context files), not in the parent's cwd. Empty
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
		// repo's background-job table injected, matching its tool sandbox.
		return engine.New(o.Provider, childReg, o.NewStore(), o.Clock, childCfg, o.engineOptions(root)...), nil
	}
	return agent.NewArbosAgent(factory, newID), nil
}

// validateGrantRoot resolves a delegated repo path, confirms it exists, and
// refuses paths outside the host workspace so a child cannot operate on /etc,
// $HOME/.ssh, or any other directory the parent did not grant.
func validateGrantRoot(hostRoot, path string) (string, error) {
	abs, err := filepath.Abs(path)
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
	hostAbs, err := filepath.Abs(hostRoot)
	if err != nil {
		return "", fmt.Errorf("host workspace: %w", err)
	}
	if !tool.WithinRoot(abs, hostAbs) {
		return "", fmt.Errorf("repo path %q is outside the host workspace %q", path, hostRoot)
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
