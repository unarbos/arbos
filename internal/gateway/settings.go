// The gateway's host-settings door: the Settings tab's read/write over the
// durable preference file (internal/settings) — agent-behavior knobs like the
// subagent model that must reach the Go host, unlike the browser's cosmetic
// localStorage settings. Same-origin guarded like the secrets door: a
// drive-by page must never repoint the user's agent at another model.
package gateway

import (
	"encoding/json"
	"net/http"

	"github.com/unarbos/arbos/internal/settings"
)

// SettingsStore is the gateway's view of the host preference file. Satisfied
// by *settings.Store; nil on the Server disables the routes.
type SettingsStore interface {
	Get() settings.Settings
	Set(settings.Settings) error
}

// settingsJSON is the preference set on the wire, field-for-field with
// settings.Settings (which already carries the wire tags).
type settingsJSON = settings.Settings

// handleSettingsGet returns the saved preferences for the Settings panel.
func (s *Server) handleSettingsGet(w http.ResponseWriter, _ *http.Request) {
	writeJSON(w, settingsJSON(s.HostSettings.Get()))
}

// handleSettingsPut replaces the preference set. The whole object travels
// (it is a handful of small fields), so the panel sends back what it loaded
// with its edits applied and there is no per-key merge protocol to version.
func (s *Server) handleSettingsPut(w http.ResponseWriter, r *http.Request) {
	var body settingsJSON
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, jsonBodyMax)).Decode(&body); err != nil {
		http.Error(w, "invalid body", http.StatusBadRequest)
		return
	}
	if err := s.HostSettings.Set(settings.Settings(body)); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}
