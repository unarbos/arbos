---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 46 — second draws, pool 31–40: the regression-era instances

No PR. Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 46; data `media/swebench/loop/cycle-46/`.

Kernel `arbos-kernel 0.2.0 249ddb5f9f1e protocol 1`, Jev off, cut, $25.98. 9 of 21 (a count on a pool selected for failure). The four instances with an earlier solve solved both; of six never-solved, django-14792 flipped once (its first solve in six draws, after a pause-killed rollout was re-run). All eleven read failures fail at the same site and class as before; django-15732 is a clean case of two halves of one fix, each rollout doing one. Across cycles 43–46: 52 of 53 repeat failures repeat site and class. Tenth host pause (16:13–17:01); walls not read. Cumulative read 406.
