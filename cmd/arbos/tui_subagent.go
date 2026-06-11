package main

import (
	"encoding/json"
	"fmt"
	"strings"

	"github.com/charmbracelet/lipgloss"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/theme"
	"github.com/unarbos/arbos/internal/transcript"
)

// subagentRun tracks one in-flight delegate call and the child's live activity.
type subagentRun struct {
	callID      string
	instruction string
	sessionID   core.SessionID
	status      string
	tokens      int
}

func delegateInstruction(args json.RawMessage) string {
	var a struct {
		Instruction string `json:"instruction"`
	}
	if json.Unmarshal(args, &a) != nil || strings.TrimSpace(a.Instruction) == "" {
		return "Sub-agent task"
	}
	line, _, _ := strings.Cut(strings.TrimSpace(a.Instruction), "\n")
	return clip(line, 72)
}

func childStatusLabel(name string, args json.RawMessage) string {
	label := transcript.ToolLabel(name, args)
	switch name {
	case "find", "grep":
		if _, rest, ok := strings.Cut(label, " "); ok {
			return "Finding " + strings.TrimSpace(rest)
		}
	case "read", "ls":
		if _, rest, ok := strings.Cut(label, " "); ok {
			return "Reading " + strings.TrimSpace(rest)
		}
	case "write", "edit":
		if _, rest, ok := strings.Cut(label, " "); ok {
			return "Editing " + strings.TrimSpace(rest)
		}
	case "bash":
		if _, rest, ok := strings.Cut(label, " "); ok {
			return "Running " + strings.TrimSpace(rest)
		}
	}
	return label
}

func formatTokenCount(n int) string {
	if n <= 0 {
		return ""
	}
	if n >= 10_000 {
		return fmt.Sprintf("%.2fk tokens", float64(n)/1000)
	}
	if n >= 1000 {
		return fmt.Sprintf("%.1fk tokens", float64(n)/1000)
	}
	return fmt.Sprintf("%d tokens", n)
}

func (r *tuiRenderer) activeSubagents() []subagentRun {
	var out []subagentRun
	for _, sa := range r.subagents {
		for _, rt := range r.running {
			if rt.id == sa.callID {
				out = append(out, sa)
				break
			}
		}
	}
	return out
}

func (r *tuiRenderer) subagentPanelActive() bool {
	return len(r.activeSubagents()) > 0
}

func (r *tuiRenderer) withChildUpdate(fn func()) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if !r.turnOpen {
		return
	}
	fn()
	r.clearBlock()
	r.renderBlock()
}

func (r *tuiRenderer) attachChildSession(sessionID core.SessionID) {
	if sessionID == "" {
		return
	}
	for i := range r.subagents {
		if r.subagents[i].sessionID == sessionID {
			return
		}
	}
	for i := range r.subagents {
		if r.subagents[i].sessionID == "" {
			r.subagents[i].sessionID = sessionID
			return
		}
	}
}

func (r *tuiRenderer) subagentIndex(sessionID core.SessionID) int {
	for i, sa := range r.subagents {
		if sa.sessionID == sessionID {
			return i
		}
	}
	return -1
}

func (r *tuiRenderer) startSubagent(call core.ToolCall) {
	r.subagents = append(r.subagents, subagentRun{
		callID:      call.ID,
		instruction: delegateInstruction(call.Args),
	})
}

func (r *tuiRenderer) finishSubagent(callID string) {
	for i, sa := range r.subagents {
		if sa.callID == callID {
			r.subagents = append(r.subagents[:i], r.subagents[i+1:]...)
			return
		}
	}
}

func (r *tuiRenderer) childToolStart(sessionID core.SessionID, call core.ToolCall) {
	r.attachChildSession(sessionID)
	if i := r.subagentIndex(sessionID); i >= 0 {
		r.subagents[i].status = childStatusLabel(call.Name, call.Args)
	}
}

func (r *tuiRenderer) childToolFinish(sessionID core.SessionID, res core.ToolResult) {
	if i := r.subagentIndex(sessionID); i >= 0 && res.IsError {
		r.subagents[i].status = "Failed"
	}
}

func (r *tuiRenderer) childTurnComplete(sessionID core.SessionID, usage core.Usage) {
	if i := r.subagentIndex(sessionID); i >= 0 && usage.TotalTokens > 0 {
		r.subagents[i].tokens = usage.TotalTokens
	}
}

// subagentLines renders Cursor's nested agent panel: a hex header, per-agent
// task names with muted status underneath, and a "Running subagent" footer
// with token count.
func (r *tuiRenderer) subagentLines() []string {
	agents := r.activeSubagents()
	if len(agents) == 0 {
		return nil
	}
	hex := r.faint.Render("⬡")
	header := fmt.Sprintf("Running %d agent", len(agents))
	if len(agents) != 1 {
		header += "s"
	}
	header += "..."
	nested := tuiProseIndent + "  "
	lines := []string{indentTranscript(hex + " " + r.working.Render(header))}
	for _, sa := range agents {
		lines = append(lines, nested+r.bright.Render(clip(sa.instruction, r.width-4)))
		status := sa.status
		if status == "" {
			status = "Working..."
		}
		lines = append(lines, nested+r.faint.Render(clip(status, r.width-4)))
	}
	lines = append(lines, nested+r.faint.Render("ctrl+o to expand"))

	totalTokens := 0
	for _, sa := range agents {
		totalTokens += sa.tokens
	}
	icon := lipgloss.NewStyle().Foreground(theme.Status).Render("⠿")
	footer := icon + " " + r.working.Render("Running subagent")
	if tok := formatTokenCount(totalTokens); tok != "" {
		footer += "  " + r.faint.Render(tok)
	}
	lines = append(lines, indentTranscript(footer))
	return lines
}
