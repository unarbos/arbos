---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# SWE-bench loop, cycle 47: pre-registration (written before the run)

Written 2026-09-19 17:55 UTC, before the run.

Second draws, the pool's next ten in order (`second10e.txt`): django-13449 (0011), pytest-10356 (01010), sympy-23950 (00000), sympy-18698 (010001), astropy-12907 (000011), sphinx-8265 (1101111), django-15503 (0/8), django-13513 (0/9), django-11099 (7/9), sympy-15017 (7/11). At `-r 2`, 20 rollouts. Each failure read same-way / different-way against its earlier draws, with the verifier report. sympy-23950 is in the pool by its rule; its reading (no change on "the reported output is gone", five of five) is recorded and is not being filed — a sixth and seventh draw are read for whether the verdict changes, nothing more. Not a measurement.

Kernel `249ddb5f9f1e` (cycles 44–46's; `main` at 262044ee with no engine or host change since). Jev off. Harness d1226d8c (#734). Network cut, sweep, one reproduction, $8 cap. Images pre-pulled. Cap $25. Walls not read if the host pauses.
