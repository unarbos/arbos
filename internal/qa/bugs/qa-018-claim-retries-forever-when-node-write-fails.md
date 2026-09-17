# qa-018: when the plan node cannot be rewritten, `claim()` leaves an open attempt and retries every 5 s forever

status: pr-open — https://github.com/unarbos/arbos/pull/68 (branch `cursor/fix-qa-017-018-52cd` -> `cursor/release-integration-52cd`)
severity: medium (a prompt that cannot be stored spins silently: 13 open attempts in a minute, node pending, no turn, no error to the client)
scenario: disk-full
rollout: /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa/rollouts/20260913T065455Z-disk-full
fingerprints: 3d0eb4ddae 22537835bf

## Repro

Kernel under `RLIMIT_FSIZE = 256 KB` (#25 keeps it alive), send a 200 KB prompt. The inbox write fits; the claim's rewrite of the node (append of the whole node again) does not.

## Expected

The claim fails once, the attempt is closed (inconclusive, "could not write plan: EFBIG"), the failure is logged and reported, and the scan backs off instead of retrying every tick.

## Actual

`attempts.jsonl` grows by one open attempt every 5 s (a1..a13 after 60 s); `plan.jsonl` node stays `pending`; no `turn` frame, no `error` frame; nothing in kernel.log beyond the scan.

## Suspected location

`crates/arbos-kernel/src/plan.rs` `claim()`: `save_attempt` then `save_node(...).ok()?` — the attempt is written before the node, and a node-write failure returns `None` with no cleanup or backoff. Fix idea: write the node first; on failure close the attempt, log `claim_failed`, and skip that node for 60 s.
