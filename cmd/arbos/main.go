// Command arbos is the pi coding agent. Run with a task (`arbos fix the
// tests`) or bare for an interactive session — both use the same live
// terminal renderer and prompt for follow-ups. Pass -once for a single
// turn; pass -serve for the headless JSON-lines control seam.
//
// Install:
//
//	go install github.com/unarbos/arbos/cmd/arbos@latest
//
// Use (OpenRouter):
//
//	export OPENROUTER_API_KEY=sk-or-...
//	arbos
package main

import (
	"bufio"
	"context"
	"flag"
	"fmt"
	"io"
	"os"
	"os/signal"
	"strings"
	"syscall"
	"time"

	"github.com/charmbracelet/x/term"

	"github.com/unarbos/arbos/internal/agent/pi"
	"github.com/unarbos/arbos/internal/control"
	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/envfile"
	"github.com/unarbos/arbos/internal/obs"
	"github.com/unarbos/arbos/internal/outbox"
	"github.com/unarbos/arbos/internal/piwire"
	"github.com/unarbos/arbos/internal/sqlite"
)

func main() {
	envfile.LoadDefault(piwire.AgentConfigDir())

	var (
		serve   = flag.Bool("serve", false, "serve the control seam over stdio (JSON lines)")
		dbPath  = flag.String("db", piwire.DefaultDBPath(), "SQLite session store path")
		query   = flag.String("q", "", "one-shot query (non-interactive)")
		prompt  = flag.String("prompt", "", "one-shot query (alias for -q)")
		p       = flag.String("p", "", "one-shot query (alias for -q)")
		session = flag.String("session", "", "resume an existing session id (one-shot mode)")
		approve = flag.Bool("approve", false, "gate write/edit/bash tools behind y/N confirmation")
		once    = flag.Bool("once", false, "exit after a single turn instead of prompting for follow-ups")
	)
	flag.Parse()

	task := firstNonEmpty(*query, *prompt, *p)
	// Fold any trailing positional words into the task so bare invocations
	// like `arbos fix the tests` (and unquoted `arbos -p fix the tests`) work
	// without shell quoting. With no args at all, task stays empty and the
	// session opens at the follow-up prompt.
	if rest := strings.TrimSpace(strings.Join(flag.Args(), " ")); rest != "" {
		if task == "" {
			task = rest
		} else {
			task = task + " " + rest
		}
	}

	cfg := piwire.LoadConfig()
	if err := run(cfg, *serve, *dbPath, task, *session, *approve, *once); err != nil {
		fmt.Fprintln(os.Stderr, "arbos:", err)
		os.Exit(1)
	}
}

func firstNonEmpty(vals ...string) string {
	for _, v := range vals {
		if v != "" {
			return v
		}
	}
	return ""
}

func run(cfg piwire.Config, serve bool, dbPath, task, session string, approve, once bool) error {
	if serve {
		return runServe(cfg, dbPath, approve)
	}
	return runOneShot(cfg, dbPath, task, session, approve, once)
}

func runServe(cfg piwire.Config, dbPath string, approve bool) error {
	host, _, cleanup, err := assemble(cfg, dbPath, approve, false)
	if err != nil {
		return err
	}
	defer cleanup()

	// A keyless host serves the deterministic fake; warn so a misconfigured
	// headless deployment is loud instead of emitting fake transcripts.
	cfg.WarnIfNoLLM(os.Stderr)

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	// The serve host is the agent's body: it owns the clock, so time-armed
	// plan nodes (deferred tasks, standing obligations) fire here. One-shot
	// runs have no clock — their projection still marks due nodes for any
	// running turn (pull mode).
	if stopSched := host.StartPlanScheduler(); stopSched != nil {
		defer stopSched()
	}

	return control.Serve(ctx, host.Engine, os.Stdin, os.Stdout, piwire.NewSessionID, cfg.ServeDrainTimeout)
}

func runOneShot(cfg piwire.Config, dbPath, task, session string, approve, once bool) error {
	host, store, cleanup, err := assemble(cfg, dbPath, approve, true)
	if err != nil {
		return err
	}
	defer cleanup()

	cfg.WarnIfNoLLM(os.Stderr)

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	cwd, _ := os.Getwd()
	interactive := term.IsTerminal(os.Stderr.Fd())
	// The front door: a bare interactive `arbos` greets with the brief —
	// what happened since you left, what waits on you, what is open — from
	// pure store reads, before any session starts.
	if task == "" && interactive {
		printBrief(ctx, store, os.Stderr)
	}

	// An interactive session is a long-lived process, so it carries the clock
	// too: deferred tasks and standing obligations fire here into fresh
	// executor sessions, and anything they must tell the user lands in the
	// outbox, which this terminal drains at quiet moments.
	if interactive {
		if stopSched := host.StartPlanScheduler(); stopSched != nil {
			defer stopSched()
		}
	}

	templates := pi.LoadPromptTemplates(cwd, piwire.AgentConfigDir())
	expand := func(s string) string { return pi.ExpandPromptTemplate(s, templates) }
	return oneShot(ctx, host.Engine, store, expand(task), session, once, expand)
}

// assemble builds the host. When quiet, diagnostics go to a debug file instead
// of the console so an interactive one-shot run shows only its transcript; the
// headless serve seam stays verbose on stderr. The store is returned alongside
// the host for read-only frontend projections (the brief and handoff).
func assemble(cfg piwire.Config, dbPath string, approve, quiet bool) (*piwire.Host, *sqlite.Store, func(), error) {
	store, err := piwire.OpenStore(dbPath)
	if err != nil {
		return nil, nil, nil, fmt.Errorf("open store: %w", err)
	}
	logger := piwire.NewLogger()
	if quiet {
		logger = piwire.NewQuietLogger()
	}
	host, err := piwire.Assemble(piwire.HostConfig{
		Config:   cfg,
		Store:    store,
		Observer: obs.NewSlogObserver(logger),
		Approve:  approve,
		Logger:   logger,
	})
	if err != nil {
		_ = store.Close()
		return nil, nil, nil, err
	}
	cleanup := func() {
		host.Cleanup()
		_ = store.Close()
	}
	return host, store, cleanup, nil
}

func oneShot(ctx context.Context, eng *engine.Engine, store *sqlite.Store, task, session string, once bool, expand func(string) string) error {
	id := piwire.NewSessionID()
	if session != "" {
		id = core.SessionID(session)
	}
	conv, err := eng.StartSession(ctx, id)
	if err != nil {
		return err
	}
	var r uiRenderer = newRenderer(os.Stdout, os.Stderr)
	if term.IsTerminal(os.Stderr.Fd()) {
		width, _, err := term.GetSize(os.Stderr.Fd())
		if err != nil {
			width = 80
		}
		r = newLiveRenderer(os.Stdout, os.Stderr, width)
	}
	defer r.close()
	// Keep the session open for follow-ups when a human is on the other end;
	// -once or piped stdio means a single turn.
	followUps := !once && term.IsTerminal(os.Stdin.Fd()) && term.IsTerminal(os.Stderr.Fd())

	cwd, _ := os.Getwd()
	// refresh recomputes the pinned footer (the plan/schedule glance) from
	// cheap store and on-disk reads. Called at quiet moments — session start,
	// turn boundaries, before each prompt — so the strip stays current without
	// a model call and without disturbing a turn in flight.
	refresh := func() { r.setFooter(footerLines(ctx, store, cwd)) }

	// deliver drains the outbox through this terminal door: claimed messages
	// print as ambient notice lines, only ever at quiet moments (session
	// start, turn boundaries, idle at the prompt) — never mid-stream.
	deliver := func() bool {
		if store == nil {
			return false
		}
		msgs, err := store.ClaimOutbox(ctx, outbox.ViaTerminal)
		if err != nil || len(msgs) == 0 {
			return false
		}
		lines := make([]string, len(msgs))
		for i, m := range msgs {
			lines[i] = m.Text
		}
		r.notice(lines)
		return true
	}

	r.header(string(conv.ID()))
	refresh()
	deliver()
	if task == "" && !followUps {
		// Piped invocation with no argv task: the pipe is the task. Consume
		// stdin fully before the line reader takes ownership of it.
		data, _ := io.ReadAll(os.Stdin)
		task = strings.TrimSpace(string(data))
		if task == "" {
			return fmt.Errorf("no task: pass one as arguments or pipe it on stdin")
		}
		task = expand(task)
	}
	in := startStdinReader()
	if task == "" {
		first, err := readFollowUp(ctx, r, in, deliver, refresh)
		if err != nil || first == "" {
			return nil
		}
		task = expand(first)
	}
	conv.Send(core.PromptIntent{Text: task})
	for env := range conv.Events() {
		// Relayed child events (Depth > 0) contribute tool activity only.
		// Child prose is internal narration — printing it interleaves
		// parallel children's text mid-word — and a child's turn ending
		// must not end the root turn.
		child := env.Depth > 0
		switch e := env.Event.(type) {
		case core.MessageDelta:
			if !child {
				r.delta(e.Text)
			}
		case core.ApprovalRequest:
			r.approvalPrompt(e.Call)
			line, _ := in.read(ctx)
			approved := strings.EqualFold(strings.TrimSpace(line), "y")
			conv.Send(core.ApprovalResponseIntent{RequestID: e.RequestID, Approved: approved})
		case core.ToolStarted:
			r.toolStart(e.Call)
		case core.ToolFinished:
			r.toolFinish(e.Result)
		case core.TurnComplete:
			if child {
				continue
			}
			r.turnComplete(e.StopReason)
			refresh()
			deliver()
			if !followUps {
				return nil
			}
			next, err := readFollowUp(ctx, r, in, deliver, refresh)
			if err != nil || next == "" {
				return nil
			}
			conv.Send(core.PromptIntent{Text: expand(next)})
		case core.Interrupted:
			if !child {
				r.interrupted()
				return nil
			}
		case core.ErrorEvent:
			if !child {
				r.errorf(e)
				return fmt.Errorf("%s: %s", e.Category, e.Err)
			}
		}
	}
	return nil
}

// stdinReader is the single owner of stdin for the whole session, so the
// follow-up prompt and mid-turn approval prompts never race for lines.
type stdinReader struct {
	lines chan string
	errs  chan error
}

func startStdinReader() *stdinReader {
	in := &stdinReader{lines: make(chan string), errs: make(chan error, 1)}
	go func() {
		br := bufio.NewReader(os.Stdin)
		for {
			line, err := br.ReadString('\n')
			if err != nil {
				in.errs <- err
				return
			}
			in.lines <- line
		}
	}()
	return in
}

// read returns the next stdin line, or an error on EOF or context cancel.
func (in *stdinReader) read(ctx context.Context) (string, error) {
	select {
	case <-ctx.Done():
		return "", ctx.Err()
	case err := <-in.errs:
		return "", err
	case line := <-in.lines:
		return line, nil
	}
}

// outboxPoll is how often an idle prompt checks for outbox messages. The
// agent's voice should feel prompt without the terminal feeling busy.
const outboxPoll = 2 * time.Second

// readFollowUp prompts for the next task in the session. It returns "" when
// the user is done: EOF (ctrl-d), an exit word, or a cancelled context
// (ctrl-c). Blank lines re-prompt rather than exit, so a stray enter never
// kills the session. While idle it polls deliver: an outbox message prints
// as an ambient notice and the prompt redraws beneath it — the agent speaking
// up between turns without ever owning one.
func readFollowUp(ctx context.Context, r uiRenderer, in *stdinReader, deliver func() bool, refresh func()) (string, error) {
	for {
		if refresh != nil {
			refresh() // pin a current schedule glance above each prompt
		}
		r.promptFollowUp()
	wait:
		for {
			select {
			case <-ctx.Done():
				fmt.Fprintln(os.Stderr)
				return "", ctx.Err()
			case err := <-in.errs:
				fmt.Fprintln(os.Stderr)
				return "", err
			case line := <-in.lines:
				switch line = strings.TrimSpace(line); line {
				case "":
					break wait // re-prompt
				case "exit", "quit", "q":
					return "", nil
				default:
					// Tear down the footer+prompt region and keep the typed
					// line as a permanent record before the turn begins.
					r.commitPrompt(line)
					return line, nil
				}
			case <-time.After(outboxPoll):
				if deliver != nil && deliver() {
					break wait // redraw footer + prompt beneath the notice
				}
			}
		}
	}
}
