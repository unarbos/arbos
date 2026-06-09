package fake

import (
	"context"
	"encoding/json"

	"github.com/unarbos/arbos/internal/core"
)

// Tools is a minimal ToolRuntime exposing the ls tool the scripted Provider
// requests. It is enough to prove the engine's call -> dispatch -> result path.
type Tools struct{}

func (Tools) Schemas() []core.ToolSchema {
	return []core.ToolSchema{{
		Name:        "ls",
		Description: "List files in a directory.",
		Parameters:  json.RawMessage(`{"type":"object","properties":{"path":{"type":"string"}}}`),
		ReadOnly:    true,
	}}
}

func (Tools) Dispatch(_ context.Context, call core.ToolCall) core.ToolResult {
	if call.Name != "ls" {
		return core.ToolResult{CallID: call.ID, IsError: true, Content: "unknown tool: " + call.Name}
	}
	return core.ToolResult{CallID: call.ID, Content: ".\n"}
}
