---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 6 — measurement on `main`, provider refusals

PR [#295](https://github.com/unarbos/arbos/pull/295) (`cursor/swebench-loop-c6`, base `main`, harness only). Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md). Bundles: `internal/qa/rollouts/swebench/loop-cycle-6/A-<instance>/` (4).

## What to know

- `main` `43d8569` with both gates on scores 36/40 on a fresh slice and 28/40 rollouts on the regression set at `-r 2`. Six regression instances are coin flips (1/1); treat any single-rollout flip on them as noise.
- Reproduction-gate refusals are down to ~1 per rollout after #186's "last failing command counts" refinement (was 2.9). Mechanism-gate refusals: 18 per 40 rollouts — the agent still often attempts the first edit without the line. Cost per instance stays ~$0.59 (was $0.35 before the gates).
- OpenRouter 403s every `openai/*` model on the shared key. The harness now exits 75 on a provider refusal before any tool ran (recorded as an error, not a zero). Probe: a 403 *after* tools ran (fallback model mid-task) is still graded — with #283 the kernel should fall through to the next model; check that the fallback list on this key does not start with `openai/gpt-5.6-terra` (it does today; features-inbox note filed).
- Vision: the harness sets `vision_model = google/gemini-2.5-flash`; the kernel default is `openai/gpt-4.1-mini` and 403s. A desktop user attaching an image to a text-only model hits that.

## Losses on slice 6 (4 of 40)

wrong mechanism 3 (xarray-7229 `where(keep_attrs)`, django-16631 session-hash fallback, matplotlib-21568 `_wrap_in_tex` one-liner), partial 1 (sympy-22080 missed `codeprinter.py`).

## Infra

Cycle spend $62.50: $2.50 over the cap because the regression and the slice runs shared one cap and in-flight rollouts finished after the check. Next cycle: separate caps.
