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
	"fmt"
	"io"
	"os"
	"os/signal"
	"path/filepath"
	"strings"
	"syscall"
	"time"

	"github.com/charmbracelet/x/term"

	"github.com/unarbos/arbos/internal/control"
	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/envfile"
	"github.com/unarbos/arbos/internal/matrix"
	"github.com/unarbos/arbos/internal/obs"
	"github.com/unarbos/arbos/internal/outbox"
	"github.com/unarbos/arbos/internal/piwire"
	"github.com/unarbos/arbos/internal/plan"
	"github.com/unarbos/arbos/internal/ports"
	"github.com/unarbos/arbos/internal/sqlite"
)

func main() {
	envfile.LoadDefault(piwire.AgentConfigDir())
	if err := dispatch(os.Args[1:]); err != nil {
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
	host, _, _, cleanup, err := assemble(cfg, dbPath, approve, false, false)
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
	host, store, _, cleanup, err := assemble(cfg, dbPath, approve, true, false)
	if err != nil {
		return err
	}
	defer cleanup()

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	interactive := term.IsTerminal(os.Stderr.Fd())
	if !interactive {
		cfg.WarnIfNoLLM(os.Stderr)
	}
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

	return oneShot(ctx, host.Engine, store, cfg, task, session, approve, once)
}

// assemble builds the host. When quiet, diagnostics go to a debug file instead
// of the console so an interactive one-shot run shows only its transcript; the
// headless serve seam stays verbose on stderr. The store is returned alongside
// the host for read-only frontend projections (the brief and handoff).
func assemble(cfg piwire.Config, dbPath string, approve, quiet, wantMatrix bool) (*piwire.Host, *sqlite.Store, *matrix.Bridge, func(), error) {
	store, err := piwire.OpenStore(dbPath)
	if err != nil {
		return nil, nil, nil, nil, fmt.Errorf("open store: %w", err)
	}
	logger := piwire.NewLogger()
	if quiet {
		logger = piwire.NewQuietLogger()
	}

	// The engine's store is the durable sqlite log by default. With Matrix
	// enabled (ADR-0041), wrap it in the composite store so the embedded
	// homeserver mirrors Room-audience events into per-session rooms while the
	// sqlite log stays the authoritative local capture (which is also what the
	// gateway reads). The bridge is the gateway's separate read-handle onto the
	// same substrate (session->room resolution + homeserver coordinates); nil
	// when Matrix is off.
	//
	// Matrix is the default substrate for the web door (the multi-party door
	// every participant shares), so the operator runs on a real homeserver as
	// @human with the room as the shared mirror — keeping the local seam as the
	// D6 presentation channel (streaming/tools/approvals never federate). A
	// one-shot CLI run does not boot a homeserver (wantMatrix is false there);
	// ARBOS_MATRIX overrides either way ("0" forces off, "1" forces on).
	matrixOn := wantMatrix
	matrixForced := false // explicitly requested via ARBOS_MATRIX=1
	switch os.Getenv(matrixEnvFlag) {
	case "1":
		matrixOn = true
		matrixForced = true
	case "0":
		matrixOn = false
	}
	var engineStore ports.SessionStore = store
	var matrixBridge *matrix.Bridge
	var matrixShutdown func()
	if matrixOn {
		em, shutdown, merr := startEmbeddedMatrix(filepath.Dir(dbPath))
		switch {
		case merr != nil && matrixForced:
			// The operator asked for Matrix explicitly — a failure to start it
			// is fatal, not a silent downgrade.
			_ = store.Close()
			return nil, nil, nil, nil, fmt.Errorf("start embedded matrix: %w", merr)
		case merr != nil:
			// Default-on, but the embedded homeserver couldn't start — most
			// commonly its fixed loopback port is already held by another arbos
			// node on this machine (a second instance in a different directory,
			// each its own directory primary). Degrade to the local-only path
			// rather than refusing to boot: the web door still serves, just
			// without the shared Matrix substrate (ADR-0041 H3).
			fmt.Fprintf(os.Stderr, "arbos: embedded Matrix unavailable (%v) — serving without it\n", merr)
		default:
			// One guest registry, shared between the mirror (which reads it to
			// route a guest's messages as their own identity) and the bridge
			// (which writes it as it seats a guest in a shared chat's room).
			guests := matrix.NewGuestRegistry()
			// Outbound redaction twin (D12): scrub managed-secret values from the
			// copy mirrored to the room. Only when a vault is configured.
			var redact func(string) string
			if cfg.Vault != nil {
				redact = redactSecretsFn(cfg.Vault)
			}
			mstore := matrix.NewStore(store, matrix.NewClientMirror(em.agent, em.human, em.humanID, guests, redact))
			engineStore = mstore
			matrixBridge = matrix.NewBridge(em.agent, em.human, mstore, em.homeserverURL, em.agentID, em.humanID, guests, em.provision)
			matrixShutdown = shutdown
		}
	}

	host, err := piwire.Assemble(piwire.HostConfig{
		Config:   cfg,
		Store:    engineStore,
		Observer: obs.NewSlogObserver(logger),
		Approve:  approve,
		Logger:   logger,
	})
	if err != nil {
		if matrixShutdown != nil {
			matrixShutdown()
		}
		_ = store.Close()
		return nil, nil, nil, nil, err
	}
	cleanup := func() {
		host.Cleanup()
		if matrixShutdown != nil {
			matrixShutdown()
		}
		_ = store.Close()
	}
	return host, store, matrixBridge, cleanup, nil
}

func oneShot(ctx context.Context, eng *engine.Engine, store *sqlite.Store, cfg piwire.Config, task, session string, approve, once bool) error {
	id := piwire.NewSessionID()
	if session != "" {
		id = core.SessionID(session)
	}
	conv, err := eng.StartSession(ctx, id)
	if err != nil {
		return err
	}
	// Keep the session open for follow-ups when a human is on the other end;
	// -once or piped stdio means a single turn.
	followUps := !once && term.IsTerminal(os.Stdin.Fd()) && term.IsTerminal(os.Stderr.Fd())
	var r uiRenderer = newRenderer(os.Stdout, os.Stderr)
	var tui *tuiRenderer
	if followUps {
		width, _, err := term.GetSize(os.Stderr.Fd())
		if err != nil {
			width = 80
		}
		tui = newTUIRenderer(os.Stderr, width, newTUIMeta(cfg.Model, approve))
		r = tui
	}
	defer r.close()

	knownStanding := map[plan.NodeID]bool{}
	// announceStanding prints standing obligations once at session start, and
	// again only when a new recurring node appears mid-session. Spacing is the
	// renderer's job — a raw stderr write here would desync the TUI's block
	// accounting and orphan the prompt line.
	announceStanding := func(onlyNew bool) {
		if store == nil {
			return
		}
		open, err := store.OpenPlanNodes(ctx)
		if err != nil {
			return
		}
		lines := collectStandingLines(open, time.Now(), knownStanding, onlyNew)
		if len(lines) == 0 {
			return
		}
		r.printStanding(lines)
	}

	// deliver drains the outbox through this terminal door: claimed messages
	// print as ambient notice lines, only ever at quiet moments (session
	// start, turn boundaries, idle at the prompt) — never mid-stream. Scoped
	// to this session plus broadcast-class messages, so the terminal never
	// steals a notice belonging to another conversation; a notice for a
	// session this terminal isn't running waits, durably, for its own door
	// to reopen rather than surfacing here.
	deliver := func() bool {
		if store == nil {
			return false
		}
		msgs, err := store.ClaimOutboxFor(ctx, outbox.ViaTerminal, core.PrincipalLocal,
			[]string{string(conv.ID())})
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
	if followUps {
		announceStanding(false)
	}
	deliver()
	if task == "" && !followUps {
		// Piped invocation with no argv task: the pipe is the task. Consume
		// stdin fully before the line reader takes ownership of it.
		data, _ := io.ReadAll(os.Stdin)
		task = strings.TrimSpace(string(data))
		if task == "" {
			return fmt.Errorf("no task: pass one as arguments or pipe it on stdin")
		}
	}
	if followUps {
		return runTUISession(ctx, conv, tui, string(conv.ID()), deliver, announceStanding, task)
	}
	in := startStdinReader()
	if task == "" {
		first, err := readFollowUp(ctx, r, in, deliver)
		if err != nil || first == "" {
			return nil
		}
		task = first
	}
	conv.Send(core.PromptIntent{Text: task})
	r.turnStart()
	for env := range conv.Events() {
		done, err := handleSessionEvent(ctx, conv, r, in, env, nil, announceStanding, deliver, nil)
		if done {
			return err
		}
		if err != nil {
			return err
		}
	}
	return nil
}

// handleSessionEvent processes one engine envelope. done means the session
// should exit (one-shot turn complete, interrupt, or error).
func handleSessionEvent(ctx context.Context, conv *engine.Conversation, r uiRenderer, in *stdinReader, env core.Envelope, turnActive *bool, announceStanding func(bool), deliver func() bool, needPrompt *bool) (done bool, err error) {
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
	case core.QuestionRequest:
		// The ask tool: print each question, read one answer line per question.
		answers := make([]core.QuestionAnswer, 0, len(e.Questions))
		for i, q := range e.Questions {
			r.notice(questionLines(e, i))
			line, _ := in.read(ctx)
			answers = append(answers, parseQuestionAnswer(q, line))
		}
		conv.Send(core.QuestionResponseIntent{RequestID: e.RequestID, Answers: answers, Skipped: questionsSkipped(answers)})
	case core.ToolStarted:
		r.toolStart(e.Call)
	case core.ToolFinished:
		r.toolFinish(e.Result)
	case core.TurnComplete:
		if child {
			return false, nil
		}
		r.turnComplete(e.StopReason)
		announceStanding(true)
		deliver()
		if turnActive != nil {
			*turnActive = false
		}
		if needPrompt != nil {
			*needPrompt = true
			return false, nil
		}
		return true, nil
	case core.Interrupted:
		if !child {
			r.interrupted()
			return true, nil
		}
	case core.ErrorEvent:
		if !child {
			r.errorf(e)
			return true, fmt.Errorf("%s: %s", e.Category, e.Err)
		}
	}
	return false, nil
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
func readFollowUp(ctx context.Context, r uiRenderer, in *stdinReader, deliver func() bool) (string, error) {
	for {
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
					r.commitPrompt(line)
					return line, nil
				}
			case <-time.After(outboxPoll):
				if deliver != nil && deliver() {
					break wait // redraw the prompt beneath the notice
				}
			}
		}
	}
}
