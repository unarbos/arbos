---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# SWE-bench loop, cycle 46: pre-registration (written before the run)

Written 2026-09-19 16:00 UTC, before the run.

Second draws, the pool's next ten in order (`second10d.txt`): xarray-6992, sphinx-7590, sympy-13878 (two draws each, from the cycle 12–28 reads) and django-14017, django-14792, django-15732, django-16877, django-14771, django-11133, astropy-13398 (four draws each, from the regression-set era). At `-r 2`, 20 rollouts. Each failure read same-way / different-way against its earlier draws (class and site, from the loop doc's readings), with the verifier report. Not a measurement.

Kernel `249ddb5f9f1e` (cycles 44–45's; `main` at 354413e0 with no engine or host change since). Jev off. Harness d1226d8c (#734). Network cut, sweep, one reproduction, $8 cap. Images pre-pulled. Cap $25. Walls not read if the host pauses.
