package gateway

import (
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
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
// session cookie. Consuming a token mints and prints a fresh one, so the
// operator's console always holds a current login link for the next browser.
// Requests arriving over loopback bypass auth entirely — a local process can
// already read the signing key off disk, so gating it buys nothing and would
// break the localhost-out-of-the-box promise.
type Auth struct {
	// Key signs session cookies (HMAC-SHA256). Loaded once per host via
	// LoadOrCreateAuthKey so cookies survive restarts.
	Key []byte
	// OnToken receives each freshly minted one-time token; the host renders
	// it as a clickable login URL on stderr.
	OnToken func(token string)

	mu    sync.Mutex
	token string
}

const (
	authCookie = "arbos_auth"
	loginPath  = "/login"
	cookieTTL  = 30 * 24 * time.Hour
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

// MintToken rotates the one-time login token and announces it via OnToken.
// Called once at startup and again on every successful login.
func (a *Auth) MintToken() {
	b := make([]byte, 32)
	if _, err := rand.Read(b); err != nil {
		return
	}
	tok := hex.EncodeToString(b)
	a.mu.Lock()
	a.token = tok
	a.mu.Unlock()
	if a.OnToken != nil {
		a.OnToken(tok)
	}
}

// consumeToken redeems the current one-time token. Single-use: success clears
// it and mints a replacement, so a leaked URL (browser history, chat preview)
// is dead after first use while the console always shows a live link.
func (a *Auth) consumeToken(tok string) bool {
	if tok == "" {
		return false
	}
	a.mu.Lock()
	cur := a.token
	ok := cur != "" && subtle.ConstantTimeCompare([]byte(tok), []byte(cur)) == 1
	if ok {
		a.token = ""
	}
	a.mu.Unlock()
	if ok {
		a.MintToken()
	}
	return ok
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

// wrap is the gate in front of the whole route table. Loopback callers pass
// (they own the box), /login passes (it is the way in), everything else needs
// the cookie. API and raw paths get a bare 401 so fetch() fails loudly;
// document navigations get redirected to the login page.
func (a *Auth) wrap(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case r.URL.Path == loginPath,
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

// handleLogin redeems a one-time token for a session cookie, or shows the
// paste-a-token page. GET on purpose: the printed URL must be clickable, and
// the token is single-use so replay from history is inert.
func (a *Auth) handleLogin(w http.ResponseWriter, r *http.Request) {
	if a.authenticated(r) {
		http.Redirect(w, r, "/", http.StatusSeeOther)
		return
	}
	if a.consumeToken(r.URL.Query().Get("token")) {
		// Secure when the browser's leg is TLS — directly, or terminated at
		// a relay that forwards plain HTTP down the tunnel (ADR-0034).
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
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	w.WriteHeader(http.StatusUnauthorized)
	_, _ = w.Write([]byte(loginPage))
}

// loginPage is the fallback when someone reaches /login without a valid
// token: paste the token from the server console. It mirrors the SPA's
// default theme (web/src/index.css) — same palette, type, and radii — so the
// front door looks like the house. Pre-auth there is no user to have picked a
// theme, so the default tokens are inlined rather than served from the
// bundle.
const loginPage = `<!doctype html>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>arbos — login</title>
<style>
  :root {
    --canvas: #2c262a; --panel: #312b2f; --line: #3e373c;
    --text: #d6d0d4; --bright: #ece7ea; --muted: #968f94;
    --faint: #6e676c; --accent: #85aab3; --btn: #b0aaae;
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
<form method="GET" action="/login">
  <h1>arbos</h1>
  <p>Paste the login token from the server console.</p>
  <input name="token" placeholder="token" autofocus autocomplete="off">
  <button>Log in</button>
</form>`

// loopbackAddr reports whether an http RemoteAddr ("host:port") is loopback.
func loopbackAddr(remote string) bool {
	host, _, err := net.SplitHostPort(remote)
	if err != nil {
		return false
	}
	ip := net.ParseIP(host)
	return ip != nil && ip.IsLoopback()
}
