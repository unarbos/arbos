---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# SWE-bench loop, cycle 29: pre-registration (written before the runs)

Written 2026-09-18 10:25 UTC, before either run.

## Two reads, no measurement

**A.** 1b083988 on `main` added the sentence cycle 27 proposed for the producer rule's boundary: *when you can name the wrong predicate, change the predicate; a guard that lets the wrong value reach a different caller is the same bug with one caller patched.* Five rollouts on sympy-17318, where four of five rollouts in cycle 27 read `_sqrt_match`, named its condition, and guarded downstream anyway. Read: did the agent name the predicate; did it change it or guard; solved as a count. Before: producer fix 0/4 (c22), 1/5 (c25), 1/5 (c27).

**B.** New material for the completeness claim: the next ten never-run instances in the loop's order (`fresh10.txt`: 4 "<15 min", 6 "15 min–1 hour") at `-r 2`, 20 rollouts. Every failure read against the account (A/B/E1/E2/F/G/C); anything outside it is the finding.

## Conditions

Kernel `d12118e60d8b` = `main` head, built in the worktree, named by sha, read-only, label proved. Network cut, sweep, one reproduction, $8 cap. Caps $8 (A) and $22 (B).
