---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 15 — lever dropped inside its own ceiling; the kernel base between `30eef166` and `7e19f9e9` moved the instrument down

No PR (the lever is dropped; its code is `media/swebench/loop/cycle-15/lever-mechanism-diff-check.patch`). Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 15; data `media/swebench/loop/cycle-15/`.

## The lever

Mechanism-vs-diff check, one nudge at the first final reply after an edit. Pre-registered ceiling: its class was 3 of 12 failures on the set, so +3 was both the most it could move and the adopt threshold. Result on the 14 instances both arms covered twice: A 11/28, B 12/28, +1, 1.23× cost. Dropped per the rule. The "your mechanism names X, the diff does not touch X" form fired zero times in 29 rollouts; when the agent writes a line, the diff touches something it names. The seven nudges that fired were all "no line recorded"; none led to another edit.

## The thing to act on: a kernel base regression candidate

Twelve instances covered twice in both cycle 14 (kernel `30eef166`) and this cycle's no-lever arm (kernel `7e19f9e9`), same harness, same cut: **16/24 → 9/24** (the lever arm: 10/24). Lost: matplotlib-24870 (2/2 → 0/4 across arms), astropy-14182, pytest-6197, astropy-13236, pylint-8898. Engine-touching merges between the bases: #399 (mechanism gate removed), #405 (rewind checkpoint written and awaited before the turn proceeds), #407 (jobs: refused kill not reported as done), #408 (turn errors said; folder marked failed), #410 (wipe guard by effective directory). No wipe refusal appears in any cycle-15 transcript; repro-gate refusals are at cycle-14 rates. I cannot attribute it from this data. Cycle 16 re-runs the twelve on `30eef166` and, if the gap holds, bisects. Until then the loop treats `7e19f9e9` as not comparable with cycle 14's 70%.

Two observations that may help whoever looks first: (a) in matplotlib-24870 both cycle-14 solutions also changed `tri/_tricontour.py`; all four cycle-15 patches touched `contour.py` only — narrower exploration, not a different mechanism; (b) the mechanism line is now optional and was still volunteered in 47 of 59 rollouts, so #399 by itself is an unlikely cause, but the prompt paragraph it changed is the one that also carried "check that line against every symptom the request names".

## The two evidence failures, read in full

Both from cycle 14, transcripts in the cycle-15 folder.

- **sympy-15017.** The agent made the gold fix (`_loop_size` = 1 for rank 0, four places), verified it, and correctly left the existing `len == 0` assertion as the one the request overrides. Then a second assertion in the same test — `raises(ValueError, lambda: rank_zero_array[0])` — also broke, because `_loop_size` bounds indexing too. Thinking, verbatim: "the original request only asked about `len()`… this side effect breaks something I shouldn't touch." It reverted the root fix and special-cased `__len__`/`__iter__`, having said one step earlier that this "feels like patching a symptom rather than the actual root cause." Hidden test: `rank_zero_array[0] == x`. The wrong evidence: an old assertion about a consequence of the same wrong value, read as intent because the request did not name it. Proposed rule for the contract: when the root fix changes a behaviour an existing test asserts, ask whether that behaviour is computed from the same wrong value; if so, the test encodes the bug too.
- **pylint-6386.** After the fix, the agent ran `-v` and `--verbose` side by side and saw the defect: `--verbose` printed "Using config file", `-v` did not. Thinking: "Odd… that discrepancy needs checking." It re-ran `-v` alone, saw no message again, and wrote "Good — consistent now." Then "Both symptoms from the report are fixed." Hidden test: "Using config file" after `-v`. The wrong evidence: the absence of the error taken as presence of the behaviour, and a re-run compared with itself rather than with the reference it had just been shown. For the reproduction gate: "exits non-zero, then exits zero" is evidence a crash stopped, not that a feature works.

## Also

`arbos-kernel run` still exits with job shells alive in some rollouts (sweep killed 29 live processes across arm A, 13 across arm B; `left` 0 everywhere). Filed in cycle 14; still open.
