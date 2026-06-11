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

	"github.com/unarbos/arbos/internal/agent/pi"
	"github.com/unarbos/arbos/internal/gateway"
	"github.com/unarbos/arbos/internal/piwire"
	"github.com/unarbos/arbos/internal/tool/codingspec"
	"github.com/unarbos/arbos/web"
)

// runWeb is the browser door: one assembled host serving the control seam
// over WebSocket plus the embedded UI, at addr. Like -serve it is a
// long-lived body for the agent, so it carries the clock — the plan scheduler
// fires deferred tasks, standing obligations, and callbacks here.
func runWeb(cfg piwire.Config, dbPath, addr, dist string, approve bool) error {
	host, store, cleanup, err := assemble(cfg, dbPath, approve, false)
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

	// Jobs are keyed by workspace root (= this process's cwd, same as the
	// toolset); the ✕ on a running job in the UI lands here.
	cwd, _ := os.Getwd()
	gw := &gateway.Server{
		Engine:       host.Engine,
		Store:        store,
		NewSessionID: piwire.NewSessionID,
		Drain:        cfg.ServeDrainTimeout,
		Model:        cfg.Model,
		ModelsURL:    cfg.ModelsURL(),
		Commands:     func() []gateway.CommandInfo { return slashCommands(cwd) },
		// The surface panels (show's canvases, code, docs) read workspace
		// files back through the gateway, rooted where the tools run.
		Root: cwd,
		KillJob: func(id string) error {
			_, err := codingspec.KillJob(cwd, id)
			return err
		},
		// The composer's mic button: capture + transcribe from this machine's
		// own microphone, on-device.
		Voice: &hostVoice{},
	}
	if dist != "" {
		// Override the embedded bundle with a directory (UI development).
		gw.Dist = os.DirFS(dist)
	} else if sub, err := fs.Sub(web.Dist, "dist"); err == nil {
		gw.Dist = sub
	}

	// The browser door on the outbox: scheduled firings and finished
	// background work reach open tabs as ambient notices.
	go gw.DeliverOutbox(ctx)

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

// slashCommands maps the agent's prompt templates to the gateway's wire shape
// for the composer's popup. Re-read per call so a freshly added prompt file
// shows up without a restart; expansion itself happens in the engine at
// projection time, never here.
func slashCommands(cwd string) []gateway.CommandInfo {
	ts := pi.LoadPromptTemplates(cwd, piwire.AgentConfigDir())
	out := make([]gateway.CommandInfo, 0, len(ts))
	for _, t := range ts {
		out = append(out, gateway.CommandInfo{
			Name:         t.Name,
			Description:  t.Description,
			ArgumentHint: t.ArgumentHint,
		})
	}
	return out
}

// displayAddr turns ":8420" into a clickable "localhost:8420".
func displayAddr(addr string) string {
	if addr != "" && addr[0] == ':' {
		return "localhost" + addr
	}
	return addr
}
