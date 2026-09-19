---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 44 — second draws, pool 11–20: flippier set, same failure shapes

No PR. Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 44; data `media/swebench/loop/cycle-44/`.

Kernel `arbos-kernel 0.2.0 249ddb5f9f1e protocol 1` (main head), Jev off, cut, $25.30 (the cycle cap stopped the run at 17; two pairs finished alone). 10 of 21 (a count on a pool selected for failure). Three of five twice-failed instances got a solve on the second pair (14170, 11433, 7985), two flippy ones solved both (16560, 3069), pytest-7205 went 10 → 00. Of eleven failures, ten fail at the same site and class as their first pair; one (14170) in the same class at a new site. Across cycles 43–44: 26 of 27 repeat failures repeat site and class. Eighth host pause (12:59–13:53); walls not read. Cumulative read 380.
