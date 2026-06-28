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
	if kind != share.ScopeFile && kind != share.ScopeSession && kind != share.ScopeAll && kind != share.ScopeTrajectory {
		http.Error(w, "unsupported scope (file, session, trajectory, or all)", http.StatusBadRequest)
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
	case share.ScopeTrajectory:
		// A trajectory is a read-only reproduction — there is nothing to write
		// back to a snapshot — so it only mints at PermRead. The referent must
		// be a real session.
		if perm != share.PermRead {
			http.Error(w, "trajectory links are read-only", http.StatusBadRequest)
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
	default:
		// The kind guard above already rejects anything unmintable (e.g.
		// ScopeBoard, a later phase); the explicit default makes a future
		// ScopeKind force a mint decision here rather than fall through and
		// silently mint a grant the redemption seam can't honor.
		http.Error(w, "unsupported scope", http.StatusBadRequest)
		return
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
	token := r.PathValue("token")
	// Kick the guests this link seated in their rooms, so revocation removes
	// them from the Matrix membership too — not just the bespoke seam (which
	// the live grant check denies the moment the grant is gone). Best-effort and
	// before the grant is dropped, so the membership and the grant retire
	// together.
	if s.Matrix != nil {
		for _, seat := range s.takeSeats(r.Context(), token) {
			_ = s.Matrix.RemoveGuest(r.Context(), core.SessionID(seat.Session), seat.Author)
		}
	}
	if err := s.Store.RevokeGrant(r.Context(), token); err != nil {
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
	case share.ScopeTrajectory:
		// Served standalone like a file artifact: a self-contained snapshot
		// page, sandboxed, with no scoped cookie and no entry into the SPA.
		s.serveTrajectory(w, r, g)
	case share.ScopeSession:
		// A chat invite: the guest picks a self-asserted display name once, then
		// lands in the SPA. The name rides in the signed scoped cookie (the
		// trusted source for the author stamped onto their messages). A GET form
		// posting back to the same /s/<token> keeps redemption a plain
		// navigation — the path token is the credential, so no CSRF token needed
		// (same rationale as the /login confirm page).
		q := r.URL.Query()
		if !q.Has("name") {
			s.writeNameForm(w, g.Token)
			return
		}
		name := sanitizeGuestName(q.Get("name"))
		if name == "" {
			name = "Guest"
		}
		// Matrix-native sharing (ADR-0041 Step 4): seat this guest in the
		// session's room as their own identity, so room membership reflects the
		// share and their messages mirror under their own seat — not the host's.
		// Best-effort and additive: the scoped seam below carries the guest
		// regardless, and a node without the embedded homeserver skips this.
		//
		// Write-only: a seat is room membership, and the embedded room grants
		// members the default send power, so a seated guest can post directly
		// via the Matrix client (and drive the agent). That is exactly the
		// write capability a read-only share must withhold — so a read-only
		// guest is never seated and falls back to the seam, where the perm gate
		// enforces observe-only. Without this gate the Matrix path silently
		// escalates every read-only share to writable (ADR-0041 D5/D6).
		if s.Matrix != nil && g.Perm >= share.PermWrite {
			if _, err := s.Matrix.InviteGuest(r.Context(), core.SessionID(g.Scope.Ref), name); err == nil {
				// Remember the seat so revoking this link kicks them (membership
				// tracks the grant lifecycle). A seating failure must never block
				// joining — the bespoke seam is the floor.
				s.recordSeat(r.Context(), g.Token, g.Scope.Ref, name)
			}
		}
		s.setShareCookie(w, r, g, name)
	case share.ScopeAll:
		// Login-equivalent (a second device for the operator, not a guest): set a
		// scoped session cookie with no display name and drop the holder into the
		// real app. The scope guard enforces what that cookie may reach.
		s.setShareCookie(w, r, g, "")
	default:
		shareGone(w)
	}
}

// sanitizeGuestName normalizes a self-asserted display name: trims, strips
// control chars and newlines (so the cookie JSON stays clean and the "Name:
// <text>" projection line can't be split), and caps length. Returns "" for an
// empty/whitespace name, which the caller maps to "Guest".
func sanitizeGuestName(s string) string {
	s = strings.Map(func(r rune) rune {
		if r < 0x20 || r == 0x7f {
			return -1
		}
		return r
	}, s)
	s = strings.TrimSpace(s)
	if rs := []rune(s); len(rs) > 32 {
		s = strings.TrimSpace(string(rs[:32]))
	}
	return s
}

// writeNameForm renders the one-field name-entry page a chat-invite guest sees
// before redemption. It reuses the login page chrome and posts (via GET) back to
// the same /s/<token> with the chosen name.
func (s *Server) writeNameForm(w http.ResponseWriter, token string) {
	body := `<form method="GET" action="/s/` + html.EscapeString(token) + `">
  <h1>arbos</h1>
  <p>You’ve been invited to a chat. Pick a name the others will see.</p>
  <input name="name" placeholder="your name" autofocus autocomplete="off" maxlength="32">
  <button>Join chat</button>
</form>`
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	w.Header().Set("Cache-Control", "no-store")
	_, _ = w.Write([]byte(loginPageHead + body))
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

// anchorScrollScript guarantees in-page anchors work on a shared artifact. A
// shared canvas/blog is served into an opaque (null) origin (sandboxArtifact),
// where a bare href="#id" resolves against the document URL and 404s rather
// than scrolling — breaking a blog's [n] citation links and their ↩ back-links.
// We inject a delegated click listener that intercepts in-page hash links and
// scrolls within the document. Mirrors the panel's ANCHOR_SCROLL_SCRIPT
// (web/src/lib/surface.ts), so the doc panel and the public share page behave
// identically and neither depends on the artifact shipping its own script.
const anchorScrollScript = `<script data-arbos-anchor-scroll>` +
	`document.addEventListener("click",function(e){` +
	`var a=e.target.closest&&e.target.closest('a[href^="#"]');if(!a)return;` +
	`var id=decodeURIComponent(a.getAttribute("href").slice(1));if(!id)return;` +
	`var t=document.getElementById(id);if(t){e.preventDefault();` +
	`t.scrollIntoView({behavior:"smooth",block:"center"});` +
	`if(t.id)try{history.replaceState(null,"","#"+t.id)}catch(_){}}` +
	`});</script>`

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
	ext := strings.ToLower(path.Ext(g.Scope.Ref))
	// A markdown document renders to a self-contained reading page (the /blog
	// report's home) with clickable [n] citations, rather than shipping raw
	// source — a file share is standalone and sandboxed, so the render runs
	// here in Go (see sharedoc.go) instead of mounting the SPA's renderer.
	if isMarkdownExt(ext) {
		if b, err := os.ReadFile(abs); err == nil {
			w.Header().Set("Content-Type", "text/html; charset=utf-8")
			_, _ = io.WriteString(w, renderSharedDoc(path.Base(g.Scope.Ref), string(b)))
			return
		}
		shareGone(w)
		return
	}
	if ext != ".html" && ext != ".htm" {
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
	// Make in-page anchors (cite links, back-links) scroll under the null
	// origin — appended last so it runs regardless of the artifact's own JS.
	doc += anchorScrollScript
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
