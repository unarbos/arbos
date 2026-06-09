package pi

import (
	"context"
	"fmt"
	"os"
	"path/filepath"

	"github.com/unarbos/arbos/internal/agent"
	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/memory"
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
	// Approval is nil by default, which the engine treats as allow-all — matching
	// pi's full-privileges behavior. A user opts into gating writes/shell by
	// supplying an ApprovalPolicy here.
	Approval      ports.ApprovalPolicy
	Observer      ports.Observer
	MaxIterations int
	// MemoryStore enables the memory tool and injects stored facts into each turn.
	MemoryStore *memory.Store
	// ExtraRuntimes are merged into the top-level toolset only (e.g. MCP servers).
	ExtraRuntimes []ports.ToolRuntime
	// ExtraTools, when set, is composed alongside the coding toolset for the
	// top-level session only (via NewEngine) — this is how the primary pi agent
	// gets the delegation tools (delegate / start_coding_session) so it can spawn
	// sub-agents. Delegated children (NewAgent) get the pure coding toolset, which
	// bounds delegation depth.
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

// buildForRoot assembles the coding config and tool registry rooted at a
// specific workspace directory. The tool sandbox, the prompt's cwd, and the
// auto-loaded context files/skills all derive from root, so a delegated child
// granted a different repo (Grant.Env.Path) genuinely operates there.
func (o Options) buildForRoot(root string) (engine.Config, ports.ToolRuntime, error) {
	reg, err := coding.NewRuntime(root, o.MemoryStore)
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

// engineOptions returns the root-independent engine options (compaction policy,
// summarizer, approval, observer) shared by every pi session.
func (o Options) engineOptions() []engine.Option {
	mi := o.models().Get(o.Model)
	policy := CompactionPolicy{ContextWindow: mi.ContextWindow}
	summ := Summarizer{Provider: o.Provider, Model: o.Model, Reasoning: o.Reasoning}
	opts := []engine.Option{engine.WithContextPolicy(policy, summ)}
	if o.MemoryStore != nil {
		store := o.MemoryStore
		opts = append(opts, engine.WithContextInjector(func(_ context.Context, _ core.SessionID) ([]core.Segment, error) {
			text, err := store.FormatMerged()
			if err != nil || text == "" {
				return nil, err
			}
			return []core.Segment{{Source: "memory", Content: text}}, nil
		}))
	}
	if o.Approval != nil {
		opts = append(opts, engine.WithApproval(o.Approval))
	}
	if o.Observer != nil {
		opts = append(opts, engine.WithObserver(o.Observer))
	}
	return opts
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
	return engine.New(o.Provider, rt, o.NewStore(), o.Clock, cfg, o.engineOptions()...), nil
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
	opts := o.engineOptions()
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
		return engine.New(o.Provider, childReg, o.NewStore(), o.Clock, childCfg, opts...), nil
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
// it is reachable via delegate / start_coding_session.
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
