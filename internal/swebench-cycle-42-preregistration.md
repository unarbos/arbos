---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# SWE-bench loop, cycle 42: pre-registration (written before the run)

Written 2026-09-19 10:15 UTC, before the run.

## Two reads, concurrent, one kernel

Kernel `aa0f61da94a2` = `main` head, built in the worktree, label proved by the run. Engine changes since 18f397cb: **d8344ff2**, Features' step on the cycle-40 finding (read from this document; nothing was filed) — the no-change rule now says "already behaves as asked" means the asked output is present, not the reported output gone ("the symptom moved, not the fix present"); and 2ad607b8 (a UTF-8 BOM is the file's, not line 1's). Jev off. Harness d1226d8c (#734). Network cut, sweep, one reproduction, $8 cap.

**Read 1 — d8344ff2 where cycle 40 found the gap: three rollouts on sympy-23950.** The checkout's `Contains.as_set()` raises `NotImplementedError` where the issue reports it returning `Contains`; cycle 40's two rollouts both declared "no change needed" with a 0-byte patch. Recorded per rollout: whether the agent declares no change; if so, whether its "no change:" line claims the asked output (a set) or only the reported output's absence; whether it edits `as_set` to return the set; patch size. What would count: no "no change" verdict, or a verdict that names the asked output and is therefore not reached; a patch that returns the set. What would not: a no-change verdict on "the reported output is gone" again. Cap $8.

**Read 2 — the last never-run instances.** Eight remain in the loop's order (`fresh10n.txt`: 3 "<15 min", 4 "15 min–1 hour", 1 "1–4 hours") at `-r 2`, 16 rollouts, every failure read against the account with the verifier report. After this cycle every instance in the order has been drawn at least once; what "fresh material" means next is the coordinator's call, and the cycle will say so. Images pre-pulled. Cap $20. Walls not read if the host pauses.
