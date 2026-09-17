---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# From the SWE-bench loop: two contract rules about the reference a check is made against

For the features agent. Jacob asked for these to go to you directly (2026-09-17 11:11 UTC): "they belong in the agent's contract, not only in your notes." Full transcript reads: `media/swebench/loop/cycle-15/read-sympy-15017.txt` and `read-pylint-6386.txt`; bundles beside them.

Both failures are the agent passing a check against the wrong reference and recording the pass as evidence. Neither is about where the diff lands, what the mechanism says, or whether a reproduction exists — the current contract's levers.

## 1. A test that asserts a consequence of the wrong value encodes the bug too

**What happened (sympy-15017).** The agent found the root — `_loop_size = reduce(...) if shape else 0`, the `else 0` wrong for rank-0 arrays — and fixed it in all four places (the gold fix). Running the module's tests, it hit `assert len(rank_zero_array) == 0`, recognised it as the bug the request names, and left it, per the contract's one exception. Then a second assertion in the same test failed: `raises(ValueError, lambda: rank_zero_array[0])` — indexing a rank-0 array had raised only because `_loop_size` was 0. Its thinking, verbatim: "the original request only asked about `len()`… this side effect breaks something I shouldn't touch." It reverted the root fix and special-cased `__len__` and `__iter__`, one step after saying that "feels like patching a symptom rather than the actual root cause." Hidden test: `rank_zero_array[0] == x`.

**The fault.** The contract's exception ("the request itself says the behavior that test asserts is wrong") was read as applying only to assertions the request *names*. An assertion about a downstream consequence of the same wrong value was read as intent.

**Proposed rule**, for the paragraph that begins "Existing tests are the spec and read-only":

> When the root fix changes a behaviour that an existing test asserts, ask whether that behaviour is computed from the same wrong value you are correcting. If it is, that assertion encodes the bug too — even when the request does not mention it — and it falls under the same exception: make the fix, leave the test, name it in your reply. Do not retreat from the root to a special case to keep a downstream assertion green.

## 2. A reproduction of a behaviour asserts the behaviour; a reproduction that only stops erroring proves a crash stopped

**What happened (pylint-6386).** The issue: `-v` demands an argument and `--help` shows `VERBOSE`. The agent reproduced the error, fixed `_DoNothingAction` to take no argument, and re-ran: `-v` no longer errored; the metavar was gone. It then ran `-v` and `--verbose` side by side and saw the defect — `--verbose` printed "Using config file", `-v` did not — and wrote: "Odd that `-v` didn't show the message like `--verbose` did — that discrepancy needs checking." It re-ran `-v` alone, saw the same silence, and wrote: "Good — consistent now." Then: "Both symptoms from the report are fixed." Hidden test: "Using config file" after `-v`.

**The fault.** Two: the reproduction was the error message, so its passing meant only that the error had stopped; and when the agent had the right reference in front of it (`--verbose`), it re-checked against its own previous run instead. The pass was recorded as evidence.

**Proposed rules**, for the reproduction paragraph and the done-criterion paragraph:

> A reproduction of a *feature that does not work* asserts the feature — the output the request expects — not the absence of the error. `exits non-zero, then exits zero` is evidence that a crash stopped, not that a behaviour is present.

> When two runs that should agree disagree, the reference is the one the request says is right (the long option, the other backend, the documented example), not your own previous run. Re-running the suspect alone and getting the same answer is not consistency; it is the defect twice.

## Why these two and not a gate

Cycle 15 measured a string check on the mechanism line: zero firings in 29 rollouts, because when the agent writes a line the diff does touch something it names — it names the guard too. Whether a named place is upstream is not a property of the string. These two rules are about which *reference* the agent checks against, which is where both honest failures actually went wrong, and which no gate the loop has built can see. They are offered as prose for the contract because that is where the agent's notion of "evidence" lives; measuring them is the loop's job once the kernel base is settled (cycle 16).
