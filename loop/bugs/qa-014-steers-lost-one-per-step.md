# qa-014: steers sent faster than the model steps are lost

status: pr-open — https://github.com/unarbos/arbos/pull/28 (branch `cursor/fix-qa-014-steer-drain-de28` -> `rust`)
severity: medium (user instructions during a turn silently dropped)
scenario: steer-storm
rollout: /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa/rollouts/20260913T005356Z-steer-storm
fingerprints: e020a26071

## Repro

Start a long tool-heavy turn (15 `bash sleep 1` calls), then send 25 `user` frames with `steer: true` in quick succession.

## Expected

All 25 appear on the transcript as `user` events, in order, at the next tool boundaries.

## Actual

9 of 25 on `rust` (the first nine, in order); 13 of 25 on the features branch `cursor/steer-running-child-b027`, which requeues leftovers at turn end but still takes one steer per step.

## Suspected location

`crates/arbos-engine/src/turn.rs` top of the loop: `control.take_steer()` (one); `control.rs` steer `VecDeque` dropped with the turn.
