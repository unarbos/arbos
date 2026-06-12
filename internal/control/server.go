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
//	  {"type":"set_web_search","enabled":true}     // shorthand for SetWebSearchIntent
//	  {"type":"set_web_fetch","enabled":true}      // shorthand for SetWebFetchIntent
//	  {"type":"set_image_gen","enabled":true}      // shorthand for SetImageGenIntent
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
	"math"
	"sync"
	"sync/atomic"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
)

// liveSessions enforces the single-writer-per-session invariant that engine.go
// and sqlite/store.go assume: at most one ACTOR may be bound to a session id at
// a time, across every door in the process. A second open/switch/fork onto an
// already-active id does not spawn a second writer — it attaches to the
// existing actor as a follower (see bind), the same tap-and-inject seam the
// Telegram bridge uses. It is the engine's process-wide registry so
// out-of-band doors honor the same invariant as control connections.
var liveSessions = engine.Sessions

// connCounter distinguishes control connections so a prompt's Queued echo can
// be routed away from the connection that sent it (which already rendered it)
// while still reaching every other connection on the same session.
var connCounter atomic.Uint64

type clientFrame struct {
	Type      string          `json:"type"`
	SessionID core.SessionID  `json:"session_id,omitempty"`
	Intent    json.RawMessage `json:"intent,omitempty"`
	Model     string          `json:"model,omitempty"`
	Enabled   bool            `json:"enabled,omitempty"` // set_web_search: target toggle state

	NewSessionID core.SessionID `json:"new_session_id,omitempty"` // fork: id for the new branch (optional)
	ThroughSeq   *int64         `json:"through_seq,omitempty"`    // fork: last source seq to include; nil = whole log, negative = empty branch
}

// seamMaxLine bounds a single client frame (one newline-delimited line). It
// sits just above the gateway's 32 MiB WebSocket read limit so any frame the
// transport admits also fits the scanner; the headroom covers JSON framing
// around a max-size base64 attachment.
const seamMaxLine = 33 << 20

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
	// One client frame is one line. A prompt frame can carry base64 image/PDF
	// attachments (ADR-0022); the WebSocket transport caps a frame at 32 MiB
	// (gateway sets SetReadLimit(32<<20)), and the composer caps attachments at
	// 24 MiB. The scanner's max-token must therefore exceed the largest frame a
	// transport will deliver, or a legitimate ~MiB attachment dies mid-line with
	// "token too long" and takes the connection down. Size it above the WS read
	// limit; the buffer grows lazily, so small frames stay cheap. The transport
	// (WS read limit, stdio EOF) is the real DoS bound, not this line cap.
	sc.Buffer(make([]byte, 0, 64*1024), seamMaxLine)

	sctx, cancel := context.WithCancel(ctx)
	// connOrigin tags this connection's prompts so their cross-connection
	// Queued echo (rendered "as the user speaking" by other tabs on the same
	// session) can be suppressed on the way back to the tab that sent them.
	connOrigin := fmt.Sprintf("web:%d", connCounter.Add(1))
	var (
		conv *engine.Conversation
		// unbind releases whatever bind acquired: the owner's actor + registry
		// slot, or the follower's tap. Set by bind, cleared by teardown.
		unbind func()
		// follower marks a bind that attached to an actor another connection
		// owns; mode-dependent frames (fork) refuse rather than misbehave.
		follower bool
		pump     sync.WaitGroup
		turnDone chan struct{}
		turnMu   sync.Mutex
	)

	signalTurnStart := func() {
		turnMu.Lock()
		if turnDone != nil {
			// A new turn started before the previous one's terminal event
			// arrived; release any waiter on the old channel so it can't
			// block on a turn that will never signal again.
			close(turnDone)
		}
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

	teardown := func() {
		if unbind != nil {
			unbind()
			unbind = nil
			conv = nil
			follower = false
			// The pump exited without a terminal event (the session was
			// cancelled mid-turn); release any drain waiting on it.
			signalTurnEnd()
		}
	}
	defer pump.Wait()
	defer cancel()
	defer teardown()

	// writeEnvelope forwards one envelope to the client, suppressing the echo
	// of this connection's own prompts (the tab rendered them when it sent
	// them; the echo exists for the OTHER connections on the session).
	// Reports whether the client's write side is still alive.
	writeEnvelope := func(env core.Envelope) bool {
		if q, ok := env.Event.(core.Queued); ok && q.Origin == connOrigin {
			return true
		}
		b, err := core.EncodeEnvelope(env)
		if err != nil {
			_ = enc.write(serverFrame{Type: "error", Error: "encode envelope: " + err.Error()})
			return true
		}
		return enc.write(serverFrame{Type: "event", Envelope: b}) == nil
	}

	// bindOwner starts the session's actor on this connection — the exclusive
	// writer path, taken when the registry slot is free.
	bindOwner := func(id core.SessionID) error {
		ssctx, sscancel := context.WithCancel(sctx)
		c, err := eng.StartSession(ssctx, id)
		if err != nil {
			sscancel()
			liveSessions.Release(id)
			return err
		}
		conv = c
		unbind = func() {
			sscancel()
			pump.Wait()
			liveSessions.Release(id)
		}
		pump.Add(1)
		go func(c *engine.Conversation) {
			defer pump.Done()
			dead := false // client write side gone; keep draining, stop writing
			for env := range c.Events() {
				if !dead && !writeEnvelope(env) {
					dead = true
					cancel() // no client left to receive; tear the connection down
				}
				switch env.Event.(type) {
				case core.TurnComplete, core.ErrorEvent, core.Interrupted:
					signalTurnEnd()
				}
			}
		}(c)
		return nil
	}

	// bindFollower attaches to an actor another door owns: tap its envelope
	// stream for the outbound half and inject intents into it for the inbound
	// half — the same seam the Telegram bridge uses when a web tab holds its
	// session. Returns false if the actor died before the tap landed (the
	// caller retries, likely winning ownership). When the owning door tears
	// the actor down later, the whole connection drops: the client's
	// reconnect re-opens the session and wins the bind or follows the next
	// owner — reusing the client's existing healing path instead of growing a
	// server-side promotion protocol.
	bindFollower := func(id core.SessionID, src *engine.Conversation) bool {
		fctx, fcancel := context.WithCancel(sctx)
		// Buffered hand-off: the tap runs on the actor's emit path and must
		// never block. Overflow drops the envelope — presentation only; the
		// client reconciles from the persisted log on its next rebind.
		ch := make(chan core.Envelope, 1024)
		untap := src.Tap(func(env core.Envelope) {
			select {
			case ch <- env:
			default:
			}
		})
		select {
		case <-src.Done():
			untap()
			fcancel()
			return false
		default:
		}
		conv = src
		follower = true
		unbind = func() {
			fcancel()
			untap()
			pump.Wait()
		}
		pump.Add(1)
		go func() {
			defer pump.Done()
			dead := false
			for {
				select {
				case env := <-ch:
					if !dead && !writeEnvelope(env) {
						dead = true
						cancel()
					}
					switch env.Event.(type) {
					case core.TurnComplete, core.ErrorEvent, core.Interrupted:
						signalTurnEnd()
					}
				case <-src.Done():
					cancel()
					return
				case <-fctx.Done():
					return
				}
			}
		}()
		return true
	}

	bind := func(id core.SessionID) error {
		teardown()
		// Own the session when free; follow when another connection drives
		// it. The brief retry loop covers the races between the two (a
		// departing owner releases the registry slot before its actor exits,
		// and an actor can exit between the Live lookup and the tap).
		for attempt := 0; attempt < 50; attempt++ {
			if liveSessions.Acquire(id) {
				return bindOwner(id)
			}
			if src := eng.Live(id); src != nil && bindFollower(id, src) {
				return nil
			}
			// Held by an actor this engine can't see (e.g. a chat-only bridge
			// engine's turn); wait out the gap, then report it busy.
			select {
			case <-time.After(20 * time.Millisecond):
			case <-sctx.Done():
				return sctx.Err()
			}
		}
		return fmt.Errorf("session %q is already active on another connection", id)
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
			// A follower can't fork: the busy check below reads THIS
			// connection's turn tracking, which is blind to turns the owning
			// connection runs — a fork could silently land mid-turn.
			if follower {
				_ = enc.write(serverFrame{Type: "error", Error: "fork: session is driven by another connection"})
				return
			}
			source := f.SessionID
			if source == "" {
				if conv == nil {
					_ = enc.write(serverFrame{Type: "error", Error: "fork: no session open and no source given"})
					return
				}
				source = conv.ID()
			}
			// Refuse to fork under an in-flight turn: the source log is being
			// written, and the rebind below would silently cancel the turn.
			turnMu.Lock()
			busy := turnDone != nil
			turnMu.Unlock()
			if busy {
				_ = enc.write(serverFrame{Type: "error", Error: "fork: busy: a turn is in flight, interrupt it first"})
				return
			}
			newID := f.NewSessionID
			if newID == "" {
				newID = newSessionID()
			}
			// Omitted through_seq copies the whole log; an explicit negative
			// copies nothing (rewind to before the first message).
			through := int64(math.MaxInt64)
			if f.ThroughSeq != nil {
				through = *f.ThroughSeq
			}
			// Fork is a store copy, safe while the source is bound but idle
			// (no turn — checked above; frames are serial, so nothing can
			// start one underneath). Copying before teardown means a failure
			// leaves this connection bound to the source, transcript intact.
			if err := eng.ForkSession(sctx, source, newID, through); err != nil {
				_ = enc.write(serverFrame{Type: "error", Error: "fork: " + err.Error()})
				return
			}
			if err := bind(newID); err != nil {
				// The branch exists but can't be driven from here; rebind the
				// source (released by bind's teardown) so the connection
				// isn't left wedged with no session.
				if rerr := bind(source); rerr != nil {
					_ = enc.write(serverFrame{Type: "error", Error: "fork: bind: " + err.Error() + "; rebind source: " + rerr.Error()})
					return
				}
				_ = enc.write(serverFrame{Type: "error", Error: "fork: bind: " + err.Error()})
				return
			}
			_ = enc.write(serverFrame{Type: "forked", SessionID: newID})
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

		case "set_web_search":
			if conv == nil {
				_ = enc.write(serverFrame{Type: "error", Error: "no session open; send an 'open' frame first"})
				return
			}
			if !conv.TrySend(core.SetWebSearchIntent{Enabled: f.Enabled}) {
				_ = enc.write(serverFrame{Type: "error", Error: "busy: intent buffer full, retry"})
			}

		case "set_web_fetch":
			if conv == nil {
				_ = enc.write(serverFrame{Type: "error", Error: "no session open; send an 'open' frame first"})
				return
			}
			if !conv.TrySend(core.SetWebFetchIntent{Enabled: f.Enabled}) {
				_ = enc.write(serverFrame{Type: "error", Error: "busy: intent buffer full, retry"})
			}

		case "set_image_gen":
			if conv == nil {
				_ = enc.write(serverFrame{Type: "error", Error: "no session open; send an 'open' frame first"})
				return
			}
			if !conv.TrySend(core.SetImageGenIntent{Enabled: f.Enabled}) {
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
			if p, isPrompt := intent.(core.PromptIntent); isPrompt {
				// Stamp the connection's origin so the actor's Queued echo
				// reaches the session's OTHER connections (which haven't seen
				// this prompt) and writeEnvelope can suppress it here.
				if p.Origin == "" {
					p.Origin = connOrigin
					intent = p
				}
				signalTurnStart()
			}
			if _, isSteer := intent.(core.SteerIntent); isSteer {
				signalTurnStart()
			}
			switch intent.(type) {
			case core.InterruptIntent, core.SteerIntent:
				// Context-aware: if the actor's intent buffer is stuck (pump
				// stalled), a teardown still unblocks the request loop rather
				// than wedging every subsequent frame behind this send.
				conv.SendCtx(sctx, intent)
			default:
				if !conv.TrySend(intent) {
					_ = enc.write(serverFrame{Type: "error", Error: "busy: intent buffer full, retry"})
				}
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
