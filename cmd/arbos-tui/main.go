// Command arbos-tui is the interactive Bubble Tea front-end for the pi coding
// agent. It uses the same piwire assembly as cmd/arbos (durable store,
// delegation tools, observer) and renders assistant text, tool calls, edit
// diffs, and approval prompts in a streaming transcript.
//
// With no LLM endpoint configured it uses the deterministic fake provider.
package main

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/signal"
	"strings"
	"syscall"

	"github.com/charmbracelet/bubbles/textinput"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"

	"github.com/unarbos/arbos/internal/agent/pi"
	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/obs"
	"github.com/unarbos/arbos/internal/piwire"
)

var (
	youStyle  = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("12"))
	piStyle   = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("10"))
	toolStyle = lipgloss.NewStyle().Foreground(lipgloss.Color("11"))
	addStyle  = lipgloss.NewStyle().Foreground(lipgloss.Color("10"))
	delStyle  = lipgloss.NewStyle().Foreground(lipgloss.Color("9"))
	dimStyle  = lipgloss.NewStyle().Foreground(lipgloss.Color("8"))
)

type evMsg struct{ env core.Envelope }
type evClosed struct{}

type model struct {
	conv       *engine.Conversation
	input      textinput.Model
	transcript []string
	cur        string
	busy       bool
	width      int
	templates  []pi.PromptTemplate
	approve    bool
}

func (m model) Init() tea.Cmd { return tea.Batch(textinput.Blink, m.readNext()) }

func (m model) readNext() tea.Cmd {
	return func() tea.Msg {
		env, ok := <-m.conv.Events()
		if !ok {
			return evClosed{}
		}
		return evMsg{env}
	}
}

func (m model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width = msg.Width
		m.input.Width = msg.Width - 6
		return m, nil
	case tea.KeyMsg:
		switch msg.String() {
		case "ctrl+c", "ctrl+d":
			return m, tea.Quit
		case "enter":
			text := strings.TrimSpace(m.input.Value())
			if text == "" || m.busy {
				return m, nil
			}
			m.transcript = append(m.transcript, youStyle.Render("you> ")+text)
			m.input.Reset()
			m.busy = true
			prompt := pi.ExpandPromptTemplate(text, m.templates)
			m.conv.Send(core.PromptIntent{Text: prompt})
			return m, nil
		}
	case evMsg:
		cmd := m.apply(msg.env.Event)
		return m, tea.Batch(m.readNext(), cmd)
	case evClosed:
		return m, tea.Quit
	}
	var cmd tea.Cmd
	m.input, cmd = m.input.Update(msg)
	return m, cmd
}

func (m *model) apply(ev core.KernelEvent) tea.Cmd {
	switch e := ev.(type) {
	case core.MessageDelta:
		m.cur += e.Text
	case core.ApprovalRequest:
		m.transcript = append(m.transcript, toolStyle.Render(fmt.Sprintf("  approve %s(%s)? [y/N]", e.Call.Name, truncate(string(e.Call.Args), 60))))
		if !m.approve {
			m.conv.Send(core.ApprovalResponseIntent{RequestID: e.RequestID, Approved: true})
			return nil
		}
		return func() tea.Msg {
			fmt.Printf("\n  approve %s(%s)? [y/N] ", e.Call.Name, string(e.Call.Args))
			line, _ := bufio.NewReader(os.Stdin).ReadString('\n')
			approved := strings.EqualFold(strings.TrimSpace(line), "y")
			m.conv.Send(core.ApprovalResponseIntent{RequestID: e.RequestID, Approved: approved})
			return nil
		}
	case core.ToolStarted:
		m.transcript = append(m.transcript, toolStyle.Render("  → "+e.Call.Name)+dimStyle.Render(" "+truncate(string(e.Call.Args), 80)))
	case core.ToolFinished:
		m.transcript = append(m.transcript, m.renderToolResult(e)...)
	case core.TurnComplete:
		if s := strings.TrimSpace(m.cur); s != "" {
			m.transcript = append(m.transcript, piStyle.Render("pi> ")+s)
		}
		m.cur = ""
		m.busy = false
		m.transcript = append(m.transcript, dimStyle.Render("["+string(e.StopReason)+"]"))
	case core.Interrupted:
		m.cur = ""
		m.busy = false
		m.transcript = append(m.transcript, dimStyle.Render("[interrupted]"))
	case core.ErrorEvent:
		m.busy = false
		m.transcript = append(m.transcript, delStyle.Render("error: "+e.Err))
	}
	return nil
}

func (m *model) renderToolResult(e core.ToolFinished) []string {
	if len(e.Result.Details) > 0 {
		var d struct {
			Diff string `json:"diff"`
		}
		if json.Unmarshal(e.Result.Details, &d) == nil && d.Diff != "" {
			out := []string{dimStyle.Render("  ── diff ──")}
			for _, ln := range strings.Split(d.Diff, "\n") {
				switch {
				case strings.HasPrefix(ln, "+"):
					out = append(out, "  "+addStyle.Render(ln))
				case strings.HasPrefix(ln, "-"):
					out = append(out, "  "+delStyle.Render(ln))
				default:
					out = append(out, "  "+dimStyle.Render(ln))
				}
			}
			return out
		}
	}
	preview := strings.ReplaceAll(truncate(e.Result.Content, 200), "\n", " ")
	style := toolStyle
	if e.Result.IsError {
		style = delStyle
	}
	return []string{style.Render("  ← ") + dimStyle.Render(preview)}
}

func (m model) View() string {
	var b strings.Builder
	b.WriteString(piStyle.Render("arbos · pi coding agent") + dimStyle.Render("  (ctrl+c to quit)") + "\n\n")
	for _, ln := range m.transcript {
		b.WriteString(ln + "\n")
	}
	if m.cur != "" {
		b.WriteString(piStyle.Render("pi> ") + strings.TrimSpace(m.cur) + "\n")
	}
	b.WriteString("\n" + m.input.View() + "\n")
	if m.busy {
		b.WriteString(dimStyle.Render("…working") + "\n")
	}
	return b.String()
}

func truncate(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n] + "…"
}

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, "arbos-tui:", err)
		os.Exit(1)
	}
}

func run() error {
	store, err := piwire.OpenStore(piwire.DefaultDBPath())
	if err != nil {
		return fmt.Errorf("open store: %w", err)
	}
	defer func() { _ = store.Close() }()

	approve := os.Getenv("ARBOS_APPROVE") == "1"
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

	conv, err := host.Engine.StartSession(ctx, piwire.NewSessionID())
	if err != nil {
		return err
	}

	ti := textinput.New()
	ti.Placeholder = "ask the coding agent…"
	ti.Prompt = youStyle.Render("› ")
	ti.Focus()

	cwd, _ := os.Getwd()
	templates := pi.LoadPromptTemplates(cwd, piwire.AgentConfigDir())
	p := tea.NewProgram(model{conv: conv, input: ti, templates: templates, approve: approve}, tea.WithAltScreen())
	_, err = p.Run()
	return err
}
