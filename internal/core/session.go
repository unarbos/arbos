package core

import "time"

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
// the event log, not here.
type Session struct {
	ID       SessionID
	ParentID SessionID // fork/branch lineage (the session this was derived from);
	// empty for a root session. NOT used by compression — compression is in-place.
	Status     SessionStatus
	Model      string
	TokenCount int // derived mirror of the log's usage events; not authoritative

	// Principal and Origin are RESERVED for the gateway/frontend phase, where
	// auth and reply-routing need them; both are empty for local single-user
	// sessions today. Reserved now (cheap) rather than threaded through later
	// (a migration). See ADR-0019.
	//
	// Principal: who owns/authorizes this session (a user or account id).
	// Origin: where it was created from — frontend/platform plus its native
	// addressing, e.g. "cli" or "telegram:chat/123" — so a reply reaches the
	// right surface.
	Principal string
	Origin    string

	CreatedAt time.Time
	UpdatedAt time.Time
}
