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
	// Negative caches for the durable homeserver fallbacks below: a session
	// (or room) we've already resolved as backing no room (or no session) on
	// the homeserver. Without these, a room-less session re-scans the
	// homeserver on every mirrorable append (RoomForSession is O(joined rooms)
	// state lookups), turning a one-time miss into per-event load on the kernel
	// append path.
	noRoom    map[core.SessionID]bool
	noSession map[string]bool
}

// Mirror publishes session activity to Matrix. It is an arbos-defined seam so
// the store stays free of any Matrix client type (the mautrix-backed impl lives
// in mirror.go); tests use a nil or fake Mirror.
type Mirror interface {
	// CreateRoom provisions the room that backs a session and returns its id.
	CreateRoom(ctx context.Context, sid core.SessionID) (roomID string, err error)
	// Publish sends one Room-audience event into the session's room.
	Publish(ctx context.Context, roomID string, e *core.Event) error
	// RoomForSession resolves a session to its existing room from the
	// homeserver — the durable fallback for a session created in a prior
	// process (resumed here) that isn't in the in-memory map. ok is false when
	// no room backs the session.
	RoomForSession(ctx context.Context, sid core.SessionID) (roomID string, ok bool)
	// SessionForRoom is the inverse: it resolves a room to its session from the
	// homeserver (the room's name is the session id), the durable fallback for
	// the watcher's room->session routing after a restart. ok is false when the
	// room is not an arbos session room.
	SessionForRoom(ctx context.Context, roomID string) (sid core.SessionID, ok bool)
}

// NewStore wraps a local SessionStore with an optional Matrix mirror.
func NewStore(local ports.SessionStore, mirror Mirror) *Store {
	return &Store{
		SessionStore: local,
		mirror:       mirror,
		rooms:        map[core.SessionID]string{},
		noRoom:       map[core.SessionID]bool{},
		noSession:    map[string]bool{},
	}
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
	delete(s.noRoom, sess.ID)
	delete(s.noSession, roomID)
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
	// Resolve through RoomID so a resumed session (absent from the in-memory
	// map) still mirrors — RoomID falls back to the homeserver and caches it.
	roomID, ok := s.RoomID(e.SessionID)
	if !ok || roomID == "" {
		return nil // no room backs this session
	}
	// Best-effort: a federation/mirror hiccup must not fail the kernel turn.
	_ = s.mirror.Publish(ctx, roomID, e)
	return nil
}

// RoomID returns the mirrored room for a session, if any (for the gateway /
// frontend to point a Matrix client at the right room).
func (s *Store) RoomID(sid core.SessionID) (string, bool) {
	s.mu.Lock()
	id, ok := s.rooms[sid]
	absent := s.noRoom[sid]
	s.mu.Unlock()
	if ok {
		return id, true
	}
	// A session already resolved as room-less: skip the homeserver scan (it
	// won't acquire a room later — CreateSession is the only path that mints
	// one, and it populates the map directly).
	if absent {
		return "", false
	}
	// Durable fallback: a session created in a prior process and resumed here
	// isn't in the in-memory map, but its room still lives on the homeserver
	// (named after the session). Resolve and cache it, so room resolution
	// survives a restart — not just sessions created in the running process.
	if s.mirror == nil {
		return "", false
	}
	rid, found := s.mirror.RoomForSession(context.Background(), sid)
	if !found || rid == "" {
		s.mu.Lock()
		s.noRoom[sid] = true
		s.mu.Unlock()
		return "", false
	}
	s.mu.Lock()
	s.rooms[sid] = rid
	s.mu.Unlock()
	return rid, true
}

// SessionForRoom reverse-resolves a room id to the session it backs, if any —
// the inverse of RoomID, used by the gateway's room watcher to route an inbound
// room message to its conversation. The room set is small (one per live
// session), so a linear scan is fine.
func (s *Store) SessionForRoom(roomID string) (core.SessionID, bool) {
	s.mu.Lock()
	for sid, rid := range s.rooms {
		if rid == roomID {
			s.mu.Unlock()
			return sid, true
		}
	}
	absent := s.noSession[roomID]
	s.mu.Unlock()
	// A room already resolved as backing no tracked session (e.g. a non-arbos
	// room the agent joined): skip the per-event homeserver lookup.
	if absent {
		return "", false
	}
	// Durable fallback (mirror of RoomID): a room backing a session created in a
	// prior process isn't in the in-memory map, so the watcher couldn't route an
	// inbound message from it. Resolve it from the homeserver (the room's name
	// is the session id) and cache it — without this, the inbound loop silently
	// breaks for any resumed session.
	if s.mirror == nil {
		return "", false
	}
	sid, found := s.mirror.SessionForRoom(context.Background(), roomID)
	if !found || sid == "" {
		s.mu.Lock()
		s.noSession[roomID] = true
		s.mu.Unlock()
		return "", false
	}
	s.mu.Lock()
	s.rooms[sid] = roomID
	s.mu.Unlock()
	return sid, true
}

// mirrorable reports whether an event should be published to the room: events
// explicitly raised to Room, plus events whose kind federates by default
// (DefaultAudience). Until the engine sets Audience explicitly, the default map
// (D1) governs — prose and chat notes reach the room; private trajectory does
// not.
//
// One exception overrides everything: a user message whose Origin is OriginRoom
// arrived FROM the room (an external client posted it, and the gateway injected
// it to drive a turn). It is already in the room, so re-publishing it would
// duplicate it — record it locally for the engine's history, but never mirror.
func mirrorable(e *core.Event) bool {
	if m, ok := e.Payload.(core.MessagePayload); ok && m.Message.Origin == core.OriginRoom {
		return false
	}
	if e.Audience == core.AudienceRoom {
		return true
	}
	return core.DefaultAudience(e.Payload.Kind()) == core.AudienceRoom
}
