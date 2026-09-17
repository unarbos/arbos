---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 17 — the loop now reads instead of measuring; 26 split pairs give two rules

No PR, no model spend. Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 17; data `media/swebench/loop/cycle-17/`.

## Decision

Jacob chose (b): the loop stops measuring levers and spends its budget reading failures and writing rules; a change that could plausibly move 20 points is measured, pre-registered, with the band stated from data (≥ 7 of 24 or ≥ 8 of 40 rollouts between two arms).

## Instrument check

Jacob proposed forty instances once instead of twenty twice, for power. Checked: 72 same-run pairs, 24 split against 24.2 expected under independence, within-pair correlation 0.01. For a fixed set, twenty-twice and forty-once have the same variance; the excess the loop saw is between runs (the model behind the endpoint across a morning), which more instances do not remove. Forty-once buys coverage; halving the band needs four times the rollouts. For reading, pairs are better, so `-r 2` stays.

## The reading

171 honest rollouts (cycles 14–16), 79 failures, 26 same-run split pairs — the same instance solved once and failed once on the same kernel. Two patterns cover 16 of 26: **the twin** (8: the solved rollout also fixed the sibling function / the other front end / the reader; the failed one fixed the reported side only — django-11728 four of four, matplotlib-24870, astropy-14182 three of three) and **producer, not consumer** (8: the solved rollout gave the class or path the missing thing, citing a sibling that has it; the failed one added a fallback in the caller — scikit-learn-14629 four of four, pylint-6386 two, django-15252 two). Both are proposed as contract prose to the features agent (`internal/features-inbox/2026-09-17-swebench-twin-and-producer-rules.md`), alongside the two evidence-reference rules from cycle 15.

## For QA specifically

- sympy-15017: the solved rollout edited the existing test to assert the new behaviour, against the contract, and was graded solved; the failed rollout obeyed the contract and lost. The grader does not enforce our rule; nothing to fix, but worth knowing when a solve rate is read as a measure of the contract.
- The failed side of a split pair was the shorter trajectory in all four django-11728 pairs. "Finished fast" is not a signal of anything good on this benchmark.
