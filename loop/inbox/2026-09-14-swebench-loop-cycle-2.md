---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 2 — the coverage hook and the new loss classes

PR [#139](https://github.com/unarbos/arbos/pull/139) (`cursor/swebench-loop-c2-main`, base `main`). Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md). Bundles: `internal/qa/rollouts/swebench/loop-cycle-2/{A,B,REG}-<instance>/`.

## What changed (attack these)

1. **`[hook] Tests covering this edit` after every source edit** (`tools/git.rs::coverage_note`, `batch.rs`). Probe: (a) a repo with no git → no line, no error; (b) an edit that touches only a class body → the line names the class, which is "covered" by every test that imports it (weak signal; matplotlib-23476, sympy-21930 slipped through this way); (c) a huge repo — `git grep` over `**/tests/**` per edit; time it; (d) test-tree layouts the globs miss (`t/`, `Tests/`, `*_test.py` at top level is covered, `spec/` is covered); (e) `apply_patch` touching 20 files → 20 clauses joined with ` | ` in one line — is that readable in the desktop?
2. **Concrete pre-edit grep in the CONTRACT.** Check the agent actually runs it (transcripts show `grep -rlw` before the first edit in ~half the run-B rollouts) and whether it slows easy instances (median calls 25→27).

## Loss classes on slice 2 (run A, 13 losses; after the fix, 9)

- **wrong layer** 4 → 2: fixed pylint-6386, sphinx-9461; persisting matplotlib-23476 (`backend_bases` vs `figure.__setstate__`), sympy-21930 (generic `_print_Pow` vs `secondquant._latex`).
- **partial-complete** 4 → 2: django-15916 (metaclass path), sphinx-7462 (stopped after 10 calls, missed `pycode/ast.py`); fixed sympy-17318, astropy-13236.
- **wrong mechanism** (new: right file, wrong fix) 3 → 3: matplotlib-26208 (`relim` vs the units handler), django-12273, matplotlib-24870 (B only).
- **scope drift** 1: sphinx-11510 (9 files vs gold's 1, incl. a new `events.py` hook).
- **test editing, fixture files** 1: django-10097 rewrote `tests/validators/valid_urls.txt` / `invalid_urls.txt`. The read-only rule names assertions, tolerances, expected values, fixtures; `.txt` data under `tests/` did not register as a fixture. Candidate: `is_test_path` already flags it in `changes`; the rule text should say "any file under the test tree".

## Regression 20

15 → 15. django-15022 now passes (cycle-1 "never veto" wording verified), django-15252 passes. astropy-13398 relapsed as partial (no tolerance edit — the read-only rule held), scikit-learn-25102 relapsed with a different mechanism. Codex-baseline set 21/24 vs 22.

## Infra

The VM freezes during idle waits (4.2 h lost on 2026-09-13); wall-time metrics from frozen periods are wrong. `#131` (environment allowlist) does not break the harness: `run` spawns the kernel with the full env; jobs get PATH/HOME and the login shell still finds conda.
