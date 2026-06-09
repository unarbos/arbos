# ADR-0025 — Tool terminate hint

- Status: Accepted
- Date: 2026-06-08

## Context

pi's agent-core lets a tool result hint that the loop should stop after the
current tool batch (`terminate: true`), and the loop stops early only when every
finalized result in the batch sets it. This is loop-control behavior, not a
rendering detail, so the faithful pi port needs it. arbos's turn loop had no such
hint.

## Decision

Add an additive `Terminate bool` to `core.ToolResult` (and the `tool.Result` a
handler returns). After a tool batch completes normally, the engine ends the turn
gracefully with a new `StopTerminated` stop reason when the batch is non-empty
and every result set `Terminate`. Interrupt and persist-failure paths are
unaffected (they stop via the existing `stop`/`completedNormally` flags and leave
`terminate` false). A denied approval result is non-terminating, so an
approval-denied tool never ends the turn.

## Consequences

- A tool can cleanly end a turn (e.g. a future "done"/"submit" tool); the seven
  core coding tools do not set it, so current behavior is unchanged.
- Additive: `Terminate` is `omitempty`; old persisted tool results decode with it
  false. No event-version bump. `StopTerminated` is a new stop reason a frontend
  can render distinctly.
- Steering is intentionally not added as a separate mechanism: arbos's existing
  FIFO follow-up queue (the `Queued` event) plus `InterruptIntent` cover pi's
  follow-up and coarse steering, the minimal decision recorded in the plan.
