# Phase 14 — Loop fidelity (terminate, steering)

Back to [overview](./overview.md).

## Goal

Close the remaining loop-behavior gaps between pi's agent loop and arbos's turn
loop, where they affect intelligence rather than UI.

## Changes

- Port the `terminate` hint: a tool result may signal the loop to stop after the
  current batch when every finalized result in the batch sets it. This affects
  loop control, so it is in scope.
- Assess steering and follow-up against arbos's existing FIFO prompt queue and
  interrupt. Position: arbos's `Queued` follow-up plus interrupt already covers
  follow-up and a coarse steer; port pi's mid-batch steering only if a runtime
  gap shows up. Record the decision in the package.
- before/after tool hooks: arbos's approval gate is the `beforeToolCall` analog
  (left nil per D2). Note the seam; do not build hooks speculatively.

## Data structures

- A `terminate` flag on the tool-result path (reuses Phase 1 result shape, no new
  core type if avoidable).

## Dependencies

Phases 1 and 4 to 9 (tools and result shape).

## Verification

Static. Build, vet, test -race, generate-check, lint.

Runtime. Harness scripts a tool batch that sets terminate and asserts the loop
ends without another provider call; a non-terminating batch continues.
