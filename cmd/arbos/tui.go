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
	blockLines  int // physical lines the block currently occupies (0 = none)

	// ephemeral is a one-line live preview of activity that never reaches the
	// permanent transcript: the model's reasoning stream, or a delegated
	// child's prose (which is internal narration, not this turn's answer).
	ephemeral string

	proseWrap proseWrapper
	md        *transcript.MarkdownStyler

	running []runningTool
	counts  map[string]int
	total   int
	fails   int

	dim, ok, bad, tool, note, standing lipgloss.Style
	standingText                       lipgloss.Style
	agent                              lipgloss.Style

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

func newTUIRenderer(out io.Writer, width int) *tuiRenderer {
	if width <= 0 {
		width = 80
	}
	out = rawTerminalWriter{w: out}
	lr := lipgloss.NewRenderer(out)
	r := &tuiRenderer{
		out:          out,
		width:        width,
		counts:       map[string]int{},
		stop:         make(chan struct{}),
		proseWrap:    newProseWrapper(width),
		md:           newMarkdownStyler(out),
		dim:          lr.NewStyle().Foreground(theme.Muted),
		ok:           lr.NewStyle().Foreground(theme.Primary),
		bad:          lr.NewStyle().Bold(true).Foreground(theme.Text).Background(theme.Deep),
		tool:         lr.NewStyle().Foreground(theme.Accent),
		note:         lr.NewStyle().Bold(true).Foreground(theme.Accent),
		standing:     lr.NewStyle().Bold(true).Foreground(theme.Accent),
		standingText: lr.NewStyle().Bold(true),
		agent:        lr.NewStyle().Bold(true).Foreground(theme.Primary),
	}
	go r.tick()
	return r
}

func (r *tuiRenderer) header(string) {}

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
		r.tail = r.agent.Render(agentIcon) + " "
		r.proseWrap.startAfterPrefix(agentIcon + " ")
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
		label: transcript.ToolLabel(call.Name, call.Args),
		verb:  transcript.ToolVerb(call.Name),
	})
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
			break
		}
	}
	r.total++
	name, _, _ := strings.Cut(label, " ")
	r.counts[name]++
	if res.IsError {
		r.fails++
		r.clearBlock()
		_, _ = fmt.Fprintln(r.out, "  "+r.bad.Render(transcript.ToolDone(label, res)))
		r.renderBlock()
		return
	}
	if diff := transcript.DiffOf(res); diff != "" {
		add, del := transcript.DiffStat(diff)
		r.clearBlock()
		_, _ = fmt.Fprintln(r.out, "  "+r.ok.Render(fmt.Sprintf("%s (+%d -%d)", transcript.ToolDone(label, res), add, del)))
		r.renderBlock()
		return
	}
	r.clearBlock()
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
		_, _ = fmt.Fprintln(r.out, r.dim.Render("· stopped: "+string(reason)))
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
	for _, ln := range lines {
		_, _ = fmt.Fprintln(r.out, renderStandingLine(r.standing, r.standingText, ln))
	}
	_, _ = fmt.Fprintln(r.out)
	r.renderBlock()
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
	_, _ = fmt.Fprintln(r.out, r.dim.Render("· interrupted"))
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
	_, _ = fmt.Fprintln(r.out, r.bad.Render("· error ("+string(e.Category)+"): "+e.Err))
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
	if r.blockLines == 0 {
		r.renderBlock()
		return
	}
	// The cursor already sits on the prompt line (last block line); rewrite it
	// in place without disturbing the rest of the block.
	_, _ = fmt.Fprint(r.out, "\r\x1b[K"+r.promptLine())
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
	_, _ = fmt.Fprintln(r.out, r.note.Render("› ")+line)
	_, _ = fmt.Fprintln(r.out)
	return line
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
			if r.turnOpen && (r.tail == "" || len(r.running) > 0) {
				r.frame++
				r.clearBlock()
				r.renderBlock()
			}
			r.mu.Unlock()
		}
	}
}

// --- block painting (callers hold r.mu) ---

// clearBlock erases the repaintable block. The cursor always rests at the end
// of the block's last line, so climbing blockLines-1 rows reaches its top.
func (r *tuiRenderer) clearBlock() {
	if r.blockLines == 0 {
		return
	}
	if r.blockLines > 1 {
		_, _ = fmt.Fprintf(r.out, "\x1b[%dA", r.blockLines-1)
	}
	_, _ = fmt.Fprint(r.out, "\r\x1b[J")
	r.blockLines = 0
}

// renderBlock paints the block and records its exact line count: during a turn
// the in-progress line (spinner or streaming tail), live tool activity (one
// spinner line per verb, plus the running tally), a blank, then the prompt;
// idle, just the prompt.
func (r *tuiRenderer) renderBlock() {
	var lines []string
	if r.turnOpen && r.promptKind != promptApproval {
		lines = append(lines, r.windowLines()...)
		if r.ephemeral != "" {
			lines = append(lines, proseIndent+r.dim.Render(r.previewLine()))
		}
		lines = append(lines, r.activityLines()...)
		lines = append(lines, "")
	}
	lines = append(lines, r.promptLine())
	for i, ln := range lines {
		if i < len(lines)-1 {
			_, _ = fmt.Fprintln(r.out, ln)
		} else {
			_, _ = fmt.Fprint(r.out, ln)
		}
	}
	r.blockLines = len(lines)
}

// answerWindow is how many lines of the streaming answer stay visible in the
// block; older lines roll off (the full answer lands in scrollback at the end).
const answerWindow = 6

// windowLines is the block's streaming region: a ◆ spinner before the first
// token, then the last few wrapped lines of the in-flight answer, with a dim
// spinner row whenever the model is between lines (thinking, tools running).
func (r *tuiRenderer) windowLines() []string {
	spin := transcript.Spinner[r.frame%len(transcript.Spinner)]
	if !r.started {
		return []string{r.agent.Render(agentIcon) + " " + r.dim.Render(spin)}
	}
	window := r.answerLines
	if r.tail != "" {
		window = append(window[:len(window):len(window)], r.tail)
	}
	if len(window) > answerWindow {
		window = window[len(window)-answerWindow:]
	}
	if r.tail == "" {
		window = append(window[:len(window):len(window)], proseIndent+r.dim.Render(spin))
	}
	return window
}

// activityLines is the live tool telemetry: one spinner line per kind of work
// in flight ("delegating", "reading files (2)" — child agents' tools included),
// capped, plus the running tally.
func (r *tuiRenderer) activityLines() []string {
	groups := r.activity()
	if len(groups) == 0 && r.total == 0 {
		return nil
	}
	spin := transcript.Spinner[r.frame%len(transcript.Spinner)]
	var out []string
	for i, g := range groups {
		if i == liveSpinnerRows {
			out = append(out, "  "+r.dim.Render("…"))
			break
		}
		label := g.verb
		if g.n > 1 {
			label = fmt.Sprintf("%s (%d)", g.verb, g.n)
		}
		out = append(out, "  "+r.tool.Render(spin+" "+clip(label, r.width-5)))
	}
	if t := r.tally(); t != "" {
		out = append(out, "  "+r.dim.Render(clip("· "+t, r.width-3)))
	}
	return out
}

func (r *tuiRenderer) promptLine() string {
	if r.promptKind == promptApproval {
		return r.note.Render("  approve "+r.approval+"? [y/N] ") + r.input.String()
	}
	return r.note.Render("› ") + r.clipInput()
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
	r.md = newMarkdownStyler(r.out)
	r.proseWrap = newProseWrapper(r.width)
	if raw == "" {
		return
	}
	md := newMarkdownStyler(r.out)
	w := newProseWrapper(r.width)
	w.startAfterPrefix(agentIcon + " ")
	styled := md.Push(raw)
	if t := md.Flush(); t != "" {
		styled += t
	}
	text := w.write(styled)
	if t := w.flush(); t != "" {
		text += t
	}
	_, _ = fmt.Fprint(r.out, r.agent.Render(agentIcon)+" ")
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
	r.counts = map[string]int{}
	r.total = 0
	r.fails = 0
	r.proseWrap = newProseWrapper(r.width)
	r.md = newMarkdownStyler(r.out)
}

func runTUISession(ctx context.Context, conv *engine.Conversation, r *tuiRenderer, deliver func() bool, announceStanding func(bool), task string, expand func(string) string) error {
	oldState, err := term.MakeRaw(os.Stdin.Fd())
	if err != nil {
		return err
	}
	defer func() {
		_ = term.Restore(os.Stdin.Fd(), oldState)
		fmt.Fprintln(os.Stderr)
	}()

	inputs := startRawInput(ctx, os.Stdin)
	turnActive := false
	var approvalID core.RequestID

	sendPrompt := func(text string) {
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
		sendPrompt(task)
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
						sendPrompt(expand(line))
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
					// A delegated child's prose is internal narration, not this
					// turn's answer — preview it live, keep it out of scrollback.
					r.preview(e.Text)
				} else {
					r.delta(e.Text)
				}
			case core.ReasoningDelta:
				r.preview(e.Text)
			case core.ApprovalRequest:
				approvalID = e.RequestID
				r.approvalPrompt(e.Call)
			case core.ToolStarted:
				r.toolStart(e.Call)
			case core.ToolFinished:
				r.toolFinish(e.Result)
			case core.TurnComplete:
				if child {
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
