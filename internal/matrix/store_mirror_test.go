package matrix

import (
	"context"
	"testing"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/fake"
	"github.com/unarbos/arbos/internal/matrix/hs"
	"maunium.net/go/mautrix"
	"maunium.net/go/mautrix/event"
	"maunium.net/go/mautrix/id"
)

// TestStoreMirrorsRoomEvents proves the composite (D2): the store mirrors
// Room-audience events into the session's Matrix room while keeping every event
// (Room and Local) in the authoritative local capture, and Local-audience
// events never reach the room.
func TestStoreMirrorsRoomEvents(t *testing.T) {
	ctx := context.Background()
	homeserver, err := hs.Start(ctx, t.TempDir(), "localhost", "127.0.0.1:18101")
	if err != nil {
		t.Fatalf("hs.Start: %v", err)
	}
	defer homeserver.Shutdown()

	client, err := mautrix.NewClient(homeserver.BaseURL, "", "")
	if err != nil {
		t.Fatalf("NewClient: %v", err)
	}
	if _, err := homeserver.CreateAccount(ctx, "agent", "mirror-proof-123"); err != nil {
		t.Fatalf("CreateAccount: %v", err)
	}
	if _, err := client.Login(&mautrix.ReqLogin{
		Type:             "m.login.password",
		Identifier:       mautrix.UserIdentifier{Type: "m.id.user", User: "agent"},
		Password:         "mirror-proof-123",
		StoreCredentials: true,
	}); err != nil {
		t.Fatalf("Login: %v", err)
	}

	store := NewStore(fake.NewStore(), NewClientMirror(client))
	const sid = core.SessionID("s-mirror")
	if err := store.CreateSession(ctx, core.Session{ID: sid, Status: core.SessionActive}); err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	roomID, ok := store.RoomID(sid)
	if !ok || roomID == "" {
		t.Fatalf("no room mirrored for session")
	}

	now := time.Now()
	// Room-audience (default for a user message) -> mirrored to the room.
	if err := store.AppendEvent(ctx, core.NewMessageEvent(sid, core.Message{Role: core.RoleUser, Content: "hello"}, now)); err != nil {
		t.Fatalf("append user message: %v", err)
	}
	// Local-audience (usage) -> stays local, never federated.
	if err := store.AppendEvent(ctx, core.NewUsageEvent(sid, core.Usage{}, now)); err != nil {
		t.Fatalf("append usage: %v", err)
	}

	// The authoritative local capture holds both events.
	evs, err := store.Events(ctx, sid)
	if err != nil {
		t.Fatalf("Events: %v", err)
	}
	if len(evs) != 2 {
		t.Fatalf("local capture: want 2 events, got %d", len(evs))
	}

	// The room holds the message but not the usage event.
	sync, err := client.SyncRequest(5000, "", "", false, event.PresenceOnline, ctx)
	if err != nil {
		t.Fatalf("sync: %v", err)
	}
	joined, ok := sync.Rooms.Join[id.RoomID(roomID)]
	if !ok {
		t.Fatalf("sync did not include mirrored room %s", roomID)
	}
	var sawMessage, sawUsage bool
	for _, ev := range joined.Timeline.Events {
		switch ev.Type.Type {
		case "m.room.message":
			sawMessage = true
		case "life.arbos.usage":
			sawUsage = true
		}
	}
	if !sawMessage {
		t.Fatal("Room-audience user message was not mirrored to the room")
	}
	if sawUsage {
		t.Fatal("Local-audience usage event leaked into the room")
	}
	t.Logf("mirror ok: room %s carries the prose, not the private usage", roomID)
}
