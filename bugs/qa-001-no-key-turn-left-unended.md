# qa-001: a turn with no API key leaves the transcript unended, replays on every restart, and closes the user's node as done

status: pr-open
severity: high (state inconsistency; silent data growth; user is never told the turn failed)
scenario: prompt-no-key (also echoed by every no-key run of concurrent-sessions, rapid-create-delete, huge-input, malformed-folder, malformed-frames)
rollout: /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa/rollouts/20260912T223419Z-prompt-no-key
fingerprints: 3e3f75b8f1 80cbe8aac0 024560c4a1 15be798dbc 5b8d5d8568 dc7c17de8d eeab0db489 2fc27a317a ce456f1477 72a1f6ccd8 2e06b430ad 3a1f63922f 8221f69f07 cc36bc4eff 9a7195c831 62ec4b87fc c98493bdd3 cb102b75a3 70971cf20c 00cca844dc
pr: https://github.com/unarbos/arbos/pull/7 (branch `cursor/qa-loop-de28` -> `rust`, ready for review)

## Repro

1. Start `arbos-kernel serve <empty folder>` with no `api_key` in config and no `OPENAI_API_KEY` in the environment.
2. Attach and send `{"type":"user","agent":"root","text":"Say hello."}`.
3. Wait for `turn idle`. Read `.arbos/agents/root/transcript.jsonl` and `plan.jsonl`.
4. Stop the kernel with SIGINT and start it again twice, sending nothing.

## Expected

- The transcript ends with a failed `notice` ("no API key ...") and a `turn_complete`.
- The user's plan node is `failed` with that message as outcome.
- Restarts with no input do not change the transcript.

## Actual

- Transcript is `wake, user` and stops (`state-after/agents/root/transcript.jsonl` in the rollout). No notice, no `turn_complete`. The only trace is `turn: no API key ...` on kernel stderr.
- `plan.jsonl`: node #1 is `done` with outcome `(no reply)`; `attempts.jsonl` a1 has `verdict: success`, `verified_by: kernel`.
- Each restart appends one more `wake: serve` line: 2 lines -> 4 lines after two restarts (`driver.log`). `needs_serve()` sees an unfinished wake forever.

## Suspected location

- `crates/arbos-engine/src/turn.rs:129-131` (on `origin/rust`): `host.api_key().ok_or_else(...)?` returns after the wake and user events were appended at lines 103-116, so the `end()` closure at 213-224 never runs. Contract in the doc comment at 82-84 says every exit ends the transcript.
- `crates/arbos-kernel/src/plan.rs:383-407` `turn_outcome`: no assistant text and no failed notice reads as `(no reply)` with `ok = true`, so `finish_turn` marks the node done.
- `crates/arbos-core/src/files.rs:231-254` `needs_serve`: correct, it just reports the unended wake.

## Overlap

The features agent's PR #6 (`cursor/openrouter-default-provider-b027`, OpenRouter-first backend) contains the same fix in a `refuse()` helper in `turn.rs`. If #6 merges first, #7 conflicts in `turn.rs` and should be reduced to its regression test, which is the part #6 lacks.

## Fix

Branch `cursor/qa-loop-de28`, commit `b8dc09d`: a missing key now appends a failed `notice` naming the config file and env var, then `turn_complete`. A `Compact` wake still bails (it wrote nothing). Regression test: `crates/arbos-engine/tests/turn_contracts.rs` (fails on the old code, passes on the fix).
