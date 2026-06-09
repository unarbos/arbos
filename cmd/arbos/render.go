package main

import (
	"fmt"
	"io"
	"strings"

	"github.com/charmbracelet/lipgloss"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/theme"
	"github.com/unarbos/arbos/internal/transcript"
)

// agentIcon is the per-turn marker for arbos responses: a distinct glyph (not
// the literal name) styled in the agent's color, so each turn opens with a
// recognizable icon rather than repeating "Arbos".
const agentIcon = "◆"

// uiRenderer is what the one-shot event loop drives. Two implementations:
// renderer (static lines, for pipes and dumb terminals) and liveRenderer
// (spinners + a rotating output box that collapses at turn end, for TTYs).
type uiRenderer interface {
	header(id string)
	delta(text string)
	toolStart(call core.ToolCall)
	toolFinish(res core.ToolResult)
	approvalPrompt(call core.ToolCall)
	turnComplete(reason core.StopReason)
	promptFollowUp()
	notice(msgs []string)
	interrupted()
	errorf(e core.ErrorEvent)
	close()
}

// renderer turns a one-shot event stream into a clean console transcript:
// assistant prose streams to stdout (so `arbos "…" > file` captures just the
// answer), while tool activity and status render to stderr, styled and spaced.
// It mirrors the interactive TUI's look via the shared transcript package.
type renderer struct {
	out         io.Writer // assistant prose
	status      io.Writer // tool activity, status, errors
	labels      map[string]string
	midText     bool // last prose write had no trailing newline
	turnOpen    bool // a turn has emitted content and not yet been closed off
	atLineStart bool // next prose byte begins a fresh line (needs indent)
	md          *transcript.MarkdownStyler

	dim   lipgloss.Style
	ok    lipgloss.Style
	bad   lipgloss.Style
	tool  lipgloss.Style
	note  lipgloss.Style
	del   lipgloss.Style
	agent lipgloss.Style
}

// proseIndent is prepended to each line of streamed assistant prose so the
// agent's turn is visibly set off from the user's flush-left input.
const proseIndent = "  "

// indentStream prepends proseIndent at the start of every line in s, using
// atLineStart to carry the line-boundary state across streaming pushes.
func indentStream(s string, atLineStart *bool) string {
	var b strings.Builder
	for i := 0; i < len(s); i++ {
		if *atLineStart && s[i] != '\n' {
			b.WriteString(proseIndent)
		}
		b.WriteByte(s[i])
		*atLineStart = s[i] == '\n'
	}
	return b.String()
}

func newRenderer(out, status io.Writer) *renderer {
	// Bind styles to the status stream so color is enabled only when stderr is a
	// real terminal (lipgloss degrades to plain text when piped).
	lr := lipgloss.NewRenderer(status)
	return &renderer{
		out:         out,
		status:      status,
		labels:      map[string]string{},
		atLineStart: true,
		md:          newMarkdownStyler(out),
		dim:         lr.NewStyle().Foreground(theme.Muted),
		ok:          lr.NewStyle().Foreground(theme.Primary),
		bad:         lr.NewStyle().Bold(true).Foreground(theme.Text).Background(theme.Deep),
		tool:        lr.NewStyle().Foreground(theme.Accent),
		note:        lr.NewStyle().Bold(true).Foreground(theme.Accent),
		del:         lr.NewStyle().Foreground(theme.Muted).Strikethrough(true),
		agent:       lr.NewStyle().Bold(true).Foreground(theme.Primary),
	}
}

// agentLabel marks the start of an agent turn: a blank line for breathing
// room, then a distinct colored icon so the agent's side reads clearly
// without repeating the "Arbos" name on every turn.
func (r *renderer) agentLabel() {
	if r.turnOpen {
		return
	}
	r.turnOpen = true
	_, _ = fmt.Fprintln(r.status)
	_, _ = fmt.Fprintln(r.status, r.agent.Render(agentIcon))
}

// newMarkdownStyler builds a prose Markdown styler bound to w, using the shared
// theme so headings, code, and emphasis read consistently across front-ends.
func newMarkdownStyler(w io.Writer) *transcript.MarkdownStyler {
	lr := lipgloss.NewRenderer(w)
	return transcript.NewMarkdownStyler(transcript.MarkdownStyles{
		Heading: lr.NewStyle().Bold(true).Foreground(theme.Primary),
		Bold:    lr.NewStyle().Bold(true).Foreground(theme.Text),
		Italic:  lr.NewStyle().Italic(true),
		Code:    lr.NewStyle().Foreground(theme.Accent),
		Bullet:  lr.NewStyle().Foreground(theme.Primary),
		Body:    lr.NewStyle(),
	})
}

// styledDiff renders a display diff with +/- coloring, indented under its tool
// line. max caps the body (0 = unlimited); overflow appends a dim count line.
func styledDiff(diff string, max int, add, del, ctx lipgloss.Style) []string {
	lines := strings.Split(strings.TrimRight(diff, "\n"), "\n")
	total := len(lines)
	if max > 0 && total > max {
		lines = lines[:max]
	}
	out := make([]string, 0, len(lines)+1)
	for _, ln := range lines {
		switch {
		case strings.HasPrefix(ln, "+"):
			out = append(out, "    "+add.Render(ln))
		case strings.HasPrefix(ln, "-"):
			out = append(out, "    "+del.Render(ln))
		default:
			out = append(out, "    "+ctx.Render(ln))
		}
	}
	if max > 0 && total > max {
		out = append(out, "    "+ctx.Render(fmt.Sprintf("… (%d more lines)", total-max)))
	}
	return out
}

// header prints the green Arbos banner that opens a session, with the session
// id kept as a dim trailer (still useful for -session resume).
func (r *renderer) header(id string) {
	_, _ = fmt.Fprintln(r.status, r.agent.Render("Arbos")+r.dim.Render("  "+id))
}

func (r *renderer) delta(text string) {
	if text == "" {
		return
	}
	r.agentLabel()
	if styled := r.md.Push(text); styled != "" {
		_, _ = fmt.Fprint(r.out, indentStream(styled, &r.atLineStart))
	}
	r.midText = !strings.HasSuffix(text, "\n")
}

// breakText ends an in-progress prose line so a status line below it doesn't run
// into the assistant's text (the original bug).
func (r *renderer) breakText() {
	if tail := r.md.Flush(); tail != "" {
		_, _ = fmt.Fprint(r.out, indentStream(tail, &r.atLineStart))
	}
	r.md = newMarkdownStyler(r.out)
	if r.midText {
		_, _ = fmt.Fprintln(r.out)
		r.midText = false
		r.atLineStart = true
	}
}

func (r *renderer) toolStart(call core.ToolCall) {
	r.agentLabel()
	label := transcript.ToolLabel(call.Name, call.Args)
	r.labels[call.ID] = label
	r.breakText()
	_, _ = fmt.Fprintln(r.status, r.tool.Render("  • "+label))
}

func (r *renderer) toolFinish(res core.ToolResult) {
	label := r.labels[res.CallID]
	if label == "" {
		label = "tool"
	}
	line := transcript.ToolDone(label, res)
	style := r.ok
	if res.IsError {
		style = r.bad
	}
	_, _ = fmt.Fprintln(r.status, "  "+style.Render(line))
	if diff := transcript.DiffOf(res); diff != "" {
		for _, ln := range styledDiff(diff, 0, r.ok, r.del, r.dim) {
			_, _ = fmt.Fprintln(r.status, ln)
		}
		return
	}
	for _, ln := range transcript.ToolOutputPreview(label, res.Content) {
		_, _ = fmt.Fprintln(r.status, r.dim.Render("    "+ln))
	}
}

// approvalPrompt asks whether a gated tool call may run. It returns the prompt
// label so the caller can read the answer from stdin.
func (r *renderer) approvalPrompt(call core.ToolCall) {
	r.breakText()
	label := transcript.ToolLabel(call.Name, call.Args)
	_, _ = fmt.Fprint(r.status, r.note.Render("  approve "+label+"? [y/N] "))
}

func (r *renderer) turnComplete(reason core.StopReason) {
	r.breakText()
	if reason != core.StopAnswered {
		_, _ = fmt.Fprintln(r.status, r.dim.Render("· stopped: "+string(reason)))
	}
	r.turnOpen = false
	_, _ = fmt.Fprintln(r.status)
}

func (r *renderer) promptFollowUp() {
	r.breakText()
	_, _ = fmt.Fprint(r.status, r.note.Render("› "))
}

// notice prints outbox messages as ambient lines: the agent's voice arriving
// between turns, dimly marked so it never reads as a turn of its own.
func (r *renderer) notice(msgs []string) {
	r.breakText()
	_, _ = fmt.Fprintln(r.status)
	for _, m := range msgs {
		_, _ = fmt.Fprintln(r.status, r.note.Render("◇ ")+m)
	}
}

func (r *renderer) close() {}

func (r *renderer) interrupted() {
	r.breakText()
	_, _ = fmt.Fprintln(r.status, r.dim.Render("· interrupted"))
}

func (r *renderer) errorf(e core.ErrorEvent) {
	r.breakText()
	_, _ = fmt.Fprintln(r.status, r.bad.Render("· error ("+string(e.Category)+"): "+e.Err))
}
