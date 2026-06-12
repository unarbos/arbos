// The browser screencast door: a WebSocket that streams a Browser's live CDP
// frames to a "screencast" surface in the UI. It is deliberately separate from
// the control seam (/api/ws) — high-frequency image frames must never ride the
// event-log stream — and carries no commands, only updates out, each one a
// JSON {frame?, url?}: a base64 JPEG the panel paints, a navigation the panel's
// address readout tracks, or both.

package gateway

import (
	"encoding/json"
	"net/http"

	"github.com/coder/websocket"

	"github.com/unarbos/arbos/internal/browser"
)

// workspaceBrowser is the workspace's shared Browser — the same instance every
// toolset drives (Shared dedupes on the profile), so the UI's tab management
// and the agent's are one tab table.
func (s *Server) workspaceBrowser() *browser.Browser {
	return browser.Shared(browser.ConfigFor(s.Root))
}

// handleBrowserTabs lists the open browser tabs (GET) or opens a fresh one
// (POST) — the UI's Browser button and a panel's "+". Listing never launches
// Chrome (no tabs = not running); creating does.
func (s *Server) handleBrowserTabs(w http.ResponseWriter, r *http.Request) {
	b := s.workspaceBrowser()
	if r.Method == http.MethodPost {
		t, err := b.NewTab(r.Context())
		if err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
		writeJSON(w, t)
		return
	}
	writeJSON(w, map[string]any{"tabs": b.Tabs()})
}

// handleBrowserTabClose closes one browser tab — a panel's close-tab control.
func (s *Server) handleBrowserTabClose(w http.ResponseWriter, r *http.Request) {
	if err := s.workspaceBrowser().CloseTab(r.Context(), r.PathValue("id")); err != nil {
		http.Error(w, err.Error(), http.StatusNotFound)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

// handleScreencast streams live browser frames for one stream id to the viewer.
// It subscribes to the process-wide frame hub the browser tool publishes to;
// the last frame is delivered immediately so a freshly opened panel is never
// blank.
func (s *Server) handleScreencast(w http.ResponseWriter, r *http.Request) {
	id := r.URL.Query().Get("id")
	if id == "" {
		http.Error(w, "id is required", http.StatusBadRequest)
		return
	}
	c, err := websocket.Accept(w, r, s.wsAccept(r))
	if err != nil {
		return
	}
	defer c.Close(websocket.StatusInternalError, "screencast teardown")
	ctx := r.Context()
	// A static page emits no frames; pings keep the idle socket from being
	// reaped by intermediaries (the input-read goroutine answers the pongs).
	go keepAlive(ctx, c)

	frames, unsub := browser.Frames.Subscribe(id)
	defer unsub()

	// Inbound messages are the viewer's interactions — clicks, keys, text —
	// forwarded into the live browser through the hub. This is what lets the
	// user drive the page themselves (sign into a site, dismiss a dialog) in
	// the same browser the agent drives.
	go func() {
		for {
			_, data, err := c.Read(ctx)
			if err != nil {
				return
			}
			var ev browser.InputEvent
			if json.Unmarshal(data, &ev) == nil && ev.T != "" {
				browser.Frames.Input(id, ev)
			}
		}
	}()

	for {
		select {
		case <-ctx.Done():
			return
		case u := <-frames:
			b, err := json.Marshal(u)
			if err != nil {
				continue
			}
			if err := c.Write(ctx, websocket.MessageText, b); err != nil {
				return
			}
		}
	}
}
