package obs

import (
	"context"
	"log/slog"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/ports"
)

// SlogObserver is the structured-logging implementation of ports.Observer: it
// turns each emitted KernelEvent into a correlated log line. Trace/turn/session
// ids come from the context (Correlation), never from the event, keeping the
// domain types free of operational ids (ADR-0017). Pair its logger with a
// RedactingHandler so a stray secret in a message can't leak.
type SlogObserver struct {
	log *slog.Logger
}

var _ ports.Observer = (*SlogObserver)(nil)

func NewSlogObserver(log *slog.Logger) *SlogObserver {
	if log == nil {
		log = slog.Default()
	}
	return &SlogObserver{log: log}
}

func (o *SlogObserver) ObserveEvent(ctx context.Context, ev core.KernelEvent) {
	attrs := []slog.Attr{slog.String("event", string(ev.Kind()))}
	if cor, ok := From(ctx); ok {
		attrs = append(attrs,
			slog.String("session_id", cor.SessionID),
			slog.Int64("turn_id", cor.TurnID),
			slog.String("trace_id", cor.TraceID),
		)
	}

	switch e := ev.(type) {
	case core.ToolStarted:
		o.log.LogAttrs(ctx, slog.LevelInfo, "tool started", append(attrs, slog.String("tool", e.Call.Name))...)
	case core.ToolFinished:
		o.log.LogAttrs(ctx, slog.LevelInfo, "tool finished", append(attrs, slog.Bool("is_error", e.Result.IsError))...)
	case core.TurnComplete:
		o.log.LogAttrs(ctx, slog.LevelInfo, "turn complete",
			append(attrs, slog.String("stop_reason", string(e.StopReason)), slog.Int("total_tokens", e.Usage.TotalTokens))...)
	case core.ErrorEvent:
		o.log.LogAttrs(ctx, slog.LevelError, "turn error",
			append(attrs, slog.String("category", string(e.Category)), slog.Bool("retryable", e.Retryable), slog.String("error", e.Err))...)
	case core.Interrupted:
		o.log.LogAttrs(ctx, slog.LevelInfo, "turn interrupted", attrs...)
	case core.ApprovalRequest:
		o.log.LogAttrs(ctx, slog.LevelInfo, "approval requested", append(attrs, slog.String("tool", e.Call.Name))...)
	case core.QuestionRequest:
		o.log.LogAttrs(ctx, slog.LevelInfo, "questions asked", append(attrs, slog.Int("questions", len(e.Questions)))...)
	default:
		// Streaming deltas and other high-frequency events log at debug so they
		// don't drown the operational stream by default.
		o.log.LogAttrs(ctx, slog.LevelDebug, "kernel event", attrs...)
	}
}
