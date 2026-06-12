package piwire

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"io"
	"log/slog"
	"os"
	"path/filepath"
	"time"

	"github.com/unarbos/arbos/internal/agent"
	"github.com/unarbos/arbos/internal/agent/pi"
	"github.com/unarbos/arbos/internal/ask"
	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/extension"
	"github.com/unarbos/arbos/internal/extension/builtin"
	"github.com/unarbos/arbos/internal/fake"
	"github.com/unarbos/arbos/internal/mcp"
	"github.com/unarbos/arbos/internal/mind"
	"github.com/unarbos/arbos/internal/modelcatalog"
	"github.com/unarbos/arbos/internal/obs"
	"github.com/unarbos/arbos/internal/outbox"
	"github.com/unarbos/arbos/internal/plan"
	"github.com/unarbos/arbos/internal/ports"
	"github.com/unarbos/arbos/internal/setmodel"
	"github.com/unarbos/arbos/internal/settings"
	"github.com/unarbos/arbos/internal/sqlite"
	"github.com/unarbos/arbos/internal/tool"
)

// HostConfig wires a pi session host. Config carries the resolved environment
// (provider, model; see LoadConfig); Store and Observer are required for
// production; delegated children always get an ephemeral in-memory store.
type HostConfig struct {
	Config   Config
	Store    ports.SessionStore
	Observer ports.Observer
	Approve  bool
	// Logger receives background diagnostics (e.g. memory curation failures). It
	// MUST write to a sink that won't corrupt the frontend — leave nil when
	// stderr is the transcript to discard them.
	Logger *slog.Logger
}

// Host bundles the assembled engine with a cleanup hook for MCP subprocesses,
// plus the store and logger seams its optional organs (the plan scheduler)
// attach through.
type Host struct {
	Engine  *engine.Engine
	Cleanup func()
	// Settings is the durable host preference file (the Settings tab's
	// agent-behavior knobs, e.g. the subagent model). Nil when the agent
	// config dir is unavailable or the file is unreadable — the host runs on
	// defaults and the gateway's settings routes stay disabled.
	Settings *settings.Store

	store    ports.SessionStore
	logger   *slog.Logger
	approve  bool
	provider ports.LLMProvider
	model    string
	distill  string
	models   *pi.ModelRegistry
	cwd      string
}

// GuestEngine builds a chat-only engine — the host's provider and durable
// store, but an empty toolset — for doors that carry untrusted principals
// (e.g. a guest Telegram bot handed to a friend). A guest can converse but
// can never touch files, shell, memory, or delegation; the door supplies the
// system prompt that frames the guest's standing. Sessions land in the same
// durable store, so they replay in history like any other conversation.
func (h *Host) GuestEngine(systemPrompt string) *engine.Engine {
	cfg := engine.Config{
		Model:        h.model,
		SystemPrompt: systemPrompt,
		// Slash commands expand at projection here too — a chat-only door
		// still speaks the host's prompt templates.
		ExpandUser: pi.TemplateExpander(h.cwd, AgentConfigDir()),
	}
	policy := pi.CompactionPolicy{ContextWindow: h.models.Get(h.model).ContextWindow}
	summ := pi.Summarizer{Provider: h.provider, Model: h.distill}
	return engine.New(h.provider, tool.New(), h.store, sysClock{}, cfg,
		engine.WithContextPolicy(policy, summ))
}

// Assemble builds the top-level pi engine with delegation tools, memory, MCP, and
// built-in extensions registered on the returned router.
func Assemble(cfg HostConfig) (*Host, error) {
	cwd, _ := os.Getwd()
	agentDir := AgentConfigDir()

	mcpCfg, err := mcp.LoadConfig(cfg.Config.MCPConfigJSON, cwd, agentDir)
	if err != nil {
		return nil, fmt.Errorf("mcp config: %w", err)
	}
	mcpMgr, err := mcp.Connect(context.Background(), mcpCfg)
	if err != nil {
		return nil, fmt.Errorf("mcp connect: %w", err)
	}

	extReg := tool.New()
	extHost := extension.NewHost(extReg)
	if err := extHost.Load(builtin.All()...); err != nil {
		mcpMgr.Close()
		return nil, fmt.Errorf("load extensions: %w", err)
	}

	var extraRT []ports.ToolRuntime
	if len(mcpMgr.Runtimes()) > 0 {
		extraRT = append(extraRT, mcpMgr.Runtimes()...)
	}
	if len(extReg.Schemas()) > 0 {
		extraRT = append(extraRT, extReg)
	}

	// Durable host preferences (the Settings tab's agent knobs). A missing or
	// unreadable file is non-fatal: the host runs on defaults, and the web
	// door's settings surface stays disabled rather than blocking assembly.
	var prefs *settings.Store
	if agentDir != "" {
		st, err := settings.Open(filepath.Join(agentDir, "settings.json"))
		if err != nil && cfg.Logger != nil {
			cfg.Logger.Warn("host settings unavailable", "err", err)
		}
		prefs = st
	}

	piOpts := pi.Options{
		Provider:       cfg.Config.NewProvider(),
		NewStore:       func() ports.SessionStore { return fake.NewStore() },
		Clock:          sysClock{},
		Cwd:            cwd,
		AgentDir:       agentDir,
		Model:          cfg.Config.Model,
		Models:         pi.SeededModelRegistry(),
		DistillModel:   cfg.Config.DistillModel,
		Reasoning:      cfg.Config.Reasoning,
		CacheRetention: core.CacheShort,
		ExtraRuntimes:  extraRT,
	}
	if prefs != nil {
		// A live hook, not a snapshot: each delegation reads the latest saved
		// preference, so a Settings-tab change applies without reassembly.
		piOpts.SubagentModel = prefs.SubagentModel
	}
	if cfg.Approve {
		piOpts.Approval = pi.CodingApprovalPolicy{}
	}
	// Durable intent: when the store backs the plan forest (SQLite, not the
	// in-memory fake), every session — top-level and delegated children alike —
	// gets the plan tool and the per-turn forest injection. Set before
	// pi.Register so children inherit it: one forest in one DB is what makes
	// intent survive any session and coordinate all of them.
	if ps, ok := cfg.Store.(plan.Store); ok {
		piOpts.Plan = ps
	}
	// Durable voice: same condition and same reasoning — any session
	// (including scheduler-spawned executors) may need to reach the user
	// outside an open conversation, so notify rides wherever the store can
	// hold an outbox.
	if os, ok := cfg.Store.(outbox.Store); ok {
		piOpts.Outbox = os
	}

	// Long-term memory, when the durable store backs atoms (SQLite, not the
	// in-memory fake) and a real LLM can curate. It is the SELF's: recall,
	// curation, and the remember tool are wired only on the top engine (which
	// the scheduler's wakes reuse), not on delegated children — a child's
	// recall can't anchor (its events live in an ephemeral store) and a child
	// writing global atoms would be a memory-poisoning surface. Children pass
	// findings up via results; the self decides what to remember. Created here
	// (before pi.Register) only so it can be Closed on an early return.
	var theMind *mind.Mind
	if as, ok := cfg.Store.(mind.Store); ok && cfg.Config.HasLLM {
		// The curator runs on the same distill model as the compaction
		// summarizer: one tier for everything the agent does in the background.
		theMind = mind.New(as, piOpts.Provider, piOpts.DistillerModel(), cfg.Logger)
	}

	router := agent.NewRouter()
	if err := pi.Register(router, piOpts, NewSessionID); err != nil {
		if theMind != nil {
			theMind.Close()
		}
		mcpMgr.Close()
		return nil, fmt.Errorf("register pi: %w", err)
	}
	// Tools for the top-level session only: delegation (which bounds nesting)
	// and ask (a question needs the user watching this conversation — a
	// delegated child or scheduler wake has no one to answer).
	topTools := tool.New()
	if err := agent.RegisterDelegate(topTools, router); err != nil {
		if theMind != nil {
			theMind.Close()
		}
		mcpMgr.Close()
		return nil, fmt.Errorf("register delegate: %w", err)
	}
	if err := ask.RegisterTool(topTools); err != nil {
		if theMind != nil {
			theMind.Close()
		}
		mcpMgr.Close()
		return nil, fmt.Errorf("register ask: %w", err)
	}
	// set_model: the agent's own model switch. Tools are composed before the
	// engine exists, so deliver late-binds through topEngine — set right
	// after pi.NewEngine below, strictly before any session can run a turn.
	var topEngine *engine.Engine
	deliver := func(id core.SessionID, in core.Intent) bool {
		if topEngine == nil {
			return false
		}
		return topEngine.Deliver(id, in)
	}
	var setDefault func(string) error
	if prefs != nil {
		setDefault = prefs.SetDefaultModel
	}
	// The provider's live catalog backs list_models and set_model's id check
	// — the same listing the gateway proxies to the web picker, so the agent
	// can name exactly the models the user sees there. Nil (no LLM, no
	// catalog endpoint) keeps set_model blind and skips list_models.
	var catalog setmodel.Catalog
	if u := cfg.Config.ModelsURL(); u != "" {
		catalog = func(ctx context.Context) ([]modelcatalog.Model, error) {
			return modelcatalog.Fetch(ctx, u)
		}
	}
	if err := setmodel.RegisterTool(topTools, deliver, setDefault, catalog); err != nil {
		if theMind != nil {
			theMind.Close()
		}
		mcpMgr.Close()
		return nil, fmt.Errorf("register set_model: %w", err)
	}

	topOpts := piOpts
	topOpts.NewStore = func() ports.SessionStore { return cfg.Store }
	topOpts.Observer = cfg.Observer
	topOpts.ExtraTools = topTools
	// Memory is the self's: recall, curation, and the remember tool wire on the
	// top engine only (the scheduler's wakes reuse it). Children get none.
	topOpts.Mind = theMind

	eng, err := pi.NewEngine(topOpts)
	if err != nil {
		if theMind != nil {
			theMind.Close()
		}
		mcpMgr.Close()
		return nil, fmt.Errorf("build pi engine: %w", err)
	}
	topEngine = eng

	cleanup := func() {
		if theMind != nil {
			theMind.Close()
		}
		mcpMgr.Close()
	}
	return &Host{
		Engine: eng, Cleanup: cleanup, Settings: prefs,
		store: cfg.Store, logger: cfg.Logger, approve: cfg.Approve,
		provider: piOpts.Provider, model: piOpts.Model,
		distill: piOpts.DistillerModel(), models: piOpts.Models, cwd: cwd,
	}, nil
}

type sysClock struct{}

func (sysClock) Now() time.Time { return time.Now() }

// DefaultDBPath is the durable session store for production hosts (~/.config/arbos).
func DefaultDBPath() string {
	if h, err := os.UserHomeDir(); err == nil {
		return filepath.Join(h, ".config", "arbos", "sessions.db")
	}
	return filepath.Join(".arbos", "sessions.db")
}

// OpenStore opens the SQLite session store, creating the parent directory if
// needed so a fresh install does not fail on first run.
func OpenStore(path string) (*sqlite.Store, error) {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return nil, fmt.Errorf("create store dir: %w", err)
	}
	return sqlite.Open(path)
}

func NewSessionID() core.SessionID {
	var b [6]byte
	_, _ = rand.Read(b[:])
	return core.SessionID(fmt.Sprintf("sess-%d-%s", time.Now().UnixNano(), hex.EncodeToString(b[:])))
}

func AgentConfigDir() string {
	if h, err := os.UserHomeDir(); err == nil {
		return filepath.Join(h, ".config", "arbos")
	}
	return ""
}

func NewLogger() *slog.Logger {
	return slog.New(obs.NewRedactingHandler(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: slog.LevelInfo})))
}

// NewQuietLogger routes diagnostics (tool traces, memory-curation failures) to a
// debug file under the agent config dir instead of the console, so an
// interactive one-shot run shows only its rendered transcript. Observability is
// preserved — tail ~/.config/arbos/arbos.log — without corrupting the frontend.
// If no log file can be opened it discards, since polluting stdout/stderr is
// worse than losing best-effort diagnostics.
func NewQuietLogger() *slog.Logger {
	discard := slog.New(slog.NewTextHandler(io.Discard, &slog.HandlerOptions{Level: slog.LevelError}))
	dir := AgentConfigDir()
	if dir == "" {
		return discard
	}
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return discard
	}
	f, err := os.OpenFile(filepath.Join(dir, "arbos.log"), os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o644)
	if err != nil {
		return discard
	}
	return slog.New(obs.NewRedactingHandler(slog.NewTextHandler(f, &slog.HandlerOptions{Level: slog.LevelInfo})))
}
