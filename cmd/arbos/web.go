package main

import (
	"context"
	"errors"
	"fmt"
	"io/fs"
	"net"
	"net/http"
	"os"
	"os/signal"
	"syscall"

	"github.com/unarbos/arbos/internal/gateway"
	"github.com/unarbos/arbos/internal/piwire"
	"github.com/unarbos/arbos/web"
)

// runWeb is the browser door: one assembled host serving the control seam
// over WebSocket plus the embedded UI, at addr. Like -serve it is a
// long-lived body for the agent, so it carries the clock — the plan scheduler
// fires deferred tasks, standing obligations, and callbacks here.
func runWeb(cfg piwire.Config, dbPath, addr, dist string, approve bool) error {
	host, _, cleanup, err := assemble(cfg, dbPath, approve, false)
	if err != nil {
		return err
	}
	defer cleanup()

	cfg.WarnIfNoLLM(os.Stderr)

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	if stopSched := host.StartPlanScheduler(); stopSched != nil {
		defer stopSched()
	}

	gw := &gateway.Server{
		Engine:       host.Engine,
		NewSessionID: piwire.NewSessionID,
		Drain:        cfg.ServeDrainTimeout,
	}
	if dist != "" {
		// Override the embedded bundle with a directory (UI development).
		gw.Dist = os.DirFS(dist)
	} else if sub, err := fs.Sub(web.Dist, "dist"); err == nil {
		gw.Dist = sub
	}

	// BaseContext ties every request — including hijacked WebSocket seam
	// connections, which Shutdown cannot reach — to the signal context, so a
	// SIGINT cancels in-flight control.Serve loops instead of orphaning them.
	srv := &http.Server{
		Addr:        addr,
		Handler:     gw.Handler(),
		BaseContext: func(net.Listener) context.Context { return ctx },
	}
	errCh := make(chan error, 1)
	go func() { errCh <- srv.ListenAndServe() }()
	fmt.Fprintf(os.Stderr, "arbos web listening on http://%s\n", displayAddr(addr))

	select {
	case <-ctx.Done():
		shutCtx, cancel := context.WithTimeout(context.Background(), cfg.ServeDrainTimeout)
		defer cancel()
		_ = srv.Shutdown(shutCtx)
		return nil
	case err := <-errCh:
		if errors.Is(err, http.ErrServerClosed) {
			return nil
		}
		return err
	}
}

// displayAddr turns ":8420" into a clickable "localhost:8420".
func displayAddr(addr string) string {
	if addr != "" && addr[0] == ':' {
		return "localhost" + addr
	}
	return addr
}
