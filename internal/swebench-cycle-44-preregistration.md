---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# SWE-bench loop, cycle 44: pre-registration (written before the run)

Written 2026-09-19 12:50 UTC, before the run.

Second draws, the pool's next ten in order (`second10b.txt`: pylint-4551, django-16560, django-14170, sphinx-7985, django-16502, sympy-13852, pytest-7205, seaborn-3069, django-11433, sphinx-8056 — all from cycles 33–38, two or three draws each) at `-r 2`, 20 rollouts. Each failure read same-way / different-way against its first pair (class and site), with the verifier report. Not a measurement; a count on a pool selected for failure compares with nothing.

Kernel `249ddb5f9f1e` = `main` head, built in the worktree, label proved by the run; the engine changes since bec7284b (Jev's pickable tools — Jev is off here; a child agent's shell write into a root-owned file refused — no children here) do not reach a root agent under this harness. Jev off. Harness d1226d8c (#734). Network cut, sweep, one reproduction, $8 cap. Images pre-pulled. Cap $22. Walls not read if the host pauses.
