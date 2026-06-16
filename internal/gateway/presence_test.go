package gateway

import (
	"reflect"
	"testing"
)

func TestLocalFrameType(t *testing.T) {
	cases := map[string]struct {
		in      string
		want    string
		isLocal bool
	}{
		"hello":               {`{"type":"hello","name":"Alice"}`, "hello", true},
		"typing":              {`{"type":"typing"}`, "typing", true},
		"prompt is not local": {`{"type":"intent","intent":{"kind":"prompt","data":{"text":"hi"}}}`, "", false},
		"open is not local":   {`{"type":"open","session_id":"s1"}`, "", false},
	}
	for name, c := range cases {
		t.Run(name, func(t *testing.T) {
			got, ok := localFrameType([]byte(c.in))
			if ok != c.isLocal || got != c.want {
				t.Errorf("localFrameType(%s) = (%q,%v), want (%q,%v)", c.in, got, ok, c.want, c.isLocal)
			}
		})
	}
}

// rosterFor dedupes by name, substitutes "Host" for an unnamed connection,
// sorts, and scopes strictly to the requested session.
func TestRosterFor(t *testing.T) {
	s := &Server{}
	mk := func(sid, name string) {
		s.register(&wsLineWriter{session: sid, name: name})
	}
	mk("s1", "Alice")
	mk("s1", "Bob")
	mk("s1", "Alice") // duplicate name (two windows) -> one entry
	mk("s1", "")      // unnamed connection -> omitted from the roster
	mk("s2", "Carol") // other session, excluded

	if got, want := s.rosterFor("s1"), []string{"Alice", "Bob"}; !reflect.DeepEqual(got, want) {
		t.Errorf("rosterFor(s1) = %v, want %v", got, want)
	}
	if got, want := s.rosterFor("s2"), []string{"Carol"}; !reflect.DeepEqual(got, want) {
		t.Errorf("rosterFor(s2) = %v, want %v", got, want)
	}
	if got := s.rosterFor(""); got != nil {
		t.Errorf("rosterFor(empty) = %v, want nil", got)
	}
}

func TestSessionPeers(t *testing.T) {
	s := &Server{}
	a := &wsLineWriter{session: "s1", name: "Alice"}
	b := &wsLineWriter{session: "s1", name: "Bob"}
	c := &wsLineWriter{session: "s2", name: "Carol"}
	for _, w := range []*wsLineWriter{a, b, c} {
		s.register(w)
	}
	if n := len(s.sessionPeers("s1")); n != 2 {
		t.Errorf("sessionPeers(s1) = %d, want 2", n)
	}
	if n := len(s.sessionPeers("")); n != 0 {
		t.Errorf("sessionPeers(empty) = %d, want 0", n)
	}
}

// broadcast helpers must be safe no-ops when there is nothing to send, without
// touching a (nil) websocket conn.
func TestBroadcastNoopsAreSafe(t *testing.T) {
	s := &Server{}
	s.broadcastRoster("")                           // no session
	s.broadcastRoster("nobody")                     // no peers
	s.broadcastTyping(&wsLineWriter{})              // unbound + unnamed
	s.broadcastTyping(&wsLineWriter{session: "s1"}) // bound but unnamed
	s.broadcastTyping(&wsLineWriter{name: "Alice"}) // named but unbound
}
