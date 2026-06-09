// Command arbos is the kernel host for the pi coding agent. It runs a one-shot
// prompt or serves the headless control seam over stdio. With no LLM endpoint
// configured it uses the deterministic fake provider against the real pi
// toolset, so `go run ./cmd/arbos` works with zero setup.
//
// Live LLM via OpenRouter (doppler):
//
//	doppler run --project arbos --config dev -- env ARBOS_MODEL=google/gemini-2.5-flash ./cmd/arbos
//
// OpenRouter is auto-selected when OPENROUTER_API_KEY is set.
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
)

func main() {
	var (
		serve   = flag.Bool("serve", false, "serve the control seam over stdio (JSON lines)")
		dbPath  = flag.String("db", piwire.DefaultDBPath(), "SQLite session store path")
		prompt  = flag.String("prompt", "list files in the workspace", "one-shot prompt (ignored with -serve)")
		session = flag.String("session", "", "resume an existing session id (one-shot mode)")
		approve = flag.Bool("approve", false, "gate write/edit/bash tools behind y/N confirmation (default off = full privileges)")
	)
	flag.Parse()

	if err := run(*serve, *dbPath, *prompt, *session, *approve); err != nil {
		fmt.Fprintln(os.Stderr, "arbos:", err)
		os.Exit(1)
	}
}

func run(serve bool, dbPath, prompt, session string, approve bool) error {
	store, err := piwire.OpenStore(dbPath)
	if err != nil {
		return fmt.Errorf("open store: %w", err)
	}
	defer func() { _ = store.Close() }()

	logger := piwire.NewLogger()
	host, err := piwire.Assemble(piwire.HostConfig{
		Store:    store,
		Observer: obs.NewSlogObserver(logger),
		Approve:  approve,
	})
	if err != nil {
		return err
	}
	defer host.Cleanup()

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	if serve {
		return control.Serve(ctx, host.Engine, os.Stdin, os.Stdout, piwire.NewSessionID)
	}
	cwd, _ := os.Getwd()
	prompt = pi.ExpandPromptTemplate(prompt, pi.LoadPromptTemplates(cwd, piwire.AgentConfigDir()))
	return oneShot(ctx, host.Engine, prompt, session)
}

func oneShot(ctx context.Context, eng *engine.Engine, prompt, session string) error {
	id := piwire.NewSessionID()
	if session != "" {
		id = core.SessionID(session)
	}
	conv, err := eng.StartSession(ctx, id)
	if err != nil {
		return err
	}
	fmt.Printf("session %s\nprompt: %q\n\n", conv.ID(), prompt)
	stdin := bufio.NewReader(os.Stdin)
	conv.Send(core.PromptIntent{Text: prompt})
	for env := range conv.Events() {
		switch e := env.Event.(type) {
		case core.MessageDelta:
			fmt.Print(e.Text)
		case core.ApprovalRequest:
			fmt.Printf("\n  approve %s(%s)? [y/N] ", e.Call.Name, string(e.Call.Args))
			line, _ := stdin.ReadString('\n')
			approved := strings.EqualFold(strings.TrimSpace(line), "y")
			conv.Send(core.ApprovalResponseIntent{RequestID: e.RequestID, Approved: approved})
		case core.ToolStarted:
			fmt.Printf("\n  -> %s(%s)\n", e.Call.Name, string(e.Call.Args))
		case core.ToolFinished:
			fmt.Printf("  <- %s\n", e.Result.Content)
		case core.TurnComplete:
			fmt.Printf("\n\n[%s]\n", e.StopReason)
			return nil
		case core.Interrupted:
			fmt.Printf("\n\n[interrupted]\n")
			return nil
		case core.ErrorEvent:
			return fmt.Errorf("%s: %s", e.Category, e.Err)
		}
	}
	return nil
}
