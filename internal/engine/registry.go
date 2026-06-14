package engine

import (
	"context"
	"fmt"
	"sync"
	"time"

	"github.com/unarbos/arbos/internal/core"
)

// Sessions is the process-global session registry. It guarantees at most one
// live actor per session id across every engine and door in the process: the
// host engine and the chat-only bridge engine share one durable store, so the
// invariant protects the log, not any single engine. Every door — a web tab, a
// Telegram chat, a shared link — attaches through it and rides the one actor;
// that actor outlives any single door and stops a grace period after the last
// one detaches, so a reconnect reuses the warm actor instead of churning it.
var Sessions = &SessionRegistry{hubs: map[core.SessionID]*sessionHub{}}

// actorGrace is how long a session's actor lingers after its last door
// detaches — sized to outlast a normal reconnect (the web seam backs off ~1s)
// so a refresh or a brief blip rebinds the warm actor instead of rebuilding it.
const actorGrace = 30 * time.Second

// SessionRegistry holds the live actor (and its door count) for each active
// session id.
type SessionRegistry struct {
	mu   sync.Mutex
	hubs map[core.SessionID]*sessionHub
}

// sessionHub is one session's single live actor plus the count of doors
// attached to it. The actor's context is the hub's, not any door's, so a
// departing door never cancels work the other doors are watching. eng records
// which engine runs it, so a door on a different engine (the chat-only bridge
// vs the full host) is refused rather than spawning a second writer on the log.
type sessionHub struct {
	eng    *Engine
	conv   *Conversation
	cancel context.CancelFunc
	refs   int
	grace  *time.Timer
}

// attach binds a door to id on eng, returning the shared Conversation and a
// release. The first door starts (or adopts) the actor on a hub-owned context;
// later doors ride the same one. A door whose engine differs from the engine
// already running the session is refused: the toolset is a property of the live
// actor, and two engines on one log would mean two writers.
func (s *SessionRegistry) attach(eng *Engine, id core.SessionID, opts ...SessionOption) (*Conversation, func(), error) {
	s.mu.Lock()
	h := s.hubs[id]
	if h != nil && h.eng != eng {
		s.mu.Unlock()
		return nil, nil, fmt.Errorf("session %q is active on another engine", id)
	}
	if h == nil {
		// The actor outlives the attaching door, so it runs on a hub-owned
		// context; the hub cancels it when the last door leaves.
		hubCtx, cancel := context.WithCancel(context.Background())
		conv, err := eng.StartSession(hubCtx, id, opts...)
		if err != nil {
			cancel()
			s.mu.Unlock()
			return nil, nil, err
		}
		h = &sessionHub{eng: eng, conv: conv, cancel: cancel}
		s.hubs[id] = h
	}
	if h.grace != nil {
		h.grace.Stop() // a door arrived inside the grace window; keep the actor
		h.grace = nil
	}
	h.refs++
	conv := h.conv
	s.mu.Unlock()

	var once sync.Once
	release := func() {
		once.Do(func() {
			s.mu.Lock()
			defer s.mu.Unlock()
			h.refs--
			if h.refs > 0 {
				return
			}
			h.grace = time.AfterFunc(actorGrace, func() {
				s.mu.Lock()
				defer s.mu.Unlock()
				// A door may have re-attached (bumping refs) during the grace.
				if s.hubs[id] == h && h.refs == 0 {
					h.cancel()
					delete(s.hubs, id)
				}
			})
		})
	}
	return conv, release, nil
}

// Attach binds a door to a session on this engine — the one entry point every
// door uses (web tabs, Telegram chats, shared links), which is what makes them
// interchangeable participants in one session: each Subscribe()s the identical
// stream and Send()s intents into the one serialized actor. See SessionRegistry.
func (e *Engine) Attach(id core.SessionID, opts ...SessionOption) (*Conversation, func(), error) {
	return Sessions.attach(e, id, opts...)
}

// Live returns the live conversation for id on this engine, if any — the handle
// an out-of-band caller uses to signal a session it did not start.
func (e *Engine) Live(id core.SessionID) *Conversation {
	e.liveMu.Lock()
	defer e.liveMu.Unlock()
	return e.live[id]
}
