# Phase 16 — Entrypoint and sugar tool

Back to [overview](./overview.md).

## Goal

Make pi reachable as the primary session, as a one-call sub-task, and over the
control seam, with slash-command expansion at the input boundary.

## Changes

- A `cmd/arbos` flag (for example `-agent pi`) that makes the top-level session a
  pi coding session rooted at cwd, with the full assembly from Phase 15.
- A `start_coding_session` sugar tool desugaring to
  `delegate(Task{Backend: "pi", Grant{...}})` through the `Router`.
- Wire prompt-template expansion (Phase 12) at the input boundary for both the
  entrypoint and the control seam.

## Data structures

- `StartCodingSessionArgs{ Goal, Repo string }` (fields per the `Grant` built).

## Dependencies

Phases 12 and 15.

## Verification

Static. Build, vet, test -race, generate-check, lint.

Runtime. Control-cli drives a real end-to-end coding task (read, edit, run a
command) through the one-shot entrypoint and over the control seam, asserts the
file changed and the command ran, and asserts a `/`-prefixed template expands
before the turn.
