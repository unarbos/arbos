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

// loginResident provisions (idempotently) and logs in one localpart on the
// embedded homeserver for a test, returning its client and full user id.
func loginResident(t *testing.T, homeserver *hs.Homeserver, base, localpart string) (*mautrix.Client, string) {
	t.Helper()
	ctx := context.Background()
	client, err := mautrix.NewClient(base, "", "")
	if err != nil {
		t.Fatalf("NewClient(%s): %v", localpart, err)
	}
	if _, err := homeserver.CreateAccount(ctx, localpart, "mirror-proof-123"); err != nil {
		t.Fatalf("CreateAccount(%s): %v", localpart, err)
	}
	if _, err := client.Login(&mautrix.ReqLogin{
		Type:             "m.login.password",
		Identifier:       mautrix.UserIdentifier{Type: "m.id.user", User: localpart},
		Password:         "mirror-proof-123",
		StoreCredentials: true,
	}); err != nil {
		t.Fatalf("Login(%s): %v", localpart, err)
	}
	return client, "@" + localpart + ":localhost"
}

// TestStoreMirrorsRoomEvents proves the composite (D2): the store mirrors
// Room-audience events into the session's Matrix room while keeping every event
// (Room and Local) in the authoritative local capture, and Local-audience
// events never reach the room.
//
// It also proves the identity foundation (D9/D4): a session room has both
// resident members — the agent and the person — and each event is published as
// its real author (the person's message comes from the person, the agent's
// prose from the agent), so the Matrix sender is truthful.
func TestStoreMirrorsRoomEvents(t *testing.T) {
	ctx := context.Background()
	agent, agentID := loginResident(t, testHS, testHS.BaseURL, "agent")
	human, humanID := loginResident(t, testHS, testHS.BaseURL, "human")

	store := NewStore(fake.NewStore(), NewClientMirror(agent, human, humanID, nil, nil))
	const sid = core.SessionID("s-mirror")
	if err := store.CreateSession(ctx, core.Session{ID: sid, Status: core.SessionActive}); err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	roomID, ok := store.RoomID(sid)
	if !ok || roomID == "" {
		t.Fatalf("no room mirrored for session")
	}

	// The room has both resident members (agent created it; the person was
	// invited and joined).
	members, err := agent.JoinedMembers(id.RoomID(roomID))
	if err != nil {
		t.Fatalf("JoinedMembers: %v", err)
	}
	if _, ok := members.Joined[id.UserID(agentID)]; !ok {
		t.Errorf("agent %s is not a joined member of the room", agentID)
	}
	if _, ok := members.Joined[id.UserID(humanID)]; !ok {
		t.Errorf("person %s is not a joined member of the room — identity foundation broken", humanID)
	}

	now := time.Now()
	// A person's message (Room-audience by default) -> mirrored as the person.
	if err := store.AppendEvent(ctx, core.NewMessageEvent(sid, core.Message{Role: core.RoleUser, Content: "hello"}, now)); err != nil {
		t.Fatalf("append user message: %v", err)
	}
	// The agent's prose (Room-audience by default) -> mirrored as the agent.
	if err := store.AppendEvent(ctx, core.NewMessageEvent(sid, core.Message{Role: core.RoleAssistant, Content: "hi back"}, now)); err != nil {
		t.Fatalf("append assistant message: %v", err)
	}
	// Local-audience (usage) -> stays local, never federated.
	if err := store.AppendEvent(ctx, core.NewUsageEvent(sid, core.Usage{}, now)); err != nil {
		t.Fatalf("append usage: %v", err)
	}

	// The authoritative local capture holds all three events.
	evs, err := store.Events(ctx, sid)
	if err != nil {
		t.Fatalf("Events: %v", err)
	}
	if len(evs) != 3 {
		t.Fatalf("local capture: want 3 events, got %d", len(evs))
	}

	// The room holds both prose messages but not the usage event, and each
	// message carries its real author as the Matrix sender.
	sync, err := agent.SyncRequest(5000, "", "", false, event.PresenceOnline, ctx)
	if err != nil {
		t.Fatalf("sync: %v", err)
	}
	joined, ok := sync.Rooms.Join[id.RoomID(roomID)]
	if !ok {
		t.Fatalf("sync did not include mirrored room %s", roomID)
	}
	var personSaid, agentSaid, sawUsage bool
	for _, ev := range joined.Timeline.Events {
		switch ev.Type.Type {
		case "m.room.message":
			body, _ := ev.Content.Raw["body"].(string)
			switch ev.Sender {
			case id.UserID(humanID):
				if body == "hello" {
					personSaid = true
				}
			case id.UserID(agentID):
				if body == "hi back" {
					agentSaid = true
				}
			}
		case "life.arbos.usage":
			sawUsage = true
		}
	}
	if !personSaid {
		t.Errorf("the person's message was not mirrored under their identity (%s)", humanID)
	}
	if !agentSaid {
		t.Errorf("the agent's prose was not mirrored under the agent identity (%s)", agentID)
	}
	if sawUsage {
		t.Fatal("Local-audience usage event leaked into the room")
	}
	t.Logf("mirror ok: room %s carries the prose with truthful senders, not the private usage", roomID)
}

// TestSessionForRoomDurableFallback proves the watcher can route an inbound
// message from a resumed session's room — one created in a prior process, so
// absent from the new process's in-memory map. Without the durable reverse
// fallback, SessionForRoom misses and the watcher silently drops the message,
// breaking the whole inbound loop after a restart (caught end-to-end, not by a
// unit test, before this regression guard existed).
func TestSessionForRoomDurableFallback(t *testing.T) {
	ctx := context.Background()
	agent, _ := loginResident(t, testHS, testHS.BaseURL, "agent")
	human, humanID := loginResident(t, testHS, testHS.BaseURL, "human")

	// Process A creates the session room.
	storeA := NewStore(fake.NewStore(), NewClientMirror(agent, human, humanID, nil, nil))
	const sid = core.SessionID("s-resumed")
	if err := storeA.CreateSession(ctx, core.Session{ID: sid, Status: core.SessionActive}); err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	roomID, ok := storeA.RoomID(sid)
	if !ok || roomID == "" {
		t.Fatalf("no room mirrored for session")
	}

	// Process B resumes with a fresh, empty in-memory map (same homeserver).
	storeB := NewStore(fake.NewStore(), NewClientMirror(agent, human, humanID, nil, nil))
	got, ok := storeB.SessionForRoom(roomID)
	if !ok {
		t.Fatalf("SessionForRoom(%s) missed after resume — inbound loop would silently break", roomID)
	}
	if got != sid {
		t.Fatalf("SessionForRoom resolved %q, want %q", got, sid)
	}
	// And it's cached: a second call resolves identically from memory.
	if again, ok := storeB.SessionForRoom(roomID); !ok || again != sid {
		t.Fatalf("SessionForRoom not cached after durable resolution: ok=%v sid=%q", ok, again)
	}
	t.Logf("durable reverse-resolution ok: room %s -> session %s after a simulated restart", roomID, sid)
}
