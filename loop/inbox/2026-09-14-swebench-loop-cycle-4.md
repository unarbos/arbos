---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 4 — the `mechanism` gate (recorded; refusal opt-in)

PR [#179](https://github.com/unarbos/arbos/pull/179) (`cursor/swebench-loop-c4`, stacked on #142, base `main`). Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md). Bundles: `internal/qa/rollouts/swebench/loop-cycle-4/{A,B}-<instance>/`.

## What changed (attack these)

1. `edit`, `write`, `apply_patch` take `mechanism` (one line). The first call after a user message records it to `.arbos/agents/<id>/mechanism.md`, echoes `Mechanism recorded for this task: …`, and `changes` prints it. With `ARBOS_MECHANISM_REQUIRED=1` (the harness sets it) the first edit without it, or with < 24 chars, is refused. Probe: (a) desktop default (variable unset): no refusal ever, even for a bare `write` of a new doc; (b) `--steer` mid-turn and a second user message: the line resets on every user wake — a follow-up message in the same task counts as a new task; (c) `apply_patch` as the first write with a 23-char line → refused; (d) child agents: each has its own `mechanism.md`; (e) the replay provider with recorded `edit` calls that lack the argument under `ARBOS_MECHANISM_REQUIRED=1` → refusals change the replay.
2. Measured: refusal on → 50/50 rollouts stated a mechanism (20 refused once), score 39 → 38, cost +18%, median calls 22 → 26.5. The stated mechanism is the wrong one the agent already believed. Do not expect the gate to change outcomes; it is a record.

## Loss classes on slice 4 (run A, 11)

- **wrong mechanism** 8 (django-16950, 14140, sympy-13974, 13798, astropy-14598, 14369, requests-5414, sympy-18698): gold's file, reporter's example passes, hidden test's second consequence missed.
- **partial-complete** 3 (django-13212 missed `forms/fields.py`; sympy-16597 missed 5 files; django-10999: 5 tool calls, regex lookahead, stopped).
- wrong layer 0, scope drift 0, test editing 0.

## Regression 20 at `-r 2` (29 of 40 rollouts before the cap)

21/29 solved. django-14792, 15022, pylint-8898, astropy-13398 split 1/1 each — coin flips, not regressions. Any single-rollout ±2 on this set is noise.

## Infra

Disk filled at 254 GB (183 SWE-bench images); the first launch of both runs errored (`No space left on device` in the grader and in the harness's artifact collection). `cycle_run.sh` now prunes images not in the current slice or regression set when under 40 GB free. Also: `pkill -f "swebench-verified"` matched my own shell's command line — match on the binary path.
