package main

import (
	"fmt"
	"io"
	"strings"
	"time"

	"github.com/charmbracelet/lipgloss"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/theme"
	"github.com/unarbos/arbos/internal/transcript"
)

// agentIcon is the per-turn marker for arbos responses: a distinct glyph (not
// the literal name) styled in the agent's color, so each turn opens with a
// recognizable icon rather than repeating "Arbos".
const agentIcon = "◆"

// uiRenderer is what non-interactive and compatibility paths drive. Interactive
// TTY sessions use tuiRenderer directly so raw-mode input and rendering share
// one owner.
type uiRenderer interface {
	header(id string)
	turnStart()
	delta(text string)
	toolStart(call core.ToolCall)
	toolFinish(res core.ToolResult)
	approvalPrompt(call core.ToolCall)
	turnComplete(reason core.StopReason)
	promptFollowUp()
	steeringPrompt()
	commitPrompt(text string)
	printStanding(lines []string)
	notice(msgs []string)
	interrupted()
	steerCut()
	errorf(e core.ErrorEvent)
	close()
}

// renderer turns a one-shot event stream into a clean console transcript:
// assistant prose streams to stdout (so `arbos "…" > file` captures just the
// answer), while tool activity and status render to stderr, styled and spaced.
// It mirrors the interactive TUI's look via the shared transcript package.
type renderer struct {
	out       io.Writer // assistant prose
	status    io.Writer // tool activity, status, errors
	labels    map[string]string
	midText   bool // last prose write had no trailing newline
	turnOpen  bool // a turn has emitted content and not yet been closed off
	proseWrap proseWrapper
	md        *transcript.MarkdownStyler

	dim          lipgloss.Style
	ok           lipgloss.Style
	bad          lipgloss.Style
	tool         lipgloss.Style
	note         lipgloss.Style
	standing     lipgloss.Style
	standingText lipgloss.Style
	agent        lipgloss.Style
}

// renderStandingLine styles one standing-obligation line: the recurring glyph
// is the light accent, while the obligation text itself reads bold.
func renderStandingLine(glyph, text lipgloss.Style, ln string) string {
	if rest, found := strings.CutPrefix(ln, "↻ "); found {
		return glyph.Render("↻") + " " + text.Render(rest)
	}
	return text.Render(ln)
}

// proseIndent is prepended to each line of streamed assistant prose so the
// agent's turn is visibly set off from the user's flush-left input.
const proseIndent = "  "

// tuiProseIndent matches the horizontal inset inside query bands so assistant
// prose lines up with the text inside shaded user/follow-up rows.
const tuiProseIndent = "  "

const (
	maxProseWidth = 88
	minProseWidth = 48
)

const (
	liveSpinnerRows = 5
	liveTick        = 120 * time.Millisecond
)

type runningTool struct {
	id    string
	name  string // tool name (delegate, read, …)
	label string // precise label (read foo.go) for permanent diff/error lines
	verb  string // gerund phrase (reading files) for live activity lines
}

// proseWidth chooses a readable line length. Wide terminals still get a measure
// that is easy to scan; narrow terminals keep enough room for indentation.
func proseWidth(termWidth int) int {
	w := termWidth - len(proseIndent)
	if w > maxProseWidth {
		return maxProseWidth
	}
	if w < minProseWidth {
		return minProseWidth
	}
	return w
}

type proseWrapper struct {
	atLineStart bool
	col         int
	width       int
	indent      string
	word        strings.Builder
}

func newProseWrapper(width int) proseWrapper {
	return newProseWrapperWithIndent(width, proseIndent)
}

func newTUIProseWrapper(width int) proseWrapper {
	return newProseWrapperWithIndent(width, tuiProseIndent)
}

func newProseWrapperWithIndent(termWidth int, indent string) proseWrapper {
	w := termWidth - len(indent)
	if w > maxProseWidth {
		w = maxProseWidth
	}
	if w < minProseWidth {
		w = minProseWidth
	}
	return proseWrapper{atLineStart: true, width: w, indent: indent}
}

func (w *proseWrapper) startAfterPrefix(prefix string) {
	if prefix == "" {
		w.atLineStart = true
		w.col = 0
	} else {
		w.atLineStart = false
		w.col = lipgloss.Width(prefix)
	}
	w.word.Reset()
}

func (w *proseWrapper) write(s string) string {
	var b strings.Builder
	for _, r := range s {
		switch r {
		case '\n':
			w.flushWord(&b)
			b.WriteByte('\n')
			w.atLineStart = true
			w.col = 0
		case ' ', '\t':
			w.flushWord(&b)
			if !w.atLineStart {
				b.WriteByte(' ')
				w.col++
			}
		default:
			w.word.WriteRune(r)
		}
	}
	return b.String()
}

func (w *proseWrapper) flush() string {
	var b strings.Builder
	w.flushWord(&b)
	return b.String()
}

func (w *proseWrapper) flushWord(b *strings.Builder) {
	if w.word.Len() == 0 {
		return
	}
	word := w.word.String()
	wordWidth := lipgloss.Width(word)
	if w.atLineStart {
		b.WriteString(w.indent)
		w.col = len(w.indent)
		w.atLineStart = false
	}
	if w.col > len(w.indent) && w.col+1+wordWidth > w.width {
		b.WriteByte('\n')
		b.WriteString(w.indent)
		w.col = len(w.indent)
	}
	b.WriteString(word)
	w.col += wordWidth
	w.word.Reset()
}

func newRenderer(out, status io.Writer) *renderer {
	// Bind styles to the status stream so color is enabled only when stderr is a
	// real terminal (lipgloss degrades to plain text when piped).
	lr := lipgloss.NewRenderer(status)
	return &renderer{
		out:          out,
		status:       status,
		labels:       map[string]string{},
		proseWrap:    newProseWrapper(80),
		md:           newMarkdownStyler(out),
		dim:          lr.NewStyle().Foreground(theme.Faint),
		ok:           lr.NewStyle().Foreground(theme.Primary),
		bad:          lr.NewStyle().Bold(true).Foreground(theme.Text).Background(theme.Deep),
		tool:         lr.NewStyle().Foreground(theme.Muted),
		note:         lr.NewStyle().Foreground(theme.Accent),
		standing:     lr.NewStyle().Bold(true).Foreground(theme.Accent),
		standingText: lr.NewStyle().Bold(true).Foreground(theme.Text),
		agent:        lr.NewStyle().Bold(true).Foreground(theme.Primary),
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

// turnStart opens the agent's side of the turn immediately on submit, before
// the first model event — so ◆ appears right after enter, not after LLM latency.
func (r *renderer) turnStart() { r.agentLabel() }

// newMarkdownStyler builds a prose Markdown styler bound to w, using the shared
// theme so headings, code, and emphasis read consistently across front-ends.
func newMarkdownStyler(w io.Writer) *transcript.MarkdownStyler {
	return newMarkdownStylerWithBody(w, lipgloss.NewStyle())
}

func newTUIMarkdownStyler(w io.Writer) *transcript.MarkdownStyler {
	lr := lipgloss.NewRenderer(w)
	return newMarkdownStylerWithBody(w, lr.NewStyle().Foreground(theme.Text))
}

func newMarkdownStylerWithBody(w io.Writer, body lipgloss.Style) *transcript.MarkdownStyler {
	lr := lipgloss.NewRenderer(w)
	return transcript.NewMarkdownStyler(transcript.MarkdownStyles{
		Heading: lr.NewStyle().Bold(true).Foreground(theme.Text),
		Bold:    lr.NewStyle().Bold(true).Foreground(theme.Text),
		Italic:  lr.NewStyle().Italic(true).Foreground(theme.Muted),
		Code:    lr.NewStyle().Foreground(theme.Accent),
		Bullet:  lr.NewStyle().Foreground(theme.Muted),
		Body:    body,
	})
}

// header prints the green Arbos banner that opens a session, with the session
// id kept as a dim trailer (still useful for -session resume).
func (r *renderer) header(id string) {
	_, _ = fmt.Fprintln(r.status, r.agent.Render("Arbos Agent"))
	_, _ = fmt.Fprintln(r.status, r.dim.Render(buildVersion()))
	if id != "" {
		_, _ = fmt.Fprintln(r.status, r.dim.Render(id))
	}
	_, _ = fmt.Fprintln(r.status)
}

func (r *renderer) delta(text string) {
	if text == "" {
		return
	}
	r.agentLabel()
	if styled := r.md.Push(text); styled != "" {
		_, _ = fmt.Fprint(r.out, r.proseWrap.write(styled))
	}
	r.midText = !strings.HasSuffix(text, "\n")
}

// breakText ends an in-progress prose line so a status line below it doesn't run
// into the assistant's text (the original bug).
func (r *renderer) breakText() {
	if tail := r.md.Flush(); tail != "" {
		_, _ = fmt.Fprint(r.out, r.proseWrap.write(tail))
	}
	if tail := r.proseWrap.flush(); tail != "" {
		_, _ = fmt.Fprint(r.out, tail)
	}
	r.md = newMarkdownStyler(r.out)
	if r.midText {
		_, _ = fmt.Fprintln(r.out)
		r.midText = false
		r.proseWrap = newProseWrapper(80)
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
	if res.IsError {
		_, _ = fmt.Fprintln(r.status, "  "+r.bad.Render(transcript.ToolDone(label, res)))
		return
	}
	// One concise line per tool — a +/- stat for edits, never the whole diff
	// or a dump of tool output. Even redirected to a file the status stream
	// stays a readable activity log.
	line := transcript.ToolDone(label, res)
	if diff := transcript.DiffOf(res); diff != "" {
		add, del := transcript.DiffStat(diff)
		line = fmt.Sprintf("%s (+%d −%d)", line, add, del)
	}
	_, _ = fmt.Fprintln(r.status, "  "+r.ok.Render(line))
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
	_, _ = fmt.Fprint(r.status, r.dim.Render("→ "+followUpHint))
}

func (r *renderer) steeringPrompt() {}

func (r *renderer) commitPrompt(string) {}

func (r *renderer) printStanding(lines []string) {
	for _, ln := range lines {
		_, _ = fmt.Fprintln(r.status, renderStandingLine(r.standing, r.standingText, ln))
	}
	// Spacing is renderer-owned (announceStanding no longer writes raw blanks).
	_, _ = fmt.Fprintln(r.status)
}

// notice prints outbox messages as ambient lines: the agent's voice arriving
// between turns, dimly marked so it never reads as a turn of its own.
func (r *renderer) notice(msgs []string) {
	r.breakText()
	_, _ = fmt.Fprintln(r.status)
	for _, m := range msgs {
		_, _ = fmt.Fprintln(r.status, r.note.Render("◇ ")+sanitizeNotice(m))
	}
	_, _ = fmt.Fprintln(r.status)
}

// sanitizeNotice strips control bytes (notably ESC) from an outbox message
// before it reaches the terminal. Outbox text is model-authored and may carry
// content the model read from the web or files; without this a crafted
// message could inject ANSI escapes (cursor moves, color, title rewrites)
// into the user's terminal. Tabs and newlines survive; everything else below
// 0x20 and the DEL byte are dropped.
func sanitizeNotice(s string) string {
	return strings.Map(func(r rune) rune {
		if r == '\n' || r == '\t' {
			return r
		}
		if r < 0x20 || r == 0x7f {
			return -1
		}
		return r
	}, s)
}

func (r *renderer) close() {}

func (r *renderer) interrupted() {
	r.breakText()
	_, _ = fmt.Fprintln(r.status, r.dim.Render("· interrupted"))
}

func (r *renderer) steerCut() {
	r.breakText()
	r.turnOpen = false
}

func (r *renderer) errorf(e core.ErrorEvent) {
	r.breakText()
	_, _ = fmt.Fprintln(r.status, r.bad.Render("· error ("+string(e.Category)+"): "+e.Err))
}

// clip truncates to n display runes with an ellipsis; terminal activity lines
// must never wrap or they become hard to scan.
func clip(s string, n int) string {
	if n <= 1 {
		return "…"
	}
	runes := []rune(s)
	if len(runes) <= n {
		return s
	}
	return string(runes[:n-1]) + "…"
}
