package piwire

import (
	"fmt"
	"io"
	"net/url"
	"os"
	"strconv"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/fake"
	"github.com/unarbos/arbos/internal/ports"
	"github.com/unarbos/arbos/internal/provider/anthropic"
	"github.com/unarbos/arbos/internal/provider/google"
	"github.com/unarbos/arbos/internal/provider/openai"
	"github.com/unarbos/arbos/internal/secret"
)

// openRouterModel is the default model for the OpenRouter onboarding path,
// where model ids use provider-slug form.
const openRouterModel = "anthropic/claude-opus-4.8"

// Config is the host's resolved environment: which provider adapter to build,
// where it lives, which env var holds its key, and which models to run.
// LoadConfig is the only place the provider/model environment is read, so the
// full env-var surface is auditable in this one file. Secret values never
// enter Config — only the env var's name; the broker resolves it per request.
type Config struct {
	ProviderName string // "openai", "anthropic", or "google"
	BaseURL      string
	KeyEnv       string // env var naming the API key
	HasLLM       bool   // a key is present; false selects the deterministic fake
	Model        string
	DistillModel string
	// MCPConfigJSON is an inline MCP server config (ARBOS_MCP_CONFIG); empty
	// falls back to the mcp.json files.
	MCPConfigJSON string
	// ServeDrainTimeout bounds how long `arbos -serve` waits for an in-flight
	// turn to finish after the client disconnects. Zero means control.Serve's
	// default.
	ServeDrainTimeout time.Duration
}

// providerSpec is the per-provider composition table: default endpoint, the
// env vars that configure it, and how the broker injects the credential
// (ADR-0028: Anthropic wants x-api-key, Google x-goog-api-key, OpenAI Bearer).
type providerSpec struct {
	baseEnv     string
	defaultBase string
	keyEnv      string
	// defaultModel is used when ARBOS_MODEL is unset; it must be an id the
	// provider's own API accepts (the OpenRouter path overrides it with the
	// slug-form id that gateway expects).
	defaultModel string
	inject       secret.Injector
	build        func(baseURL string, broker *secret.Broker, ref core.SecretRef) ports.LLMProvider
}

var providerSpecs = map[string]providerSpec{
	"openai": {
		baseEnv:      "ARBOS_OPENAI_BASE_URL",
		defaultBase:  "https://api.openai.com/v1",
		keyEnv:       "ARBOS_OPENAI_API_KEY",
		defaultModel: "gpt-5",
		inject:       secret.BearerInjector,
		build: func(base string, br *secret.Broker, ref core.SecretRef) ports.LLMProvider {
			return openai.New(base, openai.WithAuth(br, ref))
		},
	},
	"anthropic": {
		baseEnv:      "ARBOS_ANTHROPIC_BASE_URL",
		defaultBase:  "https://api.anthropic.com",
		keyEnv:       "ARBOS_ANTHROPIC_API_KEY",
		defaultModel: "claude-opus-4-20250514",
		inject:       secret.HeaderInjector("x-api-key"),
		build: func(base string, br *secret.Broker, ref core.SecretRef) ports.LLMProvider {
			return anthropic.New(base, anthropic.WithAuth(br, ref))
		},
	},
	"google": {
		baseEnv:      "ARBOS_GOOGLE_BASE_URL",
		defaultBase:  "https://generativelanguage.googleapis.com",
		keyEnv:       "ARBOS_GOOGLE_API_KEY",
		defaultModel: "gemini-2.5-pro",
		inject:       secret.HeaderInjector("x-goog-api-key"),
		build: func(base string, br *secret.Broker, ref core.SecretRef) ports.LLMProvider {
			return google.New(base, google.WithAuth(br, ref))
		},
	},
}

// LoadConfig resolves the host configuration from the environment.
func LoadConfig() Config {
	name := os.Getenv("ARBOS_PROVIDER")
	spec, ok := providerSpecs[name]
	if !ok {
		name = "openai"
		spec = providerSpecs[name]
	}

	base := os.Getenv(spec.baseEnv)
	keyEnv := spec.keyEnv
	defaultModel := spec.defaultModel
	// OpenRouter onboarding path: an OpenRouter key with no explicit OpenAI key
	// routes the OpenAI-compatible adapter at openrouter.ai, whose model ids
	// use slug form.
	if name == "openai" && os.Getenv(keyEnv) == "" && os.Getenv("OPENROUTER_API_KEY") != "" {
		keyEnv = "OPENROUTER_API_KEY"
		defaultModel = openRouterModel
		if base == "" {
			base = "https://openrouter.ai/api/v1"
		}
	}
	if base == "" {
		base = spec.defaultBase
	}

	cfg := Config{
		ProviderName:  name,
		BaseURL:       base,
		KeyEnv:        keyEnv,
		HasLLM:        os.Getenv(keyEnv) != "",
		Model:         os.Getenv("ARBOS_MODEL"),
		DistillModel:  os.Getenv("ARBOS_DISTILL_MODEL"),
		MCPConfigJSON: os.Getenv("ARBOS_MCP_CONFIG"),
	}
	if cfg.Model == "" {
		cfg.Model = "fake"
		if cfg.HasLLM {
			cfg.Model = defaultModel
		}
	}
	// Whole seconds, matching the var's original contract. Zero is fine:
	// control.Serve owns the default, the same callee-defaults-zero convention
	// as engine.New's MaxIterations.
	if sec, err := strconv.Atoi(os.Getenv("ARBOS_SERVE_DRAIN_TIMEOUT")); err == nil && sec > 0 {
		cfg.ServeDrainTimeout = time.Duration(sec) * time.Second
	}
	return cfg
}

// NewProvider builds the configured LLM adapter. Without a key it returns the
// deterministic fake — for every provider, not just the default — so a keyless
// host always degrades to the same offline mode instead of failing at the
// first authorize.
func (c Config) NewProvider() ports.LLMProvider {
	if !c.HasLLM {
		return fake.Provider{}
	}
	spec, ok := providerSpecs[c.ProviderName]
	if !ok {
		// A hand-built Config with an unrecognized name gets the same
		// normalization LoadConfig applies, instead of a nil-func panic.
		spec = providerSpecs["openai"]
	}
	host := ""
	if u, err := url.Parse(c.BaseURL); err == nil {
		host = u.Hostname()
	}
	ref := core.SecretRef{Name: c.KeyEnv}
	broker := secret.NewBroker(secret.EnvProvider{}, secret.Binding{
		Ref:    ref,
		Hosts:  []string{host},
		Inject: spec.inject,
	})
	return spec.build(c.BaseURL, broker, ref)
}

// WarnIfNoLLM prints a one-line hint when no API credentials are configured.
func (c Config) WarnIfNoLLM(w io.Writer) {
	if c.HasLLM {
		return
	}
	_, _ = fmt.Fprintf(w, "arbos: no API key configured — set OPENROUTER_API_KEY (or see https://github.com/unarbos/arbos)\n")
}
