package main

import (
	"context"
	"flag"
	"fmt"
	"io"
	"os"
	"strings"
	"text/tabwriter"

	"github.com/unarbos/arbos/internal/forest"
	"github.com/unarbos/arbos/internal/piwire"
	"github.com/unarbos/arbos/internal/theme"
)

const helpName = "arbos"

type cliConfig struct {
	version   bool
	help      bool
	print     bool
	query     string
	prompt    string
	session   string
	continue_ bool
	model     string
	theme     string
	approve   bool
	once      bool
	serve     bool
	web       string
	webDist   string
	forest    string
	db        string
	workspace string
}

func parseCLI(args []string) (cliConfig, []string, error) {
	args = normalizeLongFlags(args)
	fs := flag.NewFlagSet(helpName, flag.ContinueOnError)
	fs.SetOutput(io.Discard)

	var cfg cliConfig
	fs.BoolVar(&cfg.version, "version", false, "")
	fs.BoolVar(&cfg.version, "v", false, "")
	fs.BoolVar(&cfg.help, "help", false, "")
	fs.BoolVar(&cfg.help, "h", false, "")
	fs.BoolVar(&cfg.print, "print", false, "")
	fs.BoolVar(&cfg.print, "p", false, "")
	fs.StringVar(&cfg.query, "query", "", "")
	fs.StringVar(&cfg.query, "q", "", "")
	fs.StringVar(&cfg.prompt, "prompt", "", "")
	fs.StringVar(&cfg.session, "session", "", "")
	fs.BoolVar(&cfg.continue_, "continue", false, "")
	fs.StringVar(&cfg.model, "model", "", "")
	fs.StringVar(&cfg.theme, "theme", "", "")
	fs.BoolVar(&cfg.approve, "approve", false, "")
	fs.BoolVar(&cfg.once, "once", false, "")
	fs.BoolVar(&cfg.serve, "serve", false, "")
	fs.StringVar(&cfg.web, "web", "", "")
	fs.StringVar(&cfg.webDist, "web-dist", "", "")
	fs.StringVar(&cfg.forest, "forest", "", "")
	fs.StringVar(&cfg.db, "db", piwire.DefaultDBPath(), "")
	fs.StringVar(&cfg.workspace, "workspace", "", "")

	if err := fs.Parse(args); err != nil {
		return cfg, nil, err
	}
	return cfg, fs.Args(), nil
}

func normalizeLongFlags(args []string) []string {
	out := make([]string, 0, len(args))
	for _, a := range args {
		if a == "--" {
			out = append(out, a)
			continue
		}
		if strings.HasPrefix(a, "--") {
			a = "-" + a[1:]
		}
		out = append(out, a)
	}
	return out
}

func dispatch(args []string) error {
	cfg, rest, err := parseCLI(args)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		printUsage(os.Stderr)
		return err
	}

	if cfg.version {
		fmt.Println(buildVersion())
		return nil
	}
	if cfg.help && len(rest) == 0 {
		printUsage(os.Stdout)
		return nil
	}
	if len(rest) > 0 && isCommand(rest[0]) {
		switch rest[0] {
		case "help":
			printCommandHelp(os.Stdout, rest[1:])
			return nil
		case "ls":
			return runListSessions(cfg, rest[1:])
		case "resume":
			return runResume(cfg, rest[1:])
		case "export":
			return runExport(cfg, rest[1:])
		case "web":
			return runWebCommand(cfg, rest[1:])
		case "upgrade":
			return runUpgrade(rest[1:])
		case "changelog":
			return runChangelog(rest[1:])
		default:
			return fmt.Errorf("unknown command %q", rest[0])
		}
	}

	if cfg.workspace != "" {
		if err := os.Chdir(cfg.workspace); err != nil {
			return fmt.Errorf("workspace: %w", err)
		}
	}
	if cfg.model != "" {
		_ = os.Setenv("ARBOS_MODEL", cfg.model)
	}
	if cfg.theme != "" {
		_ = os.Setenv("ARBOS_THEME", cfg.theme)
		theme.Apply(cfg.theme)
	}

	task := firstNonEmpty(cfg.query, cfg.prompt)
	if tail := strings.TrimSpace(strings.Join(rest, " ")); tail != "" {
		if task == "" {
			task = tail
		} else {
			task = task + " " + tail
		}
	}

	session := cfg.session
	if cfg.continue_ && session == "" {
		id, err := latestSessionID(cfg.db)
		if err != nil {
			return err
		}
		if id == "" {
			return fmt.Errorf("no sessions to continue")
		}
		session = id
	}

	piCfg := piwire.LoadConfig()
	if cfg.forest != "" && cfg.web == "" {
		return fmt.Errorf("--forest requires --web (the forest exposes the web gateway)")
	}
	if cfg.web != "" {
		return runWeb(piCfg, cfg.db, cfg.web, cfg.webDist, cfg.forest, cfg.approve)
	}
	return run(piCfg, cfg.serve, cfg.db, task, session, cfg.approve, cfg.once || cfg.print)
}

func isCommand(s string) bool {
	switch s {
	case "ls", "resume", "export", "help", "web", "upgrade", "changelog":
		return true
	default:
		return false
	}
}

// runWebCommand is `arbos web [addr]` — the canonical front door. It binds
// loopback by default and joins the default forest, so one command on any
// machine yields a public, auth-gated URL to visit immediately. `--local`
// keeps the node off the forest; `--forest <url>` points it at another head.
// The bare `--web <addr>` flag stays the un-opinionated primitive (no forest
// unless asked); this subcommand is the path with batteries included.
func runWebCommand(cfg cliConfig, args []string) error {
	fs := flag.NewFlagSet("web", flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	var local bool
	forestURL := cfg.forest
	fs.BoolVar(&local, "local", false, "")
	fs.StringVar(&forestURL, "forest", forestURL, "")
	if err := fs.Parse(normalizeLongFlags(args)); err != nil {
		return fmt.Errorf("web: %w", err)
	}

	addr := cfg.web
	if rest := fs.Args(); len(rest) > 0 {
		if len(rest) > 1 {
			return fmt.Errorf("web: unexpected argument %q", rest[1])
		}
		addr = rest[0]
	}
	if addr == "" {
		addr = "127.0.0.1:8420"
	}

	switch {
	case local:
		forestURL = ""
	case forestURL == "":
		forestURL = forest.DefaultHead
	}

	if cfg.workspace != "" {
		if err := os.Chdir(cfg.workspace); err != nil {
			return fmt.Errorf("workspace: %w", err)
		}
	}
	if cfg.model != "" {
		_ = os.Setenv("ARBOS_MODEL", cfg.model)
	}
	if cfg.theme != "" {
		_ = os.Setenv("ARBOS_THEME", cfg.theme)
		theme.Apply(cfg.theme)
	}
	return runWeb(piwire.LoadConfig(), cfg.db, addr, cfg.webDist, forestURL, cfg.approve)
}

func latestSessionID(dbPath string) (string, error) {
	store, err := piwire.OpenStore(dbPath)
	if err != nil {
		return "", fmt.Errorf("open store: %w", err)
	}
	defer func() { _ = store.Close() }()
	ctx := context.Background()
	sessions, err := store.ListSessions(ctx, 1)
	if err != nil {
		return "", err
	}
	if len(sessions) == 0 {
		return "", nil
	}
	return string(sessions[0].ID), nil
}

func runListSessions(cfg cliConfig, args []string) error {
	if len(args) > 0 {
		return fmt.Errorf("ls takes no arguments")
	}
	store, err := piwire.OpenStore(cfg.db)
	if err != nil {
		return fmt.Errorf("open store: %w", err)
	}
	defer func() { _ = store.Close() }()
	sessions, err := store.ListSessions(context.Background(), 50)
	if err != nil {
		return err
	}
	if len(sessions) == 0 {
		fmt.Println("No sessions yet.")
		return nil
	}
	tw := tabwriter.NewWriter(os.Stdout, 0, 0, 2, ' ', 0)
	_, _ = fmt.Fprintln(tw, "ID\tUPDATED\tTITLE")
	for _, s := range sessions {
		title := s.Title
		if title == "" {
			title = "(untitled)"
		}
		_, _ = fmt.Fprintf(tw, "%s\t%s\t%s\n",
			s.ID,
			s.UpdatedAt.Local().Format("2006-01-02 15:04"),
			title,
		)
	}
	return tw.Flush()
}

func runResume(cfg cliConfig, args []string) error {
	if len(args) > 1 {
		return fmt.Errorf("resume takes at most one session id")
	}
	session := ""
	if len(args) == 1 {
		session = args[0]
	} else {
		id, err := latestSessionID(cfg.db)
		if err != nil {
			return err
		}
		if id == "" {
			return fmt.Errorf("no sessions to resume")
		}
		session = id
	}
	if cfg.workspace != "" {
		if err := os.Chdir(cfg.workspace); err != nil {
			return fmt.Errorf("workspace: %w", err)
		}
	}
	if cfg.model != "" {
		_ = os.Setenv("ARBOS_MODEL", cfg.model)
	}
	if cfg.theme != "" {
		_ = os.Setenv("ARBOS_THEME", cfg.theme)
		theme.Apply(cfg.theme)
	}
	piCfg := piwire.LoadConfig()
	if cfg.web != "" {
		return runWeb(piCfg, cfg.db, cfg.web, cfg.webDist, cfg.forest, cfg.approve)
	}
	return run(piCfg, cfg.serve, cfg.db, "", session, cfg.approve, cfg.once || cfg.print)
}

func printCommandHelp(w io.Writer, args []string) {
	if len(args) == 0 {
		printUsage(w)
		return
	}
	switch args[0] {
	case "web":
		printSection(w, "Usage:", "  arbos web [addr] [--local | --forest <url>]")
		printSection(w, "Serve the web UI (loopback, default 127.0.0.1:8420) and join the default forest for a public, auth-gated URL.", "")
		_, _ = fmt.Fprintln(w)
		printSection(w, "Options:", "")
		printHelpFlag(w, "--local", "Serve locally only; do not join a forest")
		printHelpFlag(w, "--forest <url>", fmt.Sprintf("Join this forest head instead of the default (%s)", forest.DefaultHead))
	case "upgrade":
		printSection(w, "Usage:", "  arbos upgrade [--from <path|url>] [--to <path>]")
		printSection(w, "Replace the running arbos binary with a newer one; a serving instance hot-swaps it at its next idle moment. Inside an arbos source checkout this builds the checkout; elsewhere it downloads the latest release and prints its notes. --from installs a binary you supply instead.", "")
		_, _ = fmt.Fprintln(w)
		printSection(w, "Options:", "")
		printHelpFlag(w, "--from <path|url>", "Install this binary (local file or http(s) URL; .tar.gz unpacked) instead of downloading the latest release")
		printHelpFlag(w, "--to <path>", "Binary to replace (default: ARBOS_EXE from a serving instance, else this executable)")
	case "changelog":
		printSection(w, "Usage:", "  arbos changelog [version]")
		printSection(w, "Print a release's notes (the latest by default, or a specific tag like v0.1.1). Fetched from GitHub releases — what changed, since the binary carries no local changelog.", "")
	case "ls":
		printSection(w, "Usage:", "  arbos ls")
		printSection(w, "List resumable chat sessions from the local store.", "")
	case "resume":
		printSection(w, "Usage:", "  arbos resume [session-id]")
		printSection(w, "Resume a session. Without an id, opens the most recent chat.", "")
	case "export":
		printSection(w, "Usage:", "  arbos export [--all] [--messages] [session-id...]")
		printSection(w, "Export session trajectories as JSONL on stdout.", "")
		_, _ = fmt.Fprintln(w)
		printSection(w, "Options:", "")
		printHelpFlag(w, "--all", "Export every session in the store (including machine-spawned runs)")
		printHelpFlag(w, "--messages", "Render each session as one provider-facing conversation (system prompt, tools, and messages) instead of raw events")
	default:
		_, _ = fmt.Fprintf(w, "Unknown command %q\n\n", args[0])
		printUsage(w)
	}
}

func printUsage(w io.Writer) {
	_, _ = fmt.Fprintf(w, "Usage: %s [options] [command] [prompt...]\n\n", helpName)
	_, _ = fmt.Fprintln(w, "Start the Arbos coding agent")
	_, _ = fmt.Fprintln(w)
	printSection(w, "Arguments:", "")
	_, _ = fmt.Fprintln(w, "  prompt                       Initial prompt for the agent")
	_, _ = fmt.Fprintln(w)
	printSection(w, "Options:", "")
	flags := []helpFlag{
		{"-v, --version", "Output the version number"},
		{"-h, --help", "Display help for command"},
		{"-p, --print", "Print responses to stdout (for scripts or non-interactive use). Reads stdin when no prompt is given (default: false)"},
		{"-q, --query <text>", "Initial prompt for a one-shot run"},
		{"--prompt <text>", "Alias for --query"},
		{"--session <id>", "Resume an existing session"},
		{"--continue", "Resume the most recent session (default: false)"},
		{"--model <model>", "Model override (default: ARBOS_MODEL or provider default)"},
		{"--theme <name>", "Color theme: dark or light (default: ARBOS_THEME or dark)"},
		{"--approve", "Gate write/edit/bash tools behind y/N confirmation (default: false)"},
		{"--once", "Exit after one turn instead of prompting for follow-ups (default: false)"},
		{"--serve", "Serve the control seam over stdio (JSON lines) (default: false)"},
		{"--web <addr>", "Serve the web gateway at this address (e.g. :8420)"},
		{"--web-dist <path>", "Built web UI directory to serve alongside --web"},
		{"--forest <url>", "Join a forest head and serve the gateway at an assigned public URL (e.g. http://forest.example:8080)"},
		{"--db <path>", fmt.Sprintf("SQLite session store (default: %s)", piwire.DefaultDBPath())},
		{"--workspace <path>", "Working directory for the agent (default: current directory)"},
	}
	for _, f := range flags {
		printHelpFlag(w, f.name, f.desc)
	}
	_, _ = fmt.Fprintln(w)
	printSection(w, "Commands:", "")
	commands := []helpFlag{
		{"web [addr]", "Serve the web UI and join the default forest for a public URL (--local to stay off it)"},
		{"upgrade", "Update the running arbos in place (source, latest release, or --from a supplied binary); hot-swaps at idle"},
		{"changelog", "Show a release's notes (latest or a given version)"},
		{"ls", "List resumable chat sessions"},
		{"resume [id]", "Resume a session (latest when id is omitted)"},
		{"export [id...]", "Export session trajectories as JSONL (--all, --messages)"},
		{"help [command]", "Display help for command"},
	}
	for _, c := range commands {
		printHelpFlag(w, c.name, c.desc)
	}
	_, _ = fmt.Fprintln(w)
	printSection(w, "Environment:", "")
	envs := []helpFlag{
		{"OPENROUTER_API_KEY", "OpenRouter API key (default onboarding path)"},
		{"ARBOS_PROVIDER", "LLM provider: openai, anthropic, or google"},
		{"ARBOS_MODEL", "Model id for the selected provider"},
		{"ARBOS_THEME", "Color theme: dark or light"},
		{"ARBOS_OPENAI_API_KEY", "OpenAI-compatible API key"},
		{"ARBOS_ANTHROPIC_API_KEY", "Anthropic API key"},
		{"ARBOS_GOOGLE_API_KEY", "Google API key"},
	}
	for _, e := range envs {
		printHelpFlag(w, e.name, e.desc)
	}
}

type helpFlag struct {
	name, desc string
}

const helpNameWidth = 30

func printSection(w io.Writer, title, body string) {
	if title != "" {
		_, _ = fmt.Fprintln(w, title)
	}
	if body != "" {
		_, _ = fmt.Fprintln(w, body)
	}
}

func printHelpFlag(w io.Writer, name, desc string) {
	lines := wrapHelp(desc, 72-helpNameWidth)
	if len(lines) == 0 {
		_, _ = fmt.Fprintf(w, "  %-*s\n", helpNameWidth, name)
		return
	}
	_, _ = fmt.Fprintf(w, "  %-*s %s\n", helpNameWidth, name, lines[0])
	for _, ln := range lines[1:] {
		_, _ = fmt.Fprintf(w, "  %-*s %s\n", helpNameWidth, "", ln)
	}
}

func wrapHelp(text string, width int) []string {
	if width < 16 {
		width = 16
	}
	words := strings.Fields(text)
	if len(words) == 0 {
		return nil
	}
	var lines []string
	var b strings.Builder
	for _, word := range words {
		if b.Len() == 0 {
			b.WriteString(word)
			continue
		}
		if b.Len()+1+len(word) > width {
			lines = append(lines, b.String())
			b.Reset()
			b.WriteString(word)
			continue
		}
		b.WriteByte(' ')
		b.WriteString(word)
	}
	if b.Len() > 0 {
		lines = append(lines, b.String())
	}
	return lines
}
