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
	"github.com/unarbos/arbos/internal/mcp"
	"github.com/unarbos/arbos/internal/mind"
	"github.com/unarbos/arbos/internal/modelcatalog"
	"github.com/unarbos/arbos/internal/obs"
	"github.com/unarbos/arbos/internal/outbox"
	"github.com/unarbos/arbos/internal/plan"
	"github.com/unarbos/arbos/internal/ports"
	"github.com/unarbos/arbos/internal/recall"
	"github.com/unarbos/arbos/internal/setmodel"
	"github.com/unarbos/arbos/internal/settings"
	"github.com/unarbos/arbos/internal/sqlite"
	"github.com/unarbos/arbos/internal/tool"
)

// HostConfig wires a pi session host. Config carries the resolved environment
// (provider, model; see LoadConfig); Store and Observer are required for
// production. Delegated children share that same durable Store — their
// transcripts persist and replay like any session; only memory (Mind) stays
// the self's.
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

// ReclaimOrphanedRuns retires machine-spawned sessions (delegated children and
// scheduler wakes) that a crash left flagged active, so they stop showing as
// perpetually-live runs in the activity feed. It is a startup pass the caller
// runs while holding the directory lock — the previous owner is gone, so any
// still-active run is an orphan (see sqlite.Store.ReclaimOrphanedRuns). A no-op
// when the store does not track sessions this way (the in-memory fake).
func (h *Host) ReclaimOrphanedRuns(ctx context.Context) (int, error) {
	r, ok := h.store.(interface {
		ReclaimOrphanedRuns(context.Context) (int, error)
	})
	if !ok {
		return 0, nil
	}
	return r.ReclaimOrphanedRuns(ctx)
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
		Provider: cfg.Config.NewProvider(),
		// The top-level chat and every delegated child share the one durable
		// store, so a sub-agent's transcript replays like any other session
		// (see Options.Store).
		Store:          cfg.Store,
		Clock:          sysClock{},
		Cwd:            cwd,
		AgentDir:       agentDir,
		Model:          cfg.Config.Model,
		Models:         pi.SeededModelRegistry(),
		DistillModel:   cfg.Config.DistillModel,
		Reasoning:      cfg.Config.Reasoning,
		CacheRetention: core.CacheShort,
		FallbackModels: cfg.Config.FallbackModels,
		RetryAttempts:  cfg.Config.RetryAttempts,
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
	// the scheduler's wakes reuse), not on delegated children. A child now
	// persists its events to the same store, so recall could anchor — but
	// memory writes deliberately stay at the self: a child is the agent's
	// exposure to untrusted input (arbitrary repos, web pages), and atoms are
	// one global, unscoped set, so letting a child curate or remember would be
	// a memory-poisoning surface. Children pass findings up via results; the
	// self decides what to remember. Created here (before pi.Register) only so
	// it can be Closed on an early return.
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
	// sessions: read the agent's other chats and autonomous runs. Wired only
	// when the durable store backs them (SQLite, not the in-memory fake) — a
	// fake has no cross-session history to recall. The reader spawns through
	// the same router as delegate, so a review's big transcript lands in the
	// sub-agent's context and only its digest returns to the calling chat.
	if st, ok := cfg.Store.(*sqlite.Store); ok {
		reader := func(ctx context.Context, instruction string) (string, string, error) {
			ag, err := router.Resolve("")
			if err != nil {
				return "", "", err
			}
			c, _ := obs.From(ctx)
			res, err := ag.Run(ctx, agent.Task{
				Instruction: instruction,
				Owner:       core.SessionID(c.SessionID),
				SpawnedBy:   "review",
				// The reader's prompt is assembled from raw transcript text,
				// which can carry content the agent ingested from untrusted
				// sources. Confine it to read-only tools so an injected
				// instruction can never turn a review into a write or a shell
				// command — the surface that matters since sessions rides the
				// top engine the scheduler's unattended wakes reuse. An empty
				// allowlist would mean "no restriction" (tool.Filter), i.e. the
				// full toolset, so this list must stay non-empty.
				Grant: agent.Grant{Tools: []string{"read", "ls", "find", "grep"}},
			}, engine.Relay(ctx))
			if err != nil {
				return "", "", err
			}
			return res.Text, res.ChildSession, nil
		}
		if err := recall.RegisterTool(topTools, recallStore{st}, reader); err != nil {
			if theMind != nil {
				theMind.Close()
			}
			mcpMgr.Close()
			return nil, fmt.Errorf("register sessions: %w", err)
		}
	}

	topOpts := piOpts
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

// recallStore adapts the concrete SQLite store to recall.Store, mapping the
// store's RecentSession rows into the tool's sqlite-free SessionInfo. Events
// passes straight through (the signatures already match).
type recallStore struct{ s *sqlite.Store }

func (r recallStore) Events(ctx context.Context, id core.SessionID) ([]core.Event, error) {
	return r.s.Events(ctx, id)
}

func (r recallStore) RecentSessions(ctx context.Context, limit int) ([]recall.SessionInfo, error) {
	rows, err := r.s.RecentSessions(ctx, limit)
	if err != nil {
		return nil, err
	}
	out := make([]recall.SessionInfo, len(rows))
	for i, row := range rows {
		out[i] = recall.SessionInfo{
			ID:        row.ID,
			Kind:      row.Kind,
			Title:     row.Title,
			Status:    row.Status,
			UpdatedAt: row.UpdatedAt,
		}
	}
	return out, nil
}

// DefaultDBPath is the agent's brain — sessions, mind (atoms), plans, and
// callbacks — scoped to one directory at <cwd>/.arbos/sessions.db. Two sibling
// directories are two separate minds that do not know about each other: the
// path is relative, resolved against the working directory the agent opens in
// (after any --workspace / `arbos <dir>` chdir), so the brain always belongs to
// the directory it runs in. Merging brains across directories is a future
// capability, not a default.
func DefaultDBPath() string {
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
