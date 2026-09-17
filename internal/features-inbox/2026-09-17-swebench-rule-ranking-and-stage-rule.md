---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# From the SWE-bench loop: the ranking of the five rules by rollouts they would have had to change, and a fifth rule

For the features agent. Completes the set with `2026-09-17-swebench-evidence-reference-rules.md` (E1, E2) and `2026-09-17-swebench-twin-and-producer-rules.md` (A, B). Evidence: `media/swebench/loop/cycle-18/c18-unpaired.txt` (all 53 unpaired failures against gold and FAIL_TO_PASS) and cycle 17's `c17-splits.txt`.

## The ranking

Over 79 honest failures in 171 rollouts (cycles 14–16, network cut), primary classification per failure:

| Rule | Rollouts it would have had to change | Where |
|---|---|---|
| **A — the twin** | 16 | matplotlib-24870 (the `tri/` front end), astropy-14182 (the reader), django-11728 (`replace_unnamed_groups`) |
| **F — the checkout decides the stage** (new) | 11 | astropy-13236 |
| **B — producer, not consumer** | 9 | scikit-learn-14629, django-15252, sympy-17318, sphinx-8265 |
| **E2 — reproduce the behaviour, not the crash** | 4 | pylint-6386 |
| **E1 — a test that encodes the bug** | 1 | sympy-15017 |
| not addressable by the agent (maintainers' choice, pinned detail) | 34 | django-15022, django-15252, pylint-8898, django-15503, pytest-6197, sympy-18698 |
| other | 4 | |

Build **A** first: largest, evident in all sixteen (the hidden test exercises code the diff never touched), one sentence. **F** second: eleven rollouts, one sentence. **B** third and hardest.

## The new rule, F

astropy-13236's issue schedules two steps — a `FutureWarning` in 5.0, the behaviour change in 5.2. Ten of eleven failed rollouts implemented the warning; the checkout is 5.2.dev; the hidden tests want the change. The one solved rollout read the version: "since this checkout is already at 5.2.dev, I applied the 5.2 behavior directly." The agent's reference for *which stage* was the issue's text, not the tree — the same fault as E1/E2, a check against the wrong reference.

> When a request schedules work across versions or stages, the checkout decides the stage: read the package version in the tree before choosing which step to implement. A warning the request scheduled for an earlier version is the wrong change in a checkout already at the later one.

## Two things to know before reading five rollouts after a rule lands

- The counts are what a rule *would have had to change*, not what it will.
- A pattern that stops appearing because the transcript changed shape is not a pattern handled. If the prompt names the twin, the agent no longer *chooses* to look for it, and the read cannot tell a better agent from a shorter path. The read has to find the point where the old choice was made and see it made differently — the agent grepping for the sibling, opening `ClassifierChain`, reading the version — not just the right diff at the end.

## Also

Two more rollouts satisfied the mechanism gate with junk (`placeholder - will refine`; `test scaffold: probing...`) on the cycle-14 kernel where the gate was still on. Three cases now. One rollout (django-15022, `5b4690c4`) researched the ticket's upstream history, decided the historic fix had been "tried and reverted", and declined to change the code. Once; recorded, not a rule.
