// Command arbos is the pi coding agent. Run with no arguments for an
// interactive session; pass -q for a one-shot task; pass -serve for the
// headless JSON-lines control seam.
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
	"os"
	"os/signal"
	"strings"
	"syscall"

	"github.com/unarbos/arbos/internal/agent/pi"
	"github.com/unarbos/arbos/internal/control"
	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/obs"
	"github.com/unarbos/arbos/internal/piwire"
	"github.com/unarbos/arbos/internal/tui"
)

func main() {
	var (
		serve   = flag.Bool("serve", false, "serve the control seam over stdio (JSON lines)")
		dbPath  = flag.String("db", piwire.DefaultDBPath(), "SQLite session store path")
		query   = flag.String("q", "", "one-shot query (non-interactive)")
		prompt  = flag.String("prompt", "", "one-shot query (alias for -q)")
		session = flag.String("session", "", "resume an existing session id (one-shot mode)")
		approve = flag.Bool("approve", false, "gate write/edit/bash tools behind y/N confirmation")
	)
	flag.Parse()

	task := *query
	if task == "" {
		task = *prompt
	}

	if err := run(*serve, *dbPath, task, *session, *approve); err != nil {
		fmt.Fprintln(os.Stderr, "arbos:", err)
		os.Exit(1)
	}
}

func run(serve bool, dbPath, task, session string, approve bool) error {
	if serve {
		return runServe(dbPath, approve)
	}
	if task != "" {
		return runOneShot(dbPath, task, session, approve)
	}
	return tui.Run(tui.Options{DBPath: dbPath, Approve: approve})
}

func runServe(dbPath string, approve bool) error {
	host, cleanup, err := assemble(dbPath, approve)
	if err != nil {
		return err
	}
	defer cleanup()

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	return control.Serve(ctx, host.Engine, os.Stdin, os.Stdout, piwire.NewSessionID)
}

func runOneShot(dbPath, task, session string, approve bool) error {
	host, cleanup, err := assemble(dbPath, approve)
	if err != nil {
		return err
	}
	defer cleanup()

	piwire.WarnIfNoLLM(os.Stderr)

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	cwd, _ := os.Getwd()
	task = pi.ExpandPromptTemplate(task, pi.LoadPromptTemplates(cwd, piwire.AgentConfigDir()))
	return oneShot(ctx, host.Engine, task, session)
}

func assemble(dbPath string, approve bool) (*piwire.Host, func(), error) {
	store, err := piwire.OpenStore(dbPath)
	if err != nil {
		return nil, nil, fmt.Errorf("open store: %w", err)
	}
	logger := piwire.NewLogger()
	host, err := piwire.Assemble(piwire.HostConfig{
		Store:    store,
		Observer: obs.NewSlogObserver(logger),
		Approve:  approve,
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

func oneShot(ctx context.Context, eng *engine.Engine, task, session string) error {
	id := piwire.NewSessionID()
	if session != "" {
		id = core.SessionID(session)
	}
	conv, err := eng.StartSession(ctx, id)
	if err != nil {
		return err
	}
	fmt.Fprintf(os.Stderr, "session %s\n", conv.ID())
	conv.Send(core.PromptIntent{Text: task})
	stdin := bufio.NewReader(os.Stdin)
	for env := range conv.Events() {
		switch e := env.Event.(type) {
		case core.MessageDelta:
			fmt.Print(e.Text)
		case core.ApprovalRequest:
			fmt.Fprintf(os.Stderr, "\n  approve %s(%s)? [y/N] ", e.Call.Name, string(e.Call.Args))
			line, _ := stdin.ReadString('\n')
			approved := strings.EqualFold(strings.TrimSpace(line), "y")
			conv.Send(core.ApprovalResponseIntent{RequestID: e.RequestID, Approved: approved})
		case core.ToolStarted:
			fmt.Fprintf(os.Stderr, "\n  -> %s(%s)\n", e.Call.Name, string(e.Call.Args))
		case core.ToolFinished:
			fmt.Fprintf(os.Stderr, "  <- %s\n", e.Result.Content)
		case core.TurnComplete:
			fmt.Fprintln(os.Stderr, "\n["+string(e.StopReason)+"]")
			return nil
		case core.Interrupted:
			fmt.Fprintln(os.Stderr, "\n[interrupted]")
			return nil
		case core.ErrorEvent:
			return fmt.Errorf("%s: %s", e.Category, e.Err)
		}
	}
	return nil
}
