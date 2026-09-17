# qa-021: every `ask` is delivered twice and `answer` names no question, so a late or duplicate answer resolves whatever is pending

status: pr-open — https://github.com/unarbos/arbos/pull/81 (branch `cursor/fix-qa-021-ask-ids-52cd` -> `cursor/release-integration-52cd`); desktop reaction fixed in fb492ce
severity: high (questions to the user resolved blank 0.1 s after being asked)
scenario: ask-identity (runner, model) and `crates/arbos-kernel/tests/ask_identity_e2e.rs`
found: Jacob's Mac, desktop on the integration branch
fingerprints: none 2111b8f0be 559b02d119 aff06d8428 fccce121b0

## Repro

Any `ask` tool call. The client receives `Frame::Ask` immediately and, up to 200 ms later, the same question as a `Frame::Event { kind: ask }` from the transcript tail. A client that treats the second as new and "skips" the first sends `Frame::Answer { text: "" }`; the kernel resolves the pending ask with it.

## Expected

An ask has an identity; an answer names it; a late or duplicate answer resolves nothing and the client is told.

## Actual

`Frame::Ask { agent, question, options }`, `Frame::Answer { agent, text }`: the kernel keyed pending asks by agent only and accepted any answer for that agent, appending a stray `answer` line even when nothing was pending.

## Fix

Ids on `ask`/`answer` (approve uses `call_id`); `answer_allowed` rule: nothing pending → refused; a known but non-pending id → refused; no id or an unknown id → accepted only when exactly one question is pending; refusals as `error` frames.

## Rule change on `main` (ui-004), 2026-09-14

`answer_allowed` now takes an id the kernel never issued, with exactly one question pending, as that question's answer (the desktop's Skip once sent a placeholder id and the turn hung; ui-004). The `ask-identity` scenario no longer sends a never-issued id; the stale case it checks is a *late* answer (the real id after the question resolved), which is still refused. Trade-off noted: a blank answer with a made-up id now skips the one pending question.
