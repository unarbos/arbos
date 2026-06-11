package engine

import (
	"context"

	"github.com/unarbos/arbos/internal/core"
)

// AskFunc suspends the turn to put structured questions to the user and blocks
// until they answer (or the turn is cancelled — ok is then false). It is
// delivered to a tool through the dispatch context, exactly like RelayFunc:
// the kernel's tool interface stays unaware of questioning, and tools that do
// not ask ignore it. The exchange is the ApprovalRequest suspend-and-await
// (ADR-0018) with a richer payload; nothing about it is persisted — the
// durable record is the asking tool's result.
type AskFunc func(title string, questions []core.Question) (core.QuestionResponseIntent, bool)

type askKey struct{}

// withAsker attaches the suspend-and-ask capability to ctx for the duration of
// a turn's tool dispatch.
func withAsker(ctx context.Context, fn AskFunc) context.Context {
	return context.WithValue(ctx, askKey{}, fn)
}

// Asker returns the suspend-and-ask capability the engine attached to the
// dispatch context, or nil when none is present (a host without an
// interactive frontend).
func Asker(ctx context.Context) AskFunc {
	fn, _ := ctx.Value(askKey{}).(AskFunc)
	return fn
}
