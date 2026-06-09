// Package control is the kernel's headless control seam: a newline-delimited JSON
// protocol that carries Intents in and Envelopes out over any io.Reader/io.Writer
// (stdio, a Unix socket, a WebSocket shim). It is the single attach point every
// out-of-process frontend uses so none of them embeds the engine or re-implements
// turn logic.
//
// Protocol (one JSON object per line):
//
//	client -> server:
//	  {"type":"open","session_id":"abc"}           // omit id to start fresh
//	  {"type":"intent","intent":{kind,data}}       // encoded core.Intent
//	  {"type":"set_model","model":"m"}             // shorthand for SetModelIntent
//	  {"type":"compact"}                           // shorthand for CompactIntent
//	  {"type":"switch_session","session_id":"abc"} // bind another session
//	  {"type":"fork","session_id":"child",...}     // fork from current session
//	server -> client:
//	  {"type":"opened","session_id":"abc"}
//	  {"type":"switched","session_id":"abc"}
//	  {"type":"forked","session_id":"child"}
//	  {"type":"event","envelope":{...}}
//	  {"type":"error","error":"..."}
package control

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"sync"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
)

// liveSessions enforces the single-writer-per-session invariant that engine.go
// and sqlite/store.go assume: at most one actor may be bound to a session id at
// a time, across every connection in the process. A second open/switch/fork onto
// an already-active id is refused rather than spawning a second writer.
var liveSessions = &sessionRegistry{live: make(map[core.SessionID]bool)}

type sessionRegistry struct {
	mu   sync.Mutex
	live map[core.SessionID]bool
}

// acquire marks id active, returning false if it already is.
func (s *sessionRegistry) acquire(id core.SessionID) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.live[id] {
		return false
	}
	s.live[id] = true
	return true
}

func (s *sessionRegistry) release(id core.SessionID) {
	s.mu.Lock()
	defer s.mu.Unlock()
	delete(s.live, id)
}

type clientFrame struct {
	Type         string          `json:"type"`
	SessionID    core.SessionID  `json:"session_id,omitempty"`
	Intent       json.RawMessage `json:"intent,omitempty"`
	Model        string          `json:"model,omitempty"`
	NewSessionID core.SessionID  `json:"new_session_id,omitempty"` // fork: id for the new branch (optional)
	ThroughSeq   *int64          `json:"through_seq,omitempty"`    // fork: last source seq to include; nil = whole log
}

type serverFrame struct {
	Type      string          `json:"type"`
	SessionID core.SessionID  `json:"session_id,omitempty"`
	Envelope  json.RawMessage `json:"envelope,omitempty"`
	Error     string          `json:"error,omitempty"`
}

// Serve runs one client connection to completion: it reads framed requests from
// r and writes framed responses to w until r reaches EOF or ctx is cancelled.
// One connection drives one session (opened or resumed by id). newSessionID
// supplies an id when the client opens without one. drain bounds how long a
// disconnecting client waits for an in-flight turn; zero means 120s.
func Serve(ctx context.Context, eng *engine.Engine, r io.Reader, w io.Writer, newSessionID func() core.SessionID, drain time.Duration) error {
	if drain <= 0 {
		drain = 120 * time.Second
	}
	enc := &lineWriter{w: w}
	sc := bufio.NewScanner(r)
	sc.Buffer(make([]byte, 0, 64*1024), 1024*1024)

	sctx, cancel := context.WithCancel(ctx)
	var (
		conv          *engine.Conversation
		sessionCancel context.CancelFunc
		boundID       core.SessionID
		pump          sync.WaitGroup
		turnDone      chan struct{}
		turnMu        sync.Mutex
	)

	teardown := func() {
		if sessionCancel != nil {
			sessionCancel()
			pump.Wait()
			liveSessions.release(boundID)
			sessionCancel = nil
			conv = nil
			boundID = ""
		}
	}
	defer pump.Wait()
	defer cancel()
	defer teardown()

	signalTurnStart := func() {
		turnMu.Lock()
		turnDone = make(chan struct{})
		turnMu.Unlock()
	}

	signalTurnEnd := func() {
		turnMu.Lock()
		if turnDone != nil {
			close(turnDone)
			turnDone = nil
		}
		turnMu.Unlock()
	}

	bind := func(id core.SessionID) error {
		teardown()
		if !liveSessions.acquire(id) {
			return fmt.Errorf("session %q is already active on another connection", id)
		}
		ssctx, sscancel := context.WithCancel(sctx)
		c, err := eng.StartSession(ssctx, id)
		if err != nil {
			sscancel()
			liveSessions.release(id)
			return err
		}
		conv = c
		sessionCancel = sscancel
		boundID = id
		pump.Add(1)
		go func(c *engine.Conversation) {
			defer pump.Done()
			for env := range c.Events() {
				b, err := core.EncodeEnvelope(env)
				if err != nil {
					_ = enc.write(serverFrame{Type: "error", Error: "encode envelope: " + err.Error()})
					continue
				}
				_ = enc.write(serverFrame{Type: "event", Envelope: b})
				switch env.Event.(type) {
				case core.TurnComplete, core.ErrorEvent, core.Interrupted:
					signalTurnEnd()
				}
			}
		}(c)
		return nil
	}

	handleFrame := func(f clientFrame) {
		switch f.Type {
		case "open":
			if conv != nil {
				_ = enc.write(serverFrame{Type: "error", Error: "session already open on this connection"})
				return
			}
			id := f.SessionID
			if id == "" {
				id = newSessionID()
			}
			if err := bind(id); err != nil {
				_ = enc.write(serverFrame{Type: "error", Error: "open: " + err.Error()})
				return
			}
			_ = enc.write(serverFrame{Type: "opened", SessionID: id})

		case "switch_session":
			if f.SessionID == "" {
				_ = enc.write(serverFrame{Type: "error", Error: "switch_session requires session_id"})
				return
			}
			if err := bind(f.SessionID); err != nil {
				_ = enc.write(serverFrame{Type: "error", Error: "switch_session: " + err.Error()})
				return
			}
			_ = enc.write(serverFrame{Type: "switched", SessionID: f.SessionID})

		case "fork":
			source := f.SessionID
			if source == "" {
				if conv == nil {
					_ = enc.write(serverFrame{Type: "error", Error: "fork: no session open and no source given"})
					return
				}
				source = conv.ID()
			}
			newID := f.NewSessionID
			if newID == "" {
				newID = newSessionID()
			}
			through := int64(-1)
			if f.ThroughSeq != nil {
				through = *f.ThroughSeq
			}
			teardown()
			if err := eng.ForkSession(sctx, source, newID, through); err != nil {
				_ = enc.write(serverFrame{Type: "error", Error: "fork: " + err.Error()})
				return
			}
			_ = enc.write(serverFrame{Type: "forked", SessionID: newID})
			if err := bind(newID); err != nil {
				_ = enc.write(serverFrame{Type: "error", Error: "fork: bind: " + err.Error()})
				return
			}
			_ = enc.write(serverFrame{Type: "switched", SessionID: newID})

		case "set_model":
			if conv == nil {
				_ = enc.write(serverFrame{Type: "error", Error: "no session open; send an 'open' frame first"})
				return
			}
			if f.Model == "" {
				_ = enc.write(serverFrame{Type: "error", Error: "set_model requires model"})
				return
			}
			if !conv.TrySend(core.SetModelIntent{Model: f.Model}) {
				_ = enc.write(serverFrame{Type: "error", Error: "busy: intent buffer full, retry"})
			}

		case "compact":
			if conv == nil {
				_ = enc.write(serverFrame{Type: "error", Error: "no session open; send an 'open' frame first"})
				return
			}
			if !conv.TrySend(core.CompactIntent{}) {
				_ = enc.write(serverFrame{Type: "error", Error: "busy: intent buffer full, retry"})
			}

		case "intent":
			if conv == nil {
				_ = enc.write(serverFrame{Type: "error", Error: "no session open; send an 'open' frame first"})
				return
			}
			intent, err := core.DecodeIntent(f.Intent)
			if err != nil {
				_ = enc.write(serverFrame{Type: "error", Error: "decode intent: " + err.Error()})
				return
			}
			if _, isPrompt := intent.(core.PromptIntent); isPrompt {
				signalTurnStart()
			}
			if _, isInterrupt := intent.(core.InterruptIntent); isInterrupt {
				conv.Send(intent)
			} else if !conv.TrySend(intent) {
				_ = enc.write(serverFrame{Type: "error", Error: "busy: intent buffer full, retry"})
			}

		default:
			_ = enc.write(serverFrame{Type: "error", Error: fmt.Sprintf("unknown frame type %q", f.Type)})
		}
	}

	waitDrain := func() {
		turnMu.Lock()
		ch := turnDone
		turnMu.Unlock()
		if ch == nil {
			return
		}
		select {
		case <-ch:
		case <-time.After(drain):
		case <-sctx.Done():
		}
	}

	lines := make(chan []byte, 1)
	go func() {
		defer close(lines)
		for sc.Scan() {
			b := append([]byte(nil), sc.Bytes()...)
			select {
			case lines <- b:
			case <-sctx.Done():
				return
			}
		}
	}()

	for {
		select {
		case <-ctx.Done():
			return ctx.Err()
		case b, ok := <-lines:
			if !ok {
				waitDrain()
				if err := sc.Err(); err != nil {
					return err
				}
				return nil
			}
			var f clientFrame
			if err := json.Unmarshal(b, &f); err != nil {
				_ = enc.write(serverFrame{Type: "error", Error: "bad frame: " + err.Error()})
				continue
			}
			handleFrame(f)
		}
	}
}

// lineWriter serializes concurrent writes (the event pump goroutine and the
// request loop both write) to one newline-delimited JSON stream.
type lineWriter struct {
	mu sync.Mutex
	w  io.Writer
}

func (l *lineWriter) write(f serverFrame) error {
	b, err := json.Marshal(f)
	if err != nil {
		return err
	}
	l.mu.Lock()
	defer l.mu.Unlock()
	if _, err := l.w.Write(append(b, '\n')); err != nil {
		return err
	}
	return nil
}
