// Package share is the capability model behind shareable links. One Grant type
// spans the whole range from "read this one canvas for an hour" to the node's
// own login (the maximal grant): a grant is a bearer capability — the opaque
// Token is the secret in the /s/<token> URL — scoped to one artifact, bounded
// by a permission and a lifetime. The login cookie is just the maximal
// instance of this same object (ScopeAll, PermAdmin). See ADR-0034.
package share

import "time"

// Perm is how much a grant authorizes *within its scope*. It is a monotonic
// ladder — Admin implies Write implies Read — and is always bounded by the
// scope: PermAdmin on a board grants control of that board, never the node's
// secrets, which live only under ScopeAll.
type Perm int

const (
	PermRead  Perm = iota // observe the artifact
	PermWrite             // act within scope: post to the chat, curate the board
	PermAdmin             // write + mint sub-links (delegation)
)

// ScopeKind discriminates what a grant points at; Ref carries the address in
// that kind's own namespace.
type ScopeKind string

const (
	ScopeFile    ScopeKind = "file"    // Ref = workspace file path (canvas HTML, image, pdf, doc, code)
	ScopeSession ScopeKind = "session" // Ref = session id
	ScopeBoard   ScopeKind = "board"   // Ref = board id
	ScopeAll     ScopeKind = "all"     // Ref = "" — the whole node (login's scope)
	// ScopeTrajectory shares a session's full debug trajectory (Ref = session
	// id): a static, self-contained snapshot of the event log — every message,
	// tool call and result, injected context, the turn config, and the token
	// totals — rendered as a standalone page for a bug report. Unlike
	// ScopeSession (the live chat behind the real UI), a trajectory link is a
	// read-only reproduction served straight from the log, so it survives a
	// node that has since crashed or moved on.
	ScopeTrajectory ScopeKind = "trajectory" // Ref = session id
)

// Scope is what a grant shares.
type Scope struct {
	Kind ScopeKind
	Ref  string
}

// Grant is the one credential primitive. Every share link is this object with
// a narrower scope, a weaker perm, or a shorter life than the login it
// descends from.
type Grant struct {
	// Token is the opaque bearer secret — the /s/<token> URL tail and the
	// row's primary key. Holding it is the capability.
	Token string
	Scope Scope
	Perm  Perm
	// Expires is when the grant dies; the zero time means never.
	Expires time.Time
	Created time.Time
}

// Live reports whether g is still redeemable at t — i.e. not past its expiry.
// Use-budget enforcement is a separate, stateful concern (the store), kept out
// of this pure check.
func (g Grant) Live(t time.Time) bool {
	return g.Expires.IsZero() || t.Before(g.Expires)
}
