package matrix

import (
	"testing"

	"github.com/unarbos/arbos/internal/core"
)

type fakeResolver map[core.SessionID]string

func (f fakeResolver) RoomID(sid core.SessionID) (string, bool) {
	id, ok := f[sid]
	return id, ok
}

func (f fakeResolver) SessionForRoom(roomID string) (core.SessionID, bool) {
	for sid, rid := range f {
		if rid == roomID {
			return sid, true
		}
	}
	return "", false
}

func TestBridgeResolvesRoomAndCoordinates(t *testing.T) {
	b := NewBridge(nil, nil, fakeResolver{"s-1": "!room:localhost"}, "http://127.0.0.1:18008", "@agent:localhost", "@human:localhost", nil, nil)

	if got, ok := b.RoomID("s-1"); !ok || got != "!room:localhost" {
		t.Fatalf("RoomID(s-1) = %q,%v; want !room:localhost,true", got, ok)
	}
	if _, ok := b.RoomID("missing"); ok {
		t.Error("RoomID(missing) reported a room")
	}
	if b.HomeserverURL() != "http://127.0.0.1:18008" {
		t.Errorf("HomeserverURL = %q", b.HomeserverURL())
	}
	if b.AgentID() != "@agent:localhost" {
		t.Errorf("AgentID = %q", b.AgentID())
	}
	if b.HumanID() != "@human:localhost" {
		t.Errorf("HumanID = %q", b.HumanID())
	}
}

// A nil bridge is the Matrix-off path: every accessor is safe and reports
// "not enabled" so the gateway can wire the field unconditionally.
func TestNilBridgeIsSafe(t *testing.T) {
	var b *Bridge
	if _, ok := b.RoomID("s-1"); ok {
		t.Error("nil bridge reported a room")
	}
	if b.HomeserverURL() != "" || b.AgentID() != "" || b.HumanID() != "" {
		t.Error("nil bridge returned non-empty coordinates")
	}
}

// *Store satisfies RoomResolver — the bridge resolves rooms through the same
// map the composite store fills as it provisions a room per session.
func TestStoreSatisfiesRoomResolver(t *testing.T) {
	var _ RoomResolver = (*Store)(nil)
}
