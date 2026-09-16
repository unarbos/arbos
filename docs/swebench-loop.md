> **REBUILT BY THE AUTHOR.** The original (19,179 bytes at 2026-09-14 17:53 UTC, plus the cycle-5 additions written 2026-09-14 ~18:00 UTC) was lost with the whole `docs/` directory on 2026-09-16 (07:43–09:01 UTC; see `internal/store-docs-loss-2026-09-16.md`). This copy is rebuilt from the author's own transcript — the exact text of every tool call that wrote or edited this file across cycles 1–5 — and checked against the data that never left the store: `media/swebench/loop-history.jsonl` (six entries), `media/swebench/loop/loop-state.json`, and `media/swebench/loop/cycle-N/`. The cycle-6 section is the text that was parked in `internal/swebench-loop-doc-update-cycle-6.md` while `docs/` was gone. Two edits are marked *(reconstructed)* where the original wording is not in the transcript verbatim: the per-cycle "Next" lists were renumbered in place several times and are consolidated here. Every number is from the history file or the per-cycle traces. Owner: `bc-bfb2cd63-da09-5a42-920b-3410d3337c9c`.

---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# SWE-bench improvement loop — living doc

One cycle = run Arbos on 50 fresh SWE-bench Verified instances, classify every loss, fix the top cause in the agent, re-run, record the delta. Model: Claude Sonnet 5 via OpenRouter (cache breakpoints on). Grader: `primeintellect/swebench-verified` (Harbor). Data: [`media/swebench/loop/`](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/swebench/loop/) (`loop-state.json` = stratified order and slices; `cycle-N/` = traces, A-vs-B table, scripts), history in [`media/swebench/loop-history.jsonl`](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/swebench/loop-history.jsonl). Failing bundles for QA: `internal/qa/rollouts/swebench/loop-cycle-N/`.

## Score board

| | Instances | Arbos before | Arbos after cycle 1 | Codex |
|---|---|---|---|---|
| Fixed baseline set (24, stratified: 7 easy / 12 medium / 5 hard) | 24 | 20 | 22 → **21** after cycle 2 | **22** ($41.30, no caching) |
| Regression set (original 16 → 20 from cycle 2) | 16 / 20 | 12 | 13 / 15 → 15 → **13** (cycle 3) | 10/12 run |
| Cycle 1 slice (50 fresh: 16 easy / 31 medium / 3 hard) | 50 | 42 (run A) | 40 (run B) | not run |
| Cycle 2 slice (50 fresh: 20 easy / 26 medium / 4 hard) | 50 | 37 (run A) | **41** (run B) | not run |
| Cycle 3 slice (50 fresh: 20 easy / 26 medium / 4 hard) | 50 | 39 (run A) | 38 (run B) | not run |
| Cycle 4 slice (50 fresh: 19 easy / 27 medium / 4 hard) | 50 | 39 (run A) | 38 (run B) | not run |
| Cycle 5 slice (50 fresh: 20 easy / 26 medium / 4 hard) | 50 | 40 (run A) | **42** (run B) | not run |
| Cycle 6 slice, first 40 (`main` `43d8569`, measurement only) | 40 | **36** | — | not run |
| Regression 20 at `-r 2`, complete (cycle 6) | 40 rollouts | **28** (17 instances once, 11 both) | — | — |
| Covered so far | 316 of 500 (slice 6: 40 of 50 run) | | | |

Cost per instance: Arbos $0.42–0.53 before the gates, ~$0.59 with both gates; Codex $1.72 on the same 24. Wall time per instance (median): Arbos 139–157 s; Codex 56 s.

**Baseline to beat (set 2026-09-13 after cycle 1): parity with Codex on the 24-set, 22/24 each.** The next cycles count as a win only when Arbos passes 22 on that set, or holds 22 while the slice score rises.

## Cause histogram (losses on the cycle slices, before the fix)

| Cause | Cycle 1 (run A, 8 losses + 1 error) | Cycle 2 (run A, 13 losses) | Cycle 2 after fix (run B, 9) | Cycle 3 (run A, 11) | Cycle 4 (run A, 11) | Cycle 5 (run A, 10 → run B, 8) | Cycle 6 (`main`, 4 of 40) |
|---|---|---|---|---|---|---|---|
| wrong layer — fixed the symptom's caller/outer layer; hidden tests exercise the shared helper | 5 | 4 | 2 | 0 | 0 | 3 → 1 | 0 |
| partial-complete — an adjacent case the issue implies was not covered | 1 | 4 | 2 | 3 | 3 | 3 → 3 | 1 |
| wrong mechanism — right file, wrong fix (new in cycle 2) | 0 | 3 | 3 | **6** | **8** | 4 → 2 | **3** |
| scope drift — generalised past the issue; hidden test pins the narrow behaviour | 1 | 1 | 1 | 2 | 0 | 0 | 0 |
| test editing | 0 (on this slice; 2 on the original 16) | 1 (fixture files under `tests/`) | 1 | 0 | 0 | 0 | 0 |
| env discovery | 0 losses (waste only; fixed by #96's login shell) | 0 | 0 | 0 | 0 | 0 | 0 |
| call granularity | 0 losses (wall time only) | 0 | 0 | 0 | 0 | 0 | 0 |
| tool gap | 0 on the slice; 1 on the regression set (`rm -rf /tmp/x` asked for approval) | 0 | 0 | 0 | 0 | 0 | 0 |
| grader / harness artefact | 1 (trace dir over Harbor's 32 MB artifact cap) | 0 | 0 | 0 | 0 (disk full before the runs; restarted) | 0 | 0 |

## Fixes shipped

| Cycle | PR | What | Verified on |
|---|---|---|---|
| 0 | [#96](https://github.com/unarbos/arbos/pull/96) (features agent) | tests are the spec, done-criterion pass, `bash -lc` + environment probe, `changes` names edited tests | pylint-8898 flipped |
| 1 | [#102](https://github.com/unarbos/arbos/pull/102) (merged to main) | existing tests read-only (no "say why" exception), then: a test never vetoes the requested change; fix at the root; verify with the module's tests; `rm -rf /tmp/x` no longer asks; harness artifacts out of `/logs/artifacts` | regression 12→13 (astropy-13398, pylint-8898 flipped); baseline-24 20→22 |
| 2 | [#139](https://github.com/unarbos/arbos/pull/139) | `[hook] Tests covering this edit` line after every source edit (git grep of the test tree for the functions the diff touches; "no existing test names …" = wrong-layer signal) + the concrete pre-edit grep in the CONTRACT; rules re-expressed in main's condensed CONTRACT | slice 2: 37→41; regression 20: 15→15 (+15022 never-veto verified, +15252; −13398, −25102) |
| 3 | [#142](https://github.com/unarbos/arbos/pull/142) (stacked on #139) | coverage hook per changed function; "class-level match only (Class named in …), no test names <method>"; a prose "name the mechanism before you edit" rule was tried and dropped (followed in 3/50) | slice 3: 39→38; regression 20: 15→13 (with the rule in) — no score claim; signal improvement only |
| 4 | [#179](https://github.com/unarbos/arbos/pull/179) (stacked on #142) | `mechanism` argument on the first edit: recorded, echoed, shown by `changes`; refusal without it is opt-in (`ARBOS_MECHANISM_REQUIRED=1`, harness default) after the measurement | slice 4: 39→38 with the refusal on (50/50 stated a mechanism, 20 refusals); regression 20 at `-r 2`: 21/29 rollouts, the four flip-prone instances split 1/1 |
| 5 | [#186](https://github.com/unarbos/arbos/pull/186) (stacked on #179) | `bash repro:true` records a failing reproduction; first edit refused without one (`ARBOS_REPRO_REQUIRED=1`, harness default); `changes` re-runs reproductions and reports pass / STILL FAILS; the last failing bash command counts as the reproduction | slice 5: 40→**42** (+2: two wrong-mechanism and two wrong-layer losses flipped; two variance losses); cost +53% from refusals, refinement unmeasured |
| 6 | [#295](https://github.com/unarbos/arbos/pull/295) (harness only) | provider refusals exit 75 → verifiers error, not a zero; `vision_model` on a working route (`openai/*` is 403 on this key); `grep -c` doubling | measurement cycle: `main` `43d8569` 36/40 on slice 6; regression `-r 2` complete 28/40; #186's refinement cut reproduction refusals 2.9 → 1.05 per rollout |

## Cycle 1 (2026-09-13) — detail

- **Run A** (integration kernel `108a324`, no #96): 42/50, $26.56, 1 errored (django-15629, harness artefact).
- **Fix**: #102 on top of #96 (see table). Regression 16: 13/16 vs 12/16; sphinx-7590 lost to the `rm -rf /tmp` approval false positive, fixed in the same PR after that rollout.
- **Run B** (kernel `f610f39`): 40/50, $21.15 (−20% cost, −14% calls, −11% wall).
- **Delta on the slice: −2.** Flipped to solved: django-14017 (scope drift), django-14792 (wrong layer) — both the targeted classes. Flipped to lost: django-11734, 11728, 15629 (partial, hard/variance: 11734 and 15629 are 3–4-file gold patches), **django-15252** (the "fix at the root" rule sent the agent one layer deeper than the maintainers — recorder instead of executor), **django-15022** (the read-only-tests rule read as "keep the old behaviour": the request contradicted an existing test's expected value, and the agent preserved the test's behaviour). 15% of Verified test patches change an existing assertion, so the rule was reworded in `b9ad38d`: never rewrite a test, but a test never vetoes the requested change — verified in cycle 2's regression set (15022 flipped).
- **Persisting losses** (both runs): matplotlib-25479, pylint-4970, pylint-7080, sphinx-8548 — all wrong-layer. The rule alone did not move them; the agent still finds a plausible outer-layer fix and stops. Next lever: make "look at the existing tests of the module you change" concrete (a `grep` of the test tree for the function name before editing), or a `changes`-time check that lists which test files import the touched symbols.
- **Spend**: Arbos $59.70 of the $60 cap (A 26.56 + regression 11.99 + B 21.15). Codex baseline $41.30 one-time (12 reused from the comparison run, 12 new at $10.88).

## Cycle 2 (2026-09-13/14) — detail

**Branch** `cursor/swebench-loop-c2` = integration `844e835` (coordinator contract #98/#103, subscriptions #104) + cycle 1 (`b9ad38d`) + the cycle-2 fix `16dec37`; rebased onto `main` `e7fb6a7` as `7ae09ca` with the loop's rules re-expressed in `main`'s condensed CONTRACT. The rules and the coordinator directive coexist in `prompt.rs` (`CONTRACT` vs `COORDINATOR_CONTRACT`).

**Target**: wrong-layer losses (5 of 8 in cycle 1; 4 persisted after the prose rule).

**The fix (`16dec37`)**: after every `edit`, `write`, or `apply_patch` on a source file, the tool result ends with `[hook] Tests covering this edit — <file>: <fn>, <fn> named in tests/a.py, tests/b.py` or `<file>: no existing test names <fn>. If the wrong value comes from a helper this calls, the fix belongs in the helper that has tests (fix at the root); if this is the right level, add a test here.` Implementation: `git diff HEAD -- <file>` → function/class names from hunk headers and hunk lines → `git grep -lw` over the test tree (`**/test*`, `**/tests/**`, `**/testing/**`, `**/spec/**`). Test files and non-code files are skipped; outside git nothing is said. Unit tests cover symbol extraction and the two note shapes on a temp repo (`tools/git.rs`, wired in `batch.rs` next to the after-tool hooks). CONTRACT: the fix-at-the-root paragraph names one concrete grep to run before the first edit and explains what the `[hook]` line means. Smoke (deepseek-flash, matplotlib-25479): the hook fired on every edit and the run landed in `cm.py` + `colors.py`, the gold files.

**Slice 2** (50: 20 easy / 26 medium / 4 hard), paused 21:51–23:51 UTC on Jacob's request, resumed with `--resume`:

| | Solved | Cost | Median calls |
|---|---|---|---|
| Run A (cycle-1 kernel on integration `844e835`) | 37/50 | $20.79 | 25 |
| Run B (`16dec37`: coverage hook + concrete pre-edit grep) | **41/50** | $23.69 | 27 |

**Delta on the slice: +4.** Flipped to solved: pylint-6386 (wrong layer → the hook said "no existing test names _preprocess_options" and the fix moved into `config/utils.py`, one of gold's files), sphinx-9461 (wrong layer/partial → after a "none" on `PyProperty` the patch grew to `domains/python.py` + `util/inspect.py` + autodoc, gold's set), sympy-17318 and astropy-13236 (partial), django-11206 (wrong mechanism). Flipped to lost: matplotlib-24870 (same two gold files edited, a different auto-level rule; variance). Persisting: matplotlib-23476 and sympy-21930 (wrong layer — the hook reported the touched class *is* named in tests, `FigureCanvasBase`, `_print_Pow`, so the signal was weak: broad classes and generic printer methods are named everywhere), django-15916 and sphinx-7462 (partial; 7462 stopped after 10 calls), matplotlib-26208 and django-12273 (wrong mechanism inside the right file), sphinx-11510 (scope drift, 9 files vs gold's 1), django-10097 (edited `tests/validators/*.txt` fixture files — the read-only rule names assertions and fixtures, but `.txt` data files under `tests/` were not read as fixtures).

**Regression 20** (original 16 + 15022, 15252, 14017, 14792) with `16dec37`: **15/20 → 15/20**, $15.08. Flipped to solved: django-15022 (the cycle-1 "a test never vetoes the requested change" rewording, now verified) and django-15252 (the fix-at-root overshoot, resolved). Flipped to lost: astropy-13398 (partial again — refraction added this time but `ITRS.location` missing; no tolerance edit, so the read-only rule held) and scikit-learn-25102 (a different mechanism from cycle 1; variance). Codex-baseline 24-set: 21/24 vs Codex 22/24 — not beaten this cycle.

**Infrastructure finding**: this VM is frozen while the worker sits idle between tool calls (monotonic uptime 7.0 h against 11.2 h of wall clock at 21:43 UTC — 4.2 h lost on 2026-09-13). Rollouts in flight during a freeze show wall times of 2900–3400 s that are not real; the kernel's own timeout uses the monotonic clock, so it did not fire early. Cycle wall times in this doc are therefore upper bounds. Mitigation from then on: wait with a foreground shell command, not an idle sleep.

**Spend**: Arbos $59.56 of the $60 cap (A $20.79, B $23.69, regression $15.08).

## Cycle 3 (2026-09-14) — detail

**Branch** `cursor/swebench-loop-c3` on #139 (rebased onto `main` `f0b0897`). **Targets**: the coverage hook's class-level blind spot; "wrong mechanism" (right file, wrong fix).

**Slice 3** (50: 20 easy / 26 medium / 4 hard): run A (#139 kernel) **39/50**, $17.59, median 20 calls; run B (hook per function + mechanism prose rule) **38/50**, $18.00, median 23 calls. Flips: +sympy-15017 (scope drift healed); −django-16454, −django-16263 (variance: both solved in A with shorter runs; B's 16263 ran 119 calls into the wrong file set). All six wrong-mechanism losses persisted: django-13794 (`lazy` proxy `__radd__`), django-15563 (MTI update), sphinx-7748 (overloaded docstring signatures), sympy-18199 (`nthroot_mod` with `a % p == 0`), matplotlib-26466 (copy of `xy`), requests-2931 (`to_native_string` on binary bodies) — each in gold's file with a fix that satisfies the reporter's example and not the hidden test.

**Why the mechanism rule did nothing**: the transcripts show a `Mechanism:` line *before the first edit* in 3 of 50 run-B rollouts and in the *final summary* in the rest. The model treats "before you edit" prose as a reporting format. Same pattern as cycle 2's pre-edit grep (~half). The rule is dropped in `1eaecde`; the hook stays. Lesson for the loop: a pre-edit step must be enforced by the kernel.

**Regression 20** (with the rule in): 15 → **13** (+astropy-13398, +scikit-learn-25102; −django-14792, −15022, −15252, −pylint-8898). These four have flipped back and forth across cycles 1–3; the regression set is noisier than the slice at n=1 per instance.

**Signal check**: the sharpened hook fired "class-level match only" in 12 of 50 rollouts and "none" in 9; no wrong-layer loss appeared on slice 3 in either run (0 of 11 in A), so its effect on the score could not be measured here — it is carried as a signal improvement.

**Spend**: $51.99 of the $60 cap (A $17.59, B $18.00, regression $16.40).

## Cycle 4 (2026-09-14) — detail

**Branch** `cursor/swebench-loop-c4` on #142 (`main` `e793b03`; #139 merged). **Target**: wrong mechanism, kernel-enforced this time.

**The fix**: `mechanism` argument on `edit`/`write`/`apply_patch`; the first such call after a user message must carry one line (code path, why, change) or is refused with the reason; the line is recorded, echoed in the tool result, printed by `changes` with the done-criterion question. **Enforcement worked**: 50 of 50 run-B rollouts recorded a mechanism before their first edit (20 were refused once first). **Outcome did not move**: slice 4 (19 easy / 27 medium / 4 hard) run A **39/50** ($18.15, median 22 calls) → run B **38/50** ($21.41, median 26.5). Flips: +sympy-18698; −django-15098, −astropy-14182. Nine losses in both runs, eight of them wrong-mechanism in gold's file (django-16950, 14140, 10999, sympy-13974, 13798, astropy-14598, 14369, requests-5414). The agent's stated mechanism is the same wrong one it acted on; making it say so does not make it check it. The refusal is therefore opt-in (`71f37c4`); the argument and the `changes` line stay as reviewer-facing record.

**Regression 20 at `-r 2`** (gate on), stopped at the $60 cap after 29 of 40 rollouts: 21/29 solved; 15 instances seen, 13 solved at least once, 9 both times. The four instances that flipped across cycles 1–3 (django-14792, 15022, pylint-8898, astropy-13398) each split 1/1: coin flips at this model, not regressions. django-15252 2/2.

**Incident**: the first launch of both runs failed on a full disk (254 GB, 183 instance images from slices 1–3); images not in the current slice or the regression set were pruned, the runs restarted, and `cycle_run.sh` now prunes before each batch when under 40 GB.

**Spend**: $58.98 of $60 (A $18.15, B $21.41, regression $19.42).

## What four cycles say

- Fixes that changed what the agent *can see* moved the score: login shell + environment probe (#96), coverage hook (cycle 2: +4). Fixes that ask the agent to *think differently* did not: prose rules for scope, mechanism, or done-criterion moved ±1 within noise; kernel-forcing the statement (cycle 4) got 100% compliance and 0 effect.
- The remaining loss class, wrong mechanism (8 of 11 on slice 4), is the agent landing in gold's file, satisfying the reporter's example, and missing the hidden test's second consequence. Levers that can still work are *evidence* levers: a second failing input derived from the issue text, run before the first edit and re-run after.
- Noise floor: single-rollout deltas of ±2 on 50 instances and ±2 on the regression 20 are within variance; four regression instances are coin flips. A win is ≥ +4 on the slice or `-r 2` majorities.

## Cycle 5 (2026-09-14) — the evidence lever

**Branch** `cursor/swebench-loop-c5` on #179 (`main` `73ad619`). **Target**: wrong mechanism, with evidence instead of a statement.

**The fix (`0e5d5ee`)**: `bash repro:true` records a reproduction with its exit code (`repro.jsonl`, reset per user message; exit 0 refused as "not a reproduction"). With `ARBOS_REPRO_REQUIRED=1` (harness default) the first edit of a task is refused until one failing reproduction is on record. `changes` re-runs every recorded reproduction and reports `pass` / `STILL FAILS`. CONTRACT asks for the reporter's example plus a second input the request implies.

**Slice 5** (50: 20 easy / 26 medium / 4 hard): run A (mechanism gate) **40/50**, $17.54, median 23 calls → run B (+ reproduction gate) **42/50**, $26.80, median 30. Flips: +django-13297, +sphinx-8035 (wrong mechanism in gold's file), +scikit-learn-14629, +django-14011 (wrong layer); −astropy-13977, −sphinx-8265 (variance). **+2 is inside the noise band**, but it is the first lever since cycle 2 whose flips land on the targeted classes, and the only one to move a wrong-mechanism loss at all.

**Compliance and cost**: 50/50 rollouts recorded a failing reproduction; 41/50 reached a `changes` re-run report; 4 saw `STILL FAILS` and kept working. 146 refusals over 50 rollouts (2.9 each): in 46 the agent had just run the failing snippet *without* `repro:true`, in 27 it went on reading instead. Refinement `8182be2`: every failing bash command before the first edit is noted and the gate takes the last one as the reproduction instead of refusing. Effect on cost measured in cycle 6.

**Regression 20 at `-r 2`**: stopped at the $60 cap after 14 of 40 rollouts (the hard regression instances cost ~$1 each with the gate on): 11/14; astropy-13398 0/2, django-14792 1/1. Too partial to claim either way.

**Spend**: $58.99 of $60 (A $17.54, B $26.80, regression $14.65).

## Cycle 6 (2026-09-16) — measurement on `main` `43d8569`

Kernel behaviours merged since cycle 5 (#186 repro gate + last-failing refinement, #285 spawn guard, #278 tool markup stripped, #283 403 falls through, #287 archived workers) were measured together; no run B this cycle — the complete `-r 2` regression took $38.94 and left room for 40 of the 50 slice instances, not for a second pass.

**Regression 20 at `-r 2`, complete for the first time: 28/40 rollouts.** 17 of 20 instances solved at least once, 11 both times. Six instances split 1/1 (astropy-13398, django-14792, django-15252, pylint-8898, scikit-learn-25102, sphinx-7590); three at 0/2 (django-15022, requests-2317 grader, xarray-6992). This is the noise-floor baseline for later cycles: compare rollout counts (28/40), not instance counts.

**Slice 6, first 40: 36/40 (90%)**, $23.54, median 29.5 calls. Losses: xarray-7229, django-16631, matplotlib-21568 (wrong mechanism in gold's file), sympy-22080 (missed `codeprinter.py`). Highest rate of any cycle, on a different slice, so not a like-for-like claim; slice 6's remaining 10 run at the start of cycle 7.

**#186's refinement, measured**: reproduction-gate refusals fell from 2.9 per rollout (cycle 5) to **1.05**; the last failing bash command was taken as the reproduction in 20 of 40 rollouts; all 40 had a reproduction on record; 17 reached a `changes` re-run report. Mechanism-gate refusals: 18 over 40 rollouts.

**Provider note**: OpenRouter returns 403 on every `openai/*` model for this key. Nothing in the loop or the comparison ever named an OpenAI model (Sonnet 5 throughout; the Codex baseline was Sonnet over the Responses wire), so cycles 1–6 stay comparable. The harness now records a refusal as an error, not a zero, and describes images through `google/gemini-2.5-flash` ([#295](https://github.com/unarbos/arbos/pull/295)); the kernel's own `openai/*` defaults (`OPENROUTER_FALLBACKS`, `OPENROUTER_VISION_DEFAULT`) are filed in `internal/features-inbox/2026-09-16-openrouter-openai-block-kernel-defaults.md`.

**Spend**: $62.50 — **$2.50 over the cap**: the two runs shared the cap and rollouts in flight finished after the batch-level check. From cycle 7 the regression run has its own cap ($30) and the slice runner's cap is set from what remains.

## Next (cycle 7)

1. Finish slice 6 (10 instances) so the 50-instance number exists, then slice 7 with a run B on the top loss class of the day — wrong mechanism again (3 of 4), now that both gates are in place and cheap.
2. Wall time: Django `runtests.py` to the 1800 s timeout persists; `bash_wait_ms` 600 s for headless runs.
3. Cap discipline: regression `-r 2` under its own $30, slice runs under the remainder, checked per batch *and* per rollout in flight.
4. Partial-complete (3 of 11 in cycle 3, 3 of 11 in cycle 4): the done-criterion pass reads the request, not the *tests the request implies*; a check that lists the hidden-test-shaped cases (each example, each edge in the issue) against new test functions. *(reconstructed: this item stood in the "Next" list from cycle 2 onward.)*
