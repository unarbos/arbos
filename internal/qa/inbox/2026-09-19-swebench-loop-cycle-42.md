---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 42 — the no-change predicate did not move the choice; the never-run pool is spent

No PR. Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 42; data `media/swebench/loop/cycle-42/`.

Kernel `arbos-kernel 0.2.0 aa0f61da94a2 protocol 1` (main head, with d8344ff2), Jev off, cut, $7.51.

- **d8344ff2 on sympy-23950, three rollouts:** three of three still declare "no change needed" on "the reported output is gone" — five of five across cycles 40 and 42. If this is wanted it is a check, not a sentence. The issue's asked output is implicit, which makes the instance a hard one for the rule.
- **Last eight never-run instances: 7 of 16** (a count). django-14315 ×2 the twin (base client untouched); django-12193 ×2 the producer (caller copies, `CheckboxInput` still mutates); django-11141 ×2 an existing PASS_TO_PASS test judged "asserts the old behaviour" on *"I recall the actual Django fix did remove it"* — upstream kept it. That recollection-as-evidence shape is now three instances deep (cycles 35, 40/42, 42).
- **The never-run pool is exhausted**: 137 fresh instances across cycles 29–42, 58 failures, none outside the account. Cumulative read 352. Sixth host pause (10:24–11:00); walls not read.
