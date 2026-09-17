---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 7 — two reproductions before the first edit

PR [#314](https://github.com/unarbos/arbos/pull/314) (`cursor/swebench-loop-c7`, base `main`). Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md). Bundles: `internal/qa/rollouts/swebench/loop-cycle-7/{A,B}-<instance>/`.

## What changed

`ARBOS_REPRO_REQUIRED=N` (repro.rs): the first edit needs N distinct failing reproductions; "distinct" = different command text after whitespace normalisation. Probe: (a) two commands that differ only by a comment or an extra flag count as two — a cheap way for the agent to satisfy N=2 without a new input; consider comparing the command with comments/flags stripped, or requiring different *inputs* (hard to define); (b) N=2 with the last-failing auto-take: the auto-take supplies at most one, so the refusal loop for the second is 1.8 per rollout; (c) `changes` re-runs both; a second reproduction that fails for an unrelated reason (a typo) will STILL FAIL forever and the agent may loop on it.

## Numbers

Regression `-r 2` at N=2: 28/32 rollouts (floor 28/40; like-for-like 23/34 → 28/32); the four coin-flip instances 2/2. Kernel base moved too (`43d8569` → `c964294`), so cycle 8 attributes N=2 with N=1 vs N=2 on one kernel. Slice subset: −1 of 13 (noise). Losses in both arms: django-11532, django-13195 (wrong mechanism).

## Infra

Cap watcher: when it fired for the regression it removed every container on the host and the slice eval died the same minute (its rollouts saw the interception server vanish → kernel exit 2). Fixed to SIGINT only. Also: a per-batch cap check lets a batch start at $13.38 against $14 and finish at $19 — check per rollout or size the last batch.
