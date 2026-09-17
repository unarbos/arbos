---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# SWE-bench loop, cycle 11: pre-registration (written before the runs)

Written 2026-09-17 00:30 UTC, before either arm started.

## Lever

Second-model critique (`ARBOS_CRITIQUE=1`, kernel branch `cursor/swebench-loop-c11`, commit `9bd06a63` on `main` `5017ef45`).
On the first final reply after an edit, the kernel makes one fresh model call with no history: the request text plus `git diff HEAD`.
The reviewer lists each behaviour the request names, marks it addressed or not, names one input the diff still gets wrong, and ends with `VERDICT: COMPLETE` or `VERDICT: INCOMPLETE — <gap>`.
On INCOMPLETE the agent is nudged once with the review; on COMPLETE nothing is shown. Once per turn.
The reviewer is the same model as the agent (Sonnet 5), because the harness routes every call through the run's interception endpoint, which pins the model.

## Arms

Same kernel binary (`arbos-kernel-c11`), same harness defaults: one reproduction, $8 per-rollout cap, 2400 s timeout, Sonnet 5 via OpenRouter, regression set `reg20` at `-r 2`, concurrency 2.

- A: critique off. This is the cycle-10 baseline arm re-run (cycle 10: 26/35 = 74%).
- B: critique on.

Cap $30 each; watcher stops the eval at $27 recorded (SIGINT only).

## Decision rule (fixed now)

Compared on the instances both arms cover twice (2 rollouts each per arm).

- Adopt (harness default on): B solves at least 3 more rollouts than A on the shared instances, and B's cost per rollout is at most 1.3x A's.
- Drop (remove the code, not opt-in): B minus A is 2 or fewer rollouts, or B costs more than 1.3x per rollout without at least 4 more solved.
- Also reported, not decisive: how many rollouts the critique nudged (INCOMPLETE verdicts), how many of those flipped from failed to solved, and how many previously solved rollouts B lost.

The band is set from cycles 9 and 10: the `changes` nudge moved +2 of 20 and was called noise; N=1 against N=2 on 9 shared instances was a tie at 14/18 each.
