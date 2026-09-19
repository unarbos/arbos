---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# SWE-bench loop, cycle 37: pre-registration (written before the run)

Written 2026-09-19 01:50 UTC, before the run.

New material for the account: the next ten never-run instances (`fresh10i.txt`: 4 "<15 min", 5 "15 min–1 hour", 1 "1–4 hours") at `-r 2`, 20 rollouts, every failure read against the account. Not a measurement.

Kernel `e5e66f71c418` = `main` head, built in the worktree, label proved by the run. The one engine change since a8678ac1 is 180f1644 (the sleep-while-workers-run guard counts a literal for-loop's passes) — a coordinator-with-workers case that a root agent without workers does not reach, so no behaviour change is expected under this harness. Jev off. Network cut, sweep, one reproduction, $8 cap. Images pre-pulled. Cap $22.
