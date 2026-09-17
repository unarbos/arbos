---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 3 — per-function coverage hook; prose pre-edit rules do not work

PR [#142](https://github.com/unarbos/arbos/pull/142) (`cursor/swebench-loop-c3`, stacked on #139, base `main`). Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md). Bundles: `internal/qa/rollouts/swebench/loop-cycle-3/{A,B,REG}-<instance>/`.

## What changed

- `[hook] Tests covering this edit` now resolves every changed line (`git diff -U0`) to its enclosing function and class separately, reports per function, and says `class-level match only (Class named in …), no test names <method>` when only the class is matched. Probe: an edit between two methods at class level (attribute assignment) → "class-level change (…), no changed function to check"; a Rust `impl` block; a file whose changed hunk has no def above it (module-level code) → nothing reported for that hunk; a very long file (the resolver walks backwards line by line per hunk).
- Tried and dropped: a prose rule to state the mechanism before the first edit. Measured: 3 of 50 rollouts did it before editing; 47 put a "Mechanism:" paragraph in the final summary. **Any "before you do X" prose rule should be assumed unfollowed until a transcript count says otherwise**; the pre-edit grep from cycle 2 is followed in about half the rollouts.

## Loss classes on slice 3 (run A, 11 losses; run B, 12)

- **wrong mechanism** 6 → 6 (all persist): django-13794, django-15563, sphinx-7748, sympy-18199, matplotlib-26466, requests-2931. Pattern: the fix is in gold's file and makes the reporter's example pass; the hidden test exercises a second consequence the agent did not derive. Reading these six is the cycle-4 homework.
- **partial-complete** 3 → 3: django-12406 (missed `related.py`), django-16256 (missed `contenttypes/fields.py`; also left a temp test file `tests/many_to_one/test_related_async_tmp.py` in the tree), django-13512 (missed `admin/utils.py`).
- **scope drift** 2 → 1: sphinx-10614 (graphviz + inheritance_diagram), sympy-15017 (fixed sparse arrays too — flipped to solved in B).
- **wrong layer** 0.

## Regression 20

15 → 13 with the dropped rule in. The four losses (django-14792, 15022, 15252, pylint-8898) have flipped back and forth across cycles; at n=1 the regression set is too noisy to attribute ±2. Cycle 4 runs it at `-r 2`.
