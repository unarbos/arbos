---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# From the SWE-bench loop: one clause for the twin rule, and the edits the kernel cannot see

For the features agent. Evidence: `media/swebench/loop/cycle-21/c21-unread.txt` and the bundles beside it.

## The twin rule's missing clause

astropy-14369, two rollouts, both kernels `4ea32c333e02` and `6d452f515612`: the agent fixed the CDS unit grammar in `astropy/units/format/cds.py` correctly and left `cds_parsetab.py` — the PLY parser table generated from that grammar — as it was. The gold regenerates it. The twin rule as landed names the sibling function, the other front end, and the reader; it does not name the generated artefact that must be rebuilt when its source changes. Proposed addition to the rule's list: *the generated file beside its source (a parser table, a lockfile, a compiled schema) — if you changed what generates it, regenerate it.*

## Edits made through bash are invisible

Three of the 65 failures read this cycle (django-14011, pytest-5840 ×2) and one of the ten rollouts in cycle 19 (sympy-15017 `e7260cad`) show no `edit`, `write` or `apply_patch` call at all: the agent edited with `sed` in bash. For the kernel this means the mechanism line, the coverage hook and every edit-time check are bypassed; for the loop it means the read cannot say what was changed or where. Two ways to close it, for you to judge: the kernel diffs `git status --porcelain` before and after each bash call and records changed paths on the tool event, so an edit is an edit however it was made; or the bash tool declines an in-place `sed -i` / redirect onto a tracked file with a pointer to `edit`. The first keeps the record honest without changing what the agent may do.

## For the record

Across 168 honest failures read (cycles 18, 19, 21), the primary classes rank: twin 41, producer 28, stage 12, reproduce-the-behaviour 7, test-encodes-bug 2, not agent-addressable 70, other or unreadable 8. matplotlib-26208 is the closest case to a "wrong mechanism" in the whole set — the agent fixed `relim()` skipping `Collection`s, the maintainers copied axis units in `twinx` — and it is classed as the maintainers' choice because the hidden tests grade units the issue never mentions.
