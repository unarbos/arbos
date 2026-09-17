---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# Update to `docs/swebench-loop.md` — cycle 6 (APPLIED 2026-09-16 09:50 UTC)

**Applied**: `docs/swebench-loop.md` was rebuilt in full by the author (banner at its top) and carries this text as its Cycle 6 section. Kept for the record.

`docs/` vanished from this VM's view of the store while cycle 6's results were being written (media/ and internal/ stayed). This file holds the exact text; apply it to `docs/swebench-loop.md` (or I will, the moment `docs/` is back — the script is `/tmp/swe/loop/doc-update-c6.py` on the SWE-bench worker VM).

## Score board — add rows

| Cycle 6 slice, first 40 (`main` `43d8569`, measurement only) | 40 | **36** | — | not run |
| Regression 20 at `-r 2`, complete (cycle 6) | 40 rollouts | **28** (17 instances once, 11 both) | — | — |
| Covered so far | 316 of 500 (slice 6: 40 of 50 run) | | | |

## Cause histogram — add column "Cycle 6 (`main`, 4 of 40)"

wrong layer 0 · partial-complete 1 · wrong mechanism **3** · scope drift 0 · test editing 0 · env discovery 0 · call granularity 0 · tool gap 0 · grader/harness artefact 0.

## Fixes shipped — add row

| 6 | [#295](https://github.com/unarbos/arbos/pull/295) (harness only) | provider refusals exit 75 → verifiers error, not a zero; `vision_model` on a working route (`openai/*` is 403 on this key); `grep -c` doubling | measurement cycle: `main` `43d8569` 36/40 on slice 6; regression `-r 2` complete 28/40; #186's refinement cut reproduction refusals 2.9 → 1.05 per rollout |

## New section (replaces "## Next (cycle 6)")

### Cycle 6 (2026-09-16) — measurement on `main` `43d8569`

Kernel behaviours merged since cycle 5 (#186 repro gate + last-failing refinement, #285 spawn guard, #278 tool markup stripped, #283 403 falls through, #287 archived workers) were measured together; no run B this cycle — the complete `-r 2` regression took $38.94 and left room for 40 of the 50 slice instances, not for a second pass.

**Regression 20 at `-r 2`, complete for the first time: 28/40 rollouts.** 17 of 20 instances solved at least once, 11 both times. Six instances split 1/1 (astropy-13398, django-14792, django-15252, pylint-8898, scikit-learn-25102, sphinx-7590); three at 0/2 (django-15022, requests-2317 grader, xarray-6992). This is the noise-floor baseline for later cycles: compare rollout counts (28/40), not instance counts.

**Slice 6, first 40: 36/40 (90%)**, $23.54, median 29.5 calls. Losses: xarray-7229, django-16631, matplotlib-21568 (wrong mechanism in gold's file), sympy-22080 (missed `codeprinter.py`). Highest rate of any cycle, on a different slice, so not a like-for-like claim; slice 6's remaining 10 run at the start of cycle 7.

**#186's refinement, measured**: reproduction-gate refusals fell from 2.9 per rollout (cycle 5) to **1.05**; the last failing bash command was taken as the reproduction in 20 of 40 rollouts; all 40 had a reproduction on record; 17 reached a `changes` re-run report. Mechanism-gate refusals: 18 over 40 rollouts.

**Provider note**: OpenRouter returns 403 on every `openai/*` model for this key. Nothing in the loop or the comparison ever named an OpenAI model (Sonnet 5 throughout; the Codex baseline was Sonnet over the Responses wire), so cycles 1–6 stay comparable. The harness now records a refusal as an error, not a zero, and describes images through `google/gemini-2.5-flash` (#295); the kernel's own `openai/*` defaults are filed in `internal/features-inbox/2026-09-16-openrouter-openai-block-kernel-defaults.md`.

**Spend**: $62.50 — **$2.50 over the cap**: the two runs shared the cap and rollouts in flight finished after the batch-level check. From cycle 7 the regression run has its own cap ($30) and the slice runner's cap is set from what remains.

### Next (cycle 7)

1. Finish slice 6 (10 instances), then slice 7 with a run B on the top loss class — wrong mechanism again (3 of 4), now that both gates are in place and cheap.
2. Wall time: Django `runtests.py` to the 1800 s timeout persists; `bash_wait_ms` 600 s for headless runs.
3. Cap discipline: regression `-r 2` under its own $30, slice runs under the remainder, checked per batch and per rollout in flight.
