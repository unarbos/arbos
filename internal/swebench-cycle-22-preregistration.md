---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# SWE-bench loop, cycle 22: pre-registration (written before the runs)

Written 2026-09-17 21:15 UTC, before either arm started.

## The band, first

From the six repeated runs of cycles 14–16 (SD 2.8 on a 24-rollout arm; binomial 2.0), a 40-rollout arm has an SD near 3.6 and an 80-rollout arm near 5.1; the difference between two 80-rollout arms has an SD near 7. The loop's stated band is 8 of 40 — 20 points — which on 80 rollouts is **16 of 80**. A difference below 16 on the shared 2×-covered instances is reported as *not distinguishable from no effect*, never as "+n". Solve counts are reported as counts; no rate is claimed on a slice of an arm.

## What is measured, and why it can plausibly move 20 points

The only change in the loop's history with a ceiling that reaches the band: the five contract rules landed since cycle 18 (E1, E2 in #449; A, F in 0245425e; B in 7bf12e68). Cycle 18's ranking said that if all five worked perfectly on the instances read, the set would move from 54% to 78%. Cycle 20's read saw the choice made differently in 24 of 25 rollouts. This cycle asks whether that shows as a score.

## Arms

- **Control:** kernel `4b833de9860f` = `main` `7017eb75` (the base #440 was cut from: #380, #393, #397, #413 in; none of the five rules; the mechanism gate removed) plus #477's build plumbing only, so the binary can prove its label. The commit is pushed as `cursor/swebench-c22-control-7c9c`.
- **Treatment:** kernel `aaf3dbe98de7` = `main` head at 21:10 UTC, all five rules in.
- Both built in the loop's worktree by `kbuild.sh`, named by sha under `kernels/`, read-only; each run passes `kernel-sha` and refuses a binary that says otherwise; `kernel-identity.json` in every artifact.
- Same harness (this checkout, `main`), one reproduction, $8 cap, 2400 s, Sonnet 5, network cut with the pre-grading sweep, concurrency 3, both arms interleaved in time.

## The set: regression 40

`reg40.txt` = regression 20b v2 (20 instances; half of them known to flip) plus the next 20 never-run instances in `order_remaining`, taken in order. 40 instances at `-r 2` = 80 rollouts per arm. Cap $85 per arm, watcher at $80, 4 h 30 timer.

## Decision rule

On the instances both arms cover twice: **treatment − control ≥ 16** → the rules moved the set, and by at least the band; **< 16** → not distinguishable from no effect at this cost, stated as such. Either way: per-instance results, the six carrying instances tracked (11728, 24870, 14182, 13236, 14629, 6386 where present), capped rollouts, cost per rollout, and both arms' `--version` strings recorded in the write-up.
