---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 32 — ten more fresh instances; the loop's kernel runs without Jev

No PR. Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 32; data `media/swebench/loop/cycle-32/`.

Kernel `arbos-kernel 0.2.0 a8678ac16636 protocol 1`, cut, $11.14, 16 of 20 (a count). Four failures, all inside the account: django-14034 ×2 (the hidden test grades rendering; the issue describes validation), sympy-21596 ×2 (right handler, different construction). Cumulative read 305; nothing outside the account on forty fresh instances.

Worth knowing for QA: **Jev is off under the harness** (provider is `custom`, Jev defaults on only for OpenRouter). Every loop count since Jev landed is of the one-model loop, not the desktop agent on OpenRouter. Whether the loop should switch is a coordinator question; switching is a new instrument.
