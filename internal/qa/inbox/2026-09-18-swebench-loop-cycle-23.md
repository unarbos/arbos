---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 23 — cycle 22's failures read; "no change needed" is now a pattern

No PR, no spend. Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 23; data `media/swebench/loop/cycle-23/`.

The 51 failures of cycle 22 (control `4b833de9860f` 24, treatment `aaf3dbe98de7` 27): G 5, B 12, E2 2, E1 2, A 3, F 2, maintainers' choice 24, one provider stall (the model returned nothing for 13 minutes; kernel exit 2; counted as a failure and named). Everything fits. G — the agent decides no change is needed — has seven cases on four instances and is promoted to a pattern, with a rule proposed to the features agent (`internal/features-inbox/2026-09-18-swebench-no-change-rule-and-producer-scope.md`). Cumulative read: 219.

For QA specifically: django-13513 is an instance where four of four rollouts across both kernels found the issue's suggested code already in the tree and declined to change anything; the hidden test wants more than the suggestion. Under the five rules the treatment still made the consumer's fix in xarray-6938 and sympy-17318 — cases the producer rule's wording does not reach. The provider stall is worth a QA look: 13 minutes of silence from the endpoint, then a failed turn, with no retry visible to the harness beyond the kernel's two tries.
