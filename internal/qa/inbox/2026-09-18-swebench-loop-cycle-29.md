---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 29 — the predicate sentence read; ten fresh instances, nothing outside the account

No PR. Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 29; data `media/swebench/loop/cycle-29/`.

Kernel `arbos-kernel 0.2.0 d12118e60d8b protocol 1` (1b083988 in), cut, $21.15. sympy-17318 under "when you can name the wrong predicate, change the predicate": five rollouts name the predicate, two change it (both solve), three guard downstream (all fail) — 0/4, 1/5, 1/5, 2/5 across the four cycles this instance has been read. Ten never-run instances at `-r 2`: 15 of 20 solved (a count); the five failures are two twin cases (sympy-20438: the `Eq` handler beside the `is_subset` handler) and three right-file-right-mechanism details. Nothing outside the account; cumulative read 285.
