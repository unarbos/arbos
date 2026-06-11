// The gateway's file door: the two routes the surface panels read through.
// /api/file returns metadata + text content for the code/doc viewers (and a
// cheap stat for change polling); /raw serves bytes with a real content-type
// so a canvas iframe (and its relative assets) loads like a normal page.
// Paths resolve exactly the way the tools resolve them (tool.Resolve against
// the workspace root — a starting point, not a wall), so what show presented
// is what the panel fetches.

package gateway

import (
	"net/http"
	"os"
	"unicode/utf8"

	"github.com/unarbos/arbos/internal/tool"
)

// fileMaxBytes caps /api/file content so a panel never pulls a giant blob
// into the page; the viewer shows the head and says so.
const fileMaxBytes = 2 << 20

// resolveFile turns a panel's path reference back into the file on disk.
// References are workspace-relative when the file lives under the root
// (show's normalization); URL cleaning strips the leading slash off an
// absolute reference arriving via /raw, so a miss retries it as absolute.
func (s *Server) resolveFile(p string) (string, os.FileInfo, error) {
	abs, err := tool.Resolve(s.Root, p)
	if err != nil {
		return "", nil, err
	}
	info, err := os.Stat(abs)
	if err != nil && len(p) > 0 && p[0] != '/' {
		if altInfo, altErr := os.Stat("/" + p); altErr == nil {
			return "/" + p, altInfo, nil
		}
	}
	return abs, info, err
}

// handleFile is the surface viewers' read: metadata always, content unless
// ?stat=1 (the change-poll that keeps an open panel live without refetching).
func (s *Server) handleFile(w http.ResponseWriter, r *http.Request) {
	p := r.URL.Query().Get("path")
	if p == "" {
		http.Error(w, "path is required", http.StatusBadRequest)
		return
	}
	abs, info, err := s.resolveFile(p)
	if err != nil {
		http.Error(w, err.Error(), http.StatusNotFound)
		return
	}
	if info.IsDir() {
		http.Error(w, "not a file", http.StatusBadRequest)
		return
	}
	out := map[string]any{
		"path":  p,
		"mtime": info.ModTime().UnixMilli(),
		"size":  info.Size(),
	}
	if r.URL.Query().Get("stat") == "" {
		b, err := os.ReadFile(abs)
		if err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
		if len(b) > fileMaxBytes {
			b = b[:fileMaxBytes]
			out["truncated"] = true
		}
		if !utf8.Valid(b) {
			out["binary"] = true
		} else {
			out["content"] = string(b)
		}
	}
	writeJSON(w, out)
}

// handleRaw serves a file as itself — content-type, ranges, caching all from
// net/http — so a canvas iframe renders the artifact exactly as a browser
// would, and relative asset references inside it resolve under /raw too.
func (s *Server) handleRaw(w http.ResponseWriter, r *http.Request) {
	abs, info, err := s.resolveFile(r.PathValue("path"))
	if err != nil || info.IsDir() {
		http.NotFound(w, r)
		return
	}
	// Artifacts change underfoot (the agent re-writes an open canvas); the
	// panel cache-busts with a query param, so don't let a stale 304 win.
	w.Header().Set("Cache-Control", "no-store")
	http.ServeFile(w, r, abs)
}
