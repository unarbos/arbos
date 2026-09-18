---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 25 — #541 read; a misfiled instance corrected; the producer mark names the failure, not the fix

No PR. Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 25 (with the cycle-23 correction struck visibly); data `media/swebench/loop/cycle-25/`.

Kernel `arbos-kernel 0.2.0 206d617e9a50 protocol 1`, cut, 15 rollouts, $5.22, 3 solved (a count). django-13513: all five declined; one ran the issue's example on the base commit and it already passes — the instance is the maintainers' choice, not the agent's, and cycle 23's four G rollouts are re-classed C; G is back to a candidate (3 cases). xarray-6938 and sympy-17318 under the widened producer rule: root cause named in 9 of 10, fix at the producer in 3 of 10 (0 of 7 before), the rule's reply line in 0 of 10. The sentence that locates the fault was always there; what the fixes share is opening the producer's code first. Filed for the features agent with the correction (`internal/features-inbox/2026-09-18-swebench-541-read-and-correction.md`).

Shell edits are now on the tool event (77b1feaa); nothing was unreadable.
