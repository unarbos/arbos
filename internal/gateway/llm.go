// The gateway's provider door: the Settings tab's view of which LLM endpoint
// the host talks to and whether a key is configured, the write path that
// repoints it, and a same-origin proxy for the account's credit totals. The
// key follows the secrets door's discipline — write-only on the wire, stored
// in the vault, attached to the credits request server-side and never
// returned. Saving either field schedules a graceful self-restart so the new
// provider is rebuilt through the host's one assembly path.
package gateway

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"net/http"
	"strings"
)

// LLMInfo is the provider picture the Settings panel shows: the effective
// endpoint and model after env/stored resolution, whether any key resolves,
// and whether the endpoint is OpenRouter (which gates the credits row).
type LLMInfo struct {
	Endpoint   string `json:"endpoint"`
	Provider   string `json:"provider"`
	Model      string `json:"model"`
	KeySet     bool   `json:"key_set"`
	OpenRouter bool   `json:"openrouter"`
	// BootID names this process instance; it changes exactly when the apply
	// restart lands, so the panel polls it to know the reported configuration
	// is the rebuilt one (a successful fetch alone may still be the old
	// process, or a restart parked behind a busy agent).
	BootID string `json:"boot_id"`
	// RestartPending is true while a saved change waits for an idle boundary.
	RestartPending bool `json:"restart_pending"`
	// Providers is the configured set (ADR-0040); empty for a single-provider
	// host that never used the multi-provider panel. ActiveProviderID names
	// the live one. ActiveLabel is its display name, surfaced beside the
	// composer's model selector.
	Providers        []ProviderView `json:"providers,omitempty"`
	ActiveProviderID string         `json:"active_provider_id,omitempty"`
	ActiveLabel      string         `json:"active_label,omitempty"`
}

// ProviderView is one configured provider as the panel sees it: identity and
// routing, plus whether its key resolves and whether it is owned by the
// environment (read-only — env config the UI must not delete or edit).
type ProviderView struct {
	ID           string `json:"id"`
	Label        string `json:"label"`
	ProviderName string `json:"provider_name"`
	Endpoint     string `json:"endpoint"`
	KeySet       bool   `json:"key_set"`
	EnvSourced   bool   `json:"env_sourced"`
}

// bootID is minted once per process. Configuration applies by re-exec, so a
// new value on the wire is proof the new configuration is being reported.
var bootID = func() string {
	var b [8]byte
	_, _ = rand.Read(b[:])
	return hex.EncodeToString(b[:])
}()

// LLMAdmin is the host's provider-configuration seam, wired by the web door.
// Nil on the Server disables the routes.
type LLMAdmin struct {
	// Info returns the currently effective provider configuration.
	Info func() LLMInfo
	// SetEndpoint persists a new base URL ("" resets to the default).
	// Single-provider compatibility path; the multi-provider panel uses the
	// AddProvider/UpdateProvider seam below.
	SetEndpoint func(url string) error
	// SetKey stores the API key in the secret vault. Called after SetEndpoint
	// on a combined save, so the key binds to the new endpoint's host.
	SetKey func(value string) error
	// Apply schedules the graceful restart that rebuilds the provider from
	// the persisted configuration.
	Apply func()
	// Credits fetches the account's credit totals from the provider, with the
	// key attached server-side. Nil when the endpoint doesn't serve them.
	Credits func(ctx context.Context) (LLMCredits, error)

	// Multi-provider seam (ADR-0040). Nil on a host that doesn't support it
	// (the routes 404). AddProvider creates an entry (id assigned by the
	// implementation) with an optional key; UpdateProvider edits one in place
	// (empty key leaves the existing one); RemoveProvider deletes one and its
	// key; SetActiveProvider selects the live one. Each mutation persists and
	// schedules the apply restart via the same path as Apply.
	AddProvider       func(p ProviderInput) error
	UpdateProvider    func(id string, p ProviderInput) error
	RemoveProvider    func(id string) error
	SetActiveProvider func(id string) error
}

// ProviderInput is the panel's write shape for a provider entry. Key is a
// pointer so an edit can leave the stored credential untouched (nil) versus
// replacing it (non-nil); it is write-only and never returned.
type ProviderInput struct {
	Label        string  `json:"label"`
	ProviderName string  `json:"provider_name"`
	Endpoint     string  `json:"endpoint"`
	DefaultModel string  `json:"default_model"`
	Key          *string `json:"key"`
}

// LLMCredits is the account's lifetime totals in USD;
// remaining = total_credits - total_usage.
type LLMCredits struct {
	TotalCredits float64 `json:"total_credits"`
	TotalUsage   float64 `json:"total_usage"`
}

// handleLLMGet returns the effective provider configuration.
func (s *Server) handleLLMGet(w http.ResponseWriter, _ *http.Request) {
	info := s.LLM.Info()
	info.BootID = bootID
	writeJSON(w, info)
}

// handleLLMPut updates the endpoint and/or key. Fields are pointers so the
// panel can send exactly what changed — including an explicit "" endpoint to
// reset to the default. Any accepted change schedules the apply restart.
func (s *Server) handleLLMPut(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Endpoint *string `json:"endpoint"`
		Key      *string `json:"key"`
	}
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, jsonBodyMax)).Decode(&body); err != nil {
		http.Error(w, "invalid body", http.StatusBadRequest)
		return
	}
	changed := false
	if body.Endpoint != nil {
		ep := strings.TrimSpace(*body.Endpoint)
		if ep != "" && !strings.HasPrefix(ep, "https://") && !strings.HasPrefix(ep, "http://") {
			http.Error(w, "endpoint must be an http(s) URL", http.StatusBadRequest)
			return
		}
		if err := s.LLM.SetEndpoint(ep); err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
		changed = true
	}
	if body.Key != nil && strings.TrimSpace(*body.Key) != "" {
		if err := s.LLM.SetKey(strings.TrimSpace(*body.Key)); err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		changed = true
	}
	if !changed {
		http.Error(w, "nothing to update", http.StatusBadRequest)
		return
	}
	s.LLM.Apply()
	w.WriteHeader(http.StatusNoContent)
}

// handleProvidersPost adds a new configured provider (ADR-0040), then schedules
// the apply restart. 404 when the host has no multi-provider seam.
func (s *Server) handleProvidersPost(w http.ResponseWriter, r *http.Request) {
	if s.LLM.AddProvider == nil {
		http.Error(w, "multiple providers not supported on this host", http.StatusNotFound)
		return
	}
	var in ProviderInput
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, jsonBodyMax)).Decode(&in); err != nil {
		http.Error(w, "invalid body", http.StatusBadRequest)
		return
	}
	if _, ok := providerNameOK(in.ProviderName); !ok {
		http.Error(w, "provider_name must be openai, anthropic, or google", http.StatusBadRequest)
		return
	}
	if !endpointOK(in.Endpoint) {
		http.Error(w, "endpoint must be an http(s) URL", http.StatusBadRequest)
		return
	}
	if err := s.LLM.AddProvider(in); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	s.LLM.Apply()
	w.WriteHeader(http.StatusNoContent)
}

// handleProviderPut edits a configured provider in place. An empty/omitted key
// leaves the stored credential untouched.
func (s *Server) handleProviderPut(w http.ResponseWriter, r *http.Request) {
	if s.LLM.UpdateProvider == nil {
		http.Error(w, "multiple providers not supported on this host", http.StatusNotFound)
		return
	}
	id := r.PathValue("id")
	var in ProviderInput
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, jsonBodyMax)).Decode(&in); err != nil {
		http.Error(w, "invalid body", http.StatusBadRequest)
		return
	}
	if _, ok := providerNameOK(in.ProviderName); !ok {
		http.Error(w, "provider_name must be openai, anthropic, or google", http.StatusBadRequest)
		return
	}
	if !endpointOK(in.Endpoint) {
		http.Error(w, "endpoint must be an http(s) URL", http.StatusBadRequest)
		return
	}
	if err := s.LLM.UpdateProvider(id, in); err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	s.LLM.Apply()
	w.WriteHeader(http.StatusNoContent)
}

// handleProviderDelete removes a configured provider and its key.
func (s *Server) handleProviderDelete(w http.ResponseWriter, r *http.Request) {
	if s.LLM.RemoveProvider == nil {
		http.Error(w, "multiple providers not supported on this host", http.StatusNotFound)
		return
	}
	if err := s.LLM.RemoveProvider(r.PathValue("id")); err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	s.LLM.Apply()
	w.WriteHeader(http.StatusNoContent)
}

// handleProviderActivate selects the live provider and schedules the restart.
func (s *Server) handleProviderActivate(w http.ResponseWriter, r *http.Request) {
	if s.LLM.SetActiveProvider == nil {
		http.Error(w, "multiple providers not supported on this host", http.StatusNotFound)
		return
	}
	if err := s.LLM.SetActiveProvider(r.PathValue("id")); err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	s.LLM.Apply()
	w.WriteHeader(http.StatusNoContent)
}

// providerNameOK validates the adapter name against the supported set.
func providerNameOK(name string) (string, bool) {
	switch name {
	case "openai", "anthropic", "google":
		return name, true
	default:
		return "", false
	}
}

// endpointOK accepts an empty endpoint (use the adapter default) or an http(s)
// URL, the same rule as the single-provider endpoint field.
func endpointOK(ep string) bool {
	ep = strings.TrimSpace(ep)
	return ep == "" || strings.HasPrefix(ep, "https://") || strings.HasPrefix(ep, "http://")
}

// handleLLMCredits proxies the provider's credit totals. 404 when the
// configured endpoint has no such surface (non-OpenRouter bases).
func (s *Server) handleLLMCredits(w http.ResponseWriter, r *http.Request) {
	if s.LLM.Credits == nil {
		http.Error(w, "credits not available for this endpoint", http.StatusNotFound)
		return
	}
	info, err := s.LLM.Credits(r.Context())
	if err != nil {
		http.Error(w, err.Error(), http.StatusBadGateway)
		return
	}
	writeJSON(w, info)
}
