---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 30 — ten more fresh instances; nothing outside the account

No PR. Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 30; data `media/swebench/loop/cycle-30/`.

Kernel `arbos-kernel 0.2.0 a8678ac16636 protocol 1`, cut, ten never-run instances at `-r 2`, $7.49, 11 of 20 solved (a count). Nine failures: two twin (django-12325: `options.py` beside `base.py`, both rollouts in ~15 tool calls), seven right-file-right-function details. Nothing outside the account on twenty fresh instances across cycles 29–30; cumulative read 294.
