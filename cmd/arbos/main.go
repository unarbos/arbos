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

	"github.com/charmbracelet/x/term"

	"github.com/unarbos/arbos/internal/agent/pi"
	"github.com/unarbos/arbos/internal/control"
	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/envfile"
	"github.com/unarbos/arbos/internal/obs"
	"github.com/unarbos/arbos/internal/piwire"
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
	host, cleanup, err := assemble(cfg, dbPath, approve, false)
	if err != nil {
		return err
	}
	defer cleanup()

	// A keyless host serves the deterministic fake; warn so a misconfigured
	// headless deployment is loud instead of emitting fake transcripts.
	cfg.WarnIfNoLLM(os.Stderr)

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	return control.Serve(ctx, host.Engine, os.Stdin, os.Stdout, piwire.NewSessionID, cfg.ServeDrainTimeout)
}

func runOneShot(cfg piwire.Config, dbPath, task, session string, approve, once bool) error {
	host, cleanup, err := assemble(cfg, dbPath, approve, true)
	if err != nil {
		return err
	}
	defer cleanup()

	cfg.WarnIfNoLLM(os.Stderr)

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	cwd, _ := os.Getwd()
	templates := pi.LoadPromptTemplates(cwd, piwire.AgentConfigDir())
	expand := func(s string) string { return pi.ExpandPromptTemplate(s, templates) }
	return oneShot(ctx, host.Engine, expand(task), session, once, expand)
}

// assemble builds the host. When quiet, diagnostics go to a debug file instead
// of the console so an interactive one-shot run shows only its transcript; the
// headless serve seam stays verbose on stderr.
func assemble(cfg piwire.Config, dbPath string, approve, quiet bool) (*piwire.Host, func(), error) {
	store, err := piwire.OpenStore(dbPath)
	if err != nil {
		return nil, nil, fmt.Errorf("open store: %w", err)
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
		return nil, nil, err
	}
	cleanup := func() {
		host.Cleanup()
		_ = store.Close()
	}
	return host, cleanup, nil
}

func oneShot(ctx context.Context, eng *engine.Engine, task, session string, once bool, expand func(string) string) error {
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

	r.header(string(conv.ID()))
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
		first, err := readFollowUp(ctx, r, in)
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
			if !followUps {
				return nil
			}
			next, err := readFollowUp(ctx, r, in)
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

// readFollowUp prompts for the next task in the session. It returns "" when
// the user is done: EOF (ctrl-d), an exit word, or a cancelled context
// (ctrl-c). Blank lines re-prompt rather than exit, so a stray enter never
// kills the session.
func readFollowUp(ctx context.Context, r uiRenderer, in *stdinReader) (string, error) {
	for {
		r.promptFollowUp()
		line, err := in.read(ctx)
		if err != nil {
			fmt.Fprintln(os.Stderr)
			return "", err
		}
		switch line = strings.TrimSpace(line); line {
		case "":
			continue
		case "exit", "quit", "q":
			return "", nil
		default:
			return line, nil
		}
	}
}
