---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# SWE-bench loop, cycle 38: pre-registration (written before the run)

Written 2026-09-19 03:20 UTC, before the run.

## Two reads, concurrent, one kernel

Kernel `1ec3de643d79` = `main` head, built in the worktree, label proved by the run. The engine change since e5e66f71 is bcf11b67: a command that runs a server is refused as a reproduction with the reason, and `changes`' re-run pass keeps to a 300 s budget, listing what it does not reach as "not re-run" and never saying "all pass now" on their account. Jev off. Network cut, sweep, one reproduction, $8 cap.

**Read 1 — the step where cycle 36 found the gap: three rollouts on django-13809.** Recorded per rollout: the longest `changes` duration; whether a `runserver` command run with `repro:true` is refused with the server reason (visible in the tool output); how many reproductions the gate records; whether the agent, refused, runs a request against the server as the reproduction instead. What would count: no `changes` over ~330 s, and the refusal visible where a server is offered. What would not: a `changes` over 600 s, or a server recorded as a reproduction. Not a measurement of the solve. Cap $12.

**Read 2 — new material.** Next ten never-run instances (`fresh10j.txt`: 4 "<15 min", 5 "15 min–1 hour", 1 "1–4 hours") at `-r 2`, 20 rollouts, every failure read against the account. Images pre-pulled. Cap $22.
