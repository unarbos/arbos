// The scope guard: a share link redeems into a scoped *session cookie*, and the
// real SPA + real endpoints run behind it — same UI, narrower permission — so
// there is no parallel share API to drift. The guard is deny-by-default: a
// session-scoped principal may reach only the handful of routes the granted
// chat needs (the seam, frame-filtered to that one session; that session's
// replay; the composer's catalogs; the SPA shell). A full-agent (ScopeAll)
// principal is login-equivalent and passes through. Everything else is 403.
//
// This file holds the guard, the /api/me capabilities probe the SPA reads to
// enter share mode, and the seam frame-filter that locks a guest to one
// session and gates writes by permission. See ADR-0034.

package gateway

import (
	"context"
	"encoding/json"
	"net/http"
	"path"
	"strings"
	"time"

	"github.com/unarbos/arbos/internal/share"
)

// shareCtxKey carries the resolved scoped grant from the guard down to the
// handlers that scope themselves further (the seam frame-filter).
type shareCtxKey struct{}

func shareFromContext(ctx context.Context) (share.Grant, bool) {
	g, ok := ctx.Value(shareCtxKey{}).(share.Grant)
	return g, ok
}

// scopeGuard wraps the whole authed handler. It is a passthrough for every
// principal except a scoped share session: a full login ("local"), a loopback
// caller, and a full-agent share (ScopeAll, login-equivalent) all flow through
// untouched. A session-scoped share cookie is held to a deny-by-default
// allowlist, and the grant is stashed for the seam to scope.
func (s *Server) scopeGuard(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		sub, ok := s.Auth.principal(r)
		if !ok || !strings.HasPrefix(sub, sharePrincipal) {
			next.ServeHTTP(w, r)
			return
		}
		// Redemption and login routes always pass, even while a scoped cookie
		// is held: they (re)establish the credential — reloading a link, or
		// opening a different one — and would otherwise be trapped by the very
		// scope they're trying to replace.
		p := path.Clean(r.URL.Path)
		if strings.HasPrefix(p, sharePath) || p == loginPath {
			next.ServeHTTP(w, r)
			return
		}
		g, live := s.shareGrant(r, strings.TrimPrefix(sub, sharePrincipal))
		if !live {
			// The share cookie outlived its grant (expired or revoked): clear
			// it and send the holder to the dead-link page.
			http.SetCookie(w, &http.Cookie{Name: authCookie, Path: "/", MaxAge: -1})
			shareGone(w)
			return
		}
		switch g.Scope.Kind {
		case share.ScopeAll:
			next.ServeHTTP(w, r) // full access — login-equivalent
		case share.ScopeSession:
			if !sessionScopeAllows(r, g.Scope.Ref, g.Perm) {
				http.Error(w, "not permitted for this share link", http.StatusForbidden)
				return
			}
			next.ServeHTTP(w, r.WithContext(context.WithValue(r.Context(), shareCtxKey{}, g)))
		default:
			http.Error(w, "not permitted for this share link", http.StatusForbidden)
		}
	})
}

// sessionScopeAllows is the deny-by-default allowlist for a chat share. It
// permits exactly what the real ChatTab needs to render and (when write) drive
// the one granted session — the SPA shell, the capabilities probe, the seam
// (scoped by the frame-filter), the composer's read-only catalogs, and that
// session's own replay — and refuses everything else (other sessions, the
// session list, secrets, settings, terminals, jobs, files, the host mic).
func sessionScopeAllows(r *http.Request, ref string, perm share.Perm) bool {
	p := path.Clean(r.URL.Path)
	get := r.Method == http.MethodGet
	switch {
	case p == "/" || p == "/index.html" || strings.HasPrefix(p, "/assets/"):
		return true
	case p == "/api/me":
		return true
	case p == "/api/ws": // the seam — frame-filtered to ref in handleWS
		return true
	case p == "/api/models" || p == "/api/commands":
		return get
	case p == "/api/sessions/"+ref+"/events" || p == "/api/sessions/"+ref+"/children":
		return get
	default:
		return false
	}
}

// handleMe reports the current principal's capabilities so the SPA can enter
// share mode: a full login (or full-agent share) sees the whole workspace; a
// session-scoped share is dropped onto just that chat, read-only or writable.
func (s *Server) handleMe(w http.ResponseWriter, r *http.Request) {
	out := map[string]any{"kind": "local"}
	if sub, ok := s.Auth.principal(r); ok && strings.HasPrefix(sub, sharePrincipal) {
		if g, live := s.shareGrant(r, strings.TrimPrefix(sub, sharePrincipal)); live && g.Scope.Kind == share.ScopeSession {
			out = map[string]any{
				"kind":    "share",
				"scope":   string(g.Scope.Kind),
				"session": g.Scope.Ref,
				"perm":    permWord(g.Perm),
			}
		}
	}
	writeJSON(w, out)
}

// setShareCookie drops a scoped session cookie for a grant and sends the
// holder into the real app. The cookie's life is the grant's remaining life
// (capped at the session TTL); the full token rides in the subject so the
// guard can resolve scope and perm on every request.
func (s *Server) setShareCookie(w http.ResponseWriter, r *http.Request, g share.Grant) {
	ttl := cookieTTL
	if !g.Expires.IsZero() {
		if d := time.Until(g.Expires); d < ttl {
			ttl = d
		}
	}
	if ttl <= 0 {
		shareGone(w)
		return
	}
	secure := r.TLS != nil || r.Header.Get("X-Forwarded-Proto") == "https"
	http.SetCookie(w, &http.Cookie{
		Name:     authCookie,
		Value:    s.Auth.signCookie(cookiePayload{Sub: sharePrincipal + g.Token, Exp: time.Now().Add(ttl).Unix()}),
		Path:     "/",
		MaxAge:   int(ttl / time.Second),
		HttpOnly: true,
		SameSite: http.SameSiteLaxMode,
		Secure:   secure,
	})
	http.Redirect(w, r, "/", http.StatusSeeOther)
}

// filterShareFrame scopes one inbound seam frame for a session-scoped guest.
// It returns the frame to forward (rewritten) and whether to forward it: an
// "open" is forced onto the granted session; prompts/steers/interrupts pass
// only with write permission; everything else (switch, fork, set_model, other
// sessions) is dropped, so a guest can neither drive another session nor
// restructure this one.
func filterShareFrame(line []byte, ref string, perm share.Perm) ([]byte, bool) {
	var f struct {
		Type      string          `json:"type"`
		SessionID string          `json:"session_id,omitempty"`
		Intent    json.RawMessage `json:"intent,omitempty"`
	}
	if json.Unmarshal(line, &f) != nil {
		return nil, false
	}
	switch f.Type {
	case "open":
		// Always bind the granted session, whatever the client asked for.
		out, _ := json.Marshal(map[string]string{"type": "open", "session_id": ref})
		return out, true
	case "intent":
		if perm < share.PermWrite {
			return nil, false
		}
		var kind struct {
			Kind string `json:"kind"`
		}
		if json.Unmarshal(f.Intent, &kind) != nil {
			return nil, false
		}
		// Only conversational intents — never session-structural ones
		// (set_model, fork, compact) which would let a guest reshape the chat.
		switch kind.Kind {
		case "prompt", "steer", "interrupt", "approval_response", "question_response":
			return line, true
		default:
			return nil, false
		}
	default:
		// switch_session, fork, set_model, set_web_*, compact: not for a guest.
		return nil, false
	}
}
