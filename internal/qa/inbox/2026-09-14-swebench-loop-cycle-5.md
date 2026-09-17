---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 5 — the reproduction gate

PR [#186](https://github.com/unarbos/arbos/pull/186) (`cursor/swebench-loop-c5`, stacked on #179, base `main`). Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md). Bundles: `internal/qa/rollouts/swebench/loop-cycle-5/{A,B}-<instance>/`.

## What changed (attack these)

1. `bash repro:true` records the command and exit code to `.arbos/agents/<id>/repro.jsonl`; exit 0 → "not a reproduction". With `ARBOS_REPRO_REQUIRED=1` the first edit is refused until a failing one is on record; since `8182be2` the last unmarked failing bash command is taken instead of refusing. Probe: (a) a command that fails for the wrong reason (missing module, typo) gets recorded as "the reproduction" — the gate cannot tell a broken command from a bug reproduction; (b) a reproduction that only fails under `wait_ms` (a hang → killed → exit None) is recorded as failing and will "STILL FAIL" forever at `changes`; (c) `changes` re-runs every reproduction with a 180 s cap each — five recorded suites = 15 min inside one tool call; (d) reproductions with relative paths and `cwd` set on the bash call: the re-run uses the recorded cwd; (e) reset happens on every user message — a steer mid-task drops the record.
2. Measured (kernel `0e5d5ee`, before the refinement): slice 40 → 42; 50/50 recorded; 146 refusals / 50 rollouts; cost +53%; median calls 23 → 30.

## Loss classes on slice 5 (run A, 10 → run B, 8)

- wrong mechanism 4 → 2 (astropy-13033, sphinx-9602 persist; django-13297, sphinx-8035 flipped).
- wrong layer 3 → 1 (sympy-20428 persists — `densearith` vs `expressiondomain`; scikit-learn-14629, django-14011 flipped).
- partial-complete 3 → 3 (pytest-5840, django-11400, django-14376 — 7 calls, missed `client.py`).
- variance −2 (astropy-13977, sphinx-8265).

## Infra

VM froze again during an idle stretch (12:04–16:04 UTC); wall times in this cycle's traces are inflated for rollouts in flight then. Regression at `-r 2` reached only 14 of 40 rollouts before the cap.
