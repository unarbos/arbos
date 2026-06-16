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
		conv      *engine.Conversation
		release   func() // detach this door from the session's ref-counted hub
		cancelSub func() // end this connection's subscription to the stream
		pump      sync.WaitGroup
		// turnActive tracks whether a turn is in flight on the bound session,
		// derived from the stream so it reflects a turn ANY door started — fork
		// refuses while it is true. Guarded by turnMu.
		turnMu     sync.Mutex
		turnActive bool
	)
	setTurn := func(active bool) {
		turnMu.Lock()
		turnActive = active
		turnMu.Unlock()
	}

	// teardown detaches this door: end the subscription, wait for the pump to
	// drain, then release the hub ref. It never cancels the actor — other doors
	// (or a quick reconnect inside the hub's grace window) keep it warm.
	teardown := func() {
		if cancelSub != nil {
			cancelSub()
			cancelSub = nil
		}
		pump.Wait()
		if release != nil {
			release()
			release = nil
		}
		conv = nil
		setTurn(false)
	}
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
		// Same echo suppression for a side-chat line: the sending tab already
		// rendered it optimistically; the broadcast is for the OTHER doors.
		if n, ok := env.Event.(core.ChatNote); ok && n.Origin == connOrigin {
			return true
		}
		b, err := core.EncodeEnvelope(env)
		if err != nil {
			_ = enc.write(serverFrame{Type: "error", Error: "encode envelope: " + err.Error()})
			return true
		}
		return enc.write(serverFrame{Type: "event", Envelope: b}) == nil
	}

	// bind attaches this connection to a session as one of N equal doors. It
	// joins the session's shared, ref-counted actor (so the actor outlives any
	// single door) and subscribes to the identical envelope stream every other
	// door sees — a web tab, a sibling browser window, a Telegram chat. There
	// is no owner/follower split and nothing to rebind when a turn ends: the
	// actor stays warm while any door is attached, and intents from every door
	// serialize through its one inbox.
	bind := func(id core.SessionID) error {
		teardown()
		c, rel, err := eng.Attach(id)
		if err != nil {
			return err
		}
		sub, cancelS := c.Subscribe()
		conv = c
		release = rel
		cancelSub = cancelS
		pump.Add(1)
		go func(self core.SessionID) {
			defer pump.Done()
			dead := false // client write side gone; keep draining, stop writing
			for env := range sub {
				// Track the shared session's turn state from the stream so fork
				// can refuse mid-turn no matter which door started the turn.
				if env.SessionID == self && env.Depth == 0 {
					switch env.Event.(type) {
					case core.TurnComplete, core.Interrupted, core.ErrorEvent:
						setTurn(false)
					case core.Queued, core.MessageDelta, core.ToolStarted:
						setTurn(true)
					}
				}
				if !dead && !writeEnvelope(env) {
					dead = true
					cancel() // no client left to receive; tear the connection down
				}
			}
		}(id)
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
			// Refuse to fork under an in-flight turn (tracked from the stream,
			// so it catches a turn any door started): the source log is being
			// written, and forking would branch a partial transcript.
			turnMu.Lock()
			busy := turnActive
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
				// reaches the session's OTHER doors (which haven't seen this
				// prompt) while writeEnvelope suppresses it here.
				if p.Origin == "" {
					p.Origin = connOrigin
					intent = p
				}
			}
			if n, isNote := intent.(core.ChatNoteIntent); isNote {
				// Same origin stamp as a prompt so the ChatNote broadcast reaches
				// the OTHER doors while writeEnvelope suppresses it here.
				// Deliberately absent from the setTurn switch below — a side-chat
				// note is not a turn and must not flip turn state on any door.
				if n.Origin == "" {
					n.Origin = connOrigin
					intent = n
				}
			}
			switch intent.(type) {
			case core.PromptIntent, core.SteerIntent:
				// Mark the turn in flight synchronously, so a disconnect right
				// after the prompt still drains it; the stream confirms this and
				// the pump flips it false at the terminal event.
				setTurn(true)
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

	// waitIdle holds Serve open on a client disconnect until the in-flight turn
	// reaches its terminal event (bounded by drain), so a client that pipes a
	// prompt and closes stdin — the CLI's one-shot path — still streams the
	// answer before teardown ends the subscription. The pump keeps forwarding
	// envelopes meanwhile; turnActive flips false when the terminal lands.
	waitIdle := func() {
		deadline := time.Now().Add(drain)
		for {
			turnMu.Lock()
			active := turnActive
			turnMu.Unlock()
			if !active || time.Now().After(deadline) {
				return
			}
			select {
			case <-time.After(20 * time.Millisecond):
			case <-sctx.Done():
				return
			}
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
				waitIdle()
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
