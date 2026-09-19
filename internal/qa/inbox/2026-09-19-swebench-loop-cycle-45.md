---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 45 — second draws, pool 21–30: fifteen of fifteen the same way

No PR. Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 45; data `media/swebench/loop/cycle-45/`.

Kernel `arbos-kernel 0.2.0 249ddb5f9f1e protocol 1`, Jev off, cut, $10.30. 5 of 20 (a count on a pool selected for failure). None of seven twice-failed instances flipped; all fifteen failures fail at the same site and class as their first pair. Across cycles 43–45: 41 of 42. One sentence worth a QA case, django-14315 `3e1d4889`: *"Only the two expected pre-existing tests fail (they assert the exact buggy behavior)"* — said of the two tests that encode the fix in the base client the agent never touched. Ninth host pause (14:48–15:39); walls not read. Cumulative read 395.
