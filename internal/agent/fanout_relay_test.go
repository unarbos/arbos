package agent_test

import (
	"context"
	"encoding/json"
	"fmt"
	"sync/atomic"
	"testing"
	"time"

	"github.com/unarbos/arbos/internal/agent"
	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/fake"
	"github.com/unarbos/arbos/internal/ports"
	"github.com/unarbos/arbos/internal/tool"
)

// childProvider emits one recognizable message delta per turn, recording the
// peak number of concurrently in-flight streams so a test can prove that two
// delegations actually overlapped (fan-out), not merely that both completed.
type childProvider struct {
	cur  *atomic.Int32
	peak *atomic.Int32
}

func (childProvider) Name() string                     { return "child" }
func (childProvider) Capabilities() ports.Capabilities { return ports.Capabilities{} }

func (p childProvider) Stream(ctx context.Context, _ core.LLMRequest) (<-chan core.LLMChunk, error) {
	n := p.cur.Add(1)
	for {
		peak := p.peak.Load()
		if n <= peak || p.peak.CompareAndSwap(peak, n) {
			break
		}
	}
	out := make(chan core.LLMChunk, 4)
	go func() {
		defer close(out)
		time.Sleep(30 * time.Millisecond) // widen the overlap window
		p.cur.Add(-1)
		out <- core.LLMChunk{ContentDelta: "child-hello"}
		out <- core.LLMChunk{Done: true, Usage: &core.Usage{TotalTokens: 1}}
	}()
	return out, nil
}

// delegatingParent asks for two delegations in its first response, then answers
// plainly once their results return. Two delegate calls drive the parallel
// fan-out + live-relay path end to end.
type delegatingParent struct{}

func (delegatingParent) Name() string                     { return "parent" }
func (delegatingParent) Capabilities() ports.Capabilities { return ports.Capabilities{Tools: true} }

func (delegatingParent) Stream(_ context.Context, req core.LLMRequest) (<-chan core.LLMChunk, error) {
	out := make(chan core.LLMChunk, 8)
	go func() {
		defer close(out)
		for _, m := range req.Messages {
			if m.Role == core.RoleTool {
				out <- core.LLMChunk{ContentDelta: "done"}
				out <- core.LLMChunk{Done: true}
				return
			}
		}
		out <- core.LLMChunk{ToolCalls: []core.ToolCall{
			{ID: "a", Name: "delegate", Args: json.RawMessage(`{"instruction":"explore a","backend":"child","tools":["read"]}`)},
			{ID: "b", Name: "delegate", Args: json.RawMessage(`{"instruction":"explore b","backend":"child","tools":["read"]}`)},
		}}
		out <- core.LLMChunk{Done: true}
	}()
	return out, nil
}

// TestDelegateFanOutAndLiveRelay proves the whole chain: a parent emits two
// delegate calls, the engine schedules them concurrently (every delegation
// advertises an empty footprint, so siblings never conflict), each child streams
// a message delta, and those deltas surface in the PARENT's event stream tagged
// Depth==1. It checks the real artifacts — the relayed envelopes and the peak
// concurrency the children observed — not a self-report.
func TestDelegateFanOutAndLiveRelay(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	var cur, peak atomic.Int32
	var ids atomic.Int32
	childAgent := agent.NewArbosAgent(
		func(agent.Grant) (*engine.Engine, error) {
			return engine.New(childProvider{cur: &cur, peak: &peak}, fake.Tools{}, fake.NewStore(), fake.NewClock(),
				engine.Config{Model: "child", MaxIterations: 3}), nil
		},
		func() core.SessionID { return core.SessionID(fmt.Sprintf("child-%d", ids.Add(1))) },
	)

	router := agent.NewRouter()
	router.Register("child", childAgent)

	reg := tool.New()
	// Every delegation has an empty footprint, so the two delegate calls do not
	// conflict and the engine fans them out.
	if err := agent.RegisterDelegate(reg, router); err != nil {
		t.Fatal(err)
	}

	parent := engine.New(delegatingParent{}, reg, fake.NewStore(), fake.NewClock(),
		engine.Config{Model: "parent", MaxIterations: 5})
	conv, err := parent.StartSession(ctx, "parent")
	if err != nil {
		t.Fatal(err)
	}
	conv.Send(core.PromptIntent{Text: "go"})

	relayedDeltas := 0
	sawParentComplete := false
	for env := range conv.Events() {
		if env.Depth == 1 {
			if d, ok := env.Event.(core.MessageDelta); ok && d.Text == "child-hello" {
				relayedDeltas++
			}
		}
		if env.Depth == 0 {
			if _, ok := env.Event.(core.TurnComplete); ok {
				sawParentComplete = true
				break
			}
		}
	}

	if !sawParentComplete {
		t.Fatal("parent turn never completed (no Depth-0 TurnComplete)")
	}
	// Live relay: each child's delta reached the parent stream at Depth 1.
	if relayedDeltas != 2 {
		t.Fatalf("want 2 relayed child deltas at Depth 1, got %d", relayedDeltas)
	}
	// Fan-out: the two children were in flight at the same time.
	if p := peak.Load(); p < 2 {
		t.Fatalf("delegations did not run concurrently (peak in-flight=%d)", p)
	}
}
