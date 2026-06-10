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
// assistant's prose plus anything that went wrong; in-flight tool activity only
// ever exists as a bounded repaintable region at the bottom — one spinner line
// per kind of work in progress ("reading files", "running commands"), never the
// tools' output. When the turn completes the region collapses to a single dim
// tally, so a 100-tool repo crawl leaves a one-line footprint instead of a
// scrollback full of dumps. A persistent footer (the plan/schedule glance) is
// pinned at the bottom of the same region, so the schedule is something you
// look at while you work rather than state reprinted at every door.
type liveRenderer struct {
	mu     sync.Mutex
	prose  io.Writer // assistant text (stdout)
	status io.Writer // live region + persistent failure lines (stderr, a TTY)

	midText     bool // prose cursor is mid-line; suppress repaints until newline
	suspended   bool // approval/follow-up prompt owns the cursor; freeze the region
	turnOpen    bool // a turn has emitted content and not yet been collapsed
	pendingIcon bool // turn opened; its ◆ marker not yet emitted
	atLineStart bool // next prose byte begins a fresh line (needs indent)

	running []runningTool
	footer  []string // cached status strip, painted at the bottom of the region
	live    int      // lines currently painted in the live region
	frame   int
	width   int

	counts map[string]int // finished-tool tally by tool name
	total  int
	fails  int

	stop     chan struct{}
	stopOnce sync.Once

	md *transcript.MarkdownStyler

	dim, ok, bad, tool, note lipgloss.Style
	agent                    lipgloss.Style
}

type runningTool struct {
	id    string
	label string // precise label (read foo.go) for the permanent diff/error line
	verb  string // gerund phrase (reading files) for the live spinner line
}

const (
	liveSpinnerRows = 5
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
		agent:  lr.NewStyle().Bold(true).Foreground(theme.Primary),
	}
	r.atLineStart = true
	go r.tick()
	return r
}

// agentLabel opens an agent turn (caller holds r.mu): a blank line for
// breathing room, then the turn's ◆ marker is deferred — prose prefixes the
// first line with it (delta), and a tool-first turn flushes it as its own line
// (flushIconLine), so the icon always sits beside the first thing the turn says.
func (r *liveRenderer) agentLabel() {
	if r.turnOpen {
		return
	}
	r.turnOpen = true
	r.erase()
	_, _ = fmt.Fprintln(r.status)
	r.pendingIcon = true
}

// flushIconLine emits the turn's ◆ on its own permanent line when the turn
// opens with tool activity (there is no prose to prefix). No-op once the icon
// has been emitted (e.g. prose already prefixed it).
func (r *liveRenderer) flushIconLine() {
	if r.pendingIcon {
		_, _ = fmt.Fprintln(r.status, r.agent.Render(agentIcon))
		r.pendingIcon = false
	}
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

// header is intentionally silent in the live UI: the session opens straight
// into its footer and prompt, not a banner. The id still lands in the store for
// -session resume.
func (r *liveRenderer) header(string) {}

func (r *liveRenderer) delta(text string) {
	if text == "" {
		return
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	r.suspended = false
	r.agentLabel()
	r.erase()
	if r.pendingIcon {
		// Prefix the first prose line with the turn's ◆ so the icon sits beside
		// the text, not on a line of its own. "◆ " is two columns, matching the
		// proseIndent that aligns every following line beneath it.
		_, _ = fmt.Fprint(r.prose, r.agent.Render(agentIcon)+" ")
		r.atLineStart = false
		r.pendingIcon = false
	}
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
	r.flushIconLine()
	r.endProse()
	r.running = append(r.running, runningTool{
		id:    call.ID,
		label: transcript.ToolLabel(call.Name, call.Args),
		verb:  transcript.ToolVerb(call.Name),
	})
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
	// Two things earn a permanent line: a failure (what went wrong) and an
	// edit (what changed, as a one-line +/- stat — never the whole diff).
	// Everything else is just a tick in the tally that collapses at turn end.
	if res.IsError {
		r.fails++
		r.persist("  " + r.bad.Render(transcript.ToolDone(label, res)))
		return
	}
	if diff := transcript.DiffOf(res); diff != "" {
		add, del := transcript.DiffStat(diff)
		r.persist("  " + r.ok.Render(fmt.Sprintf("%s (+%d −%d)", transcript.ToolDone(label, res), add, del)))
		return
	}
	r.repaint()
}

// setFooter swaps the cached status strip and, if a turn is on screen, repaints
// so the schedule glance stays current. At the prompt the next promptFollowUp
// paints it; while typing (suspended) the cache just updates silently.
func (r *liveRenderer) setFooter(lines []string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.footer = lines
	if r.turnOpen && !r.suspended {
		r.repaint()
	}
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

// promptFollowUp paints the status bar and the input marker, then freezes
// repaints until the user submits. The bar sits above a blank separator and the
// "› " prompt, so the order is always output → bar → prompt. The block is
// tracked as the erasable region only so an arriving outbox notice can lift it
// out of the way (see notice); on submit it is left in place permanently
// (commitPrompt), because a typed line may wrap and the renderer cannot know its
// height — so it never tries to erase across it.
func (r *liveRenderer) promptFollowUp() {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.erase()
	r.endProse()
	r.suspended = true
	lines := r.footerBlock()
	if len(lines) > 0 {
		lines = append(lines, "") // a blank line of breathing room above the prompt
	}
	var b strings.Builder
	for _, ln := range lines {
		b.WriteString(ln)
		b.WriteByte('\n')
	}
	b.WriteString(r.note.Render("› "))
	r.live = len(lines) + 1 // bar + separator above the prompt line (cursor inline)
	_, _ = io.WriteString(r.status, b.String())
}

// commitPrompt finalizes the bar+prompt block as the user submits. It does NOT
// erase: the typed line is already echoed in place, and a long prompt may have
// wrapped to several visual rows whose count the renderer can't know, so any
// cursor-up math would desync (the bug that stranded stale bars and double-
// printed prose). It just drops the block from the erasable count so the turn's
// output appends cleanly below it.
func (r *liveRenderer) commitPrompt(string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.suspended = false
	r.live = 0
	r.atLineStart = true
}

// notice prints outbox messages as permanent ambient lines. When the cursor is
// parked inline on the follow-up prompt, the region's last line holds the
// cursor, so erase one line shallower; the caller redraws footer+prompt beneath.
func (r *liveRenderer) notice(msgs []string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.suspended {
		r.clearLive(true) // cursor is inline on the prompt line
		r.suspended = false
	} else {
		r.erase()
	}
	r.endProse()
	for _, m := range msgs {
		_, _ = fmt.Fprintln(r.status, r.note.Render("◇ ")+sanitizeNotice(m))
	}
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
	r.pendingIcon = false
	r.atLineStart = true
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

// erase removes the painted live region when the cursor sits on a fresh line
// below it — the common case (after a trailing newline, or after the user
// pressed enter at the prompt).
func (r *liveRenderer) erase() { r.clearLive(false) }

// clearLive removes the painted live region and parks the cursor at its top,
// column 0, ready for persistent output. inline means the cursor is currently
// on the region's last line (the idle prompt the user is typing on), so the
// climb is one line shallower.
func (r *liveRenderer) clearLive(inline bool) {
	if r.live <= 0 {
		return
	}
	up := r.live
	if inline {
		up--
	}
	if up > 0 {
		_, _ = fmt.Fprintf(r.status, "\x1b[%dA", up)
	}
	_, _ = io.WriteString(r.status, "\r\x1b[J")
	r.live = 0
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

// repaint redraws the live region: the spinner activity lines, the running
// tally, then the pinned footer. Painted in a single write to avoid flicker.
// Suppressed while prose is mid-line or a prompt owns the cursor (in which case
// the cursor is inline and repaint's above-the-cursor erase would not apply).
func (r *liveRenderer) repaint() {
	if r.midText || r.suspended {
		return
	}
	lines := r.liveLines()
	var b strings.Builder
	if r.live > 0 {
		fmt.Fprintf(&b, "\x1b[%dA\r\x1b[J", r.live)
	}
	for _, ln := range lines {
		b.WriteString(ln)
		b.WriteByte('\n')
	}
	r.live = len(lines)
	_, _ = io.WriteString(r.status, b.String())
}

// liveLines is the repaintable region: one spinner line per kind of work in
// flight (grouped by verb so parallel reads read as "reading files (3)" rather
// than a churn of paths, and never their output), the running tally, then the
// pinned footer.
func (r *liveRenderer) liveLines() []string {
	var out []string
	spin := transcript.Spinner[r.frame%len(transcript.Spinner)]
	for i, g := range r.activity() {
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
	return append(out, r.footerBlock()...)
}

type verbGroup struct {
	verb string
	n    int
}

// activity groups the running tools by verb, preserving first-seen order so the
// list stays stable as parallel calls come and go.
func (r *liveRenderer) activity() []verbGroup {
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

// footerBlock styles the cached status strip with a dim rule above it. Empty
// when there is no footer, so an empty forest pins nothing.
func (r *liveRenderer) footerBlock() []string {
	if len(r.footer) == 0 {
		return nil
	}
	w := r.width
	if w > 80 {
		w = 80
	}
	out := []string{"  " + r.dim.Render(strings.Repeat("─", max(w-2, 1)))}
	for _, ln := range r.footer {
		out = append(out, r.dim.Render(clip(ln, r.width-1)))
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
