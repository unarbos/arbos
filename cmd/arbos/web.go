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
	"path/filepath"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/unarbos/arbos/internal/agent/pi"
	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/forest"
	"github.com/unarbos/arbos/internal/gateway"
	"github.com/unarbos/arbos/internal/messenger"
	"github.com/unarbos/arbos/internal/piwire"
	"github.com/unarbos/arbos/internal/secret"
	"github.com/unarbos/arbos/internal/settings"
	"github.com/unarbos/arbos/internal/tool/codingspec"
	"github.com/unarbos/arbos/web"
)

// runWeb is the browser door: one assembled host serving the control seam
// over WebSocket plus the embedded UI, at addr. Like -serve it is a
// long-lived body for the agent, so it carries the clock — the plan scheduler
// fires deferred tasks, standing obligations, and callbacks here. A non-empty
// forestURL additionally joins a forest head and serves the same gateway
// (auth gate included) at the assigned public URL through an outbound tunnel.
func runWeb(cfg piwire.Config, dbPath, addr, dist, forestURL string, approve bool) error {
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

	// Graceful self-restart (dev loop, ADR self-edit safety): when a rebuild
	// touches the sentinel, swap to the new binary at the next idle turn
	// boundary instead of being pkill-ed mid-turn. Opt-in via env so it only
	// runs under scripts/dev.sh; the watcher re-execs this same process in
	// place, so it must share the signal ctx and stop with it.
	if sentinel := os.Getenv("ARBOS_RESTART_SENTINEL"); sentinel != "" {
		go host.Engine.WatchRestart(ctx, engine.RestartConfig{
			Sentinel: sentinel,
			Poll:     500 * time.Millisecond,
			Logf:     func(f string, a ...any) { fmt.Fprintf(os.Stderr, "arbos "+f+"\n", a...) },
		})
	}

	// The managed-secret vault behind the Settings tab. Opening it wires the
	// bash toolset's env injection (codingspec.SetEnvSource) so opted-in
	// secrets reach tool subprocesses without ever entering this process's
	// own environment, and the fetch tool's broker (SetSecretApplier) so
	// host-allowlisted secrets attach to outbound HTTPS without the agent
	// ever holding the value. A vault failure is non-fatal — the web door
	// still serves, just without the secrets surface.
	var secrets *secret.Store
	if dir := piwire.AgentConfigDir(); dir != "" {
		if vault, err := secret.Open(filepath.Join(dir, "secrets")); err != nil {
			fmt.Fprintf(os.Stderr, "arbos: secret vault unavailable: %v\n", err)
		} else {
			secrets = vault
			codingspec.SetEnvSource(vault.EnvValues)
			codingspec.SetSecretApplier(func(ctx context.Context, name string, req *http.Request) error {
				// Bindings are rebuilt per call so a secret added in the
				// Settings tab mid-session is attachable immediately.
				br := secret.NewBroker(vault, vault.Bindings(secret.BearerInjector)...)
				return br.Apply(ctx, core.SecretRef{Name: name}, req)
			})
		}
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
		// The terminal tab's tail poll: a job derived from the same on-disk
		// dirs the tools use, mapped to the gateway's snapshot shape.
		FindJob: func(id string) (gateway.JobSnapshot, error) {
			j, err := codingspec.FindJob(cwd, id)
			if err != nil {
				return gateway.JobSnapshot{}, err
			}
			return gateway.JobSnapshot{
				ID:          j.ID,
				Command:     j.Meta.Command,
				Cwd:         j.Meta.Cwd,
				Status:      string(j.Status),
				ExitCode:    j.ExitCode,
				JournalPath: j.JournalPath(),
			}, nil
		},
		// The composer's mic button: capture + transcribe from this machine's
		// own microphone, on-device.
		Voice: &hostVoice{},
		// Cloud audio (OpenRouter bases only): the mic's cross-platform
		// fallback + voice-memo transcription, and spoken responses.
		Transcribe: cfg.Transcriber(),
		Speak:      cfg.Speaker(),
		// The Settings tab's secrets CRUD. nil when the vault failed to open.
		Secrets: secretAdmin(secrets),
		// The Settings tab's agent knobs (subagent model). nil when the
		// preference file was unavailable at assembly.
		HostSettings: settingsAdmin(host.Settings),
	}
	if dist != "" {
		// Override the embedded bundle with a directory (UI development).
		gw.Dist = os.DirFS(dist)
	} else if sub, err := fs.Sub(web.Dist, "dist"); err == nil {
		gw.Dist = sub
	}

	// The auth gate (ADR-0034): a bind reachable beyond loopback — or any
	// forest join, whose tunnel is remote by definition — turns the cookie
	// gate on, not optionally. Loopback-only binds keep today's frictionless
	// behavior. Failure to load the signing key is fatal because serving
	// remotely without the gate would expose file write, PTY, and secrets.
	//
	// prints tracks the pieces of the login line that arrive at different
	// times: tokens rotate on every consumption, and the public URL exists
	// only once a forest join lands. Each event reprints a complete URL.
	var prints struct {
		sync.Mutex
		token  string
		public string
	}
	if remoteReachable(addr) || forestURL != "" {
		if piwire.AgentConfigDir() == "" {
			return fmt.Errorf("gateway auth: no home directory for the signing key")
		}
		key, err := gateway.LoadOrCreateAuthKey(
			filepath.Join(piwire.AgentConfigDir(), "identity", "gateway.key"))
		if err != nil {
			return fmt.Errorf("gateway auth key: %w", err)
		}
		login := loginAddr(addr)
		gw.Auth = &gateway.Auth{Key: key, OnToken: func(tok string) {
			prints.Lock()
			prints.token = tok
			public := prints.public
			prints.Unlock()
			fmt.Fprintf(os.Stderr, "arbos web login: http://%s/login?token=%s\n", login, tok)
			if public != "" {
				fmt.Fprintf(os.Stderr, "arbos forest login: %s/login?token=%s\n", public, tok)
			}
		}}
	}

	// The Telegram bridge behind the Messenger tab: registered bots poll for
	// inbound messages and bridge each chat to a kernel session. A failure to
	// load its state is non-fatal — the web door serves without the tab.
	if dir := piwire.AgentConfigDir(); dir != "" {
		msgr, err := messenger.New(messenger.Config{
			Dir:        filepath.Join(dir, "messenger"),
			Full:       host.Engine,
			Guest:      host.GuestEngine,
			Store:      store,
			Transcribe: cfg.Transcriber(),
			// The same templates the web composer's popup offers, published
			// to each bot's Telegram "/" menu.
			Commands: func() []messenger.Command {
				cmds := slashCommands(cwd)
				out := make([]messenger.Command, 0, len(cmds))
				for _, c := range cmds {
					out = append(out, messenger.Command{Name: c.Name, Description: c.Description})
				}
				return out
			},
			Logf: func(format string, args ...any) {
				fmt.Fprintf(os.Stderr, format+"\n", args...)
			},
		})
		if err != nil {
			fmt.Fprintf(os.Stderr, "arbos: messenger unavailable: %v\n", err)
		} else {
			msgr.Start(ctx)
			gw.Messenger = msgr
		}
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
	if gw.Auth != nil {
		gw.Auth.MintToken()
	}

	// Join the forest: register the device, lease a name, and serve this
	// same handler (gate and all) back through the outbound tunnel. The
	// client reconnects forever on its own; a head outage never takes the
	// local door down with it.
	if forestURL != "" {
		fc := &forest.Client{
			Base:    strings.TrimRight(forestURL, "/"),
			KeyPath: filepath.Join(piwire.AgentConfigDir(), "identity", "device.key"),
			Handler: gw.Handler(),
			OnJoin: func(j forest.JoinInfo) {
				prints.Lock()
				prints.public = j.URL
				tok := prints.token
				prints.Unlock()
				fmt.Fprintf(os.Stderr, "arbos forest: joined as %s — %s\n", j.Name, j.URL)
				if tok != "" {
					fmt.Fprintf(os.Stderr, "arbos forest login: %s/login?token=%s\n", j.URL, tok)
				}
			},
			Logf: func(format string, args ...any) {
				fmt.Fprintf(os.Stderr, format+"\n", args...)
			},
		}
		go func() { _ = fc.Run(ctx) }()
	}

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

// secretAdmin adapts the vault to the gateway's SecretStore seam, returning a
// genuinely nil interface (not an interface wrapping a nil pointer) when no
// vault opened, so the gateway's nil check disables the routes cleanly.
func secretAdmin(s *secret.Store) gateway.SecretStore {
	if s == nil {
		return nil
	}
	return s
}

// settingsAdmin does the same nil-interface dance for the host preference
// file behind the Settings tab's agent knobs.
func settingsAdmin(s *settings.Store) gateway.SettingsStore {
	if s == nil {
		return nil
	}
	return s
}

// slashCommands maps the agent's prompt templates to the gateway's wire shape
// for the composer's popup. Re-read per call so a freshly added prompt file
// shows up without a restart; expansion itself happens in the engine at
// projection time, never here.
func slashCommands(cwd string) []gateway.CommandInfo {
	ts := pi.LoadPromptTemplates(cwd, piwire.AgentConfigDir())
	out := make([]gateway.CommandInfo, 0, len(ts))
	for _, t := range ts {
		// Workspace-relative when under the root (show's normalization), so
		// the editor panel and a doc surface for the same file share a tab.
		path := t.Path
		if rel, err := filepath.Rel(cwd, t.Path); err == nil && !strings.HasPrefix(rel, "..") {
			path = rel
		}
		out = append(out, gateway.CommandInfo{
			Name:         t.Name,
			Description:  t.Description,
			ArgumentHint: t.ArgumentHint,
			Path:         path,
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

// remoteReachable reports whether a bind address accepts connections from
// beyond loopback — the line where the auth gate becomes mandatory
// (ADR-0034). ":8420" and "0.0.0.0:8420" bind every interface; anything
// unparseable is treated as reachable so a mistake fails closed.
func remoteReachable(addr string) bool {
	host, _, err := net.SplitHostPort(addr)
	if err != nil {
		return true
	}
	if strings.EqualFold(host, "localhost") {
		return false
	}
	ip := net.ParseIP(host)
	if host == "" || ip == nil {
		return true
	}
	return !ip.IsLoopback()
}

// loginAddr picks the host:port for the printed login URL: the bind host when
// concrete, else the machine's first non-loopback IPv4 — the best guess at
// how a remote browser reaches this box — else localhost.
func loginAddr(addr string) string {
	host, port, err := net.SplitHostPort(addr)
	if err != nil {
		return addr
	}
	if ip := net.ParseIP(host); host != "" && (ip == nil || !ip.IsUnspecified()) {
		return net.JoinHostPort(host, port)
	}
	for _, a := range interfaceAddrs() {
		ipn, ok := a.(*net.IPNet)
		if !ok {
			continue
		}
		if ip := ipn.IP.To4(); ip != nil && !ip.IsLoopback() {
			return net.JoinHostPort(ip.String(), port)
		}
	}
	return net.JoinHostPort("localhost", port)
}

// interfaceAddrs wraps net.InterfaceAddrs so an enumeration failure just
// falls through to the localhost guess.
func interfaceAddrs() []net.Addr {
	addrs, err := net.InterfaceAddrs()
	if err != nil {
		return nil
	}
	return addrs
}
