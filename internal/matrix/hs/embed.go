// Package hs embeds the in-process Matrix homeserver (ADR-0041 D8): every node
// is its own homeserver, compiled into the single binary. It is the only arbos
// package that imports Dendrite (via the fork's public embed package), kept
// separate from internal/matrix so the SessionStore adapter and its callers do
// not drag the homeserver into their builds.
package hs

import (
	"context"
	"fmt"
	"net/http"
	"os"
	"time"

	dendrite "github.com/element-hq/dendrite/embed"
)

// Homeserver is the running embedded homeserver, on loopback. Federation, TLS,
// and the device-key-derived signing identity layer on in later phases; this is
// the "a node is its own homeserver" core.
type Homeserver struct {
	srv *dendrite.Server
	// BaseURL is the loopback client-server API base the node's own mautrix
	// client (and only it, until federation) connects to.
	BaseURL string
}

// Start brings up the embedded homeserver. dataDir holds the per-component
// SQLite databases and media (under .arbos/matrix in production); serverName is
// the Matrix server_name (the machine's forest name, or "localhost" for a purely
// local node); bindAddr is the loopback listen address. It returns once the
// listener is launched and the client API answers.
func Start(ctx context.Context, dataDir, serverName, bindAddr string) (*Homeserver, error) {
	// The per-component SQLite databases are created directly in dataDir, so it
	// must exist first (SQLite will not create the parent directory).
	if err := os.MkdirAll(dataDir, 0o700); err != nil {
		return nil, fmt.Errorf("matrix/hs: create data dir: %w", err)
	}
	srv, err := dendrite.Start(dendrite.Options{
		ServerName:       serverName,
		DataDir:          dataDir,
		BindAddr:         bindAddr,
		OpenRegistration: true, // loopback-only; the appservice/identity flow replaces this later
	})
	if err != nil {
		return nil, err
	}
	h := &Homeserver{srv: srv, BaseURL: srv.BaseURL}
	if err := h.waitReady(ctx); err != nil {
		h.Shutdown()
		return nil, err
	}
	return h, nil
}

// waitReady blocks until the client-server API answers or ctx/deadline elapses.
func (h *Homeserver) waitReady(ctx context.Context) error {
	url := h.BaseURL + "/_matrix/client/versions"
	deadline := time.Now().Add(30 * time.Second)
	for time.Now().Before(deadline) {
		if ctx.Err() != nil {
			return ctx.Err()
		}
		req, _ := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
		if resp, err := http.DefaultClient.Do(req); err == nil {
			_ = resp.Body.Close()
			if resp.StatusCode == http.StatusOK {
				return nil
			}
		}
		time.Sleep(250 * time.Millisecond)
	}
	return fmt.Errorf("matrix/hs: embedded homeserver at %s never became ready", h.BaseURL)
}

// CreateAccount provisions a local Matrix user via the homeserver's user API
// (the node creating its own agent/human users, ADR-0041 bootstrapping),
// returning the full user ID. Independent of client-side registration flows.
func (h *Homeserver) CreateAccount(ctx context.Context, localpart, password string) (string, error) {
	return h.srv.CreateAccount(ctx, localpart, password)
}

// Shutdown stops the embedded homeserver and waits for its components to exit.
func (h *Homeserver) Shutdown() {
	if h != nil && h.srv != nil {
		h.srv.Shutdown()
	}
}
