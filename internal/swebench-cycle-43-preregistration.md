---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# SWE-bench loop, cycle 43: pre-registration (written before the run)

Written 2026-09-19 11:20 UTC, before the run.

## The new material: second draws on once-failed instances

The never-run pool is spent (cycle 42). The coordinator's word: second draws on the once-failed instances. The pool is every instance with a cut-era (cycle 12 onward) result that failed at least once and is not a grader artefact: 64 instances, 29 of them failed in every draw so far. It is ordered by fewest draws first (33 instances have exactly two), then the loop's stratified order, so "second draw" means what it says before it means "thirty-fourth". `second-draw-pool.json` and `second-draw-order.json` hold the pool and the order; `second10a.txt` the first ten (all from cycles 29–32, two draws each, 1 of 20 prior rollouts solved).

What is read: every failure against the account, with the verifier report — and, for each instance, whether the second pair fails the *same* way as the first (the same class, the same site) or differently. Same-way twice more is the strongest evidence the loop has that an instance's failure is a property of the instance and the account's class for it is right; a different way is the finding. Not a measurement; solve counts on a pool selected for failure will be low by construction and are not comparable to fresh draws.

Kernel `bec7284b8740` = `main` head, built in the worktree, label proved by the run; the engine changes since aa0f61da (checkpoint `add -A` retries, a read-only file refused with the way through) are not contract changes. Jev off. Harness d1226d8c (#734). Network cut, sweep, one reproduction, $8 cap. Images pre-pulled. Cap $22. Walls not read if the host pauses.
