package matrix

import (
	"context"
	"sync"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/ports"
)

// Store is the ADR-0041 composite SessionStore. It pairs an authoritative
// **local capture** — the trajectory of record (D1 refinement: the owner's
// durable local log) — with a **Matrix mirror** that publishes Room-audience
// events into a per-session room (the shared, federated window). The local
// capture provides the SessionStore contract verbatim (Seq, ordering, fidelity,
// copy-on-read), so the engine's deterministic replay is unchanged; the mirror
// makes "every session is a room" true (D2/D11).
//
// With a nil Mirror the store is purely local — the offline / solo / test path,
// which is exactly "a homeserver with no peers" reduced to its log. Mirroring is
// best-effort and never affects the authoritative local result: a Matrix
// failure cannot corrupt or block the kernel's log (ADR-0009).
type Store struct {
	ports.SessionStore // the local capture (sqlite in production, fake in tests)

	mirror Mirror

	mu    sync.Mutex
	rooms map[core.SessionID]string // session -> mirrored room id
}

// Mirror publishes session activity to Matrix. It is an arbos-defined seam so
// the store stays free of any Matrix client type (the mautrix-backed impl lives
// in mirror.go); tests use a nil or fake Mirror.
type Mirror interface {
	// CreateRoom provisions the room that backs a session and returns its id.
	CreateRoom(ctx context.Context, sid core.SessionID) (roomID string, err error)
	// Publish sends one Room-audience event into the session's room.
	Publish(ctx context.Context, roomID string, e *core.Event) error
}

// NewStore wraps a local SessionStore with an optional Matrix mirror.
func NewStore(local ports.SessionStore, mirror Mirror) *Store {
	return &Store{SessionStore: local, mirror: mirror, rooms: map[core.SessionID]string{}}
}

// CreateSession records the session locally, then (if mirroring) provisions its
// Matrix room. A mirror failure fails the create — a session that cannot be
// reached by its room is not yet usable as a shared room (the room is its
// public identity); the local-only path skips this entirely.
func (s *Store) CreateSession(ctx context.Context, sess core.Session) error {
	if err := s.SessionStore.CreateSession(ctx, sess); err != nil {
		return err
	}
	if s.mirror == nil {
		return nil
	}
	roomID, err := s.mirror.CreateRoom(ctx, sess.ID)
	if err != nil {
		return err
	}
	s.mu.Lock()
	s.rooms[sess.ID] = roomID
	s.mu.Unlock()
	return nil
}

// AppendEvent writes to the authoritative local capture first (which assigns
// Seq/ID and enforces core.Event.Validate), then best-effort mirrors the event
// to the room when its effective audience is Room. The local result is returned
// regardless of mirror outcome.
func (s *Store) AppendEvent(ctx context.Context, e *core.Event) error {
	if err := s.SessionStore.AppendEvent(ctx, e); err != nil {
		return err
	}
	if s.mirror == nil || !mirrorable(e) {
		return nil
	}
	s.mu.Lock()
	roomID := s.rooms[e.SessionID]
	s.mu.Unlock()
	if roomID == "" {
		return nil // no room (e.g. session created before mirroring was enabled)
	}
	// Best-effort: a federation/mirror hiccup must not fail the kernel turn.
	_ = s.mirror.Publish(ctx, roomID, e)
	return nil
}

// RoomID returns the mirrored room for a session, if any (for the gateway /
// frontend to point a Matrix client at the right room).
func (s *Store) RoomID(sid core.SessionID) (string, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	id, ok := s.rooms[sid]
	return id, ok
}

// mirrorable reports whether an event should be published to the room: events
// explicitly raised to Room, plus events whose kind federates by default
// (DefaultAudience). Until the engine sets Audience explicitly, the default map
// (D1) governs — prose and chat notes reach the room; private trajectory does
// not.
func mirrorable(e *core.Event) bool {
	if e.Audience == core.AudienceRoom {
		return true
	}
	return core.DefaultAudience(e.Payload.Kind()) == core.AudienceRoom
}
