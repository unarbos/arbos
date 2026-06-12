// The gateway's file door: the two routes the surface panels read through.
// /api/file returns metadata + text content for the code/doc viewers (and a
// cheap stat for change polling); /raw serves bytes with a real content-type
// so a canvas iframe (and its relative assets) loads like a normal page.
// Paths resolve exactly the way the tools resolve them (tool.Resolve against
// the workspace root — a starting point, not a wall), so what show presented
// is what the panel fetches.

package gateway

import (
	"encoding/base64"
	"encoding/json"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"unicode/utf8"

	"github.com/unarbos/arbos/internal/tool"
)

// fileMaxBytes caps /api/file content so a panel never pulls a giant blob
// into the page; the viewer shows the head and says so.
const fileMaxBytes = 2 << 20

// dirMaxEntries caps a directory listing (node_modules exists); the browser
// shows the head and says so.
const dirMaxEntries = 1000

// fileWriteMaxBody bounds a PUT /api/file request body so a hostile (but
// authenticated) caller can't OOM the host with a multi-GB upload. It sits
// above the composer's 24 MiB attachment cap (ADR-0022) inflated by base64
// (~33%) plus JSON framing, so every legitimate spooled attachment still
// fits while an unbounded body is refused.
const fileWriteMaxBody = 48 << 20

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
	out := map[string]any{
		"path":  p,
		"mtime": info.ModTime().UnixMilli(),
		"size":  info.Size(),
	}
	if info.IsDir() {
		// A directory answers on the same route with the same stat shape, so
		// the panel's change-poll (mtime — bumped by entry add/remove) works
		// for a browser tab exactly as for a file. Content is the listing.
		out["dir"] = true
		if r.URL.Query().Get("stat") == "" {
			entries, truncated, err := listDir(abs)
			if err != nil {
				http.Error(w, err.Error(), http.StatusInternalServerError)
				return
			}
			out["entries"] = entries
			if truncated {
				out["truncated"] = true
			}
		}
		writeJSON(w, out)
		return
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

// dirEntryJSON is one row of a directory listing.
type dirEntryJSON struct {
	Name  string `json:"name"`
	Dir   bool   `json:"dir,omitempty"`
	Size  int64  `json:"size,omitempty"`
	Mtime int64  `json:"mtime,omitempty"` // unix milliseconds
}

// listDir reads a directory for the browser panel: directories first, then
// files, each group case-insensitively by name, capped at dirMaxEntries.
func listDir(abs string) ([]dirEntryJSON, bool, error) {
	ents, err := os.ReadDir(abs)
	if err != nil {
		return nil, false, err
	}
	out := make([]dirEntryJSON, 0, len(ents))
	for _, e := range ents {
		row := dirEntryJSON{Name: e.Name(), Dir: e.IsDir()}
		if fi, err := e.Info(); err == nil {
			row.Mtime = fi.ModTime().UnixMilli()
			if !row.Dir {
				row.Size = fi.Size()
			}
		}
		out = append(out, row)
	}
	sort.SliceStable(out, func(i, j int) bool {
		if out[i].Dir != out[j].Dir {
			return out[i].Dir
		}
		return strings.ToLower(out[i].Name) < strings.ToLower(out[j].Name)
	})
	if len(out) > dirMaxEntries {
		return out[:dirMaxEntries], true, nil
	}
	return out, false, nil
}

// handleFileWrite is the prompt editor's save: it writes a text file back
// through the same resolution the read door uses, creating parent directories
// so a brand-new prompt (".arbos/prompts/name.md") lands without ceremony.
// The response is the fresh stat, so the editor's change-poll baseline is the
// write it just made — its own save never reads back as an external change.
func (s *Server) handleFileWrite(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Path    string `json:"path"`
		Content string `json:"content"`
		// Encoding "base64" decodes Content into raw bytes before writing, so a
		// binary attachment (e.g. a PDF spooled from the composer) lands intact
		// rather than as mangled UTF-8. Empty means the content is written as-is.
		Encoding string `json:"encoding,omitempty"`
	}
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, fileWriteMaxBody)).Decode(&body); err != nil || body.Path == "" {
		http.Error(w, "path and content are required", http.StatusBadRequest)
		return
	}
	data := []byte(body.Content)
	if body.Encoding == "base64" {
		decoded, err := base64.StdEncoding.DecodeString(body.Content)
		if err != nil {
			http.Error(w, "invalid base64 content", http.StatusBadRequest)
			return
		}
		data = decoded
	}
	abs, err := tool.Resolve(s.Root, body.Path)
	if err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	if err := os.MkdirAll(filepath.Dir(abs), 0o755); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	if err := os.WriteFile(abs, data, 0o644); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	info, err := os.Stat(abs)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	writeJSON(w, map[string]any{
		"path":  body.Path,
		"mtime": info.ModTime().UnixMilli(),
		"size":  info.Size(),
	})
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
