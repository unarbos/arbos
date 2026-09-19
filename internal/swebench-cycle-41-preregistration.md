---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# SWE-bench loop, cycle 41: pre-registration (written before the run)

Written 2026-09-19 08:45 UTC, before the run.

## Two reads, concurrent, one kernel

Kernel `18f397cb34ab` = `main` head, built in the worktree, label proved by the run. The engine changes since 3784c20d: **42753ae5** — Features' step on the cycle-39 finding (read from this document; nothing was filed): a reproduction whose command moves the working tree, index or HEAD (`git stash/checkout/reset/…` in any segment) is refused with what it would do to the fix, never taken as the last failing command, never re-run, and a re-run that moved the tree anyway is said under its verdict; plus non-UTF-8 files read with a note and refused for edit with what to do (d148561c, 8798b77a), and `read` streams pages with a cap (19c00801). Jev off. Harness d1226d8c (#734). Network cut, sweep, one reproduction, $8 cap.

**Read 1 — 42753ae5 where cycle 39 found the gap: three rollouts on pytest-10356.** Recorded per rollout: whether a tree-moving command offered with `repro:true` is refused with the reason (visible in the tool output); what the gate records instead; the patch size at exit; whether any `changes` output carries a "CHANGED THE WORKING TREE" line; whether the tree at exit matches what the last `changes` showed. What would count: no tree-moving reproduction recorded, patch at exit non-empty when `changes` showed a fix. What would not: a `git stash` reproduction recorded, or an empty patch after a shown fix. Not a measurement of the solve. Cap $10.

**Read 2 — new material.** Next ten never-run instances (`fresh10m.txt`: 4 "<15 min", 6 "15 min–1 hour") at `-r 2`, 20 rollouts, every failure read against the account with the verifier report. Images pre-pulled. Cap $22. Walls not read if the host pauses.
