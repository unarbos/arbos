package matrix

import (
	"context"
	"testing"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/fake"
	"maunium.net/go/mautrix"
	"maunium.net/go/mautrix/event"
	"maunium.net/go/mautrix/id"
)

func TestGuestLocalpart(t *testing.T) {
	cases := map[string]string{
		"Alice":     "guest-alice",
		"Alice B.":  "guest-alice-b.",
		"  spaced ": "guest-spaced",
		"!!!":       "guest",
		"":          "guest",
		"Bob_99":    "guest-bob_99",
	}
	for in, want := range cases {
		if got := GuestLocalpart(in); got != want {
			t.Errorf("GuestLocalpart(%q) = %q, want %q", in, got, want)
		}
	}
}

// TestInviteGuestSeatsParticipant proves Matrix-native sharing (ADR-0041 Step
// 4): inviting a guest by display name makes them a real, joined member of the
// session's room, and a message authored by that guest mirrors under the
// guest's own Matrix identity — not the host's seat.
func TestInviteGuestSeatsParticipant(t *testing.T) {
	ctx := context.Background()
	agent, agentID := loginResident(t, testHS, testHS.BaseURL, "agent")
	human, humanID := loginResident(t, testHS, testHS.BaseURL, "human")

	guests := NewGuestRegistry()
	store := NewStore(fake.NewStore(), NewClientMirror(agent, human, humanID, guests, nil))
	const sid = core.SessionID("s-guest")
	if err := store.CreateSession(ctx, core.Session{ID: sid, Status: core.SessionActive}); err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	roomID, _ := store.RoomID(sid)

	// The provisioner mints+logs in a guest account on the shared homeserver.
	provision := func(_ context.Context, label string) (*mautrix.Client, string, error) {
		c, uid := loginResident(t, testHS, testHS.BaseURL, GuestLocalpart(label))
		return c, uid, nil
	}
	bridge := NewBridge(agent, human, store, testHS.BaseURL, agentID, humanID, guests, provision)

	guestID, err := bridge.InviteGuest(ctx, sid, "Alice")
	if err != nil {
		t.Fatalf("InviteGuest: %v", err)
	}
	if guestID != "@guest-alice:localhost" {
		t.Fatalf("guest id = %q, want @guest-alice:localhost", guestID)
	}

	// The room now has three joined members: agent, person, and the guest.
	members, err := agent.JoinedMembers(id.RoomID(roomID))
	if err != nil {
		t.Fatalf("JoinedMembers: %v", err)
	}
	for _, want := range []string{agentID, humanID, guestID} {
		if _, ok := members.Joined[id.UserID(want)]; !ok {
			t.Errorf("%s is not a joined member of the shared room", want)
		}
	}

	// A message authored by the guest (the gateway stamps Message.Author with
	// their trusted name) mirrors under the guest's identity, not the host's.
	now := time.Now()
	if err := store.AppendEvent(ctx, core.NewMessageEvent(sid, core.Message{
		Role:    core.RoleUser,
		Content: "hi from Alice",
		Author:  "Alice",
	}, now)); err != nil {
		t.Fatalf("append guest message: %v", err)
	}

	sync, err := agent.SyncRequest(5000, "", "", false, event.PresenceOnline, ctx)
	if err != nil {
		t.Fatalf("sync: %v", err)
	}
	joined, ok := sync.Rooms.Join[id.RoomID(roomID)]
	if !ok {
		t.Fatalf("sync did not include shared room %s", roomID)
	}
	var guestSaid bool
	for _, ev := range joined.Timeline.Events {
		if ev.Type.Type != "m.room.message" {
			continue
		}
		if body, _ := ev.Content.Raw["body"].(string); body == "hi from Alice" {
			if ev.Sender != id.UserID(guestID) {
				t.Errorf("guest message sender = %s, want %s", ev.Sender, guestID)
			}
			guestSaid = true
		}
	}
	if !guestSaid {
		t.Error("the guest's message was not mirrored to the room under their identity")
	}

	// Browser-as-Matrix-client (P2.1): the guest's own access token, handed to
	// their browser, is a real working credential — a fresh client built from it
	// can /sync and see the shared room. This is what lets the browser drive
	// Matrix as the guest, with membership (not a bearer token) the authority.
	token, tokUser, ok := bridge.GuestSession(sid, "Alice")
	if !ok || token == "" {
		t.Fatalf("GuestSession returned no token for a seated guest")
	}
	if tokUser != guestID {
		t.Errorf("GuestSession user = %q, want %q", tokUser, guestID)
	}
	browserClient, err := mautrix.NewClient(testHS.BaseURL, id.UserID(tokUser), token)
	if err != nil {
		t.Fatalf("client from guest token: %v", err)
	}
	gsync, err := browserClient.SyncRequest(5000, "", "", true, event.PresenceOnline, ctx)
	if err != nil {
		t.Fatalf("guest-token sync: %v", err)
	}
	if _, ok := gsync.Rooms.Join[id.RoomID(roomID)]; !ok {
		t.Error("the guest's own token cannot see the shared room — handoff credential is not usable")
	}

	// Simulate a restart between seating and revoke: drop the in-memory client
	// binding, so RemoveGuest must reconstruct the guest's user id to kick them
	// (proving membership tracks the grant even across a process restart, not
	// only while the registry holds the live client).
	guests.Unregister(sid, "Alice")

	// Revoking the share unseats the guest: RemoveGuest kicks them, so the room
	// is back to two members and membership tracks the grant's lifecycle.
	if err := bridge.RemoveGuest(ctx, sid, "Alice"); err != nil {
		t.Fatalf("RemoveGuest: %v", err)
	}
	members, err = agent.JoinedMembers(id.RoomID(roomID))
	if err != nil {
		t.Fatalf("JoinedMembers after kick: %v", err)
	}
	if _, ok := members.Joined[id.UserID(guestID)]; ok {
		t.Error("guest is still a joined member after RemoveGuest — revoke did not track membership")
	}
	for _, want := range []string{agentID, humanID} {
		if _, ok := members.Joined[id.UserID(want)]; !ok {
			t.Errorf("%s should remain after the guest is kicked", want)
		}
	}
}
