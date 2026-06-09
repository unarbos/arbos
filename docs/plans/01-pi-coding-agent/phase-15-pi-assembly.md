# Phase 15 — pi assembly and backend registration

Back to [overview](./overview.md).

## Goal

Assemble the coding registry, model registry, system prompt, skills, prompt
templates, and compaction into an engine config, and register `pi` as a
delegatable backend.

## Changes

- A coding engine-config factory in `internal/agent/pi`: the coding tool registry
  (Phase 4 to 9), the model registry (Phase 3), the assembled prompt (Phase 10),
  skills (Phase 11), the compaction policy and summarizer (Phase 13), the
  terminate-aware loop (Phase 14), and `ApprovalPolicy: nil` for pi's full-
  privileges behavior (D2). Document the opt-in gating wiring.
- Set `Config.Reasoning` and `Config.CacheRetention` (default "short") from the
  model registry; the engine passes the session id into the request for
  session-affinity caching (D9).
- Wrap the factory as an `ArbosAgent` and register it under backend `pi` in the
  `Router`.

## Data structures

- The engine-config factory and a backend registration entry.

## Dependencies

Phases 1 to 14.

## Verification

Static. Build, vet, test -race, generate-check, lint.

Runtime. Harness issues `delegate(backend="pi", ...)` against a scripted provider
emitting a read-then-edit-then-bash sequence and asserts the child runs the
tools, applies the edit, and returns a final response.
