# qa-007: SIGINT during a running turn does not end the turn; the folder looks exactly like a crash

status: pr-open — https://github.com/unarbos/arbos/pull/12 (branch `cursor/fix-qa-007-graceful-stop-de28` -> `rust`)
severity: low-medium (recovery exists via `reclaim`, but a "stopped" agent silently resumes on next start, and the transcript has no record of the stop)
scenario: kickoff-session (kernel stopped while goal 10 was still running); reproducible with any model turn + SIGINT
rollout: /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa/rollouts/ (first kickoff attempt, superseded; see kickoff-history.jsonl for the current run)
fingerprints: bcb2294131 d4f73e35a3 fb5b128a4e 4b597d58b0 f2866e6a9f 0f2035577c 3c916ea97a ef97cb7674

## Repro

1. Start the kernel with a model key, attach, send a prompt that takes a few seconds.
2. While `turn running`, send SIGINT. The kernel prints `arbos-kernel stopping` and exits 0.
3. Run `consistency.py <place>`.

## Expected

On a graceful stop the kernel stops in-flight turns (`TurnControl::stop`), waits briefly, and each transcript ends with `interrupted { detail: "kernel stopping" }` + `turn_complete`, so the plan node closes as stopped. The next start does not resume the turn unasked.

## Actual

Findings: `transcript-unended-turn`, `plan-active-after-stop`, `attempt-running-after-stop`. The next start runs `reclaim` (`plan.rs:62-108`) and `needs_serve` continues the turn from the transcript. Nothing on disk says the user pressed stop.

## Suspected location

`crates/arbos-kernel/src/serve.rs:290-293`: the `ctrl_c` branch only `break`s. `Scheduler` (`sched.rs:34-38`) has `stop(id)` but it is not called on shutdown; the runtime drop then cancels the turn tasks mid-write.

## Also seen

- ArbosLife cycle 2026-09-12T23:45Z, scenario `malformed-frames` with a model key: the empty `user` frame (`text: ""`) started a real model turn, and SIGINT at the end of the scenario left node #1 active and attempt a1 open (fingerprints 4b597d58b0, f2866e6a9f). The empty prompt itself is qa-008.
- ArbosLife cycle 2026-09-13T00:32Z, scenario `inbox:worktree-cwd-confinement` (feature branch `cursor/worktree-cwd-confinement-b027`, features agent): a follow-up turn was still running when the scenario stopped the kernel; same three findings (0f2035577c, 3c916ea97a, ef97cb7674). The feature itself passed its exercise.
