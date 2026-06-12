// The phone's half of the ask tool (suspend-and-await questions, ADR-0018).
// When a turn on a bridged session suspends on a QuestionRequest, the bridge
// renders the form as one plain-text Telegram message — numbered options, one
// question per block — and remembers the request on the conversation. The
// owner's next text reply IS the answer: option numbers select, anything else
// travels as their own words, /skip dismisses the form. The pending ask is
// cleared the moment the turn demonstrably moves past it (deltas resume, the
// turn ends) so a later reply is conversation again, never a stale answer.
package messenger

import (
	"context"
	"encoding/base64"
	"fmt"
	"strconv"
	"strings"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
)

// pendingAsk is one QuestionRequest awaiting the phone's reply. conv is the
// actor that owns the suspended turn — the bridge's own ephemeral actor, or
// an open web tab's the bridge tapped — and is where the answer must go. at
// bounds its life: an ask older than the turn timeout is stale (its turn is
// long over) and must not capture a later message as its answer.
type pendingAsk struct {
	id        core.RequestID
	questions []core.Question
	conv      *engine.Conversation
	at        time.Time
}

func (s *Service) setAsk(cv *convo, conv *engine.Conversation, q core.QuestionRequest) {
	s.mu.Lock()
	cv.ask = &pendingAsk{id: q.RequestID, questions: q.Questions, conv: conv, at: time.Now()}
	s.mu.Unlock()
}

func (s *Service) clearAsk(cv *convo) {
	s.mu.Lock()
	cv.ask = nil
	s.mu.Unlock()
}

// takeAsk pops the pending ask atomically, so one reply can never answer the
// same request twice (a duplicate response is a protocol error to the
// engine). A stale ask — older than the turn timeout, so its turn is over —
// is dropped rather than returned: answering it would feed a dead request.
func (s *Service) takeAsk(cv *convo) *pendingAsk {
	s.mu.Lock()
	defer s.mu.Unlock()
	a := cv.ask
	cv.ask = nil
	if a != nil && time.Since(a.at) > turnTimeout {
		return nil
	}
	return a
}

// observeAsk watches one turn envelope for the ask lifecycle. On a
// QuestionRequest it records the pending ask and presents the form on the
// phone (async — this runs on emit paths that must never block on Telegram's
// network). Any sign the turn moved past the question — prose resuming, or
// the turn ending — drops the pending ask: it was answered from another door,
// steered away, or is moot. ToolStarted/ToolFinished deliberately do NOT
// clear it: a parallel tool wave can finish siblings while the ask is still
// suspended.
func (s *Service) observeAsk(ctx context.Context, b *botRunner, cv *convo, conv *engine.Conversation, sessID core.SessionID, env core.Envelope) {
	if env.SessionID != sessID || env.Depth != 0 {
		return
	}
	switch e := env.Event.(type) {
	case core.QuestionRequest:
		s.setAsk(cv, conv, e)
		go s.presentAsk(ctx, b, cv.c.ChatID, e)
	case core.MessageDelta, core.ReasoningDelta, core.TurnComplete, core.Interrupted, core.ErrorEvent:
		s.clearAsk(cv)
	}
}

func (s *Service) presentAsk(ctx context.Context, b *botRunner, chatID int64, q core.QuestionRequest) {
	// A single question whose options each fit a 64-byte callback_data gets
	// tappable buttons (one tap answers). Multi-question and multi-select forms
	// keep the plain-text path: those need accumulate-then-confirm state a
	// single tap can't express, so the typed reply stays their answer.
	if kb, ok := askKeyboard(q); ok {
		if _, err := b.client.sendMessageMarkup(ctx, chatID, renderAsk(q), kb); err == nil {
			return
		}
		// Fall through to plain text if the keyboard send was rejected.
	}
	if err := b.client.sendMessage(ctx, chatID, renderAsk(q)); err != nil {
		s.logf("messenger: @%s ask: %v", b.cfg.Username, err)
	}
}

// askKeyboard builds an inline keyboard for a single-select, single-question
// ask: one button per option, the callback_data encoding the request and the
// chosen option. ok is false when the form isn't a fit (multiple questions,
// multi-select, no options, or any callback_data over Telegram's 64-byte cap)
// — the caller then uses the plain-text form.
func askKeyboard(q core.QuestionRequest) (inlineKeyboard, bool) {
	if len(q.Questions) != 1 {
		return inlineKeyboard{}, false
	}
	question := q.Questions[0]
	if question.AllowMultiple || len(question.Options) == 0 {
		return inlineKeyboard{}, false
	}
	rows := make([][]tgInlineButton, 0, len(question.Options))
	for oi, opt := range question.Options {
		data := askCallbackData(q.RequestID, oi)
		if len(data) > 64 {
			return inlineKeyboard{}, false
		}
		rows = append(rows, []tgInlineButton{{Text: opt.Label, CallbackData: data}})
	}
	return inlineKeyboard{InlineKeyboard: rows}, true
}

// askCallbackData encodes a tapped option as "a|<requestID>|<optionIdx>". The
// request id is carried verbatim so the tap is matched to the exact pending
// ask, never a stale one; onCallback re-validates it against cv.ask.
func askCallbackData(reqID core.RequestID, optionIdx int) string {
	return fmt.Sprintf("a|%s|%d", reqID, optionIdx)
}

// parseAskCallback decodes askCallbackData back into the request id and option
// index. ok is false for any string that isn't our encoding.
func parseAskCallback(data string) (core.RequestID, int, bool) {
	parts := strings.SplitN(data, "|", 3)
	if len(parts) != 3 || parts[0] != "a" {
		return "", 0, false
	}
	oi, err := strconv.Atoi(parts[2])
	if err != nil || oi < 0 {
		return "", 0, false
	}
	return core.RequestID(parts[1]), oi, true
}

// onCallback handles an inline-keyboard tap: resolve the chat's conversation,
// confirm the tap matches the live pending ask, and send the answer intent
// down the same path a typed reply takes. The button's encoded option index
// is validated against the live question's options (boundary discipline: the
// callback is external input), so a stale or malformed tap is acknowledged
// and dropped rather than answering a dead request.
func (s *Service) onCallback(ctx context.Context, b *botRunner, cb tgCallbackQuery) {
	if cb.Message == nil || cb.From == nil {
		return
	}
	chatID := cb.Message.Chat.ID
	if b.cfg.BoundUser != 0 && b.cfg.BoundUser != cb.From.ID {
		b.client.answerCallbackQuery(ctx, cb.ID, "This arbos is private.")
		return
	}
	reqID, optIdx, ok := parseAskCallback(cb.Data)
	if !ok {
		b.client.answerCallbackQuery(ctx, cb.ID, "")
		return
	}

	key := fmt.Sprintf("%d:%d", b.cfg.ID, chatID)
	s.mu.Lock()
	cv := s.convos[key]
	s.mu.Unlock()
	if cv == nil {
		b.client.answerCallbackQuery(ctx, cb.ID, "That question has expired.")
		return
	}

	// Match against the live ask before consuming it: a tap on an old form
	// (its turn long over, or already answered) must not fire a dead intent.
	s.mu.Lock()
	live := cv.ask != nil && cv.ask.id == reqID
	s.mu.Unlock()
	if !live {
		b.client.answerCallbackQuery(ctx, cb.ID, "That question has expired.")
		return
	}
	ask := s.takeAsk(cv)
	if ask == nil || ask.id != reqID || len(ask.questions) != 1 {
		b.client.answerCallbackQuery(ctx, cb.ID, "That question has expired.")
		return
	}
	question := ask.questions[0]
	if optIdx >= len(question.Options) {
		b.client.answerCallbackQuery(ctx, cb.ID, "")
		return
	}
	opt := question.Options[optIdx]

	resp := core.QuestionResponseIntent{
		RequestID: ask.id,
		Answers:   []core.QuestionAnswer{{QuestionID: question.ID, SelectedIDs: []string{opt.ID}}},
	}
	ask.conv.Send(resp)
	b.client.answerCallbackQuery(ctx, cb.ID, "Selected: "+opt.Label)
	_ = b.client.clearReplyMarkup(ctx, chatID, cb.Message.MessageID)
}

// renderAsk formats a question form as one plain-text message the phone can
// answer by number.
func renderAsk(q core.QuestionRequest) string {
	var sb strings.Builder
	title := strings.TrimSpace(q.Title)
	if title == "" {
		title = "arbos needs your input"
	}
	sb.WriteString("❓ " + title + "\n")
	multi := len(q.Questions) > 1
	for qi, question := range q.Questions {
		sb.WriteString("\n")
		if multi {
			fmt.Fprintf(&sb, "Q%d: %s\n", qi+1, question.Prompt)
		} else {
			sb.WriteString(question.Prompt + "\n")
		}
		for oi, opt := range question.Options {
			fmt.Fprintf(&sb, "  %d. %s\n", oi+1, opt.Label)
		}
		if question.AllowMultiple {
			sb.WriteString("  (several allowed, e.g. \"1,3\")\n")
		}
	}
	sb.WriteString("\n")
	if multi {
		sb.WriteString("Answer each question on its own line — an option number or your own words.")
	} else {
		sb.WriteString("Reply with an option number or your own words.")
	}
	sb.WriteString("\n/skip to skip · /steer <text> to redirect · /stop to stop the turn")
	return sb.String()
}

// parseAskReply turns the phone's reply into the answer intent. A single
// question reads the whole reply (numbers select, anything else is the
// user's own words); multiple questions read one line each, extra lines ride
// along as free-text details.
func parseAskReply(ask *pendingAsk, text string) core.QuestionResponseIntent {
	resp := core.QuestionResponseIntent{RequestID: ask.id}
	text = strings.TrimSpace(text)
	if strings.EqualFold(text, "/skip") {
		resp.Skipped = true
		return resp
	}
	if len(ask.questions) == 1 {
		resp.Answers = []core.QuestionAnswer{answerOne(ask.questions[0], text)}
		return resp
	}
	lines := nonEmptyLines(text)
	for i, q := range ask.questions {
		line := ""
		if i < len(lines) {
			line = lines[i]
		}
		resp.Answers = append(resp.Answers, answerOne(q, line))
	}
	if len(lines) > len(ask.questions) {
		resp.Details = strings.Join(lines[len(ask.questions):], "\n")
	}
	return resp
}

// answerOne reads one reply fragment against one question: option numbers
// become selections, anything else is the user's own words.
func answerOne(q core.Question, text string) core.QuestionAnswer {
	ans := core.QuestionAnswer{QuestionID: q.ID}
	if text == "" {
		return ans
	}
	if sel := selections(text, len(q.Options)); len(sel) > 0 {
		for _, n := range sel {
			ans.SelectedIDs = append(ans.SelectedIDs, q.Options[n-1].ID)
		}
		return ans
	}
	ans.OtherText = text
	return ans
}

// selections parses "2", "1,3", "1 3" into 1-based option indices. Any token
// that is not a valid index makes the whole reply free text instead — "42"
// against three options is words, not a pick.
func selections(text string, max int) []int {
	fields := strings.FieldsFunc(text, func(r rune) bool { return r == ',' || r == ' ' || r == '\t' })
	if len(fields) == 0 {
		return nil
	}
	out := make([]int, 0, len(fields))
	for _, f := range fields {
		n, err := strconv.Atoi(f)
		if err != nil || n < 1 || n > max {
			return nil
		}
		out = append(out, n)
	}
	return out
}

func nonEmptyLines(text string) []string {
	var out []string
	for _, line := range strings.Split(text, "\n") {
		if line = strings.TrimSpace(line); line != "" {
			out = append(out, line)
		}
	}
	return out
}

// askReplyText resolves the text of a reply to a pending ask: typed text as
// is; a voice memo transcribed, so a spoken answer unblocks the turn too.
func (s *Service) askReplyText(ctx context.Context, b *botRunner, msg inbound) string {
	if t := strings.TrimSpace(msg.text); t != "" {
		return t
	}
	if msg.voice == nil || s.cfg.Transcribe == nil {
		return ""
	}
	audio, err := b.client.fetchFile(ctx, msg.voice.FileID)
	if err != nil {
		s.logf("messenger: ask voice: %v", err)
		return ""
	}
	spoken, err := s.cfg.Transcribe(ctx, base64.StdEncoding.EncodeToString(audio), "ogg")
	if err != nil {
		s.logf("messenger: ask transcribe: %v", err)
		return ""
	}
	return strings.TrimSpace(spoken)
}
