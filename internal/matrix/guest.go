package matrix

import (
	"context"
	"strings"
	"sync"

	"github.com/unarbos/arbos/internal/core"
	"maunium.net/go/mautrix"
)

// GuestRegistry maps a session's guest — a shared-chat participant identified by
// their trusted display name — to the Matrix client the mirror publishes their
// messages as (ADR-0041). It is the seam between the gateway (which provisions a
// guest when a share link is redeemed) and the mirror (which, on every Room
// event, asks "who authored this?" and routes it to that author's own
// identity). Keyed by (session, author) so the same display name in two chats
// stays two distinct seats and never collides with the host's name in a third.
//
// A nil *GuestRegistry is safe (the no-guests path): lookups miss and the mirror
// falls back to its role-based routing.
type GuestRegistry struct {
	mu      sync.RWMutex
	clients map[guestKey]*mautrix.Client
}

type guestKey struct {
	session core.SessionID
	author  string
}

func NewGuestRegistry() *GuestRegistry {
	return &GuestRegistry{clients: map[guestKey]*mautrix.Client{}}
}

// Register binds a session's guest author to the client their messages publish
// as. A later redemption by the same name in the same chat overwrites it (a
// fresh login is just another device for that identity).
func (r *GuestRegistry) Register(session core.SessionID, author string, client *mautrix.Client) {
	if r == nil || client == nil || author == "" {
		return
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	r.clients[guestKey{session, author}] = client
}

// client resolves a session's guest author to their client, if one is
// registered.
func (r *GuestRegistry) client(session core.SessionID, author string) (*mautrix.Client, bool) {
	if r == nil || author == "" {
		return nil, false
	}
	r.mu.RLock()
	defer r.mu.RUnlock()
	c, ok := r.clients[guestKey{session, author}]
	return c, ok
}

// Unregister drops a session's guest binding — after the bridge kicks them, so
// their later (rejected) messages no longer route to a seat they no longer hold.
func (r *GuestRegistry) Unregister(session core.SessionID, author string) {
	if r == nil {
		return
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	delete(r.clients, guestKey{session, author})
}

// GuestProvisioner mints (idempotently) and logs in a guest account for a
// display name, returning the live client and its full user id. It is supplied
// by the host (which owns the embedded homeserver) so the matrix package stays
// free of the homeserver type; the Bridge calls it to seat a guest in a room.
type GuestProvisioner func(ctx context.Context, label string) (client *mautrix.Client, userID string, err error)

// GuestLocalpart derives a stable, valid Matrix localpart from a guest's
// display name ("Alice B." -> "guest-alice-b"). Stable so the same name reuses
// one identity across redemptions; prefixed so guests never collide with the
// node's reserved agent/human localparts. An empty/degenerate name falls back
// to a bare "guest".
func GuestLocalpart(displayName string) string {
	var b strings.Builder
	lastDash := false
	for _, r := range strings.ToLower(displayName) {
		switch {
		case r >= 'a' && r <= 'z', r >= '0' && r <= '9', r == '.', r == '_':
			b.WriteRune(r)
			lastDash = false
		default:
			if !lastDash {
				b.WriteByte('-')
				lastDash = true
			}
		}
	}
	body := strings.Trim(b.String(), "-")
	if body == "" {
		return "guest"
	}
	return "guest-" + body
}
