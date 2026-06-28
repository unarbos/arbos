package core

import (
	"fmt"
	"strconv"
	"strings"
	"time"
)

// SessionStatus tracks a session's lifecycle.
//
// Important: compression is NOT a lifecycle transition. Context is compressed
// *in place* by appending a CompressionPayload event to the live session's log
// (see event.go and Project), so a session stays Active across any number of
// compressions — no session rotation, no row copying, no orphan sessions, no
// compression lock. Status changes only for genuine ends. See ADR-0014.
type SessionStatus string

const (
	SessionActive SessionStatus = "active"
	SessionEnded  SessionStatus = "ended"
)

// Session is metadata about a conversation. The conversation content lives in
// the event log, not here; token accounting in particular is derived from the
// log's usage events, never mirrored as a counter.
type Session struct {
	ID       SessionID
	ParentID SessionID // fork/branch lineage (the session this was derived from);
	// empty for a root session. NOT used by compression — compression is in-place.
	Status SessionStatus
	// Model is the durable per-session model authority: stamped at creation,
	// updated by SetModelIntent, and re-read on session adoption so a resumed
	// session keeps the model it was switched to.
	Model string
	// WebSearch is the durable per-session web-search toggle: updated by
	// SetWebSearchIntent and re-read on adoption, so a resumed session keeps
	// whether the model may search the web. Mirrors Model's lifecycle. See
	// ADR-0027.
	WebSearch bool
	// WebFetch is the durable per-session web-fetch toggle — whether the model
	// may retrieve page content server-side. Independent of WebSearch; same
	// lifecycle. See ADR-0027.
	WebFetch bool
	// ImageGen is the durable per-session image-generation toggle — whether
	// the model may generate images provider-side. Independent of the web
	// toggles; same lifecycle.
	ImageGen bool

	// Principal: who owns/authorizes this session — the ADDRESS of the
	// agent's voice. Single-user hosts use PrincipalLocal; a multi-user
	// gateway sets real account ids. See ADR-0019.
	// Origin: where it was created from — frontend/platform plus its native
	// addressing, e.g. "cli" or "telegram:chat/123" — so a reply reaches the
	// right surface.
	Principal string
	Origin    string

	// Owner is the conversation this session ultimately serves: empty for a
	// chat itself; the chat's id for a scheduler wake or delegated child
	// spawned on its behalf (inherited transitively, so nested spawns still
	// route to the right conversation). It scopes the voice and the UI —
	// a chat's sub-agents belong to that chat — while the work itself (plan,
	// memory, jobs) stays agent-global.
	Owner SessionID
	// SpawnedBy records what created a machine-spawned session, e.g.
	// SpawnedByNode(12) for a plan-node firing. Display/listing metadata;
	// the kernel never branches on it.
	SpawnedBy string

	CreatedAt time.Time
	UpdatedAt time.Time
}

// OriginScheduler marks sessions the plan scheduler spawned (machine-initiated
// work, not a human at a door). The front-door brief uses it to anchor "since
// you left" on the last session a human actually drove; future doors set their
// own origins ("telegram:chat/123") per the reservation above.
const OriginScheduler = "scheduler"

// OriginRoom marks a user message that arrived through the session's shared
// room — a participant typing from an external client federated into the room,
// rather than a local door. Like every door origin it drives cross-door echo
// suppression: the message is already present in the room, so the Matrix mirror
// recognizes this origin and does not re-publish it (which would duplicate it),
// while the attached frontends still render it as the cross-door echo.
const OriginRoom = "room"

// PrincipalLocal is the principal of every session on a single-user local
// host — the address the agent's voice targets until real accounts exist.
const PrincipalLocal = "local"

// SpawnedByNode encodes a plan-node firing as a Session.SpawnedBy value.
func SpawnedByNode(node int64) string {
	return fmt.Sprintf("node:%d", node)
}

// ParseSpawnedByNode inverts SpawnedByNode; ok is false for other spawners.
func ParseSpawnedByNode(s string) (node int64, ok bool) {
	rest, found := strings.CutPrefix(s, "node:")
	if !found {
		return 0, false
	}
	node, err := strconv.ParseInt(rest, 10, 64)
	return node, err == nil
}
