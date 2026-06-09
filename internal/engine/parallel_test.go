package engine_test

import (
	"context"
	"encoding/json"
	"sync/atomic"
	"testing"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/fake"
	"github.com/unarbos/arbos/internal/ports"
)

// multiToolProvider asks for three tool calls in its first response, then
// answers plainly once results come back. It drives the parallel-dispatch path.
type multiToolProvider struct {
	tools []string
}

func (multiToolProvider) Name() string                     { return "multitool" }
func (multiToolProvider) Capabilities() ports.Capabilities { return ports.Capabilities{Tools: true} }

func (p multiToolProvider) Stream(ctx context.Context, req core.LLMRequest) (<-chan core.LLMChunk, error) {
	out := make(chan core.LLMChunk, 8)
	go func() {
		defer close(out)
		hasResult := false
		for _, m := range req.Messages {
			if m.Role == core.RoleTool {
				hasResult = true
			}
		}
		if !hasResult {
			var calls []core.ToolCall
			for i, name := range p.tools {
				calls = append(calls, core.ToolCall{
					ID:   string(rune('a' + i)),
					Name: name,
					Args: json.RawMessage(`{}`),
				})
			}
			out <- core.LLMChunk{ToolCalls: calls}
			out <- core.LLMChunk{Done: true}
			return
		}
		out <- core.LLMChunk{ContentDelta: "done"}
		out <- core.LLMChunk{Done: true, Usage: &core.Usage{TotalTokens: 1}}
	}()
	return out, nil
}

// concurrencyProbe tools record peak concurrent in-flight dispatches so a test
// can prove read-only calls actually overlap.
type concurrencyProbe struct {
	readOnly bool
	cur      atomic.Int32
	peak     atomic.Int32
}

func (cp *concurrencyProbe) Schemas() []core.ToolSchema {
	return []core.ToolSchema{
		{Name: "r1", ReadOnly: cp.readOnly, Parameters: json.RawMessage(`{"type":"object"}`)},
		{Name: "r2", ReadOnly: cp.readOnly, Parameters: json.RawMessage(`{"type":"object"}`)},
		{Name: "r3", ReadOnly: cp.readOnly, Parameters: json.RawMessage(`{"type":"object"}`)},
	}
}

func (cp *concurrencyProbe) Dispatch(ctx context.Context, call core.ToolCall) core.ToolResult {
	n := cp.cur.Add(1)
	for {
		peak := cp.peak.Load()
		if n <= peak || cp.peak.CompareAndSwap(peak, n) {
			break
		}
	}
	time.Sleep(20 * time.Millisecond) // widen the overlap window
	cp.cur.Add(-1)
	return core.ToolResult{CallID: call.ID, Content: call.Name + "-ok"}
}

func runWithTools(t *testing.T, ro bool) (*concurrencyProbe, transcript) {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	probe := &concurrencyProbe{readOnly: ro}
	eng := engine.New(
		multiToolProvider{tools: []string{"r1", "r2", "r3"}},
		probe,
		fake.NewStore(),
		fake.NewClock(),
		engine.Config{Model: "multitool", MaxIterations: 10},
	)
	conv, err := eng.StartSession(ctx, "s-parallel")
	if err != nil {
		t.Fatal(err)
	}
	conv.Send(core.PromptIntent{Text: "go"})
	return probe, drain(t, conv)
}

// TestReadOnlyToolsRunConcurrently proves the parallel path: three read-only
// calls overlap (peak concurrency > 1) and all results land.
func TestReadOnlyToolsRunConcurrently(t *testing.T) {
	probe, tr := runWithTools(t, true)
	if tr.kinds[len(tr.kinds)-1] != "turn_complete" {
		t.Fatalf("expected completion, got %v", tr.kinds)
	}
	if len(tr.toolResults) != 3 {
		t.Fatalf("want 3 tool results, got %d", len(tr.toolResults))
	}
	if peak := probe.peak.Load(); peak < 2 {
		t.Fatalf("read-only tools did not run concurrently (peak=%d)", peak)
	}
}

// TestWriteToolsRunSequentially proves a non-read-only batch never overlaps.
func TestWriteToolsRunSequentially(t *testing.T) {
	probe, tr := runWithTools(t, false)
	if tr.kinds[len(tr.kinds)-1] != "turn_complete" {
		t.Fatalf("expected completion, got %v", tr.kinds)
	}
	if len(tr.toolResults) != 3 {
		t.Fatalf("want 3 tool results, got %d", len(tr.toolResults))
	}
	if peak := probe.peak.Load(); peak != 1 {
		t.Fatalf("write tools must be sequential (peak=%d)", peak)
	}
}
