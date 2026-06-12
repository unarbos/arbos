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
}

// LLMAdmin is the host's provider-configuration seam, wired by the web door.
// Nil on the Server disables the routes.
type LLMAdmin struct {
	// Info returns the currently effective provider configuration.
	Info func() LLMInfo
	// SetEndpoint persists a new base URL ("" resets to the default).
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
}

// LLMCredits is the account's lifetime totals in USD;
// remaining = total_credits - total_usage.
type LLMCredits struct {
	TotalCredits float64 `json:"total_credits"`
	TotalUsage   float64 `json:"total_usage"`
}

// handleLLMGet returns the effective provider configuration.
func (s *Server) handleLLMGet(w http.ResponseWriter, _ *http.Request) {
	writeJSON(w, s.LLM.Info())
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
