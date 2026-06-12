package piwire

import (
	"fmt"
	"io"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/fake"
	"github.com/unarbos/arbos/internal/ports"
	"github.com/unarbos/arbos/internal/provider/anthropic"
	"github.com/unarbos/arbos/internal/provider/google"
	"github.com/unarbos/arbos/internal/provider/openai"
	"github.com/unarbos/arbos/internal/secret"
	"github.com/unarbos/arbos/internal/settings"
)

// OpenRouterBase is the OpenRouter endpoint the onboarding path defaults to —
// exported so the web door's provider panel can show where a freshly pasted
// key will route before any key exists.
const OpenRouterBase = "https://openrouter.ai/api/v1"

// openRouterModel is the default model for the OpenRouter onboarding path,
// where model ids use provider-slug form.
const openRouterModel = "anthropic/claude-opus-4.8"

// openRouterImageModel is the default backing model for OpenRouter's
// image_generation server tool on the onboarding path. The tool's own default
// (openai/gpt-5-image) is overridden because Gemini's image model is cheaper,
// fast, and not subject to OpenAI's separate account gating; ARBOS_IMAGE_MODEL
// overrides it.
const openRouterImageModel = "google/gemini-2.5-flash-image"

// Config is the host's resolved environment: which provider adapter to build,
// where it lives, which env var holds its key, and which models to run.
// LoadConfig is the only place the provider/model environment is read, so the
// full env-var surface is auditable in this one file — plus the two stored
// surfaces the Settings tab writes: the host preference file (endpoint) and
// the secret vault (key). Secret values never enter Config — only the key's
// name; the broker resolves it per request from env or the vault.
type Config struct {
	ProviderName string // "openai", "anthropic", or "google"
	BaseURL      string
	KeyEnv       string // name of the API key (env var, and vault entry)
	HasLLM       bool   // a key is present; false selects the deterministic fake
	Model        string
	DistillModel string
	// Vault is the managed-secret store opened from the agent config dir, the
	// request-time fallback when the key is not exported in the environment.
	// Nil when no config dir exists or the vault failed to open (env-only).
	Vault *secret.Store
	// ImageModel is the backing model for provider-side image generation
	// (ARBOS_IMAGE_MODEL). Empty leaves the endpoint's own default; the
	// OpenRouter path defaults it to openRouterImageModel.
	ImageModel string
	// Reasoning is the requested thinking effort for every turn
	// (ARBOS_REASONING: off|minimal|low|medium|high). Empty leaves the
	// provider default (usually no reasoning stream).
	Reasoning core.ReasoningLevel
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
	build        func(cfg Config, broker *secret.Broker, ref core.SecretRef) ports.LLMProvider
}

var providerSpecs = map[string]providerSpec{
	"openai": {
		baseEnv:      "ARBOS_OPENAI_BASE_URL",
		defaultBase:  "https://api.openai.com/v1",
		keyEnv:       "ARBOS_OPENAI_API_KEY",
		defaultModel: "gpt-5",
		inject:       secret.BearerInjector,
		build: func(cfg Config, br *secret.Broker, ref core.SecretRef) ports.LLMProvider {
			opts := []openai.Option{openai.WithAuth(br, ref)}
			if cfg.ImageModel != "" {
				opts = append(opts, openai.WithImageModel(cfg.ImageModel))
			}
			return openai.New(cfg.BaseURL, opts...)
		},
	},
	"anthropic": {
		baseEnv:      "ARBOS_ANTHROPIC_BASE_URL",
		defaultBase:  "https://api.anthropic.com",
		keyEnv:       "ARBOS_ANTHROPIC_API_KEY",
		defaultModel: "claude-opus-4-20250514",
		inject:       secret.HeaderInjector("x-api-key"),
		build: func(cfg Config, br *secret.Broker, ref core.SecretRef) ports.LLMProvider {
			return anthropic.New(cfg.BaseURL, anthropic.WithAuth(br, ref))
		},
	},
	"google": {
		baseEnv:      "ARBOS_GOOGLE_BASE_URL",
		defaultBase:  "https://generativelanguage.googleapis.com",
		keyEnv:       "ARBOS_GOOGLE_API_KEY",
		defaultModel: "gemini-2.5-pro",
		inject:       secret.HeaderInjector("x-goog-api-key"),
		build: func(cfg Config, br *secret.Broker, ref core.SecretRef) ports.LLMProvider {
			return google.New(cfg.BaseURL, google.WithAuth(br, ref))
		},
	},
}

// LoadConfig resolves the host configuration from the environment, backed by
// the two stores the Settings tab writes: the host preference file (endpoint)
// and the secret vault (key). Env wins where both exist — an exported var is
// launch-explicit; the stored values are the durable default underneath.
func LoadConfig() Config {
	name := os.Getenv("ARBOS_PROVIDER")
	spec, ok := providerSpecs[name]
	if !ok {
		name = "openai"
		spec = providerSpecs[name]
	}

	stored := storedSettings()
	vault := openVault()
	// hasKey is presence, not value: a vault hit means the broker can resolve
	// the name at request time, exactly like an exported env var.
	hasKey := func(keyName string) bool {
		return os.Getenv(keyName) != "" || vaultHas(vault, keyName)
	}

	base := os.Getenv(spec.baseEnv)
	if base == "" {
		base = stored.LLMBaseURL
	}
	keyEnv := spec.keyEnv
	defaultModel := spec.defaultModel
	// OpenRouter onboarding path: an OpenRouter key with no explicit OpenAI key
	// routes the OpenAI-compatible adapter at openrouter.ai, whose model ids
	// use slug form.
	if name == "openai" && !hasKey(keyEnv) && hasKey("OPENROUTER_API_KEY") {
		keyEnv = "OPENROUTER_API_KEY"
		defaultModel = openRouterModel
		if base == "" {
			base = OpenRouterBase
		}
	}
	imageModel := os.Getenv("ARBOS_IMAGE_MODEL")
	if imageModel == "" && strings.Contains(base, "openrouter.ai") {
		imageModel = openRouterImageModel
	}
	if base == "" {
		base = spec.defaultBase
	}

	cfg := Config{
		ProviderName:  name,
		BaseURL:       base,
		KeyEnv:        keyEnv,
		HasLLM:        hasKey(keyEnv),
		Vault:         vault,
		Model:         os.Getenv("ARBOS_MODEL"),
		DistillModel:  os.Getenv("ARBOS_DISTILL_MODEL"),
		ImageModel:    imageModel,
		Reasoning:     core.ReasoningLevel(os.Getenv("ARBOS_REASONING")),
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

// storedSettings reads the host preference file (the Settings tab's durable
// knobs). Any failure is an empty preference set — config resolution must
// never die on a corrupt optional file.
func storedSettings() settings.Settings {
	dir := AgentConfigDir()
	if dir == "" {
		return settings.Settings{}
	}
	st, err := settings.Open(filepath.Join(dir, "settings.json"))
	if err != nil {
		return settings.Settings{}
	}
	return st.Get()
}

// openVault opens the managed-secret vault under the agent config dir. This is
// the same store the web door's Settings tab edits — opened once here and
// shared, so a key saved in the UI is resolvable by the very next request.
// Nil (env-only) when there is no config dir or the vault cannot open.
func openVault() *secret.Store {
	dir := AgentConfigDir()
	if dir == "" {
		return nil
	}
	v, err := secret.Open(filepath.Join(dir, "secrets"))
	if err != nil {
		return nil
	}
	return v
}

// vaultHas reports whether the vault holds an entry by name — presence only,
// the value stays sealed until the broker resolves it at a boundary.
func vaultHas(v *secret.Store, name string) bool {
	if v == nil {
		return false
	}
	for _, e := range v.List() {
		if e.Name == name {
			return true
		}
	}
	return false
}

// keyProvider is the request-time resolution chain for the host's own
// credential: the environment first (launch-explicit), then the vault (the
// Settings tab's stored key).
func (c Config) keyProvider() ports.SecretProvider {
	if c.Vault != nil {
		return secret.Chain{secret.EnvProvider{}, c.Vault}
	}
	return secret.EnvProvider{}
}

// ModelsURL is the provider's model-catalog endpoint, derived from the base
// URL the same way the chat endpoint is. OpenRouter and OpenAI-compatible
// bases serve the listing at {base}/models. Empty when no LLM is configured
// (the fake has no catalog), which leaves the UI's picker with just the
// current model.
func (c Config) ModelsURL() string {
	if !c.HasLLM || c.BaseURL == "" {
		return ""
	}
	return strings.TrimRight(c.BaseURL, "/") + "/models"
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
	broker := secret.NewBroker(c.keyProvider(), secret.Binding{
		Ref:    ref,
		Hosts:  []string{host},
		Inject: spec.inject,
	})
	return spec.build(c, broker, ref)
}

// WarnIfNoLLM prints a one-line hint when no API credentials are configured.
func (c Config) WarnIfNoLLM(w io.Writer) {
	if c.HasLLM {
		return
	}
	_, _ = fmt.Fprintf(w, "arbos: no API key configured — set OPENROUTER_API_KEY (or see https://github.com/unarbos/arbos)\n")
}
