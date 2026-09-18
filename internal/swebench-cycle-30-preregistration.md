---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# SWE-bench loop, cycle 30: pre-registration (written before the run)

Written 2026-09-18 12:45 UTC, before the run.

## What this cycle is

New material for the account. The next ten never-run instances in the loop's order (`fresh10b.txt`: 4 "<15 min", 5 "15 min–1 hour", 1 "1–4 hours") at `-r 2`, 20 rollouts. Every failure read against the account (A/B/E1/E2/F/G/C, grader artefact); anything outside it is the finding. Not a measurement; the solve count is a count.

Since the cycle-29 kernel, two contract commits changed *marks*, not asks: the producer rule's reply line is gone (the read call is the mark — cycles 25 and 27's finding); the quoted-reference rule's mark is now a test that asserts the quote rather than a reply line. Neither needs a re-read the loop can do (the producer read is done; no gradeable quoting-reference instance exists).

## Conditions

Kernel `a8678ac16636` = `main` head, built in the worktree, named by sha, read-only, label proved by the run. Network cut, sweep, one reproduction, $8 cap. Cap $22.
