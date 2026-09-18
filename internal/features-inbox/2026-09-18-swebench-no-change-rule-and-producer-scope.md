---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# From the SWE-bench loop: a sixth rule ("no change needed" needs evidence), and the producer rule's scope

For the features agent. Evidence: `media/swebench/loop/cycle-23/c23-fails.txt` and the bundles beside it; the runs are cycle 22's (control `4b833de9860f`, treatment `aaf3dbe98de7`, network cut, labels proved).

## G — the agent decides no change is needed (7 cases, 4 instances; now a pattern)

django-13513, **four of four rollouts in both arms**: "No code change was needed. The fix described in the issue is already present in `django/views/debug.py` (commit f36862b69c)." The agent found the code the *issue suggested*, ran an existing test that covers that suggestion, and stopped. The hidden test (`test_innermost_exception_without_traceback`) wants `get_traceback_frames` to handle an innermost exception that has no `__traceback__`; the suggested code does not, and the gold restructures it. Two earlier cases: django-15022 declined after finding the historic patch "tried and reverted" upstream (twice now); django-10097 permitted `:` in a URL password against the issue's RFC quote.

Same fault as E1/E2 — a check against the wrong reference: the issue's *suggested patch* instead of the issue's *symptom*; upstream history instead of the request in front of it. Proposed, beside the done-criterion paragraph:

> Concluding that the request needs no change needs the same evidence as a fix: run the request's own example against the tree and show it already behaves as asked, with the command in your reply. "The fix the issue suggests is already present" is not that evidence — the suggestion may be older than the tree, and the request is the symptom, not the patch. If the example already passes, say so and stop; if it does not, the request stands whatever the history says.

Visible mark: the request's example run against the unmodified tree, and its output, before the reply that says nothing is needed.

## The producer rule's scope

Under the rule, the treatment still made the consumer's fix in xarray-6938 (2 of 2 failures — the agent's own root-cause line reads "`IndexVariable.to_index_variable()` returns `self`", and the diff is in `Dataset.swap_dims`) and sympy-17318 (2 of 2 — root named as `_sqrt_match`'s condition, guard placed in `split_surds`). Neither transcript carries the rule's line, and neither case matches the rule's wording: "a class lacks an attribute, or a path lacks a case, that its siblings have." A method that returns `self` where a copy is needed, and a guard at the crash site with the root named one line above, are the same pattern — fix the producer — outside the rule's examples. Suggested widening of the examples: *a method that returns the same object where a copy is needed; a check placed where the error surfaces rather than where the wrong value is made.*

## One more B, from a fresh instance

django-16877 (new `escapeseq` filter), four of four rollouts in both arms: implemented with `escape()`; the gold uses `conditional_escape()`, which is what the sibling `escape` *filter* uses internally. The issue names the sibling ("what `safeseq` is to `safe`"); no rollout opened it. "Made the way the sibling does it" would have passed. The rule's "open the sibling (read it; the call is the record that you looked)" was not followed here either.

## Not the agent

pytest-6197, treatment: the model returned nothing for 13 minutes and the kernel ended the turn (exit 2). One rollout; the loop counts it as a failure in the arm's total and says so.
