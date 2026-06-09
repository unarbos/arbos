package engine

import (
	"context"

	"github.com/unarbos/arbos/internal/core"
)

// RelayFunc forwards a delegated child's envelope into the parent session's
// outbound stream, so a tool that runs a child agent can stream the child's
// live events upward (depth-incremented for nested rendering) instead of only
// returning its final text. It is delivered to a tool through the dispatch
// context rather than the ToolRuntime port, so the kernel's tool interface
// stays unaware of delegation and tools that do not delegate ignore it — the
// same way obs.Correlation rides in context. Relayed events are presentation
// only: they are NOT persisted (the child owns its log; the parent's log keeps
// the delegate tool result), so replay is unaffected.
type RelayFunc func(core.Envelope)

type relayKey struct{}

// withRelay attaches the live-relay sink to ctx for the duration of a turn's
// tool dispatch.
func withRelay(ctx context.Context, fn RelayFunc) context.Context {
	return context.WithValue(ctx, relayKey{}, fn)
}

// Relay returns the live-relay sink the engine attached to the dispatch
// context, or nil when none is present. A delegating tool passes it as the emit
// sink to the child Agent; a nil return means "run the child without streaming"
// (the child still runs to completion and returns its result).
func Relay(ctx context.Context) RelayFunc {
	fn, _ := ctx.Value(relayKey{}).(RelayFunc)
	return fn
}
