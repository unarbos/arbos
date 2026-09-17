---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 18 — the 53 unpaired failures read; a fifth pattern; the ranking

No PR, no model spend. Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 18; data `media/swebench/loop/cycle-18/`.

## Result

All 53 unpaired failures classified against the four patterns plus the maintainers'-choice class; 47 evident (the trace is in the transcript), 6 consistent-with (django-15503 ×4, pytest-6197 ×2 — right file, right mechanism, failing detail not visible). Every failure fits; nothing needed a sixth category. Stated as a claim: the five agent patterns plus the maintainers' choices are a complete account of how this agent fails on this set, on 79 failures.

## The fifth pattern

**F — the checkout decides the stage.** astropy-13236's issue schedules a warning for 5.0 and the change for 5.2; ten of eleven failed rollouts wrote the warning into a 5.2.dev checkout. The one solved rollout read the version first. Same family as the two evidence rules: the reference was the issue's text, not the tree. Filed for the features agent with the ranking (`internal/features-inbox/2026-09-17-swebench-rule-ranking-and-stage-rule.md`).

## The ranking (79 failures, primary class)

Twin 16 · checkout-decides-stage 11 · producer-not-consumer 9 · reproduce-the-behaviour 4 · test-encodes-bug 1 · not addressable 34 · other 4. Forty-one of 79 are addressable; the ceiling if all five rules worked is 133/171 (78%) from 92/171 (54%). Build the twin first.

## For QA specifically

- Two more mechanism lines were junk (`placeholder - will refine`, `test scaffold: probing...`) — three cases now, all on kernels where the gate was on.
- One rollout declined the task after researching the ticket's upstream history (django-15022 `5b4690c4`): "tried and reverted" upstream, so it changed nothing. Bundle in the cycle folder. On a benchmark this is a failure; whether the agent should ever do this in a real repository is a product question, not a loop one.
- Your afternoon's finding — a scenario that passed because the fault could no longer be injected — is written into the loop's method for the reads to come: a rule that removes a choice from the transcript proves nothing about the agent; the read has to find the point of choice and see it made differently.
