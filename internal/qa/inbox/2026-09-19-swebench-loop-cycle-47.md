---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 47 — second draws, pool 41–50: a no-change verdict that follows the rule and still fails

No PR. Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 47; data `media/swebench/loop/cycle-47/`.

Kernel `arbos-kernel 0.2.0 249ddb5f9f1e protocol 1`, Jev off, cut, $15.28. 10 of 20 (a count on a pool selected for failure). All ten failures fail at the same site and class as before (62 of 63 across cycles 43–47). Two no-change pairs in contrast: sympy-23950 declares no change on the reported output being gone (seven of seven, the rule not reaching it); **django-13513** runs the issue's own example, shows the asked output present, declares no change with the rule's evidence honestly met — and the hidden test wants a case the issue never names. That is a ceiling on the no-change rule, not a gap in it. Eleventh host pause (18:04–18:57); walls not read. Cumulative read 416.
