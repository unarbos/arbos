---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 41 — the tree-moving step not exercised in three; ten more fresh instances

No PR. Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 41; data `media/swebench/loop/cycle-41/`.

Kernel `arbos-kernel 0.2.0 18f397cb34ab protocol 1` (main head, with 42753ae5), Jev off, cut, $14.69.

- **42753ae5 on pytest-10356, three rollouts:** no rollout offered a tree-moving reproduction, so the refusal was not seen; no empty patch; the cycle-39 loss did not recur. The commit's unit test is the evidence for the refusal, not this read.
- **Fresh ten: 17 of 20** (a count). django-13401: the twin — `__eq__` and `__lt__` changed, `__hash__` left alone, on an issue whose symptom is a `set` de-duplicating (A, evident). matplotlib-20676 ×2: C, consistent.
- Fifth host pause (08:55–09:43); walls not read. Cumulative read 340; nothing outside the account.
