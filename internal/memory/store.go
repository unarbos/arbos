// Package memory provides a durable key-value store for agent memory, scoped to
// a user config dir and a project workspace dir. User entries load first; project
// entries override on key collision.
package memory

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
)

const fileName = "memories.json"

// Store persists memory entries as JSON on disk.
type Store struct {
	userPath    string
	projectPath string

	mu sync.Mutex
}

// NewStore builds a store writing to userDir/memory/memories.json and
// projectDir/.arbos/memory/memories.json. Either path may be empty to skip that scope.
func NewStore(userDir, projectDir string) *Store {
	s := &Store{}
	if userDir != "" {
		s.userPath = filepath.Join(userDir, "memory", fileName)
	}
	if projectDir != "" {
		s.projectPath = filepath.Join(projectDir, ".arbos", "memory", fileName)
	}
	return s
}

type file struct {
	Entries map[string]string `json:"entries"`
}

func (s *Store) merged() (map[string]string, error) {
	out := map[string]string{}
	if s.userPath != "" {
		user, err := s.readFile(s.userPath)
		if err != nil {
			return nil, err
		}
		for k, v := range user {
			out[k] = v
		}
	}
	if s.projectPath != "" {
		proj, err := s.readFile(s.projectPath)
		if err != nil {
			return nil, err
		}
		for k, v := range proj {
			out[k] = v
		}
	}
	return out, nil
}

func (s *Store) readFile(path string) (map[string]string, error) {
	b, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			return map[string]string{}, nil
		}
		return nil, err
	}
	var f file
	if err := json.Unmarshal(b, &f); err != nil {
		return nil, fmt.Errorf("parse %s: %w", path, err)
	}
	if f.Entries == nil {
		return map[string]string{}, nil
	}
	return f.Entries, nil
}

func (s *Store) writeFile(path string, entries map[string]string) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	b, err := json.MarshalIndent(file{Entries: entries}, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(path, b, 0o644)
}

// Remember stores a key-value pair in the project scope.
func (s *Store) Remember(key, value string) error {
	key = strings.TrimSpace(key)
	if key == "" {
		return fmt.Errorf("memory: key is required")
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.projectPath == "" {
		return fmt.Errorf("memory: no project scope configured")
	}
	entries, err := s.readFile(s.projectPath)
	if err != nil {
		return err
	}
	entries[key] = value
	return s.writeFile(s.projectPath, entries)
}

// Forget removes a key from the project scope.
func (s *Store) Forget(key string) error {
	key = strings.TrimSpace(key)
	if key == "" {
		return fmt.Errorf("memory: key is required")
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.projectPath == "" {
		return fmt.Errorf("memory: no project scope configured")
	}
	entries, err := s.readFile(s.projectPath)
	if err != nil {
		return err
	}
	delete(entries, key)
	return s.writeFile(s.projectPath, entries)
}

// Recall returns the value for a key from the merged store.
func (s *Store) Recall(key string) (string, bool, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	merged, err := s.merged()
	if err != nil {
		return "", false, err
	}
	v, ok := merged[strings.TrimSpace(key)]
	return v, ok, nil
}

// List returns all keys in the merged store, sorted.
func (s *Store) List() ([]string, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	merged, err := s.merged()
	if err != nil {
		return nil, err
	}
	keys := make([]string, 0, len(merged))
	for k := range merged {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys, nil
}

// FormatMerged renders all entries as prompt text for context injection.
func (s *Store) FormatMerged() (string, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	merged, err := s.merged()
	if err != nil {
		return "", err
	}
	if len(merged) == 0 {
		return "", nil
	}
	keys := make([]string, 0, len(merged))
	for k := range merged {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	var b strings.Builder
	for _, k := range keys {
		fmt.Fprintf(&b, "- %s: %s\n", k, merged[k])
	}
	return strings.TrimRight(b.String(), "\n"), nil
}
