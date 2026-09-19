---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 37 — ten more fresh instances, all solved; a fifty-minute host stall

No PR. Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 37; data `media/swebench/loop/cycle-37/`.

Kernel `arbos-kernel 0.2.0 e5e66f71c418 protocol 1` (main head), Jev off, cut, $7.06, 20 of 20 (a count). Nothing to read. The host stalled 01:58:53–02:49:03 UTC (no eval-log line in the gap); three live rollouts each came back with "nothing has happened for 48m" and then solved. Wall times from this cycle are not to be read. Cumulative read 314; nothing outside the account on ninety fresh instances.
