package engine

import (
	"context"
	"encoding/json"
)

// ToolDetailsSink receives a running call's presentation Details (a bash
// command's journaled job id) the moment the tool learns it, bound to the call
// it belongs to by withToolDetails. It is delivered to a tool through the
// dispatch context rather than the ToolRuntime port — exactly like RelayFunc
// and AskFunc — so the kernel's tool interface stays unaware of it and tools
// that have nothing to emit ignore it. What it carries (ToolDetails) is
// presentation only and never persisted, so replay is unaffected.
type ToolDetailsSink func(details json.RawMessage)

type toolDetailsSinkKey struct{}

// withToolDetailsSink attaches the details side-channel to ctx for the duration
// of one tool call's dispatch.
func withToolDetailsSink(ctx context.Context, sink ToolDetailsSink) context.Context {
	return context.WithValue(ctx, toolDetailsSinkKey{}, sink)
}

// EmitToolDetails forwards a running tool's Details to the sink the engine
// bound to the dispatch context. Fire-and-forget: a tool calls it
// unconditionally, and with no sink installed (a direct registry dispatch with
// no engine, as in tests) it does nothing.
func EmitToolDetails(ctx context.Context, details json.RawMessage) {
	if sink, ok := ctx.Value(toolDetailsSinkKey{}).(ToolDetailsSink); ok && sink != nil {
		sink(details)
	}
}
