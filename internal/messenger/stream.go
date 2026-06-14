// Rendering one bridged session's live envelope stream to one Telegram chat.
// Telegram has no token stream, so the idiom is send-early-edit-often: an
// assistant message appears within a moment of the first tokens and grows in
// place. The presenter is the phone's view of the shared log — the SAME stream
// every other door (a web tab, a shared link) sees — so a turn started from
// any door mirrors here identically. There is no separate "mirror tail" and no
// content-based dedup: the one stream is the single source, and a message this
// Telegram door sent itself is recognized by its origin tag and skipped (the
// phone already shows it).
package messenger

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"strings"
	"sync"
	"time"

	"github.com/unarbos/arbos/internal/core"
)

// editInterval throttles in-place edits (Telegram sustains ~1 msg/s per chat;
// edits count).
const editInterval = 1500 * time.Millisecond

// streamCap finalizes a growing message before Telegram's 4096 limit and
// continues in a fresh one.
const streamCap = 3800

// tickerMaxLines bounds the activity ticker — the latest work, not a scroll.
const tickerMaxLines = 5

// presenter renders one session's live stream to one Telegram chat. Assistant
// prose streams in place; tool activity (including sub-agent spawns) shows as
// one ephemeral ticker that edits while a batch runs and deletes itself when
// prose resumes; a user message from ANOTHER door crosses over prefixed as the
// owner's own words. It is long-lived: it seals each turn's message at the
// terminal event and stays ready for the next, until the door detaches.
type presenter struct {
	ctx    context.Context
	client *tgClient
	chatID int64
	// origin is this Telegram door's tag. A Queued echo carrying it is this
	// phone's own message coming back through the shared actor — skip it.
	origin string
	logf   func(format string, args ...any)

	mu       sync.Mutex
	msgID    int64 // current Telegram message being grown; 0 = none yet
	buf      strings.Builder
	sent     string // what Telegram currently shows for msgID
	closed   bool
	lastEdit time.Time
	timer    *time.Timer

	// The activity ticker's own message and lines (latest tool calls /
	// sub-agent spawns), edited on the same throttle.
	tickerID    int64
	tickerLines []string
	tickerSent  string
	tickerDirty bool
}

func newPresenter(ctx context.Context, client *tgClient, chatID int64, origin string, logf func(string, ...any)) *presenter {
	return &presenter{ctx: ctx, client: client, chatID: chatID, origin: origin, logf: logf}
}

// feed consumes one envelope from the session stream. Network work runs on the
// flush timer's throttle (and on segment boundaries), so the stream consumer
// stays responsive.
func (p *presenter) feed(env core.Envelope) {
	if env.Depth > 0 {
		return // a sub-agent's chatter; its spawn is already on the ticker
	}
	switch e := env.Event.(type) {
	case core.Queued:
		// A prompt crossing over from another door. Our own echo (this phone's
		// message, round-tripped through the shared actor) is skipped.
		if e.Origin == p.origin {
			return
		}
		p.finalizeSegment()
		p.sendForeign(e.Text, e.Parts)
	case core.MessageDelta:
		p.append(e.Text)
	case core.ToolStarted:
		// The prose before a tool batch is complete — finalize it so the phone
		// reads it while the tools run — then put the call on the ticker.
		p.finalizeSegment()
		p.tick(callLine(e.Call))
	case core.Interrupted:
		p.sealTurn()
		p.note("(interrupted)")
	case core.TurnComplete, core.ErrorEvent:
		p.sealTurn()
	}
}

// sendForeign renders a user message that arrived through another door: the
// text prefixed as the owner's own words crossing over, plus any image parts.
func (p *presenter) sendForeign(text string, parts []core.ContentBlock) {
	if strings.TrimSpace(text) != "" {
		if err := p.client.sendMarkdown(p.ctx, p.chatID, "[from web] "+text); err != nil {
			p.logf("messenger: cross-door: %v", err)
		}
	}
	p.sendImages(parts)
}

// sendImages delivers a message's image parts as photos (web-composer
// attachments, provider-generated images).
func (p *presenter) sendImages(parts []core.ContentBlock) {
	for _, part := range parts {
		if part.Type != core.BlockImage || part.Image == nil {
			continue
		}
		img, err := base64.StdEncoding.DecodeString(part.Image.Data)
		if err != nil || len(img) == 0 || len(img) > mediaCap {
			continue
		}
		if err := p.client.sendPhoto(p.ctx, p.chatID, img); err != nil {
			p.logf("messenger: photo: %v", err)
		}
	}
}

// note sends one standalone line (a marker like "(interrupted)").
func (p *presenter) note(text string) {
	if _, err := p.client.sendMessageID(p.ctx, p.chatID, text, parseNone); err != nil {
		p.logf("messenger: note: %v", err)
	}
}

// callLine renders one tool call as a ticker line: the name plus the one
// argument a human recognizes it by.
func callLine(call core.ToolCall) string {
	line := call.Name
	if arg := callArg(call.Args); arg != "" {
		line += " · " + arg
	}
	if len(line) > 64 {
		line = line[:64] + "…"
	}
	return line
}

// callArg picks the most recognizable argument off a tool call: the path,
// command, instruction — whatever names the work.
func callArg(raw []byte) string {
	if len(raw) == 0 {
		return ""
	}
	var args map[string]any
	if json.Unmarshal(raw, &args) != nil {
		return ""
	}
	for _, key := range []string{"command", "path", "instruction", "url", "query", "pattern", "goal", "message"} {
		if v, ok := args[key].(string); ok && strings.TrimSpace(v) != "" {
			return strings.Join(strings.Fields(v), " ")
		}
	}
	return ""
}

func (p *presenter) append(text string) {
	if text == "" {
		return
	}
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.closed {
		return
	}
	// Prose is resuming: the tool batch is over, its ticker has served.
	p.dropTickerLocked()
	p.buf.WriteString(text)
	if p.buf.Len() >= streamCap {
		// Continue in a fresh Telegram message (same assistant message split
		// across Telegram's length cap).
		p.flushLocked(true)
		return
	}
	p.scheduleLocked()
}

// tick puts one line on the activity ticker (newest last, capped).
func (p *presenter) tick(line string) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.closed {
		return
	}
	p.tickerLines = append(p.tickerLines, line)
	if len(p.tickerLines) > tickerMaxLines {
		p.tickerLines = p.tickerLines[len(p.tickerLines)-tickerMaxLines:]
	}
	p.tickerDirty = true
	p.scheduleLocked()
}

// flushTickerLocked pushes the ticker's lines to its message (send once, then
// edit in place).
func (p *presenter) flushTickerLocked() {
	if !p.tickerDirty {
		return
	}
	text := "⚙ " + strings.Join(p.tickerLines, "\n⚙ ")
	if text == p.tickerSent {
		p.tickerDirty = false
		return
	}
	p.lastEdit = time.Now()
	var err error
	// The ticker is plain (a "⚙ name · arg" activity line, ephemeral) — no
	// formatting to parse, so no parse mode.
	if p.tickerID == 0 {
		p.tickerID, err = p.client.sendMessageID(p.ctx, p.chatID, text, parseNone)
	} else {
		err = p.client.editMessageText(p.ctx, p.chatID, p.tickerID, text, parseNone)
	}
	if err != nil {
		p.logf("messenger: ticker: %v", err)
		return
	}
	p.tickerSent = text
	p.tickerDirty = false
}

// dropTickerLocked removes the ticker message — activity is ephemeral; the
// thread keeps only the conversation.
func (p *presenter) dropTickerLocked() {
	if p.tickerID == 0 {
		return
	}
	if err := p.client.deleteMessage(p.ctx, p.chatID, p.tickerID); err != nil {
		p.logf("messenger: ticker delete: %v", err)
	}
	p.tickerID = 0
	p.tickerLines = nil
	p.tickerSent = ""
	p.tickerDirty = false
}

// scheduleLocked arranges a flush once editInterval has passed since the last
// network write — the first delta flushes almost immediately.
func (p *presenter) scheduleLocked() {
	if p.timer != nil {
		return
	}
	wait := editInterval - time.Since(p.lastEdit)
	if wait < 50*time.Millisecond {
		wait = 50 * time.Millisecond
	}
	p.timer = time.AfterFunc(wait, func() {
		p.mu.Lock()
		defer p.mu.Unlock()
		p.timer = nil
		if !p.closed {
			p.flushLocked(false)
			p.flushTickerLocked()
		}
	})
}

// flushLocked pushes the buffer to Telegram: the first flush sends the message,
// later flushes edit it in place. final=true seals the current message and
// starts the next segment fresh.
func (p *presenter) flushLocked(final bool) {
	// The buffer holds the model's raw Markdown; Telegram shows it as rich text
	// via HTML. mdToHTML balances every snapshot, so even a partial buffer
	// (an open **, a half-typed code fence) renders valid entities. p.sent
	// caches the rendered HTML so an unchanged snapshot skips the edit.
	text := mdToHTML(p.buf.String())
	if strings.TrimSpace(text) == "" || text == p.sent {
		if final {
			p.resetSegmentLocked()
		}
		return
	}
	p.lastEdit = time.Now()
	var err error
	if p.msgID == 0 {
		p.msgID, err = p.client.sendMessageID(p.ctx, p.chatID, text, parseHTML)
	} else {
		err = p.client.editMessageText(p.ctx, p.chatID, p.msgID, text, parseHTML)
	}
	if err != nil {
		p.logf("messenger: stream: %v", err)
		return
	}
	p.sent = text
	if final {
		p.resetSegmentLocked()
	}
}

// resetSegmentLocked clears the per-Telegram-message state so the next segment
// starts a fresh message.
func (p *presenter) resetSegmentLocked() {
	p.msgID = 0
	p.buf.Reset()
	p.sent = ""
}

// finalizeSegment seals the in-progress message (a tool batch is starting, or a
// foreign message is about to interleave).
func (p *presenter) finalizeSegment() {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.closed {
		return
	}
	if p.timer != nil {
		p.timer.Stop()
		p.timer = nil
	}
	p.flushLocked(true)
}

// sealTurn flushes the last prose and clears the ticker at a turn's terminal
// event, leaving the presenter ready for the next turn (it is NOT closed —
// the door stays attached across turns).
func (p *presenter) sealTurn() {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.closed {
		return
	}
	if p.timer != nil {
		p.timer.Stop()
		p.timer = nil
	}
	p.dropTickerLocked()
	p.flushLocked(true)
}

// close seals everything; the door is detaching.
func (p *presenter) close() {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.closed {
		return
	}
	if p.timer != nil {
		p.timer.Stop()
		p.timer = nil
	}
	p.dropTickerLocked()
	p.flushLocked(true)
	p.closed = true
}
