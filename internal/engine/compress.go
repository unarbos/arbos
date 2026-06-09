package engine

import (
	"context"
	"fmt"

	"github.com/unarbos/arbos/internal/core"
)

// maybeCompress folds the oldest turns when the policy says the conversation is
// over budget. It returns the re-projected messages and did=true on a
// successful compression, abort=true if the compression-marker append failed
// (turn-fatal). With no policy it is a no-op.
func (e *Engine) maybeCompress(ctx context.Context, c *Conversation, msgs []core.Message) (out []core.Message, abort, did bool) {
	if e.ctxPol == nil || !e.ctxPol.ShouldCompress(estimateTokens(msgs), msgs) {
		return msgs, false, false
	}
	events, err := e.store.Events(ctx, c.id)
	if err != nil {
		// Reading the log to compress failed; treat as history-fatal so we never
		// run on a conversation we couldn't inspect.
		c.emit(ctx, core.ErrorEvent{Category: core.ErrHistory, Err: "compress: load history: " + err.Error()})
		return msgs, true, false
	}
	lo, hi, ok := e.ctxPol.CompressibleRange(events)
	if !ok {
		return msgs, false, false
	}
	summary := e.summarizeRange(ctx, events, lo, hi)
	if !e.append(ctx, c, core.NewCompressionEvent(c.id, summary, lo, hi, e.clock.Now())) {
		return msgs, true, false
	}
	reloaded, err := e.store.Events(ctx, c.id)
	if err != nil {
		c.emit(ctx, core.ErrorEvent{Category: core.ErrHistory, Err: "compress: reload: " + err.Error()})
		return msgs, true, false
	}
	return core.Project(reloaded, e.cfg.SystemPrompt), false, true
}

// compactNow forces a compaction pass regardless of the budget trigger — the
// manual /compact path (CompactIntent), run by the actor only when idle so it
// stays the single writer. It is a no-op with no ContextPolicy or when nothing
// is compressible yet. A failed marker append is surfaced but not fatal (there
// is no live turn to abort).
func (e *Engine) compactNow(ctx context.Context, c *Conversation) {
	if e.ctxPol == nil {
		return
	}
	events, err := e.store.Events(ctx, c.id)
	if err != nil {
		c.emit(ctx, core.ErrorEvent{Category: core.ErrHistory, Err: "compact: load history: " + err.Error()})
		return
	}
	lo, hi, ok := e.ctxPol.CompressibleRange(events)
	if !ok {
		return
	}
	summary := e.summarizeRange(ctx, events, lo, hi)
	_ = e.append(ctx, c, core.NewCompressionEvent(c.id, summary, lo, hi, e.clock.Now()))
}

// summarizeRange produces the summary text for the [lo,hi] span. It prefers the
// injected Summarizer; without one it emits a deterministic marker so
// compression still works (and tests stay hermetic).
func (e *Engine) summarizeRange(ctx context.Context, events []core.Event, lo, hi int64) string {
	var folded []core.Message
	for _, ev := range events {
		if ev.Seq < lo || ev.Seq > hi {
			continue
		}
		if m, ok := core.ProjectEvent(ev); ok {
			folded = append(folded, m)
		}
	}
	if e.summ != nil {
		if s, err := e.summ.Summarize(ctx, folded); err == nil && s != "" {
			return s
		}
		// fall through to the marker on summarizer failure — losing the summary
		// text is far better than failing the turn.
	}
	return fmt.Sprintf("[earlier conversation compressed: %d messages]", len(folded))
}

// estimateTokens sums the canonical per-message proxy (core.EstimateTokens)
// across the conversation to decide when to compress.
func estimateTokens(msgs []core.Message) int {
	total := 0
	for _, m := range msgs {
		total += core.EstimateTokens(m)
	}
	return total
}
