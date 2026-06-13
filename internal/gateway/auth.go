package gateway

import (
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"html"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"
)

// Auth gates every gateway route when the bind address is reachable beyond
// loopback (ADR-0034 P1). The gateway exposes file write, PTY spawn, and
// secrets CRUD, so a remotely reachable bind means auth is on — not optional.
//
// The credential model is deliberately minimal: a one-time login token printed
// to the host's stderr (the Jupyter pattern) exchanges for an HMAC-signed
// session cookie. Consuming the console token mints and prints a fresh one, so
// the operator's console always holds a current login link for the next
// browser. Share tokens (the UI's share button) are minted alongside, each
// independently single-use with a TTL, so handing out an invite never kills
// the console link — and vice versa. Requests arriving over loopback bypass
// auth entirely — a local process can already read the signing key off disk,
// so gating it buys nothing and would break the localhost-out-of-the-box
// promise.
type Auth struct {
	// Key signs session cookies (HMAC-SHA256). Loaded once per host via
	// LoadOrCreateAuthKey so cookies survive restarts.
	Key []byte
	// OnToken receives each freshly minted console token; the host renders
	// it as a clickable login URL on stderr.
	OnToken func(token string)

	mu    sync.Mutex
	token string
	// share holds outstanding share-link tokens → expiry. In-memory on
	// purpose: a restart invalidates old invites, and the login page tells
	// the holder so (no silent deaths).
	share map[string]time.Time
}

const (
	authCookie = "arbos_auth"
	loginPath  = "/login"
	cookieTTL  = 30 * 24 * time.Hour
	// shareTokenTTL bounds an unused invite: long enough to send tonight and
	// click tomorrow, short enough that a forgotten link dies on its own.
	shareTokenTTL = 24 * time.Hour
	// maxShareTokens caps outstanding invites; minting past it evicts the
	// soonest-to-expire. Nobody shares with 32 people between logins.
	maxShareTokens = 32
)

// LoadOrCreateAuthKey returns the host's cookie-signing key, generating a
// 32-byte one on first use (0600, dir 0700). It lives next to the device
// identity (ADR-0033) because it is the same kind of thing: a durable local
// secret that *is* the machine's say-so.
func LoadOrCreateAuthKey(path string) ([]byte, error) {
	if b, err := os.ReadFile(path); err == nil && len(b) >= 32 {
		return b, nil
	}
	key := make([]byte, 32)
	if _, err := rand.Read(key); err != nil {
		return nil, err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return nil, err
	}
	if err := os.WriteFile(path, key, 0o600); err != nil {
		return nil, err
	}
	return key, nil
}

// randomToken mints a 256-bit hex token; "" only if the system's entropy
// source fails.
func randomToken() string {
	b := make([]byte, 32)
	if _, err := rand.Read(b); err != nil {
		return ""
	}
	return hex.EncodeToString(b)
}

// MintToken rotates the console login token and announces it via OnToken.
// Called at startup and on every consumption of the console token, so the
// operator's console always holds a current link. Share tokens live apart
// (MintShareToken); rotating here never invalidates an outstanding invite.
func (a *Auth) MintToken() string {
	tok := randomToken()
	if tok == "" {
		return ""
	}
	a.mu.Lock()
	a.token = tok
	a.mu.Unlock()
	if a.OnToken != nil {
		a.OnToken(tok)
	}
	return tok
}

// MintShareToken mints an independent single-use invite token with a TTL.
// Independent is the point: handing out a share link must not kill the
// console link, and two invites in a row must both work.
func (a *Auth) MintShareToken() string {
	tok := randomToken()
	if tok == "" {
		return ""
	}
	a.mu.Lock()
	defer a.mu.Unlock()
	if a.share == nil {
		a.share = make(map[string]time.Time)
	}
	a.pruneShareLocked()
	for len(a.share) >= maxShareTokens {
		oldest, oldestExp := "", time.Time{}
		for t, exp := range a.share {
			if oldest == "" || exp.Before(oldestExp) {
				oldest, oldestExp = t, exp
			}
		}
		delete(a.share, oldest)
	}
	a.share[tok] = time.Now().Add(shareTokenTTL)
	return tok
}

// pruneShareLocked drops expired invites. Caller holds a.mu.
func (a *Auth) pruneShareLocked() {
	now := time.Now()
	for t, exp := range a.share {
		if now.After(exp) {
			delete(a.share, t)
		}
	}
}

// consumeToken redeems a one-time token — console or share. Single-use:
// success removes it, and a consumed console token mints a replacement so
// the console always shows a live link. A leaked URL (browser history, chat
// preview) is dead after first use.
func (a *Auth) consumeToken(tok string) bool {
	if tok == "" {
		return false
	}
	a.mu.Lock()
	console := a.token != "" && subtle.ConstantTimeCompare([]byte(tok), []byte(a.token)) == 1
	ok := console
	if console {
		a.token = ""
	} else {
		a.pruneShareLocked()
		for t := range a.share {
			if subtle.ConstantTimeCompare([]byte(tok), []byte(t)) == 1 {
				delete(a.share, t)
				ok = true
				break
			}
		}
	}
	a.mu.Unlock()
	if console {
		a.MintToken()
	}
	return ok
}

// peekToken reports whether tok is currently redeemable, without consuming
// it. This is what the GET login page checks: link previewers and unfurler
// bots GET, so a bare GET must never spend the token — only the human's
// confirming POST does.
func (a *Auth) peekToken(tok string) bool {
	if tok == "" {
		return false
	}
	a.mu.Lock()
	defer a.mu.Unlock()
	if a.token != "" && subtle.ConstantTimeCompare([]byte(tok), []byte(a.token)) == 1 {
		return true
	}
	a.pruneShareLocked()
	for t := range a.share {
		if subtle.ConstantTimeCompare([]byte(tok), []byte(t)) == 1 {
			return true
		}
	}
	return false
}

// cookiePayload is what a session cookie asserts: who, until when. The
// principal is "local" until backend-brokered identities (ADR-0034 P2) give
// us someone more specific to name.
type cookiePayload struct {
	Sub string `json:"sub"`
	Exp int64  `json:"exp"` // unix seconds
}

// signCookie mints a session cookie value: base64url(payload) + "." +
// base64url(HMAC over the encoded payload bytes — no canonicalization to get
// wrong).
func (a *Auth) signCookie(p cookiePayload) string {
	body, _ := json.Marshal(p)
	enc := base64.RawURLEncoding.EncodeToString(body)
	mac := hmac.New(sha256.New, a.Key)
	mac.Write([]byte(enc))
	return enc + "." + base64.RawURLEncoding.EncodeToString(mac.Sum(nil))
}

// verifyCookie checks signature then expiry. Any malformed input is just
// "not logged in" — there is nothing to report to an unauthenticated caller.
func (a *Auth) verifyCookie(val string) bool {
	enc, sigB64, ok := strings.Cut(val, ".")
	if !ok {
		return false
	}
	sig, err := base64.RawURLEncoding.DecodeString(sigB64)
	if err != nil {
		return false
	}
	mac := hmac.New(sha256.New, a.Key)
	mac.Write([]byte(enc))
	if !hmac.Equal(sig, mac.Sum(nil)) {
		return false
	}
	body, err := base64.RawURLEncoding.DecodeString(enc)
	if err != nil {
		return false
	}
	var p cookiePayload
	if json.Unmarshal(body, &p) != nil {
		return false
	}
	return time.Now().Unix() < p.Exp
}

// authenticated reports whether the request carries a valid session cookie.
func (a *Auth) authenticated(r *http.Request) bool {
	c, err := r.Cookie(authCookie)
	return err == nil && a.verifyCookie(c.Value)
}

// sharePath prefixes the bearer-token redemption routes (/s/<token>…). They
// carry their own credential — the token in the URL — so they pass the cookie
// gate; the share handlers verify the grant and enforce its scope themselves.
const sharePath = "/s/"

// wrap is the gate in front of the whole route table. Loopback callers pass
// (they own the box), /login passes (it is the way in), share redemption and
// the static SPA bundle pass (the token is the credential, and the compiled
// frontend is not the secret — the data endpoints behind it stay gated), and
// everything else needs the cookie. API and raw paths get a bare 401 so
// fetch() fails loudly; document navigations get redirected to the login page.
func (a *Auth) wrap(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case r.URL.Path == loginPath,
			strings.HasPrefix(r.URL.Path, sharePath),
			strings.HasPrefix(r.URL.Path, "/assets/"),
			loopbackAddr(r.RemoteAddr),
			a.authenticated(r):
			next.ServeHTTP(w, r)
		case strings.HasPrefix(r.URL.Path, "/api/") || strings.HasPrefix(r.URL.Path, "/raw/"):
			http.Error(w, "unauthorized", http.StatusUnauthorized)
		default:
			http.Redirect(w, r, loginPath, http.StatusSeeOther)
		}
	})
}

// handleLogin is the front door, in two steps. GET renders: a confirm page
// when the URL carries a live token (clicking "Open" is what spends it — a
// bare GET never consumes, so chat-app link previews and unfurler bots can't
// kill an invite before the human arrives), the paste-a-token page when
// there's no token, or a spelled-out error when the token is spent/expired.
// POST consumes the token and sets the session cookie.
func (a *Auth) handleLogin(w http.ResponseWriter, r *http.Request) {
	if a.authenticated(r) {
		http.Redirect(w, r, "/", http.StatusSeeOther)
		return
	}
	if r.Method == http.MethodPost {
		if a.consumeToken(r.FormValue("token")) {
			// Secure when the browser's leg is TLS — directly, or terminated
			// at a relay that forwards plain HTTP down the tunnel (ADR-0034).
			secure := r.TLS != nil || r.Header.Get("X-Forwarded-Proto") == "https"
			http.SetCookie(w, &http.Cookie{
				Name:     authCookie,
				Value:    a.signCookie(cookiePayload{Sub: "local", Exp: time.Now().Add(cookieTTL).Unix()}),
				Path:     "/",
				MaxAge:   int(cookieTTL / time.Second),
				HttpOnly: true,
				SameSite: http.SameSiteLaxMode,
				Secure:   secure,
			})
			http.Redirect(w, r, "/", http.StatusSeeOther)
			return
		}
		// The token lost a race between the confirm page render and the
		// click (someone else used it, or it expired in between). Say so.
		writeLoginPage(w, http.StatusUnauthorized, loginPaste, tokenSpentMsg)
		return
	}
	tok := r.URL.Query().Get("token")
	switch {
	case tok == "":
		writeLoginPage(w, http.StatusUnauthorized, loginPaste, "")
	case a.peekToken(tok):
		// Live token: one click to spend it. The hidden form field carries
		// the same secret the URL already does — no new exposure.
		writeLoginPage(w, http.StatusOK, loginConfirm, tok)
	default:
		writeLoginPage(w, http.StatusUnauthorized, loginPaste, tokenSpentMsg)
	}
}

// tokenSpentMsg is the no-silent-deaths line: what happened, and both ways
// forward (one for invitees, one for operators with console access).
const tokenSpentMsg = "That login link was already used or has expired. " +
	"Ask the person who shared it for a fresh one (the Share button mints them), " +
	"or paste the current token from the arbos console or log."

// handleShare mints an independent one-time invite token and returns it as a
// shareable login URL — the UI's "share this agent" affordance. The URL is
// built from the request's own origin, so sharing from the forest URL hands
// out the forest URL and sharing from a LAN bind hands out that bind. Each
// invite is single-use with a TTL and lives apart from the console token, so
// sharing never invalidates the console link or an earlier unredeemed invite.
func (s *Server) handleShare(w http.ResponseWriter, r *http.Request) {
	tok := s.Auth.MintShareToken()
	if tok == "" {
		http.Error(w, "could not mint a token", http.StatusInternalServerError)
		return
	}
	scheme := "http"
	if r.TLS != nil || r.Header.Get("X-Forwarded-Proto") == "https" {
		scheme = "https"
	}
	writeJSON(w, map[string]any{
		"url": scheme + "://" + r.Host + loginPath + "?token=" + tok,
	})
}

// loginMode selects which body the login page renders: the paste-a-token
// form (arg = error message, "" for none) or the one-click confirm form
// (arg = the live token).
type loginMode int

const (
	loginPaste loginMode = iota
	loginConfirm
)

// writeLoginPage renders the front door. It mirrors the SPA's default theme
// (web/src/index.css) — same palette, type, and radii — so the front door
// looks like the house. Pre-auth there is no user to have picked a theme, so
// the default tokens are inlined rather than served from the bundle.
func writeLoginPage(w http.ResponseWriter, status int, mode loginMode, arg string) {
	var body string
	switch mode {
	case loginConfirm:
		body = `<form method="POST" action="/login">
  <h1>arbos</h1>
  <p>You’ve been given access to this agent. The link works once — opening it here claims it.</p>
  <input type="hidden" name="token" value="` + html.EscapeString(arg) + `">
  <button autofocus>Open agent</button>
</form>`
	default:
		errLine := ""
		if arg != "" {
			errLine = `<p class="err">` + html.EscapeString(arg) + `</p>`
		}
		body = `<form method="POST" action="/login">
  <h1>arbos</h1>` + errLine + `
  <p>Paste a login token.</p>
  <input name="token" placeholder="token" autofocus autocomplete="off">
  <button>Log in</button>
</form>`
	}
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	w.WriteHeader(status)
	_, _ = w.Write([]byte(loginPageHead + body))
}

const loginPageHead = `<!doctype html>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>arbos — login</title>
<style>
  :root {
    --canvas: #2c262a; --panel: #312b2f; --line: #3e373c;
    --text: #d6d0d4; --bright: #ece7ea; --muted: #968f94;
    --faint: #6e676c; --accent: #85aab3; --btn: #b0aaae;
    --err: #c98a8a;
  }
  body {
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Inter, Roboto, sans-serif;
    font-size: 13px; line-height: 1.6; color-scheme: dark;
    -webkit-font-smoothing: antialiased;
    background: var(--canvas); color: var(--text);
    display: grid; place-items: center; min-height: 100dvh; margin: 0;
  }
  ::selection { background: color-mix(in srgb, var(--accent) 28%, transparent); }
  form { display: flex; flex-direction: column; gap: .7rem; width: min(20rem, 88vw); }
  h1 { font-size: 15px; font-weight: 600; margin: 0; color: var(--bright); }
  p { color: var(--muted); margin: 0 0 .3rem; }
  p.err {
    color: var(--err); background: color-mix(in srgb, var(--err) 12%, transparent);
    border: 1px solid color-mix(in srgb, var(--err) 35%, transparent);
    border-radius: 6px; padding: .5rem .7rem;
  }
  input {
    background: var(--panel); border: 1px solid var(--line); color: var(--text);
    padding: .55rem .7rem; border-radius: 6px; font: inherit; outline: none;
  }
  input::placeholder { color: var(--faint); }
  input:focus { border-color: var(--accent); }
  button {
    background: var(--btn); border: 0; color: var(--canvas); font-weight: 600;
    padding: .55rem; border-radius: 6px; font: inherit; cursor: pointer;
  }
  button:hover { background: var(--bright); }
</style>
`

// loopbackAddr reports whether an http RemoteAddr ("host:port") is loopback.
func loopbackAddr(remote string) bool {
	host, _, err := net.SplitHostPort(remote)
	if err != nil {
		return false
	}
	ip := net.ParseIP(host)
	return ip != nil && ip.IsLoopback()
}
