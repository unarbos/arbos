package main

import (
	"bufio"
	"context"
	"fmt"
	"io"
	"os"
	"strings"
	"sync"
	"time"

	"github.com/charmbracelet/lipgloss"
	"github.com/charmbracelet/x/term"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/theme"
	"github.com/unarbos/arbos/internal/transcript"
)

// tuiRenderer is the raw-mode interactive front-end. The terminal is split in
// two: everything above the "block" is permanent, append-only scrollback
// (selectable, scrollable), and the block is the only repaintable region.
//
// Streaming is Cursor-style: while a turn runs, the model's answer streams
// only inside the block as a rolling window (with thinking previews and tool
// activity below it), and nothing of it touches scrollback. When the turn
// ends, the complete answer is rendered once — full reflow, clean markdown —
// into the permanent transcript, replacing the window.
//
// The block is repainted with relative cursor moves from an exact line count;
// no save/restore-cursor escapes (they break when the screen scrolls) and no
// reliance on cooked-mode newlines (rawTerminalWriter normalizes LF to CRLF).
type tuiRenderer struct {
	mu    sync.Mutex
	out   io.Writer
	fd    uintptr
	width int

	turnOpen bool
	frame    int

	input      strings.Builder
	promptKind promptKind
	approval   string // approval prompt label, when promptKind == promptApproval

	// answerRaw accumulates the turn's raw answer tokens — the source of truth
	// for the final full render. answerLines + tail hold the same text wrapped
	// for the live window; started is true once the first token arrived.
	answerRaw   strings.Builder
	answerLines []string
	tail        string
	started     bool
	blockLines int // physical lines the block currently occupies (0 = none)
	promptIdx  int // 0-based row offset of the → input within the block

	// ephemeral is a one-line live preview of activity that never reaches the
	// permanent transcript: the model's reasoning stream, or a delegated
	// child's prose (which is internal narration, not this turn's answer).
	ephemeral string

	proseWrap proseWrapper
	md        *transcript.MarkdownStyler

	running   []runningTool
	subagents []subagentRun
	counts    map[string]int
	total     int
	fails     int

	meta tuiMeta

	dim, ok, bad, tool, note, standing lipgloss.Style
	standingText                       lipgloss.Style
	bright, faint, muted, cmd lipgloss.Style
	working                   lipgloss.Style

	sessionID string

	stop     chan struct{}
	stopOnce sync.Once
}

// rawTerminalWriter normalizes LF to CRLF: in raw mode the terminal no longer
// returns to column zero on \n, so every renderer newline must carry the \r.
type rawTerminalWriter struct {
	w io.Writer
}

func (w rawTerminalWriter) Write(p []byte) (int, error) {
	for _, b := range p {
		if b == '\n' {
			if _, err := w.w.Write([]byte{'\r', '\n'}); err != nil {
				return 0, err
			}
			continue
		}
		if _, err := w.w.Write([]byte{b}); err != nil {
			return 0, err
		}
	}
	return len(p), nil
}

type promptKind int

const (
	promptIdle promptKind = iota
	promptSteer
	promptApproval
)

const followUpHint = "Add a follow-up"

// Explicit ANSI for placeholder — lipgloss alone was rendering too bright
// in raw mode on some terminals.
const ansiDim = "\x1b[38;2;110;103;108m"
const ansiReset = "\x1b[0m"

const (
	bandPad        = 2 // horizontal inset inside a band (matches Padding(0, 2))
	bandBodyOffset = 1 // row within bandLines where the text body lives
)

func indentTranscript(line string) string {
	if line == "" {
		return ""
	}
	return tuiProseIndent + line
}

func indentTranscriptLines(lines []string) []string {
	if len(lines) == 0 {
		return nil
	}
	out := make([]string, len(lines))
	for i, ln := range lines {
		out[i] = indentTranscript(ln)
	}
	return out
}

func (r *tuiRenderer) printTranscriptLine(line string) {
	_, _ = fmt.Fprintln(r.out, indentTranscript(line))
}

func newTUIRenderer(out io.Writer, width int, meta tuiMeta) *tuiRenderer {
	if width <= 0 {
		width = 80
	}
	out = rawTerminalWriter{w: out}
	lr := lipgloss.NewRenderer(out)
	lr.SetHasDarkBackground(true)
	r := &tuiRenderer{
		out:          out,
		fd:           os.Stderr.Fd(),
		width:        width,
		meta:         meta,
		counts:       map[string]int{},
		stop:         make(chan struct{}),
		proseWrap:    newTUIProseWrapper(width),
		md:           newTUIMarkdownStyler(out),
		dim:          lr.NewStyle().Foreground(theme.Faint),
		ok:           lr.NewStyle().Foreground(theme.Primary),
		bad:          lr.NewStyle().Bold(true).Foreground(theme.Text).Background(theme.Deep),
		tool:         lr.NewStyle().Foreground(theme.Muted),
		note:         lr.NewStyle().Foreground(theme.Accent),
		standing:     lr.NewStyle().Bold(true).Foreground(theme.Accent),
		standingText: lr.NewStyle().Bold(true).Foreground(theme.Text),
		bright:       lr.NewStyle().Foreground(theme.Text),
		faint:        lr.NewStyle().Foreground(theme.Faint),
		muted:        lr.NewStyle().Foreground(theme.Muted),
		working: lr.NewStyle().Bold(true).Foreground(theme.Text),
		cmd:     lr.NewStyle().Foreground(theme.Command),
	}
	initTerminalChrome(out)
	go r.tick()
	return r
}

func initTerminalChrome(out io.Writer) {
	// Canvas background only — stay on the main screen so scrollback works.
	_, _ = fmt.Fprintf(out, "\x1b[48;2;44;38;42m")
}

func (r *tuiRenderer) hintLine() string {
	if r.meta.Approve {
		return "hint: write/edit/bash require approval"
	}
	return "hint: use -approve to gate dangerous tools"
}

func (r *tuiRenderer) header(string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	_, _ = fmt.Fprintln(r.out, r.bright.Render("Arbos Agent"))
	_, _ = fmt.Fprintln(r.out, r.muted.Render(r.meta.Version))
	_, _ = fmt.Fprintln(r.out, r.faint.Render(r.hintLine()))
	_, _ = fmt.Fprintln(r.out)
}

// turnStart opens the agent's side immediately on submit: the block with
// ◆ + spinner and the steer prompt below. The separating blank line is owned
// by whatever section printed last (the › echo, standing lines, a notice).
func (r *tuiRenderer) turnStart() {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.clearBlock()
	r.turnOpen = true
	r.frame = 0
	r.started = false
	r.tail = ""
	r.answerRaw.Reset()
	r.answerLines = nil
	r.renderBlock()
}

// delta streams an answer token into the live window only; the permanent
// transcript gets the full answer in one render when the turn ends.
func (r *tuiRenderer) delta(text string) {
	if text == "" {
		return
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	r.answerRaw.WriteString(text)
	styled := r.md.Push(text)
	if styled == "" {
		return
	}
	if !r.started {
		r.started = true
		r.tail = ""
		r.proseWrap.startAfterPrefix("")
	}
	out := r.proseWrap.write(styled)
	if out == "" {
		return
	}
	r.ephemeral = "" // real prose supersedes the thinking/child preview
	r.appendWindow(out)
	r.clearBlock()
	r.renderBlock()
}

// appendWindow folds newly wrapped output into the streaming window state:
// completed lines accumulate, the unfinished remainder becomes the tail.
func (r *tuiRenderer) appendWindow(out string) {
	combined := r.tail + out
	for {
		i := strings.IndexByte(combined, '\n')
		if i < 0 {
			break
		}
		r.answerLines = append(r.answerLines, combined[:i])
		combined = combined[i+1:]
	}
	r.tail = combined
}

// preview feeds the ephemeral line: reasoning tokens or a delegated child's
// streaming prose. Only the latest fragment is kept and shown.
func (r *tuiRenderer) preview(text string) {
	if text == "" {
		return
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	if !r.turnOpen {
		return
	}
	combined := r.ephemeral + sanitizeNotice(text)
	if i := strings.LastIndexByte(combined, '\n'); i >= 0 {
		if rest := strings.TrimSpace(combined[i+1:]); rest != "" {
			combined = rest
		} else {
			combined = strings.TrimSpace(combined[:i])
			if j := strings.LastIndexByte(combined, '\n'); j >= 0 {
				combined = combined[j+1:]
			}
		}
	}
	// Bound the rolling buffer; display clipping happens in previewLine.
	if runes := []rune(combined); len(runes) > 4*r.width {
		combined = string(runes[len(runes)-4*r.width:])
	}
	r.ephemeral = combined
	r.clearBlock()
	r.renderBlock()
}

// previewLine clips the rolling preview to one physical line, trimming at a
// word boundary with a leading … so it reads as a moving window, not as text
// that went missing.
func (r *tuiRenderer) previewLine() string {
	s := r.ephemeral
	maxw := r.width - 6
	if maxw < 8 || lipgloss.Width(s) <= maxw {
		return s
	}
	runes := []rune(s)
	for lipgloss.Width(string(runes)) > maxw-2 && len(runes) > 0 {
		runes = runes[1:]
	}
	if i := strings.IndexByte(string(runes), ' '); i >= 0 && i < 24 {
		runes = []rune(strings.TrimLeft(string(runes)[i:], " "))
	}
	return "… " + string(runes)
}

func (r *tuiRenderer) toolStart(call core.ToolCall) {
	r.mu.Lock()
	defer r.mu.Unlock()
	// A tool call ends the current answer segment: the next tokens are a new
	// thought. Without this break, segments concatenate ("…create it.Now let…").
	if raw := r.answerRaw.String(); raw != "" && !strings.HasSuffix(raw, "\n") {
		r.answerRaw.WriteString("\n\n")
		if styled := r.md.Push("\n\n"); styled != "" {
			r.appendWindow(r.proseWrap.write(styled))
		}
	}
	r.running = append(r.running, runningTool{
		id:    call.ID,
		name:  call.Name,
		label: transcript.ToolLabel(call.Name, call.Args),
		verb:  transcript.ToolVerb(call.Name),
	})
	if call.Name == "delegate" {
		r.startSubagent(call)
	}
	r.clearBlock()
	r.renderBlock()
}

func (r *tuiRenderer) toolFinish(res core.ToolResult) {
	r.mu.Lock()
	defer r.mu.Unlock()
	label := "tool"
	for i, rt := range r.running {
		if rt.id == res.CallID {
			label = rt.label
			r.running = append(r.running[:i], r.running[i+1:]...)
			if rt.name == "delegate" {
				r.finishSubagent(res.CallID)
			}
			break
		}
	}
	r.total++
	name, _, _ := strings.Cut(label, " ")
	r.counts[name]++
	if res.IsError {
		r.fails++
		r.clearBlock()
		r.printTranscriptLine(r.formatToolDone(label, res, ""))
		r.renderBlock()
		return
	}
	diff := transcript.DiffOf(res)
	r.clearBlock()
	r.printTranscriptLine(r.formatToolDone(label, res, diff))
	r.renderBlock()
}

func (r *tuiRenderer) approvalPrompt(call core.ToolCall) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.clearBlock()
	r.printFinalAnswer()
	r.promptKind = promptApproval
	r.approval = transcript.ToolLabel(call.Name, call.Args)
	r.renderBlock()
}

func (r *tuiRenderer) turnComplete(reason core.StopReason) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.clearBlock()
	r.printFinalAnswer()
	if reason != core.StopAnswered {
		r.printTranscriptLine(r.dim.Render("· stopped: " + string(reason)))
	}
	r.printTally()
	_, _ = fmt.Fprintln(r.out)
	r.resetTurn()
	r.promptKind = promptIdle
	r.renderBlock()
}

func (r *tuiRenderer) promptFollowUp() {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.promptKind = promptIdle
	r.clearBlock()
	r.renderBlock()
}

func (r *tuiRenderer) steeringPrompt() {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.promptKind = promptSteer
	r.clearBlock()
	r.renderBlock()
}

func (r *tuiRenderer) commitPrompt(string) {}

func (r *tuiRenderer) printStanding(lines []string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.clearBlock()
	const maxStanding = 3
	for i, ln := range lines {
		if i == maxStanding {
			_, _ = fmt.Fprintln(r.out, r.faint.Render(fmt.Sprintf("  … and %d more standing obligations", len(lines)-maxStanding)))
			break
		}
		_, _ = fmt.Fprintln(r.out, r.formatStandingLine(ln))
	}
	_, _ = fmt.Fprintln(r.out)
	r.renderBlock()
}

func (r *tuiRenderer) formatStandingLine(ln string) string {
	if rest, found := strings.CutPrefix(ln, "↻ "); found {
		return r.faint.Render("↻ ") + r.muted.Render(rest)
	}
	return r.muted.Render(ln)
}

func (r *tuiRenderer) notice(msgs []string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.clearBlock()
	for _, m := range msgs {
		_, _ = fmt.Fprintln(r.out, r.note.Render("◇ ")+sanitizeNotice(m))
	}
	_, _ = fmt.Fprintln(r.out)
	r.renderBlock()
}

func (r *tuiRenderer) interrupted() {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.clearBlock()
	r.printFinalAnswer()
	r.printTranscriptLine(r.dim.Render("· interrupted"))
	_, _ = fmt.Fprintln(r.out)
	r.resetTurn()
	r.promptKind = promptIdle
	r.renderBlock()
}

// steerCut ends the cut-off turn's rendering. The partial answer was already
// committed by submitLine (which printed the steer text below it); this only
// resets per-turn state — no "· interrupted" line, a steer is a silent cut.
func (r *tuiRenderer) steerCut() {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.clearBlock()
	r.printFinalAnswer()
	r.resetTurn()
}

func (r *tuiRenderer) errorf(e core.ErrorEvent) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.clearBlock()
	r.printFinalAnswer()
	r.printTranscriptLine(r.bad.Render("· error (" + string(e.Category) + "): " + e.Err))
	_, _ = fmt.Fprintln(r.out)
	r.resetTurn()
	r.promptKind = promptIdle
	r.renderBlock()
}

func (r *tuiRenderer) close() {
	r.stopOnce.Do(func() { close(r.stop) })
}

func (r *tuiRenderer) setInput(s string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.input.Reset()
	r.input.WriteString(s)
	// Rewrite only the prompt row in place — a full block repaint on every
	// keystroke was appending lines into scrollback (the staircase bug).
	if r.blockLines > 0 {
		r.redrawPromptLine()
		return
	}
	r.renderBlock()
}

func (r *tuiRenderer) redrawPromptLine() {
	_, _ = fmt.Fprint(r.out, "\r\x1b[2K")
	_, _ = fmt.Fprint(r.out, r.bandBodyLine(r.promptText(), theme.Panel))
	col := r.promptCursorCol()
	_, _ = fmt.Fprintf(r.out, "\x1b[%dG", col)
}

func (r *tuiRenderer) inputString() string {
	r.mu.Lock()
	defer r.mu.Unlock()
	return r.input.String()
}

// submitLine commits the typed line into the permanent transcript. Mid-turn
// (a steer), the cut-off turn's partial answer is rendered above it first so
// nothing the model already said is lost from scrollback.
func (r *tuiRenderer) submitLine() string {
	r.mu.Lock()
	defer r.mu.Unlock()
	line := strings.TrimSpace(r.input.String())
	r.input.Reset()
	if line == "" {
		r.clearBlock()
		r.renderBlock()
		return ""
	}
	r.clearBlock()
	if r.turnOpen {
		r.printFinalAnswer()
		_, _ = fmt.Fprintln(r.out)
	}
	r.printQueryUnlocked(line)
	return line
}

func (r *tuiRenderer) printQueryUnlocked(line string) {
	line = strings.TrimSpace(line)
	if line == "" {
		return
	}
	r.printLines(r.formatQueryBand(line))
	_, _ = fmt.Fprintln(r.out)
}

func (r *tuiRenderer) formatQueryBand(line string) string {
	body := lipgloss.NewStyle().Foreground(theme.Text).Render(line)
	lines := r.bandLines(body, theme.Card)
	return strings.Join(lines, "\n")
}

func (r *tuiRenderer) formatToolDone(label string, res core.ToolResult, diff string) string {
	if res.IsError {
		return r.bad.Render(transcript.ToolDone(label, res))
	}
	tool, arg, _ := strings.Cut(label, " ")
	var line string
	switch tool {
	case "write":
		line = r.ok.Render("Wrote ") + r.cmd.Render(arg)
	case "edit":
		line = r.ok.Render("Edited ") + r.cmd.Render(arg)
	case "bash":
		line = r.ok.Render("Ran ") + r.muted.Render(clip(arg, 60))
	default:
		line = r.ok.Render("✓ ") + r.muted.Render(label)
	}
	if diff != "" {
		add, del := transcript.DiffStat(diff)
		line += r.faint.Render(fmt.Sprintf(" (+%d -%d)", add, del))
	}
	return line
}

func expandPhysicalLines(lines []string) []string {
	var out []string
	for _, ln := range lines {
		ln = strings.TrimRight(ln, "\r\n")
		if ln == "" {
			out = append(out, "")
			continue
		}
		out = append(out, strings.Split(ln, "\n")...)
	}
	return out
}

func (r *tuiRenderer) printLines(styled string) {
	for _, ln := range expandPhysicalLines([]string{styled}) {
		_, _ = fmt.Fprintln(r.out, ln)
	}
}

// printQuery commits a user message into the permanent transcript as a
// full-width query band — the Cursor-style highlighted prompt row.
func (r *tuiRenderer) printQuery(line string) {
	line = strings.TrimSpace(line)
	if line == "" {
		return
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	r.clearBlock()
	r.printQueryUnlocked(line)
}

// tick animates the spinners while the block is showing any: waiting for the
// first token, between prose, or while tools (including delegated agents) run.
func (r *tuiRenderer) tick() {
	t := time.NewTicker(liveTick)
	defer t.Stop()
	for {
		select {
		case <-r.stop:
			return
		case <-t.C:
			r.mu.Lock()
			if r.turnOpen {
				r.frame++
				r.clearBlock()
				r.renderBlock()
			}
			r.mu.Unlock()
		}
	}
}

// --- block painting (callers hold r.mu) ---

// clearBlock erases the repaintable block. The cursor usually rests on the
// prompt row (not the block's last line), so we descend to the bottom row
// first — otherwise climbing blockLines-1 overshoots into scrollback and
// deletes transcript lines (the cascading-delete bug).
func (r *tuiRenderer) clearBlock() {
	if r.blockLines == 0 {
		return
	}
	if down := r.blockLines - 1 - r.promptIdx; down > 0 {
		_, _ = fmt.Fprintf(r.out, "\x1b[%dB", down)
	}
	if r.blockLines > 1 {
		_, _ = fmt.Fprintf(r.out, "\x1b[%dA", r.blockLines-1)
	}
	_, _ = fmt.Fprint(r.out, "\r\x1b[J")
	r.blockLines = 0
	r.promptIdx = 0
}

func (r *tuiRenderer) refreshSize() {
	if w, _, err := term.GetSize(r.fd); err == nil && w > 0 {
		r.width = w
	}
}

// blockContent builds the live chrome below scrollback: turn activity, the
// follow-up prompt, and the model/path footer.
// promptIdx is the 0-based row of the input line within the expanded block.
func (r *tuiRenderer) blockContent() (lines []string, promptIdx int) {
	if r.turnOpen && r.promptKind != promptApproval {
		lines = append(lines, r.windowLines()...)
		if r.subagentPanelActive() {
			lines = append(lines, r.subagentLines()...)
		} else {
			if r.ephemeral != "" {
				lines = append(lines, indentTranscript(r.dim.Render(r.previewLine())))
			}
			lines = append(lines, indentTranscriptLines(r.activityLines())...)
			lines = append(lines, indentTranscript(r.workingLine()))
		}
		lines = append(lines, "")
	}
	pre := len(expandPhysicalLines(lines))
	band := r.bandLines(r.promptText(), theme.Panel)
	promptIdx = pre + bandBodyOffset
	lines = append(lines, band...)
	lines = append(lines, "")
	lines = append(lines, r.footerLines()...)
	return lines, promptIdx
}

// renderBlock paints the block inline after scrollback — no absolute positioning,
// so the transcript stays scrollable and there is no dead space gap.
func (r *tuiRenderer) renderBlock() {
	r.refreshSize()
	r.clearBlock()
	raw, promptIdx := r.blockContent()
	lines := expandPhysicalLines(raw)
	r.promptIdx = promptIdx
	for i, ln := range lines {
		if i < len(lines)-1 {
			_, _ = fmt.Fprintln(r.out, ln)
		} else {
			_, _ = fmt.Fprint(r.out, ln)
		}
	}
	r.blockLines = len(lines)
	r.placePromptCursor(promptIdx)
}

// placePromptCursor moves the terminal cursor onto the input line, right after
// the → prefix — not on the footer row at the bottom of the block.
func (r *tuiRenderer) placePromptCursor(promptIdx int) {
	if r.blockLines == 0 || promptIdx < 0 {
		return
	}
	up := r.blockLines - 1 - promptIdx
	if up > 0 {
		_, _ = fmt.Fprintf(r.out, "\x1b[%dA", up)
	}
	_, _ = fmt.Fprint(r.out, "\r")
	col := r.promptCursorCol()
	_, _ = fmt.Fprintf(r.out, "\x1b[%dG", col)
}

func (r *tuiRenderer) promptCursorCol() int {
	const arrow = "→ "
	if r.promptKind == promptApproval {
		prefix := "approve " + r.approval + "? [y/N] "
		return bandPad + lipgloss.Width(arrow+prefix+r.input.String()) + 1
	}
	in := r.clipInput()
	if in != "" {
		return bandPad + lipgloss.Width(arrow+in) + 1
	}
	return bandPad + lipgloss.Width(arrow) + 1
}

// answerWindow is how many lines of the streaming answer stay visible in the
// block; older lines roll off (the full answer lands in scrollback at the end).
const answerWindow = 6

// windowLines is the block's streaming region: the last few wrapped lines of
// the in-flight answer, with a dim spinner row whenever the model is between
// lines (thinking, tools running). The "Working" status line is separate.
func (r *tuiRenderer) windowLines() []string {
	if !r.started {
		return nil
	}
	spin := transcript.Spinner[r.frame%len(transcript.Spinner)]
	window := r.answerLines
	if r.tail != "" {
		window = append(window[:len(window):len(window)], r.tail)
	}
	if len(window) > answerWindow {
		window = window[len(window)-answerWindow:]
	}
	if r.tail == "" {
		window = append(window[:len(window):len(window)], indentTranscript(r.dim.Render(spin)))
	}
	return window
}

func (r *tuiRenderer) workingLine() string {
	dots := lipgloss.NewStyle().Foreground(theme.Status).Render("::")
	return dots + " " + r.working.Render("Working")
}

func (r *tuiRenderer) footerLines() []string {
	path := r.meta.CWD
	if r.meta.Branch != "" {
		path += " · " + r.meta.Branch
	}
	return []string{
		r.muted.Render(r.meta.Model),
		r.muted.Render(path),
	}
}

// activityLines is the live tool telemetry: one spinner line per kind of work
// in flight ("delegating", "reading files (2)" — child agents' tools included),
// capped, plus the running tally.
func (r *tuiRenderer) activityLines() []string {
	groups := r.activity()
	if len(groups) == 0 {
		return nil
	}
	spin := transcript.Spinner[r.frame%len(transcript.Spinner)]
	var out []string
	for i, g := range groups {
		if i == liveSpinnerRows {
			out = append(out, r.dim.Render("…"))
			break
		}
		label := g.verb
		if g.n > 1 {
			label = fmt.Sprintf("%s (%d)", g.verb, g.n)
		}
		out = append(out, r.tool.Render(spin+" "+clip(label, r.width-5)))
	}
	return out
}

func (r *tuiRenderer) promptText() string {
	if r.promptKind == promptApproval {
		return ansiDim + "→ " + ansiReset + r.bright.Render("approve "+r.approval+"? [y/N] ") + r.bright.Render(r.input.String())
	}
	in := r.clipInput()
	if in != "" {
		return ansiDim + "→ " + ansiReset + r.bright.Render(in)
	}
	// Placeholder ghost text — dim, not body-bright.
	return ansiDim + "→ " + followUpHint + ansiReset
}

// clipInput keeps the prompt to one physical line (the block's line math
// depends on it): when the input overflows, show its tail.
func (r *tuiRenderer) clipInput() string {
	in := r.input.String()
	maxw := r.width - 4
	if maxw < 8 || lipgloss.Width(in) <= maxw {
		return in
	}
	runes := []rune(in)
	for lipgloss.Width(string(runes)) > maxw-1 && len(runes) > 0 {
		runes = runes[1:]
	}
	return "…" + string(runes)
}

// printFinalAnswer renders the turn's complete answer into the permanent
// transcript in one pass — fresh markdown styling and a full reflow — so
// scrollback never carries streaming artifacts. The streaming window state is
// consumed; callers print status lines (tally, interrupted) after it.
func (r *tuiRenderer) printFinalAnswer() {
	raw := strings.TrimSpace(r.answerRaw.String())
	r.answerRaw.Reset()
	r.answerLines = nil
	r.tail = ""
	r.started = false
	r.ephemeral = ""
	r.md = newTUIMarkdownStyler(r.out)
	r.proseWrap = newTUIProseWrapper(r.width)
	if raw == "" {
		return
	}
	md := newTUIMarkdownStyler(r.out)
	w := newTUIProseWrapper(r.width)
	w.startAfterPrefix("")
	styled := md.Push(raw)
	if t := md.Flush(); t != "" {
		styled += t
	}
	text := w.write(styled)
	if t := w.flush(); t != "" {
		text += t
	}
	_, _ = fmt.Fprint(r.out, text)
	if !strings.HasSuffix(text, "\n") {
		_, _ = fmt.Fprintln(r.out)
	}
}

func (r *tuiRenderer) printTally() {
	if t := r.tally(); t != "" {
		_, _ = fmt.Fprintln(r.out, r.rightAlign(r.dim.Render("· "+t), "· "+t))
	}
}

func (r *tuiRenderer) tally() string {
	return transcript.Tally(r.total, r.fails, r.counts)
}

func (r *tuiRenderer) rightAlign(styled, plain string) string {
	pad := r.width - lipgloss.Width(plain)
	if pad <= 0 {
		return styled
	}
	return strings.Repeat(" ", pad) + styled
}

type verbGroup struct {
	verb string
	n    int
}

func (r *tuiRenderer) activity() []verbGroup {
	var groups []verbGroup
	idx := map[string]int{}
	for _, rt := range r.running {
		if rt.name == "delegate" {
			continue
		}
		if i, ok := idx[rt.verb]; ok {
			groups[i].n++
			continue
		}
		idx[rt.verb] = len(groups)
		groups = append(groups, verbGroup{verb: rt.verb, n: 1})
	}
	return groups
}

func (r *tuiRenderer) resetTurn() {
	r.turnOpen = false
	r.started = false
	r.tail = ""
	r.answerRaw.Reset()
	r.answerLines = nil
	r.ephemeral = ""
	r.running = nil
	r.subagents = nil
	r.counts = map[string]int{}
	r.total = 0
	r.fails = 0
	r.proseWrap = newTUIProseWrapper(r.width)
	r.md = newTUIMarkdownStyler(r.out)
}

func printSessionResume(out io.Writer, sessionID string) {
	if sessionID == "" {
		return
	}
	lr := lipgloss.NewRenderer(out)
	muted := lr.NewStyle().Foreground(theme.Muted)
	cmd := lr.NewStyle().Foreground(theme.Command)
	_, _ = fmt.Fprintln(out)
	_, _ = fmt.Fprint(out, muted.Render("To resume this session: "))
	_, _ = fmt.Fprintln(out, cmd.Render("arbos resume "+sessionID))
}

func runTUISession(ctx context.Context, conv *engine.Conversation, r *tuiRenderer, sessionID string, deliver func() bool, announceStanding func(bool), task string, expand func(string) string) error {
	r.sessionID = sessionID
	oldState, err := term.MakeRaw(os.Stdin.Fd())
	if err != nil {
		return err
	}
	defer func() {
		r.mu.Lock()
		r.clearBlock()
		sid := r.sessionID
		r.mu.Unlock()
		_ = term.Restore(os.Stdin.Fd(), oldState)
		_, _ = fmt.Fprint(os.Stderr, "\x1b[0m\n")
		printSessionResume(os.Stderr, sid)
	}()

	inputs := startRawInput(ctx, os.Stdin)
	turnActive := false
	var approvalID core.RequestID

	sendPrompt := func(text string, echo bool) {
		if echo {
			r.printQuery(text)
		}
		conv.Send(core.PromptIntent{Text: text})
		turnActive = true
		r.turnStart()
		r.steeringPrompt()
	}
	steer := func(text string) {
		r.steerCut()
		conv.Send(core.SteerIntent{Text: expand(text)})
		turnActive = true
		r.turnStart()
		r.steeringPrompt()
	}

	if task != "" {
		sendPrompt(task, true)
	} else {
		r.promptFollowUp()
	}

	events := conv.Events()
	for {
		select {
		case <-ctx.Done():
			return ctx.Err()
		case err := <-inputs.errs:
			return err
		case ev := <-inputs.events:
			switch ev.kind {
			case rawRune:
				r.setInput(r.inputString() + string(ev.r))
			case rawBackspace:
				s := r.inputString()
				if s != "" {
					rs := []rune(s)
					r.setInput(string(rs[:len(rs)-1]))
				}
			case rawClear:
				r.setInput("")
			case rawCtrlC:
				if turnActive {
					conv.Send(core.InterruptIntent{})
				} else {
					return nil
				}
			case rawCtrlD:
				if r.inputString() == "" {
					return nil
				}
			case rawEnter:
				line := r.submitLine()
				if approvalID != "" {
					approved := strings.EqualFold(strings.TrimSpace(line), "y")
					conv.Send(core.ApprovalResponseIntent{RequestID: approvalID, Approved: approved})
					approvalID = ""
					if turnActive {
						r.steeringPrompt()
					} else {
						r.promptFollowUp()
					}
					continue
				}
				switch line {
				case "":
					// submitLine already re-rendered the block.
				case "exit", "quit", "q":
					return nil
				default:
					if turnActive {
						steer(line)
					} else {
						sendPrompt(expand(line), false)
					}
				}
			}
		case env, ok := <-events:
			if !ok {
				return nil
			}
			child := env.Depth > 0
			switch e := env.Event.(type) {
			case core.MessageDelta:
				if child {
					continue
				}
				r.delta(e.Text)
			case core.ReasoningDelta:
				r.preview(e.Text)
			case core.ApprovalRequest:
				approvalID = e.RequestID
				r.approvalPrompt(e.Call)
			case core.ToolStarted:
				if child {
					r.withChildUpdate(func() { r.childToolStart(env.SessionID, e.Call) })
				} else {
					r.toolStart(e.Call)
				}
			case core.ToolFinished:
				if child {
					r.withChildUpdate(func() { r.childToolFinish(env.SessionID, e.Result) })
				} else {
					r.toolFinish(e.Result)
				}
			case core.TurnComplete:
				if child {
					r.withChildUpdate(func() { r.childTurnComplete(env.SessionID, e.Usage) })
					continue
				}
				r.turnComplete(e.StopReason)
				announceStanding(true)
				deliver()
				turnActive = false
			case core.Interrupted:
				if !child {
					r.interrupted()
					turnActive = false
				}
			case core.ErrorEvent:
				if !child {
					r.errorf(e)
					return fmt.Errorf("%s: %s", e.Category, e.Err)
				}
			}
		case <-time.After(outboxPoll):
			deliver()
		}
	}
}

type rawEventKind int

const (
	rawRune rawEventKind = iota
	rawEnter
	rawBackspace
	rawClear
	rawCtrlC
	rawCtrlD
)

type rawEvent struct {
	kind rawEventKind
	r    rune
}

type rawInput struct {
	events chan rawEvent
	errs   chan error
}

func startRawInput(ctx context.Context, in io.Reader) rawInput {
	out := rawInput{events: make(chan rawEvent, 32), errs: make(chan error, 1)}
	go func() {
		br := bufio.NewReader(in)
		for {
			r, _, err := br.ReadRune()
			if err != nil {
				out.errs <- err
				return
			}
			var ev rawEvent
			switch r {
			case '\r', '\n':
				ev.kind = rawEnter
			case 0x03:
				ev.kind = rawCtrlC
			case 0x04:
				ev.kind = rawCtrlD
			case 0x15:
				ev.kind = rawClear
			case 0x7f, '\b':
				ev.kind = rawBackspace
			case 0x1b:
				discardEscape(br)
				continue
			default:
				if r < 0x20 {
					continue
				}
				ev = rawEvent{kind: rawRune, r: r}
			}
			select {
			case <-ctx.Done():
				return
			case out.events <- ev:
			}
		}
	}()
	return out
}

func discardEscape(br *bufio.Reader) {
	next, err := br.Peek(1)
	if err != nil || len(next) == 0 {
		return
	}
	if next[0] != '[' && next[0] != 'O' {
		_, _, _ = br.ReadRune()
		return
	}
	_, _, _ = br.ReadRune()
	for {
		r, _, err := br.ReadRune()
		if err != nil {
			return
		}
		if r >= '@' && r <= '~' {
			return
		}
	}
}
