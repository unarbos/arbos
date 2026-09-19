---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# SWE-bench loop, cycle 39: pre-registration (written before the fresh run)

Written 2026-09-19 05:50 UTC. The harness change and its check on requests-6028 (two rollouts, $4.44) ran first and are recorded in the cycle; this pre-registers the fresh read that follows.

New material for the account: the next ten never-run instances (`fresh10k.txt`: 4 "<15 min", 5 "15 min–1 hour", 1 "1–4 hours") at `-r 2`, 20 rollouts, every failure read against the account. Not a measurement.

Kernel `04345f70e141` = `main` head, built in the worktree, label proved by the run. Engine changes since 1ec3de64: grep skips the repository's own `.git/` (d3b373a6), a record append that fails part-way is cut back to the last whole line (64935c0b), an unwritable config folder no longer kills the kernel (4e186020). None is a contract change; grep's is the only one an agent sees. Jev off. Harness at d1226d8c (keeps the in-run verifier log). Network cut, sweep, one reproduction, $8 cap. Images pre-pulled. Cap $22. Walls are not read if the host pauses.
