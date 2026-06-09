package engine

import (
	"context"
	"fmt"
	"sync"

	"github.com/unarbos/arbos/internal/core"
)

// maxParallelTools caps concurrent read-only tool dispatch so a model that
// requests a hundred fetches doesn't open a hundred sockets at once.
const maxParallelTools = 8

// dispatchResult is the outcome of running a turn's tool batch. stop reports
// whether the turn loop should end; completedNormally then distinguishes a
// terminal outcome (persist failure -> true) from an interrupt (-> false), so
// processPrompt can tell a real interrupt from a completing turn (H1).
type dispatchResult struct {
	msgs              []core.Message
	stop              bool
	completedNormally bool
	// terminate is set when the batch completed normally and every result asked
	// to terminate (pi's all-or-nothing terminate hint). The turn loop ends the
	// turn gracefully when it is set. Interrupt/persist-failure paths leave it
	// false; they stop via stop/completedNormally instead.
	terminate bool
}

// interrupted / persistFailed build the two terminal dispatchResults so the
// (msgs, stop, completedNormally) triple is constructed in exactly one place
// each — completedNormally distinguishes a real interrupt (false) from a
// persist failure (true, a terminal outcome) for the H1 race in processPrompt.
func (dr dispatchResult) interrupted() dispatchResult {
	return dispatchResult{msgs: dr.msgs, stop: true, completedNormally: false}
}

func (dr dispatchResult) persistFailed() dispatchResult {
	return dispatchResult{msgs: dr.msgs, stop: true, completedNormally: true}
}

// canParallelize reports whether a tool batch is safe to run concurrently: more
// than one call, every call a known read-only tool, and none requiring human
// approval (an approval pause is inherently sequential and interactive).
func (e *Engine) canParallelize(calls []core.ToolCall, readOnly map[string]bool) bool {
	if len(calls) < 2 {
		return false
	}
	for _, call := range calls {
		if !readOnly[call.Name] {
			return false
		}
		if _, required := e.requiresApproval(call); required {
			return false
		}
	}
	return true
}

// requiresApproval consults the optional policy. A nil policy approves
// everything (pi's default full-privileges behavior).
func (e *Engine) requiresApproval(call core.ToolCall) (reason string, required bool) {
	if e.approval == nil {
		return "", false
	}
	return e.approval.Requires(call)
}

// recordResult is the single place a tool result enters the log: it persists the
// result event (on a cancellation-free context so a result computed just as the
// turn is interrupted still lands — an interrupt must never orphan a tool_call),
// extends the in-turn projection, and emits ToolFinished. Returns false
// (turn-fatal) on a persist failure. Both dispatch paths route through it, so
// they can differ in HOW results are produced, not in HOW they are recorded.
func (e *Engine) recordResult(ctx context.Context, c *Conversation, dr *dispatchResult, res core.ToolResult) bool {
	pctx := context.WithoutCancel(ctx)
	toolEv := core.NewToolResultEvent(c.id, res, e.clock.Now())
	if err := e.store.AppendEvent(pctx, e.stamp(pctx, toolEv)); err != nil {
		c.emit(ctx, core.ErrorEvent{Category: core.ErrPersist, Err: "persist tool_result: " + err.Error()})
		return false
	}
	if m, ok := core.ProjectEvent(*toolEv); ok {
		dr.msgs = append(dr.msgs, m)
	}
	c.emit(ctx, core.ToolFinished{Result: res})
	return true
}

// gateApproval pauses for human confirmation. ok is false when the turn was
// cancelled while waiting.
func (e *Engine) gateApproval(ctx context.Context, c *Conversation, call core.ToolCall, reason string) (approved, ok bool) {
	id := nextRequestID()
	intent, ok := e.requestAndAwait(ctx, c, id, core.ApprovalRequest{RequestID: id, Call: call, Reason: reason})
	if !ok {
		return false, false
	}
	ar, _ := intent.(core.ApprovalResponseIntent)
	return ar.Approved, true
}

// requestAndAwait emits a mid-turn request (ApprovalRequest/ClarifyRequest) and
// blocks until the actor routes the matching response, or the turn is cancelled.
//
// Ordering is load-bearing: the waiter is registered on the unbuffered awaits
// rendezvous BEFORE the request is emitted. The actor is a single goroutine, so
// once this send is received the actor records the waiter in `pending` before it
// can ever process the response intent on c.intents. Emitting the request first
// (the previous order) let a fast or duplicate response land before its waiter
// existed — it was dropped as "unknown request" and the turn blocked forever.
// ok is false when the turn was cancelled while registering, emitting, or
// waiting.
func (e *Engine) requestAndAwait(ctx context.Context, c *Conversation, id core.RequestID, req core.KernelEvent) (core.Intent, bool) {
	resp := make(chan core.Intent, 1)
	select {
	case c.awaits <- awaitReg{id: id, resp: resp}:
	case <-ctx.Done():
		return nil, false
	}
	if !c.emit(ctx, req) {
		return nil, false
	}
	select {
	case intent := <-resp:
		return intent, true
	case <-ctx.Done():
		return nil, false
	}
}

// dispatchSequential runs tools one at a time, the conservative path for writes
// and mixed batches. On interrupt it backfills synthetic results for the
// not-yet-run calls so the assistant's tool_calls always have matching results.
func (e *Engine) dispatchSequential(ctx context.Context, c *Conversation, calls []core.ToolCall) dispatchResult {
	var dr dispatchResult
	allTerm := len(calls) > 0
	for i, call := range calls {
		if ctx.Err() != nil {
			e.backfillInterruptedTools(ctx, c, calls[i:])
			return dr.interrupted()
		}

		// Approval gate: a denial still produces a tool result so the assistant's
		// tool_call keeps its matching result and the log stays valid.
		if reason, required := e.requiresApproval(call); required {
			approved, ok := e.gateApproval(ctx, c, call, reason)
			if !ok {
				e.backfillInterruptedTools(ctx, c, calls[i:])
				return dr.interrupted()
			}
			if !approved {
				if !e.recordResult(ctx, c, &dr, core.ToolResult{CallID: call.ID, IsError: true, Content: "tool call denied by user"}) {
					// Include i: its result failed to persist, so it too is a
					// potential orphan (backfill is best-effort).
					e.backfillInterruptedTools(ctx, c, calls[i:])
					return dr.persistFailed()
				}
				allTerm = false
				continue
			}
		}

		if !c.emit(ctx, core.ToolStarted{Call: call}) {
			e.backfillInterruptedTools(ctx, c, calls[i:])
			return dr.interrupted()
		}
		res := e.tools.Dispatch(ctx, call)
		if !e.recordResult(ctx, c, &dr, res) {
			// Include i: its own result failed to persist, so backfill it too
			// (best-effort) to avoid orphaning its tool_call.
			e.backfillInterruptedTools(ctx, c, calls[i:])
			return dr.persistFailed()
		}
		allTerm = allTerm && res.Terminate
	}
	dr.terminate = allTerm
	return dr
}

// dispatchParallel runs an all-read-only batch concurrently (bounded by
// maxParallelTools), then persists and emits results in call order. Because
// every call is launched, the log gets a result for each one even if the turn
// is cancelled mid-flight, so results persist on a cancellation-free context —
// an interrupt can never leave a dangling tool_call. A tool panic degrades to
// an error result rather than crashing the host.
func (e *Engine) dispatchParallel(ctx context.Context, c *Conversation, calls []core.ToolCall) dispatchResult {
	results := make([]core.ToolResult, len(calls))
	sem := make(chan struct{}, maxParallelTools)
	var wg sync.WaitGroup
	for i, call := range calls {
		c.emit(ctx, core.ToolStarted{Call: call}) // best-effort, in call order
		wg.Add(1)
		sem <- struct{}{}
		go func(i int, call core.ToolCall) {
			defer wg.Done()
			defer func() { <-sem }()
			defer func() {
				if r := recover(); r != nil {
					results[i] = core.ToolResult{CallID: call.ID, IsError: true, Content: fmt.Sprintf("panic in tool %q: %v", call.Name, r)}
				}
			}()
			results[i] = e.tools.Dispatch(ctx, call)
		}(i, call)
	}
	wg.Wait()

	// Persist results in call order through the shared recorder (which itself
	// persists cancellation-free). On a persist failure, backfill from the failed
	// index (inclusive) so a failure partway through can't leave dangling
	// tool_calls — including call i, whose own result just failed to persist.
	var dr dispatchResult
	allTerm := len(calls) > 0
	for i := range calls {
		if !e.recordResult(ctx, c, &dr, results[i]) {
			e.backfillInterruptedTools(ctx, c, calls[i:])
			return dr.persistFailed()
		}
		allTerm = allTerm && results[i].Terminate
	}
	if ctx.Err() != nil {
		return dr.interrupted()
	}
	dr.terminate = allTerm
	return dr
}

// backfillInterruptedTools writes a synthetic error result for each tool_call
// that never got dispatched, so an interrupted turn cannot leave a dangling
// assistant tool_call with no matching result. It uses a cancellation-free
// context derived from the turn's: the turn ctx is already cancelled, but these
// results MUST still persist to keep the log valid for replay.
func (e *Engine) backfillInterruptedTools(ctx context.Context, c *Conversation, pending []core.ToolCall) {
	bg := context.WithoutCancel(ctx) // keeps the turn correlation, drops cancellation
	for _, call := range pending {
		res := core.ToolResult{CallID: call.ID, IsError: true, Content: "interrupted before tool execution"}
		_ = e.store.AppendEvent(bg, e.stamp(bg, core.NewToolResultEvent(c.id, res, e.clock.Now())))
	}
}
