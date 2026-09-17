---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# SWE-bench loop, cycle 20: pre-registration (written before the run)

Written 2026-09-17 19:35 UTC, before the run.

## What this cycle is

A read, not a measurement. The remaining three rules are in the contract on `main`: A the twin (0245425e), F the checkout decides the stage (0245425e), B producer-not-consumer (7bf12e68). Each was written to leave a visible mark: a grep described "Look for the twin of <fn>" and a reply line "twin: none found (grepped <pattern>)" or "twin: <file>:<fn>, fixed in the same change"; a version read with bash and "checkout is at <version>, so I implemented <step>"; a read of the sibling and "producer: <Class>.<attr> added the way <Sibling> has it" or "fallback in <caller>: <why>".

## Conditions

Kernel `2c8d8791f6af` = `main` head (#477 in, so the binary proves its label and the run refuses otherwise), built in the loop's worktree, named by its sha, read-only. Network cut, sweep, one reproduction, $8 cap. Five rollouts each on the carrying instances: django-11728, matplotlib-24870, astropy-14182 (A); astropy-13236 (F); scikit-learn-14629 (B). 25 rollouts, cap $30. Solve counts reported, not the finding.

## What is read, decided in advance

1. **Could the old choice still have been made?** For A: did the agent fix the reported side and reach the point where the twin was or was not looked for — i.e. is there a moment before the final reply where the sibling was not yet touched? For F: did the agent read the issue's two stages and choose one? For B: did the agent see that `MultiOutputClassifier` lacks `classes_` and choose where to put the fix? A rollout that never reached the point says nothing.
2. **Was the step taken, not just the line written?** The mark is two-part by design. "twin: none found" without a grep in the transcript, "checkout is at 5.2.dev" without a version read, "producer: ... the way ClassifierChain has it" without ClassifierChain opened — each is the line without the step, and is recorded as such, separately. This is the QA caution in its sharpest form: a rule that teaches the agent to say the words has changed the transcript's shape and nothing else.
3. At the point of choice, what was chosen: the twin fixed / not; the 5.2 step / the 5.0 warning; the class / the caller.
4. Anything a third time: the "argues with the request" candidate becomes a pattern on a third case.
