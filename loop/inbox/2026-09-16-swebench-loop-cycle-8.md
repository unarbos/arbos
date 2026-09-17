---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 8 — two reproductions is now the harness default

PR [#314](https://github.com/unarbos/arbos/pull/314) (`cursor/swebench-loop-c7`, base `main`; `3608f49` sets `repro_required = 2` in the harness and `ARBOS_REPRO_REQUIRED` default 2 in `arbos-swe-run`). Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md).

## Attribution

Same kernel, regression 20 at `-r 2`: N=1 23/32 rollouts, N=2 15/18; like-for-like 13/18 → 15/18; pooled over cycles 6–8 on the same instances N=1 45/64 (70%) vs N=2 42/49 (86%). The kernel base change moved one rollout. Two reproductions before the first edit is the first lever with a repeatable effect on the regression set.

## What to probe

1. Cost: one N=2 rollout (django-15252) ran to $14.46 — 8× the median. Find what it did (bundle in `internal/qa/rollouts/swebench/loop-cycle-7/` if it recurs; this one's trace is in `media/swebench/loop/cycle-8/traces-c8-reg-n2.jsonl`). Candidate: repeated `changes` re-runs of two slow reproductions (180 s each) inside a long turn.
2. "Distinct" is command-text after whitespace normalisation: two commands differing by a comment count as two. Watch for agents gaming it.
3. Desktop is untouched: the kernel default is no gate. Only headless runs get N=2.

## Infra

Cap watcher now SIGINTs only the run it watches; verifiers cleans its own containers. Bundles are tarred in `/tmp` and copied, after the store returned EIO on a streamed write.
