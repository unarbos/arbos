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

	startOpts := []engine.SessionOption{engine.WithPrincipal(core.PrincipalLocal)}
	if t.Origin != "" {
		startOpts = append(startOpts, engine.WithOrigin(t.Origin))
	}
	if t.Owner != "" || t.SpawnedBy != "" {
		startOpts = append(startOpts, engine.WithOwner(t.Owner, t.SpawnedBy))
	}
	conv, err := eng.StartSession(ctx, id, startOpts...)
	if err != nil {
		return res, fmt.Errorf("delegate: start child session: %w", err)
	}
	// A spawned session is one-shot: it runs this single turn and is never
	// resumed, so mark it ended once the work returns (success or error).
	// Otherwise it lingers active forever — the bug that left the activity
	// panel spinning on scheduler wakes whose turns finished long ago. A
	// detached context records the transition even when ctx is already
	// cancelled (parent interrupt / host shutdown); the engine's store handle
	// is open through the call.
	defer func() {
		if e := eng.EndSession(context.WithoutCancel(ctx), id); e != nil {
			// Best-effort: a failed end-write must not mask the turn's own
			// result, and the stale-run UI guard already dims orphaned rows.
			_ = e
		}
	}()
	conv.Send(core.PromptIntent{Text: t.Instruction})

	// engine.Drive forwards the child's envelopes to emit (the parent's relay
	// sink increments Depth, so nesting accumulates) and returns on the child's
	// own TurnComplete.
	tc, err := engine.Drive(ctx, conv, emit)
	if err != nil {
		return res, err
	}
	res.Text = tc.FinalResponse
	res.Usage = tc.Usage
	return res, nil
}
