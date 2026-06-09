package main

import (
	"fmt"
	"io"
	"strings"
	"sync"
	"time"

	"github.com/charmbracelet/lipgloss"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/theme"
	"github.com/unarbos/arbos/internal/transcript"
)

// liveRenderer is the TTY one-shot front-end. The permanent transcript is the
// assistant's prose plus anything that went wrong; successful tool activity
// only ever exists in a bounded repaintable region at the bottom — capped
// spinner rows for running tools, a small rounded box through which recent
// output rotates, and a running tally. When the turn completes the region
// collapses to a single dim summary line, so a 100-tool repo crawl leaves a
// one-line footprint instead of a scrollback full of checkmarks.
type liveRenderer struct {
	mu     sync.Mutex
	prose  io.Writer // assistant text (stdout)
	status io.Writer // live region + persistent failure lines (stderr, a TTY)

	midText     bool // prose cursor is mid-line; suppress repaints until newline
	suspended   bool // approval prompt is waiting on stdin; freeze the region
	turnOpen    bool // a turn has emitted content and not yet been collapsed
	atLineStart bool // next prose byte begins a fresh line (needs indent)

	running []runningTool
	box     []string // rotating tail of recent tool output
	live    int      // lines currently painted in the live region
	frame   int
	width   int

	counts map[string]int // finished-tool tally by tool name
	total  int
	fails  int

	stop     chan struct{}
	stopOnce sync.Once

	md *transcript.MarkdownStyler

	dim, ok, bad, tool, note, del lipgloss.Style
	agent                         lipgloss.Style
	boxStyle                      lipgloss.Style
}

type runningTool struct {
	id    string
	label string
}

const (
	liveBoxLines    = 4
	liveSpinnerRows = 4
	liveDiffLines   = 24
	liveTick        = 120 * time.Millisecond
)

func newLiveRenderer(prose, status io.Writer, width int) *liveRenderer {
	if width <= 0 {
		width = 80
	}
	lr := lipgloss.NewRenderer(status)
	r := &liveRenderer{
		prose:  prose,
		status: status,
		width:  width,
		counts: map[string]int{},
		stop:   make(chan struct{}),
		md:     newMarkdownStyler(prose),
		dim:    lr.NewStyle().Foreground(theme.Muted),
		ok:     lr.NewStyle().Foreground(theme.Primary),
		bad:    lr.NewStyle().Bold(true).Foreground(theme.Text).Background(theme.Deep),
		tool:   lr.NewStyle().Foreground(theme.Accent),
		note:   lr.NewStyle().Bold(true).Foreground(theme.Accent),
		del:    lr.NewStyle().Foreground(theme.Muted).Strikethrough(true),
		agent:  lr.NewStyle().Bold(true).Foreground(theme.Primary),
		boxStyle: lr.NewStyle().
			Border(lipgloss.RoundedBorder()).
			BorderForeground(theme.Deep).
			PaddingLeft(1).PaddingRight(1),
	}
	r.atLineStart = true
	go r.tick()
	return r
}

// agentLabel opens an agent turn (caller holds r.mu): a blank line then a
// distinct colored icon so the agent's side is visibly distinct from the
// user's, without repeating the "Arbos" name every turn.
func (r *liveRenderer) agentLabel() {
	if r.turnOpen {
		return
	}
	r.turnOpen = true
	r.erase()
	_, _ = fmt.Fprintln(r.status)
	_, _ = fmt.Fprintln(r.status, r.agent.Render(agentIcon))
}

// tick advances the spinner while tools are running. Repaints happen inline on
// events too; the ticker only keeps the spinner alive between them.
func (r *liveRenderer) tick() {
	t := time.NewTicker(liveTick)
	defer t.Stop()
	for {
		select {
		case <-r.stop:
			return
		case <-t.C:
			r.mu.Lock()
			if len(r.running) > 0 {
				r.frame++
				r.repaint()
			}
			r.mu.Unlock()
		}
	}
}

func (r *liveRenderer) header(id string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.persist(r.agent.Render("Arbos") + r.dim.Render("  "+id))
}

func (r *liveRenderer) delta(text string) {
	if text == "" {
		return
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	r.suspended = false
	r.agentLabel()
	r.erase()
	if styled := r.md.Push(text); styled != "" {
		_, _ = fmt.Fprint(r.prose, indentStream(styled, &r.atLineStart))
	}
	r.midText = !strings.HasSuffix(text, "\n")
	r.repaint()
}

func (r *liveRenderer) toolStart(call core.ToolCall) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.suspended = false
	r.agentLabel()
	r.endProse()
	r.running = append(r.running, runningTool{id: call.ID, label: transcript.ToolLabel(call.Name, call.Args)})
	r.repaint()
}

func (r *liveRenderer) toolFinish(res core.ToolResult) {
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
	// Failures and edit diffs earn permanent lines — they're what went wrong
	// and what changed. Everything else is just a tick in the tally.
	if res.IsError {
		r.fails++
		r.persist("  " + r.bad.Render(transcript.ToolDone(label, res)))
		return
	}
	if diff := transcript.DiffOf(res); diff != "" {
		lines := append(
			[]string{"  " + r.ok.Render(transcript.ToolDone(label, res))},
			styledDiff(diff, liveDiffLines, r.ok, r.del, r.dim)...,
		)
		r.persist(lines...)
		return
	}
	if lines := transcript.ToolOutputPreview(label, res.Content); len(lines) > 0 {
		r.box = append(r.box, lines...)
		if len(r.box) > liveBoxLines {
			r.box = r.box[len(r.box)-liveBoxLines:]
		}
	}
	r.repaint()
}

func (r *liveRenderer) tally() string {
	return transcript.Tally(r.total, r.fails, r.counts)
}

func (r *liveRenderer) approvalPrompt(call core.ToolCall) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.erase()
	r.endProse()
	r.suspended = true
	label := transcript.ToolLabel(call.Name, call.Args)
	_, _ = fmt.Fprint(r.status, r.note.Render("  approve "+label+"? [y/N] "))
}

// turnComplete collapses the live region to a one-line tally and resets the
// per-turn state, ready for a follow-up prompt in the same session.
func (r *liveRenderer) turnComplete(reason core.StopReason) {
	r.finish(func() {
		if t := r.tally(); t != "" {
			_, _ = fmt.Fprintln(r.status, r.dim.Render("· "+t))
		}
		if reason != core.StopAnswered {
			_, _ = fmt.Fprintln(r.status, r.dim.Render("· stopped: "+string(reason)))
		}
	})
}

// promptFollowUp paints the input marker and freezes repaints until the next
// turn's events arrive (the user owns the cursor while typing).
func (r *liveRenderer) promptFollowUp() {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.erase()
	r.endProse()
	r.suspended = true
	_, _ = fmt.Fprint(r.status, r.note.Render("› "))
}

func (r *liveRenderer) interrupted() {
	r.finish(func() {
		_, _ = fmt.Fprintln(r.status, r.dim.Render("· interrupted"))
	})
}

func (r *liveRenderer) errorf(e core.ErrorEvent) {
	r.finish(func() {
		_, _ = fmt.Fprintln(r.status, r.bad.Render("· error ("+string(e.Category)+"): "+e.Err))
	})
}

// close stops the spinner ticker; call once when the program is done rendering.
func (r *liveRenderer) close() {
	r.stopOnce.Do(func() { close(r.stop) })
}

// finish collapses the live region and resets per-turn state: erase the box
// and tally, leaving only whatever closing line the turn deserves. The ticker
// keeps running — a follow-up turn may start in this same session.
func (r *liveRenderer) finish(closing func()) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.erase()
	r.endProse()
	closing()
	if r.turnOpen {
		_, _ = fmt.Fprintln(r.status)
	}
	r.turnOpen = false
	r.atLineStart = true
	r.box = nil
	r.running = nil
	r.counts = map[string]int{}
	r.total = 0
	r.fails = 0
}

// --- painting internals (callers hold r.mu) ---

// persist writes permanent transcript lines above the live region.
func (r *liveRenderer) persist(lines ...string) {
	r.erase()
	r.endProse()
	for _, line := range lines {
		_, _ = fmt.Fprintln(r.status, line)
	}
	r.repaint()
}

// erase removes the currently painted live region, leaving the cursor where
// persistent output should continue.
func (r *liveRenderer) erase() {
	if r.live > 0 {
		_, _ = fmt.Fprintf(r.status, "\x1b[%dA\x1b[J", r.live)
		r.live = 0
	}
}

// endProse terminates a mid-line prose stream so status output never runs into
// assistant text.
func (r *liveRenderer) endProse() {
	if tail := r.md.Flush(); tail != "" {
		_, _ = fmt.Fprint(r.prose, indentStream(tail, &r.atLineStart))
	}
	r.md = newMarkdownStyler(r.prose)
	if r.midText {
		_, _ = fmt.Fprintln(r.prose)
		r.midText = false
		r.atLineStart = true
	}
}

// repaint redraws the live region: one spinner line per running tool, then the
// rotating output box. Painted in a single write to avoid flicker. Suppressed
// while prose is mid-line or an approval prompt owns the cursor.
func (r *liveRenderer) repaint() {
	if r.midText || r.suspended {
		return
	}
	lines := r.liveLines()
	var b strings.Builder
	if r.live > 0 {
		fmt.Fprintf(&b, "\x1b[%dA\x1b[J", r.live)
	}
	for _, ln := range lines {
		b.WriteString(ln)
		b.WriteByte('\n')
	}
	r.live = len(lines)
	_, _ = io.WriteString(r.status, b.String())
}

func (r *liveRenderer) liveLines() []string {
	var out []string
	spin := transcript.Spinner[r.frame%len(transcript.Spinner)]
	for i, rt := range r.running {
		if i == liveSpinnerRows {
			out = append(out, "  "+r.dim.Render(fmt.Sprintf("… +%d more", len(r.running)-liveSpinnerRows)))
			break
		}
		out = append(out, "  "+r.tool.Render(spin+" "+clip(rt.label, r.width-5)))
	}
	if len(r.box) > 0 {
		w := r.width - 6
		if w > 100 {
			w = 100
		}
		var content []string
		for _, ln := range r.box {
			content = append(content, r.dim.Render(clip(ln, w-2)))
		}
		box := r.boxStyle.Width(w).Render(strings.Join(content, "\n"))
		for _, ln := range strings.Split(box, "\n") {
			out = append(out, "  "+ln)
		}
	}
	if t := r.tally(); t != "" {
		out = append(out, "  "+r.dim.Render(clip("· "+t, r.width-3)))
	}
	return out
}

// clip truncates to n display runes with an ellipsis; live-region lines must
// never wrap or the repaint math breaks.
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
