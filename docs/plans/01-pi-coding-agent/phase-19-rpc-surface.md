# Phase 19 — RPC command surface (non-tree)

Back to [overview](./overview.md). Status: done. ADR-0026.

## Goal

Extend the control seam to carry pi's RPC commands beyond prompt+events, for the
non-tree ones: set_model, compact, abort, follow_up/steer.

## Mapping and changes

- abort -> `InterruptIntent` (pre-existing). follow_up / steer -> `PromptIntent`
  (queued mid-turn, surfaced as `Queued`; pre-existing). These already flow over
  the seam's `intent` frame.
- New `core.SetModelIntent{Model}` and `core.CompactIntent{}` (sealed set +
  codec). The engine applies them idle-only: a per-session model override on the
  `Conversation` for set_model, and `compactNow` (force a `ContextPolicy` pass)
  for compact. Mid-turn they are benign no-ops (preserve single-writer / no
  mid-turn mutation). `runTurn` reads the per-session model override.

## Verification

Static gate green, no new lints, tests pass with -race. Runtime: over the real
control seam, `set_model m2` then a prompt produced a turn that used model `m2`;
a `CompactIntent` (idle, FIFO before the next prompt) produced a
`CompressionPayload`.

Deferred to Phase 21 (needs the session tree): fork, switch_session.
