package piwire

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"io"
	"log/slog"
	"net/url"
	"os"
	"path/filepath"
	"time"

	"github.com/unarbos/arbos/internal/agent"
	"github.com/unarbos/arbos/internal/agent/pi"
	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/extension"
	"github.com/unarbos/arbos/internal/extension/builtin"
	"github.com/unarbos/arbos/internal/fake"
	"github.com/unarbos/arbos/internal/mcp"
	"github.com/unarbos/arbos/internal/mind"
	"github.com/unarbos/arbos/internal/obs"
	"github.com/unarbos/arbos/internal/ports"
	"github.com/unarbos/arbos/internal/provider/anthropic"
	"github.com/unarbos/arbos/internal/provider/google"
	"github.com/unarbos/arbos/internal/provider/openai"
	"github.com/unarbos/arbos/internal/secret"
	"github.com/unarbos/arbos/internal/sqlite"
	"github.com/unarbos/arbos/internal/tool"
	"github.com/unarbos/arbos/internal/tool/codingspec"
)

// Host wires a pi session host. Store and Observer are required for production;
// delegated children always get an ephemeral in-memory store.
type HostConfig struct {
	Store    ports.SessionStore
	Observer ports.Observer
	Approve  bool
	// Logger receives background diagnostics (e.g. memory curation failures). It
	// MUST write to a sink that won't corrupt the frontend — leave nil under the
	// TUI (where stderr is the transcript) to discard them.
	Logger *slog.Logger
}

// Host bundles the assembled engine with a cleanup hook for MCP subprocesses.
type Host struct {
	Engine  *engine.Engine
	Router  *agent.Router
	Cleanup func()
}

// Assemble builds the top-level pi engine with delegation tools, memory, MCP, and
// built-in extensions registered on the returned router.
func Assemble(cfg HostConfig) (*Host, error) {
	cwd, _ := os.Getwd()
	agentDir := AgentConfigDir()

	mcpCfg, err := mcp.LoadConfig(cwd, agentDir)
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

	piOpts := pi.Options{
		Provider:       BuildProvider(),
		NewStore:       func() ports.SessionStore { return fake.NewStore() },
		Clock:          sysClock{},
		Cwd:            cwd,
		AgentDir:       agentDir,
		Model:          ModelName(),
		Models:         pi.SeededModelRegistry(),
		DistillModel:   os.Getenv("ARBOS_DISTILL_MODEL"),
		CacheRetention: core.CacheShort,
		ExtraRuntimes:  extraRT,
	}
	if cfg.Approve {
		piOpts.Approval = pi.CodingApprovalPolicy{}
	}

	router := agent.NewRouter()
	if err := pi.Register(router, piOpts, NewSessionID); err != nil {
		mcpMgr.Close()
		return nil, fmt.Errorf("register pi: %w", err)
	}
	delegation := tool.New()
	if err := agent.RegisterDelegate(delegation, router, codingReadOnlyTools()); err != nil {
		mcpMgr.Close()
		return nil, fmt.Errorf("register delegate: %w", err)
	}

	topOpts := piOpts
	topOpts.NewStore = func() ports.SessionStore { return cfg.Store }
	topOpts.Observer = cfg.Observer
	topOpts.ExtraTools = delegation

	// Long-term memory: only when the durable store backs atoms (SQLite, not the
	// in-memory fake) and a real LLM is configured to curate. Attached to the
	// top-level engine only; delegated children share the same global atoms
	// through it. The same DB across every session is what makes this one agent
	// that learns everywhere.
	var theMind *mind.Mind
	if as, ok := cfg.Store.(mind.Store); ok && HasLLMConfigured() {
		// The curator runs on the same distill model as the compaction
		// summarizer: one tier for everything the agent does in the background.
		theMind = mind.New(as, piOpts.Provider, piOpts.DistillerModel(), cfg.Logger)
		topOpts.Mind = theMind
	}

	eng, err := pi.NewEngine(topOpts)
	if err != nil {
		if theMind != nil {
			theMind.Close()
		}
		mcpMgr.Close()
		return nil, fmt.Errorf("build pi engine: %w", err)
	}

	cleanup := func() {
		if theMind != nil {
			theMind.Close()
		}
		mcpMgr.Close()
	}
	return &Host{Engine: eng, Router: router, Cleanup: cleanup}, nil
}

// codingReadOnlyTools is the set of coding tool names that do not mutate the
// workspace, sourced from the specs themselves so it cannot drift from their
// ReadOnly flags. A delegation confined to these is safe to fan out in parallel
// (see RegisterDelegate).
func codingReadOnlyTools() map[string]bool {
	out := map[string]bool{}
	for _, s := range codingspec.Specs("") {
		if s.ReadOnly {
			out[s.Name] = true
		}
	}
	return out
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

// BuildProvider selects the LLM adapter from ARBOS_PROVIDER (openai, anthropic,
// google) with env-based credentials. With no provider configured it returns the
// deterministic fake, which exercises the pi coding toolset.
func BuildProvider() ports.LLMProvider {
	switch os.Getenv("ARBOS_PROVIDER") {
	case "anthropic":
		base := os.Getenv("ARBOS_ANTHROPIC_BASE_URL")
		if base == "" {
			base = "https://api.anthropic.com"
		}
		return anthropic.New(base, anthropic.WithAuth(brokerForHost(base, "ARBOS_ANTHROPIC_API_KEY"), core.SecretRef{Name: "ARBOS_ANTHROPIC_API_KEY"}))
	case "google":
		base := os.Getenv("ARBOS_GOOGLE_BASE_URL")
		if base == "" {
			base = "https://generativelanguage.googleapis.com"
		}
		return google.New(base, google.WithAuth(brokerForHost(base, "ARBOS_GOOGLE_API_KEY"), core.SecretRef{Name: "ARBOS_GOOGLE_API_KEY"}))
	default:
		base, keyEnv := openAIConfig()
		if base == "" {
			return fake.Provider{}
		}
		return openai.New(base, openai.WithAuth(brokerForHost(base, keyEnv), core.SecretRef{Name: keyEnv}))
	}
}

func openAIConfig() (baseURL, keyEnv string) {
	if b := os.Getenv("ARBOS_OPENAI_BASE_URL"); b != "" {
		baseURL = b
	}
	if os.Getenv("ARBOS_OPENAI_API_KEY") != "" {
		return baseURL, "ARBOS_OPENAI_API_KEY"
	}
	if os.Getenv("OPENROUTER_API_KEY") != "" {
		if baseURL == "" {
			baseURL = "https://openrouter.ai/api/v1"
		}
		return baseURL, "OPENROUTER_API_KEY"
	}
	return baseURL, "ARBOS_OPENAI_API_KEY"
}

func brokerForHost(baseURL, secretName string) *secret.Broker {
	host := ""
	if u, err := url.Parse(baseURL); err == nil {
		host = u.Hostname()
	}
	return secret.NewBroker(fake.EnvSecretProvider{}, secret.Binding{
		Ref:    core.SecretRef{Name: secretName},
		Hosts:  []string{host},
		Inject: secret.BearerInjector,
	})
}

func ModelName() string {
	if m := os.Getenv("ARBOS_MODEL"); m != "" {
		return m
	}
	if HasLLMConfigured() {
		return "anthropic/claude-opus-4.8"
	}
	return "fake"
}

// HasLLMConfigured reports whether a real LLM provider will be selected.
func HasLLMConfigured() bool {
	switch os.Getenv("ARBOS_PROVIDER") {
	case "anthropic":
		return os.Getenv("ARBOS_ANTHROPIC_API_KEY") != ""
	case "google":
		return os.Getenv("ARBOS_GOOGLE_API_KEY") != ""
	default:
		_, keyEnv := openAIConfig()
		return os.Getenv(keyEnv) != ""
	}
}

// WarnIfNoLLM prints a one-line hint when no API credentials are configured.
func WarnIfNoLLM(w io.Writer) {
	if HasLLMConfigured() {
		return
	}
	fmt.Fprintf(w, "arbos: no API key configured — set OPENROUTER_API_KEY (or see https://github.com/unarbos/arbos)\n")
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
