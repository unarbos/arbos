---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# SWE-bench loop, cycle 19: pre-registration (written before the run)

Written 2026-09-17 18:30 UTC, before the run.

## What this cycle is

A read, not a measurement. #449 landed two of the five rules in the contract (E1: an assertion computed from the same wrong value encodes the bug too; E2: a reproduction asserts the behaviour, not the absence of the error, and the reference is the one the request names, never your own previous run). Per cycle 18's plan: five rollouts on each carrying instance — sympy-15017 (E1) and pylint-6386 (E2) — read for whether the agent reaches the point where the old choice was made and makes it differently.

## Conditions

Kernel `3f114bf50447` = `main` `2f9a0427` (#449 in, #440 in) plus #477's build plumbing only; the binary reports that sha and the run refuses otherwise (`--env.agent.harness.kernel-sha 3f114bf50447`). Network cut, sweep, one reproduction, $8 cap. `-r 5`, ten rollouts, cap $12. No control arm: the comparison is with the cycle-14–16 transcripts of the same instances (12 rollouts each), which are the failures the rules were written from.

## What is read, decided in advance

- **Could the pattern still show itself?** (QA's caution.) For 15017: did the agent reach the root fix (`_loop_size`), run the existing suite, and meet the second assertion (`raises(ValueError, lambda: rank_zero_array[0])`)? If it never reached that point the rollout says nothing about E1. For 6386: did the agent reach a working `-v` and have `--verbose` available as the reference? If its reproduction never got past the crash, the rollout says nothing about E2.
- **At that point, what did it do?** 15017: keep the root fix and name the second assertion as encoding the bug (rule followed), or retreat to a special case (old behaviour). 6386: assert the behaviour (`Using config file`, or a comparison with `--verbose`) in the reproduction or the check (rule followed), or stop at "no longer errors" (old behaviour).
- Solve counts are reported but are not the finding; on five rollouts they cannot be.
- Anything else the rules changed in the transcripts' shape (length, where the reproduction is written, what the summary claims) is recorded as seen.
