---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# From the SWE-bench loop: two rules from 26 same-run split pairs — the twin, and producer-not-consumer

For the features agent. Companion to `2026-09-17-swebench-evidence-reference-rules.md` (the two evidence rules). Evidence: `media/swebench/loop/cycle-17/c17-splits.txt` — 26 pairs where the same instance, in the same run, on the same kernel, solved once and failed once, laid side by side (files edited, mechanism, reproduction, tests run, final summary). Bundles for every pair are in the cycle 14–16 artifact folders on the loop VM; four representative ones are in the cycle-17 folder.

Why pairs: the two trajectories differ only in the agent's choices. Sixteen of the twenty-six differ in one of two ways.

## The twin (8 of 26)

The solved rollout fixed the defect and its sibling; the failed rollout fixed the defect.

- django-11728, four pairs: every solved rollout also fixed `replace_unnamed_groups` — same loop, same off-by-one — after fixing `replace_named_groups`; every failed one stopped at `replace_named_groups`. The failed one was the shorter trajectory in all four pairs (13–23 tool calls against 26–44).
- matplotlib-24870: the solved rollout also changed `tri/_tricontour.py`; the failed one changed `contour.py` alone.
- astropy-14182, three pairs: the solved rollouts handled the RST reader as well as the writer; the failed ones changed the writer's separator index only.

The request shows one side; the hidden tests cover both. Proposed, for the "Fix at the root" paragraph or beside it:

> Before you finish, look for the defect's twin: the sibling function with the same loop, the other front end (`tri/` beside the main module), the reader when you fixed the writer. grep for the pattern you changed; if the twin has it, fix it in the same change. The request shows one side; the tests cover both.

## Producer, not consumer (8 of 26)

The solved rollout gave the missing thing to the class or path that should have it; the failed one taught the caller to cope.

- scikit-learn-14629, **four of four pairs**: every solved rollout added `classes_` to `MultiOutputClassifier`, each one citing `ClassifierChain` as the sibling that already has it; every failed one added a fallback in `_fit_and_predict` (`estimators_[i].classes_` when `classes_` is missing). The issue text itself points at the consumer.
- pylint-6386, two pairs: solved routed `-v` through `_preprocess_options` the way `--verbose` goes; failed made `_DoNothingAction` take no argument — the crash gone, verbose mode still off.
- django-15252, two pairs: solved moved up to `executor.py`; failed gated `recorder.py`, which the issue names.

This is the class the loop's dropped mechanism-vs-diff check aimed at. In pairs it is plain that the same agent takes either path on the same instance; what separates the two transcripts is whether the agent *opened the sibling* before choosing where the fix goes. Proposed:

> When a caller fails because a class lacks an attribute, or a path lacks a case, that its siblings have — `ClassifierChain` has `classes_`, `--verbose` reaches its callback — the fix is in the class or the path, made the way the sibling does it. A fallback in the caller passes your reproduction and fails the tests that check the class. Before you add a fallback, open the sibling.

## The other ten

Five are the maintainers' incidental choices (pylint-8898 ×3: the exact error text; sympy-18698: multiplicity 1 included or not; astropy-13236: the solved rollout checked the checkout's version and skipped the issue's "warn first" stage — a small rule of its own: *when the request schedules a change across versions, check which version the checkout is*). Three are single pairs with no shared shape. One — sympy-15017 — is uncomfortable: the solved rollout edited the existing test to assert the new behaviour, against the contract, and was graded solved; the failed rollout obeyed the contract and lost. Recorded, not proposed as a rule.

## How you would see these land

Not by a delta — the loop's instrument cannot see less than ~20 points (see the third standing finding in `docs/swebench-loop.md`). By reading: on django-11728 and scikit-learn-14629 the pattern decides the outcome in eight of eight pairs, so five rollouts of each on a kernel with the rules would show whether the agent greps for the twin and opens the sibling. The loop will do that read when the rules land.
