// Package settings is the host's durable preference file: the small set of
// agent-behavior knobs the Settings tab edits and every door (web, TUI,
// one-shot, serve) reads at assembly. It is deliberately not the browser's
// localStorage settings store (purely cosmetic, per-browser) and not the
// secret vault (write-only values) — these are plain, non-secret preferences
// that must reach the Go host, so they persist as one JSON file in the agent
// config dir and survive restarts on this machine.
package settings

import (
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"sync"
)

// Settings is the host preference set. Unknown keys in the file are dropped on
// the next save; missing keys take zero values — the shape can grow without
// migrations, mirroring the web store's defaults-merge discipline.
type Settings struct {
	// DefaultModel is the model new sessions start on, read by LoadConfig at
	// assembly. Empty falls back to ARBOS_MODEL / the provider default. The
	// agent's set_model tool writes it (default:true), so the agent can
	// repoint itself durably; an exported ARBOS_MODEL still wins (env is
	// launch-explicit).
	DefaultModel string `json:"default_model,omitempty"`
	// SubagentModel is the model delegated sub-agents run on. Empty follows
	// the main model.
	SubagentModel string `json:"subagent_model,omitempty"`
	// LLMBaseURL is the chat provider's endpoint, set from the Settings tab.
	// Empty falls back to the environment / provider default. The matching
	// API key is not here — it lives in the secret vault (values are never
	// plaintext on disk); this file carries only non-secret preferences.
	//
	// With multiple configured providers (Providers below) this becomes a
	// DOWNGRADE MIRROR: mutations keep it pointed at the active provider's
	// endpoint, so an older binary that doesn't understand Providers still
	// resolves the active endpoint from this field (ADR-0040). DefaultModel is
	// mirrored the same way.
	LLMBaseURL string `json:"llm_base_url,omitempty"`
	// Providers is the set of configured LLM providers (ADR-0040). Empty on a
	// pre-existing install or a fresh one — LoadConfig then synthesizes a
	// single entry from the legacy fields above, so the file only grows the
	// array once the user saves through the new provider panel (no eager
	// migration). Each entry names its own vault key (KeyVaultName); secrets
	// never live here.
	Providers []ProviderEntry `json:"providers,omitempty"`
	// ActiveProviderID selects which entry in Providers is live. Empty (or an
	// id no longer present) falls back to the first entry. Ignored when
	// Providers is empty.
	ActiveProviderID string `json:"active_provider_id,omitempty"`
}

// ProviderEntry is one configured LLM provider: where requests go, which
// adapter shapes them, and the vault entry holding its key. All non-secret —
// the key value lives in the vault under KeyVaultName, never in this file.
type ProviderEntry struct {
	// ID is a stable, opaque identifier used by ActiveProviderID and as the
	// basis for KeyVaultName. Assigned on add; never reused.
	ID string `json:"id"`
	// Label is the human name shown in the panel and the composer chip
	// (e.g. "OpenRouter", "gm"). Free text; defaults to the provider name.
	Label string `json:"label,omitempty"`
	// ProviderName is the adapter to build: "openai", "anthropic", or
	// "google" (the providerSpecs keys; ADR-0028).
	ProviderName string `json:"provider_name"`
	// Endpoint is the base URL; empty uses the adapter's default base.
	Endpoint string `json:"endpoint,omitempty"`
	// KeyVaultName is the vault entry holding this provider's API key, so
	// multiple providers' keys coexist under distinct names (ADR-0040). Empty
	// means no key configured for this entry yet.
	KeyVaultName string `json:"key_vault_name,omitempty"`
	// DefaultModel is the model this provider starts sessions on; empty falls
	// back to the adapter default.
	DefaultModel string `json:"default_model,omitempty"`
}

// Active returns the selected provider entry and true, or the zero entry and
// false when none is configured. The selection is ActiveProviderID when it
// names a present entry, else the first entry (a stable, predictable fallback
// for a stale or empty id).
func (s Settings) Active() (ProviderEntry, bool) {
	if len(s.Providers) == 0 {
		return ProviderEntry{}, false
	}
	if s.ActiveProviderID != "" {
		for _, p := range s.Providers {
			if p.ID == s.ActiveProviderID {
				return p, true
			}
		}
	}
	return s.Providers[0], true
}

// Store is the file-backed settings holder. It is safe for concurrent use by
// the gateway's handlers and the per-delegation reads on the agent side.
type Store struct {
	mu   sync.Mutex
	path string
	cur  Settings
}

// Open loads the settings file at path. A missing file is an empty store, not
// an error — a fresh install has no preferences yet.
func Open(path string) (*Store, error) {
	s := &Store{path: path}
	b, err := os.ReadFile(path)
	if errors.Is(err, fs.ErrNotExist) {
		return s, nil
	}
	if err != nil {
		return nil, fmt.Errorf("settings: %w", err)
	}
	if err := json.Unmarshal(b, &s.cur); err != nil {
		return nil, fmt.Errorf("settings %s: %w", path, err)
	}
	return s, nil
}

// Get returns the current settings.
func (s *Store) Get() Settings {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.cur
}

// Set persists the new settings and then updates the in-memory copy — the
// same persist-then-cache discipline as the engine's SetModelIntent, so a
// write that fails to land on disk never leaves memory ahead of the file.
func (s *Store) Set(v Settings) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.write(v)
}

// write persists v and updates the cache. Callers hold s.mu.
func (s *Store) write(v Settings) error {
	b, err := json.MarshalIndent(v, "", "  ")
	if err != nil {
		return fmt.Errorf("settings encode: %w", err)
	}
	if err := os.MkdirAll(filepath.Dir(s.path), 0o755); err != nil {
		return fmt.Errorf("settings dir: %w", err)
	}
	if err := os.WriteFile(s.path, append(b, '\n'), 0o644); err != nil {
		return fmt.Errorf("settings write: %w", err)
	}
	s.cur = v
	return nil
}

// SetDefaultModel updates just the default-model preference — a
// read-modify-write under the store lock, so the agent's set_model tool and a
// concurrent Settings-tab save cannot clobber each other's other fields.
func (s *Store) SetDefaultModel(model string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	v := s.cur
	v.DefaultModel = model
	return s.write(v)
}

// mirrorActive points the legacy LLMBaseURL/DefaultModel fields at the active
// provider entry, so an older binary that predates Providers still resolves the
// active endpoint/model (ADR-0040 downgrade safety). A no-op when no provider
// is configured. Callers hold s.mu.
func mirrorActive(v *Settings) {
	if act, ok := v.Active(); ok {
		v.LLMBaseURL = act.Endpoint
		v.DefaultModel = act.DefaultModel
	}
}

// AddProvider appends a new provider entry and persists. When it is the first
// entry it also becomes active. The active mirror is refreshed so a downgrade
// stays consistent.
func (s *Store) AddProvider(p ProviderEntry) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	v := s.cur
	v.Providers = append(append([]ProviderEntry(nil), v.Providers...), p)
	if v.ActiveProviderID == "" {
		v.ActiveProviderID = p.ID
	}
	mirrorActive(&v)
	return s.write(v)
}

// UpdateProvider replaces the entry with the matching ID in place, preserving
// order. Returns an error when no entry matches. KeyVaultName and DefaultModel
// are preserved when the incoming entry leaves them empty, so an edit that only
// touches one field (e.g. a rename) never orphans the existing vault binding or
// clears the stored default model.
func (s *Store) UpdateProvider(p ProviderEntry) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	v := s.cur
	next := append([]ProviderEntry(nil), v.Providers...)
	found := false
	for i := range next {
		if next[i].ID == p.ID {
			if p.KeyVaultName == "" {
				p.KeyVaultName = next[i].KeyVaultName
			}
			if p.DefaultModel == "" {
				p.DefaultModel = next[i].DefaultModel
			}
			next[i] = p
			found = true
			break
		}
	}
	if !found {
		return fmt.Errorf("settings: no provider with id %q", p.ID)
	}
	v.Providers = next
	mirrorActive(&v)
	return s.write(v)
}

// RemoveProvider drops the entry with the given ID and persists. When the
// removed entry was active, the selection falls to the first remaining entry
// (or empty when none remain). Returns an error when no entry matches.
func (s *Store) RemoveProvider(id string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	v := s.cur
	next := make([]ProviderEntry, 0, len(v.Providers))
	found := false
	for _, p := range v.Providers {
		if p.ID == id {
			found = true
			continue
		}
		next = append(next, p)
	}
	if !found {
		return fmt.Errorf("settings: no provider with id %q", id)
	}
	v.Providers = next
	if v.ActiveProviderID == id {
		v.ActiveProviderID = ""
		if len(next) > 0 {
			v.ActiveProviderID = next[0].ID
		}
	}
	mirrorActive(&v)
	return s.write(v)
}

// SetActiveProvider selects which configured provider is live and persists,
// refreshing the downgrade mirror. Returns an error when no entry matches
// (an empty list, or a stale id).
func (s *Store) SetActiveProvider(id string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	v := s.cur
	found := false
	for _, p := range v.Providers {
		if p.ID == id {
			found = true
			break
		}
	}
	if !found {
		return fmt.Errorf("settings: no provider with id %q", id)
	}
	v.ActiveProviderID = id
	mirrorActive(&v)
	return s.write(v)
}

// SubagentModel is the per-delegation read the pi agent wires as its
// Options.SubagentModel hook: each delegate call sees the latest saved value
// without rebuilding the host.
func (s *Store) SubagentModel() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.cur.SubagentModel
}
