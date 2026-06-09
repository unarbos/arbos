package agent_test

import (
	"context"
	"encoding/json"
	"testing"

	"github.com/unarbos/arbos/internal/agent"
	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/fake"
	"github.com/unarbos/arbos/internal/tool"
)

// childEngineFactory builds a fake-backed child engine, honoring the grant's
// iteration budget.
func childEngineFactory(g agent.Grant) (*engine.Engine, error) {
	maxIter := g.Budget.MaxIterations
	if maxIter == 0 {
		maxIter = 5
	}
	return engine.New(fake.Provider{}, fake.Tools{}, fake.NewStore(), fake.NewClock(),
		engine.Config{Model: "fake", MaxIterations: maxIter}), nil
}

func newArbosAgent() *agent.ArbosAgent {
	n := 0
	return agent.NewArbosAgent(childEngineFactory, func() core.SessionID {
		n++
		return core.SessionID("child")
	})
}

func TestArbosAgentReturnsChildResult(t *testing.T) {
	ag := newArbosAgent()
	res, err := ag.Run(context.Background(), agent.Task{Instruction: "say hello"}, nil)
	if err != nil {
		t.Fatal(err)
	}
	if res.Text != "This is a deterministic fake response." {
		t.Fatalf("unexpected child result: %q", res.Text)
	}
	if res.ChildSession != "child" {
		t.Fatalf("child session id not reported: %q", res.ChildSession)
	}
}

// TestDelegateToolRoutesAndReturnsText proves the single delegate dispatch path:
// the tool resolves the backend via the router, runs the child, and returns its
// text.
func TestDelegateToolRoutesAndReturnsText(t *testing.T) {
	router := agent.NewRouter()
	router.Register("arbos", newArbosAgent())

	reg := tool.New()
	if err := agent.RegisterDelegate(reg, router, nil); err != nil {
		t.Fatal(err)
	}

	res := reg.Dispatch(context.Background(), core.ToolCall{
		ID:   "1",
		Name: "delegate",
		Args: json.RawMessage(`{"instruction":"do a thing","backend":"arbos"}`),
	})
	if res.IsError {
		t.Fatalf("delegate errored: %s", res.Content)
	}
	if res.Content != "This is a deterministic fake response." {
		t.Fatalf("delegate did not return the child result: %q", res.Content)
	}
}

// TestDelegateUnknownBackend surfaces a routing miss as a tool error, not a
// panic.
func TestDelegateUnknownBackend(t *testing.T) {
	router := agent.NewRouter()
	router.Register("arbos", newArbosAgent())
	reg := tool.New()
	_ = agent.RegisterDelegate(reg, router, nil)

	res := reg.Dispatch(context.Background(), core.ToolCall{
		ID: "1", Name: "delegate",
		Args: json.RawMessage(`{"instruction":"x","backend":"nonexistent"}`),
	})
	if !res.IsError {
		t.Fatal("expected an error for an unknown backend")
	}
}
