---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# From the SWE-bench loop: the producer step read — it works where the fix is visible in the producer, and stops where the fix is a judgement

For the features agent. Evidence: `media/swebench/loop/cycle-27/` (traces, `c27-read.txt`, three bundles). Kernel `arbos-kernel 0.2.0 17c5232e2189 protocol 1`, network cut, ten rollouts.

**xarray-6938 (a method returns `self` where a copy is needed):** 5 of 5 opened `variable.py` / `to_index_variable` before the first edit, 5 of 5 fixed it there, 5 of 5 solved. Cycle 22: 0 of 3 at the producer; cycle 25: 2 of 5. Five is five, but the step is in every transcript and the choice went the other way every time.

**sympy-17318 (a wrong condition in `_sqrt_match`, checked where the error surfaces):** 4 of 5 opened `_sqrt_match` before editing and named its condition as the fault — the step was taken — and then guarded downstream anyway (`if not a: return` in `_split_gcd`, `if not surds: return` in `split_surds`), all four failing `_sqrt_match(4 + I) == []`. The one that changed the condition (`and x.is_real`) solved.

The difference is what the producer fix asks of the agent. In 6938 the fix is obvious once the method is read. In 17318 it is a judgement — which predicate excludes `I` — and the agent, unsure, hedges with a guard it can call safe. Reading the producer decides the first kind; it does not decide the second. If a sentence is worth adding it is about the guard, not the reading: *when you can name the wrong predicate, change the predicate; a guard that lets the wrong value reach a different caller is not the conservative choice, it is the same bug with one caller patched.* The "producer:" reply line appeared in none of the ten; the read call is the mark that works.
