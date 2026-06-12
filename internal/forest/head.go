package forest

import (
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net"
	"net/http"
	"net/http/httputil"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/coder/websocket"
	"github.com/hashicorp/yamux"
)

// HeadConfig configures a forest head.
type HeadConfig struct {
	// Domain is the public apex the head answers as, including a non-default
	// port if any (e.g. "204-12-163-231.sslip.io:8080"). Lease hosts are
	// "<name>.<Domain>"; anything else 404s.
	Domain string
	// Scheme ("http" | "https") for the URLs minted into heartbeat
	// responses. The head itself only listens plain; TLS termination in
	// front (or autocert, later) flips this to https.
	Scheme string
	// StatePath persists registered devices (a JSON map), so an anonymous
	// account survives head restarts. Leases are deliberately not persisted:
	// they are ephemeral by design and re-establish on the next heartbeat.
	StatePath string
	// LeaseTTL is how long a lease outlives its last heartbeat. Heartbeat is
	// the cadence handed to nodes. Head-controlled on purpose.
	LeaseTTL  time.Duration
	Heartbeat time.Duration
	Logf      func(format string, args ...any)
}

// Head is the forest head: device registry, token mint, lease table, and the
// relay that routes "<name>.<domain>" requests down each node's tunnel. The
// relay is a dumb pipe — it never makes auth decisions for a node; the node's
// own gateway gate does (ADR-0034).
type Head struct {
	cfg  HeadConfig
	apex string // Domain without the port, for Host matching

	mu      sync.Mutex
	devices map[string]deviceRec // by public key (base64)
	byID    map[string]deviceRec // by device id
	nonces  map[string]nonceRec  // by device id
	tokens  map[string]tokenRec  // by opaque token
	leases  map[string]*lease    // by name
	byDev   map[string]*lease    // by device id
}

type deviceRec struct {
	AccountID string `json:"account_id"`
	DeviceID  string `json:"device_id"`
	PublicKey string `json:"public_key"`
}

type nonceRec struct {
	nonce   []byte
	expires time.Time
}

type tokenRec struct {
	deviceID string
	expires  time.Time
}

// lease is one node's ephemeral claim on a name: directory row, expiry, and
// (once the tunnel is up) the live session + proxy that serve it.
type lease struct {
	name     string
	deviceID string

	mu      sync.Mutex
	expires time.Time
	sess    *yamux.Session
	proxy   *httputil.ReverseProxy
}

const (
	nonceTTL = 2 * time.Minute
	tokenTTL = 24 * time.Hour
	// maxTunnelStreams caps concurrent proxied requests per node — the
	// anonymous tier's brake against one lease soaking the relay.
	maxTunnelStreams = 32
)

// NewHead loads persisted devices and returns a serving-ready head.
func NewHead(cfg HeadConfig) (*Head, error) {
	if cfg.Scheme == "" {
		cfg.Scheme = "http"
	}
	if cfg.LeaseTTL == 0 {
		cfg.LeaseTTL = time.Hour
	}
	if cfg.Heartbeat == 0 {
		cfg.Heartbeat = 30 * time.Second
	}
	if cfg.Logf == nil {
		cfg.Logf = func(string, ...any) {}
	}
	apex := cfg.Domain
	if h, _, err := net.SplitHostPort(cfg.Domain); err == nil {
		apex = h
	}
	head := &Head{
		cfg:     cfg,
		apex:    apex,
		devices: map[string]deviceRec{},
		byID:    map[string]deviceRec{},
		nonces:  map[string]nonceRec{},
		tokens:  map[string]tokenRec{},
		leases:  map[string]*lease{},
		byDev:   map[string]*lease{},
	}
	if err := head.loadState(); err != nil {
		return nil, err
	}
	return head, nil
}

func (h *Head) loadState() error {
	if h.cfg.StatePath == "" {
		return nil
	}
	b, err := os.ReadFile(h.cfg.StatePath)
	if os.IsNotExist(err) {
		return nil
	}
	if err != nil {
		return err
	}
	var devs []deviceRec
	if err := json.Unmarshal(b, &devs); err != nil {
		return fmt.Errorf("forest state %s: %w", h.cfg.StatePath, err)
	}
	for _, d := range devs {
		h.devices[d.PublicKey] = d
		h.byID[d.DeviceID] = d
	}
	return nil
}

// saveState persists the device registry. Called with h.mu held.
func (h *Head) saveState() {
	if h.cfg.StatePath == "" {
		return
	}
	devs := make([]deviceRec, 0, len(h.devices))
	for _, d := range h.devices {
		devs = append(devs, d)
	}
	b, err := json.MarshalIndent(devs, "", "  ")
	if err != nil {
		return
	}
	if err := os.MkdirAll(filepath.Dir(h.cfg.StatePath), 0o700); err != nil {
		return
	}
	tmp := h.cfg.StatePath + ".tmp"
	if err := os.WriteFile(tmp, b, 0o600); err != nil {
		return
	}
	_ = os.Rename(tmp, h.cfg.StatePath)
}

// Handler routes by Host: the apex serves the API, "<name>.<apex>" serves
// that lease's tunnel, anything else 404s.
func (h *Head) Handler() http.Handler {
	api := http.NewServeMux()
	api.HandleFunc("POST /v1/devices/register", h.handleRegister)
	api.HandleFunc("GET /v1/devices/challenge", h.handleChallenge)
	api.HandleFunc("POST /v1/devices/token", h.handleToken)
	api.HandleFunc("POST /v1/nodes/heartbeat", h.handleHeartbeat)
	api.HandleFunc("GET "+tunnelPath, h.handleTunnel)
	api.HandleFunc("GET /", h.handleIndex)

	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		host := strings.ToLower(r.Host)
		if hh, _, err := net.SplitHostPort(host); err == nil {
			host = hh
		}
		// Subdomains of the apex are leases; every other Host — the apex
		// itself, the bare IP, whatever a join URL used before DNS existed —
		// is the API. Liberal on purpose: the API leaks nothing by Host.
		if name, ok := strings.CutSuffix(host, "."+strings.ToLower(h.apex)); ok {
			h.relay(w, r, name)
			return
		}
		api.ServeHTTP(w, r)
	})
}

func (h *Head) handleIndex(w http.ResponseWriter, r *http.Request) {
	if r.URL.Path != "/" {
		http.NotFound(w, r)
		return
	}
	h.mu.Lock()
	n := len(h.leases)
	h.mu.Unlock()
	fmt.Fprintf(w, "arbos forest head — %d active lease(s)\n", n)
}

func (h *Head) handleRegister(w http.ResponseWriter, r *http.Request) {
	var req registerReq
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeErr(w, http.StatusBadRequest, "bad_request", "malformed body")
		return
	}
	pub, err := base64.StdEncoding.DecodeString(req.PublicKey)
	if err != nil || len(pub) != ed25519.PublicKeySize {
		writeErr(w, http.StatusBadRequest, "bad_request", "public_key must be a base64 ed25519 key")
		return
	}
	h.mu.Lock()
	dev, ok := h.devices[req.PublicKey]
	if !ok {
		dev = deviceRec{
			AccountID: "acct_" + randHex(12),
			DeviceID:  "dev_" + randHex(12),
			PublicKey: req.PublicKey,
		}
		h.devices[req.PublicKey] = dev
		h.byID[dev.DeviceID] = dev
		h.saveState()
		h.cfg.Logf("forest: registered device %s (%s@%s/%s)", dev.DeviceID, req.Machine.Hostname, req.Machine.OS, req.Machine.Arch)
	}
	h.mu.Unlock()
	writeJSON(w, registerResp{AccountID: dev.AccountID, DeviceID: dev.DeviceID})
}

func (h *Head) handleChallenge(w http.ResponseWriter, r *http.Request) {
	id := r.URL.Query().Get("device_id")
	h.mu.Lock()
	_, known := h.byID[id]
	var nonce []byte
	if known {
		nonce = make([]byte, 32)
		_, _ = rand.Read(nonce)
		h.nonces[id] = nonceRec{nonce: nonce, expires: time.Now().Add(nonceTTL)}
	}
	h.mu.Unlock()
	if !known {
		writeErr(w, http.StatusNotFound, "not_found", "unknown device")
		return
	}
	writeJSON(w, challengeResp{
		Nonce:     base64.StdEncoding.EncodeToString(nonce),
		ExpiresAt: time.Now().Add(nonceTTL).Format(time.RFC3339),
	})
}

func (h *Head) handleToken(w http.ResponseWriter, r *http.Request) {
	var req tokenReq
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeErr(w, http.StatusBadRequest, "bad_request", "malformed body")
		return
	}
	sig, err := base64.StdEncoding.DecodeString(req.Signature)
	if err != nil {
		writeErr(w, http.StatusBadRequest, "bad_request", "malformed signature")
		return
	}
	h.mu.Lock()
	dev, known := h.byID[req.DeviceID]
	nr, hasNonce := h.nonces[req.DeviceID]
	delete(h.nonces, req.DeviceID) // single-use either way
	h.mu.Unlock()
	if !known || !hasNonce || time.Now().After(nr.expires) {
		writeErr(w, http.StatusUnauthorized, "unauthorized", "no live challenge for device")
		return
	}
	pub, _ := base64.StdEncoding.DecodeString(dev.PublicKey)
	if base64.StdEncoding.EncodeToString(nr.nonce) != req.Nonce ||
		!ed25519.Verify(ed25519.PublicKey(pub), nr.nonce, sig) {
		writeErr(w, http.StatusUnauthorized, "unauthorized", "signature does not verify")
		return
	}
	tok := randHex(32)
	h.mu.Lock()
	h.tokens[tok] = tokenRec{deviceID: dev.DeviceID, expires: time.Now().Add(tokenTTL)}
	h.mu.Unlock()
	writeJSON(w, tokenResp{AccessToken: tok, TokenType: "Bearer", ExpiresIn: int(tokenTTL / time.Second)})
}

// authDevice resolves the bearer token to a device id ("" = unauthorized).
func (h *Head) authDevice(r *http.Request) string {
	tok, ok := strings.CutPrefix(r.Header.Get("Authorization"), "Bearer ")
	if !ok {
		return ""
	}
	h.mu.Lock()
	defer h.mu.Unlock()
	rec, ok := h.tokens[tok]
	if !ok || time.Now().After(rec.expires) {
		delete(h.tokens, tok)
		return ""
	}
	return rec.deviceID
}

// handleHeartbeat is the lease for the anonymous tier: first beat assigns a
// name, every beat extends it. The response carries the head-controlled TTL
// and cadence.
func (h *Head) handleHeartbeat(w http.ResponseWriter, r *http.Request) {
	dev := h.authDevice(r)
	if dev == "" {
		writeErr(w, http.StatusUnauthorized, "unauthorized", "valid bearer token required")
		return
	}
	var req heartbeatReq
	_ = json.NewDecoder(r.Body).Decode(&req)

	h.mu.Lock()
	l := h.byDev[dev]
	if l == nil {
		name := mintName()
		for h.leases[name] != nil {
			name = mintName()
		}
		l = &lease{name: name, deviceID: dev}
		h.leases[name] = l
		h.byDev[dev] = l
		h.cfg.Logf("forest: leased %s.%s to %s", name, h.cfg.Domain, dev)
	}
	h.mu.Unlock()

	l.mu.Lock()
	l.expires = time.Now().Add(h.cfg.LeaseTTL)
	l.mu.Unlock()

	writeJSON(w, heartbeatResp{
		Name:             l.name,
		URL:              fmt.Sprintf("%s://%s.%s", h.cfg.Scheme, l.name, h.cfg.Domain),
		TTLSeconds:       int(h.cfg.LeaseTTL / time.Second),
		HeartbeatSeconds: int(h.cfg.Heartbeat / time.Second),
	})
}

// handleTunnel upgrades to WebSocket and becomes the lease's transport: a
// yamux session where the head opens one stream per proxied request. The
// handler blocks for the tunnel's lifetime — the WebSocket dies with the
// request. A reconnect replaces the previous session.
func (h *Head) handleTunnel(w http.ResponseWriter, r *http.Request) {
	dev := h.authDevice(r)
	if dev == "" {
		writeErr(w, http.StatusUnauthorized, "unauthorized", "valid bearer token required")
		return
	}
	h.mu.Lock()
	l := h.byDev[dev]
	h.mu.Unlock()
	if l == nil {
		writeErr(w, http.StatusConflict, "no_lease", "heartbeat before tunneling")
		return
	}

	c, err := websocket.Accept(w, r, &websocket.AcceptOptions{InsecureSkipVerify: true})
	if err != nil {
		return
	}
	c.SetReadLimit(-1) // the tunnel carries arbitrary HTTP bodies; yamux frames bound reads
	nc := websocket.NetConn(r.Context(), c, websocket.MessageBinary)
	sess, err := yamux.Client(nc, nil) // head is the opener; the node accepts
	if err != nil {
		_ = c.Close(websocket.StatusInternalError, "yamux")
		return
	}

	l.mu.Lock()
	if l.sess != nil {
		_ = l.sess.Close()
	}
	l.sess = sess
	l.proxy = newLeaseProxy(l.name, h.cfg.Scheme, sess)
	l.mu.Unlock()
	h.cfg.Logf("forest: tunnel up for %s (%s)", l.name, dev)

	<-sess.CloseChan()
	h.cfg.Logf("forest: tunnel down for %s (%s)", l.name, dev)
	l.mu.Lock()
	if l.sess == sess {
		l.sess, l.proxy = nil, nil
	}
	l.mu.Unlock()
}

// newLeaseProxy reverse-proxies one lease's requests down its yamux session.
// The transport dials streams instead of sockets; a semaphore caps concurrent
// streams (the anonymous-tier brake). ReverseProxy passes WebSocket upgrades
// through, so the gateway's control seam and terminals ride the same path.
func newLeaseProxy(name, scheme string, sess *yamux.Session) *httputil.ReverseProxy {
	slots := make(chan struct{}, maxTunnelStreams)
	transport := &http.Transport{
		DialContext: func(ctx context.Context, network, addr string) (net.Conn, error) {
			select {
			case slots <- struct{}{}:
			case <-ctx.Done():
				return nil, ctx.Err()
			}
			stream, err := sess.Open()
			if err != nil {
				<-slots
				return nil, err
			}
			return &slotConn{Conn: stream, release: func() { <-slots }}, nil
		},
		MaxIdleConnsPerHost: 4,
		IdleConnTimeout:     30 * time.Second,
	}
	return &httputil.ReverseProxy{
		Rewrite: func(pr *httputil.ProxyRequest) {
			pr.Out.URL.Scheme = "http"
			pr.Out.URL.Host = name // routing is the session; the host is cosmetic
			pr.Out.Host = pr.In.Host
			pr.SetXForwarded()
			// SetXForwarded stamps X-Forwarded-Proto from the head's own
			// (plain) listener, but TLS is terminated in front of the head, so
			// the node would always see "http" and mint its session cookie
			// without Secure even for an https forest. Override with the
			// forest's configured scheme so the node's Secure decision matches
			// the URL the user actually loaded.
			pr.Out.Header.Set("X-Forwarded-Proto", scheme)
		},
		Transport: transport,
		ErrorHandler: func(w http.ResponseWriter, r *http.Request, err error) {
			http.Error(w, "node unreachable", http.StatusBadGateway)
		},
	}
}

// slotConn returns its semaphore slot when the proxied connection closes.
type slotConn struct {
	net.Conn
	once    sync.Once
	release func()
}

func (s *slotConn) Close() error {
	err := s.Conn.Close()
	s.once.Do(s.release)
	return err
}

// relay serves one request arriving for "<name>.<domain>".
func (h *Head) relay(w http.ResponseWriter, r *http.Request, name string) {
	h.mu.Lock()
	l := h.leases[name]
	h.mu.Unlock()
	if l == nil {
		http.Error(w, "no such node", http.StatusNotFound)
		return
	}
	l.mu.Lock()
	expired := time.Now().After(l.expires)
	proxy := l.proxy
	l.mu.Unlock()
	if expired || proxy == nil {
		http.Error(w, "node offline", http.StatusBadGateway)
		return
	}
	proxy.ServeHTTP(w, r)
}

// Sweep reaps expired leases (closing any lingering tunnel) until ctx ends.
// Run it as a goroutine next to the HTTP server.
func (h *Head) Sweep(ctx context.Context) {
	tick := time.NewTicker(time.Minute)
	defer tick.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-tick.C:
		}
		now := time.Now()
		h.mu.Lock()
		for name, l := range h.leases {
			l.mu.Lock()
			dead := now.After(l.expires)
			sess := l.sess
			l.mu.Unlock()
			if !dead {
				continue
			}
			if sess != nil {
				_ = sess.Close()
			}
			delete(h.leases, name)
			delete(h.byDev, l.deviceID)
			h.cfg.Logf("forest: lease %s expired", name)
		}
		h.mu.Unlock()
	}
}

func writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(v)
}

func writeErr(w http.ResponseWriter, code int, typ, msg string) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	_ = json.NewEncoder(w).Encode(errorResp{Error: errorBody{Type: typ, Message: msg}})
}

func randHex(n int) string {
	b := make([]byte, (n+1)/2)
	_, _ = rand.Read(b)
	return hex.EncodeToString(b)[:n]
}
