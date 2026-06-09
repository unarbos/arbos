package fake

import (
	"context"
	"fmt"
	"sync"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/ports"
)

// Store is an in-memory SessionStore. It assigns Seq/ID on append, validates
// events, and is safe for concurrent use; the durable SQLite store (Phase 2)
// implements the same interface.
type Store struct {
	mu       sync.Mutex
	sessions map[core.SessionID]core.Session
	events   map[core.SessionID][]core.Event
	nextID   int64
}

func NewStore() *Store {
	return &Store{
		sessions: make(map[core.SessionID]core.Session),
		events:   make(map[core.SessionID][]core.Event),
	}
}

func (s *Store) CreateSession(ctx context.Context, sess core.Session) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, ok := s.sessions[sess.ID]; ok {
		return fmt.Errorf("session %q already exists", sess.ID)
	}
	s.sessions[sess.ID] = sess
	return nil
}

func (s *Store) Get(ctx context.Context, sessionID core.SessionID) (core.Session, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	sess, ok := s.sessions[sessionID]
	if !ok {
		return core.Session{}, fmt.Errorf("get session %q: %w", sessionID, ports.ErrSessionNotFound)
	}
	return sess, nil
}

func (s *Store) UpdateSession(ctx context.Context, sess core.Session) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, ok := s.sessions[sess.ID]; !ok {
		return fmt.Errorf("update session %q: %w", sess.ID, ports.ErrSessionNotFound)
	}
	s.sessions[sess.ID] = sess
	return nil
}

func (s *Store) AppendEvent(ctx context.Context, e *core.Event) error {
	if err := e.Validate(); err != nil {
		return fmt.Errorf("invalid event: %w", err)
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, ok := s.sessions[e.SessionID]; !ok {
		return fmt.Errorf("append to session %q: %w", e.SessionID, ports.ErrSessionNotFound)
	}
	s.nextID++
	e.ID = s.nextID
	e.Seq = int64(len(s.events[e.SessionID]))
	s.events[e.SessionID] = append(s.events[e.SessionID], *e)
	return nil
}

func (s *Store) Events(ctx context.Context, sessionID core.SessionID) ([]core.Event, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	src := s.events[sessionID]
	out := make([]core.Event, len(src))
	copy(out, src)
	return out, nil
}
