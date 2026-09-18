---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# SWE-bench loop, cycle 24: pre-registration (written before the runs)

Written 2026-09-18 00:45 UTC, before either arm started.

## Ceiling on the measured set, then the band — in that order, as cycle 22 taught

**Set:** the six instances the five rules were written from (`carry6.txt`): django-11728, matplotlib-24870, astropy-14182 (twin), astropy-13236 (stage), scikit-learn-14629 (producer), pylint-6386 (behaviour, not crash). `-r 6` = 36 rollouts per arm.

**Ceiling.** On pre-rule kernels these six solved 37 of 72 rollouts across cycles 14–16 (rate 0.51) and 7 of 12 in cycle 22's control. Expected control on 36 rollouts: about 18–21. The most the treatment can gain if every rule works on every rollout: **15–18 of 36.**

**Band.** From the six-run estimate scaled to 36 rollouts: per-arm SD ~3.4, difference SD ~4.8, two SDs **~10 of 36**. The ceiling is above the band, so the run is allowed under the rule set after cycle 22. If the control comes in above 26 of 36 the realised ceiling falls under the band and the write-up says so before any number.

## Arms

Control kernel `4b833de9860f` (`main` `7017eb75` + #477 plumbing; `arbos-kernel 0.2.0 4b833de9860f protocol 1`); treatment `aaf3dbe98de7` (`main` head 21:10 UTC 17 Sep, all five rules; `arbos-kernel 0.2.0 aaf3dbe98de7 protocol 1`). The same two binaries as cycle 22, read-only under `kernels/`, labels proved by the run (`kernel-sha`). Same harness, one reproduction, $8 cap, network cut with the sweep, concurrency 3, arms interleaved. Cap $45 each, watcher $42, 3 h timer.

## Decision rule

Treatment − control on the 36 shared rollouts: **≥ 10 → the rules moved the instances they were written for, by at least the band; < 10 → not distinguishable from no effect at this cost.** Reported alongside: per-instance results per arm, the marks (step and line) in the treatment transcripts, every failure read against the account (A/B/E1/E2/F/G/C), capped rollouts, provider stalls, both `--version` strings.
