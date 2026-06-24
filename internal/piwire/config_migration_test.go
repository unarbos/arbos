package piwire

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"github.com/unarbos/arbos/internal/secret"
	"github.com/unarbos/arbos/internal/settings"
)

// isolate points AgentConfigDir at a temp HOME and clears the provider env so
// each case resolves only from the files it seeds. It returns the config dir
// (~/.config/arbos) for seeding settings.json and the vault.
func isolate(t *testing.T) string {
	t.Helper()
	home := t.TempDir()
	t.Setenv("HOME", home)
	// Clear every var LoadConfig reads so the test controls resolution.
	for _, k := range []string{
		"ARBOS_PROVIDER", "ARBOS_MODEL", "ARBOS_OPENAI_API_KEY", "ARBOS_OPENAI_BASE_URL",
		"ARBOS_ANTHROPIC_API_KEY", "ARBOS_ANTHROPIC_BASE_URL", "ARBOS_GOOGLE_API_KEY",
		"ARBOS_GOOGLE_BASE_URL", "OPENROUTER_API_KEY", "ARBOS_IMAGE_MODEL",
		"ARBOS_DISTILL_MODEL", "ARBOS_REASONING", "ARBOS_FALLBACK_MODELS", "ARBOS_LLM_RETRIES",
	} {
		t.Setenv(k, "")
	}
	dir := filepath.Join(home, ".config", "arbos")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	return dir
}

func seedSettings(t *testing.T, dir string, s settings.Settings) {
	t.Helper()
	b, _ := json.MarshalIndent(s, "", "  ")
	if err := os.WriteFile(filepath.Join(dir, "settings.json"), b, 0o644); err != nil {
		t.Fatal(err)
	}
}

func seedVaultKey(t *testing.T, dir, name string) {
	t.Helper()
	v, err := secret.Open(filepath.Join(dir, "secrets"))
	if err != nil {
		t.Fatal(err)
	}
	if err := v.Set(secret.Entry{Name: name}, "sk-test-value"); err != nil {
		t.Fatal(err)
	}
}

// A pre-existing install — legacy scalar fields, key under OPENROUTER_API_KEY,
// no providers[] — must resolve byte-identically to the old single-provider
// behavior (the lazy-migration fall-through).
func TestLoadConfigLegacyOpenRouterUnchanged(t *testing.T) {
	dir := isolate(t)
	seedSettings(t, dir, settings.Settings{
		LLMBaseURL:   OpenRouterBase,
		DefaultModel: "anthropic/claude-opus-4.8",
	})
	seedVaultKey(t, dir, "OPENROUTER_API_KEY")

	cfg := LoadConfig()
	if cfg.ProviderName != "openai" {
		t.Errorf("provider = %q, want openai", cfg.ProviderName)
	}
	if cfg.BaseURL != OpenRouterBase {
		t.Errorf("base = %q, want %q", cfg.BaseURL, OpenRouterBase)
	}
	if cfg.KeyEnv != "OPENROUTER_API_KEY" {
		t.Errorf("keyEnv = %q, want OPENROUTER_API_KEY", cfg.KeyEnv)
	}
	if !cfg.HasLLM {
		t.Error("HasLLM should be true with a seeded key")
	}
	if cfg.Model != "anthropic/claude-opus-4.8" {
		t.Errorf("model = %q, want the stored default", cfg.Model)
	}
}

// A new-world file with providers[] selects the active entry, resolves its own
// vault key name, endpoint, and default model.
func TestLoadConfigSelectsActiveProvider(t *testing.T) {
	dir := isolate(t)
	seedSettings(t, dir, settings.Settings{
		// Legacy mirror present (pointed at the active entry, as the store writes it).
		LLMBaseURL:       "https://api.saygm.com/v1",
		DefaultModel:     "gpt-5.5",
		ActiveProviderID: "gm",
		Providers: []settings.ProviderEntry{
			{ID: "or", Label: "OpenRouter", ProviderName: "openai", Endpoint: OpenRouterBase, KeyVaultName: "LLM_KEY_or", DefaultModel: "anthropic/claude-opus-4.8"},
			{ID: "gm", Label: "gm", ProviderName: "openai", Endpoint: "https://api.saygm.com/v1", KeyVaultName: "LLM_KEY_gm", DefaultModel: "gpt-5.5"},
		},
	})
	seedVaultKey(t, dir, "LLM_KEY_gm")

	cfg := LoadConfig()
	if cfg.BaseURL != "https://api.saygm.com/v1" {
		t.Errorf("base = %q, want gm endpoint", cfg.BaseURL)
	}
	if cfg.KeyEnv != "LLM_KEY_gm" {
		t.Errorf("keyEnv = %q, want LLM_KEY_gm (per-provider vault name)", cfg.KeyEnv)
	}
	if !cfg.HasLLM {
		t.Error("HasLLM should be true — the active provider's key is seeded")
	}
	if cfg.Model != "gpt-5.5" {
		t.Errorf("model = %q, want gpt-5.5 (active entry default)", cfg.Model)
	}
}

// Selecting a provider whose key is NOT seeded resolves HasLLM=false (the
// active entry's own key name is what's checked, not some other provider's).
func TestLoadConfigActiveWithoutKeyIsKeyless(t *testing.T) {
	dir := isolate(t)
	seedSettings(t, dir, settings.Settings{
		ActiveProviderID: "anthropic",
		Providers: []settings.ProviderEntry{
			{ID: "anthropic", ProviderName: "anthropic", KeyVaultName: "LLM_KEY_anthropic"},
		},
	})
	// No vault key seeded.
	cfg := LoadConfig()
	if cfg.ProviderName != "anthropic" {
		t.Errorf("provider = %q, want anthropic", cfg.ProviderName)
	}
	if cfg.HasLLM {
		t.Error("HasLLM should be false without a key for the active provider")
	}
	// Keyless resolves to the fake model id, matching single-provider behavior.
	if cfg.Model != "fake" {
		t.Errorf("model = %q, want fake when keyless", cfg.Model)
	}
}

// The native anthropic adapter's default base is used when the active entry
// leaves Endpoint empty.
func TestLoadConfigActiveEmptyEndpointUsesAdapterDefault(t *testing.T) {
	dir := isolate(t)
	seedSettings(t, dir, settings.Settings{
		ActiveProviderID: "an",
		Providers: []settings.ProviderEntry{
			{ID: "an", ProviderName: "anthropic", KeyVaultName: "LLM_KEY_an"},
		},
	})
	seedVaultKey(t, dir, "LLM_KEY_an")
	cfg := LoadConfig()
	if cfg.BaseURL != "https://api.anthropic.com" {
		t.Errorf("base = %q, want the anthropic adapter default", cfg.BaseURL)
	}
	if cfg.ProviderName != "anthropic" {
		t.Errorf("provider = %q, want anthropic", cfg.ProviderName)
	}
}

// An explicit ARBOS_PROVIDER env pin wins over the stored active selection —
// env stays launch-explicit.
func TestLoadConfigEnvProviderOverridesStoredActive(t *testing.T) {
	dir := isolate(t)
	t.Setenv("ARBOS_PROVIDER", "google")
	t.Setenv("ARBOS_GOOGLE_API_KEY", "g-key")
	seedSettings(t, dir, settings.Settings{
		ActiveProviderID: "gm",
		Providers: []settings.ProviderEntry{
			{ID: "gm", ProviderName: "openai", Endpoint: "https://api.saygm.com/v1", KeyVaultName: "LLM_KEY_gm"},
		},
	})
	cfg := LoadConfig()
	if cfg.ProviderName != "google" {
		t.Errorf("provider = %q, want google (env pin wins)", cfg.ProviderName)
	}
	if cfg.BaseURL == "https://api.saygm.com/v1" {
		t.Error("stored active endpoint must not override an explicit env provider")
	}
}
