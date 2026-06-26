package hs

import (
	"context"
	"testing"

	"maunium.net/go/mautrix"
	"maunium.net/go/mautrix/event"
	"maunium.net/go/mautrix/id"
)

// TestEmbeddedHomeserverRoundTrip is the ADR-0041 substrate proof: start the
// embedded Dendrite, then drive it through all three D10 channels via mautrix —
// room timeline (prose + a custom life.arbos.* event), to-device control, and a
// /sync round-trip that reads both back. Confirms the single-binary in-process
// path end to end.
func TestEmbeddedHomeserverRoundTrip(t *testing.T) {
	ctx := context.Background()
	homeserver, err := Start(ctx, t.TempDir(), "localhost", "127.0.0.1:18099")
	if err != nil {
		t.Fatalf("Start: %v", err)
	}
	defer homeserver.Shutdown()

	client, err := mautrix.NewClient(homeserver.BaseURL, "", "")
	if err != nil {
		t.Fatalf("NewClient: %v", err)
	}
	if _, err := homeserver.CreateAccount(ctx, "agent", "substrate-proof-123"); err != nil {
		t.Fatalf("CreateAccount: %v", err)
	}
	if _, err := client.Login(&mautrix.ReqLogin{
		Type:             "m.login.password",
		Identifier:       mautrix.UserIdentifier{Type: "m.id.user", User: "agent"},
		Password:         "substrate-proof-123",
		StoreCredentials: true,
	}); err != nil {
		t.Fatalf("Login: %v", err)
	}

	cr, err := client.CreateRoom(&mautrix.ReqCreateRoom{Name: "substrate", Preset: "private_chat"})
	if err != nil {
		t.Fatalf("CreateRoom: %v", err)
	}

	if _, err := client.SendMessageEvent(cr.RoomID, event.EventMessage,
		event.MessageEventContent{MsgType: event.MsgText, Body: "hello from arbos"}); err != nil {
		t.Fatalf("send m.room.message: %v", err)
	}
	if _, err := client.SendMessageEvent(cr.RoomID, event.NewEventType("life.arbos.tool_result"),
		map[string]any{"call_id": "c1", "content": "ok", "is_error": false}); err != nil {
		t.Fatalf("send life.arbos.tool_result: %v", err)
	}
	if _, err := client.SendToDevice(event.NewEventType("life.arbos.control"), &mautrix.ReqSendToDevice{
		Messages: map[id.UserID]map[id.DeviceID]*event.Content{
			client.UserID: {id.DeviceID("*"): {Raw: map[string]any{"op": "interrupt"}}},
		},
	}); err != nil {
		t.Fatalf("send to-device life.arbos.control: %v", err)
	}

	sync, err := client.SyncRequest(5000, "", "", false, event.PresenceOnline, ctx)
	if err != nil {
		t.Fatalf("sync: %v", err)
	}
	joined, ok := sync.Rooms.Join[cr.RoomID]
	if !ok {
		t.Fatalf("sync did not include room %s", cr.RoomID)
	}
	var sawMessage, sawCustom bool
	for _, ev := range joined.Timeline.Events {
		switch ev.Type.Type {
		case "m.room.message":
			sawMessage = true
		case "life.arbos.tool_result":
			sawCustom = true
		}
	}
	if !sawMessage || !sawCustom {
		t.Fatalf("round-trip incomplete: m.room.message=%v life.arbos.tool_result=%v", sawMessage, sawCustom)
	}
	t.Logf("substrate round-trip ok: %s on %s", client.UserID, homeserver.BaseURL)
}
