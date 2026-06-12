// The messenger door: REST for the bot registry (add/remove a Telegram bot)
// and one WebSocket carrying the bridge's live event stream to the panel —
// the same dedicated-socket pattern as the screencast, because conversation
// traffic is the messenger's own concern, not the control seam's.

package gateway

import (
	"encoding/json"
	"net/http"
	"strconv"

	"github.com/coder/websocket"
)

// handleMessengerState returns the registry snapshot (bots without tokens,
// conversations) for the panel's initial render.
func (s *Server) handleMessengerState(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, s.Messenger.State())
}

// handleMessengerAddBot validates a bot token against Telegram and starts
// polling it. The token enters here and never comes back out. Tools default
// on — a connector is a normal chat unless explicitly restricted.
func (s *Server) handleMessengerAddBot(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Token string `json:"token"`
		Tools *bool  `json:"tools"`
	}
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, jsonBodyMax)).Decode(&req); err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	if req.Token == "" {
		http.Error(w, "token is required", http.StatusBadRequest)
		return
	}
	tools := req.Tools == nil || *req.Tools
	view, err := s.Messenger.AddBot(r.Context(), req.Token, tools)
	if err != nil {
		http.Error(w, err.Error(), http.StatusBadGateway)
		return
	}
	writeJSON(w, view)
}

// handleMessengerSetTools flips a connector's permission switch; it applies
// from the next turn.
func (s *Server) handleMessengerSetTools(w http.ResponseWriter, r *http.Request) {
	id, err := strconv.ParseInt(r.PathValue("id"), 10, 64)
	if err != nil {
		http.Error(w, "bad bot id", http.StatusBadRequest)
		return
	}
	var req struct {
		Tools bool `json:"tools"`
	}
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, jsonBodyMax)).Decode(&req); err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	view, err := s.Messenger.SetTools(id, req.Tools)
	if err != nil {
		http.Error(w, err.Error(), http.StatusNotFound)
		return
	}
	writeJSON(w, view)
}

// handleMessengerRemoveBot stops a bot and forgets it.
func (s *Server) handleMessengerRemoveBot(w http.ResponseWriter, r *http.Request) {
	id, err := strconv.ParseInt(r.PathValue("id"), 10, 64)
	if err != nil {
		http.Error(w, "bad bot id", http.StatusBadRequest)
		return
	}
	if err := s.Messenger.RemoveBot(id); err != nil {
		http.Error(w, err.Error(), http.StatusNotFound)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

// handleMessengerWS streams bridge events (registry snapshots, conversation
// upserts, live messages) to the panel. Outbound only; the panel acts
// through the REST routes.
func (s *Server) handleMessengerWS(w http.ResponseWriter, r *http.Request) {
	c, err := websocket.Accept(w, r, s.wsAccept(r))
	if err != nil {
		return
	}
	defer c.Close(websocket.StatusInternalError, "messenger teardown")
	ctx := r.Context()
	go keepAlive(ctx, c)
	// Drain inbound frames so pongs are processed and a client close is seen.
	go func() {
		for {
			if _, _, err := c.Read(ctx); err != nil {
				return
			}
		}
	}()

	events, unsub := s.Messenger.Subscribe()
	defer unsub()
	for {
		select {
		case <-ctx.Done():
			return
		case ev := <-events:
			b, err := json.Marshal(ev)
			if err != nil {
				continue
			}
			if err := c.Write(ctx, websocket.MessageText, b); err != nil {
				return
			}
		}
	}
}
