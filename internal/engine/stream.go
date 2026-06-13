package engine

import (
	"context"
	"errors"
	"fmt"
	"math/rand"
	"strings"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/ports"
)

// streamOutcome tells runTurn how a (retried) provider call resolved, so the
// turn loop reacts without re-deriving it from a sentinel error.
type streamOutcome int

const (
	streamOK        streamOutcome = iota // sr is a complete response
	streamCancelled                      // ctx was cancelled mid-call (interrupt); the turn returns "not completed"
	streamFailed                         // every model and attempt exhausted; the terminal ErrorEvent is already emitted
)

// streamWithRetry issues one LLM call and rides through transient provider
// failures so a single hiccup does not end a long-running turn. For each model
// in [req.Model, cfg.FallbackModels...] it retries a retryable failure up to
// cfg.Retry.MaxAttempts times with exponential backoff (honoring a server
// Retry-After), then advances to the next model. A permanent failure (a 4xx
// that is not a rate limit) stops immediately — it would fail identically on
// every model. Cancellation always wins over a retry: an interrupt must not be
// papered over by another attempt. On success it returns the complete result;
// on exhaustion it emits the terminal provider ErrorEvent (the engine's
// long-standing behavior, now reached only after the chain is spent).
//
// Re-issuing is safe for durable state (nothing is appended until a complete
// response), but NOT for the live plane once deltas have streamed: the frontend
// appends, so a second attempt's text would concatenate onto the first's. So a
// failure that has already shown visible output (emitted) is terminal — only
// failures with nothing on screen yet (pre-stream errors, or a truncation
// before the first delta) are re-issued. The dominant outage modes (rate
// limits and 5xx) fail before any byte streams, so they still retry fully.
func (e *Engine) streamWithRetry(ctx context.Context, c *Conversation, req core.LLMRequest) (streamResult, streamOutcome) {
	models := append([]string{req.Model}, e.cfg.FallbackModels...)
	var lastErr error
	for mi := 0; mi < len(models); mi++ {
		req.Model = models[mi]
		for attempt := 0; attempt < e.cfg.Retry.MaxAttempts; attempt++ {
			if ctx.Err() != nil {
				return streamResult{}, streamCancelled
			}
			sr, emitted, err := e.streamOnce(ctx, c, req)
			// Cancellation is checked before the error: a cancelled stream
			// surfaces as an error, but it is an interrupt, not a provider
			// failure to retry.
			if ctx.Err() != nil {
				return streamResult{}, streamCancelled
			}
			if err == nil {
				return sr, streamOK
			}
			lastErr = err
			// A permanent provider error (bad request, dead key) will not
			// change across retries or fallback models — surface it now rather
			// than burning the turn's deadline.
			var pe *ports.ProviderError
			if errors.As(err, &pe) && !pe.Retryable() {
				return e.failStream(ctx, c, err), streamFailed
			}
			// A mid-stream failure that already streamed visible text cannot be
			// retried without the re-attempt's deltas concatenating onto what
			// the frontend already rendered. End the turn instead; the user
			// re-runs via the error card's Retry button if they want.
			if emitted {
				return e.failStream(ctx, c, err), streamFailed
			}
			// Out of attempts for this model: stop retrying and fall back.
			if attempt+1 >= e.cfg.Retry.MaxAttempts {
				break
			}
			e.observeProvider(ctx, c, fmt.Sprintf("retrying provider call (model %s): %v", req.Model, err))
			if !sleepBackoff(ctx, e.cfg.Retry, attempt, retryAfter(err)) {
				return streamResult{}, streamCancelled
			}
		}
		if mi+1 < len(models) {
			e.observeProvider(ctx, c, fmt.Sprintf("falling back from model %s to %s after: %v", req.Model, models[mi+1], lastErr))
		}
	}
	return e.failStream(ctx, c, lastErr), streamFailed
}

// streamOnce performs a single provider round-trip, folding a mid-stream
// truncation (sr.err, an aborted/truncated response) into the returned error
// so the retry loop treats it like a pre-stream failure. emitted reports
// whether any assistant text or reasoning reached the live plane this attempt —
// the signal streamWithRetry uses to refuse a re-issue that would double-render.
func (e *Engine) streamOnce(ctx context.Context, c *Conversation, req core.LLMRequest) (streamResult, bool, error) {
	chunks, err := e.provider.Stream(ctx, req)
	if err != nil {
		return streamResult{}, false, err
	}
	sr := e.streamResponse(ctx, c, chunks)
	// Content/reasoning deltas append on the frontend; tool-progress is a
	// replace-by-id composing card, so it does not corrupt on re-issue and is
	// not counted here.
	emitted := sr.content != "" || sr.reasoning != ""
	if sr.err != nil {
		return sr, emitted, sr.err
	}
	return sr, emitted, nil
}

// failStream emits the terminal provider ErrorEvent and returns an empty
// result — the one place the turn-ending error is shaped, reached after the
// retry/fallback chain is exhausted or on a non-retryable failure. Retryable
// mirrors the classification: a transient/exhausted failure stays retryable (a
// frontend can offer "try again", a scheduler wake re-fires next period), while
// a permanent one (bad request, dead key) is marked non-retryable so the UI
// does not dangle a futile retry.
func (e *Engine) failStream(ctx context.Context, c *Conversation, err error) streamResult {
	msg := "provider error"
	retryable := true
	if err != nil {
		msg = err.Error()
		var pe *ports.ProviderError
		if errors.As(err, &pe) {
			retryable = pe.Retryable()
		}
	}
	c.emit(ctx, core.ErrorEvent{Category: core.ErrProvider, Retryable: retryable, Err: msg})
	return streamResult{}
}

// observeProvider records a retry or fallback on the operational plane only
// (never the events channel), so the recovery is visible in telemetry without
// ending the turn or touching the sealed KernelEvent set.
func (e *Engine) observeProvider(ctx context.Context, c *Conversation, msg string) {
	if c.observer == nil {
		return
	}
	c.observer.ObserveEvent(ctx, core.ErrorEvent{Category: core.ErrProvider, Retryable: true, Err: msg})
}

// retryAfter extracts a provider's Retry-After hint when one rode on the error,
// so the backoff honors the server's pace instead of guessing.
func retryAfter(err error) time.Duration {
	var pe *ports.ProviderError
	if errors.As(err, &pe) {
		return pe.RetryAfter
	}
	return 0
}

// sleepBackoff waits before the next attempt, always with jitter so concurrent
// sessions riding the same outage do not resynchronize their retries (the
// thundering herd) — the risk is sharpest on a shared Retry-After, where every
// caller would otherwise wake on the same instant. With a server hint it waits
// AT LEAST the hint plus up to a quarter more (respecting the server's pace
// while spreading the wakeups); otherwise it uses an exponential backoff (base
// doubled per attempt, capped at MaxBackoff) with equal jitter. It is ctx-aware
// — a cancelled turn or a hit deadline ends the wait at once — and reports
// whether the wait completed (false = cancelled, so the caller stops retrying).
func sleepBackoff(ctx context.Context, p RetryPolicy, attempt int, hint time.Duration) bool {
	var d time.Duration
	if hint > 0 {
		// Respect the server's pace, then add up to 25% spread on top so
		// identical hints across sessions desync without ever retrying early.
		d = hint + time.Duration(rand.Int63n(int64(hint/4)+1))
	} else {
		d = p.BaseBackoff
		for i := 0; i < attempt && d < p.MaxBackoff; i++ {
			d *= 2
		}
		if d > p.MaxBackoff {
			d = p.MaxBackoff
		}
		// Equal jitter: half the window fixed, half random.
		if d > 0 {
			d = d/2 + time.Duration(rand.Int63n(int64(d/2)+1))
		}
	}
	if d <= 0 {
		return ctx.Err() == nil
	}
	t := time.NewTimer(d)
	defer t.Stop()
	select {
	case <-ctx.Done():
		return false
	case <-t.C:
		return true
	}
}

// streamResult is the accumulated outcome of one provider response. Keeping
// accumulation in one place means new chunk fields have a single tested home.
type streamResult struct {
	content      string
	reasoning    string
	toolCalls    []core.ToolCall
	citations    []core.Citation
	images       []core.ContentBlock
	usage        *core.Usage
	finishReason string
	err          error // mid-stream provider failure (LLMChunk.Err)
}

func (e *Engine) streamResponse(ctx context.Context, c *Conversation, chunks <-chan core.LLMChunk) streamResult {
	var content, reasoning strings.Builder
	var res streamResult
	for ch := range chunks {
		if ch.ContentDelta != "" {
			content.WriteString(ch.ContentDelta)
			if !c.emit(ctx, core.MessageDelta{Text: ch.ContentDelta}) {
				drainChunks(chunks)
				break
			}
		}
		if ch.ReasoningDelta != "" {
			reasoning.WriteString(ch.ReasoningDelta)
			if !c.emit(ctx, core.ReasoningDelta{Text: ch.ReasoningDelta}) {
				drainChunks(chunks)
				break
			}
		}
		if ch.ToolProgress != nil {
			// A tool call's arguments accumulating mid-stream: forward it as a
			// live progress event (never logged) so the frontend can show a
			// composing card in the gap before the finished call lands.
			if !c.emit(ctx, core.ToolProgress{
				CallID: ch.ToolProgress.ID,
				Name:   ch.ToolProgress.Name,
				Bytes:  ch.ToolProgress.Bytes,
			}) {
				drainChunks(chunks)
				break
			}
		}
		if len(ch.ToolCalls) > 0 {
			res.toolCalls = append(res.toolCalls, ch.ToolCalls...)
		}
		if len(ch.Citations) > 0 {
			res.citations = append(res.citations, ch.Citations...)
		}
		if len(ch.Images) > 0 {
			res.images = append(res.images, ch.Images...)
		}
		if ch.Usage != nil {
			res.usage = ch.Usage
		}
		if ch.FinishReason != "" {
			res.finishReason = ch.FinishReason
		}
		if ch.Err != nil {
			res.err = ch.Err
		}
	}
	res.content = content.String()
	res.reasoning = reasoning.String()
	return res
}

// drainChunks consumes any remaining chunks in the background so a cancelled
// turn doesn't leak a provider goroutine blocked on a send. Providers must honor
// ctx (they will stop and close), so this drains quickly; it's a safety net for
// the window between cancellation and the provider noticing.
func drainChunks(chunks <-chan core.LLMChunk) {
	go func() {
		for range chunks {
		}
	}()
}

func stopReasonFor(finishReason string) core.StopReason {
	switch finishReason {
	case "length", "max_tokens":
		return core.StopLengthLimit
	default:
		return core.StopAnswered
	}
}
