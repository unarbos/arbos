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
)

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
			if len(data) == 0 || data[len(data)-1] != '\n' {
				data = append(data, '\n')
			}
			if _, err := pw.Write(data); err != nil {
				return
			}
		}
	}()

	lw := &wsLineWriter{ctx: ctx, c: c}
	s.register(lw) // join the outbox fan-out for the connection's lifetime
	defer s.unregister(lw)

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
// deliver a chat's notices to that chat alone. Sniffing the stream keeps
// control.Serve ignorant of the gateway (the seam stays frontend-agnostic).
type wsLineWriter struct {
	ctx context.Context
	c   *websocket.Conn
	mu  sync.Mutex

	sessMu  sync.Mutex
	session string
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
		w.session = f.SessionID
		w.sessMu.Unlock()
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
