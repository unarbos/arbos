---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# SWE-bench loop, cycle 15: pre-registration (written before the runs)

Written 2026-09-17 09:40 UTC, before either arm started.

## The lever's own ceiling, first

The lever aims at one pattern: the agent names the root cause and edits somewhere else (a guard at the crash site, a fallback in a consumer). On regression 20b that pattern is **3 of the 12 failures** (sympy-17318 ×2, scikit-learn-14629); on the old set it was 2 of 12 (django-14792). So on this set the most the lever can move, if it converts every rollout in its class and loses none, is **+3**. The adopt threshold below is +3. A drop therefore reads as "the lever could not clear a bar its own class barely reaches", not "the idea was wrong" — and a result of +1 or +2 is consistent with the lever working on part of its class. Both readings are stated now so neither is argued afterwards.

Two of the twelve failures are about the agent believing the wrong evidence and are not touched by this lever at all: sympy-15017 (tried the right fix, watched an existing test that encoded the bug fail, retreated) and pylint-6386 (a reproduction that proved the crash had stopped rather than that `-v` worked). They are read separately this cycle, whatever the lever does.

## The lever

`ARBOS_MECHANISM_DIFF_CHECK=1` (branch `cursor/swebench-loop-c15-7c9c`, `88049581` on `main` `7e19f9e9`). At the first final reply after an edit, once per turn: the identifiers in the recorded mechanism line (snake_case, dotted paths, CamelCase, file names — plain words are not names) are looked for in `git diff HEAD`'s changed lines, hunk headers and file names. None present → one nudge naming both sides ("Your mechanism names `_sqrt_match`. Your diff touches `radsimp.py` (in `split_surds`). None of the names is in the diff. Fix where the fault is, or say in one line why the fix belongs where it is"). No mechanism line recorded → one nudge asking for the code path and showing what the diff touches. Line present and matched → nothing.

Note on the kernel: `main` no longer refuses an edit without a mechanism line (the features agent removed the gate after cycle 13's `placeholder` finding, `8f4a62f3`). In cycle 14's kernel the gate was still on and 40/40 rollouts stated a line; on this kernel the line is optional, so the "no line" branch of the nudge will fire in arm B where the agent did not volunteer one. How often it volunteers one is itself recorded.

## The set: regression 20b, v2

Cycle 14's four never-run draws all solved 2/2. Two are swapped for the next two "1–4 hours" instances in `order_remaining` never run: scikit-learn-10908 → django-14631, django-13033 → django-15503. The other 18 are unchanged. The swapped pair's baseline is arm A of this cycle.

## Arms

Same kernel (`arbos-kernel-c15`, built from `88049581`), same harness (this branch), one reproduction, $8 cap, 2400 s, Sonnet 5, concurrency 3, network cut with the pre-grading sweep, `-r 2`.

- A: check off.
- B: check on.

Cap $30 each, watcher at $27; each arm also has a wall-clock limit of 3 h 30 min (`timeout`), so the cycle closes without anyone noticing.

## Decision rule

Compared on the instances both arms cover twice.

- Adopt (harness default on): B solves at least 3 more rollouts than A on the shared instances, at no more than 1.3× A's cost per rollout.
- Drop (remove the code): B − A ≤ 2, or cost > 1.3× without ≥ 4.
- Recorded, not decisive: how many B rollouts were nudged (line-mismatch vs no-line), how many of those moved the diff to the named location, how many flipped failed → solved, how many solved rollouts B lost, and the volunteer rate of the mechanism line on the gate-less kernel.
