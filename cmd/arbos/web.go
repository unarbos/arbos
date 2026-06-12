package main

import (
	"context"
	"errors"
	"fmt"
	"io/fs"
	"net"
	"net/http"
	"net/url"
	"os"
	"os/signal"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"syscall"

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

	// Graceful self-restart: arbos is safe to rebuild in place while it runs.
	// The watcher stats this process's own executable; when the file on disk
	// is replaced — `arbos upgrade`, the dev loop, or the agent editing its
	// own source and running `go build` — it re-execs the new binary at the
	// next idle turn boundary, never mid-turn. Always on: it costs one stat
	// per poll and does nothing until the binary actually changes. The
	// watcher re-execs this same process in place, so it shares the signal
	// ctx and stops with it.
	//
	// ARBOS_EXE advertises the watched path to every child process (tool
	// shells inherit this environment), so `arbos upgrade` run from inside an
	// agent turn replaces the server that spawned it — not whichever arbos
	// happens to be first on PATH. Set on our own environment rather than
	// per-child: it is not a secret, and the re-exec passes os.Environ()
	// through, so the value stays correct across swaps.
	if exe, err := os.Executable(); err == nil {
		os.Setenv("ARBOS_EXE", exe)
	}
	go host.Engine.WatchRestart(ctx, engine.RestartConfig{
		Logf: func(f string, a ...any) { fmt.Fprintf(os.Stderr, "arbos "+f+"\n", a...) },
	})

	// The managed-secret vault behind the Settings tab — the same instance
	// LoadConfig opened for the provider's key chain, so a key saved here is
	// resolvable by the very next LLM request. Having it wires the bash
	// toolset's env injection (codingspec.SetEnvSource) so opted-in secrets
	// reach tool subprocesses without ever entering this process's own
	// environment, and the fetch tool's broker (SetSecretApplier) so
	// host-allowlisted secrets attach to outbound HTTPS without the agent
	// ever holding the value. A missing vault is non-fatal — the web door
	// still serves, just without the secrets surface.
	secrets := cfg.Vault
	if secrets == nil {
		fmt.Fprintf(os.Stderr, "arbos: secret vault unavailable — secrets settings disabled\n")
	} else {
		vault := secrets
		codingspec.SetEnvSource(vault.EnvValues)
		codingspec.SetSecretApplier(func(ctx context.Context, name string, req *http.Request) error {
			// Bindings are rebuilt per call so a secret added in the
			// Settings tab mid-session is attachable immediately.
			br := secret.NewBroker(vault, vault.Bindings(secret.BearerInjector)...)
			return br.Apply(ctx, core.SecretRef{Name: name}, req)
		})
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
		// The Settings tab's provider panel (endpoint, key, credits).
		LLM: llmAdmin(cfg, host, secrets),
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

	// Bind before anything prints a URL: when the requested port is taken
	// (another arbos on this machine), walk forward to a free one, so the
	// login lines below always carry the port that actually answers.
	ln, boundAddr, err := listenWeb(addr)
	if err != nil {
		return fmt.Errorf("web listen: %w", err)
	}
	addr = boundAddr

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
		Handler:     gw.Handler(),
		BaseContext: func(net.Listener) context.Context { return ctx },
	}
	errCh := make(chan error, 1)
	go func() { errCh <- srv.Serve(ln) }()
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

// llmKeyName is the vault entry the provider panel's key saves under — the
// same name the install flow exports and LoadConfig's onboarding path
// resolves, so a pasted key and an exported one are one credential.
const llmKeyName = "OPENROUTER_API_KEY"

// llmAdmin wires the Settings tab's provider panel: read the effective
// endpoint/key state, persist changes (endpoint into the preference file,
// key into the vault), and apply them by scheduling a graceful self-restart —
// the new provider is rebuilt by LoadConfig's one assembly path at the next
// idle turn instead of hot-swapping half the host's organs. nil (routes
// disabled) when either backing store is unavailable.
func llmAdmin(cfg piwire.Config, host *piwire.Host, vault *secret.Store) *gateway.LLMAdmin {
	if host.Settings == nil || vault == nil {
		return nil
	}
	// effectiveEndpoint is where a saved key will be sent: the stored
	// endpoint when the user set one, else the boot-resolved base — except a
	// keyless default host, which LoadConfig onboards onto OpenRouter once a
	// key lands, so that is what the panel shows and binds to.
	effectiveEndpoint := func() string {
		if s := host.Settings.Get().LLMBaseURL; s != "" {
			return s
		}
		if !cfg.HasLLM && cfg.ProviderName == "openai" {
			return piwire.OpenRouterBase
		}
		return cfg.BaseURL
	}
	// keySet reads live (env or vault) rather than the boot snapshot, so the
	// panel reflects a just-saved key during the restart window.
	keySet := func() bool {
		if cfg.HasLLM {
			return true
		}
		for _, e := range vault.List() {
			if e.Name == llmKeyName {
				return true
			}
		}
		return false
	}
	credits := cfg.CreditsFetcher()
	admin := &gateway.LLMAdmin{
		Info: func() gateway.LLMInfo {
			return gateway.LLMInfo{
				Endpoint:   effectiveEndpoint(),
				Provider:   cfg.ProviderName,
				Model:      cfg.Model,
				KeySet:     keySet(),
				OpenRouter: credits != nil,
				// True while a saved change waits out a busy agent; the
				// gateway stamps BootID itself.
				RestartPending: host.Engine.RestartPending(),
			}
		},
		SetEndpoint: func(u string) error {
			cur := host.Settings.Get()
			cur.LLMBaseURL = u
			return host.Settings.Set(cur)
		},
		SetKey: func(v string) error {
			hostName := ""
			if u, err := url.Parse(effectiveEndpoint()); err == nil {
				hostName = u.Hostname()
			}
			return vault.Set(secret.Entry{
				Name:  llmKeyName,
				Label: "LLM API key (Settings → Model Provider)",
				Hosts: []string{hostName},
			}, v)
		},
		Apply: func() { host.Engine.RequestRestart() },
	}
	if credits != nil {
		admin.Credits = func(ctx context.Context) (gateway.LLMCredits, error) {
			ci, err := credits(ctx)
			return gateway.LLMCredits{TotalCredits: ci.TotalCredits, TotalUsage: ci.TotalUsage}, err
		}
	}
	return admin
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

// maxPortProbes bounds the walk past a busy port — room for a stack of
// instances on one box without scanning into someone else's range.
const maxPortProbes = 20

// listenWeb binds addr; when the port is taken it walks forward through the
// next maxPortProbes ports so several arbos instances coexist on one machine
// without flags. Returns the listener and the address actually bound, with
// the caller's host spelling preserved (":8420" stays host-less).
func listenWeb(addr string) (net.Listener, string, error) {
	ln, err := net.Listen("tcp", addr)
	if err == nil {
		return ln, addr, nil
	}
	if !errors.Is(err, syscall.EADDRINUSE) {
		return nil, "", err
	}
	host, portStr, splitErr := net.SplitHostPort(addr)
	if splitErr != nil {
		return nil, "", err
	}
	port, atoiErr := strconv.Atoi(portStr)
	if atoiErr != nil || port == 0 {
		return nil, "", err
	}
	for next := port + 1; next <= port+maxPortProbes && next <= 65535; next++ {
		nextAddr := net.JoinHostPort(host, strconv.Itoa(next))
		ln, lerr := net.Listen("tcp", nextAddr)
		if lerr == nil {
			fmt.Fprintf(os.Stderr, "arbos: port %d is in use — using %d\n", port, next)
			return ln, nextAddr, nil
		}
		if !errors.Is(lerr, syscall.EADDRINUSE) {
			return nil, "", lerr
		}
	}
	return nil, "", fmt.Errorf("ports %d-%d all in use: %w", port, port+maxPortProbes, err)
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
