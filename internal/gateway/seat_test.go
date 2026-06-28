package gateway

import (
	"context"
	"path/filepath"
	"testing"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/sqlite"
)

// recordingMatrix records RemoveGuest calls so the revoke->kick wiring can be
// asserted without a live homeserver.
type recordingMatrix struct {
	fakeMatrix
	removed []sqlite.ShareSeat
}

func (m *recordingMatrix) RemoveGuest(_ context.Context, sid core.SessionID, author string) error {
	m.removed = append(m.removed, sqlite.ShareSeat{Session: string(sid), Author: author})
	return nil
}

// Seats are durable (survive a restart) and a revoke kicks exactly the guests a
// link seated — once — then forgets them, so membership tracks the grant.
func TestRevokeKicksSeatedGuests(t *testing.T) {
	store, err := sqlite.Open(filepath.Join(t.TempDir(), "seats.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = store.Close() }()
	ctx := context.Background()

	m := &recordingMatrix{}
	s := &Server{Store: store, Matrix: m}

	// Two guests seated by one link; a third by a different link.
	s.recordSeat(ctx, "tok-1", "sess-A", "Alice")
	s.recordSeat(ctx, "tok-1", "sess-A", "Bob")
	s.recordSeat(ctx, "tok-1", "sess-A", "Alice") // dup: recorded once
	s.recordSeat(ctx, "tok-2", "sess-B", "Carol")

	for _, seat := range s.takeSeats(ctx, "tok-1") {
		_ = m.RemoveGuest(ctx, core.SessionID(seat.Session), seat.Author)
	}

	want := map[sqlite.ShareSeat]bool{
		{Session: "sess-A", Author: "Alice"}: true,
		{Session: "sess-A", Author: "Bob"}:   true,
	}
	if len(m.removed) != len(want) {
		t.Fatalf("kicked %v, want the two sess-A guests", m.removed)
	}
	for _, seat := range m.removed {
		if !want[seat] {
			t.Errorf("unexpected kick: %v", seat)
		}
	}

	// tok-1's seats are now cleared; tok-2's untouched.
	if got := s.takeSeats(ctx, "tok-1"); len(got) != 0 {
		t.Errorf("tok-1 seats not cleared after revoke: %v", got)
	}
	if got := s.takeSeats(ctx, "tok-2"); len(got) != 1 || got[0] != (sqlite.ShareSeat{Session: "sess-B", Author: "Carol"}) {
		t.Errorf("tok-2 seat = %v, want one Carol seat", got)
	}
}
