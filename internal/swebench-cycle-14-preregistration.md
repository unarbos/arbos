---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# SWE-bench loop, cycle 14: pre-registration (written before the run)

Written 2026-09-17 06:50 UTC, before the run started.

## What this cycle is

The baseline of the loop's new instrument, **regression 20b**. One arm, no lever. Approved by Jacob after cycle 13's decomposition of the old set (30 of 40 rollouts decided before the run).

## The set (`reg20b.txt`)

- 12 instances with a clean 1/2 split in cycles 1–5 and never fetched: astropy-13236, astropy-14182, django-11728, django-16454, matplotlib-24870, pylint-6386, scikit-learn-14629, sphinx-8035, sphinx-8265, sympy-15017, sympy-17318, sympy-18698.
- 2 with variance in the old set: pylint-8898 (clean 6/10), scikit-learn-25102 (clean 3/5).
- 2 near-floor instances the agent has solved cleanly at least once: django-15022 (2/19), django-15252 (1/7).
- 4 never run, taken in order from `order_remaining`: scikit-learn-10908, django-13033, sympy-13615 (all "15 min – 1 hour"), pytest-6197 ("1–4 hours").

Selection on past variance means the first 14 are expected to land near 50%; that is the point — an instrument that can move both ways.

## Conditions

- Kernel: `main` `30eef166` (#393 merged; kernel unchanged since #380's `864d6b00` except by merges of #390 and #393, which are rewind and harness work). Built static as `arbos-kernel-c14`.
- Harness: branch `cursor/swebench-sweep-zombies-7c9c` (#397: #393 plus zombie separation); refusal on open egress, sweep before grading. One reproduction, mechanism gate on, $8 cap, 2400 s, Sonnet 5, concurrency 3, `--env.agent.runtime.block '["*"]'`.
- `-r 2` = 40 rollouts. Cap $45, watcher at $42.

## Decided in advance

- The graded rate on the 40 rollouts is regression 20b's baseline, unadjusted. If the cap stops the run short, the rate is stated on the rollouts done and the instances not reached are listed and finished next cycle before any lever is compared.
- Soundness checks that must hold: `arbos_egress_open` 0.0 on every rollout; transcript audit finds no fetch; `left` = 0 in every sweep; no setup errors.
- Also recorded, not part of the number: per-instance results, cost, capped rollouts, and — for choosing cycle 15's lever — a classification of every failure against the gold patch and FAIL_TO_PASS list, using cycle 13's categories (wrong layer; conservatism vs a maintainers' behaviour change; hidden test grades what the issue does not determine; and anything new). "Wrong mechanism" is not assumed; if it appears it has to be shown.
- The old regression 20 is not run this cycle. It runs once per kernel base as a sanity check (the ten pass, the five fail); the next kernel base change triggers it.
