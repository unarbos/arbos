package gateway

import (
	"context"
	"io"
	"net/http"
	"sync"

	"github.com/coder/websocket"

	"github.com/unarbos/arbos/internal/control"
)

// handleWS carries the control seam over one WebSocket connection: each text
// message from the browser is one client frame (a line), each server line is
// one text message back. control.Serve never learns it isn't on stdio — the
// bridge below adapts message framing to the seam's line framing, so the
// browser speaks the exact protocol documented in internal/control.
func (s *Server) handleWS(w http.ResponseWriter, r *http.Request) {
	c, err := websocket.Accept(w, r, &websocket.AcceptOptions{
		// The gateway binds localhost and has no cookie auth to ride; the
		// origin check would only refuse the Vite dev server.
		InsecureSkipVerify: true,
	})
	if err != nil {
		return
	}
	defer c.Close(websocket.StatusInternalError, "gateway teardown")
	c.SetReadLimit(1 << 20)

	ctx := r.Context()

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

	err = control.Serve(ctx, s.Engine, pr, &wsLineWriter{ctx: ctx, c: c}, s.NewSessionID, s.Drain)
	if err != nil && ctx.Err() == nil {
		c.Close(websocket.StatusInternalError, "seam: "+err.Error())
		return
	}
	c.Close(websocket.StatusNormalClosure, "")
}

// wsLineWriter sends each seam write as one text message. control.Serve's
// lineWriter emits exactly one line (with trailing newline) per Write, so the
// 1:1 mapping holds without buffering; the trailing newline is stripped since
// the message boundary already is the frame.
type wsLineWriter struct {
	ctx context.Context
	c   *websocket.Conn
	mu  sync.Mutex
}

func (w *wsLineWriter) Write(p []byte) (int, error) {
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
