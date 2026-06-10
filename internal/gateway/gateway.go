// Package gateway is the browser door onto the kernel: an HTTP server that
// carries the control seam over WebSocket (one connection = one control.Serve,
// frame-per-message) and serves the built web UI. It re-implements no turn
// logic — the browser speaks the exact protocol every other frontend speaks.
package gateway

import (
	"io/fs"
	"net/http"
	"strings"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
)

// Server hosts the web frontend's surface: the live seam and the static SPA.
type Server struct {
	Engine       *engine.Engine
	NewSessionID func() core.SessionID
	Drain        time.Duration // in-flight turn drain on disconnect; zero = control default
	Dist         fs.FS         // built SPA to serve at /; nil = API only
}

// Handler builds the route table.
func (s *Server) Handler() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /api/ws", s.handleWS)
	if s.Dist != nil {
		mux.Handle("/", spaHandler(s.Dist))
	}
	return mux
}

// spaHandler serves the built SPA: real files as-is, everything else (client
// routes) falls back to index.html.
func spaHandler(dist fs.FS) http.Handler {
	fileServer := http.FileServerFS(dist)
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		p := strings.TrimPrefix(r.URL.Path, "/")
		if p != "" {
			if f, err := dist.Open(p); err == nil {
				_ = f.Close()
				fileServer.ServeHTTP(w, r)
				return
			}
		}
		r2 := r.Clone(r.Context())
		r2.URL.Path = "/"
		fileServer.ServeHTTP(w, r2)
	})
}
