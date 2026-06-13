// Package board persists a workspace layout — the split-tree of panes and the
// tabs in them — as a first-class, addressable entity. A board exists so a
// layout can be saved and reopened, and (the reason it is first-class rather
// than client state) shared: a board share grant authorizes exactly the
// artifacts the board contains, so the board enumerates its shareable members
// independently of the opaque layout blob the UI round-trips. See ADR-0034.
package board

import (
	"encoding/json"
	"time"

	"github.com/unarbos/arbos/internal/share"
)

// Member is one shareable referent in a board: a file artifact or a chat. The
// UI computes the member list when it saves a board — it owns layout
// semantics — and the backend stores it so a board share can authorize member
// reads without parsing the frontend layout. Non-shareable tabs (terminals,
// runs, settings) are simply not members.
type Member struct {
	Kind share.ScopeKind `json:"kind"` // ScopeFile | ScopeSession
	Ref  string          `json:"ref"`  // workspace path | session id
}

// Board is a saved workspace layout.
type Board struct {
	ID    string
	Title string
	// Layout is the UI's serialized split-tree + tabs, kept opaque so the
	// backend never couples to frontend layout internals: it round-trips
	// verbatim for a faithful restore.
	Layout json.RawMessage
	// Members are the board's shareable referents — the transitive
	// authorization a board share grant checks against.
	Members   []Member
	CreatedAt time.Time
	UpdatedAt time.Time
}
