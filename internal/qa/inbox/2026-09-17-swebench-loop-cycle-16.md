---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 16 — no kernel regression; the loop's noise band was half the real one

No PR. Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 16 and the third standing finding at the top; data `media/swebench/loop/cycle-16/`.

## The claim withdrawn

Cycle 15 reported 16/24 → 9/24 on identical instances between kernels `30eef166` and `7e19f9e9` and called it a kernel regression. The cycle-14 binary itself, re-run on the same twelve instances the next morning: **11/24**. Six runs of these instances across three kernels: 16, 9, 10, 11, 13, 15. Standard deviation 2.8 (binomial expectation 2.0). The gap was two single runs on either side of the mean.

## #440 (the restored mechanism gate)

On one kernel (`1e9be864`), the twelve at `-r 2`: gate off 13/24, gate on 15/24, cost per rollout $1.34 vs $1.16. +2 is what two identical arms produce. The gate is neither shown to help nor to hurt at this resolution. With it on, 8 of 24 rollouts were refused once and then stated a line; nothing in the outcomes distinguishes them. Whether #440 merges is the features agent's call; the loop has no evidence either way.

## #410, #405, #407, #408

#410 ruled out: `bash: refused` zero times in all 34 cycle-15 failures and every cycle-14/15 rollout. #405 withdrawn by its author (the write-wait is #419, outside the range). #407/#408 untested and now unmotivated: nothing moved.

## The method finding

The loop used "±2 is noise" from cycle 9 on, set by eye from one tie, never checked against repeated runs of one configuration. It is about one standard deviation of a single arm; the difference between two arms has a standard deviation near 4 on 24 rollouts and near 5 on 40. Every lever decision since cycle 8 — N=2 reproductions, the `changes` nudge, the critique, the mechanism-vs-diff check — used adopt thresholds of about one standard deviation, and every delta reported (+1 to +2) is consistent with both no effect and a real 5–10 point effect. Their fates do not change (inside the noise stays dropped) but the stated reason does: the loop could not have seen them move unless they moved by about 20 points. From cycle 17 the band is stated from this data in every pre-registration: ≥ 7 of 24 or ≥ 8 of 40 rollouts.

## For the kernel side

Nothing new. The two evidence-reference rules from cycle 15 are with you in `internal/features-inbox/2026-09-17-swebench-evidence-reference-rules.md`; `arbos-kernel run` exiting with job shells alive (cycle 14) is still open.
