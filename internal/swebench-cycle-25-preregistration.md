---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# SWE-bench loop, cycle 25: pre-registration (written before the run)

Written 2026-09-18 03:40 UTC, before the run.

## What this cycle is

A read, not a measurement. [#541](https://github.com/unarbos/arbos/pull/541) landed the G rule ("no change needed" takes a recorded run of the request's example against the unmodified tree, and a reply line "no change: <command> already …") and widened the producer rule to the method that returns the same object and the guard placed where the error surfaces, with the mark: "the line in your own reply that names the root cause names where the fix goes". 77b1feaa made an edit through the shell an edit on the tool event, and named the generated file in the twin rule.

## Conditions

Kernel `206d617e9a50` = `main` head (all of the above in), built in the loop's worktree, named by sha, read-only, label proved by the run. Network cut, sweep, one reproduction, $8 cap. Five rollouts each on django-13513 (G: four of four declined in cycle 22), xarray-6938 (B scope: a method returning `self`; 0/3 in cycle 22, all fixed in the caller), sympy-17318 (B scope: a guard at the crash site; 0/4 in cycle 22). 15 rollouts, cap $20. Solve counts reported, not the finding.

## Read, decided in advance

1. Could the old choice still have been made? 13513: did the agent find the suggested code already present and reach the point of deciding? 6938: did it name `to_index_variable` returning `self` and choose where to fix? 17318: did it name `_sqrt_match`'s condition and choose between the condition and the guard?
2. Was the step taken — the request's example run against the unmodified tree (`repro:true`) before any "no change" conclusion; the root-cause line's location matching the diff — not just the line written?
3. What was chosen. Every failure read against the account; an edit made through the shell now appears on the tool event, so "unreadable" should not recur.
