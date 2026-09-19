---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 43 — second draws: sixteen of sixteen repeat failures fail the same way

No PR. Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 43; data `media/swebench/loop/cycle-43/`.

Kernel `arbos-kernel 0.2.0 bec7284b8740 protocol 1` (main head), Jev off, cut, $12.00. Second draws on the first ten of the once-failed pool (64 instances, ordered fewest-draws-first): 3 of 20 (a count on a pool selected for failure). Two instances flipped; eight failed both draws again, and every one of the sixteen rollouts failed the same way as its first pair — same class, same file, in sympy-15875 the same construction. The two twins (django-12325 `options.py`, sympy-20438 `comparison.py`/`relational.py`) are four-of-four misses on sites the issue points to. Seventh host pause (11:25–12:17); walls not read. Cumulative read 369.
