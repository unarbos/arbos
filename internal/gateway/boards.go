// Board persistence (ADR-0034): save the current workspace layout — the
// split-tree of panes and their tabs — as a reopenable, addressable entity,
// and the foundation a board share grant points at. The layout blob is opaque
// here (the UI owns its shape); the members list is structured so a later
// board share can authorize exactly the artifacts the board holds.

package gateway

import (
	"encoding/json"
	"errors"
	"net/http"
	"time"

	"github.com/unarbos/arbos/internal/board"
	"github.com/unarbos/arbos/internal/sqlite"
)

// boardListLimit caps the board picker; nobody scrolls past this.
const boardListLimit = 200

// handleBoardSave creates or updates a board. An empty id mints a new one;
// a known id updates in place (created_at preserved by the store). The layout
// is required and stored verbatim; members are the shareable referents the UI
// extracted from the layout.
func (s *Server) handleBoardSave(w http.ResponseWriter, r *http.Request) {
	var req struct {
		ID      string          `json:"id"`
		Title   string          `json:"title"`
		Layout  json.RawMessage `json:"layout"`
		Members []board.Member  `json:"members"`
	}
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, jsonBodyMax)).Decode(&req); err != nil || len(req.Layout) == 0 {
		http.Error(w, "layout is required", http.StatusBadRequest)
		return
	}
	now := time.Now()
	b := board.Board{
		ID:        req.ID,
		Title:     req.Title,
		Layout:    req.Layout,
		Members:   req.Members,
		CreatedAt: now, // ignored on update; the store keeps the original
		UpdatedAt: now,
	}
	if b.ID == "" {
		b.ID = "brd_" + randomToken()[:16]
		if b.ID == "brd_" {
			http.Error(w, "could not mint a board id", http.StatusInternalServerError)
			return
		}
	}
	if err := s.Store.PutBoard(r.Context(), b); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	writeJSON(w, map[string]any{"id": b.ID})
}

type boardSummaryJSON struct {
	ID        string `json:"id"`
	Title     string `json:"title"`
	UpdatedAt int64  `json:"updated_at"` // unix milliseconds
}

// handleBoardsList returns board summaries for the picker.
func (s *Server) handleBoardsList(w http.ResponseWriter, r *http.Request) {
	sums, err := s.Store.ListBoards(r.Context(), boardListLimit)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	out := make([]boardSummaryJSON, 0, len(sums))
	for _, sum := range sums {
		out = append(out, boardSummaryJSON{
			ID:        sum.ID,
			Title:     sum.Title,
			UpdatedAt: sum.UpdatedAt.UnixMilli(),
		})
	}
	writeJSON(w, map[string]any{"boards": out})
}

// handleBoardGet loads a board for restore: its layout blob (verbatim) and
// members.
func (s *Server) handleBoardGet(w http.ResponseWriter, r *http.Request) {
	b, err := s.Store.GetBoard(r.Context(), r.PathValue("id"))
	if errors.Is(err, sqlite.ErrNoBoard) {
		http.Error(w, "no such board", http.StatusNotFound)
		return
	}
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	members := b.Members
	if members == nil {
		members = []board.Member{}
	}
	writeJSON(w, map[string]any{
		"id":         b.ID,
		"title":      b.Title,
		"layout":     b.Layout,
		"members":    members,
		"updated_at": b.UpdatedAt.UnixMilli(),
	})
}

// handleBoardDelete removes a board.
func (s *Server) handleBoardDelete(w http.ResponseWriter, r *http.Request) {
	if err := s.Store.DeleteBoard(r.Context(), r.PathValue("id")); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}
