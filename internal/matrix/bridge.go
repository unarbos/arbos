package matrix

import (
	"context"
	"fmt"
	"strings"

	"github.com/unarbos/arbos/internal/core"
	"maunium.net/go/mautrix"
	"maunium.net/go/mautrix/id"
)

// Bridge is the gateway's handle onto the live Matrix substrate (ADR-0041). It
// resolves a session to its room and reports the node's coordinates and its two
// resident seats — the autonomous agent and the person operating it (D9) — so a
// Matrix-aware door (the web gateway) can show a chat is a real, addressable
// room without importing a Matrix client type. The browser stays a thin seam
// translator; the Matrix client of record is server-side, in-process. It also
// carries the agent client so the gateway can Watch its rooms for messages that
// arrive from outside the engine (an external Matrix client or a federated
// peer) — see watch.go.
//
// A nil *Bridge is the Matrix-off path: every accessor is safe and reports
// "not enabled", so the gateway wires the field unconditionally and each
// Matrix-backed route degrades cleanly when ARBOS_MATRIX is unset.
type Bridge struct {
	agent         *mautrix.Client
	human         *mautrix.Client // the operating person's client (@human); their browser drives Matrix as this identity
	rooms         RoomResolver
	homeserverURL string
	agentID       string
	humanID       string
	guests        *GuestRegistry   // shared between bridge (writes) and mirror (reads)
	provision     GuestProvisioner // mints guest accounts; nil disables InviteGuest
}

// RoomResolver maps between a session and its mirrored room (both directions).
// *Store satisfies it, so the Bridge resolves rooms through the same map the
// composite store fills as it provisions a room per session.
type RoomResolver interface {
	RoomID(core.SessionID) (roomID string, ok bool)
	SessionForRoom(roomID string) (core.SessionID, bool)
}

// NewBridge builds the gateway's Matrix handle over the node agent's client,
// the live room resolver, and the node's stable coordinates: the loopback
// homeserver URL and the full user ids of the agent and the person — the
// address and seats a power user points an external Matrix client at to reach
// the same rooms.
func NewBridge(agent, human *mautrix.Client, rooms RoomResolver, homeserverURL, agentID, humanID string, guests *GuestRegistry, provision GuestProvisioner) *Bridge {
	return &Bridge{
		agent:         agent,
		human:         human,
		rooms:         rooms,
		homeserverURL: homeserverURL,
		agentID:       agentID,
		humanID:       humanID,
		guests:        guests,
		provision:     provision,
	}
}

// OperatorSession returns the operating person's own Matrix credentials (@human's
// access token + user id), so the operator's browser/TUI can drive Matrix as
// that identity directly — the operator-side counterpart to GuestSession. The
// operator is a member of every session room, so their client syncs them all
// (there is no single room to scope to). ok is false when Matrix is off.
func (b *Bridge) OperatorSession() (accessToken, userID string, ok bool) {
	if b == nil || b.human == nil || b.human.AccessToken == "" {
		return "", "", false
	}
	return b.human.AccessToken, b.human.UserID.String(), true
}

// InviteGuest seats a shared-chat participant in a session's room as their own
// Matrix identity (ADR-0041 Step 4): it provisions (idempotently) a guest
// account for the display name, has the agent invite them, joins them, and
// registers (session, author) -> their client so the mirror publishes their
// messages under that identity. Returns the guest's full user id. This is the
// Matrix-native form of a chat share — room membership IS the grant — built on
// the same invite+join path that seats the person.
//
// Best-effort and idempotent: re-inviting an already-seated guest is a no-op on
// the room and just refreshes the registry binding. A nil provisioner (Matrix
// off, or guests disabled) returns an error the caller treats as "not enabled".
func (b *Bridge) InviteGuest(ctx context.Context, sid core.SessionID, author string) (string, error) {
	if b == nil || b.provision == nil || b.agent == nil {
		return "", fmt.Errorf("matrix: guest invites not enabled")
	}
	roomID, ok := b.RoomID(sid)
	if !ok {
		return "", fmt.Errorf("matrix: session %q has no room", sid)
	}
	client, userID, err := b.provision(ctx, author)
	if err != nil {
		return "", fmt.Errorf("matrix: provision guest: %w", err)
	}
	// Invite (idempotent: already-invited/joined is benign) then join, so the
	// guest is a real member able to post — the same path that seats the person.
	_, _ = b.agent.InviteUser(id.RoomID(roomID), &mautrix.ReqInviteUser{UserID: id.UserID(userID)})
	if _, err := client.JoinRoomByID(id.RoomID(roomID)); err != nil {
		return "", fmt.Errorf("matrix: guest join room: %w", err)
	}
	b.guests.Register(sid, author, client)
	return userID, nil
}

// RemoveGuest unseats a shared-chat participant when their share is revoked: the
// agent kicks them from the session's room and the registry binding is dropped,
// so membership tracks the grant's lifecycle (revoke -> no longer a member).
// This is what lets room membership become the access authority — a revoked
// guest who logged straight into an external Matrix client is removed too, not
// just denied at the bespoke seam. Best-effort and idempotent: a guest already
// gone is a benign no-op. A nil/unknown guest just clears the binding.
func (b *Bridge) RemoveGuest(_ context.Context, sid core.SessionID, author string) error {
	if b == nil {
		return nil
	}
	defer b.guests.Unregister(sid, author)
	roomID, ok := b.RoomID(sid)
	if !ok || b.agent == nil {
		return nil
	}
	// Kick by the guest's reconstructed user id: GuestLocalpart is deterministic
	// and the server name is fixed, so the mxid is reproducible even when the
	// in-memory client binding is gone (e.g. a revoke after a restart). This is
	// what makes membership track the grant across restarts, not just within a
	// process.
	guestID := id.NewUserID(GuestLocalpart(author), b.serverName())
	_, _ = b.agent.KickUser(id.RoomID(roomID), &mautrix.ReqKickUser{
		UserID: guestID,
		Reason: "share revoked",
	})
	return nil
}

// GuestSession returns a seated guest's live Matrix credentials — the access
// token and user id of the account the gateway logged them into — so the
// browser can drive Matrix as that identity itself (the browser-as-Matrix-client
// path, ADR-0041 P2). ok is false when the guest isn't currently seated in this
// process (e.g. before redemption, or across a restart), in which case the
// caller keeps using the bespoke seam. The token is the guest's own low-power
// credential, scoped to what their room membership allows.
func (b *Bridge) GuestSession(sid core.SessionID, author string) (accessToken, userID string, ok bool) {
	if b == nil {
		return "", "", false
	}
	client, ok := b.guests.client(sid, author)
	if !ok || client == nil || client.AccessToken == "" {
		return "", "", false
	}
	return client.AccessToken, client.UserID.String(), true
}

// serverName is the homeserver's Matrix server_name, recovered from the agent's
// own user id ("@agent:localhost" -> "localhost").
func (b *Bridge) serverName() string {
	if i := strings.IndexByte(b.agentID, ':'); i >= 0 {
		return b.agentID[i+1:]
	}
	return ""
}

// SessionForRoom reverse-resolves a room id to its session, if the room backs
// one (the watcher uses it to route an inbound room message to its chat).
func (b *Bridge) SessionForRoom(roomID string) (core.SessionID, bool) {
	if b == nil || b.rooms == nil {
		return "", false
	}
	return b.rooms.SessionForRoom(roomID)
}

// RoomID resolves the room backing a session, if mirroring provisioned one.
func (b *Bridge) RoomID(sid core.SessionID) (string, bool) {
	if b == nil || b.rooms == nil {
		return "", false
	}
	return b.rooms.RoomID(sid)
}

// HomeserverURL is the base URL of the node's homeserver — the address an
// external Matrix client (Element) points at to reach the same rooms.
func (b *Bridge) HomeserverURL() string {
	if b == nil {
		return ""
	}
	return b.homeserverURL
}

// AgentID is the autonomous agent's Matrix user id (a room's resident member).
func (b *Bridge) AgentID() string {
	if b == nil {
		return ""
	}
	return b.agentID
}

// HumanID is the operating person's Matrix user id — the seat the human holds
// in every session room, distinct from the agent (D9).
func (b *Bridge) HumanID() string {
	if b == nil {
		return ""
	}
	return b.humanID
}
