package tool

import (
	"context"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/ports"
)

// Multi composes several ToolRuntimes into one: it concatenates their advertised
// schemas (in order) and routes a Dispatch to the runtime that owns the called
// tool name. It is how the coding toolset and an MCP server's tools present as a
// single runtime to the engine. A single runtime is returned unchanged.
//
// The name->runtime route is built once here (the composed runtimes are fixed at
// assembly), so Dispatch is an O(1) lookup rather than re-scanning every
// runtime's schemas on each call.
func Multi(runtimes ...ports.ToolRuntime) ports.ToolRuntime {
	switch len(runtimes) {
	case 0:
		return multi{}
	case 1:
		return runtimes[0]
	}
	route := make(map[string]ports.ToolRuntime)
	for _, rt := range runtimes {
		for _, s := range rt.Schemas() {
			if _, claimed := route[s.Name]; !claimed {
				route[s.Name] = rt // first runtime owning a name wins (matches Schemas order)
			}
		}
	}
	return multi{rts: runtimes, route: route}
}

type multi struct {
	rts   []ports.ToolRuntime
	route map[string]ports.ToolRuntime
}

func (m multi) Schemas() []core.ToolSchema {
	var out []core.ToolSchema
	for _, rt := range m.rts {
		out = append(out, rt.Schemas()...)
	}
	return out
}

func (m multi) Dispatch(ctx context.Context, call core.ToolCall) core.ToolResult {
	if rt, ok := m.route[call.Name]; ok {
		return rt.Dispatch(ctx, call)
	}
	return core.ToolResult{CallID: call.ID, IsError: true, Content: "unknown tool: " + call.Name}
}

var _ ports.ConflictAnalyzer = multi{}

// Access routes to the owning runtime's footprint so finer scheduling survives
// composition (the primary pi runtime is a Multi of the coding registry plus
// delegation/MCP tools). The ReadOnly fallback for non-analyzing runtimes lives
// in ports.AccessOf, the one home for that rule.
func (m multi) Access(call core.ToolCall) core.AccessSet {
	rt, ok := m.route[call.Name]
	if !ok {
		return core.AccessSet{Unknown: true}
	}
	return ports.AccessOf(rt, call)
}
