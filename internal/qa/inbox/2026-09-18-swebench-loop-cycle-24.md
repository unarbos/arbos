---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 24 — on their own six instances, the five rules move the score by exactly the band

No PR. Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 24; data `media/swebench/loop/cycle-24/`.

Ceiling stated first (15–18 of 36 from 72 pre-rule rollouts), band second (≈ 10 of 36), then the run: six carrying instances at `-r 6`, control `arbos-kernel 0.2.0 4b833de9860f protocol 1` 19/36, treatment `arbos-kernel 0.2.0 aaf3dbe98de7 protocol 1` 29/36, **+10** — the pre-registered threshold met at its edge. astropy-13236 2/6 → 6/6, matplotlib-24870 1/6 → 4/6, scikit-learn-14629 4/6 → 6/6; the other three flat. Both arms 36/36, no cap, no stall, egress closed, labels proved. $36.38.

With cycle 22 (same kernels, −3 of 80 on a general set) this is one statement: the rules do what they were written to do on the instances that carry their patterns, and those are a small share of the benchmark. Not a rate; not 23/25.

All 24 failures read: A 9, F 4, B 2, E2 2, C-consistent 7; nothing outside the account. Two for QA: a control rollout read the version four times and still wrote the deprecation warning (the step without the rule); a control rollout opened `ClassifierChain` three times and still put the fallback in the caller — the step alone is not the choice. In the treatment, django-11728's two failures were 14 and 17 tool calls with no twin grep — the rule was not followed, and the short trajectory is again the failing one.
