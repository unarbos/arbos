package agent

import (
	"context"
	"fmt"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
)

// EngineFactory builds the child engine for a Grant. It is where a Grant's
// budget, tool allowlist, and environment (Grant.Env.Path) become a concrete
// engine (the caller owns the provider, store, and tool wiring; the factory
// applies the grant). Returning a distinct engine per delegation keeps children
// isolated. It returns an error so an invalid grant (e.g. a bad Env.Path) fails
// loudly instead of silently running in the wrong place.
type EngineFactory func(g Grant) (*engine.Engine, error)

// ArbosAgent runs a Task as a nested arbos kernel session — recursion is free
// because the child is just another session actor. It is the local case of the
// ArbosAgent row in the taxonomy; the remote case reaches a remote kernel over
// the Phase-7 control seam with the same Agent interface.
type ArbosAgent struct {
	newEngine EngineFactory
	newID     func() core.SessionID
}

var _ Agent = (*ArbosAgent)(nil)

func NewArbosAgent(newEngine EngineFactory, newID func() core.SessionID) *ArbosAgent {
	return &ArbosAgent{newEngine: newEngine, newID: newID}
}

// Run spawns a child session, sends the instruction as a prompt, relays the
// child's events through emit, and returns the final response as the Result. A
// child ErrorEvent becomes a Go error; a parent interrupt propagates via ctx and
// surfaces as context.Canceled.
func (a *ArbosAgent) Run(ctx context.Context, t Task, emit func(core.Envelope)) (Result, error) {
	eng, err := a.newEngine(t.Grant)
	if err != nil {
		return Result{}, fmt.Errorf("delegate: build child engine: %w", err)
	}
	// Scope the child session to this call: cancelling on return tears down the
	// child's actor goroutine so N sequential delegations don't leak N actors.
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	id := a.newID()
	res := Result{ChildSession: string(id)}

	conv, err := eng.StartSession(ctx, id)
	if err != nil {
		return res, fmt.Errorf("delegate: start child session: %w", err)
	}
	conv.Send(core.PromptIntent{Text: t.Instruction})

	for env := range conv.Events() {
		// Forward the child's envelope unchanged (its own SessionID, Depth 0).
		// The parent's relay sink increments Depth, so nesting accumulates
		// correctly without this layer knowing its own depth.
		if emit != nil {
			emit(env)
		}
		switch e := env.Event.(type) {
		case core.TurnComplete:
			res.Text = e.FinalResponse
			res.Usage = e.Usage
			return res, nil
		case core.ErrorEvent:
			return res, fmt.Errorf("delegate: child error (%s): %s", e.Category, e.Err)
		case core.Interrupted:
			return res, context.Canceled
		}
	}
	// Channel closed without a terminal event (e.g. parent ctx cancelled).
	return res, ctx.Err()
}
