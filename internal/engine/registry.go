package engine

import (
	"sync"

	"github.com/unarbos/arbos/internal/core"
)

// Sessions enforces the single-writer-per-session invariant the engine and
// store assume: at most one actor may be bound to a session id at a time,
// across every door in the process — control connections (the terminal, the
// web seam) and out-of-band doors (the Telegram bridge) alike. It is
// process-wide rather than per-Engine because two engines can share one
// durable store (the host engine and the chat-only bridge engine), and the
// invariant protects the log, not the engine.
var Sessions = &SessionRegistry{live: make(map[core.SessionID]bool)}

// SessionRegistry tracks which session ids have a live actor.
type SessionRegistry struct {
	mu   sync.Mutex
	live map[core.SessionID]bool
}

// Acquire marks id active, returning false if it already is.
func (s *SessionRegistry) Acquire(id core.SessionID) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.live[id] {
		return false
	}
	s.live[id] = true
	return true
}

// Release frees id for the next door.
func (s *SessionRegistry) Release(id core.SessionID) {
	s.mu.Lock()
	defer s.mu.Unlock()
	delete(s.live, id)
}

// Live returns the live conversation for id on this engine, if any — the
// handle a door uses to tap a session another door is driving.
func (e *Engine) Live(id core.SessionID) *Conversation {
	e.liveMu.Lock()
	defer e.liveMu.Unlock()
	return e.live[id]
}
