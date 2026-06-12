package gateway

import (
	"context"
	"encoding/json"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"sync"

	"github.com/coder/websocket"
	"github.com/creack/pty"
)

// Interactive terminals: the user's own shells on the host, each a PTY the
// browser drives over one WebSocket. A terminal is a typed reference like
// every other tab (surface → path, run → session, terminal → id); the
// gateway owns the live process, the tab attaches to it. Scrollback is kept
// server-side so a re-attach (reconnect, pane move) replays what the shell
// already said. One attached client at a time — a second attach displaces
// the first, it never mirrors.

// termScrollback bounds the replay buffer per terminal.
const termScrollback = 256 * 1024

// termHost owns this process's interactive terminals. Unlike jobs (durable,
// filesystem-backed, shared across processes) a shell is conversational state:
// it lives and dies with the gateway process, like a chat's seam connection.
type termHost struct {
	mu    sync.Mutex
	next  int
	terms map[string]*termSession
}

type termSession struct {
	id  string
	cwd string

	mu     sync.Mutex
	ptmx   *os.File
	scroll []byte
	client *websocket.Conn // currently attached browser, if any
	exited bool
}

func (h *termHost) get(id string) *termSession {
	h.mu.Lock()
	defer h.mu.Unlock()
	return h.terms[id]
}

// create spawns the user's shell in a fresh PTY at cwd and starts its output
// pump. The pump is the terminal's single reader: every byte goes into the
// scrollback ring and to the attached client, whoever that currently is.
func (h *termHost) create(cwd string) (*termSession, error) {
	shell := os.Getenv("SHELL")
	if shell == "" {
		shell = "/bin/sh"
	}
	cmd := exec.Command(shell, "-l")
	cmd.Dir = cwd
	cmd.Env = append(os.Environ(), "TERM=xterm-256color")
	ptmx, err := pty.Start(cmd)
	if err != nil {
		return nil, err
	}

	h.mu.Lock()
	h.next++
	t := &termSession{id: "t" + strconv.Itoa(h.next), cwd: cwd, ptmx: ptmx}
	if h.terms == nil {
		h.terms = make(map[string]*termSession)
	}
	h.terms[t.id] = t
	h.mu.Unlock()

	go func() {
		buf := make([]byte, 8192)
		for {
			n, err := ptmx.Read(buf)
			if n > 0 {
				t.emit(buf[:n])
			}
			if err != nil {
				break
			}
		}
		_ = cmd.Wait()
		t.mu.Lock()
		t.exited = true
		client := t.client
		t.mu.Unlock()
		if client != nil {
			client.Close(websocket.StatusNormalClosure, "shell exited")
		}
	}()

	return t, nil
}

// emit appends output to the scrollback ring and forwards it to the attached
// client. A write failure just sheds the client; the shell keeps running.
func (t *termSession) emit(p []byte) {
	t.mu.Lock()
	t.scroll = append(t.scroll, p...)
	if len(t.scroll) > termScrollback {
		t.scroll = t.scroll[len(t.scroll)-termScrollback:]
	}
	client := t.client
	t.mu.Unlock()
	if client != nil {
		if err := client.Write(context.Background(), websocket.MessageBinary, p); err != nil {
			t.mu.Lock()
			if t.client == client {
				t.client = nil
			}
			t.mu.Unlock()
		}
	}
}

// attach binds a browser connection as the terminal's client: replay the
// scrollback, displace any previous client, then pump browser input into the
// PTY until the socket closes. Binary frames are keystrokes; text frames are
// control ({"resize":{cols,rows}}).
func (t *termSession) attach(r *http.Request, c *websocket.Conn) {
	ctx := r.Context()

	t.mu.Lock()
	prev := t.client
	t.client = c
	scroll := append([]byte(nil), t.scroll...)
	exited := t.exited
	t.mu.Unlock()

	if prev != nil {
		prev.Close(websocket.StatusPolicyViolation, "attached elsewhere")
	}
	if len(scroll) > 0 {
		_ = c.Write(ctx, websocket.MessageBinary, scroll)
	}
	if exited {
		c.Close(websocket.StatusNormalClosure, "shell exited")
		return
	}

	for {
		typ, data, err := c.Read(ctx)
		if err != nil {
			break
		}
		switch typ {
		case websocket.MessageBinary:
			// A write error means the shell is gone; the pump's close
			// notification ends the read loop, so just drop the keystrokes.
			_, _ = t.ptmx.Write(data)
		case websocket.MessageText:
			var msg struct {
				Resize *struct {
					Cols uint16 `json:"cols"`
					Rows uint16 `json:"rows"`
				} `json:"resize"`
			}
			if json.Unmarshal(data, &msg) == nil && msg.Resize != nil {
				_ = pty.Setsize(t.ptmx, &pty.Winsize{Cols: msg.Resize.Cols, Rows: msg.Resize.Rows})
			}
		}
	}

	t.mu.Lock()
	if t.client == c {
		t.client = nil
	}
	t.mu.Unlock()
}

// close tears the terminal down: closing the PTY master hangs up the shell
// (SIGHUP), the pump drains and notifies any attached client.
func (h *termHost) close(id string) {
	h.mu.Lock()
	t := h.terms[id]
	delete(h.terms, id)
	h.mu.Unlock()
	if t != nil {
		_ = t.ptmx.Close()
	}
}

/* ------------------------------ routes ------------------------------ */

// handleTermCreate spawns a shell, optionally in a workspace-relative cwd
// (a job terminal's "open a shell here"), and returns its reference.
func (s *Server) handleTermCreate(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Cwd string `json:"cwd"`
	}
	_ = json.NewDecoder(http.MaxBytesReader(w, r.Body, jsonBodyMax)).Decode(&req)
	cwd := s.Root
	if req.Cwd != "" {
		c := req.Cwd
		if !filepath.IsAbs(c) {
			c = filepath.Join(s.Root, c)
		}
		if st, err := os.Stat(c); err == nil && st.IsDir() {
			cwd = c
		}
	}
	t, err := s.terms.create(cwd)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	writeJSON(w, map[string]any{"id": t.id, "cwd": t.cwd})
}

func (s *Server) handleTermClose(w http.ResponseWriter, r *http.Request) {
	s.terms.close(r.PathValue("id"))
	w.WriteHeader(http.StatusNoContent)
}

// handleTermWS attaches the browser to a terminal's byte stream.
func (s *Server) handleTermWS(w http.ResponseWriter, r *http.Request) {
	t := s.terms.get(r.PathValue("id"))
	if t == nil {
		http.Error(w, "no such terminal", http.StatusNotFound)
		return
	}
	c, err := websocket.Accept(w, r, s.wsAccept(r))
	if err != nil {
		return
	}
	defer c.Close(websocket.StatusInternalError, "gateway teardown")
	// An idle shell (user reading output, long-running quiet command) must
	// not be reaped by intermediaries; attach's read loop answers the pongs.
	go keepAlive(r.Context(), c)
	t.attach(r, c)
	c.Close(websocket.StatusNormalClosure, "")
}
