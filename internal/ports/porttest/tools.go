package porttest

import (
	"context"
	"encoding/json"
	"testing"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/ports"
)

// RunToolRuntimeContract asserts the contract every ToolRuntime must satisfy:
// well-formed schemas, no panics, error-as-data, and CallID round-tripping.
func RunToolRuntimeContract(t *testing.T, newRuntime func() ports.ToolRuntime) {
	t.Helper()
	ctx := context.Background()

	t.Run("schemas_well_formed", func(t *testing.T) {
		rt := newRuntime()
		seen := map[string]bool{}
		for _, s := range rt.Schemas() {
			if s.Name == "" {
				t.Fatal("tool schema has an empty Name")
			}
			if seen[s.Name] {
				t.Fatalf("duplicate tool name %q", s.Name)
			}
			seen[s.Name] = true
			if len(s.Parameters) == 0 {
				t.Fatalf("tool %q has an empty Parameters schema", s.Name)
			}
			if !json.Valid(s.Parameters) {
				t.Fatalf("tool %q Parameters is not valid JSON", s.Name)
			}
		}
	})

	t.Run("unknown_tool_is_error_not_panic", func(t *testing.T) {
		rt := newRuntime()
		res := rt.Dispatch(ctx, core.ToolCall{ID: "x1", Name: "definitely_not_a_real_tool", Args: json.RawMessage(`{}`)})
		if !res.IsError {
			t.Fatal("dispatching an unknown tool must return IsError=true")
		}
		if res.CallID != "x1" {
			t.Fatalf("result must preserve CallID, got %q", res.CallID)
		}
	})

	t.Run("dispatch_preserves_callid", func(t *testing.T) {
		rt := newRuntime()
		schemas := rt.Schemas()
		if len(schemas) == 0 {
			t.Skip("runtime exposes no tools to dispatch")
		}
		res := rt.Dispatch(ctx, core.ToolCall{ID: "c42", Name: schemas[0].Name, Args: json.RawMessage(`{}`)})
		if res.CallID != "c42" {
			t.Fatalf("result must preserve CallID, got %q", res.CallID)
		}
	})
}
