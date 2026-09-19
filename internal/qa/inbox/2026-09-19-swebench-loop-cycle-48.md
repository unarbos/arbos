---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 48 — the second-draw pool's last fourteen; the pool is spent

No PR. Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 48; data `media/swebench/loop/cycle-48/`.

Kernel `arbos-kernel 0.2.0 bd50ec58eddc protocol 1` (main head), Jev off, cut, $27.44. 18 of 28 (a count on a pool selected for failure); the fourteen regression members matched their long histories. Nine failures read, all at the same site and class as before (the two cycle-27 producer instances, sympy-17318 and xarray-6938, still fail the producer way). One rollout lost to a host pause mid-fix. Second-draw pool totals over cycles 43–48: 64 instances, 112 rollouts, 71 of 72 repeat failures same site and class, 4 of 28 twice-failed instances flipped, nothing outside the account. Both pools are now spent. Cumulative read 425.
