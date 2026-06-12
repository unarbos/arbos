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
	LLMBaseURL string `json:"llm_base_url,omitempty"`
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

// SubagentModel is the per-delegation read the pi agent wires as its
// Options.SubagentModel hook: each delegate call sees the latest saved value
// without rebuilding the host.
func (s *Store) SubagentModel() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.cur.SubagentModel
}
