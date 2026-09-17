# qa-013: after a clock rewind a recurring node waits until the old clock time comes back

status: pr-open — https://github.com/unarbos/arbos/pull/26 (branch `cursor/fix-qa-013-cron-clock-rewind-de28` -> `rust`)
severity: medium (standing jobs go silent for as long as the clock jumped)
scenario: clock-jump-cron
rollout: /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa/rollouts/20260913T004855Z-clock-jump-cron
fingerprints: 9d52339d76

## Repro

Plan with a recurring shell node (`every 30s`) whose `next_due_ms` is ten days ahead (what a clock rewind leaves behind). Start the kernel, wait 8 s.

## Expected

The node fires once now and re-arms from now (mirror of the overdue case, which coalesces correctly).

## Actual

`next_due_ms` unchanged: still 10.0 days ahead. The overdue node fired exactly once (correct); the far-future one-shot waited (correct).

## Suspected location

`crates/arbos-core/src/node.rs` `ready()`: recurring nodes are due only when `now >= next_due`.
