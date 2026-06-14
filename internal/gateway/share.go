// Scoped share links (ADR-0034): a bearer-token grant to one artifact, served
// under /s/<token>. The token in the URL IS the credential, so these routes
// bypass the cookie gate (see Auth.wrap) and verify the grant here instead.
// Minting (/api/share/link) stays cookie-gated and same-origin — only the
// logged-in operator hands out a link; redeeming one needs only the secret.
//
// v1 scope is read-only file artifacts — canvases, images, PDFs, docs, code
// (ScopeFile) — plus the read endpoint for a chat (ScopeSession). Board,
// all-node, write, and admin/delegation grants are later phases; the mint
// endpoint rejects them so a link is never issued the redemption seam can't
// honor.

package gateway

import (
	"encoding/json"
	"html"
	"io"
	"net/http"
	"os"
	"path"
	"regexp"
	"strings"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/share"
)

// parseSharePerm maps the wire permission word to a share.Perm. The empty
// string defaults to read — the safe floor. ok is false for an unknown word.
func parseSharePerm(s string) (share.Perm, bool) {
	switch s {
	case "", "read":
		return share.PermRead, true
	case "write":
		return share.PermWrite, true
	case "admin":
		return share.PermAdmin, true
	default:
		return share.PermRead, false
	}
}

// permWord is the inverse of parseSharePerm, for the info endpoint the share
// view reads to decide whether to show a composer.
func permWord(p share.Perm) string {
	switch p {
	case share.PermWrite:
		return "write"
	case share.PermAdmin:
		return "admin"
	default:
		return "read"
	}
}

// shareMaxTTL caps a link's life. A share link is a standing exposure; an
// unbounded one is a forgotten open door, so even "no expiry" tops out here
// until the management/revocation UI (a later phase) makes long-lived links
// safe to track.
const shareMaxTTL = 30 * 24 * time.Hour

// handleShareLink mints a scoped grant and returns its /s/<token> URL. The
// scope's referent is validated to exist before a link is issued, so a link
// never points at a path outside the workspace or a session that isn't there.
func (s *Server) handleShareLink(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Scope struct {
			Kind string `json:"kind"`
			Ref  string `json:"ref"`
		} `json:"scope"`
		Perm       string `json:"perm"`        // "read" (default) | "write" | "admin"
		TTLSeconds int64  `json:"ttl_seconds"` // 0 = no expiry (capped at shareMaxTTL)
	}
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, jsonBodyMax)).Decode(&req); err != nil {
		http.Error(w, "bad request", http.StatusBadRequest)
		return
	}
	kind := share.ScopeKind(req.Scope.Kind)
	if kind != share.ScopeFile && kind != share.ScopeSession && kind != share.ScopeAll {
		http.Error(w, "unsupported scope (file, session, or all)", http.StatusBadRequest)
		return
	}
	perm, ok := parseSharePerm(req.Perm)
	if !ok {
		http.Error(w, "unknown perm", http.StatusBadRequest)
		return
	}
	// Only the cells the redemption seam actually enforces are mintable, so a
	// link never promises more than it delivers: a file artifact is read-only
	// (no edit path yet); a chat is read or write (talk); the full agent is
	// admin only — a full-agent link is login-equivalent, and a read-only
	// whole-app view is not yet built. The referent must exist.
	switch kind {
	case share.ScopeFile:
		if perm != share.PermRead {
			http.Error(w, "file links are read-only for now", http.StatusBadRequest)
			return
		}
		if _, info, err := s.resolveFile(req.Scope.Ref); err != nil || info.IsDir() {
			http.Error(w, "no such file", http.StatusNotFound)
			return
		}
	case share.ScopeSession:
		if perm > share.PermWrite {
			http.Error(w, "admin chat links are not available yet", http.StatusBadRequest)
			return
		}
		if _, err := s.Store.Get(r.Context(), core.SessionID(req.Scope.Ref)); err != nil {
			http.Error(w, "no such session", http.StatusNotFound)
			return
		}
	case share.ScopeAll:
		if perm != share.PermAdmin {
			http.Error(w, "a full-agent link must be full access", http.StatusBadRequest)
			return
		}
		req.Scope.Ref = "" // a node-wide grant has no referent
	}
	tok := randomToken()
	if tok == "" {
		http.Error(w, "could not mint a token", http.StatusInternalServerError)
		return
	}
	// Every link gets a real expiry: a requested TTL is capped at shareMaxTTL,
	// and "no expiry" (TTLSeconds == 0) becomes the cap rather than forever —
	// an unrevocable permanent link is the forgotten open door, and there is
	// no revocation/management UI yet to track one (see shareMaxTTL).
	ttl := shareMaxTTL
	if req.TTLSeconds > 0 {
		if d := time.Duration(req.TTLSeconds) * time.Second; d < shareMaxTTL {
			ttl = d
		}
	}
	expires := time.Now().Add(ttl)
	g := share.Grant{
		Token:   tok,
		Scope:   share.Scope{Kind: kind, Ref: req.Scope.Ref},
		Perm:    perm,
		Expires: expires,
		Created: time.Now(),
	}
	if err := s.Store.PutGrant(r.Context(), g); err != nil {
		http.Error(w, "could not store grant", http.StatusInternalServerError)
		return
	}
	writeJSON(w, map[string]any{"url": shareScheme(r) + "://" + r.Host + sharePath + tok})
}

// handleShareRevoke kills a link by token — and, via RevokeGrant's cascade,
// every grant delegated from it. Cookie-gated and same-origin like minting:
// only the logged-in operator revokes. Revoking an unknown token is a no-op
// success, so a double-click or a stale management view doesn't error.
func (s *Server) handleShareRevoke(w http.ResponseWriter, r *http.Request) {
	if err := s.Store.RevokeGrant(r.Context(), r.PathValue("token")); err != nil {
		http.Error(w, "could not revoke", http.StatusInternalServerError)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

// shareGrant resolves a token to a live grant, or false for anything a holder
// can't tell apart: absent, revoked, or expired.
func (s *Server) shareGrant(r *http.Request, token string) (share.Grant, bool) {
	g, err := s.Store.Grant(r.Context(), token)
	if err != nil || !g.Live(time.Now()) {
		return share.Grant{}, false
	}
	return g, true
}

// handleShareView is the landing for a link: it renders the shared artifact by
// kind. A canvas (or any file artifact) is served standalone; a chat boots the
// SPA, which enters read-only share mode off the /s/<token> path.
func (s *Server) handleShareView(w http.ResponseWriter, r *http.Request) {
	g, ok := s.shareGrant(r, r.PathValue("token"))
	if !ok {
		shareGone(w)
		return
	}
	switch g.Scope.Kind {
	case share.ScopeFile:
		s.serveSharedFile(w, r, g)
	case share.ScopeSession, share.ScopeAll:
		// Both redeem the same way: set a scoped session cookie and drop the
		// holder into the real app. The scope guard then enforces what that
		// cookie may reach — the whole workspace for ScopeAll, just the
		// granted chat for ScopeSession. One redemption shape, no parallel API.
		s.setShareCookie(w, r, g)
	default:
		shareGone(w)
	}
}

// sandboxArtifact forces a served artifact into an opaque origin, the same
// boundary the authed canvas viewer gets from its iframe sandbox: scripts in
// agent-authored HTML (or a scriptable SVG) still run, but the document is no
// longer same-origin with the node, so a /api/* fetch from it is cross-origin
// and credential-less — sameOrigin and the missing cookie both refuse it.
// nosniff stops a mislabeled artifact from being reinterpreted as a document.
// Set on every artifact response (the view and its raw assets) so a directly
// navigated asset can't escape the sandbox either.
func sandboxArtifact(w http.ResponseWriter) {
	w.Header().Set("Content-Security-Policy", "sandbox allow-scripts")
	w.Header().Set("X-Content-Type-Options", "nosniff")
}

// headOpenRe matches the opening <head> tag (any attributes) so a <base> can
// be injected right after it — the same place the panel's themedCanvasDoc puts
// it — so a canvas's relative assets resolve through the scoped raw route.
var headOpenRe = regexp.MustCompile(`(?i)<head[^>]*>`)

// serveSharedFile serves a file artifact. HTML (a canvas) gets a <base>
// injected so its relative assets load through /s/<token>/raw/; every other
// type is served as itself so the browser renders it natively (image, PDF) or
// shows it as text (markdown, code).
func (s *Server) serveSharedFile(w http.ResponseWriter, r *http.Request, g share.Grant) {
	abs, info, err := s.resolveFile(g.Scope.Ref)
	if err != nil || info.IsDir() {
		shareGone(w)
		return
	}
	w.Header().Set("Cache-Control", "no-store")
	sandboxArtifact(w)
	if ext := strings.ToLower(path.Ext(g.Scope.Ref)); ext != ".html" && ext != ".htm" {
		http.ServeFile(w, r, abs)
		return
	}
	b, err := os.ReadFile(abs)
	if err != nil {
		shareGone(w)
		return
	}
	dir := path.Dir(g.Scope.Ref)
	if dir == "." {
		dir = ""
	} else {
		dir += "/"
	}
	// dir comes from the operator-set scope ref; escape it so a path with a
	// quote can't break out of the href attribute.
	base := `<base href="` + html.EscapeString(sharePath+r.PathValue("token")+"/raw/"+dir) + `">`
	doc := string(b)
	if loc := headOpenRe.FindStringIndex(doc); loc != nil {
		doc = doc[:loc[1]] + base + doc[loc[1]:]
	} else {
		doc = base + doc
	}
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	_, _ = io.WriteString(w, doc)
}

// handleShareRaw serves a canvas's relative assets, scoped to the canvas's own
// directory — the grant authorizes that subtree and nothing above it.
func (s *Server) handleShareRaw(w http.ResponseWriter, r *http.Request) {
	g, ok := s.shareGrant(r, r.PathValue("token"))
	if !ok || g.Scope.Kind != share.ScopeFile {
		http.NotFound(w, r)
		return
	}
	req := r.PathValue("path")
	if !withinShareDir(g.Scope.Ref, req) {
		http.NotFound(w, r)
		return
	}
	abs, info, err := s.resolveFile(req)
	if err != nil || info.IsDir() {
		http.NotFound(w, r)
		return
	}
	w.Header().Set("Cache-Control", "no-store")
	sandboxArtifact(w)
	http.ServeFile(w, r, abs)
}

// withinShareDir reports whether req is reachable under a canvas's directory.
// A canvas at the workspace root (dir ".") scopes to the file itself only —
// never the whole root — so a root-level canvas link can't be walked into a
// workspace file browser.
func withinShareDir(canvasRef, req string) bool {
	clean := path.Clean(req)
	if clean == ".." || strings.HasPrefix(clean, "../") {
		return false
	}
	dir := path.Dir(canvasRef)
	if dir == "." {
		return clean == path.Clean(canvasRef)
	}
	return clean == dir || strings.HasPrefix(clean, dir+"/")
}

// shareScheme reports the public scheme for building a link URL: https when
// the browser's leg is TLS, directly or via a relay that terminates it.
func shareScheme(r *http.Request) string {
	if r.TLS != nil || r.Header.Get("X-Forwarded-Proto") == "https" {
		return "https"
	}
	return "http"
}

// shareGone is the uniform dead-link response: a revoked, expired, or unknown
// token all look the same to a holder, with nothing to probe.
func shareGone(w http.ResponseWriter) {
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	w.WriteHeader(http.StatusNotFound)
	_, _ = io.WriteString(w, shareGonePage)
}

const shareGonePage = loginPageHead + `<div style="text-align:center">
  <h1>arbos</h1>
  <p>This link is no longer available — it may have expired or been revoked.</p>
</div>`
