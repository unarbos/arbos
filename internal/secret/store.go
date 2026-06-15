// Store is arbos's managed-secret backend: a local, encrypted-at-rest vault the
// user fills through the Settings tab. It is the concrete SecretProvider that
// closes the gap ADR-0016 left open ("where users declare ref -> hosts ->
// injector"), replacing the dev-only env provider's plaintext .env — which the
// agent's whole process tree inherits — with named, labeled entries whose
// values are encrypted on disk and surfaced only at a bound boundary.
//
// The split is the security boundary: metadata (name, label, hosts, the env
// flag) is plaintext and freely listed to the UI; values live in a separate
// vault file, encrypted with a key the OS keeps 0600 in the agent config dir.
// The agent and its LLM context still only ever see SecretRefs (names) — values
// surface only at HTTP injection (the Broker) or at a tool subprocess spawn, and
// never enter the parent process environment.
package secret

import (
	"context"
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"sync"

	"github.com/unarbos/arbos/internal/core"
)

// nameRe bounds an entry name to a valid environment-variable identifier, so a
// name is safe to inject as KEY=value and safe to use as a file-free map key.
var nameRe = regexp.MustCompile(`^[A-Za-z_][A-Za-z0-9_]*$`)

// Entry is one managed secret's public metadata — everything the UI may see.
// It deliberately carries no value: the value lives encrypted in the vault and
// only the Broker (HTTP) or a subprocess spawn ever resolves it.
type Entry struct {
	// Name is the environment-variable identifier and the SecretRef name the
	// agent uses to refer to this secret.
	Name string `json:"name"`
	// Label is the human description shown in the UI ("GitHub PAT for CI").
	Label string `json:"label,omitempty"`
	// Hosts is the destination allowlist for HTTP brokering: the value may be
	// attached only to outbound requests to these hosts (anti-exfiltration).
	Hosts []string `json:"hosts,omitempty"`
	// Env, when set, opts this secret into injection at tool subprocess spawn
	// (bash jobs and the like) for CLI tools that read the value from the
	// environment. Off by default: a brokered HTTP secret never needs to sit in
	// a child's environment, and not putting it there is strictly safer.
	Env bool `json:"env,omitempty"`
	// Header is the HTTP header the brokered value is injected into. Empty (the
	// default) means "Authorization: Bearer <value>" — the common case. Set it
	// (e.g. "X-API-Key", or "Authorization" with an empty Prefix) for APIs whose
	// scheme is not Bearer.
	Header string `json:"header,omitempty"`
	// Prefix is placed before the value in Header (e.g. "Bearer ", "Token ").
	// Empty injects the raw value — for x-api-key-style headers or keys that
	// already carry their own scheme (e.g. a "prefix.secret" credential).
	// Ignored when Header is empty.
	Prefix string `json:"prefix,omitempty"`
}

// injector builds the Injector this entry's scheme describes. An entry with no
// Header keeps the default Authorization/Bearer scheme, so existing secrets and
// the common case need no configuration; otherwise the value is injected into
// Header behind Prefix.
func (e Entry) injector() Injector {
	if e.Header == "" {
		return BearerInjector
	}
	return SchemeInjector(e.Header, e.Prefix)
}

// Store is the encrypted managed-secret vault. Metadata and ciphertext persist
// as two files under dir; the AES key is a third. It is safe for concurrent use
// by the gateway's CRUD handlers and the session actors that resolve refs.
type Store struct {
	dir  string
	gcm  cipher.AEAD
	mu   sync.RWMutex
	meta map[string]Entry  // name -> public metadata
	enc  map[string][]byte // name -> nonce||ciphertext
}

func (s *Store) indexPath() string { return filepath.Join(s.dir, "index.json") }
func (s *Store) vaultPath() string { return filepath.Join(s.dir, "vault.json") }
func (s *Store) keyPath() string   { return filepath.Join(s.dir, "key") }

// Open loads (or initializes) the vault under dir, creating the directory and a
// fresh 0600 AES-256 key on first use. A missing index or vault is an empty
// store, not an error — a fresh install has no secrets yet.
func Open(dir string) (*Store, error) {
	if dir == "" {
		return nil, fmt.Errorf("secret store: empty dir")
	}
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return nil, fmt.Errorf("secret store dir: %w", err)
	}
	s := &Store{dir: dir, meta: map[string]Entry{}, enc: map[string][]byte{}}
	key, err := s.loadOrCreateKey()
	if err != nil {
		return nil, err
	}
	block, err := aes.NewCipher(key)
	if err != nil {
		return nil, fmt.Errorf("secret cipher: %w", err)
	}
	if s.gcm, err = cipher.NewGCM(block); err != nil {
		return nil, fmt.Errorf("secret gcm: %w", err)
	}
	if err := s.load(); err != nil {
		return nil, err
	}
	return s, nil
}

// loadOrCreateKey reads the 32-byte vault key, minting one on first use. The key
// file is 0600: the box-attacker surface is the same as the SQLite store beside
// it, and a world-readable plaintext .env was strictly worse.
func (s *Store) loadOrCreateKey() ([]byte, error) {
	if b, err := os.ReadFile(s.keyPath()); err == nil {
		if len(b) != 32 {
			return nil, fmt.Errorf("secret key: want 32 bytes, got %d", len(b))
		}
		return b, nil
	}
	key := make([]byte, 32)
	if _, err := rand.Read(key); err != nil {
		return nil, fmt.Errorf("secret key gen: %w", err)
	}
	if err := os.WriteFile(s.keyPath(), key, 0o600); err != nil {
		return nil, fmt.Errorf("secret key write: %w", err)
	}
	return key, nil
}

// load reads the index and vault files into memory. Both absent is the
// empty-store case; a present-but-corrupt file is a hard error so a bug never
// silently drops the user's secrets.
func (s *Store) load() error {
	if b, err := os.ReadFile(s.indexPath()); err == nil {
		var entries []Entry
		if err := json.Unmarshal(b, &entries); err != nil {
			return fmt.Errorf("secret index: %w", err)
		}
		for _, e := range entries {
			s.meta[e.Name] = e
		}
	} else if !os.IsNotExist(err) {
		return fmt.Errorf("secret index: %w", err)
	}
	if b, err := os.ReadFile(s.vaultPath()); err == nil {
		raw := map[string][]byte{}
		if err := json.Unmarshal(b, &raw); err != nil {
			return fmt.Errorf("secret vault: %w", err)
		}
		s.enc = raw
	} else if !os.IsNotExist(err) {
		return fmt.Errorf("secret vault: %w", err)
	}
	return nil
}

// persist writes both files atomically-ish (temp + rename) at 0600. Holds no
// lock itself; callers hold the write lock.
func (s *Store) persist() error {
	entries := make([]Entry, 0, len(s.meta))
	for _, e := range s.meta {
		entries = append(entries, e)
	}
	sort.Slice(entries, func(i, j int) bool { return entries[i].Name < entries[j].Name })
	if err := writeFileJSON(s.indexPath(), entries); err != nil {
		return fmt.Errorf("secret index write: %w", err)
	}
	if err := writeFileJSON(s.vaultPath(), s.enc); err != nil {
		return fmt.Errorf("secret vault write: %w", err)
	}
	return nil
}

func writeFileJSON(path string, v any) error {
	b, err := json.Marshal(v)
	if err != nil {
		return err
	}
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, b, 0o600); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}

// List returns the public metadata for every entry, sorted by name. Values are
// never included — this is the shape the gateway hands the UI.
func (s *Store) List() []Entry {
	s.mu.RLock()
	defer s.mu.RUnlock()
	out := make([]Entry, 0, len(s.meta))
	for _, e := range s.meta {
		out = append(out, e)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Name < out[j].Name })
	return out
}

// Set upserts an entry. A non-empty value re-encrypts and replaces the stored
// material; an empty value on an existing entry edits only metadata (label,
// hosts, env) and keeps the current value — so the UI can offer "edit" without
// forcing the user to re-paste the secret. A new entry requires a value.
func (s *Store) Set(e Entry, value string) error {
	e.Name = strings.TrimSpace(e.Name)
	if !nameRe.MatchString(e.Name) {
		return fmt.Errorf("invalid secret name %q (use A-Z, 0-9, _)", e.Name)
	}
	e.Hosts = normalizeHosts(e.Hosts)
	s.mu.Lock()
	defer s.mu.Unlock()
	_, exists := s.meta[e.Name]
	if value == "" && !exists {
		return fmt.Errorf("secret %q: a value is required to create it", e.Name)
	}
	if value != "" {
		ct, err := s.seal([]byte(value))
		if err != nil {
			return err
		}
		s.enc[e.Name] = ct
	}
	s.meta[e.Name] = e
	return s.persist()
}

// Delete removes an entry and its value. Deleting an absent entry is a no-op
// success — the caller's intent (it's gone) is already satisfied.
func (s *Store) Delete(name string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, ok := s.meta[name]; !ok {
		return nil
	}
	delete(s.meta, name)
	delete(s.enc, name)
	return s.persist()
}

// Name identifies the backend in logs (ports.SecretProvider).
func (*Store) Name() string { return "arbos-vault" }

// Resolve decrypts the value for ref.Name (ports.SecretProvider). The returned
// SecretValue is the caller's to Destroy after use at the boundary; the Store
// keeps only ciphertext in memory.
func (s *Store) Resolve(_ context.Context, ref core.SecretRef) (core.SecretValue, error) {
	s.mu.RLock()
	ct, ok := s.enc[ref.Name]
	s.mu.RUnlock()
	if !ok {
		return core.SecretValue{}, fmt.Errorf("secret %q not in vault", ref.Name)
	}
	pt, err := s.open(ct)
	if err != nil {
		return core.SecretValue{}, fmt.Errorf("secret %q decrypt: %w", ref.Name, err)
	}
	return core.NewSecretValue(pt), nil
}

// EnvValues resolves every entry flagged for environment injection into
// "NAME=value" strings for a tool subprocess's cmd.Env. This is the one place
// values legitimately leave the boundary as plaintext, scoped to the child the
// user explicitly opted in — not the agent's whole process tree as a broad
// .env would be. An undecryptable entry is skipped rather than failing the
// whole spawn.
func (s *Store) EnvValues() []string {
	s.mu.RLock()
	defer s.mu.RUnlock()
	var out []string
	for name, e := range s.meta {
		if !e.Env {
			continue
		}
		pt, err := s.open(s.enc[name])
		if err != nil {
			continue
		}
		out = append(out, name+"="+string(pt))
	}
	sort.Strings(out)
	return out
}

// Bindings builds a broker binding per entry that named at least one host, so
// the HTTP boundary can attach these secrets to their allowed destinations and
// refuse every other host. Each binding carries the entry's own injection
// scheme (see Entry.injector), so different secrets can speak different auth
// dialects through the same broker.
func (s *Store) Bindings() []Binding {
	s.mu.RLock()
	defer s.mu.RUnlock()
	var out []Binding
	for name, e := range s.meta {
		if len(e.Hosts) == 0 {
			continue
		}
		out = append(out, Binding{
			Ref:    core.SecretRef{Name: name},
			Hosts:  append([]string(nil), e.Hosts...),
			Inject: e.injector(),
		})
	}
	return out
}

// seal encrypts plaintext as nonce||ciphertext with a fresh random nonce.
func (s *Store) seal(pt []byte) ([]byte, error) {
	nonce := make([]byte, s.gcm.NonceSize())
	if _, err := io.ReadFull(rand.Reader, nonce); err != nil {
		return nil, fmt.Errorf("secret nonce: %w", err)
	}
	return s.gcm.Seal(nonce, nonce, pt, nil), nil
}

// open reverses seal.
func (s *Store) open(blob []byte) ([]byte, error) {
	ns := s.gcm.NonceSize()
	if len(blob) < ns {
		return nil, fmt.Errorf("ciphertext too short")
	}
	return s.gcm.Open(nil, blob[:ns], blob[ns:], nil)
}

// normalizeHosts trims, lowercases, and de-dupes the host allowlist so the
// broker's case-insensitive match has a clean set and the UI shows no blanks.
func normalizeHosts(in []string) []string {
	seen := map[string]bool{}
	var out []string
	for _, h := range in {
		h = strings.ToLower(strings.TrimSpace(h))
		if h == "" || seen[h] {
			continue
		}
		seen[h] = true
		out = append(out, h)
	}
	sort.Strings(out)
	return out
}
