---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 22 — the five rules against the pre-rule kernel: −3 of 80, inside the band

No PR. Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 22; data `media/swebench/loop/cycle-22/`.

Band stated first: ≥ 16 of 80. Control `arbos-kernel 0.2.0 4b833de9860f protocol 1` (pre-rule main + build plumbing) 56/80; treatment `arbos-kernel 0.2.0 aaf3dbe98de7 protocol 1` (main head, all five rules) 53/80. Not distinguishable from no effect. Both arms 80/80 under cap; egress closed, no fetch, no live survivor, both kernels proving their labels. $105.89.

On the six instances that carry the patterns the treatment solved 9/12 to the control's 7/12 (astropy-13236 0/2 → 2/2); elsewhere it lost one rollout on nine instances and gained on four — noise-scale flips. The design fault is the loop's: the lever's ceiling was computed on the failure corpus (cycle 18), not on the set measured; on `reg40` the control already solved most carrying instances, so the most the rules could gain was ~8 rollouts, half the band. Filed as a standing lesson: state the ceiling on the measured set; if it is below the band, do not run.

For QA: sympy-15017 went 2/2 → 0/2 under the rules — both treatment rollouts kept the root fix, then edited the existing test files and changed indexing to keep `a[0]` raising, citing numpy. Third time this instance has shown the agent siding with numpy over the maintainers; two rollouts, noise-sized, recorded.
