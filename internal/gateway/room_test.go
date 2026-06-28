package gateway

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/unarbos/arbos/internal/core"
)

type fakeMatrix struct {
	rooms map[core.SessionID]string
}

func (f fakeMatrix) RoomID(sid core.SessionID) (string, bool) {
	id, ok := f.rooms[sid]
	return id, ok
}
func (f fakeMatrix) HomeserverURL() string { return "http://127.0.0.1:18008" }
func (f fakeMatrix) AgentID() string       { return "@agent:localhost" }
func (f fakeMatrix) HumanID() string       { return "@human:localhost" }
func (f fakeMatrix) Watch(context.Context, func(core.SessionID, string, string, bool)) error {
	return nil
}
func (f fakeMatrix) InviteGuest(context.Context, core.SessionID, string) (string, error) {
	return "", nil
}
func (f fakeMatrix) RemoveGuest(context.Context, core.SessionID, string) error { return nil }
func (f fakeMatrix) GuestSession(core.SessionID, string) (string, string, bool) {
	return "", "", false
}
func (f fakeMatrix) OperatorSession() (string, string, bool) { return "", "", false }

// With Matrix on, a session's room and the homeserver coordinates are served so
// the UI can show the chat is a real, addressable room.
func TestHandleSessionRoom(t *testing.T) {
	s := &Server{Matrix: fakeMatrix{rooms: map[core.SessionID]string{"s-1": "!abc:localhost"}}}
	srv := httptest.NewServer(s.Handler())
	defer srv.Close()

	resp, err := http.Get(srv.URL + "/api/sessions/s-1/room")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("status = %d, want 200", resp.StatusCode)
	}
	var got roomJSON
	if err := json.NewDecoder(resp.Body).Decode(&got); err != nil {
		t.Fatal(err)
	}
	if got.RoomID != "!abc:localhost" {
		t.Errorf("room_id = %q, want !abc:localhost", got.RoomID)
	}
	if got.Homeserver != "http://127.0.0.1:18008" || got.AgentID != "@agent:localhost" || got.HumanID != "@human:localhost" {
		t.Errorf("coordinates wrong: %+v", got)
	}
}

// A session with no mirrored room 404s — the chat still works through the seam,
// it just isn't reachable as a room.
func TestHandleSessionRoomMissing(t *testing.T) {
	s := &Server{Matrix: fakeMatrix{rooms: map[core.SessionID]string{}}}
	srv := httptest.NewServer(s.Handler())
	defer srv.Close()

	resp, err := http.Get(srv.URL + "/api/sessions/nope/room")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusNotFound {
		t.Fatalf("status = %d, want 404", resp.StatusCode)
	}
}

// With Matrix off (nil bridge) the route is absent entirely — Matrix is purely
// additive, so the default gateway is byte-for-byte unchanged.
func TestRoomRouteAbsentWithoutMatrix(t *testing.T) {
	s := &Server{}
	srv := httptest.NewServer(s.Handler())
	defer srv.Close()

	resp, err := http.Get(srv.URL + "/api/sessions/s-1/room")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusNotFound {
		t.Fatalf("status = %d, want 404 (route should not exist)", resp.StatusCode)
	}
}
