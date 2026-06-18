package gateway

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"net/http"
	"sync"
	"time"

	"github.com/coder/websocket"

	"github.com/unarbos/arbos/internal/control"
	"github.com/unarbos/arbos/internal/share"
)

// localFrameType detects the two gateway-local ephemeral client frames
// (presence/typing) by a cheap prefix check, so ordinary prompt/intent frames
// fall straight through to the seam without an extra unmarshal. The web client
// always serializes these with "type" first.
func localFrameType(data []byte) (string, bool) {
	switch {
	case bytes.HasPrefix(data, []byte(`{"type":"hello"`)):
		return "hello", true
	case bytes.HasPrefix(data, []byte(`{"type":"typing"`)):
		return "typing", true
	default:
		return "", false
	}
}

// wsPingInterval keeps browser-facing sockets warm. The seam is silent
// between turns, and every hop in between (forest relay, TLS terminator,
// NAT) reaps idle connections — pings make the quiet socket look alive.
const wsPingInterval = 20 * time.Second

// keepAlive pings until the connection or ctx dies. The caller's read loop
// must be running (that's what processes the pongs).
func keepAlive(ctx context.Context, c *websocket.Conn) {
	t := time.NewTicker(wsPingInterval)
	defer t.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-t.C:
		}
		pctx, cancel := context.WithTimeout(ctx, wsPingInterval)
		err := c.Ping(pctx)
		cancel()
		if err != nil {
			return
		}
	}
}

// handleWS carries the control seam over one WebSocket connection: each text
// message from the browser is one client frame (a line), each server line is
// one text message back. control.Serve never learns it isn't on stdio — the
// bridge below adapts message framing to the seam's line framing, so the
// browser speaks the exact protocol documented in internal/control.
func (s *Server) handleWS(w http.ResponseWriter, r *http.Request) {
	c, err := websocket.Accept(w, r, s.wsAccept(r))
	if err != nil {
		return
	}
	defer func() { _ = c.Close(websocket.StatusInternalError, "gateway teardown") }()
	// A prompt can carry base64-encoded image attachments (ADR-0022), which dwarf
	// a text turn; 1 MiB would reject them. 32 MiB covers several inline images
	// while still bounding a hostile client.
	c.SetReadLimit(32 << 20)

	ctx := r.Context()
	go keepAlive(ctx, c)

	// A session-scoped share principal drives the seam through a frame-filter:
	// it is forced onto the one granted session and may send only
	// conversational intents (and only with write permission), so the real
	// ChatTab can run unchanged while a guest can neither reach another
	// session nor restructure this one. Full logins (and ScopeAll shares) have
	// no scoped grant here and pass through untouched.
	scoped, isScoped := shareFromContext(ctx)
	// The guest's self-asserted display name, resolved once from the trusted
	// scoped cookie (never the client frame) and stamped onto every prompt/steer
	// the frame-filter forwards, so a guest cannot impersonate another name.
	guestName := ""
	if isScoped {
		guestName = s.Auth.principalName(r)
	}

	lw := &wsLineWriter{ctx: ctx, c: c, srv: s}
	// A scoped guest's name is server-authoritative (from the cookie); set it now
	// so the roster names them the moment they bind. The host announces its own
	// name over the wire (a "hello" frame), since it lives client-side.
	if guestName != "" {
		lw.setName(guestName)
	}
	s.register(lw) // join the outbox + presence fan-out for the connection's life
	defer func() {
		sid := lw.boundSession()
		s.unregister(lw)
		s.broadcastRoster(sid) // the room learns this connection left
	}()

	// Browser -> seam: messages become newline-terminated lines on a pipe the
	// seam's scanner reads. Closing the pipe on read failure is what lets
	// control.Serve observe EOF and run its drain-then-exit path.
	pr, pw := io.Pipe()
	go func() {
		defer func() { _ = pw.Close() }()
		for {
			typ, data, err := c.Read(ctx)
			if err != nil {
				return
			}
			if typ != websocket.MessageText {
				continue
			}
			// Gateway-local ephemeral frames (presence/typing) are handled here
			// and NEVER piped to control.Serve or the engine — they're transport
			// state, not kernel events.
			if t, ok := localFrameType(data); ok {
				switch t {
				case "hello":
					// The host announces its display name; a scoped guest's name
					// is server-authoritative, so its announce is ignored
					// (anti-spoof, mirroring stampAuthor).
					if !isScoped {
						var f struct {
							Name string `json:"name"`
						}
						if json.Unmarshal(data, &f) == nil && lw.setName(sanitizeGuestName(f.Name)) {
							s.broadcastRoster(lw.boundSession())
						}
					}
				case "typing":
					// Only write-capable connections emit typing — a read-only
					// guest can't post, so it can't be "typing" either.
					if !isScoped || scoped.Perm >= share.PermWrite {
						s.broadcastTyping(lw)
					}
				}
				continue
			}
			if isScoped {
				line, keep := filterShareFrame(data, scoped.Scope.Ref, scoped.Perm, guestName)
				if !keep {
					continue
				}
				data = line
			}
			if len(data) == 0 || data[len(data)-1] != '\n' {
				data = append(data, '\n')
			}
			if _, err := pw.Write(data); err != nil {
				return
			}
		}
	}()

	err = control.Serve(ctx, s.Engine, pr, lw, s.NewSessionID, s.Drain)
	if err != nil && ctx.Err() == nil {
		_ = c.Close(websocket.StatusInternalError, "seam: "+err.Error())
		return
	}
	_ = c.Close(websocket.StatusNormalClosure, "")
}

// wsLineWriter sends each seam write as one text message. control.Serve's
// lineWriter emits exactly one line (with trailing newline) per Write, so the
// 1:1 mapping holds without buffering; the trailing newline is stripped since
// the message boundary already is the frame.
//
// It also watches the frames it carries: opened/switched/forked name the
// session this connection is bound to, which is what lets the outbox fan-out
// deliver a chat's notices to that chat alone. (A branched frame does NOT
// rebind — the parent connection stays on its session; the child is driven by
// its own sibling connection — so it is deliberately absent here.) Sniffing the
// stream keeps
// control.Serve ignorant of the gateway (the seam stays frontend-agnostic).
type wsLineWriter struct {
	ctx context.Context
	c   *websocket.Conn
	srv *Server // back-ref so a session bind can refresh the presence roster
	mu  sync.Mutex

	sessMu  sync.Mutex
	session string

	// name is the connection's display name for the People-panel roster: a
	// guest's server-stamped cookie name, or the host's client-announced name.
	// Ephemeral, gateway-only — never on the kernel log.
	nameMu sync.Mutex
	name   string
}

func (w *wsLineWriter) displayName() string {
	w.nameMu.Lock()
	defer w.nameMu.Unlock()
	return w.name
}

// setName records the connection's display name, reporting whether it changed
// (so the caller only re-broadcasts the roster on a real change).
func (w *wsLineWriter) setName(n string) bool {
	w.nameMu.Lock()
	defer w.nameMu.Unlock()
	if w.name == n {
		return false
	}
	w.name = n
	return true
}

// boundFrame is the slice of a server frame that names a session binding.
type boundFrame struct {
	Type      string `json:"type"`
	SessionID string `json:"session_id"`
}

func (w *wsLineWriter) sniffSession(p []byte) {
	// Event frames dominate the stream and never carry a binding; serverFrame
	// marshals Type first, so the prefix check skips them cheaply.
	if bytes.HasPrefix(p, []byte(`{"type":"event"`)) {
		return
	}
	var f boundFrame
	if json.Unmarshal(p, &f) != nil || f.SessionID == "" {
		return
	}
	switch f.Type {
	case "opened", "switched", "forked":
		w.sessMu.Lock()
		old := w.session
		w.session = f.SessionID
		w.sessMu.Unlock()
		// A new binding changes who is present on both the old and new session.
		// Broadcast asynchronously: this runs inside the hot Write path, and
		// broadcastRoster writes to peers (locking their mu) — doing it inline
		// would re-enter this connection's own Write and reorder frames.
		if w.srv != nil && f.SessionID != old {
			go w.srv.broadcastRoster(f.SessionID)
			if old != "" {
				go w.srv.broadcastRoster(old)
			}
		}
	}
}

// boundSession is the session this connection currently drives ("" before
// the first open).
func (w *wsLineWriter) boundSession() string {
	w.sessMu.Lock()
	defer w.sessMu.Unlock()
	return w.session
}

func (w *wsLineWriter) Write(p []byte) (int, error) {
	w.sniffSession(p)
	w.mu.Lock()
	defer w.mu.Unlock()
	n := len(p)
	if n > 0 && p[n-1] == '\n' {
		p = p[:n-1]
	}
	if err := w.c.Write(w.ctx, websocket.MessageText, p); err != nil {
		return 0, err
	}
	return n, nil
}
