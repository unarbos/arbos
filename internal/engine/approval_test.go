package engine_test

import (
	"context"
	"encoding/json"
	"strings"
	"testing"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/fake"
	"github.com/unarbos/arbos/internal/ports"
)

// approvePolicy requires approval for the named tools.
type approvePolicy struct{ gate map[string]bool }

func (p approvePolicy) Requires(call core.ToolCall) (string, bool) {
	if p.gate[call.Name] {
		return "tool " + call.Name + " needs confirmation", true
	}
	return "", false
}

// writeTool is a single non-read-only tool that reports "wrote".
type writeTool struct{}

func (writeTool) Schemas() []core.ToolSchema {
	return []core.ToolSchema{{Name: "write_file", Parameters: json.RawMessage(`{"type":"object"}`), ReadOnly: false}}
}

func (writeTool) Dispatch(ctx context.Context, call core.ToolCall) core.ToolResult {
	return core.ToolResult{CallID: call.ID, Content: "wrote"}
}

// singleToolProvider asks for one named tool, then answers once a result lands.
type singleToolProvider struct{ tool string }

func (singleToolProvider) Name() string                     { return "single" }
func (singleToolProvider) Capabilities() ports.Capabilities { return ports.Capabilities{Tools: true} }

func (p singleToolProvider) Stream(ctx context.Context, req core.LLMRequest) (<-chan core.LLMChunk, error) {
	out := make(chan core.LLMChunk, 4)
	go func() {
		defer close(out)
		for _, m := range req.Messages {
			if m.Role == core.RoleTool {
				out <- core.LLMChunk{ContentDelta: "all done"}
				out <- core.LLMChunk{Done: true, Usage: &core.Usage{TotalTokens: 1}}
				return
			}
		}
		out <- core.LLMChunk{ToolCalls: []core.ToolCall{{ID: "t1", Name: p.tool, Args: json.RawMessage(`{}`)}}}
		out <- core.LLMChunk{Done: true}
	}()
	return out, nil
}

func approvalEngine(t *testing.T) *engine.Engine {
	t.Helper()
	return engine.New(
		singleToolProvider{tool: "write_file"},
		writeTool{},
		fake.NewStore(),
		fake.NewClock(),
		engine.Config{Model: "single", MaxIterations: 10},
		engine.WithApproval(approvePolicy{gate: map[string]bool{"write_file": true}}),
	)
}

// drainUntilApproval reads events until an ApprovalRequest and returns its id.
func drainUntilApproval(t *testing.T, conv *engine.Conversation) core.RequestID {
	t.Helper()
	timeout := time.After(3 * time.Second)
	for {
		select {
		case env := <-conv.Events():
			if ar, ok := env.Event.(core.ApprovalRequest); ok {
				return ar.RequestID
			}
		case <-timeout:
			t.Fatal("never received an ApprovalRequest")
		}
	}
}

func TestApprovalApprovedProceeds(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	conv, err := approvalEngine(t).StartSession(ctx, "s-approve")
	if err != nil {
		t.Fatal(err)
	}
	conv.Send(core.PromptIntent{Text: "go"})
	id := drainUntilApproval(t, conv)
	conv.Send(core.ApprovalResponseIntent{RequestID: id, Approved: true})

	tr := drain(t, conv)
	if tr.finalText != "all done" {
		t.Fatalf("approved turn should complete: %q (%v)", tr.finalText, tr.kinds)
	}
	if len(tr.toolResults) != 1 || tr.toolResults[0] != "wrote" {
		t.Fatalf("tool should have run on approval: %+v", tr.toolResults)
	}
}

func TestApprovalDeniedSkipsTool(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	conv, err := approvalEngine(t).StartSession(ctx, "s-deny")
	if err != nil {
		t.Fatal(err)
	}
	conv.Send(core.PromptIntent{Text: "go"})
	id := drainUntilApproval(t, conv)
	conv.Send(core.ApprovalResponseIntent{RequestID: id, Approved: false})

	tr := drain(t, conv)
	if len(tr.toolResults) != 1 || !strings.Contains(tr.toolResults[0], "denied") {
		t.Fatalf("denied call must yield a denial result: %+v", tr.toolResults)
	}
}

// TestApprovalFastReplyNoRace guards the rendezvous-ordering fix: replying the
// instant the ApprovalRequest is seen must never drop the response. Run many
// times under -race to surface any residual ordering window.
func TestApprovalFastReplyNoRace(t *testing.T) {
	for i := 0; i < 50; i++ {
		ctx, cancel := context.WithCancel(context.Background())
		conv, err := approvalEngine(t).StartSession(ctx, core.SessionID("s-fast"))
		if err != nil {
			t.Fatal(err)
		}
		conv.Send(core.PromptIntent{Text: "go"})
		id := drainUntilApproval(t, conv)
		conv.Send(core.ApprovalResponseIntent{RequestID: id, Approved: true})
		tr := drain(t, conv)
		if tr.kinds[len(tr.kinds)-1] != "turn_complete" {
			cancel()
			t.Fatalf("iteration %d: fast reply was dropped (got %v)", i, tr.kinds)
		}
		cancel()
	}
}

func TestInterruptWhileAwaitingApproval(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	conv, err := approvalEngine(t).StartSession(ctx, "s-int")
	if err != nil {
		t.Fatal(err)
	}
	conv.Send(core.PromptIntent{Text: "go"})
	drainUntilApproval(t, conv)
	conv.Send(core.InterruptIntent{})

	tr := drain(t, conv)
	if tr.kinds[len(tr.kinds)-1] != "interrupted" {
		t.Fatalf("interrupt while awaiting approval should interrupt the turn: %v", tr.kinds)
	}
}
