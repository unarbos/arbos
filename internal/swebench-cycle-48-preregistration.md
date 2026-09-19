---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# SWE-bench loop, cycle 48: pre-registration (written before the run)

Written 2026-09-19 19:30 UTC, before the run.

The second-draw pool's last fourteen (`second14f.txt`): the many-draw regression members — xarray-6938, django-16454, sphinx-8035, pytest-6197, pylint-8898, django-15252, sympy-17318, django-15022, django-11728, matplotlib-24870, astropy-13236, pylint-6386, astropy-14182, scikit-learn-14629 — 14 to 33 draws each already. At `-r 2`, 28 rollouts. Each failure read same-way / different-way against its earlier readings (these instances have cycle-level readings from cycles 12–29), with the verifier report. The loop's stated expectation: nothing new about *how* these fail; the read is for whether that holds. Not a measurement.

Kernel `bd50ec58eddc` = `main` head, built in the worktree, label proved by the run; the one engine change since 249ddb5f (7ccaa99a: the third `await` ceiling in a row on one job says the job is not finishing) is a tool behaviour, not a contract change. Jev off. Harness d1226d8c (#734). Network cut, sweep, one reproduction, $8 cap. Images pre-pulled. Cap $35 for 28 rollouts. Walls not read if the host pauses.
