// Live-streaming a turn to Telegram. Telegram has no token stream; the idiom
// is send-early-edit-often — the reply appears within a moment of the first
// tokens and grows in place. A streamer consumes one turn's envelopes
// (message deltas, tool boundaries, the terminal event) from whichever actor
// runs the turn: the bridge's own Drive loop, or a tap on the web tab's
// conversation. Each assistant segment (text between tool batches) becomes
// one Telegram message; at each message boundary the FULL delivered text is
// recorded — the mirror tail dedups against the persisted assistant
// message's whole content, so a long reply split across several Telegram
// messages must still be recorded as one — and the tail never sends it
// twice.

package messenger

import (
	"context"
	"encoding/json"
	"strings"
	"sync"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
)

// editInterval throttles in-place edits (Telegram sustains ~1 msg/s per
// chat; edits count).
const editInterval = 1500 * time.Millisecond

// streamCap finalizes a growing message before Telegram's 4096 limit and
// continues in a fresh one.
const streamCap = 3800

// tickerMaxLines bounds the activity ticker — the latest work, not a scroll.
const tickerMaxLines = 5

// streamSettle bounds the wait for the consumer to seal a finished turn's
// streamer before the mirror tail reads the log — the dedup record must land
// first, or the tail re-sends text the phone already watched stream.
const streamSettle = 5 * time.Second

// --- the turn stream ---------------------------------------------------------

// turnStream is the live pipeline from an actor's envelopes to one Telegram
// chat: a non-blocking push feeds a single consumer goroutine (emit and tap
// paths must never block on Telegram's network), which drives one streamer
// per turn. expected counts the depth-0 terminal events still owed — every
// prompt the bridge sends through this stream adds one — so a burst of
// injected follow-ups shares ONE tap and ONE consumer instead of stacking
// duplicates that would each stream the same turn to the phone.
type turnStream struct {
	conv   *engine.Conversation // tapped actor; nil when the bridge runs the turn itself
	ch     chan core.Envelope
	done   chan struct{}
	cancel context.CancelFunc
	// expected is guarded by Service.mu.
	expected int
}

// push delivers one envelope without blocking; a dropped delta costs
// smoothness, never correctness (the mirror tail still delivers the text).
func (ts *turnStream) push(env core.Envelope) {
	select {
	case ts.ch <- env:
	default:
	}
}

// finish waits briefly for the consumer to drain the terminal event and seal
// the streamer, then cancels whatever is left.
func (ts *turnStream) finish() {
	select {
	case <-ts.done:
	case <-time.After(streamSettle):
		ts.cancel()
		<-ts.done
	}
}

// stop tears the stream down now (its prompt never landed).
func (ts *turnStream) stop() {
	ts.cancel()
	<-ts.done
}

// startStream builds the live pipeline for turns on this conversation. With
// conv set it taps that actor (an open web tab's) and registers itself on
// the convo so follow-up prompts reuse it — two taps on one actor would
// stream the same turn twice; nil conv means the bridge's own ephemeral
// actor, whose emit closure feeds push directly and whose worker serializes
// turns, so no registration is needed.
//
// Known limit: envelopes carry no turn identity, so if the tapped actor is
// already mid-turn (the web user's own), that turn's terminal is
// indistinguishable from ours and consumes one expected count — the injected
// turn then arrives via the mirror tail instead of streaming live, and a
// question it asks is not presented (/steer and /stop still work). Exact
// binding needs turn ids on the engine's envelopes.
func (s *Service) startStream(ctx context.Context, b *botRunner, cv *convo, sessID core.SessionID, conv *engine.Conversation) *turnStream {
	streamCtx, cancel := context.WithCancel(ctx)
	ts := &turnStream{
		conv:     conv,
		ch:       make(chan core.Envelope, 256),
		done:     make(chan struct{}),
		cancel:   cancel,
		expected: 1,
	}
	var untap func()
	if conv != nil {
		untap = conv.Tap(func(env core.Envelope) {
			s.observeAsk(ctx, b, cv, conv, sessID, env)
			ts.push(env)
		})
		s.mu.Lock()
		cv.stream = ts
		s.mu.Unlock()
	}
	go s.consumeStream(streamCtx, b, cv, sessID, ts, untap)
	return ts
}

// consumeStream feeds envelopes to a streamer, one fresh streamer per turn,
// until every expected terminal has arrived. It is the single consumer for
// all turns the bridge has in flight on this conversation.
func (s *Service) consumeStream(ctx context.Context, b *botRunner, cv *convo, sessID core.SessionID, ts *turnStream, untap func()) {
	defer close(ts.done)
	defer ts.cancel()
	if untap != nil {
		defer untap()
	}
	defer func() {
		s.mu.Lock()
		if cv.stream == ts {
			cv.stream = nil
		}
		s.mu.Unlock()
	}()
	fresh := func() *streamer {
		return newStreamer(ctx, b.client, cv.c.ChatID,
			func(text string) { s.recordStreamed(cv, text) }, s.logf)
	}
	st := fresh()
	defer func() { st.close() }()
	timeout := time.NewTimer(turnTimeout + time.Minute)
	defer timeout.Stop()
	for {
		select {
		case env := <-ts.ch:
			st.feed(env)
			if env.SessionID != sessID || env.Depth != 0 {
				continue
			}
			switch env.Event.(type) {
			case core.TurnComplete, core.Interrupted, core.ErrorEvent:
				s.mu.Lock()
				ts.expected--
				rem := ts.expected
				if rem <= 0 && cv.stream == ts {
					cv.stream = nil
				}
				s.mu.Unlock()
				if rem <= 0 {
					return
				}
				// Another injected prompt is queued behind this turn: seal
				// this turn's streamer and stream the next one fresh.
				st.close()
				st = fresh()
				timeout.Reset(turnTimeout + time.Minute)
			}
		case <-timeout.C:
			return
		case <-ctx.Done():
			return
		}
	}
}

// injectTurn hands a prompt to the live actor another door holds, reusing
// the conversation's existing stream when one is already consuming that
// actor (a follow-up while a turn runs) and starting one otherwise. Returns
// false when the actor would not take the prompt.
func (s *Service) injectTurn(ctx context.Context, b *botRunner, cv *convo, sessID core.SessionID, conv *engine.Conversation, intent core.PromptIntent) bool {
	s.mu.Lock()
	ts := cv.stream
	if ts != nil && ts.conv == conv {
		ts.expected++
		s.mu.Unlock()
		if conv.TrySend(intent) {
			return true
		}
		s.mu.Lock()
		ts.expected--
		s.mu.Unlock()
		return false
	}
	// No stream, or a stale one whose actor died (a reopened tab is a new
	// actor): start fresh; startStream replaces the registration and the
	// old consumer exits on its own timeout without clobbering the new one.
	s.mu.Unlock()
	ts = s.startStream(ctx, b, cv, sessID, conv)
	if conv.TrySend(intent) {
		return true
	}
	ts.stop()
	return false
}

// streamer live-streams one turn's text to one Telegram chat, and renders
// tool activity (including sub-agent spawns) as one ephemeral ticker message
// that edits in place while a batch runs and deletes itself when prose
// resumes — the thread keeps only the conversation.
type streamer struct {
	ctx    context.Context
	client *tgClient
	chatID int64
	// delivered records each finalized segment so the tail skips it when
	// the same text lands in the log.
	delivered func(text string)
	logf      func(format string, args ...any)

	mu       sync.Mutex
	msgID    int64 // current Telegram message being grown; 0 = none yet
	buf      strings.Builder
	sent     string // what Telegram currently shows for msgID
	closed   bool
	lastEdit time.Time
	timer    *time.Timer
	// msgAll accumulates the WHOLE assistant message across streamCap
	// splits; the dedup record at each message boundary must equal the
	// persisted message's full content or the mirror tail re-sends it.
	msgAll strings.Builder

	// The activity ticker's own message and lines (latest tool calls /
	// sub-agent spawns), edited on the same throttle.
	tickerID    int64
	tickerLines []string
	tickerSent  string
	tickerDirty bool
}

func newStreamer(ctx context.Context, client *tgClient, chatID int64, delivered func(string), logf func(string, ...any)) *streamer {
	return &streamer{ctx: ctx, client: client, chatID: chatID, delivered: delivered, logf: logf}
}

// feed consumes one envelope from the turn. It runs on the flush timer's
// throttle for all network work, so the emit path stays fast.
func (st *streamer) feed(env core.Envelope) {
	if env.Depth > 0 {
		return // a sub-agent's chatter; its spawn is on the ticker already
	}
	switch e := env.Event.(type) {
	case core.MessageDelta:
		st.append(e.Text)
	case core.ToolStarted:
		// The segment before a tool batch is complete prose — finalize it
		// so the phone reads it while the tools run — then put the call on
		// the activity ticker.
		st.finalizeSegment()
		st.tick(callLine(e.Call))
	case core.TurnComplete, core.Interrupted, core.ErrorEvent:
		st.close()
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

func (st *streamer) append(text string) {
	if text == "" {
		return
	}
	st.mu.Lock()
	defer st.mu.Unlock()
	if st.closed {
		return
	}
	// Prose is resuming: the tool batch is over, its ticker has served.
	st.dropTickerLocked()
	st.buf.WriteString(text)
	st.msgAll.WriteString(text)
	if st.buf.Len() >= streamCap {
		// Continue in a fresh Telegram message; same assistant message, so
		// no boundary record yet.
		st.flushLocked(true)
		return
	}
	st.scheduleLocked()
}

// tick puts one line on the activity ticker (newest last, capped).
func (st *streamer) tick(line string) {
	st.mu.Lock()
	defer st.mu.Unlock()
	if st.closed {
		return
	}
	st.tickerLines = append(st.tickerLines, line)
	if len(st.tickerLines) > tickerMaxLines {
		st.tickerLines = st.tickerLines[len(st.tickerLines)-tickerMaxLines:]
	}
	st.tickerDirty = true
	st.scheduleLocked()
}

// flushTickerLocked pushes the ticker's lines to its message (send once,
// then edit in place).
func (st *streamer) flushTickerLocked() {
	if !st.tickerDirty {
		return
	}
	text := "⚙ " + strings.Join(st.tickerLines, "\n⚙ ")
	if text == st.tickerSent {
		st.tickerDirty = false
		return
	}
	st.lastEdit = time.Now()
	var err error
	if st.tickerID == 0 {
		st.tickerID, err = st.client.sendMessageID(st.ctx, st.chatID, text)
	} else {
		err = st.client.editMessageText(st.ctx, st.chatID, st.tickerID, text)
	}
	if err != nil {
		st.logf("messenger: ticker: %v", err)
		return
	}
	st.tickerSent = text
	st.tickerDirty = false
}

// dropTickerLocked removes the ticker message — activity is ephemeral; the
// thread keeps only the conversation.
func (st *streamer) dropTickerLocked() {
	if st.tickerID == 0 {
		return
	}
	if err := st.client.deleteMessage(st.ctx, st.chatID, st.tickerID); err != nil {
		st.logf("messenger: ticker delete: %v", err)
	}
	st.tickerID = 0
	st.tickerLines = nil
	st.tickerSent = ""
	st.tickerDirty = false
}

// scheduleLocked arranges a flush once editInterval has passed since the
// last network write — the first delta flushes almost immediately.
func (st *streamer) scheduleLocked() {
	if st.timer != nil {
		return
	}
	wait := editInterval - time.Since(st.lastEdit)
	if wait < 50*time.Millisecond {
		wait = 50 * time.Millisecond
	}
	st.timer = time.AfterFunc(wait, func() {
		st.mu.Lock()
		defer st.mu.Unlock()
		st.timer = nil
		if !st.closed {
			st.flushLocked(false)
			st.flushTickerLocked()
		}
	})
}

// flushLocked pushes the buffer to Telegram: first flush sends the message,
// later flushes edit it in place. final=true seals the current message and
// starts the next segment fresh. Returns false only when the network write
// failed — the boundary record must then be withheld so the mirror tail
// re-delivers the message rather than the phone losing its tail end.
func (st *streamer) flushLocked(final bool) bool {
	text := st.buf.String()
	if strings.TrimSpace(text) == "" || text == st.sent {
		if final {
			st.resetSegmentLocked()
		}
		return true
	}
	st.lastEdit = time.Now()
	var err error
	if st.msgID == 0 {
		st.msgID, err = st.client.sendMessageID(st.ctx, st.chatID, text)
	} else {
		err = st.client.editMessageText(st.ctx, st.chatID, st.msgID, text)
	}
	if err != nil {
		st.logf("messenger: stream: %v", err)
		return false
	}
	st.sent = text
	if final {
		st.resetSegmentLocked()
	}
	return true
}

// resetSegmentLocked clears the per-Telegram-message state for the next
// segment; recording happens per assistant message (recordMessageLocked),
// not per segment.
func (st *streamer) resetSegmentLocked() {
	st.msgID = 0
	st.buf.Reset()
	st.sent = ""
}

// recordMessageLocked hands the assistant message's full streamed text to
// delivered — the record consumeStreamed matches against the persisted
// message content — and clears for the next message. delivered=false (a
// failed final flush) discards instead: the phone is missing text, and the
// tail's "duplicate" is the correction.
func (st *streamer) recordMessageLocked(delivered bool) {
	text := st.msgAll.String()
	st.msgAll.Reset()
	if delivered && strings.TrimSpace(text) != "" && st.delivered != nil {
		st.delivered(text)
	}
}

// finalizeSegment seals the in-progress message (a tool batch is starting).
func (st *streamer) finalizeSegment() {
	st.mu.Lock()
	defer st.mu.Unlock()
	if st.closed {
		return
	}
	if st.timer != nil {
		st.timer.Stop()
		st.timer = nil
	}
	st.recordMessageLocked(st.flushLocked(true))
}

// close seals everything; the turn is over.
func (st *streamer) close() {
	st.mu.Lock()
	defer st.mu.Unlock()
	if st.closed {
		return
	}
	if st.timer != nil {
		st.timer.Stop()
		st.timer = nil
	}
	st.dropTickerLocked()
	st.recordMessageLocked(st.flushLocked(true))
	st.closed = true
}
