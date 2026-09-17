---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# SWE-bench loop, cycle 16: pre-registration (written before the runs)

Written 2026-09-17 11:20 UTC, before the first run.

## The question

Cycle 14 (kernel `30eef166`) and cycle 15's no-lever arm (kernel `7e19f9e9`) covered twelve instances twice each under identical harness settings and the network cut: 16/24 → 9/24. Is that the kernel, and if so which merge?

## The twelve (`shared12.txt`)

astropy-13236, astropy-14182, django-11728, django-15022, django-15252, django-16454, matplotlib-24870, pylint-6386, pylint-8898, pytest-6197, scikit-learn-14629, sphinx-8035. Every run below is these twelve at `-r 2` = 24 rollouts, one reproduction, $8 cap, 2400 s, Sonnet 5, concurrency 3, network cut with the pre-grading sweep. Harness: `main` + #413 (checkout does not change during a run; kernels are built from a separate worktree).

## Step 1 — confirm

Re-run on `30eef166` (`arbos-kernel-c14`, the cycle-14 binary itself). Reference points: cycle 14 gave 16/24 on this kernel; cycle 15 arm A gave 9/24 on `7e19f9e9`.

- **Gap holds** if the re-run is ≥ 13/24 (at least 4 above arm A, outside the ±2 band on both sides). Go to step 2.
- **Gap does not hold** if the re-run is ≤ 11/24: cycle 14's 16 was the outlier; the instrument's variance on this set is wider than the band and the write-up says so. No bisect.
- 12/24: ambiguous; run `7e19f9e9` once more on the same twelve before deciding.

## Step 2 — bisect, in Jacob's suspected order

Kernel-touching merges on `main` between the bases, in merge order: #400, #401 (hub/worker, 1 file each), #399 (mechanism gate removed), #385 (binary_gone, kernel/hub), #407 (jobs), #408 (turn errors), #405 (checkpoint written and awaited before the turn proceeds), #410 (wipe guard).

1. `1c550f9e` (everything up to and including #408; #405 and #410 not in). If it scores like the good base (≥ 13) the drop is in #405 or #410; run `b133af2c` (#405 in, #410 not) to split them. If it scores like the bad base (≤ 11) the drop is in #399/#385/#407/#408; run `58edb3e6` (#399 in, the rest not), then `10e11f1d` (#385 in) as needed.
2. A point "carries the drop" when it scores ≤ 11 and the point before it scores ≥ 13. In between, one more run at the same point rather than a guess.

Each point costs about $25; the confirm plus two to four points is $75–125. Stated now: the loop's $60 cycle budget does not cover this, and Jacob has said to spend what it takes.

## What is recorded either way

Per-instance results at every point; the five instances that fell (matplotlib-24870, astropy-14182, pytest-6197, astropy-13236, pylint-8898) tracked across points; for the culprit, what the transcripts do differently (first-write latency for #405; the pre-edit shape for #399). Soundness at every point: egress 0, no fetch, sweep `left` 0.

No lever this cycle.

## Amended 11:35 UTC, before the flag arms started (the engine's author's reading of the five merges)

- #405 is withdrawn as a suspect: the write-wait Jacob suspected landed in #419 (`663d8ce5`), after `7e19f9e9`; within the range #405 awaits only a cheap record. No run on it.
- #410 is **ruled out by grep**: `bash: refused` appears 0 times in all 34 cycle-15 failures and 0 times in any cycle-14 or cycle-15 rollout (`rm -rf` commands: 9–10 per run, none refused).
- #399 is the prime suspect and is tested directly on today's kernel: [#440](https://github.com/unarbos/arbos/pull/440) (`1e9be864`, on `main` `7017eb75`) restores the pre-#399 gate behind `ARBOS_MECHANISM_REQUIRED=1`. Two arms on that one kernel, the twelve at `-r 2`: **gate off** (A) and **gate on** (B, `--env.agent.harness.env.ARBOS_MECHANISM_REQUIRED 1`; the harness no longer passes the variable itself since 8f4a62f3, so it goes through the harness's generic `env`). $34 watchers, 3 h timer each.
- Decision: the gate explains the drop if B ≥ A + 4 on the 24 rollouts; it does not if B ≤ A + 2; +3 → one more run of each. If it does not, the confirm run on `30eef166` (already running, kept as the anchor) tells whether the gap itself reproduces, and #407/#408 are next, in that order.
- The confirm run on `30eef166` started at 11:12 UTC before this amendment and continues; it is the anchor for "what does the good base do today".
