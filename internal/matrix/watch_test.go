package matrix

import (
	"context"
	"testing"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/fake"
	"maunium.net/go/mautrix/event"
	"maunium.net/go/mautrix/id"
)

type watched struct {
	session core.SessionID
	sender  string
	body    string
	foreign bool
}

// TestBridgeWatchSurfacesForeignMessages proves the inbound channel (ADR-0041
// "the room is the session"): a message posted into a session room from an
// external Matrix client (here, the person's client sending raw, with no
// life.arbos metadata) is delivered to the watcher as foreign, while a message
// the engine itself mirrored (carrying the metadata) is delivered as NOT
// foreign — the signal the gateway uses to forward only genuinely-new inbound
// and never echo the local stream back.
func TestBridgeWatchSurfacesForeignMessages(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	agent, agentID := loginResident(t, testHS, testHS.BaseURL, "agent")
	human, humanID := loginResident(t, testHS, testHS.BaseURL, "human")

	store := NewStore(fake.NewStore(), NewClientMirror(agent, human, humanID, nil, nil))
	const sid = core.SessionID("s-watch")
	if err := store.CreateSession(ctx, core.Session{ID: sid, Status: core.SessionActive}); err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	roomID, _ := store.RoomID(sid)

	bridge := NewBridge(agent, human, store, testHS.BaseURL, agentID, humanID, nil, nil)
	seen := make(chan watched, 8)
	go func() {
		_ = bridge.Watch(ctx, func(session core.SessionID, sender, body string, foreign bool) {
			seen <- watched{session, sender, body, foreign}
		})
	}()

	// Let the watcher capture its starting sync token before any message is
	// sent, so the two below are streamed as new rather than skipped as history.
	time.Sleep(2 * time.Second)

	// (1) Foreign: the person posts raw from an "external client" — no metadata.
	if _, err := human.SendMessageEvent(id.RoomID(roomID), event.EventMessage, map[string]any{
		"msgtype": "m.text",
		"body":    "from element",
	}); err != nil {
		t.Fatalf("send foreign message: %v", err)
	}
	// (2) Not foreign: an engine event mirrored into the room (carries metadata).
	if err := store.AppendEvent(ctx, core.NewMessageEvent(sid, core.Message{Role: core.RoleUser, Content: "from browser"}, time.Now())); err != nil {
		t.Fatalf("append engine message: %v", err)
	}

	deadline := time.After(20 * time.Second)
	var sawForeign bool
	for !sawForeign {
		select {
		case m := <-seen:
			switch m.body {
			case "from element":
				if !m.foreign {
					t.Error("external-client message was not marked foreign")
				}
				if m.session != sid {
					t.Errorf("foreign message routed to %q, want %q", m.session, sid)
				}
				sawForeign = true
			case "from browser":
				if m.foreign {
					t.Error("engine-mirrored message was marked foreign — would echo the local stream back to the browser")
				}
			}
		case <-deadline:
			t.Fatal("watcher never surfaced the external-client message")
		}
	}
}
