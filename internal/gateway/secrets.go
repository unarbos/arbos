// The gateway's secrets door: the Settings tab's CRUD over the managed-secret
// vault (internal/secret). The list and write shapes enforce the security
// boundary in the wire protocol itself — metadata (name, label, hosts, the env
// flag) flows both ways, but a value is write-only: it travels browser ->
// gateway once on save and is never returned, so an open Settings tab (or a
// drive-by page) can never read back a stored secret. All routes are
// same-origin guarded like the file and job doors.
package gateway

import (
	"encoding/json"
	"net/http"

	"github.com/unarbos/arbos/internal/secret"
)

// SecretStore is the gateway's view of the managed-secret vault: list metadata,
// upsert an entry (empty value edits metadata only), and delete by name.
// Satisfied by *secret.Store; nil on the Server disables the routes.
type SecretStore interface {
	List() []secret.Entry
	Set(e secret.Entry, value string) error
	Delete(name string) error
}

// secretJSON is one entry's metadata on the wire — never a value. Header and
// Prefix describe the brokered auth scheme (empty = Authorization/Bearer).
type secretJSON struct {
	Name   string   `json:"name"`
	Label  string   `json:"label,omitempty"`
	Hosts  []string `json:"hosts,omitempty"`
	Env    bool     `json:"env,omitempty"`
	Header string   `json:"header,omitempty"`
	Prefix string   `json:"prefix,omitempty"`
}

// handleSecretsList returns every entry's metadata for the Settings panel.
func (s *Server) handleSecretsList(w http.ResponseWriter, _ *http.Request) {
	entries := s.Secrets.List()
	out := make([]secretJSON, 0, len(entries))
	for _, e := range entries {
		out = append(out, secretJSON{Name: e.Name, Label: e.Label, Hosts: e.Hosts, Env: e.Env, Header: e.Header, Prefix: e.Prefix})
	}
	writeJSON(w, map[string]any{"secrets": out})
}

// handleSecretUpsert creates or updates an entry. The name comes from the path;
// the body carries label, hosts, the env flag, the brokered auth scheme
// (header/prefix), and an optional value (omitted to edit metadata without
// re-entering the secret).
func (s *Server) handleSecretUpsert(w http.ResponseWriter, r *http.Request) {
	name := r.PathValue("name")
	var body struct {
		Label  string   `json:"label"`
		Value  string   `json:"value"`
		Hosts  []string `json:"hosts"`
		Env    bool     `json:"env"`
		Header string   `json:"header"`
		Prefix string   `json:"prefix"`
	}
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, jsonBodyMax)).Decode(&body); err != nil {
		http.Error(w, "invalid body", http.StatusBadRequest)
		return
	}
	entry := secret.Entry{Name: name, Label: body.Label, Hosts: body.Hosts, Env: body.Env, Header: body.Header, Prefix: body.Prefix}
	if err := s.Secrets.Set(entry, body.Value); err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

// handleSecretDelete removes an entry and its value.
func (s *Server) handleSecretDelete(w http.ResponseWriter, r *http.Request) {
	if err := s.Secrets.Delete(r.PathValue("name")); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}
