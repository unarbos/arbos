---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# SWE-bench loop, cycle 12: pre-registration (written before the run)

Written 2026-09-17 03:06 UTC, before the run started.

## What this cycle is

A re-baseline, not a comparison. One arm, no lever.

- Kernel: `main` `fa17987e`, built static (`arbos-kernel-c12`).
- Harness: one reproduction, mechanism gate on, $8 per-rollout cap, 2400 s timeout, Sonnet 5 via OpenRouter, concurrency 3.
- Runtime: docker with `--env.agent.runtime.block '["*"]'` — the container reaches only the interception proxy. The harness now refuses to run without this (commit `95ca8f8d`).
- Set: `reg20` at `-r 2` = 40 rollouts. Cap $55, watcher stops at $50 recorded.

## What is decided in advance

- The graded rate on the 40 rollouts (or on however many the cap allows, stated as such) **is** the loop's baseline from here on. It is not adjusted, weighted, or compared favourably with the old 74%. The old figure appears only as the number that was wrong.
- If the cap stops the run before 40, the baseline is reported on the rollouts done, with the instances not reached listed; a second run finishes them in the next cycle before any lever is compared.
- Soundness checks that must all hold for the number to stand: `arbos_egress_open` = 0.0 on every rollout; a transcript audit (`net_audit.py`) finds zero package fetches; no rollout errored at setup.
- Also recorded, not part of the number: per-instance results, cost per rollout, capped rollouts, and how the six instances that used to fetch upstream (astropy-13398, django-13449, django-14792, django-15022, django-15252, pylint-8898) fare without the shortcut.
