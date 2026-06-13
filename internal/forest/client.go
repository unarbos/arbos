package forest

import (
	"bytes"
	"context"
	"crypto/ed25519"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"os"
	"runtime"
	"strings"
	"time"

	"github.com/coder/websocket"
	"github.com/hashicorp/yamux"
)

// DefaultHead is the forest a bare `arbos web` joins when no --forest/--local
// override is given: the canonical "one command, get a URL" path. Nodes are
// auth-gated by their own login token; the head is transport, not a trust
// boundary, so defaulting in is safe — but the join is always printed loudly
// so nobody is surprised their node has a public name.
const DefaultHead = "https://arbos.life"

// JoinInfo is what a successful join hands back: the leased name and the
// public base URL the node is now reachable at.
type JoinInfo struct {
	Name string
	URL  string
}

// Client joins a node to a forest: registers the device, mints a token,
// heartbeats to hold the lease, and serves Handler back through an outbound
// tunnel. Run blocks and reconnects forever — a dropped tunnel or a restarted
// head is rejoined with backoff, because the lease model already assumes
// nodes come and go.
type Client struct {
	// Base is the forest head's URL, e.g. "http://204-12-163-231.sslip.io:8080".
	Base string
	// KeyPath is the device key seed (LoadOrCreateDeviceKey).
	KeyPath string
	// AgentDir is the workspace whose per-directory key (<dir>/.arbos/agent.key)
	// composes with the device to derive a stable URL (ADR-0035). Empty leaves
	// the name a function of the device alone.
	AgentDir string
	// Handler is served to tunneled requests — the same gateway handler the
	// local listener serves, auth gate included. Serving it directly off the
	// tunnel (no loopback hop) is deliberate: tunneled requests must never
	// look like local ones to the gate (ADR-0034).
	Handler http.Handler
	// OnJoin fires whenever a join (or rejoin) succeeds.
	OnJoin func(JoinInfo)
	Logf   func(format string, args ...any)

	httpc    *http.Client
	key      ed25519.PrivateKey
	agentKey string // base64 ed25519 public key of AgentDir's agent key
	devID    string
	token    string
}

const (
	joinBackoffMin = 2 * time.Second
	joinBackoffMax = time.Minute
)

// Run joins and serves until ctx ends. Every failure path falls back to a
// fresh join attempt after backoff; the only fatal errors are local ones
// (an unreadable device key).
func (c *Client) Run(ctx context.Context) error {
	if c.Logf == nil {
		c.Logf = func(string, ...any) {}
	}
	c.httpc = &http.Client{Timeout: 15 * time.Second}
	key, err := LoadOrCreateDeviceKey(c.KeyPath)
	if err != nil {
		return fmt.Errorf("device key: %w", err)
	}
	c.key = key
	if c.AgentDir != "" {
		ak, err := LoadOrCreateAgentKey(c.AgentDir)
		if err != nil {
			return fmt.Errorf("agent key: %w", err)
		}
		c.agentKey = base64.StdEncoding.EncodeToString(ak.Public().(ed25519.PublicKey))
	}

	backoff := joinBackoffMin
	for {
		err := c.join(ctx)
		if ctx.Err() != nil {
			return nil
		}
		c.Logf("forest: connection lost (%v); rejoining in %s", err, backoff)
		select {
		case <-ctx.Done():
			return nil
		case <-time.After(backoff):
		}
		if backoff *= 2; backoff > joinBackoffMax {
			backoff = joinBackoffMax
		}
	}
}

// join performs one full cycle: register → token → heartbeat (lease) →
// tunnel. It returns when the tunnel dies; the caller rejoins.
func (c *Client) join(ctx context.Context) error {
	if err := c.register(ctx); err != nil {
		return err
	}
	if err := c.mintToken(ctx); err != nil {
		return err
	}
	hb, err := c.heartbeat(ctx)
	if err != nil {
		return err
	}
	if c.OnJoin != nil {
		c.OnJoin(JoinInfo{Name: hb.Name, URL: hb.URL})
	}

	// Heartbeats hold the lease for as long as the tunnel lives; both share
	// a context so a dead tunnel also stops beating (and vice versa nothing
	// holds a lease for an unreachable node beyond its TTL).
	tctx, cancel := context.WithCancel(ctx)
	defer cancel()
	go c.beat(tctx, time.Duration(hb.HeartbeatSeconds)*time.Second)

	return c.tunnel(tctx)
}

func (c *Client) register(ctx context.Context) error {
	host, _ := os.Hostname()
	var resp registerResp
	err := c.post(ctx, "/v1/devices/register", registerReq{
		PublicKey: base64.StdEncoding.EncodeToString(c.key.Public().(ed25519.PublicKey)),
		Machine:   machine{Hostname: host, OS: runtime.GOOS, Arch: runtime.GOARCH},
	}, &resp)
	if err != nil {
		return fmt.Errorf("register: %w", err)
	}
	c.devID = resp.DeviceID
	return nil
}

func (c *Client) mintToken(ctx context.Context) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet,
		c.Base+"/v1/devices/challenge?device_id="+url.QueryEscape(c.devID), nil)
	if err != nil {
		return err
	}
	var ch challengeResp
	if err := c.do(req, &ch); err != nil {
		return fmt.Errorf("challenge: %w", err)
	}
	nonce, err := base64.StdEncoding.DecodeString(ch.Nonce)
	if err != nil {
		return fmt.Errorf("challenge nonce: %w", err)
	}
	var tok tokenResp
	err = c.post(ctx, "/v1/devices/token", tokenReq{
		DeviceID:  c.devID,
		Nonce:     ch.Nonce,
		Signature: base64.StdEncoding.EncodeToString(ed25519.Sign(c.key, nonce)),
	}, &tok)
	if err != nil {
		return fmt.Errorf("token: %w", err)
	}
	c.token = tok.AccessToken
	return nil
}

func (c *Client) heartbeat(ctx context.Context) (heartbeatResp, error) {
	host, _ := os.Hostname()
	cwd, _ := os.Getwd()
	var resp heartbeatResp
	err := c.post(ctx, "/v1/nodes/heartbeat", heartbeatReq{Host: host, Cwd: cwd, AgentKey: c.agentKey}, &resp)
	if err != nil {
		return resp, fmt.Errorf("heartbeat: %w", err)
	}
	return resp, nil
}

// beat renews the lease on the head's cadence until ctx ends. A failed beat
// is logged, not fatal: the tunnel's own death is what triggers a rejoin.
func (c *Client) beat(ctx context.Context, every time.Duration) {
	if every <= 0 {
		every = 30 * time.Second
	}
	tick := time.NewTicker(every)
	defer tick.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-tick.C:
		}
		if _, err := c.heartbeat(ctx); err != nil && ctx.Err() == nil {
			c.Logf("forest: heartbeat failed: %v", err)
		}
	}
}

// tunnel dials the head, becomes the accepting side of the yamux session,
// and serves the gateway handler on it. Blocks until the session dies.
func (c *Client) tunnel(ctx context.Context) error {
	wsURL := strings.Replace(c.Base, "http", "ws", 1) + tunnelPath
	if c.agentKey != "" {
		wsURL += "?agent=" + url.QueryEscape(c.agentKey)
	}
	conn, _, err := websocket.Dial(ctx, wsURL, &websocket.DialOptions{
		HTTPHeader: http.Header{"Authorization": {"Bearer " + c.token}},
	})
	if err != nil {
		return fmt.Errorf("tunnel dial: %w", err)
	}
	conn.SetReadLimit(-1)
	nc := websocket.NetConn(ctx, conn, websocket.MessageBinary)
	// Route yamux's own logging through Logf and silence it during shutdown:
	// the default config logs to stderr, so a Ctrl-C (which cancels ctx and
	// fails the session's blocked frame read with "context canceled") would
	// otherwise dump a raw "[ERR] yamux: ..." line. A live session's errors
	// still surface.
	muxCfg := yamux.DefaultConfig()
	muxCfg.LogOutput = nil
	muxCfg.Logger = yamuxLogger{ctx: ctx, logf: c.Logf}
	sess, err := yamux.Server(nc, muxCfg)
	if err != nil {
		_ = conn.Close(websocket.StatusInternalError, "yamux")
		return fmt.Errorf("tunnel mux: %w", err)
	}
	defer func() { _ = sess.Close() }()

	// One HTTP server per tunnel: streams are its connections. RemoteAddr on
	// these is the yamux pseudo-address — unparseable as host:port, which the
	// gateway's loopback check correctly treats as remote.
	srv := &http.Server{
		Handler:     c.Handler,
		BaseContext: func(net.Listener) context.Context { return ctx },
	}
	go func() {
		<-ctx.Done()
		_ = srv.Close()
	}()
	err = srv.Serve(sess)
	if ctx.Err() != nil || err == http.ErrServerClosed {
		return nil
	}
	return err
}

// yamuxLogger adapts yamux's logger to the client's Logf and suppresses it
// while ctx is canceled. yamux runs a read loop blocked on the next frame
// header; on shutdown the canceled context fails that read with "context
// canceled", which is expected teardown, not an error worth printing. Errors
// on a still-live session pass through to Logf, prefixed as forest output.
type yamuxLogger struct {
	ctx  context.Context
	logf func(format string, args ...any)
}

func (l yamuxLogger) emit(msg string) {
	if l.logf == nil || l.ctx.Err() != nil {
		return
	}
	l.logf("forest: %s", strings.TrimSpace(msg))
}

func (l yamuxLogger) Print(v ...any)                 { l.emit(fmt.Sprint(v...)) }
func (l yamuxLogger) Printf(format string, v ...any) { l.emit(fmt.Sprintf(format, v...)) }
func (l yamuxLogger) Println(v ...any)               { l.emit(fmt.Sprintln(v...)) }

// post sends JSON (bearer-authenticated when a token is held) and decodes
// the response.
func (c *Client) post(ctx context.Context, path string, body, out any) error {
	b, err := json.Marshal(body)
	if err != nil {
		return err
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, c.Base+path, bytes.NewReader(b))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/json")
	return c.do(req, out)
}

func (c *Client) do(req *http.Request, out any) error {
	if c.token != "" {
		req.Header.Set("Authorization", "Bearer "+c.token)
	}
	resp, err := c.httpc.Do(req)
	if err != nil {
		return err
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusOK {
		var e errorResp
		if json.NewDecoder(io.LimitReader(resp.Body, 4096)).Decode(&e) == nil && e.Error.Message != "" {
			return fmt.Errorf("%s: %s", resp.Status, e.Error.Message)
		}
		return fmt.Errorf("%s", resp.Status)
	}
	if out == nil {
		return nil
	}
	return json.NewDecoder(resp.Body).Decode(out)
}
