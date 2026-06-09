package engine

import (
	"context"
	"fmt"
	"sync"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/ports"
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

// canSchedule reports whether a tool batch may use the concurrent scheduler:
// more than one call and none requiring human approval (an approval pause is
// inherently sequential and interactive, so an approval-bearing batch takes the
// sequential path). It no longer requires every call to be read-only — the
// scheduler partitions by per-call conflict, so a mixed batch parallelizes its
// independent calls and serializes only the conflicting ones. A batch whose
// calls all conflict (e.g. all writes to one file, or any unbounded call)
// degrades to one-per-wave, i.e. sequential, with no special-casing.
func (e *Engine) canSchedule(calls []core.ToolCall) bool {
	if len(calls) < 2 {
		return false
	}
	for _, call := range calls {
		if _, required := e.requiresApproval(call); required {
			return false
		}
	}
	return true
}

// partitionWaves groups call indices into sequential waves: every call in a wave
// is conflict-free with the others in it (safe to run concurrently), and a call
// that conflicts with an earlier one lands in a strictly later wave, preserving
// that pair's original order (so a read before a write of the same file stays
// ordered). A read-only batch collapses to a single wave; an all-conflicting
// batch degrades to one call per wave. It is pure (indices + access sets in,
// waves out) so it is testable without an engine, store, or actor.
//
// CONTRACT: waves are contiguous, ascending index ranges. dispatchScheduled
// relies on this to treat the launched calls as the prefix calls[:ran] when an
// interrupt stops later waves — a repartitioning that packs a later call into
// an earlier wave (breaking contiguity) would silently corrupt that interrupt
// path and must change it in step.
func partitionWaves(access []core.AccessSet) [][]int {
	var waves [][]int
	var cur []int
	conflictsCurrent := func(i int) bool {
		for _, j := range cur {
			if access[i].Conflicts(access[j]) {
				return true
			}
		}
		return false
	}
	for i := range access {
		if len(cur) > 0 && conflictsCurrent(i) {
			waves = append(waves, cur)
			cur = nil
		}
		cur = append(cur, i)
	}
	if len(cur) > 0 {
		waves = append(waves, cur)
	}
	return waves
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
// It is the third persistence path beside engine.append (must-persist on the
// live ctx) and engine.persistBestEffort (teardown writes): it needs append's
// loud abort but persistBestEffort's cancellation-free write, so it composes
// the two contracts inline.
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

// requestAndAwait emits a mid-turn request (ApprovalRequest) and blocks until
// the actor routes the matching response, or the turn is cancelled.
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

// dispatchScheduled runs a batch with maximum safe concurrency: it derives each
// call's resource footprint, partitions the batch into conflict-free waves, and
// runs each wave concurrently (bounded by maxParallelTools) before starting the
// next. Independent calls — read-only ones, or writes to distinct files — run
// together; only conflicting calls (same file, or an unbounded call like bash)
// are serialized into later waves, in call order. Results are then persisted and
// emitted in call order, so the persisted Seq — and therefore replay — stays
// deterministic regardless of execution order. An interrupt stops new waves
// from launching (a write must not start after the user cancelled, matching
// dispatchSequential's per-call check); calls that did run keep their real
// results and the rest are backfilled, so the log gets a result for every
// tool_call either way. A tool panic degrades to an error result.
func (e *Engine) dispatchScheduled(ctx context.Context, c *Conversation, calls []core.ToolCall) dispatchResult {
	access := make([]core.AccessSet, len(calls))
	for i, call := range calls {
		access[i] = ports.AccessOf(e.tools, call)
	}
	waves := partitionWaves(access)

	results := make([]core.ToolResult, len(calls))
	sem := make(chan struct{}, maxParallelTools)
	ran := 0 // waves are contiguous index ranges, so calls[:ran] have results
	for _, wave := range waves {
		if ctx.Err() != nil {
			break
		}
		for _, idx := range wave {
			c.emit(ctx, core.ToolStarted{Call: calls[idx]}) // best-effort, in call order
		}
		var wg sync.WaitGroup
		for _, idx := range wave {
			wg.Add(1)
			sem <- struct{}{}
			go func(i int) {
				defer wg.Done()
				defer func() { <-sem }()
				defer func() {
					if r := recover(); r != nil {
						results[i] = core.ToolResult{CallID: calls[i].ID, IsError: true, Content: fmt.Sprintf("panic in tool %q: %v", calls[i].Name, r)}
					}
				}()
				results[i] = e.tools.Dispatch(ctx, calls[i])
			}(idx)
		}
		wg.Wait()
		ran += len(wave)
	}

	// Persist the results that ran, in call order, through the shared recorder
	// (which itself persists cancellation-free). On a persist failure, backfill
	// from the failed index (inclusive) so a failure partway through can't leave
	// dangling tool_calls — including call i, whose own result just failed to
	// persist.
	var dr dispatchResult
	allTerm := len(calls) > 0
	for i := 0; i < ran; i++ {
		if !e.recordResult(ctx, c, &dr, results[i]) {
			e.backfillInterruptedTools(ctx, c, calls[i:])
			return dr.persistFailed()
		}
		allTerm = allTerm && results[i].Terminate
	}
	if ran < len(calls) {
		e.backfillInterruptedTools(ctx, c, calls[ran:])
		return dr.interrupted()
	}
	if ctx.Err() != nil {
		return dr.interrupted()
	}
	dr.terminate = allTerm
	return dr
}

// backfillInterruptedTools writes a synthetic error result for each tool_call
// that never got dispatched, so an interrupted turn cannot leave a dangling
// assistant tool_call with no matching result. These results MUST still
// persist to keep the log valid for replay; persistBestEffort drops the
// cancellation (the turn ctx is already cancelled) and surfaces any failure on
// the operational plane.
func (e *Engine) backfillInterruptedTools(ctx context.Context, c *Conversation, pending []core.ToolCall) {
	for _, call := range pending {
		res := core.ToolResult{CallID: call.ID, IsError: true, Content: "interrupted before tool execution"}
		e.persistBestEffort(ctx, c, core.NewToolResultEvent(c.id, res, e.clock.Now()))
	}
}
