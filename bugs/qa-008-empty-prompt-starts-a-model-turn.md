# qa-008: an empty prompt is accepted and starts a real model turn

status: pr-open — https://github.com/unarbos/arbos/pull/19 (branch `cursor/fix-qa-008-empty-prompt-de28` -> `rust`)
severity: low (wasted model call per empty send; the window's send button may already guard it, the socket does not)
scenario: malformed-frames (with a model key)
rollout: /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa/rollouts/20260912T234707Z-malformed-frames (ArbosLife)
fingerprints: none yet (found while triaging 4b597d58b0)

## Repro

Attach and send `{"type":"user","agent":"root","text":""}` with a model configured.

## Expected

The frame is refused (`error` frame: "empty prompt"); nothing is written; no model call.

## Actual

A plan node with an empty goal is written, a turn starts, the model is called with an empty user message, and money is spent. In the rollout the turn was still running when the scenario ended (see qa-007).

## Suspected location

`crates/arbos-kernel/src/serve.rs:308-326` (`Frame::User`): no check on `text.trim().is_empty()` before `hooks.inbox`. `hooks.rs:423-443` `inbox` accepts any goal; `NewNode::build` refuses an empty goal for the `plan` tool but the inbox path does not go through it.
