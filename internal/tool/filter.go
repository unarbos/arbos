package tool

import (
	"context"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/ports"
)

// Filter returns a ToolRuntime that exposes and dispatches only the named tools,
// the mechanism behind a delegation Grant's tool allowlist. An empty allowlist
// means "no restriction" and returns the runtime unchanged, so a grant that does
// not narrow tools keeps the full set. A call to a non-granted tool comes back
// as an error result (never a panic), so the model sees why it was refused.
func Filter(inner ports.ToolRuntime, names []string) ports.ToolRuntime {
	if len(names) == 0 {
		return inner
	}
	allow := make(map[string]bool, len(names))
	for _, n := range names {
		allow[n] = true
	}
	return filtered{inner: inner, allow: allow}
}

type filtered struct {
	inner ports.ToolRuntime
	allow map[string]bool
}

func (f filtered) Schemas() []core.ToolSchema {
	var out []core.ToolSchema
	for _, s := range f.inner.Schemas() {
		if f.allow[s.Name] {
			out = append(out, s)
		}
	}
	return out
}

func (f filtered) Dispatch(ctx context.Context, call core.ToolCall) core.ToolResult {
	if !f.allow[call.Name] {
		return core.ToolResult{CallID: call.ID, IsError: true, Content: "tool not granted: " + call.Name}
	}
	return f.inner.Dispatch(ctx, call)
}

var _ ports.ConflictAnalyzer = filtered{}

// Access delegates a granted call's footprint to the inner runtime; a
// non-granted call is unbounded (it will be refused at Dispatch, so it must not
// be reordered around real work).
func (f filtered) Access(call core.ToolCall) core.AccessSet {
	if !f.allow[call.Name] {
		return core.AccessSet{Unknown: true}
	}
	return ports.AccessOf(f.inner, call)
}
