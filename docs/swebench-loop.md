> **REBUILT BY THE AUTHOR.** The original (19,179 bytes at 2026-09-14 17:53 UTC, plus the cycle-5 additions written 2026-09-14 ~18:00 UTC) was lost with the whole `docs/` directory on 2026-09-16 (07:43–09:01 UTC; see `internal/store-docs-loss-2026-09-16.md`). This copy is rebuilt from the author's own transcript — the exact text of every tool call that wrote or edited this file across cycles 1–5 — and checked against the data that never left the store: `media/swebench/loop-history.jsonl` (six entries), `media/swebench/loop/loop-state.json`, and `media/swebench/loop/cycle-N/`. The cycle-6 section is the text that was parked in `internal/swebench-loop-doc-update-cycle-6.md` while `docs/` was gone. Two edits are marked *(reconstructed)* where the original wording is not in the transcript verbatim: the per-cycle "Next" lists were renumbered in place several times and are consolidated here. Every number is from the history file or the per-cycle traces. Owner: `bc-bfb2cd63-da09-5a42-920b-3410d3337c9c`.

---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# SWE-bench improvement loop — living doc

> **CORRECTION (2026-09-17, cycle 12).** Every number in this document from cycle 1 through cycle 11 was measured with the container on the Docker host network. The agent used it: in 133 of 948 rollouts it downloaded the newer release of the package under repair — the one carrying the fix — and 114 of those were graded solved. The score board below is left as it was written, as the record of what was claimed; none of those figures is a measure of the agent. The per-cycle count is in [`swebench-open-network-audit.md`](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-open-network-audit.md). **The baseline on the old regression 20 is the cycle-12/13 figure: 24 of 40 rollouts (60%), network cut, kernel `864d6b00`. From cycle 14 the loop's instrument is regression 20b: 28 of 40 (70%), network cut, kernel `30eef166`.** The 74% that cycles 10–11 reported was wrong and is not to be compared against.

One cycle = run Arbos on 50 fresh SWE-bench Verified instances, classify every loss, fix the top cause in the agent, re-run, record the delta. Model: Claude Sonnet 5 via OpenRouter (cache breakpoints on). Grader: `primeintellect/swebench-verified` (Harbor). Data: [`media/swebench/loop/`](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/swebench/loop/) (`loop-state.json` = stratified order and slices; `cycle-N/` = traces, A-vs-B table, scripts), history in [`media/swebench/loop-history.jsonl`](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/swebench/loop-history.jsonl). Failing bundles for QA: `internal/qa/rollouts/swebench/loop-cycle-N/`.

## Three findings about the method, stated once

**1. The "wrong mechanism" class was an artefact of contaminated data.** Cycles 3, 4, 5, 9 and 11 built levers for the failure class "right file, wrong fix": a prose rule, a kernel-enforced `mechanism` argument on the first edit, a reproduction gate, a `changes`-before-done nudge, a second-model critique. The class was defined by reading rollouts on the regression set that, the audit later showed, had in many cases downloaded the upstream fix: the agent's own reasoning was being compared against a copied answer, and where the copy and the reasoning diverged the reasoning read as "the wrong mechanism". When the network was cut (cycle 12) and the twelve honest failures were read against the gold patches (cycle 13), the class did not appear in a single one of them. Five cycles and $279 of model spend (cycles 3, 4, 5, 9, 11, from the history file) went to a class the contaminated data invented. The lesson is about method, not about the levers: a failure class must be defined on rollouts whose provenance is sound, and the first check on any new class is whether the rollouts that define it could have seen the answer.

**2. This benchmark has a ceiling for Arbos well below 100%, and it is not the agent's.** Of the twelve honest failures on the regression 20, eight are on instances where the hidden tests grade something the issue text does not determine: the exact error string on a malformed input (pylint-8898 — the agent's more thorough splitter errors differently), an ID-mangling scheme the issue never mentions (sphinx-7590), a twelve-test redesign behind a one-line symptom (xarray-6992), and a feature whose accepted scope grew past the issue (astropy-13398: refraction and topocentric ITRS). A stronger agent would fail these the same way; the only route past them anyone found was the upstream diff. Two more are the agent preserving behaviour the maintainers chose to change (django-15022). Only two of twelve — django-14792, root cause named and the consumers fixed instead — are reachable by a behaviour lever. Anyone reading the loop's numbers should read them against that ceiling: on this set it is about 34 of 40, and the agent stands at 24.

**3. The loop's noise band was half the real one, and every lever decision since cycle 8 sat inside it.** Cycle 16 ran the same twelve instances at `-r 2` six times across three kernels (cycles 14–16): 16, 9, 10, 11, 13, 15 of 24. The same kernel binary scored 16 one morning and 11 the next. Observed standard deviation of a 24-rollout run: 2.8 (binomial at the pooled per-instance rates: 2.0). The band the loop used from cycle 9 on — "±2 is noise" — is about one standard deviation of *one* arm; the difference between two arms has a standard deviation near 4 on 24 rollouts and near 5 on 40. So the adopt thresholds (+3 on the shared instances) were one standard deviation, and every delta the loop has reported since cycle 8 (N=2, the `changes` nudge, the critique, the mechanism-vs-diff check, "the kernel base moved") is inside what two identical arms produce. Cycle 15's alarm — 16 → 9 on identical instances, "a third of our capability" — was the band's failure, not the kernel's. What a $30 arm can detect on this benchmark is an effect of roughly 8 rollouts in 40, about 20 percentage points; nothing the loop has tried is that large, and nothing the twelve honest failures suggest would be. From cycle 17 the decision rule states the band from this data, and a lever that cannot plausibly move 20 points is measured at `-r 4` or not at all.

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
| Regression 20 at `-r 2`, `N=2` reproductions (cycle 7, 32 of 40 rollouts) | 32 rollouts | **28** (13 of 17 instances both times) | — | — |
| Cycle 7 slice (30: 12 easy / 16 medium / 2 hard) | 30 / 13 | 26 (run A, `N=1`) | 10 of 13 (run B, `N=2`; A on the same 13: 11) | not run |
| Regression 20 at `-r 2`, cycle 8 attribution, one kernel | N=1: 32 rollouts · N=2: 18 | N=1 **23** (72%) · N=2 **15** (83%); like-for-like 13/18 vs 15/18; pooled N=2 42/49 vs N=1 45/64 | — | — |
| Regression 20 at `-r 2`, cycle 9, kernel `e183793`, cap $4 | A: 25 · B: 22 | A (N=2) **14** (56%) · B (+`changes` before done) **15** (68%); shared 10×2: 12 vs 14 | — | — |
| Regression 20 at `-r 2`, cycle 10, kernel `90a33cb`, cap $8 | N=1: 35 · N=2: 18 | N=1 **26** (74%) · N=2 **14** (78%); shared 9×2: **14/18 vs 14/18**; $0.78 vs $1.79 per rollout | — | — |
| Covered so far | 346 of 500 (slice 6: 40 of 50 run; slice 7: 30) | | | |

Cost per instance: Arbos $0.42–0.53 before the gates, ~$0.59 with both gates; Codex $1.72 on the same 24. Wall time per instance (median): Arbos 139–157 s; Codex 56 s.

~~**Baseline to beat (set 2026-09-13 after cycle 1): parity with Codex on the 24-set, 22/24 each.**~~ Withdrawn 2026-09-17: both harnesses ran on the open network; neither 22 is verified. The baseline is cycles 12–13's 24/40 (60%) under the cut; see the correction at the top.

## Cause histogram (losses on the cycle slices, before the fix)

| Cause | Cycle 1 (run A, 8 losses + 1 error) | Cycle 2 (run A, 13 losses) | Cycle 2 after fix (run B, 9) | Cycle 3 (run A, 11) | Cycle 4 (run A, 11) | Cycle 5 (run A, 10 → run B, 8) | Cycle 6 (`main`, 4 of 40) | Cycle 7 (run A, 4 of 30) |
|---|---|---|---|---|---|---|---|---|
| wrong layer — fixed the symptom's caller/outer layer; hidden tests exercise the shared helper | 5 | 4 | 2 | 0 | 0 | 3 → 1 | 0 | 0 |
| partial-complete — an adjacent case the issue implies was not covered | 1 | 4 | 2 | 3 | 3 | 3 → 3 | 1 | 1 |
| wrong mechanism — right file, wrong fix (new in cycle 2) | 0 | 3 | 3 | **6** | **8** | 4 → 2 | **3** | **3** |
| scope drift — generalised past the issue; hidden test pins the narrow behaviour | 1 | 1 | 1 | 2 | 0 | 0 | 0 | 0 |
| test editing | 0 (on this slice; 2 on the original 16) | 1 (fixture files under `tests/`) | 1 | 0 | 0 | 0 | 0 | 0 |
| env discovery | 0 losses (waste only; fixed by #96's login shell) | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| call granularity | 0 losses (wall time only) | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| tool gap | 0 on the slice; 1 on the regression set (`rm -rf /tmp/x` asked for approval) | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| grader / harness artefact | 1 (trace dir over Harbor's 32 MB artifact cap) | 0 | 0 | 0 | 0 (disk full before the runs; restarted) | 0 | 0 | 0 |

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
| 7 | [#314](https://github.com/unarbos/arbos/pull/314) | `ARBOS_REPRO_REQUIRED=N`: the first edit needs N distinct failing reproductions (N=2 = the reporter's example plus one the agent derives); harness knobs `repro_required`, `mechanism_required` | regression `-r 2` at N=2: **28/32** vs the 28/40 floor (like-for-like on the same 17 instances 23/34 → 28/32; the four coin-flip instances 2/2 each); slice 7 subset: A 11/13 vs B 10/13 (noise); kernel base also moved, so attribution waits for cycle 8 |
| 8 | [#314](https://github.com/unarbos/arbos/pull/314) `3608f49` | N=2 becomes the harness default — **never reached `main`** (#314 was merged from the branch state before that commit) and, after cycle 10, is not adopted | attribution on one kernel: N=1 23/32, N=2 15/18; like-for-like 13/18 → 15/18; pooled over cycles 6–8 on the same instances N=1 45/64 (70%) vs N=2 42/49 (86%); the kernel-base change alone moved 22/32 → 23/32 |
| 9 | [#347](https://github.com/unarbos/arbos/pull/347) | `max_turn_cost_usd` / `ARBOS_MAX_TURN_COST`: a turn past its dollar cap ends with a notice (kernel default none; harness $8 after a $4 trial); `ARBOS_CHANGES_BEFORE_DONE` nudges a final reply after edits to run `changes` once (opt-in) | cap: both arms reached 22–25 rollouts per $27 (cycle 8: 18), three hard rollouts capped and lost; `changes` before done +2 of 20 shared rollouts (noise band); N=2's cycle-8 gain did not reproduce at $4 on the new base (12/18 on the first nine vs 15/18) |
| 10 | — (no code) | re-measure N=1 vs N=2 at the $8 cap on one kernel | tie on the shared instances (14/18 each) at 1.8× the cost; **the default stays at one reproduction**; N=2 remains a knob |

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

## Cycle 7 (2026-09-16) — one experiment: two reproductions before the first edit

**Branch** `cursor/swebench-loop-c7` on `main` `c964294` ([#314](https://github.com/unarbos/arbos/pull/314)). **Target**: right file, wrong mechanism (3 of 4 losses in cycle 6). **Lever**: `ARBOS_REPRO_REQUIRED=2` — before the first edit the agent needs two *distinct* failing reproductions: the reporter's example and a second input it derives from the request text; the refusal says which is missing. Same kernel in both arms; only the variable differs. Caps were separate this cycle: $30 regression (watcher at $27), $30 slice (watcher at $28).

**Regression 20 at `-r 2`, N=2: 28 of 32 rollouts** (stopped at its cap; 17 instances reached, 13 passed both rollouts, 15 at least once). Against the cycle-6 floor of 28/40 — and like-for-like on the same 17 instances, **23/34 → 28/32**. The four instances that were coin flips through cycles 1–6 (django-14792, 15022, 15252, pylint-8898) went **2/2 each**; astropy-13398 stayed 1/1; requests-2317 and xarray-6992 stayed 0. This is the largest regression movement of the loop. **Confound**: the kernel base also moved (`43d8569` → `c964294`: first-byte replacement, refused-family memory, markup stripping), and there is no N=1 regression run on `c964294`, so N=2 cannot yet be separated from those. Cost $0.87 per rollout (cycle 6: $0.97).

**Slice 7** (30: 12 easy / 16 medium / 2 hard): run A (N=1) **26/30**, $19.07, median 32 calls. Run B (N=2) reached 13 of 20 before the slice cap: **10/13**, A on the same 13: 11 — one flip against (django-11265), two losses shared (django-11532, django-13195, both wrong mechanism). Inside noise. Compliance: 13/13 run-B rollouts recorded a second reproduction; 24 second-reproduction refusals over 13 rollouts (1.8 each — the agent's first instinct is still to edit after one reproduction).

**Incident**: when the regression watcher fired it removed *all* containers and the slice run-B eval died in the same minute (its two in-flight rollouts ended with kernel exit 2 as the interception server vanished); run B was resumed with `--resume`. The watcher now sends SIGINT and leaves container cleanup to verifiers. Run A's per-batch cap check also let a third batch start at $13.38 against a $14 cap, which is why run B only had budget for 13.

**Spend**: $58.28 — regression $27.82 (under its cap), slice $30.47 ($0.47 over, in flight).

## Cycle 8 (2026-09-16) — attribution: one reproduction vs two, same kernel

Both arms on kernel `c964294` + gate N (PR #314), regression 20 at `-r 2`, $30 each, polite SIGINT watcher at $27 (it fired for both; no container outside the stopped run was touched).

| Arm | Rollouts before cap | Solved | Cost | Instances reached |
|---|---|---|---|---|
| N=1 | 32 | **23** (72%) | $27.19 | 16 |
| N=2 | 18 | **15** (83%) | $29.88 | 9 (django-15252 alone cost $14.46) |

Like-for-like on the 9 instances with two rollouts in both arms: **N=1 13/18, N=2 15/18** (django-14792 1→2, django-15022 0→1). Pooled with cycle 7's N=2 run on the same set: **N=2 42/49 (86%) vs N=1 45/64 (70%)** — the N=1 pool is cycle 6's 22/32 on kernel `43d8569` plus cycle 8's 23/32 on `c964294`, so the kernel base change between those cycles moved one rollout; the second reproduction moved the rest. That settles cycle 7's confound.

**Decision**: two failing reproductions before the first edit is the harness default (`3608f49`). Kernel default behaviour is unchanged (opt-in by env). The price: N=2 rollouts cost more ($1.66 vs $0.85 per rollout here; $0.87 in cycle 7), and one rollout ran away to $14 — a per-rollout spend cap in the harness is the next infrastructure item.

**Spend**: $57.07 (N=1 $27.19, N=2 $29.88), both under their caps; the N=2 arm's last recorded rollout carried it from $16.45 to $29.88 in one step, which is why the watcher's threshold must account for a single expensive rollout.

## Cycle 9 (2026-09-16) — the cost cap, then `changes` before done

**Kernel** `main` `e183793` (interrupted tools recorded on restart, failed-write nudge, over-budget report) + this cycle's two additions ([#347](https://github.com/unarbos/arbos/pull/347)); both arms on that one kernel. **Built first**: `max_turn_cost_usd` — a turn past its dollar cap ends with a notice naming the cap; harness knob, `result.json.cost_capped`, metric. **Experiment**: `ARBOS_CHANGES_BEFORE_DONE=1` — a final reply after edits without a `changes` since the last edit is nudged once; `changes` is where the recorded reproductions are re-run, and in cycle 6 most rollouts never called it.

| Arm | Rollouts | Solved | Cost | Capped |
|---|---|---|---|---|
| A: N=2 (harness default), cap $4 | 25 | **14** (56%) | $27.16 | 2 |
| B: A + `changes` before done | 22 | **15** (68%) | $25.86 | 1 |

Shared (10 instances × 2 rollouts): A 12/20, B **14/20** — +2, inside the band, cost neutral; B's extra solves are astropy-13398 1/2 and django-14792 1/2 where A had 0/2. **The cap worked as an instrument**: 22–25 rollouts per $27 where cycle 8's N=2 arm managed 18, no rollout above $4.10. **And it cost score**: all three capped rollouts were on hard instances that had solved at $6–$14 in cycles 7–8 (astropy-13398 twice, django-14792), and all lost. Arm A also sits well under cycle 8's N=2 on the same first nine instances (17/18 in cycle 7, 15/18 in cycle 8, **12/18** here): part cap, part another kernel-base move, part the variance this set carries. The harness cap default is therefore **$8** (`dee617e`), and N=2 is re-measured at $8 in cycle 10 before anything more is claimed about it.

**Spend**: $53.02 (A $27.16, B $25.86), both under their caps; the watchers stopped both runs politely (SIGINT) and touched nothing else. Arm B's eval sat 30 minutes on two idle containers after its 22nd rollout (no grader, no kernel process inside) before I stopped it — a verifiers-side hang to watch for.

## Cycle 10 (2026-09-16) — does two reproductions hold at the $8 cap?

One kernel (`main` `90a33cb`, before #349's cap-ordering fix), regression 20 at `-r 2`, N=1 vs N=2, $8 cap, $30 each with polite watchers.

| Arm | Rollouts before cap | Solved | Instances reached | Cost | Per rollout |
|---|---|---|---|---|---|
| N=1 | 35 | **26** (74%) | 18 | $27.26 | $0.78 |
| N=2 | 18 | **14** (78%) | 9 | $32.18 | $1.79 |

On the nine instances with two rollouts in both arms: **N=1 14/18, N=2 14/18.** A tie, at 1.8× the cost per rollout — N=2 spent its whole budget on the first nine instances (django-15252 $8.85, astropy-13398 $8.32, one rollout capped) while N=1 covered eighteen. Across cycles, N=2 on those first nine: 17/18, 15/18, 12/18, 14/18; N=1: 13/18, 13/18, 14/18. Cycles 7–8's gap has closed on the cleanest comparison so far, and what remains is the cost.

**Plainly: two reproductions does not hold at the $8 cap.** The harness default stays at **one** reproduction — it turns out it was never changed on `main` (#314 was merged from the branch state before the N=2-default commit landed), so this is a decision not to adopt, recorded here and in the history file, rather than a revert. `repro_required=2` stays available as a knob. ~~The N=1 arm's 26/35 (74%) is above the cycle-6 floor of 70% on a kernel two bases newer, which is the loop's real current baseline.~~ Wrong — see the correction at the top: 6 of those 26 solves downloaded the upstream fix; the run was on the open network.

**Notes**: the kernel predates #349, so the one capped N=2 rollout has its cap-crossing step missing from the transcript (the grade is from git and unaffected). The VM froze during an idle gap (19:40–21:00 UTC), so this cycle's wall times are inflated again. Spend $59.44, both arms under their own caps.

## Cycle 11 (2026-09-17) — the second-model critique, and what the network was doing

One kernel (`main` `5017ef45` plus an opt-in critique, `ARBOS_CRITIQUE=1`), regression 20 at `-r 2`, one reproduction, $8 cap, $30 per arm with polite watchers. The decision rule was written down before the runs ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/swebench/loop/cycle-11/preregistration.md)): adopt at +3 or more rollouts on the instances both arms cover twice at no more than 1.3× the cost per rollout; drop at +2 or less, or dearer than 1.3× without +4.

The lever: on the first final reply after an edit, one fresh model call with no history — the request text and `git diff HEAD` — lists each behaviour the request names, marks it addressed or not, and ends `VERDICT: COMPLETE` or `INCOMPLETE — <gap>`. On INCOMPLETE the agent is nudged once with the review. The reviewer is Sonnet 5 too: the harness pins every call to the run's model.

| Arm | Rollouts before cap | Solved | Instances reached | Cost | Per rollout |
|---|---|---|---|---|---|
| A, critique off | 29 | **20** (69%) | 15 | $27.07 | $0.93 |
| B, critique on | 15 | **12** (80%) | 8 | $27.96 | $1.86 |

On the seven instances with two rollouts in both arms: **A 12/14, B 12/14**, at 2.4× the cost per rollout. The critique fired 15 times and said INCOMPLETE four times (both astropy-12907 rollouts, both astropy-13398). In three of the four the reviewer was wrong — it asked for a symmetric `cleft` fix that the code never needed, and for a re-timing behaviour the agent had already checked — and the agent said so in one sentence and finished, solved. In the fourth the agent made two more edits and also solved; the same instance solved without them in arm A. The cost is not the review call (about $0.03 each) but the instances B happened to spend on (astropy-13398 $12.23 for two solved rollouts; django-14792 $9.33 for two failed ones, no nudge involved).

**Plainly: dropped, per the rule.** A tie inside the band, at 2.4× the price. The code is reverted on the branch (three commits and their reverts, so the experiment stays in history); nothing is kept as an opt-in.

**The finding that matters more.** Reading arm B's astropy-13398 rollouts to see how the agent answered the critique showed it installing `astropy==5.2` from PyPI — the release that carries this issue's fix — and comparing its patch against it. The docker runtime has been running every cycle with `--network host`. Counting rollouts whose bash output shows a pip download of a *newer release of the package under repair*:

| Run | Rollouts | Fetched upstream | Of those, solved | Solved without them |
|---|---|---|---|---|
| Cycle 10, N=1 (the "74%" baseline) | 35 | 6 | 6 | 20/35 (57%) |
| Cycle 10, N=2 | 18 | 6 | 6 | 8/18 (44%) |
| Cycle 11, A | 29 | 8 | 7 | 13/29 (45%) |
| Cycle 11, B | 15 | 2 | 2 | 10/15 (67%) |

The instances are the hard ones (astropy-13398, django-13449, django-14792, django-15252, pylint-8898, scikit-learn-25102), and the fetching rollouts solve at 21 of 22. The "solved without them" column is a floor, not the clean number — some of those rollouts might have solved anyway — but the baseline is not 74%. The same is true of cycles 1–9 to an unknown degree (bundles from those cycles are in the store and can be counted the same way; only cycles 10–11 were counted here). The harness told the agent it had no network; nothing enforced it.

verifiers has the enforcement already: `--env.agent.runtime.block '["*"]'` puts the container on a bridge network with iptables rejecting everything but the interception proxy. A smoke rollout on django-11099 under the cut, with the agent told to try `pip download` first: pip refused (`NewConnectionError`), the model calls went through, the kernel ran normally. Two harness changes on the branch: the documented command carries the flag, and every rollout records `arbos_egress_open` (1.0 when the runtime is unrestricted) with a warning in the log.

Two smaller things the smoke showed: a failed `pip download` was recorded as the task's reproduction (the last failing bash command counts, whatever it was), and a harness `instructions` override replaces the standing headless rules (do not commit, do not branch) rather than adding to them — the agent committed on a branch and the patch extraction saw nothing.

Spend $55.75 (A $27.07, B $27.96, smokes $0.72). Two idle containers were left behind after each arm's SIGINT stop (`sleep infinity`, no agent process, eval exited); removed by ID after checking, not swept.

## Cycle 12 (2026-09-17) — the baseline, measured with the network cut

One arm, no lever. Pre-registered before the run ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/swebench/loop/cycle-12/preregistration.md)): the graded rate on the regression 20 at `-r 2` under the cut *is* the baseline, unadjusted; soundness checks that had to hold: `arbos_egress_open` 0.0 on every rollout, zero fetches in a transcript audit, no setup errors.

**Kernel `864d6b00`** — the head of [#380](https://github.com/unarbos/arbos/pull/380) (`main` `fa17987e` + #380: a failed command is a reproduction only if it ran code; harness instructions layer under the headless rules). #380 was not merged at run time; both behaviours are fully in, none half. One reproduction, mechanism gate on, $8 cap, 2400 s, Sonnet 5, concurrency 3, `--env.agent.runtime.block '["*"]'`. Cap $40 (the cycle had spent $19 finding two faults first, below); the watcher stopped the run at $37.14.

| | Rollouts | Solved | Instances reached | Cost | Per rollout |
|---|---|---|---|---|---|
| Regression 20 at `-r 2`, network cut | 36 of 40 | **21 (58%)** | 18 of 20 | $37.14 | $1.03 |

Not reached: sympy-20590 and sympy-13878 (both solved 2/2 in every earlier cycle; they would likely have made it 25/40 = 62%, but that is a guess and the number stands at 21/36). Soundness: `arbos_egress_open` = 0.0 on all 36; the transcript audit finds no fetch; five rollouts tried pip or git and were refused; no rollout timed out. *Corrected in cycle 14:* one rollout (django-15252, the solved one, $8.09) hit the per-turn cap; the harness's detector was still matching the pre-#349 notice wording and reported 0 ([#413](https://github.com/unarbos/arbos/pull/413)).

| Instance | Result | Cost | | Instance | Result | Cost |
|---|---|---|---|---|---|---|
| astropy-12907 | SS | $0.28 | | pytest-5787 | SS | $1.74 |
| django-11099 | SS | $0.18 | | sphinx-7590 | .. | $2.69 |
| django-11133 | SS | $0.57 | | scikit-learn-25102 | SS | $3.04 |
| pytest-7432 | SS | $0.41 | | django-13449 | SS | $1.77 |
| requests-2317 | .. | $1.13 | | django-15022 | .. | $3.59 |
| scikit-learn-13142 | SS | $0.23 | | django-15252 | .S | $10.60 |
| pylint-6903 | SS | $0.38 | | django-14017 | SS | $1.52 |
| astropy-13398 | .. | $4.72 | | django-14792 | .. | $2.86 |
| xarray-6992 | .. | $0.31 | | pylint-8898 | .. | $1.12 |

**What the cut took away.** The six instances that used to fetch upstream: astropy-13398 0/2, django-14792 0/2, django-15022 0/2, pylint-8898 0/2, django-15252 1/2 (at $10.60 — one rollout ran to the $8 cap's neighbourhood), django-13449 2/2. Compared with cycle 10's open-network N=1 arm on the same instances: 13398 1/2→0/2, 14792 2/2→0/2, 15022 1/2→0/2, 8898 2/2→0/2, 15252 0/2→1/2, 13449 2/2→2/2. Six rollouts lost on the instances that used to fetch, one gained; that is most of the gap between 74% and 58%, and the rest is variance on a set this size.

**Supporting run, not the number.** Before the grading fault below was found, a run on the same kernel under the same cut produced 16 rollouts whose patches verifiers graded 0 for the wrong reason. They were re-graded afterwards with each task's own `tests/test.sh` in a fresh container (`regrade_run.sh`, `regrade-of-ungraded-cut-run.json`): **10/16 (63%)**, with 13398 0/2, 14792 0/2, 15022 0/1, 15252 0/1 and everything else 2/2. Same shape as the counted run. It is not pooled into the baseline because the grading was mine, not verifiers'.

**Two faults found on the way, both fixed on the branch:**
1. The first launch ran on `main` `fa17987e`, before #380 — a baseline across a behaviour change would not have been usable. Stopped after 2 rollouts ($0.49) when Jacob flagged it; set aside as `c12-reg-aborted-pre380`.
2. Under the cut, **every rollout graded 0 with a patch in place.** verifiers grades in the agent's container; the SWE-bench verifier's `uv run parser.py` fetches `swebench` from PyPI, the proxy denied it, `set -e` ended the script, reward 0. The harness now reopens egress after the agent has exited and before the grader runs (`prepare_execution(None)`; the agent phase stays cut). Found after 19 rollouts ($18.55); those are the re-graded 16 above.

**The harness now refuses an open network.** `setup()` raises unless the runtime's egress is restricted; the rollout errors with no score and no model spend (smoke: `ok=False`, `$0.00`). `allow_open_egress=true` overrides for debugging and the `arbos_egress_open` metric marks the rollout. A run cannot quietly produce a number on the open network any more.

**How far back it goes.** All eleven earlier cycles are counted in [`swebench-open-network-audit.md`](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-open-network-audit.md): 133 of 948 rollouts fetched the package under repair, 114 graded solved; three instances (14792, 13398, 15252) were never solved without it. Every bundle from cycle 1 on was still on the VM, so nothing had to be estimated.

Spend $56.29 (aborted pre-#380 run $0.49, ungraded run $18.55, smokes $0.11, the counted run $37.14). Orphaned containers after each SIGINT stop (2 + 2, `sleep infinity`, no agent) removed by ID.

## Cycle 13 (2026-09-17) — the baseline on 40 rollouts, the twelve clean failures, and whether the set is still the right one

**Baseline finished: 24 of 40 rollouts, 60%.** The two instances the cycle-12 watcher cut off, run under the cut on the same kernel `864d6b00`: sympy-20590 2/2 ($0.26), sympy-13878 1/2 ($9.62 — the loss ran 121 model calls and finished with a patch the hidden tests reject; first clean loss of that instance in six runs). `arbos_egress_open` 0.0, no fetch, 0 live survivors on all four. With cycle 12's 21/36, the regression 20 at `-r 2` stands at **24/40 (60%)**. This is the number. The 74% is the number that was wrong.

**The grading boundary is structural now** ([#393](https://github.com/unarbos/arbos/pull/393) merged; [#397](https://github.com/unarbos/arbos/pull/397) refines it). Before egress reopens for the verifier, the harness kills every live process in the container except PID 1 and reads `/proc` back; a live survivor keeps the cut and fails the rollout. Smoke with the agent told to `nohup sleep 100000 &`: 2 killed, 0 left, graded. Plain rollouts leave 0 live processes and 1 zombie (PID 1 is `sleep infinity` and reaps nothing) — #393 counted that zombie as a kill; #397 does not.

### The twelve clean failures, read

Six instances at 0/2 under the cut. For each: what the agent did, what the hidden tests wanted, and what kind of gap that is.

| Instance | What the agent changed | What the hidden tests want | Gap |
|---|---|---|---|
| django-14792 | `_prepare_tzname_delta` in the postgresql, mysql and oracle backends: parse the offset out of `Etc/GMT-10` and flip it correctly (both rollouts, 11–13 edits) | `django/utils/timezone.py::_get_timezone_name` returns the offset for fixed-offset zones; tested directly | **Wrong layer.** Both rollouts *named* the root cause correctly in their summary ("`_get_timezone_name()` changed in 3.2 to return the full name") and then fixed the three consumers. One rollout passed the mechanism gate with the literal text `placeholder`. |
| django-15022 | Rollout 1: combine the per-word `Q`s into one `filter()` but keep chaining for multi-valued lookups. Rollout 2: `Exists()` subqueries for multi-valued lookups | Gold: `queryset.filter(Q(*term_queries))` once — the maintainers accepted that a multi-word search over a multi-valued relation now matches within one related row | **Deliberate conservatism lost.** The agent saw that the simple fix changes semantics and preserved them; the maintainers changed them. |
| pylint-8898 | A depth-aware CSV splitter that respects `()`, `[]`, `{}` and backslash escapes (both rollouts) | `test_csv_regex_error`: `(foo{1,}, foo{1,3}})` must error with `"(foo{1,} beginning at index 0"` — the gold splitter tracks only `{}`, so it splits at the comma and the *first half* errors | **Hidden test pins an incidental detail.** The agent's splitter is the more thorough one; it produces a different error string on malformed input. |
| sphinx-7590 | User-defined literals in the C++ parser, both rollouts, with `get_id` mangling `cl` + `li` + ident + literal + `E` | `test_expressions` checks the exact mangled ID `clL_Zli{ident}E{literal}E` | **Hidden test pins an unspecified output.** The issue says nothing about ID mangling; the Itanium ABI form the gold uses is knowable but not stated. |
| xarray-6992 | One line in `reset_index`: subtract `drop_variables` from `_coord_names` (both rollouts; 12–16 tool calls, the cheapest failures in the set) | 12 tests across `reset_index`/`set_index`, including `test_reset_index_drop_convert[...]` — the gold reworks `dataset.py` and `indexes.py` (8.9 kB) | **The issue is one symptom of a redesign.** The agent fixed what was reported; the tests grade the redesign. |
| astropy-13398 | A new `itrs_observed_transforms.py` from the issue's draft (both rollouts), registered in `__init__.py` | `test_itrs_topo_to_altaz_with_refraction`, `..._hadec_with_refraction`, `test_cirs_itrs_topo`: the gold also gives `ITRS` a `location` attribute and applies refraction | **The accepted PR grew past the issue text.** Refraction and topocentric ITRS are in the tests and not in the issue. |

**Wrong mechanism: 0 of 12.** With the copied fixes gone, the class the last four cycles aimed at is not in the clean failures at all. What is: two rollouts at the wrong layer (agent-addressable, and the "fix at the root" rule from cycle 1 plainly did not hold — the agent wrote the root cause down and fixed elsewhere); two where the agent preserved behaviour the maintainers changed; and **eight of twelve where the hidden tests grade something the issue text does not determine** — an error string, a mangling scheme, a redesign, a feature's final scope. Those eight are not reachable by a behaviour lever; a stronger agent would fail them the same way, and the audit shows the only way past them so far was the upstream diff.

One gate finding from the read: `mechanism: "placeholder"` satisfied the mechanism gate (django-14792, second rollout). The gate checks length, not content. Filed in the QA note.

### Is regression 20 still the right set?

No. Across all clean rollouts of cycles 1–13 the set decomposes as: **ten instances at 100%** (11–27 clean rollouts each: 12907, 11099, 11133, 13449, 14017, 6903, 5787, 7432, 13142, 20590), **five at 0%** (13398 0/13, 14792 0/12, requests-2317 0/16 — the grader hang, xarray-6992 0/19, sphinx-7590 0/4), two near it (15022 2/19, 15252 1/7), and **two with variance** (pylint-8898 6/10, scikit-learn-25102 3/5). Thirty of forty rollouts are decided before the run starts. The set has a floor of ~14 and a ceiling of ~34 that no lever can move, and the four to six rollouts in between are inside the noise band we already refuse to read. A lever would have to be very large to show here, and cycle 12's re-baseline already told us what the twelve fixed failures are.

Proposed **regression 20b**, from the audit data, for cycle 14 to baseline under the cut (about $40 at `-r 2`):

- The 12 instances with a clean 1/2 split in cycles 1–5, never fetched: astropy-13236, astropy-14182, django-11728, django-16454, matplotlib-24870, pylint-6386, scikit-learn-14629, sphinx-8035, sphinx-8265, sympy-15017, sympy-17318, sympy-18698.
- The 2 with variance in the old set: pylint-8898, scikit-learn-25102.
- The 2 near-floor instances where the agent has solved cleanly at least once: django-15022, django-15252.
- 4 fresh medium/hard instances from `order_remaining` (never run), so the set is not entirely selected on past variance.

Keep the old regression 20 as a yearly-style check, not the loop's instrument: run it once per kernel base to see that the ten still solve and the five still do not, at $40 a time, and nothing more.

Spend $10.83 (finish $9.89, sweep smokes $0.94). Cycle total for 12+13: $67.12; the overrun is the ungraded run.

## Cycle 14 (2026-09-17) — regression 20b's baseline: 28/40, 70%

One arm, no lever; pre-registered ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-14-preregistration.md)). Kernel `main` `30eef166` (#393 merged; the kernel proper is #380's plus #390's rewind work), harness with #397's sweep. One reproduction, mechanism gate, $8 cap, network cut, `-r 2`, concurrency 3. Cap $45; the run finished all 40 at $37.48.

| | Rollouts | Solved | Cost | Per rollout |
|---|---|---|---|---|
| Regression 20b at `-r 2`, network cut | 40 of 40 | **28 (70%)** | $37.48 | $0.94 |

Soundness: `arbos_egress_open` 0.0 on all 40; transcript audit finds no fetch; every sweep reports `left` 0; every rollout `ok`. One rollout hit the $8 cap (django-15252, 134 tool calls, ended mid-work).

| Instance | Result | $ | | Instance | Result | $ |
|---|---|---|---|---|---|---|
| astropy-13236 | S. | 1.17 | | sympy-18698 | .S | 1.38 |
| astropy-14182 | SS | 1.09 | | pylint-8898 | S. | 1.71 |
| django-11728 | SS | 1.00 | | scikit-learn-25102 | SS | 2.72 |
| django-16454 | SS | 0.48 | | django-15022 | .. | 3.96 |
| matplotlib-24870 | SS | 1.85 | | django-15252 | .. | 9.34 |
| pylint-6386 | .S | 0.52 | | scikit-learn-10908 | SS | 0.24 |
| scikit-learn-14629 | .S | 1.00 | | django-13033 | SS | 1.49 |
| sphinx-8035 | SS | 3.61 | | sympy-13615 | SS | 0.93 |
| sphinx-8265 | SS | 0.90 | | pytest-6197 | SS | 1.96 |
| sympy-15017 | S. | 0.85 | | sympy-17318 | .. | 1.27 |

The instrument moves: eight instances split 1/1 or fell to 0/2 from a 1/2 history, and the four never-run instances all went 2/2 — the fresh draw was easier than intended, and cycle 15 may swap two of them for harder ones from the same order. Twelve of forty rollouts are decided (ten instances at 2/2 that were 1/2 before — variance in our favour this time, not a ceiling); the other twenty-eight can go either way.

### The twelve failures, read against gold and FAIL_TO_PASS

| Instance | What the agent did | What the tests want | Gap |
|---|---|---|---|
| sympy-17318 ×2 | Guarded `split_surds` against an empty surd list (the crash site) | Gold fixes `_sqrt_match`'s condition so `I` is never treated as a surd; the test checks `_sqrt_match(4 + I) == []` | **Root named, symptom guarded.** Both mechanism statements say the Add branch in `_sqrt_match` is "over-eager"; both patches guard downstream. |
| scikit-learn-14629 | A fallback in `_validation.py` that reads `estimators_[i].classes_` when `classes_` is missing | Gold gives `MultiOutputClassifier` a `classes_` attribute; the test checks the attribute | **Root named, consumer patched.** The issue itself pointed at the consumer; the agent followed the issue rather than the shape of the fix. |
| pylint-6386 | `_DoNothingAction` takes `nargs=0`, so `-v` no longer demands an argument | `-v` must *turn verbose on* ("Using config file" in stderr); gold maps `-v` to `_set_verbose_mode` | **Reproduction proved the crash gone, not the behaviour present.** The agent's check was "does `-v` error?". |
| sympy-15017 | Special-cased `__len__`/`__iter__` for rank-0 arrays, leaving `_loop_size` 0 | Gold sets `_loop_size` to 1 for rank 0; the test also asserts `rank_zero_array[0] == x` | **An existing test encoded the bug and the agent obeyed it.** The transcript shows the agent trying the gold fix first, seeing `assert len(rank_zero_array) == 0` fail in the existing suite, and reverting to a special case that kept the old test green. The cycle-1 rule "a test never vetoes the requested change" did not hold. |
| django-15252 (capped) | Router gating in `recorder.py` plus executor changes and new tests; ended at the $8 cap mid-work | — | **Capped.** 134 tool calls; the second rollout of the same instance (below) finished and lost. |
| django-15252 | Gated `ensure_schema`/`record_*` on `router.allow_migrate_model`, as the issue asks | Gold does not consult routers: it stops the executor creating `django_migrations` when there is nothing to migrate | **Maintainers' fix differs from the issue's ask.** |
| django-15022 ×2 | Kept per-term semantics for multi-valued lookups (Exists / pk__in subqueries) | Gold: one `filter(Q(*term_queries))`, accepting the semantic change | **Preserved behaviour the maintainers changed** (same as cycle 13). |
| astropy-13236 | The `FutureWarning` the issue proposes as step one | Gold makes the behaviour change directly; tests check the new behaviour | **Maintainers skipped the issue's proposed path.** The other rollout read it the maintainers' way and solved. |
| sympy-18698 | Combined equal-multiplicity factors, deliberately excluding multiplicity 1 | Gold combines all multiplicities; test checks `sqf_list(x*(x + y))` | **Scope the issue did not state**; the agent's exclusion was a choice, not an oversight. |
| pylint-8898 | Depth-aware splitter (parens, brackets, braces) | Exact error text on a malformed input that only a brace-only splitter produces | **Hidden test pins an incidental detail** (same as cycle 13). |

By kind: **agent-addressable 6** (root named and symptom fixed ×3, reproduction of the crash rather than the behaviour ×1, existing test obeyed over the issue ×1, capped ×1); **the issue does not determine the fix 5** (maintainers chose a different path, scope, or semantics); **incidental detail pinned 1**. Half the failures are reachable, against two of twelve on the old set — that is the difference between an instrument and a floor.

**Wrong mechanism, again: 0 of 12.** And the pattern that does recur is now in five rollouts across both sets (django-14792 ×2, sympy-17318 ×2, scikit-learn-14629): the agent writes the root cause down — in its `mechanism` argument or its summary — and then edits somewhere else: a guard at the crash site, a fallback in the consumer. The mechanism gate made the agent *say* where the fault is; nothing checks that the diff goes there.

### Also found

- **Job shells outlive the kernel.** In 4 of 40 rollouts (all scikit-learn), the sweep found 4–10 live processes after `arbos-kernel run` had exited: the kernel's own job-runner shells (`sh -c D=$1; P=$2 ... ARBOS_JOB_LOG_CAP`) with test runs still going. The network stayed cut until they were dead and every rollout graded; without the sweep they would have had the network back. Filed for the kernel: `run` should not exit with jobs running, or should kill them.
- `cost_capped` missed the cap notice since #349 reworded it: two capped rollouts (cycles 12 and 14) read as 0. Fixed ([#413](https://github.com/unarbos/arbos/pull/413)); cycle 12's line corrected above.
- **Two unlanded commits, declared.** A steward check (2026-09-17 08:54 UTC) found two of this loop's commits pushed after their PR had merged. `3608f49a` on `swebench-loop-c7` — "two failing reproductions before the first edit is the default", the cycle-8 decision — **is dead**: cycle 10 reversed it (N=2 does not hold at the $8 cap), the harness default on `main` is and was one reproduction, and every run script in this loop passes `--env.agent.harness.repro-required` explicitly (1, or `REPRO_N` for the N=2 arms), with the harness checkout for every run taken from a `main`-based branch, never from `c7`. No measurement in cycles 8–14 depended on it; the branch is deleted. `2f6f0fcf` on `swebench-sweep-zombies-7c9c` — the `cost_capped` fix — is the same change as [#413](https://github.com/unarbos/arbos/pull/413) line for line; the branch is deleted. `swebench-loop-c12-7c9c` was also removed: its one post-merge commit is in `main` by patch through #397. The habit that caused all three: pushing a follow-up to a branch whose PR the steward had already merged, which looks identical to pushing before the merge. From here, a follow-up after a merge starts a new branch off `main`, and `git cherry origin/main <branch>` over every loop branch is part of each cycle's close.

Spend $37.48.

## Cycle 15 (2026-09-17) — the mechanism-vs-diff check, and a kernel base that moved the instrument

**The lever's own ceiling, before the number.** The check aims at one pattern — the agent names the root cause and edits somewhere else — which was 3 of 12 failures on regression 20b in cycle 14 (sympy-17318 ×2, scikit-learn-14629) and 2 of 12 on the old set. If it converted every rollout in its class and lost none, the most it could move this set is +3; the pre-registered adopt threshold was +3. So a drop here means the lever could not clear a bar its own class barely reaches. It does not mean the idea was wrong. ([Pre-registration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-15-preregistration.md), written 09:40 UTC before the runs.)

**Setup.** Regression 20b v2 (scikit-learn-10908 → django-14631, django-13033 → django-15503, the next two "1–4 hours" instances never run; other 18 unchanged). One kernel, `main` `7e19f9e9` plus the opt-in check (`88049581`, kept as `lever-mechanism-diff-check.patch` in the cycle folder; the branch is gone). One reproduction, $8 cap, network cut, sweep, `-r 2`, concurrency 3, $30 per arm with watchers at $27 and a 3 h 30 wall-clock timer. The kernel on `main` no longer refuses an edit without a mechanism line (#399); the line is still volunteered in 47 of 59 rollouts across both arms.

| Arm | Rollouts | Solved | Instances | Cost | Per rollout |
|---|---|---|---|---|---|
| A, check off | 30 | 12 (40%) | 16 | $29.05 | $0.97 |
| B, check on | 29 | 13 (45%) | 15 | $29.41 | $1.01 |

**On the 14 instances both arms covered twice: A 11/28, B 12/28, +1, at 1.23× the cost per rollout.** Drop, per the rule.

What the check did in B: it fired in 7 of 29 rollouts, every one of them the "no line recorded" form (asking for the code path and listing what the diff touched); the "your mechanism names X and the diff does not touch X" form fired **zero** times. In 22 rollouts the line's identifiers were in the diff. None of the 7 nudged rollouts edited again afterwards; each answered with a line and finished. Of the lever's own class, sympy-17318 was not reached by either arm before the cap, and scikit-learn-14629 went A 1/2, B 2/2 — the +1. On this evidence the pattern "root named, fix elsewhere" is rarer at the moment of the final reply than it looked in the failure reads: when the agent has written a line, the diff usually does touch what it names, and in the five failures of that shape the line named the place the agent edited (the guard, the consumer), sometimes alongside the root. A check that asks whether the diff touches *something* the line names cannot separate those; one that asked which of the named places is upstream would need to read the code, not the string. Dropped; the code is not kept as an opt-in.

**~~The result that matters more: the kernel base moved the instrument.~~** *Corrected in cycle 16: not supported. The same `30eef166` binary re-run on the same twelve instances the next morning scored 11/24, not 16; six runs on these instances range 9–16. The paragraph is left as written because it is what cycle 15 concluded, and cycle 16 explains why it was wrong.* Twelve instances were covered twice in both cycle 14 (kernel `30eef166`) and this cycle's arm A (kernel `7e19f9e9`), same harness settings, same cut: **cycle 14 16/24, arm A 9/24** (arm B 10/24). Lost outright: matplotlib-24870 (2/2 → 0/2 in A, 0/2 in B — all four cycle-15 patches touch only `contour.py`; both cycle-14 solutions also changed `tri/_tricontour.py`), astropy-14182 (2/2 → 1/2, 0/2), pytest-6197 (2/2 → 1/2, 0/2), astropy-13236 (1/2 → 0/2, 0/2), pylint-8898 (1/2 → 0/2, 1/2). Seven rollouts on the same instances is outside the band; the arms agree with each other and disagree with cycle 14. Between the two bases the engine changed in: the mechanism gate removed (#399), the rewind checkpoint written and awaited before the turn proceeds (#405), job kills reported honestly (#407), turn errors said and folders marked failed (#408), the wipe guard reading the effective directory (#410), and `git.rs` +285 lines. No wipe refusal appears in any cycle-15 transcript; the repro-gate refusals are at cycle-14 rates. Which of these moved the set is not knowable from this data; it is cycle 16's first job, and until it is done, **no number on this kernel base is comparable with cycle 14's 70%.** *(Cycle 16: the comparison was inside the instrument's noise; see below.)* The once-per-kernel sanity check on the old regression 20 that cycle 13 prescribed would have caught this before the lever ran; it was deferred for budget. It should not be deferred again: $40 per base is cheaper than a cycle spent measuring a lever against a moved floor.

### The two failures that are not about where the diff lands

Read from the full transcripts (`read-sympy-15017.txt`, `read-pylint-6386.txt` in the cycle folder). Both are the agent believing the wrong evidence.

**sympy-15017 — the agent tried the right fix and retreated.** It found the root in minutes: `_loop_size = reduce(...) if shape else 0` in four places, the `else 0` wrong for rank 0. It changed all four (the gold fix), verified `len(Array(3)) == 1`, and ran the module's tests. One existing assertion failed — `assert len(rank_zero_array) == 0`, the bug itself — and the agent handled it exactly as the contract says: "this is the exact test case the request calls out as incorrect… leave it untouched and flag it." Then it noticed a *second* assertion in the same test: `raises(ValueError, lambda: rank_zero_array[0])`, which its fix also broke, because `_loop_size` bounds indexing too. Its reasoning at that moment, verbatim from the thinking: "The original request only asked about `len()`… this side effect breaks something I shouldn't touch." It reverted the root fix and special-cased `__len__` and `__iter__` instead — saying, one step earlier, "that feels like patching a symptom rather than the actual root cause." The hidden test asserts `rank_zero_array[0] == x`. The wrong evidence: an old assertion about a *consequence* of the same wrong value, read as intent because the issue did not mention indexing. The rule "a test never vetoes the requested change" held for the assertion that named the symptom and lost to the one that named its shadow. A sharper rule would say: when the root fix changes a behaviour an existing test asserts, ask whether that behaviour is computed from the same wrong value; if it is, the test encodes the bug too, whether or not the request mentions it.

**pylint-6386 — the reproduction proved the crash had stopped.** The issue says `-v` demands an argument and `--help` shows `VERBOSE`. The agent reproduced the error, fixed `_DoNothingAction` to take no argument, and re-ran: `-v` no longer errored, `--help` no longer showed the metavar. Then it ran `-v` and `--verbose` side by side and *saw the defect*: `--verbose` printed "Using config file"; `-v` did not. Its thinking: "Odd that `-v` didn't show the message like `--verbose` did — that discrepancy needs checking, maybe ordering or caching." It re-ran `-v` alone, saw again no message, and wrote: "Good — consistent now." It had compared `-v` with `-v`. Then: "Both symptoms from the report are fixed." The hidden test asserts "Using config file" after `-v`. The wrong evidence: the absence of the error, taken as the presence of the behaviour; and a re-run that reproduced the defect, read as consistency because it matched its own previous run rather than the reference (`--verbose`) it had just been shown. The lesson for the reproduction gate is that "exits non-zero, then exits zero" is evidence a crash stopped, not that a feature works; a reproduction of a *behaviour* has to assert the behaviour.

Both share a shape with what this loop found in itself this week: a check passed against the wrong reference, and the passing was recorded as evidence.

### Also

- Soundness: egress 0.0, no fetch, `left` 0 in every sweep, both arms. One capped rollout (A, django-15252).
- The two swapped instances: django-14631 4/4 (easy after all), django-15503 0/4.
- Four idle containers after the watchers' SIGINT (`sleep infinity`, no eval process); removed by ID.
- Spend $58.58 (A $29.05, B $29.41, smoke $0.12).

## Cycle 16 (2026-09-17) — the base did not move; the instrument's noise did

Pre-registered at 11:20 and amended at 11:35 after the engine's author read the five merges ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-16-preregistration.md)). The plan became: rule #410 out by grep; test #399 directly on today's kernel with [#440](https://github.com/unarbos/arbos/pull/440)'s restored gate on and off; keep the `30eef166` re-run as the anchor; #405 withdrawn (its write-wait is #419, after the range).

**#410 ruled out.** `bash: refused` appears zero times in all 34 cycle-15 failures and in every cycle-14 and cycle-15 rollout; 9–10 `rm -rf` commands per run, none refused.

**Three runs, the same twelve instances at `-r 2`, network cut, one reproduction, $8 cap:**

| Run | Kernel | Solved / 24 | Cost |
|---|---|---|---|
| Cycle 14 (for reference) | `30eef166` | 16 | $27.70 |
| Cycle 15 arm A (for reference) | `7e19f9e9` | 9 | $19.69 |
| Cycle 15 arm B (for reference) | `7e19f9e9` + dropped lever | 10 | $24.81 |
| **Confirm** | `30eef166`, the cycle-14 binary itself | **11** | $27.08 |
| **Gate off** | `1e9be864` (#440 head, `main` `7017eb75`) | **13** | $32.20 |
| **Gate on** (`ARBOS_MECHANISM_REQUIRED=1`) | same | **15** | $27.93 |

Soundness on all three: egress 0.0, no fetch, `left` 0 in every sweep; one capped rollout (gate off). Upstream provider on every call in every run since cycle 12: "Claude Platform on AWS" — no provider switch behind the endpoint.

**Confirm: the gap does not hold.** The pre-registered rule said ≤ 11 means cycle 14's 16 was the outlier. The same binary, the same instances, the same harness, one morning apart: 16 then 11. Ten of the twelve instances flipped at least once across the six runs; only django-15022 (0/12) and astropy-13236 (1/12) are stable.

**Gate: +2, inside the band.** Gate on 15, gate off 13, at a lower cost per rollout with the gate ($1.16 vs $1.34). The rule said ≤ +2 does not explain the drop; it does not — there was no drop to explain. Nor is this evidence the gate helps: +2 is what two identical arms produce. What the run does establish: with the gate on, every rollout again states a line before its first edit (refusals recorded in 8 of 24 rollouts, all then satisfied), and nothing about the outcome distinguishes that from the gate-less arm. "Useless as evidence" and "harmless to remove" are different claims; on this data both stand, and neither is proven.

**What was wrong in cycle 15.** The ±2 band. Six runs of this set give 16, 9, 10, 11, 13, 15 — standard deviation 2.8, against a binomial expectation of 2.0 at the observed per-instance rates. A 7-rollout gap between two single runs is under two standard deviations of their difference. Cycle 15 read it as a kernel regression because the loop's band said anything past 2 was real; the band was set in cycle 9 by eye from one tie and never checked against repeated runs of the same configuration. This is the third standing finding at the top of the document, and it reaches back: every lever decision since cycle 8 was made with a threshold of about one standard deviation.

**What it means for the levers already dropped.** Nothing changes for their fate — a result inside the noise was dropped, and inside a wider band it stays dropped — but the *reason* changes. The write-ups said "the lever did not move the set"; the honest statement is "the loop could not have seen it move unless it moved by 20 points". For N=2 reproductions (cycles 7–10), the `changes` nudge (9), the critique (11), and the mechanism-vs-diff check (15), the measured deltas of +1 to +2 on 18–28 shared rollouts are all consistent with both "no effect" and "a real 5–10 point effect". The loop has not tested any of them at the resolution its own claims required.

**#399, #407, #408.** Not shown to have moved anything, because nothing moved. The bisect is off. #440 is the features agent's call; the loop's evidence is that the gate neither helps nor hurts at the resolution of a $30 arm.

Spend $87.21 (confirm $27.08, gate off $32.20, gate on $27.93). Jacob said to spend what it takes; it took $87 to learn that the previous $58 measured noise.

## Cycle 17 (2026-09-17) — the loop changes what it does: reading, not measuring

**Decision (Jacob, 12:43 UTC): option (b).** The loop stops measuring levers and spends its budget reading failures and writing rules. One door stays open: a change that should plausibly move 20 points is measured, pre-registered, with the band stated from data. Two habits from here: every pre-registration states the band (≥ 7 of 24 or ≥ 8 of 40 rollouts between two arms, from the six-run estimate), and a struck number stays visible.

### On the instrument: forty instances once, or twenty twice?

Jacob's proposal: forty instances at one rollout each, for the same money, gives twice the independent draws and removes the within-instance correlation. Checked against the data before settling: across the six runs of the twelve shared instances there are 72 same-run pairs. Under independence at each instance's pooled rate, 24.2 of them should split (one solved, one not); **24 did**. The within-pair correlation is **0.01**. Two rollouts of the same instance in the same run behave as independent draws from that instance's rate, so for a *fixed set of instances* 20 × 2 and 40 × 1 have the same binomial variance — the count that matters is rollouts, not instances. The excess variance the loop actually saw (observed SD 2.8 against binomial 2.0) is between runs, not within them: something that moves every instance in a run together, most likely the model behind the endpoint drifting across a morning. Forty-once does not remove that; running both arms interleaved in time, as the loop already does, is the only guard the harness has.

So the power claim does not hold on this data, and I disagree with it on that point only. The coverage claim does: forty instances sample the distribution twice as widely, which matters now that ten of twelve are flippy and the set's rate is a property of which instances were drawn. And for *reading*, which is what the loop now does, pairs are the better shape — the 26 same-run split pairs below are the cleanest evidence the loop has ever had, because the two trajectories differ in the agent's choices and nothing else. Recommendation: keep `-r 2` for reading; if the open door is ever used, run the measured arms on forty instances at `-r 2` (80 rollouts, ~$75 an arm), because halving the band needs four times the rollouts, not a different split of the same number.

### The 26 split pairs, read

The paid-for corpus: 171 honest rollouts from cycles 14–16 on the 20b/shared-12 instances, 79 failures, and **26 pairs where the same instance in the same run solved once and failed once**. Read side by side (files edited, mechanism, reproduction, tests run, final summary; `c17-splits.txt` in the cycle folder). Two patterns account for 16 of the 26:

**A. The twin (8 pairs).** The solved rollout fixed the defect and its sibling; the failed one fixed the defect. django-11728 ×4: every solved rollout also fixed `replace_unnamed_groups` ("the identical pattern"); every failed one fixed `replace_named_groups` alone — and the failed one was the shorter trajectory in all four pairs (13–23 calls against 26–44). matplotlib-24870: the solved rollout also changed `tri/_tricontour.py`. astropy-14182 ×3: solved rollouts handled the RST reader as well as the writer; failed ones changed `write()`'s separator index only. The hidden tests cover the twin; the request shows one side.

**B. Producer, not consumer (8 pairs).** The solved rollout gave the thing that was missing to the class or path that should have it; the failed one taught the caller to cope. scikit-learn-14629 ×4 — **four of four**: every solved rollout added `classes_` to `MultiOutputClassifier` ("mirroring `ClassifierChain`", which has it); every failed one added a fallback in `_fit_and_predict`. pylint-6386 ×2: solved routed `-v` through `_preprocess_options` like `--verbose`; failed made the action take no argument (the crash gone, the behaviour absent — cycle 15's evidence failure, seen again from the other side). django-15252 ×2: solved moved up to `executor.py`; failed gated the recorder the issue pointed at. This is the class the dropped lever aimed at, now seen in pairs: the same agent, the same instance, the same run, takes either path. What separates them in the transcripts is whether the agent looked at a *sibling* — `ClassifierChain`, `--verbose`, the executor — before choosing where to put the fix.

**C. Maintainers' incidental detail (5 pairs):** pylint-8898 ×3 (the solved ones split on braces only, one of them by writing the gold's own `_check_regexp_csv` in `utils`; the failed ones were depth-aware and produced a different error string), sympy-18698 (all multiplicities vs all but 1), astropy-13236 (the solved rollout checked the checkout's version — "already at 5.2.dev" — and made the change the issue scheduled for 5.2; the failed one added the warning the issue scheduled for 5.0). **D. Other (3):** django-16454, sphinx-8035, sympy-15017 — where the solved rollout edited the existing test to assert the new behaviour, against the contract, and was graded solved anyway.

**Proposed rules, to the features agent** (`internal/features-inbox/2026-09-17-swebench-twin-and-producer-rules.md`):

> *The twin.* Before you finish, look for the defect's twin: the sibling function with the same loop (`replace_unnamed_groups` beside `replace_named_groups`), the other front end (`tri/` beside the main module), the reader when you fixed the writer. grep for the pattern you changed; if the twin has it, fix it in the same change. The request shows one side; the tests cover both.

> *Producer, not consumer.* When a caller fails because a class lacks an attribute, or a path lacks a case, that its siblings have — `ClassifierChain` has `classes_`, `--verbose` reaches the callback — the fix is in the class or the path, made the way the sibling does it. A fallback in the caller passes your reproduction and fails the tests that check the class.

Both are visible in a transcript read: does the agent grep for the twin; does it open the sibling. The next reading pass checks the remaining 53 failures (no solved partner) against the same two patterns, then the two evidence rules already filed. No model spend this cycle.

## Cycle 18 (2026-09-17) — the 53 unpaired failures, and a fifth pattern

Read against the four patterns from cycles 15 and 17 — **A** the twin, **B** producer-not-consumer, **E1** an existing test encoding the bug obeyed over the request, **E2** a reproduction that proves the crash gone rather than the behaviour present — plus **C**, the maintainers' choice the issue does not determine, which is not an agent pattern. For each instance the gold patch and FAIL_TO_PASS list were read against every failed rollout's files, mechanism line, reproduction command and final summary (`c18-unpaired.txt` in the cycle folder). Unpaired failures are a weaker instrument than pairs: **evident** below means the specific trace is in the transcript (the diff misses a file the hidden test exercises; the reproduction asserts only an exit code; the mechanism names the root and the diff sits downstream); **consistent with** means the shape fits and nothing in the transcript contradicts it, but the trace is not there.

| Instance | Unpaired failures | Pattern | Evident or consistent | The trace |
|---|---|---|---|---|
| matplotlib-24870 | 6 | **A** twin | evident | all six changed `contour.py` only; `test_bool_autolevel` asserts `tricontour`/`tricontourf` levels; the gold and all four solved rollouts changed `tri/_tricontour.py` |
| astropy-14182 | 2 | **A** twin | evident | both changed the writer only (13 and 19 calls); `test_rst_with_header_rows` is a round-trip through `QTable.read(..., header_rows=...)` |
| sympy-17318 | 2 | **B** producer | evident | both mechanism lines name `_sqrt_match`'s Add branch as over-eager; both diffs guard `split_surds` downstream |
| sphinx-8265 | 1 | **B** producer | consistent | the fix's logic went into `util/inspect.py::_unparse_default_value` (the caller); `test_unparse` tests `pycode/ast.py` directly |
| pylint-6386 | 2 | **E2** crash-only reproduction (B secondary) | evident | both reproductions are `pylint ... -v; echo EXIT:$?`; both fixes make the action take no argument; neither checks that `-v` turns verbose on |
| astropy-13236 | 10 | **F** — new, below | evident | all ten added the `FutureWarning` the issue schedules for 5.0; the checkout is 5.2.dev; the gold and the one solved rollout make the 5.2 change |
| django-15022 | 12 | C: maintainers changed semantics | evident | all twelve preserve per-term semantics for multi-valued lookups (subqueries, `Exists`, per-term joins); gold ANDs the terms in one `filter()` and the test patch rewrites the assertions |
| django-15252 | 7 (+1 capped) | C: maintainers' design differs from the issue's ask | evident | all seven gate the recorder the issue names; gold stops the executor creating the table when nothing is to migrate. (Both *solved* rollouts of this instance, in cycle-17's pairs, went up to `executor.py` — so a B reading is also available.) |
| pylint-8898 | 4 | C: pinned incidental detail | evident | all four depth-aware splitters; the hidden test pins the error text a brace-only splitter produces |
| django-15503 | 4 | C | consistent | all four in the gold file with the right mechanism (numeric keys as object keys); the failing detail is not visible without the test's SQL |
| pytest-6197 | 2 | C | consistent | both in the gold file, moving the eager `__init__.py` import; the failing detail is not visible |
| django-15022 (one rollout, `5b4690c4`) | — | **G** — observed once, below | evident | no fix made: the agent researched the ticket's upstream history, found the historic patch "tried and reverted", and declined to change the code |

**The fifth pattern, F: a staged request, and the checkout decides the stage.** astropy-13236's issue proposes two steps: warn in 5.0, change the behaviour in 5.2. Ten of eleven failed rollouts implemented the warning; the hidden tests want the change; the checkout's version is 5.2.dev, and the one solved rollout said so ("since this checkout is already at 5.2.dev, I applied the 5.2 behavior directly"). The agent's reference for "which stage are we at" was the issue's text, not the tree in front of it — the same family as the two evidence rules (a check against the wrong reference) and a distinct, rule-able behaviour:

> When a request schedules work across versions or stages, the checkout decides the stage: read the package version in the tree before choosing which step to implement. A warning the request scheduled for an earlier version is the wrong change in a checkout that is already at the later one.

**Observed once, not a pattern: G, the agent declined the task.** One rollout investigated the ticket's history upstream, concluded the fix had been tried and reverted, wrote a report, and changed nothing. On this benchmark that is a failure; in a real repository it might be the right call. Recorded as seen, not as a rule.

**Does everything fit?** Every one of the 53 fits A, B, E2, F, C or the single G; nothing needed a further category, and E1 does not appear outside its one paired case (sympy-15017). Two of the maintainers'-choice rows (django-15503, pytest-6197) are consistent-with rather than evident, so the claim is: **the five agent patterns plus the maintainers' choices are a complete account of how this agent fails on this set, with six of 53 rollouts fitting only by shape.** That is a strong claim and it is made here explicitly, on 79 failures across cycles 14–16, so that the next contrary case is noticed as one.

Two more rollouts satisfied the mechanism gate with junk (`placeholder - will refine`; `test scaffold: probing...`) — the cycle-13 finding, now three cases.

### What a fix would have had to change: the ranking

Over all 79 failures (26 paired + 53 unpaired), primary classification, no double counting (pylint-6386's four are E2 with B as the secondary reading):

| Pattern | Rollouts a rule would have had to change | Instances |
|---|---|---|
| **A** the twin | **16** | matplotlib-24870 (7), astropy-14182 (5), django-11728 (4) |
| **F** the checkout decides the stage | **11** | astropy-13236 (11) |
| **B** producer, not consumer | **9** | scikit-learn-14629 (4), django-15252 pairs (2), sympy-17318 (2), sphinx-8265 (1) |
| **E2** reproduction of the behaviour, not the crash | **4** | pylint-6386 (4) |
| **E1** a test that encodes the bug | **1** | sympy-15017 |
| C, not agent-addressable | 34 | django-15022 (12), django-15252 (7), pylint-8898 (7), django-15503 (4), pytest-6197 (3), sympy-18698 (1) |
| other / declined / capped | 4 | django-16454, sphinx-8035 pairs; one declined; one capped |

Forty-one of 79 failures are agent-addressable by five rules; 34 are not addressable by any rule that fixes the agent. If the five rules worked perfectly the set would move from 92/171 to about 133/171 — 54% to 78% — which is the ceiling of what contract work can do here, and it is above anything a lever has been credited with. **The twin is the rule to build first**: it is the largest class, it is evident in all sixteen (the hidden test exercises code the diff never touched), and it is the cheapest to state. F is second and is one sentence. B is third and hardest, because it needs the agent to open a sibling before it chooses.

The counts are what a rule *would have had to change*, not what it will; whether a rule changes it is a transcript read after the rule lands, and that read must check the pattern could still show itself — a twin the agent no longer has to look for because the prompt now names it is not the same as an agent that looks.

**A caution for the reads to come**, from QA's afternoon: a scenario passed because the fix made the fault impossible to inject, not because the fault was handled. The reading equivalent is a failure that stops appearing because the transcripts changed shape. When any of the five rules lands and five rollouts are read, the check is: could the pattern still have shown itself — did the agent reach the point where it would have skipped the twin, or reached for the fallback, and choose otherwise? A rule that removes the choice from the transcript proves nothing about the agent.

No model spend this cycle.

### Can each arm's binary prove its label? (asked by Jacob after QA's control was rebuilt underneath it, 14:42 UTC)

Checked the same afternoon, before any next measurement. Two questions, two answers.

**Does anything else write to the path an arm's kernel lives at?** No. Every kernel the loop has run is its own file under `/tmp/swe/loop/arbos-kernel-<tag>`, produced once by `docker cp` out of an image built for it, and the harness copies those bytes into each rollout's container at setup — so a rollout's kernel cannot change under it. Both arms of every A/B read the *same* file by design (one kernel per comparison). No loop step builds into a path an arm reads from. The one in-place rebuild — `arbos-kernel-c11`, after the smoke and before the arms, when the critique's clipping was fixed — was recorded at the time. The bisect builds of cycle 16 went to their own names and were never run.

**Can the binaries prove their labels?** Cycles 1–10: yes. Those kernels were host builds; `--version` reports a sha, and every one matches its role (c4b = c5a = `6d452f51`, the mechanism commit; c7 = `d7a59534`, the N-reproduction gate; c9 = `c5ce595c`, the cost cap; c10 = `90a33cb2`). **Cycles 11–16: no.** From cycle 11 the kernels were built in Docker from a context that excludes `.git`, and every one of them says `arbos-kernel 0.2.0 unknown protocol 1`. Their labels rest on file names, the `BUILD_OK <commit>` lines in the build logs (cycles 12-380, 16), and the tmux build commands in this loop's transcript (cycles 11, 12, 14, 15). That is documentary evidence, not proof from the artefact. So: no past arm is *known* to have measured the wrong kernel, the file discipline makes it unlikely, and for cycles 11–16 the binaries cannot rule it out. The full inventory — every kernel file, its sha256, what it says it is, what it was meant to be, and the evidence — is `media/swebench/loop/kernels-manifest.json`.

**Fixed so the next reader does not have to take the loop's word** ([#477](https://github.com/unarbos/arbos/pull/477)): `build.rs` takes `ARBOS_GIT_SHA` from the environment when `.git` is absent; the Dockerfile passes it as a build argument; the harness runs the kernel's `--version` at setup, hashes the bytes it installs, writes `kernel-identity.json` into every rollout's artifacts, and — given `kernel_sha` in the run config — refuses a binary that says anything else, `unknown` included. Smoke: a run labelled `deadbeef0000` against a `3f114bf50447` binary is refused at $0; the right label runs and the artifact carries version, sha, sha256. The loop's build script now names kernels by their sha under `kernels/`, checks `--version` against the commit before keeping the file, and makes it read-only; run scripts pass `kernel-sha`. From here a measured run's label is proved by the binary or the run does not start.

## Cycle 19 (2026-09-17) — two rules landed; the first read of whether the choice changed, and the completeness claim tested out of sample

**Conditions.** #449 put E1 and E2 into the contract (an assertion computed from the same wrong value encodes the bug too; a reproduction asserts the behaviour and the reference is the one the request names, never your own previous run). Kernel `3f114bf50447` = `main` `2f9a0427` (#449 and #440 in) plus [#477](https://github.com/unarbos/arbos/pull/477)'s build plumbing — the first run whose kernel proves its label: every artifact carries `kernel-identity.json` with `arbos-kernel 0.2.0 3f114bf50447 protocol 1` and the binary's sha256, and the run would have refused any other. Network cut, egress 0.0 on all ten, sweep clean. Pre-registered as a read, not a measurement ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-19-preregistration.md)): five rollouts each on the two carrying instances, read for whether the agent reached the point of the old choice and what it did there. Spend $7.38. Eight of ten graded solved; on five rollouts that number means nothing and is not the finding.

**Could the pattern still show itself?** Yes, in both instances — the check QA's afternoon made necessary. For sympy-15017 (E1), four of five rollouts reached the root fix (`_loop_size`), ran the existing suite, and met the second assertion (`raises(ValueError, lambda: rank_zero_array[0])`); the fifth edited through `sed`, left no thinking on it, and solved. For pylint-6386 (E2), all five reached a working `-v` with `--verbose` available as the reference. The rules did not remove the choice from the transcript.

**At the point of choice, sympy-15017.** Three of the four kept the root fix. One of them used the rule's own words: "since this assertion is computed from the same wrong value I'm correcting, I should make the fix but leave the test as-is, flagging it as one that asserts a consequence of the same wrong value" — the mark E1 was written to leave. One rollout made the old choice, and it is the interesting one: it *asked the rule's question* ("whether that test reflects intentional design or was just relying on the same bug") and answered it by consulting numpy — where integer-indexing a 0-d array does raise — concluded the assertion was design, kept `a[0]` raising, and failed the hidden test that asserts `a[0] == x`. The rule was followed; the reference it reached for was a reasonable one; sympy's maintainers chose to diverge from numpy. In cycles 14–16 this instance retreated in one of two rollouts; here one of four. Not a measurement. What the read establishes: the question is now being asked where before it was not, and a wrong answer to it is still possible.

**At the point of choice, pylint-6386.** Four of five compared `-v` with `--verbose` in a single command after the fix; two named the reference in their reasoning; four fixed the routing (`_preprocess_options`) rather than the argument count. The fifth made the old choice — `_DoNothingAction` takes no argument, crash gone, verbose still off — and the transcript shows how: it ran `--verbose` against `-v` *before* the fix, to reproduce, and after the fix compared `--verbose` with `--verbose=True` and `--enable-all-extensions`, never `-v` with `--verbose` again. The reference was seen and not used at the moment that mattered. In cycles 14–16, 4 of 12 rollouts on this instance failed this way; here 1 of 5. Not a measurement either; the mark is there in four transcripts.

**The completeness claim, out of sample.** Cycle 18 claimed that five agent patterns plus the maintainers' choices account for every honest failure on regression 20b. Tested on material the patterns were not built from: the cycle 1–5 slices (older kernels, 250 fresh instances), keeping only the 89 failed rollouts that the audit shows made no network fetch of any kind, and reading one per instance — 24 instances, none of them in 20b except three that recur (`c19-oos.txt` in the cycle folder). All 24 fit: **A twin 8** (django-13512 — the admin display path beside the form field, evident; django-11400 — the reverse-related field's `get_choices`, evident; sphinx-9461 — the domain directive beside autodoc, evident; django-15629 — the schema-alteration path beside the FK, evident; sympy-13798, sympy-16597, django-11734, django-12406 consistent), **B producer 6** (matplotlib-23476 — the canvas patched instead of the Figure's `__setstate__`; requests-5414 — the adapter catching what `prepare_url` should refuse; pylint-4970 — `process_module` instead of `Similar.run`; sphinx-8265 — `_pseudo_parse_arglist` instead of the unparser; sympy-17318 and scikit-learn-14629 again; all evident), **C 9** (mostly consistent-with: the failing detail not visible), and **G 1**: django-10097, where the issue quotes RFC 1738 — `:`, `@` and `/` in user and password must be encoded — and the agent's fix "still permits `:`" in the password, on its own reading. E1, E2 and F did not appear, which on old kernels without reproduction records is expected for E2 and says nothing either way.

G has now been seen twice — once declining a ticket on its upstream history, once overriding a request's explicit statement with its own reading — and gets a name: **the agent argues with the request.** Two cases are not a pattern; they are a candidate, recorded so the third is noticed. Twin and producer together are 14 of 24 out of sample against 25 of 79 in sample — the same shape, on different instances and an older agent. The claim stands, with the same qualifier as before: some of C is consistent-with, not evident.

## Cycle 20 (2026-09-17) — the last three rules landed; the read of whether the step is taken, the line written, and the choice changed

**Conditions.** A (the twin), F (the checkout decides the stage) and B (producer, not consumer) reached `main` in 0245425e and 7bf12e68, each written with a two-part mark: a visible step (a grep for the twin; a version read; the sibling opened) and a line in the reply ("twin: none found (grepped …)" / "checkout is at <version>, so I implemented <step>" / "producer: <Class>.<attr> added the way <Sibling> has it"). Kernel `2c8d8791f6af` = `main` head with #477 in — built in the loop's worktree, named by its sha, read-only, and proving its label in every artifact. Network cut, egress 0.0 on all 25, sweep clean. Pre-registered as a read ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-20-preregistration.md)): five rollouts on each carrying instance, three questions per rollout. $13.52. Twenty-three of 25 graded solved — reported, not the finding.

**Could the old choice still have been made?** Yes, everywhere. The issue texts are what they were; the twin is still off the request's page; astropy-13236 still describes two stages; scikit-learn-14629's issue still points at the consumer. And the old choice *was* made once: astropy-14182 `92ed2f61` (11 tool calls) fixed the writer only, ran a round-trip it called correct, and failed — the pattern can still show itself, so the read means something.

| Rule, instance | The step (seen in the transcript) | The line (in the reply) | The choice | The same choice, cycles 14–16 |
|---|---|---|---|---|
| **F**, astropy-13236 | version read with bash **5/5**, before the edit in every case | "checkout is at 5.2.dev64, so I implemented the 5.2 behavior" **4/5** | removed the auto-view (the 5.2 step) **5/5**; no `FutureWarning` in any patch | 1 of 12 |
| **B**, scikit-learn-14629 | `ClassifierChain` / `multioutput.py` opened before the fix **4/5** | "producer: …" **0/5** | `classes_` added to `MultiOutputClassifier` **5/5**; no fallback in `_validation.py` | 8 of 12 |
| **A**, django-11728 | greps for the pattern in all five; a step described as looking for the twin **3/5** | "twin: …" **0/5** | `replace_unnamed_groups` fixed too **5/5** | 8 of 12 (4 of 8 in the split pairs) |
| **A**, matplotlib-24870 | greps in all five (4–16 each) | "twin: `TriContourSet._contour_args` … also calls `_process_contour_level_args`" **2/5**, mid-reply | `tri/_tricontour.py` changed **5/5** | 5 of 12 |
| **A**, astropy-14182 | greps in four of five | **0/5** | the reader handled (`data.start_line` from the header count) **4/5** | 7 of 12 |

**What the read establishes, and what it does not.** The *choice* the five rules were written for is present in 24 of 25 transcripts on instances where, across 60 earlier rollouts on the same five instances (cycles 14–16), it was present in 29 — and in the one instance the rules were most needed for, astropy-13236, in 1 of 12. The *step* is visible in most: every 13236 rollout read the version before it chose, four of five 14629 rollouts opened the sibling before they fixed, and the twin was found by grep on both other instances. The *line* is the part the agent mostly ignores — written in 4 of 5 for the stage rule, 2 of 15 for the twin, 0 of 5 for the producer — and where it appears it is in the agent's own words, not the template's. None of this is a measurement: five rollouts per instance cannot carry a rate, and 23 of 25 is reported as a count. What it is: the behaviour the rules ask for, seen at the point where it used to be absent, in transcripts where the old choice was still available and was taken once.

**A note for the rule-writers, from the read.** The visible step is the reliable mark; the reply line is not. The features agent pinned the line wordings in tests so a rewording could not make the rules unevaluable — a sound instinct — but the agent evaluates them by taking the step and skipping the sentence. Future reads should look for the step (the grep described as looking for the twin, the version read, the sibling opened) and treat the line as a bonus; a rule whose only mark is a sentence in the reply would not be readable at all.

**Two smaller things.** The one 14182 failure tested a round-trip and wrote "Round-trip read/write works correctly" with the reader unfixed — its round trip did not pass `header_rows` on the read side, so it compared the fix against a reference that could not fail: E2's shape, on a rule that landed two cycles ago; recorded. No third case of the agent arguing with the request in 25 rollouts.

Spend $13.52. Cycles 17–20 together: $20.90, against $87 for cycle 16 alone.

## Cycle 21 (2026-09-17) — the last 65 unread failures; the account holds, one new shape of twin, and 26208

No new score was cut. The material is the remaining unread failures from the cycle 1–5 slices: 89 non-fetching failures in that pool, 24 read in cycle 19, **65 read here** on 45 instances (`c21-unread.txt` in the cycle folder). Each is read against the gold patch and FAIL_TO_PASS list with the agent's files, mechanism line where the kernel had one, and final summary; **evident** means the trace is in the transcript, **consistent** means the shape fits and the failing detail is not visible.

**The kernels these rollouts ran on**, from each bundle's `meta.json` (`kernel_git`), recorded here because the loop now records the kernel it reads as well as the one it runs: c1a `unknown` (the first Docker build, before the sha was baked), c1b `f610f39d14bf`, c2a `65522984a9b5`, c2b `16dec37e6492`, c3a `2e89c252baaa`, c3b `14dfac7d0bb0`, c4a `4ea32c333e02`, c4b and c5a `6d452f515612`, c5b `0e5d5eed88a0`. All match the files in `kernels-manifest.json`. These are pre-repro-gate, pre-network-cut agents; the network was open but these rollouts did not use it.

| Pattern | Rollouts | Evident | Consistent | Instances |
|---|---|---|---|---|
| **A** the twin | 17 | 14 | 3 | astropy-14369 ×2 (the *generated parser table* `cds_parsetab.py` beside the grammar it is built from — a new shape of twin), django-13212 ×2 (the form-field side beside the validators), django-16256 ×2 (`GenericRelatedObjectManager` beside the related managers), matplotlib-26466 ×2 (`_ref_coord` beside `xy`), sphinx-7462 ×2 (the unparser beside the domain), astropy-14182, django-11728, django-13512, django-14376 (the dbshell client beside the backend); consistent: django-12406, sympy-13798, sympy-16597 |
| **B** producer, not consumer | 13 | 9 | 4 | sympy-20428 ×2 (the callers patched; the `EX` domain's zero test was wrong), sympy-21930 ×2 (the LaTeX printer patched; the secondquant operators' own `_latex` was wrong), django-13794 ×2 (the `add` filter patched; the lazy proxy lacked `__radd__`), matplotlib-25479 ×2 (`pyplot.set_cmap` patched; the registry should rename on register), pylint-7080 (the linter's walk; `expand_modules` should normalise), matplotlib-23476, django-14792, django-12273 ×2 (consistent) |
| **E2** reproduce the behaviour | 3 | 1 | 2 | pylint-6386; django-10999 ×2 (five and six tool calls: the reporter's example fixed, the PostgreSQL format in the same test never run) |
| **E1** a test encodes the bug | 1 | 0 | 1 | sympy-15017 (the `__len__` special case, on a kernel with no thinking to read) |
| **F** the checkout decides the stage | 1 | 1 | 0 | astropy-13236 |
| C, the issue does not determine the fix | 27 | 8 | 19 | matplotlib-26208 ×2 (below), django-14140 ×2, sympy-18199 ×2, django-15252 (evident); the rest right file, right mechanism, failing detail not visible |
| edit path invisible | 3 | — | — | django-14011, pytest-5840 ×2: no `edit`/`write` call recorded — edits made through `sed` in bash — so the read cannot see what was changed; the QA note of cycle 19 already flagged this |
| **G** argues with the request | 0 | | | |

**Everything fits.** 62 of 65 land in A, B, E1, E2, F or C; 3 cannot be read because the edit path bypassed the tools. No sixth agent pattern, no third case of G. With cycle 18's 79 and cycle 19's 24, the account has now been checked on 168 honest failures across 13 kernels and three generations of the agent; the qualifier stands — 25 of the 168 are consistent-with rather than evident, almost all of them in C.

**One new shape of an old pattern.** astropy-14369: the agent fixed the CDS unit grammar in `cds.py` and left `cds_parsetab.py`, the PLY-generated parser table built from that grammar, as it was; both rollouts. The twin here is not a sibling function but a generated artefact that must be rebuilt when its source changes. The twin rule as written ("the sibling function… the other front end… the reader") does not name this case; worth one more clause: *the generated file beside its source.*

**The nearest thing to "wrong mechanism" in 168 failures**, and why it is still C: matplotlib-26208 (dataLims become inf with `twinx` and a stackplot). Both rollouts found that `relim()` skips `Collection` artists and fixed that — a real defect, plausibly the cause. The maintainers fixed something else entirely: `twinx` now copies the axis units to the twin. The hidden tests are `test_twin_units[x|y]`; the issue never mentions units. The agent's mechanism may well have been right for the symptom as reported; the tests grade a mechanism the issue does not determine. Recorded because it is the case a wrong-mechanism reading would have claimed, and the classification by what the tests require puts it in C.

**The ranking, over all 168 read** (primary class): A twin **41**, B producer **28**, F stage **12**, E2 **7**, E1 **2**, C **70**, other, unreadable or G **8**. Same order as cycle 18's on 79; A and B together are 69 of 168, 41%.

Spend $0.00.

## Cycle 22 (2026-09-17/18) — the measured door: the five rules against the pre-rule kernel, 40 instances × 2, both arms

**The band, stated first** ([pre-registration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-22-preregistration.md), 21:15 UTC, before the runs): on the six-run estimate a difference between two 80-rollout arms has a standard deviation near 7; the loop's band of 8 in 40 is **16 in 80**. Below 16 on the shared instances the result is *not distinguishable from no effect*, never "+n" or "−n" as a finding.

**Arms.** Control: kernel `4b833de9860f` = `main` `7017eb75` (pre-#440, none of the five rules, the mechanism gate removed) plus #477's build plumbing, pushed as `cursor/swebench-c22-control-7c9c` so the sha is public. Treatment: kernel `aaf3dbe98de7` = `main` head at 21:10 UTC, all five rules in. Both built in the loop's worktree, named by sha, read-only; both runs passed `kernel-sha` and every artifact carries the identity. **Measured `--version`: control `arbos-kernel 0.2.0 4b833de9860f protocol 1`; treatment `arbos-kernel 0.2.0 aaf3dbe98de7 protocol 1`.** Same harness, one reproduction, $8 cap, network cut with the sweep, concurrency 3, interleaved in time. Set: `reg40` = regression 20b v2 plus the next 20 never-run instances in the loop's order (10 "<15 min", 10 "15 min–1 hour"). Both arms ran all 80 rollouts under their caps.

| Arm | Solved / 80 | Cost | Per rollout | Capped | Egress open | Fetches | Sweep survivors |
|---|---|---|---|---|---|---|---|
| Control `4b833de9860f` | **56** | $55.64 | $0.70 | 0 | 0 | 0 | 0 |
| Treatment `aaf3dbe98de7` | **53** | $50.25 | $0.63 | 0 | 0 | 0 | 0 |

**Treatment − control on the 40 shared instances: −3 of 80. Not distinguishable from no effect.** Split: on the 20b half, control 24/40, treatment 22/40; on the fresh 20, control 32/40, treatment 31/40 — the fresh draw was easy (half of it "<15 min") and both arms solved 15 of those 20 instances 2/2.

**What this does and does not say.** It does not say the rules do nothing: cycle 20 saw the choice they ask for made in 24 of 25 transcripts, and on the six instances that carry the patterns the treatment solved 9 of 12 against the control's 7 (astropy-13236 went 0/2 → 2/2, the stage rule's own instance; scikit-learn-14629 and matplotlib-24870 1/2 → 2/2). It says that whatever they do is smaller than 20 points on this set, and that the loop was right in cycle 16 to stop measuring anything smaller. The paired per-instance picture is mixed at the noise scale: the treatment gained on 13236, 24870, 14629, 16454, 18698 and lost on 15017, 8898, 14182, 11728, 6386, 6197, 15022, 15252, xarray-6938 — one rollout each way, the kind of flips the six repeated runs of cycle 16 produce with no change at all.

**The design fault, said plainly.** The pre-registration set the band from the noise and the plausibility from cycle 18's ceiling — 41 of 79 failures addressable, 54% → 78% *if every rule worked on every failure*. That ceiling was computed on the failure corpus, not on the set being measured. On `reg40` the control already solved most of the carrying instances (11728 2/2, 14182 2/2, 24870 1/2, 14629 1/2, 6386 1/2), so the most the five rules could have gained here was about eight rollouts — half the band — before a single loss elsewhere. The door was opened on a set where the lever's ceiling was below the band; the result was foreseeable from the control arm's per-instance table, which did not exist until the run was over. The lesson joins the standing findings: **state the lever's ceiling on the measured set, not on the corpus, and if it is below the band do not run.** A cheaper design would have been the six carrying instances at `-r 6` per arm (72 rollouts, ~$60), where the ceiling is the whole set.

**Two things from the per-instance table.** sympy-15017 went 2/2 → 0/2: both treatment rollouts changed the constructors (the root fix), then also edited the *existing* test files and changed indexing to keep `a[0]` raising, both citing numpy — the cycle-19 retreat again, now with the test edited too, on a kernel carrying E1. Two rollouts, noise-sized, and the third time this instance has shown the agent choosing numpy's semantics over the maintainers'. django-15022 and django-15252 each solved once in the control arm — the first clean solves of 15022 in 14 rollouts and of 15252 in 9; recorded, not explained.

Spend $105.89. Soundness held on all 160 rollouts: egress 0.0, no fetch, no live survivor, both kernels proving their labels.

## Cycle 23 (2026-09-18) — cycle 22's 51 failures read; "the agent decides no change is needed" becomes a pattern

No new score. Material: the 24 control and 27 treatment failures of cycle 22 (`c23-fails.txt`), the first failures from a kernel carrying all five rules, on 40 instances of which 20 had never been read. Kernels: control `arbos-kernel 0.2.0 4b833de9860f protocol 1`, treatment `arbos-kernel 0.2.0 aaf3dbe98de7 protocol 1`, from the artifacts' `kernel-identity.json`.

| Pattern | Rollouts | Control / treatment | Instances |
|---|---|---|---|
| **G** the agent decides no change is needed | ~~**5**~~ **1** | ~~2 / 3~~ 0 / 1 | ~~django-13513 ×4~~ *(re-classed C in cycle 25: the issue's own example already passes on the base commit — `'my error' in html == False`, `'my new error' in html == True` — so "no change" was the correct answer to the request; the hidden test grades an innermost exception with no traceback, which the issue does not describe)*; django-15022 ×1 (treatment) — "concluded no code change should be made" after finding the historic patch reverted upstream |
| **B** producer, not consumer | 12 | 5 / 7 | xarray-6938 ×3 (the agent's own root-cause line: "`IndexVariable.to_index_variable()` returns `self`" — then fixes the caller in `swap_dims`), sympy-17318 ×4 (guard at the crash site, root named), django-16877 ×4 (below), scikit-learn-14629 ×1 (control) |
| **E2** reproduce the behaviour | 2 | 1 / 1 | pylint-6386 (`_DoNothingAction` takes no argument; verbose still off) |
| **E1** a test encodes the bug | 2 | 0 / 2 | sympy-15017 (root fix kept, then the existing tests edited and indexing changed to keep `a[0]` raising, citing numpy) |
| **A** the twin | 3 | 1 / 2 | matplotlib-24870 (control, `contour.py` only), django-11728 (treatment, 17 tool calls, named groups only), astropy-14182 (treatment, writer only) |
| **F** the checkout decides the stage | 2 | 2 / 0 | astropy-13236 — the treatment had none |
| C, the issue does not determine the fix | ~~24~~ 28 | ~~13 / 11~~ 15 / 13 | django-15503 ×4, django-15732 ×4, django-16454 ×3, pylint-8898 ×3, django-15252 ×3, sympy-18698 ×3, django-14771 ×2, django-15022 ×2 |
| provider stall | 1 | 0 / 1 | pytest-6197 (treatment): the model returned nothing for 13 minutes, the kernel ended the turn (exit 2), no patch. Not the agent. Treatment is 53 of 79 without it; still inside the band. |

**~~G is a pattern.~~** *Corrected in cycle 25: with django-13513 re-classed, G has three cases on two instances (django-15022 ×2, django-10097), not seven on four; it goes back to a candidate. The rule proposed below was still landed in #541 and is read in cycle 25.* Two cases before this cycle (django-15022 declining on upstream history; django-10097 overriding the issue's RFC quote), ~~five now — seven across four instances~~ one more now. Two shapes: *"it is already fixed"* (django-13513, four of four: the agent found the code the issue *suggested* already in the tree, ran a test that covers the suggestion, and stopped; the hidden test wants the innermost exception handled when it has no traceback, which the suggested code does not do) and *"it should not be fixed"* (15022, 10097). Both rest on the same fault as E1 and E2 — a check against the wrong reference: the issue's suggested patch instead of the issue's symptom; upstream history instead of the request in front of it. Proposed rule, for the features agent:

> Concluding that the request needs no change needs the same evidence as a fix: run the request's own example against the tree and show it already behaves as asked. "The fix the issue suggests is already present" is not that evidence — the suggestion may be older than the tree and the request is the symptom, not the patch. If the example already passes, say so with the command; if it does not, the request stands whatever the history says.

**The rules' marks under the rules.** In the treatment's 27 failures the old choices the rules target were still made: the producer fallback in xarray-6938 ×2 and sympy-17318 ×2, the crash-only fix in pylint-6386, the twin missed in django-11728 and astropy-14182, the retreat in sympy-15017 ×2. None of those transcripts carries a rule's reply line. Two of them show the B rule's *scope* rather than its failure: "a class lacks an attribute, or a path lacks a case, that its siblings have" does not describe a method that returns `self` where a copy is needed (6938) or a guard placed at the crash site with the root named one line above (17318). The pattern is producer-not-consumer; the rule's examples are narrower than the pattern.

**A fresh instance that fails the same way in both arms, four of four:** django-16877, the new `escapeseq` filter. Every rollout implemented it with `escape()`; the gold uses `conditional_escape()`, which is what the `escape` *filter* — the sibling the issue names ("what `safeseq` is to `safe`") — uses internally. Mirroring the sibling's implementation rather than its name would have passed; the producer rule says "made the way the sibling does it" and no rollout opened the `escape` filter to see how. Classed B.

**Everything fits, again.** 50 of 51 in the patterns or C; one provider stall. With cycles 18, 19 and 21: **219 honest failures read**, ranking A 44, B 40, ~~G 7~~ G 3, F 14, E2 9, E1 4, ~~C 94~~ C 98, other 7 *(corrected in cycle 25)*. ~~G enters the ranking above E2.~~

Spend $0.00.

## Cycle 24 (2026-09-18) — the six carrying instances at `-r 6`: the rules move the instances they were written for, at the edge of the band

**Ceiling first, band second** ([pre-registration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-24-preregistration.md), 00:45 UTC, before the runs). Set: the six instances the five rules were written from — django-11728, matplotlib-24870, astropy-14182 (twin), astropy-13236 (stage), scikit-learn-14629 (producer), pylint-6386 (behaviour, not crash) — at `-r 6`, 36 rollouts per arm. Ceiling: pre-rule kernels solved 37 of 72 on these six across cycles 14–16, so the treatment could gain at most 15–18 of 36. Band: from the six-run estimate scaled to 36 rollouts, two standard deviations of the difference ≈ **10 of 36**. Ceiling above band; the run was allowed. Decision rule: ≥ 10 → moved by at least the band; < 10 → not distinguishable.

**Arms.** The same two binaries as cycle 22, read-only, labels proved by the run: control **`arbos-kernel 0.2.0 4b833de9860f protocol 1`** (`main` `7017eb75`, pre-rule, plus #477's plumbing), treatment **`arbos-kernel 0.2.0 aaf3dbe98de7 protocol 1`** (`main` head 21:10 UTC 17 Sep, all five rules). Same harness, one reproduction, $8 cap, network cut with the sweep, concurrency 3, arms interleaved. Both ran 36 of 36; no rollout capped, no provider stall; egress 0.0, no fetch, no live survivor in either arm. $36.38.

| Instance (rule) | Control `4b833de9860f` | Treatment `aaf3dbe98de7` | Δ |
|---|---|---|---|
| astropy-13236 (F, stage) | 2/6 | **6/6** | +4 |
| matplotlib-24870 (A, twin) | 1/6 | **4/6** | +3 |
| scikit-learn-14629 (B, producer) | 4/6 | **6/6** | +2 |
| astropy-14182 (A, twin) | 3/6 | 4/6 | +1 |
| django-11728 (A, twin) | 4/6 | 4/6 | 0 |
| pylint-6386 (E2) | 5/6 | 5/6 | 0 |
| **Total** | **19/36** | **29/36** | **+10** |

**Treatment − control = +10 of 36: the pre-registered threshold, met at its edge.** By the rule written before the run, the five rules moved the instances they were written for by at least the band. Said with the precision it deserves: the difference is two standard deviations of the loop's own noise estimate, not more; the realised ceiling was 17, so the treatment took about ten of the seventeen rollouts that were there to take; and the gain sits where the pre-rule control was weakest (13236 and 24870 account for seven of the ten), which is what a rule that fixes a specific failure should look like. Read together with cycle 22 — the same two kernels, −3 of 80 on a general 40-instance set — the two results are one statement: **the rules do what they were written to do on the instances that carry their patterns, and those instances are a small share of the benchmark** (cycle 18's ranking: 41 of 79 failures addressable, about a tenth of rollouts). Neither number is 23/25 and neither is claimed as a rate.

**Every failure read against the account** (24: 17 control, 7 treatment; `c24-fails.txt`):

- Control 17: astropy-13236 ×4 — F, evident, every one wrote the `FutureWarning`; one read the version four times and wrote the warning anyway (the step without the rule that tells it what the step means). matplotlib-24870 ×5 — A, evident: `tri/_tricontour.py` untouched in all five. django-11728 ×2 — A, evident: `replace_unnamed_groups` untouched. scikit-learn-14629 ×2 — B, evident: the fallback in `_validation.py`; one of them opened the sibling three times and put the fallback in the caller anyway. pylint-6386 ×1 — E2, evident. astropy-14182 ×3 — consistent with C: the reader was handled, the failing detail is not visible.
- Treatment 7: django-11728 ×2 — A, evident: 14 and 17 tool calls, no grep described as looking for the twin, `replace_unnamed_groups` untouched — the rule not followed, and the short trajectory again. matplotlib-24870 ×2 — the twin *was* fixed (one carries "Twin check: … twin found and fixed in the same change"), and the rollout failed on something else: the pattern removed, a different residue; consistent with C. astropy-14182 ×2 — as the control's, consistent with C. pylint-6386 ×1 — E2, evident: `_DoNothingAction` again, one `-v`/`--verbose` comparison in the transcript and still the crash fix.

Everything fits; nothing new. Cumulative read: **243 honest failures.**

Spend $36.38.

## Cycle 25 (2026-09-18) — #541 read: the "no change" rule was tested on the wrong instance, and the widened producer rule's mark is the one thing the agent does not do

**Conditions.** [#541](https://github.com/unarbos/arbos/pull/541) landed the G rule ("no change needed" takes a recorded run of the request's example against the unmodified tree and a line "no change: <command> already …") and widened the producer rule to the method that returns the same object and the guard placed where the error surfaces, with a new mark: *the line in your own reply that names the root cause names where the fix goes.* 77b1feaa made an edit through the shell an edit on the tool event and named the generated file in the twin rule. Kernel **`arbos-kernel 0.2.0 206d617e9a50 protocol 1`** = `main` head, all of it in, built in the worktree, label proved by the run. Network cut, egress 0.0 on all 15, no stall. Pre-registered as a read ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-25-preregistration.md)): five rollouts each on django-13513, xarray-6938, sympy-17318. $5.22. Three of fifteen solved — a count.

**django-13513 was the wrong instance for G, and the read says so.** All five rollouts declined to change the code, as all four did in cycle 22. Two wrote the rule's line; one of them ran the issue's own example — `TestView.get` with `raise ... from None` — against the unmodified tree and showed `'my error' in html == False`, `'my new error' in html == True`: **the request is already satisfied on the base commit.** The hidden test grades an innermost exception that has no `__traceback__`, a case the issue never describes. By the G rule's own logic the correct answer here is "no change", and the agent gave it. So cycle 23 misfiled these four rollouts: they are C (the issue does not determine the fix), not G; the strike is above, and G returns to a candidate with three cases (django-15022 ×2, django-10097). What the read does show about the rule's marks: the "no change:" line 2 of 5; the recorded reproduction (`repro:true`) 0 of 5 — the example was run, when it was run, as an ordinary command. The rule's valid targets, 15022 and 10097, were not in this read.

**The widened producer rule, on its two new cases.**

| Instance | Root-cause line names the producer | Diff at the producer | Diff at the consumer / guard | "producer:" line | Cycle 22 |
|---|---|---|---|---|---|
| xarray-6938 | 4/5 name `to_index_variable` returning `self` | **2/5** (`variable.py`; both solved) | 3/5 (`swap_dims` in `dataset.py`; all failed) | 0/5 | 0/3 at the producer |
| sympy-17318 | 5/5 name `_sqrt_match` | **1/5** (`sqrtdenest.py`'s condition; solved) | 4/5 (guard in `radsimp.py`) | 0/5 | 0/4 at the producer |

The choice moved from 0 of 7 to 3 of 10; small numbers, a count. The rule's new mark — the root-cause line in the reply names where the fix goes — is present in 9 of 10 transcripts, and in 6 of those 9 the diff is somewhere else. The agent writes the sentence that locates the fault and then fixes the caller. That is the pattern the rule was written against, now observed *with the rule in force*, and it says the mark is a description of the failure rather than a lever on it: the agent already knew where the fault was in cycles 12–24 too. What separated the three producer fixes here is what separated them in cycle 17's pairs — the agent opened the producer's code (`IndexVariable`, `_sqrt_match`'s loop) and edited there, rather than reasoning about it from the call site.

**Edits through the shell.** 77b1feaa's record works: one rollout (xarray-6938 `bebeff6b`) edited `variable.py` through `sed` and the bash event carries the path; nothing in this read was unreadable.

**Cumulative:** 258 honest failures read (243 + 15); with the correction, G 3, C 98 + this cycle's.

Spend $5.22.

## Cycle 26 (2026-09-18) — the "no change" rule on its real targets: the behaviour did not appear, and the rule's reach ends where the agent argues instead of declining

**Conditions.** Kernel **`arbos-kernel 0.2.0 206d617e9a50 protocol 1`** (`main` with #541 in; `2492c601` is the merge that carries it), read-only, label proved. Network cut, no stall, no cap. Five rollouts each on django-15022 (declined twice before on upstream history) and django-10097 (once before, permitting `:` in the password against the issue's RFC quote). $16.81; **0 of 10 solved** — a count, and the expected one: 15022 has solved 2 of 24 clean rollouts in the loop's history and 10097 0 of 2. Pre-registered ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-26-preregistration.md)) as a read of whether any rollout concludes "no change" and, if so, whether the recorded run and the line are there.

**No rollout declined.** All ten produced a patch. Every 15022 rollout found the upstream history (the ticket, the reverted patch) and changed the code anyway — the choice to decline was available and was not taken, 5 of 5 (cycle 22's treatment took it in 1 of 2; cycles 14–16, 1 of 12). Because no rollout said "no change", the rule's marks — the `repro:true` run of the request's example, the "no change: …" line — had nothing to attach to; the read cannot say whether they would have been present. Three cases in 258 was always too rare a pattern to read in ten rollouts, and this cycle confirms the rarity rather than the rule.

**django-10097 shows where the rule stops.** The issue quotes RFC 1738: `:`, `@` and `/` in user and password must be encoded. Two of five patches forbid `:` in the password as the issue says (and still failed, on a detail not visible in the transcript — consistent with C). **Three of five permit it** — two with the argument, again, that a colon in the password is legal; one by collapsing `user:pass` into a single character class. This is the shape cycle 19 named (the agent's own reading over the request's explicit statement), now 4 cases on this one instance, 3 of them with #541 in force. The landed rule says "the request stands whatever the history says", which reaches a decline; it does not reach a fix that quietly implements the agent's reading of the spec instead of the issue's. Whether that deserves its own sentence is the features agent's call; the loop records it as a candidate distinct from declining: **G-decline** (15022 ×2, no new cases) and **G-override** (10097 ×4).

Every failure read against the account: 15022 ×5 C (semantics preserved, as in all 24 before); 10097 ×2 C-consistent, ×3 G-override. Nothing new. Cumulative read: 268.

Spend $16.81.

## Cycle 27 (2026-09-18) — the producer rule's step landed; it moves the case where the producer fix is obvious once read, not the case where it takes a judgement

**Conditions.** c8c62b68 on `main` gave the producer rule the step cycle 25 asked for: *read the function your root-cause line names before the first edit — the read call is the record.* Kernel **`arbos-kernel 0.2.0 17c5232e2189 protocol 1`** = `main` head, built in the worktree, label proved. Network cut, egress 0.0, no stall. Pre-registered as a read ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-27-preregistration.md)): five rollouts each on xarray-6938 and sympy-17318. $5.80; six of ten solved — a count.

| Instance | Root named in the reply | Producer opened *before the first edit* | Fix at the producer | Solved | Before (c22 → c25) |
|---|---|---|---|---|---|
| xarray-6938 (`to_index_variable` returns `self`) | 5/5 | **5/5** (2–8 reads of `variable.py` / the method) | **5/5** (`variable.py`) | 5/5 | 0/3 → 2/5 |
| sympy-17318 (`_sqrt_match`'s condition) | 5/5 | 4/5 | **1/5** (`is_real` added to the condition; solved) | 1/5 | 0/4 → 1/5 |

**xarray-6938 changed.** Every rollout opened the producer before editing, every one fixed it there, every one solved — on an instance that had solved 2 of 8 clean rollouts before and never once in cycle 22. Five is five; but it is the same instance, the same choice, and the choice went the other way five times out of five with the step in the transcript each time. This is what a rule landing looks like at the resolution the loop can read.

**sympy-17318 did not.** Four of five opened `_sqrt_match` before editing — the step was taken — named its condition as the fault, and then guarded downstream anyway: two in `_split_gcd` (`if not a: return`), two in `split_surds` (`if not surds: return`). All four failed the hidden test (`_sqrt_match(4 + I) == []`). The one that fixed the condition (`and x.is_real`) solved. The difference between the two instances is what the producer fix asks of the agent: in 6938 the fix is *obvious once the method is read* (return a copy instead of `self`); in 17318 it takes a judgement (which predicate excludes `I` — `is_real`, `is_positive`, `is_extended_real`?) and the agent, unsure, hedges with a guard it can defend as "safe". Reading the producer is the step that separates the two fixes when the producer's fix is plain; it does not decide a judgement the agent would rather not make. That is a boundary of the producer rule worth stating: **the step works where the fault is visible in the producer's code; where the producer fix is a semantic choice, the agent still prefers the guard.** No "producer:" reply line in any of the ten — the line remains the part the agent skips.

Every failure read: sympy-17318 ×4, B (the guard), evident — the fourth cycle this instance has shown it, now with the producer read in the transcript. Cumulative read: 272.

Spend $5.80.

## Cycle 28 (2026-09-18) — #601 read on django-10097: the choice unchanged, the mark unwritten, and the instance turns out to be ungradeable here

**Conditions.** #601 (c1d92e96) put the override shape into the contract: *a request that quotes its reference fixes what done means — implement the quote as written, even where your own reading of the wider standard would allow more*, with an "as quoted: …" line as the mark; its own example is this instance. Kernel **`arbos-kernel 0.2.0 e70e1b5c78e4 protocol 1`** = `main` head, built in the worktree, label proved. Network cut, no stall, no cap. Five rollouts on django-10097 ([pre-registration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-28-preregistration.md)). $7.71; 0 of 5 solved — see below for why that number means nothing here.

**The choice.** Two of five forbid `:` in the password as the RFC quote says (`[^\s:@/]*`); **three of five permit it** — the same 2/3 split as cycle 26's five, with #601 in force. Four of five name RFC 1738 in the reply; **none writes the "as quoted:" line.** The rule did not change the choice on its own example in five rollouts, and its mark did not appear.

**The instance cannot be solved in this environment.** One rollout (`a1214553`) produced a patch byte-identical to the gold — the same one-line regex change — and was graded failed. Grading the gold patch itself with the task's own `tests/test.sh` in a fresh container: **reward 0**, with `sqlite3.OperationalError: no such table: main.django_site__old` across the fixtures tests — the Django 2.2 / SQLite ≥ 3.26 `ALTER TABLE` incompatibility, in a FAIL_TO_PASS list 438 tests wide. django-10097 joins requests-2317 as a **grader artefact**: no agent patch passes here. The four earlier 10097 failures classed C or G-override keep their *behavioural* reading — the agent did permit `:` against the quote, seven times in twelve rollouts across cycles 2, 22, 26 and 28 — but their grades never depended on it, and they are re-labelled grader-artefact in the tallies (C 98 → 95; the G-override candidate is a behaviour observed, with no gradeable instance behind it).

So the loop cannot tell whether #601 would have changed a grade; it can say the rule did not change the choice or produce its mark in five rollouts on the instance it was written from, and that the instance is the wrong one to grade it on. A gradeable instance where the request quotes its reference would be needed to say more; none is known in the loop's corpus.

Every failure read: 10097 ×5 — grader artefact (3 of them also the override shape). Cumulative read: 277.

Spend $7.71.

## Cycle 29 (2026-09-18) — the predicate sentence read on sympy-17318, and ten fresh instances read for the account

**Conditions.** 1b083988 on `main` added the sentence cycle 27 proposed: *when you can name the wrong predicate, change the predicate; a guard that lets the wrong value reach a different caller is the same bug with one caller patched.* Kernel **`arbos-kernel 0.2.0 d12118e60d8b protocol 1`** = `main` head, built in the worktree, label proved. Network cut, no stall, no cap. Two reads, pre-registered together ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-29-preregistration.md)); $21.15.

**A. sympy-17318 under the predicate sentence, five rollouts.** All five name the condition in `_sqrt_match` as the fault. **Two changed it** (one adding `is_real` to the predicate, one rewriting the branch) — both solved; **three guarded** downstream (`if not surds` / `if not a`) — all failed. Producer fix by cycle: 0/4 (c22), 1/5 (c25), 1/5 (c27), 2/5 now. Counts, not a rate; the sentence did not settle the choice on the instance it was written from, and the guard remains what the agent reaches for when the right predicate is a judgement.

**B. Ten never-run instances at `-r 2`, read for the account.** 15 of 20 solved — reported as a count on a fresh draw, not as a rate. Five failures, all inside the account: sympy-20438 ×2 — **A, the twin**: the gold adds an `Eq` handler beside the `is_subset` handler (`comparison.py`, `relational.py`); one rollout fixed `issubset.py` alone (evident), the other missed `comparison.py` (consistent); django-15695 ×2 and sympy-18211 ×1 — right file, right mechanism, failing detail not visible: C, consistent. Nothing outside the account on ten instances the loop had never seen, on a kernel carrying every rule.

Cumulative read: **285 honest failures**; the account has held on every one since cycle 18, with the qualifier unchanged — a share of C is consistent-with rather than evident.

Spend $21.15. Cycles 25–29 together: $56.69, five landed rules read on their own instances.

## Cycle 30 (2026-09-18) — ten more fresh instances read for the account

**Conditions.** Kernel **`arbos-kernel 0.2.0 a8678ac16636 protocol 1`** = `main` head, built in the worktree, label proved. Since cycle 29 two contract commits changed marks, not asks — the producer rule's reply line is gone (the read call is the mark, as cycles 25 and 27 found), and the quoted-reference rule's mark is now a test that asserts the quote — neither needing a re-read the loop can do. Network cut, no stall, no cap. Ten never-run instances (`fresh10b.txt`) at `-r 2`, pre-registered ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-30-preregistration.md)). $7.49; 11 of 20 solved — a count on a fresh draw.

**Nine failures, all inside the account.** django-12325 ×2 — **A, the twin**, evident: the gold changes `options.py` beside `base.py` (the `parent_link` check that `test_clash_parent_link` exercises); both rollouts fixed `base.py` alone, in 14 and 15 tool calls. django-16667 ×2 — C, consistent: the same `except`, a different return value than the pinned `"0-0-0"`. matplotlib-23299 ×2, sympy-15875 ×2, sympy-19495 ×1 — C, consistent: right file, right function, a different mechanism or detail than the gold's; the failing assertion not visible in the transcript.

Twenty fresh instances across cycles 29–30, fourteen failures, none outside the account. Cumulative read: **294**.

Spend $7.49.

## Cycle 31 (2026-09-18) — the quoted-reference rule's new mark on its own example; ten more fresh instances

**Conditions.** Kernel **`arbos-kernel 0.2.0 a8678ac16636 protocol 1`** (cycle 30's; the contract unchanged since). Network cut, no stall, no cap. Two reads, concurrent, pre-registered ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-31-preregistration.md)). $19.94.

**Read 1 — #601 with the test mark (5bb0ddec), five rollouts on django-10097.** The instance cannot be graded here (cycle 28: the gold grades 0), so the read is of the transcript only.

*The choice.* Three of five forbid `:` in the password as the RFC quote says — two of them the gold regex exactly, one stricter (`[^\s:@/?#]*`); two permit it (one drops the `user:pass` structure altogether). Cycles 26 and 28 had two of five each. Three of five on a five-draw is not a change the loop can read; it is not the other direction either.

*The mark.* The rule now asks for a test that asserts the quote. Three of five write tests; **none asserts the quote's clause.** All three add the issue's literal example (`http://foo/bar@example.com`, the `/` in the userinfo) to `invalid_urls.txt`; one adds `user:pass/word@`; none adds a URL with a second `:` in the userinfo as invalid — the one case that would pin the quoted grammar rather than the reported symptom. The two rollouts that followed the quote most exactly wrote no test at all. All five name RFC 1738 in the reply. So the mark as written does not appear in five rollouts on the instance the rule was written from; what appears instead is a test of the *symptom*. If a mark is wanted here, the reading is that "a test that asserts the quote" is being read as "a test for the issue's example", and the rule would have to say the difference — an input the quote forbids and the example does not show.

**Read 2 — ten never-run instances at `-r 2`.** **18 of 20** solved; the two failures are both requests-1766, and both are a **grader artefact**: the agent's patch is the gold's one-line change (`qop="auth"`), byte-for-byte in the changed line. Grading the gold patch itself with the task's `tests/test.sh`, network on: **reward 0** — three tests outside the fix fail in this environment (`test_conflicting_post_params`, `test_prepared_from_session`, `test_unicode_multipart_post`), while all six FAIL_TO_PASS tests pass. requests-1766 joins requests-2317 and django-10097 as an instance no patch passes here. On the eighteen gradeable rollouts: 18 of 18.

Thirty fresh instances across cycles 29–31, sixteen failures, none outside the account. Every failure read: 10097 ×5 (grader artefact; two also the override shape), 1766 ×2 (grader artefact). Cumulative read: **301**.

Spend $19.94.

## Cycle 32 (2026-09-18) — ten more fresh instances; Jev is not in the loop's kernel

**Conditions.** Kernel **`arbos-kernel 0.2.0 a8678ac16636 protocol 1`** (cycles 30–31's). `main` moved (fba8688d) but every new commit is Jev — the router model that picks the next mechanical step and, since a47c5104, ends the turn when it fails — and **Jev is off under this harness**: it is on by default only when the provider is OpenRouter, and the harness sets `provider = custom` (the interception endpoint). Checked in cycle 31's traces: 772 provider calls, all `purpose: turn`, none `jev`. So the loop has measured, since Jev landed at 01:23 today (dcd8dba6, in the cycle-28 kernel onward), the one-model loop — which is no longer what a desktop user on OpenRouter runs. A rebuild for the Jev commits would change nothing here; the kernel stays. Stated so nobody reads the loop's counts as counts of the shipped desktop agent. Network cut, no stall, no cap. Ten never-run instances (`fresh10d.txt`) at `-r 2`, pre-registered ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-32-preregistration.md)). $11.14; **16 of 20** solved (a count).

**Four failures, all inside the account.** django-14034 ×2 — C: the issue describes validation (`MultiValueField` ignoring a sub-field's `required`), both rollouts fix `MultiValueField.clean()` in `fields.py`; the gold and the one hidden test (`test_render_required_attributes`) are about *rendering* the `required` attribute in `boundfield.py` — a thing the issue does not determine. sympy-21596 ×2 — C, consistent: right file, right handler, a different construction (restrict the base set to the roots) than the gold's (`_solution_union` with a `ConditionSet` for the unsolvable case); the hidden test pins the form.

Forty fresh instances across cycles 29–32, twenty failures, none outside the account. Cumulative read: **305**.

Spend $11.14.

## Cycle 33 (2026-09-18) — ten more fresh instances

**Conditions.** Kernel **`arbos-kernel 0.2.0 a8678ac16636 protocol 1`** (cycles 30–32's; `main` at 4db6f8ee with no change to the engine's contract, gates, turn, tools or host config). Jev off — the coordinator's standing rule: this harness measures the one-model loop, and Jev would be a new instrument with its own named baseline. Network cut, no stall, no cap. Ten never-run instances (`fresh10e.txt`) at `-r 2`, pre-registered ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-33-preregistration.md)). $11.14; **18 of 20** solved (a count).

**Two failures, both pylint-4551, both inside the account — C, the redesign shape.** The issue is one symptom (`a: str = None` shows as `NoneType` in pyreverse's UML). The gold changes four files (`diagrams.py`, `inspector.py`, `utils.py`, `writer.py`) and ten hidden tests grade exact `.dot` output and a new visibility helper. Both rollouts read the annotation in `inspector.py` (one also in `utils.py`), fix the symptom, and verify it end to end in 67 and 78 tool calls — the account's "hidden tests grade a redesign the issue does not determine".

*A hygiene note, not a cause.* Rollout `2f5db56f`'s patch contains `classes.dot`, a file the pyreverse tests write into the working tree. The agent saw it (`?? classes.dot` in `changes`, twice), removed it twice, and the last test run recreated it after the last `rm`, so the harness's patch — which includes untracked files — carried it. The grade would have been 0 anyway. But a patch with a generated artefact in it is a patch a reviewer would send back; if `changes` marked an untracked file that appears only after a test run as *generated by the run*, or the done-criterion pass refused an untracked artefact the request did not ask for, the agent's own `rm` would not have been undone by its own verification. Filed for the features agent as small.

Fifty fresh instances across cycles 29–33, twenty-two failures, none outside the account. Cumulative read: **307**.

Spend $11.14.

## Cycle 34 (2026-09-18) — ten more fresh instances; astropy-7606 is a fourth grader artefact

**Conditions.** Kernel **`arbos-kernel 0.2.0 a8678ac16636 protocol 1`** (cycles 30–33's; `main` at 1b4ef7a9, the two engine commits since Jev-only). Jev off. Network cut, no stall, no cap. Ten never-run instances (`fresh10f.txt`) at `-r 2`, pre-registered ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-34-preregistration.md)). $6.58; **18 of 20** solved (a count).

**Two failures, both astropy-7606, both a grader artefact.** Both rollouts wrap `UnrecognizedUnit.__eq__` in the same `try/except` its base class already uses, in 7 and 12 tool calls — the second names `UnitBase.__eq__` as "the twin implementation" in its reply, the twin rule visibly at work. The agent's patch passes the hidden test (`test_unknown_unit3`) when run directly: 1 passed. Under the task's own `tests/test.sh` the log shows **242 passed** — all 241 PASS_TO_PASS and the one FAIL_TO_PASS — and the parser prints FAILED. The gold patch, same script: 242 passed, FAILED. The swebench 4.0.3 log parser does not read this image's pytest-3 `-v` output, so no patch passes here. astropy-7606 joins requests-2317, django-10097 and requests-1766. On the eighteen gradeable rollouts: 18 of 18.

A difference worth recording but not a cause: the gold returns `NotImplemented` where the agent returns `False`, and also changes `UnitBase.__eq__` the same way. The hidden test does not distinguish them.

Sixty fresh instances across cycles 29–34, twenty-four failures, none outside the account; four of the sixty instances are ungradeable in this environment. Cumulative read: **309**.

Spend $6.58.

## Cycle 35 (2026-09-18) — ten more fresh instances; the twin named and then dropped on a recollection

**Conditions.** Kernel **`arbos-kernel 0.2.0 a8678ac16636 protocol 1`** (cycles 30–34's; `main` at a29da225, no engine or host change since). Jev off. Network cut, no stall, no cap. Ten never-run instances (`fresh10g.txt`) at `-r 2`, pre-registered ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-35-preregistration.md)). One of the twenty rollouts never started — two concurrent pulls of the same image raced and `docker run` failed before the kernel ran (0 tool calls, a `SandboxError`, not a rollout); it was re-run alone and solved. $13.57 + $0.33. **19 of 20** rollouts solved (a count).

**One failure, django-16560, inside the account — and the most legible twin-drop the loop has read.** The request: let `BaseConstraint` take a `violation_error_code` the way it takes `violation_error_message`. The agent implemented it across `__init__`, `__eq__`, `deconstruct`, `validate`, the postgres subclass and three docs files (81 tool calls, 19.8 KB). Six of the eight hidden tests pass; the two that fail want `violation_error_code=%r` in `__repr__`, exactly where `violation_error_message=%r` already is. The transcript shows the agent **name the twin twice** — "update `__repr__`/`__eq__` … to include violation_error_code (matching pattern for violation_error_message)" — and then drop it: *"I recall the actual Django `__repr__` doesn't display violation_error_code, only `__eq__` compares it. I'll leave `__repr__` untouched since existing tests check exact strings without that field."* Both halves of that sentence are wrong: upstream's `__repr__` does show it, and the existing tests pass strings without the field because none of them set one — the message twin is already conditional in the same way. So: A (the twin) found, then overridden by **G-decline** (a recollection of upstream, not the request) with **E1** as the stated reason (keep existing tests green). The account holds it; what is new is the shape — the rule's own step visibly done and then undone by memory. If a rule wanted to catch this, it would say: a twin you have named is not dropped on a recollection; it is dropped on a read of the twin's test, or kept.

Seventy fresh instances across cycles 29–35, twenty-five failures, none outside the account. Cumulative read: **310**.

Spend $13.90.

## Cycle 36 (2026-09-19) — ten more fresh instances; `changes` blocked 51 minutes re-running a server

**Conditions.** Kernel **`arbos-kernel 0.2.0 a8678ac16636 protocol 1`** (cycles 30–35's; `main` at 11e991cc, no engine or host change since). Jev off. Network cut, no stall, no cap. Ten never-run instances (`fresh10h.txt`) at `-r 2`, images pre-pulled so no pull race, pre-registered ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-36-preregistration.md)). $14.55; **16 of 20** solved (a count).

**Four failures, all inside the account.**

*django-14170 ×2 — C, and a case where the issue's own proposal is graded wrong.* The issue says the `BETWEEN` optimisation in `YearLookup` "should not be used for `__iso_year`". Both rollouts do exactly that — skip the optimisation when the lookup is not `year` — and the two FAIL_TO_PASS boundary tests pass. What fails are six *existing* tests (`test_extract_year_*_lookup [iso_year]`) that assert the SQL contains `between` and no `extract` for `iso_year`: the existing tests pin the mechanism, and the gold keeps them green by computing ISO-week bounds in `operations.py` instead. The existing tests encoded the bug's mechanism (E1's shape), the agent went the way the issue said and E1 allows, and the grade wanted the mechanism kept. The account holds it as C: the hidden tests grade a thing the issue does not determine, here the opposite of what it proposes.

*sphinx-7985 ×2 — C, consistent.* Both rollouts add a local-link check to `linkcheck` and report broken local links; the hidden test counts output lines and wants a *working* local link silent (`'working'`), where the agent kept the existing `[local]` line for it — one reply says so in words, "working local links are unaffected (still reported as `[local]`)". Seven lines against six.

**A kernel finding from a rollout that solved.** django-13809 rollout `dab2e1ba` solved in 80 tool calls and $2.89 — and took 7,749 s of wall, of which 7,257 s was tool time. Three `changes` calls took 3,086 s, 361 s and 3,070 s; the kernel's own notice said "nothing has happened for 46m: waiting on `changes`". The cause is in the reproduction gate: `changes` **re-runs every recorded reproduction, serially, each under `timeout 180`, with no total cap**, and the gate records *every* failing code-running command before the first edit as a reproduction. The agent's reproductions were `runserver` invocations — a server that never exits — so each re-run ran the full 180 s; seventeen recorded, 17 × 180 ≈ 3,060 s, twice. The container showed the re-runs: a fresh `timeout 180 bash -lc … runserver` every three minutes while the kernel waited. Two things follow. First, a reproduction that is a long-running server cannot be re-run as evidence; the re-run should either cap the whole pass or skip a command that timed out on first run (its exit was `None`, which `not_evidence` already names as saying nothing about the code). Second, `arbos-kernel run --timeout 2400` did not end this run at 2,400 s: the run timeout does not interrupt a blocking tool. Both are observations recorded here, not filed (coordinator, cycles 33–35).

Eighty fresh instances across cycles 29–36, twenty-nine failures, none outside the account. Cumulative read: **314**.

Spend $14.55.

## Cycle 37 (2026-09-19) — ten more fresh instances, all twenty solved; a fifty-minute host stall

**Conditions.** Kernel **`arbos-kernel 0.2.0 e5e66f71c418 protocol 1`** = `main` head, built in the worktree, label proved. The one engine change since a8678ac1 (180f1644: the sleep-while-workers-run guard counts a literal for-loop's passes) is a coordinator-with-workers case a root agent without workers does not reach. Jev off. Network cut, no stall of the agent's own, no cap. Ten never-run instances (`fresh10i.txt`) at `-r 2`, images pre-pulled, pre-registered ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-37-preregistration.md)). $7.06; **20 of 20** solved (a count). Nothing to read.

**The host stalled for fifty minutes.** The eval log has no line between 01:58:53 and 02:49:03. Three rollouts were live at 01:59 and each came back at 02:47 with the kernel's notice "nothing has happened for 48m" — two waiting on a `bash` (a pytest run), one waiting on the model — and all three then finished and solved (walls 3,050 s, 3,048 s, 3,105 s against 80–300 s for their siblings). Inside the containers `ps` showed the kernel as seconds old while its files were an hour old: the clock jumped. This is the VM freeze the loop has met before, not a kernel event and not the cycle-36 reproduction re-run; the cycle-36 finding stands on its own evidence (the `changes` durations and the re-runs seen in the container). Wall times from this cycle are not to be read.

Ninety fresh instances across cycles 29–37, twenty-nine failures, none outside the account. Cumulative read: **314** (unchanged).

Spend $7.06.

## Cycle 38 (2026-09-19) — the server-reproduction step read where it was found; ten more fresh instances; an in-run grading discrepancy

**Conditions.** Kernel **`arbos-kernel 0.2.0 1ec3de643d79 protocol 1`** = `main` head, built in the worktree, label proved. The engine change since e5e66f71 is bcf11b67 — Features' step on the cycle-36 finding (read from this document; nothing was filed): a command that runs a server is refused as a reproduction with the reason, and `changes`' re-run pass keeps to a 300 s budget. Jev off. Network cut, no cap. Two reads, concurrent, pre-registered ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-38-preregistration.md)). $23.21 in all.

**Read 1 — bcf11b67 on django-13809, three rollouts.** The step is visible and does what it says. Eleven `runserver`-shaped commands were offered with `repro:true` across the three; every one was refused in one second with the reason ("runs a server or a watcher, which never exits on its own — its exit is the timeout's, not the bug's; the reproduction is the request that hits the server"); the agents then recorded a parser-level script (`parse_args(['--skip-checks'])` failing) as the reproduction, and no `changes` call re-ran anything for long. Walls 292 s, 372 s, 457 s against cycle 36's 7,760 s; all three solved. One edge worth recording: the refusal keys on the word — a script that only *imports* `commands.runserver` and exits at once with an argparse error (a true reproduction) was refused too; one agent split the string (`'commands.' + 'runserver'`), was refused again, then wrote the same code to a file and ran it by path, which was accepted. The mark is textual, so it over-matches and is dodgeable; the behaviour it wants — a command that exits on its own — is not what it checks. Recorded, not filed.

**Read 2 — ten never-run instances at `-r 2`: 10 of 20** (a count), the lowest fresh draw since cycle 29, and nine failures read plus two re-runs:

- django-11433 ×2 — C, consistent: right function, a different condition than the gold's `in empty_values`.
- django-16502 ×2 (+1) — C, consistent: `write()` skips the body for HEAD; the gold reworks `finish_response` and `Content-Length`. One of the two rollouts never finished — killed at exit 143 by the 2,400 s harness timeout after the host pause below; re-run alone: the same shape, 0.
- seaborn-3069 ×1 — C: the fix put into the scale (`Nominal._setup`) where the gold puts it into the plotter's finalize; the hidden test grades the axis after `plot()`.
- pytest-7205 ×1 — C, pinned format: a bytes-only branch where the gold uses `saferepr` for every value, and ten hidden tests grade the quoting.
- sphinx-8056 ×1 — **B, the producer**: the agent fixed `docfields.py`, where the `:param x1, x2:` field is *consumed* and split; the gold fixes napoleon, which *produces* it; the hidden test is napoleon's.
- sympy-13852 ×1 — C, consistent: the closed forms differ from the gold's.
- **psf-requests-6028 ×2 — an in-run grading discrepancy the loop cannot yet explain.** Both rollouts produced the gold's change line for line (`netloc = '@'.join([auth, netloc])`, differing only in the comment). The run graded both 0 in ~7 s of scoring. The task's own `tests/test.sh` in a fresh container, network on, grades the agent's patch **1** and the gold **1**. With no network at all the tests still pass (195 passed, the two FAIL_TO_PASS among them; only `uv` for the parser is missing). Proxy variables in the environment change nothing. Nothing in either transcript touches the tree beyond `utils.py`. A third rollout, run alone, chose a different function and failed honestly (C). So: a right patch graded wrong twice inside the run and right outside it, cause not found. Not counted as agent failures; listed as a grader artefact of a new kind — in-run only — and the harness does not keep the in-run verifier log, which is what would settle it. That is the loop's own gap to close before the next such case.

**The host paused again.** The second pause in two cycles: containers created 55 minutes earlier whose PID 1 had ten minutes of process time; the kernel's notices "nothing has happened for 46m" from 03:38 on three rollouts; the eval log silent from 03:32 to 04:28. Wall times from this cycle are not to be read. One rollout was lost to it (the 143 above).

Ninety-nine fresh instances across cycles 29–38 (100 drawn, one instance's second rollout lost and re-run), thirty-eight failures, none outside the account. Cumulative read: **325**.

Spend $23.21.

## Cycle 39 (2026-09-19) — the harness keeps the verifier's log; requests-6028 explained; a correct fix stashed away by its own reproduction

**Harness change ([#734](https://github.com/unarbos/arbos/pull/734), branch `cursor/swebench-verifier-log-7c9c`).** `cleanup` — which verifiers runs after the task's score — now copies the in-run verifier's output (`/logs/verifier/report.json`, `reward.txt`, and the test script's mktemp log) into the rollout's artefact folder under `verifier/`. Checked on requests-6028, two rollouts, $4.44: both present, and the report answers cycle 38 at once. **In-run, both FAIL_TO_PASS tests pass and seven PASS_TO_PASS tests fail** — `TestGetEnvironProxies.test_bypass[…]` ×5, `test_set_environ[no_proxy-None]`, `test_zipped_paths_extracted` — because they read the process's proxy environment, and the verifiers runtime sets `http_proxy`/`https_proxy`/`no_proxy` for its egress proxy in every process it runs, the grader included. Offline, in a fresh container with the network open, no proxy is set and they pass. requests-6028 is ungradeable under this harness; the cycle-38 discrepancy is closed and it moves from "unexplained" to the grader-artefact list with its cause. (The credential in that log is the run's own local proxy secret; the harness keeps it as-is because it is dead the moment the run ends.)

**Conditions for the fresh read.** Kernel **`arbos-kernel 0.2.0 04345f70e141 protocol 1`** = `main` head, built in the worktree, label proved; the engine changes since 1ec3de64 (grep skips `.git/`, a part-written record is cut back, an unwritable config folder no longer kills the kernel) are not contract changes. Jev off. Network cut, no cap. Ten never-run instances (`fresh10k.txt`) at `-r 2`, pre-registered ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-39-preregistration.md)). $11.32; **16 of 20** solved (a count). The host paused a third time (06:01–06:49; three rollouts each "nothing has happened for 48m", PID 1 seconds old in 48-minute-old containers); walls are not read.

**Four failures, now read with the verifier's own report beside the transcript.**

- django-14534 ×1 — C, a detail: `self.data['attrs']['id']` where the gold has `.get('id')`; one of two hidden tests raises `KeyError`. The report says so directly: FAIL_TO_PASS 1 of 2, PASS_TO_PASS 118 of 118.
- pylint-4604 ×2 — C, the hidden tests grade a thing the issue never names: **the agent's change to `variables.py` is the gold's, line for line** (`isinstance(type_annotation, astroid.Attribute)` → recurse on `.expr`). The gold also adds `IS_PYPY` to `pylint/constants.py`; the hidden test module imports it; without it the module fails to import and all 21 tests fail — FAIL_TO_PASS 0 of 21, the log's first line `ImportError … from pylint.constants import IS_PYPY`. Not reachable from the issue.
- **pytest-10356 ×1 — a correct fix lost to the kernel, outside the agent account.** The agent wrote the gold's design (`get_unpacked_marks(obj, *, consider_mro=True)` walking `__mro__`, each class's own `pytestmark`), verified it — `changes` at step 65 shows both files modified and 89 tests passing — and the patch at exit was **0 bytes**; the verifier saw the base tree. Cause: reproduction 2, recorded at step 42, was `cd /testbed && git stash && python -m pytest /tmp/verify_markers2.py …`. The gate accepted it (it runs code). `changes` re-runs every recorded reproduction, so every `changes` call ran `git stash` and moved the fix into the stash. The agent found the stash after the first `changes` (step 61: `git stash list` → `stash@{0}`), popped it, wrote "my earlier bash call with repro:true auto-stashed changes via the baseline snapshot mechanism" — the right observation with the wrong actor — and called `changes` once more before finishing, which stashed the fix again; 20 s later the turn ended with a clean tree. **A reproduction re-run must not change the tree**: a recorded command that stashes, checks out, resets or restores is not a reproduction of the bug, it is a manoeuvre around the fix, and re-running it at the done check undoes the work it is meant to check. This is the third gate finding in four cycles (servers, cycle 36; the word-match, cycle 38; tree side-effects here). Recorded, not filed.

Cumulative read: **331** (325 + the two 6028 rollouts that settled the artefact + these four).

Spend $15.76.

## Cycle 40 (2026-09-19) — ten more fresh instances; "no change needed" when the wrong output is gone but the right one is not there

**Conditions.** Kernel **`arbos-kernel 0.2.0 3784c20d6516 protocol 1`** = `main` head, built in the worktree, label proved; the engine changes since 04345f70 (`find` skips `.git/`, CRLF kept through an edit, a model fallback held for the turn) are not contract changes. Jev off. Harness d1226d8c (#734). Network cut, no cap. Ten never-run instances (`fresh10l.txt`) at `-r 2`, pre-registered ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-40-preregistration.md)). $7.67; **16 of 20** solved (a count). The host paused a fourth time (eval log silent 07:39:43–08:27:38; three rollouts affected); walls not read.

**Four failures, two instances, both consistent across their pair.**

*pylint-4661 ×2 — C, a dependency the issue does not name.* Both rollouts make `PYLINT_HOME` follow XDG (`$XDG_DATA_HOME/pylint`, the spec the issue cites). The gold uses `appdirs.user_cache_dir("pylint")` and adds `appdirs` to `setup.cfg`; the hidden test module does `import appdirs`, which is not installed, so the module fails to import — the verifier report: FAIL_TO_PASS 0 of 1 on `ModuleNotFoundError: No module named 'appdirs'`. Not reachable from the issue.

*sympy-23950 ×2 — the no-change verdict on a checkout that has moved past the issue's text: E2 inside the no-change path, with G-decline as the argument.* The issue says `Contains(x, Reals).as_set()` returns `Contains(x, Reals)` and shows `Piecewise` crashing on `as_relational`. In this checkout `as_set` already `raise NotImplementedError()`, and the `Piecewise` example no longer crashes. Both rollouts ran the example, saw neither the reported output nor the reported crash, traced the change to an ancestor commit ("confirmed via `git merge-base --is-ancestor`"), and finished with **"no change needed"** and a 0-byte patch — 16 and 27 tool calls. The hidden test asks `Contains(x, S.Reals).as_set() == S.Reals`: the gold is one line, `return self.args[1]`. So the request's example does not do the *wrong* thing any more, and the agent took that for doing the *right* thing. The request said what right is — "Contains is not a set", so `as_set` should give the set — and a `NotImplementedError` is not a set. This is E2's shape (the reproduction proves the crash is gone, not that the behaviour is there) arriving through the no-change-needed exit, which asks the agent to run the request's example before declaring no change: the example was run, and what it showed was read as "fixed" because it was not the reported failure. If the no-change rule wanted to hold here it would have to say: the example must do what the request asks, not merely stop doing what the request reports. Two of two rollouts, so not a coin. Recorded, not filed.

One hundred and nineteen fresh instances across cycles 29–40, forty-six failures, none outside the account. Cumulative read: **335**.

Spend $7.67.

## Cycle 41 (2026-09-19) — the tree-moving-reproduction step where it was found; ten more fresh instances

**Conditions.** Kernel **`arbos-kernel 0.2.0 18f397cb34ab protocol 1`** = `main` head, built in the worktree, label proved. The engine changes since 3784c20d: **42753ae5**, Features' step on the cycle-39 finding (read from this document; nothing was filed) — a reproduction whose command moves the working tree, index or HEAD is refused with what it would do to the fix, never taken as the last failing command, never re-run, and a re-run that moved the tree anyway is said under its verdict; plus non-UTF-8 files read with a note and refused for edit with what to do, and `read` streaming pages under a cap. Jev off. Harness d1226d8c (#734). Network cut, no cap. Two reads, concurrent, pre-registered ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-41-preregistration.md)). $14.69. The host paused a fifth time (eval log silent 08:55:44–09:43:06); walls not read.

**Read 1 — 42753ae5 on pytest-10356, three rollouts: the step was not exercised, and the loss did not recur.** None of the three offered a tree-moving command as a reproduction (seven offers, all plain pytest runs on a scratch file), so the refusal had nothing to refuse and no "CHANGED THE WORKING TREE" line appeared. One rollout ran `git stash` three times by hand, outside the gate, and its tree at exit still carried its fix. All three patches were non-empty (3.3–4.2 KB) and matched what their last `changes` showed. One of three solved; the two that did not are C, consistent — the MRO walk built differently from the gold's and the hidden test pinning the result. So the loop can say the cycle-39 shape did not recur in three draws and cannot say it saw the refusal; the step's own unit test is the evidence for the refusal, not this read.

**Read 2 — ten never-run instances at `-r 2`: 17 of 20** (a count). Three failures:

- django-13401 ×1 — **A, the twin, evident from the issue.** The issue's symptom is a `set` of fields de-duplicating across models — equality *and hashing*. The agent changed `__eq__` (line for line the gold's) and `__lt__`, and left `__hash__` on `creation_counter` alone; the hidden test's `assertNotEqual(hash(a), hash(b))` fails, `1172 == 1172`. Fourteen tool calls.
- matplotlib-20676 ×2 — C, consistent: right file, right class; the agent freezes and restores `dataLim`/`viewLim` around the handle lines (one rollout) or moves the placeholder rectangle inside the data limits (the other), where the gold seeds the handles from the axis bounds; both hidden tests fail on the bound the gold preserves.

One hundred and twenty-nine fresh instances across cycles 29–41, forty-nine failures, none outside the account. Cumulative read: **340** (335 + 3 + the two C in read 1).

Spend $14.69.

## Cycle 42 (2026-09-19) — the no-change rule's new predicate on its own example; the last never-run instances

**Conditions.** Kernel **`arbos-kernel 0.2.0 aa0f61da94a2 protocol 1`** = `main` head, built in the worktree, label proved. Engine changes since 18f397cb: **d8344ff2**, Features' step on the cycle-40 finding (read from this document; nothing was filed) — the no-change rule now says "already behaves as asked" is the asked output present, "not that the wrong output the request reports is gone … the symptom moved, not the fix present"; and 2ad607b8 (a BOM is the file's, not line 1's). Jev off. Harness d1226d8c (#734). Network cut, no cap. Two reads, concurrent, pre-registered ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-42-preregistration.md)). $7.51. The host paused a sixth time (eval log silent 10:24:51–11:00:53); walls not read.

**Read 1 — d8344ff2 on sympy-23950, three rollouts: the choice did not move.** Three of three declare no change with a 0-byte patch, in 14, 16 and 21 tool calls, on exactly the predicate the new text forbids: "`as_set()` already raises `NotImplementedError` (not `Contains(x, Reals)`)", "no `AttributeError` any more", "already fixed in the current checkout". One writes the rule's mark in form — "No change: `<command>` — already raises NotImplementedError" — with the forbidden content inside it. Five of five across cycles 40 and 42, two kernels. Two things to say plainly. The prose did not reach the choice: the agent reads "as asked" as "not as reported" whatever the rule says, so if this is wanted it is a check, not a sentence — the no-change verdict would have to name the asked output and show it, and a verdict that names only the reported output's absence would be refused. And the instance is a hard one for the rule: the issue never states the asked output — it says "Contains is not a set", and the set is what the hidden test wants; a `NotImplementedError` is a defensible reading of "not a set". So the account holds it as E2 in the no-change path with a share of C. Recorded, not filed.

**Read 2 — the last eight never-run instances at `-r 2`: 7 of 16** (a count). Nine failures:

- django-14315 ×2 — **A, the twin**: the gold fixes the postgres client (`env or None`) *and* the base client (merge `os.environ` when an env is given); both rollouts fix the postgres client alone, in 12 and 13 tool calls, and the two failing hidden tests are the base client's. Identical 554-byte patches.
- django-12193 ×2 — **B, the producer**: the issue says `CheckboxInput` mutates the `attrs` dict it is handed; both rollouts make the caller (`SplitArrayWidget`) pass a copy; the gold stops `CheckboxInput.get_context` mutating; the hidden test is `CheckboxInput…not_mutate_attrs`.
- django-11141 ×2 — **E1 invoked on a recollection**: the FAIL_TO_PASS test passes; one PASS_TO_PASS test (`test_load_empty_dir`) fails in both. The first rollout ran the loader tests, saw it fail, and reasoned: *"This test asserts the old, incorrect behavior … I recall the actual Django fix for this ticket did remove `test_load_empty_dir`"* — and shipped. Upstream kept it; the gold keeps an empty migrations directory unmigrated with a `migration_names` check. The existing-test exception was taken on memory of upstream, not on a read of the test. Third time this cycle-35 shape appears (16560's `__repr__`, 23950's git history, here) — **a recollection of upstream standing in for evidence** is now the account's most specific G-decline.
- django-13837 ×1 — C: the `-m` detection is the gold's, with extra changes around it that break two neighbouring tests.
- scikit-learn-25747 ×2 — C, consistent: skip the index when lengths differ, where the gold ignores it whenever the output is already a DataFrame; the hidden test pins the gold's rule.

**The never-run pool is spent.** Every instance in the loop's order has now been drawn at least once: 137 fresh instances across cycles 29–42, 58 failures, none outside the account. "A fresh ten when there is nothing else to read" has nothing left to draw from. What fresh material means next — second draws on once-failed instances, a re-read of an early cycle's set on the current kernel, or waiting on new failures only — is the coordinator's call; the loop will not pick for itself.

Cumulative read: **352** (340 + 9 + 3).

Spend $7.51.

## Cycle 43 (2026-09-19) — second draws on the once-failed instances: sixteen of sixteen repeat failures fail the same way

**The new material.** The never-run pool is spent; the coordinator's word is second draws on once-failed instances. The pool: every instance with a cut-era result that failed at least once and is not a grader artefact — 64 instances, 29 failed in every draw — ordered by fewest draws first (33 have exactly two), then the loop's stratified order (`second-draw-pool.json`, `second-draw-order.json`). This cycle took the first ten: all from cycles 29–32, two draws each, 1 of 20 prior rollouts solved.

**Conditions.** Kernel **`arbos-kernel 0.2.0 bec7284b8740 protocol 1`** = `main` head, built in the worktree, label proved; the engine changes since aa0f61da (checkpoint `add -A` retries, a read-only file refused with the way through) are not contract changes. Jev off. Harness d1226d8c (#734). Network cut, no cap. Pre-registered ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-43-preregistration.md)). $12.00. Seventh host pause (eval log silent 11:25–12:17); walls not read. **3 of 20** solved — on a pool selected for failure, a count that compares with nothing.

**What the second pair showed against the first.**

- Two instances flipped: sympy-19495 (first draw 1 of 2) solved both; sympy-18211 (first draw 1 of 2) solved one of two again. These are the pool's flippy members and behave as flippy.
- **Eight instances failed both draws again, and every one of the sixteen rollouts failed the same way as its first pair** — the same class, the same site, in several the same code:
  - django-12325: `base.py` alone, `options.py` untouched, both times — **A, the twin**, four of four.
  - django-14034: `MultiValueField.clean()` in `fields.py`, where the gold and the hidden test are `boundfield.py` rendering — C, four of four.
  - django-15695: a name-equality guard in `RenameIndex.database_forwards` both times; the unnamed-index test fails — C, four of four.
  - django-16667: `except (ValueError, OverflowError)` both times, the pinned `"0-0-0"` return not produced — C, four of four (the second draw in 9 and 14 tool calls).
  - matplotlib-23299: the right symptom, a mechanism other than the gold's `del orig['backend']` — this time one rollout in `pyplot.switch_backend` — C, four of four.
  - sympy-15875: an `im_I` list summing the imaginary terms — the *same construction* as cycle 30, near line for line — where the gold is a one-line condition — C, four of four.
  - sympy-20438: `issubset.py` alone, the `Eq` handler in `comparison.py`/`relational.py` untouched — **A, the twin**, four of four.
  - sympy-21596: the base set restricted to the roots, the gold's `_solution_union` not built — C, four of four.

So the account's class for an instance is a property of the instance, not of the draw: across sixteen repeat failures the model went to the same file and made the same kind of change it made a day earlier on a different kernel. Three consequences worth stating. The account's per-instance readings from cycles 29–42 can be trusted as readings of the instance. A second draw on a twice-failed instance buys almost no new information about *where* it fails — it confirms; so the pool's remaining 54 are best spent on the 33 two-draw instances first (as ordered), and the many-draw regression members not at all. And the two twins (12325, 20438) are now four-of-four misses on sites the issue text points to (`parent_link` validation; `Eq` beside `is_subset`), which is what a twin rule with a visible step would have to reach.

Cumulative read: **369**.

Spend $12.00.

## Cycle 44 (2026-09-19) — second draws, the pool's next ten: this set flips more, and still fails the same way

**Conditions.** Kernel **`arbos-kernel 0.2.0 249ddb5f9f1e protocol 1`** = `main` head, built in the worktree, label proved; the engine changes since bec7284b (Jev's pickable tools; a child agent's shell write into a root-owned file refused) do not reach a root agent under this harness. Jev off. Harness d1226d8c (#734). Network cut, no cap hit per turn. The pool's next ten (`second10b.txt`, all from cycles 33–38) at `-r 2`, pre-registered ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-44-preregistration.md)). The cycle's $22 cap stopped the run at seventeen rollouts (these instances are dear: 16560 and 3069 run long); the two incomplete pairs (sympy-13852, sphinx-8056) were finished alone, so the set has 21 rollouts. $25.30. Eighth host pause (12:59–13:53, and a short one 13:55–14:04); walls not read. **10 of 21** solved — a count on a pool selected for failure.

**Against the first pairs.** Unlike cycle 43's set (0 of 8 twice-failed instances flipped), this set moved: django-16560 (01 → 11) and seaborn-3069 (01 → 11) solved both; django-14170 (00 → 10), django-11433 (00 → 01) and sphinx-7985 (00 → 10) each got a solve; pytest-7205 went the other way (10 → 00); sympy-13852 stayed flippy (01 → 10). Three of five twice-failed instances produced a solve on the second pair — so on this set a second draw did add information about *whether* the instance is reachable, where cycle 43's did not. What did not change is *how* the failures fail: of the eleven failed rollouts, ten fail at the same site and in the same class as their first pair, and the eleventh (14170 `ff4ace98`) in the same class at a different site:

- django-16502 ×2 — the `write()` override again, four of four (five with cycle 38's re-run); C.
- pylint-4551 ×2 — `inspector.py` alone against a four-file gold and ten `.dot`-output tests, four of four; C, the redesign shape.
- pytest-7205 ×2 — the bytes-only branch again where the gold quotes every value with `saferepr`; nine of ten hidden tests fail on the quoting; C, pinned format. Cycle 33's solved rollout used `saferepr` for all; this pair's both did not.
- sphinx-8056 ×1 — `docfields.py`, the consumer, again; B.
- django-11433 ×1, sphinx-7985 ×1, sympy-13852 ×1 — the same sites as their first failures; C.
- django-14170 ×1 — a new site: the `BETWEEN` lookups un-registered from `ExtractIsoYear` in `functions/datetime.py` rather than skipped in `lookups.py`; the same nine existing tests that pin `between` fail; C, the class cycle 36 gave it.

So across cycles 43–44: 27 repeat failures, 26 at the same site and class, one at a different site in the same class. The account's reading of an instance holds across draws and kernels; whether an instance is *reachable* is what a second draw can still tell, and it told it for three of five here.

Cumulative read: **380**.

Spend $25.30.

## Cycle 45 (2026-09-19) — second draws, the pool's next ten: fifteen of fifteen the same way

**Conditions.** Kernel **`arbos-kernel 0.2.0 249ddb5f9f1e protocol 1`** (cycle 44's; `main` at 3bccf924 with no engine or host change since). Jev off. Harness d1226d8c (#734). Network cut, no cap. The pool's next ten (`second10c.txt`, all from cycles 39–42) at `-r 2`, pre-registered ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-45-preregistration.md)). $10.30. Ninth host pause (14:48–15:39); walls not read. **5 of 20** solved — a count on a pool selected for failure.

**Against the first pairs.** Like cycle 43's set and unlike cycle 44's, nothing twice-failed moved: seven instances that failed both first draws failed both again (0 of 7), and the two flippy ones (django-13401, django-13837: 10 → 11) and django-14534 (10 → 01) behaved as flippy. **All fifteen failures fail at the same site and in the same class as their first pair:**

- django-14315 ×2 — the postgres client alone, `env or None`, 14 and 16 tool calls, the base client untouched: **A, the twin**, four of four with near-identical 550-byte patches. One rollout saw the two base-client tests fail and wrote *"Only the two expected pre-existing tests fail (they assert the exact buggy behavior)"* — the existing-test exception applied to the tests that encode the fix. That sentence is the twin miss and E1's misuse in one line.
- django-12193 ×2 — the caller copies the dict, `CheckboxInput` still mutates: **B**, four of four.
- django-11141 ×2 — the `__file__` guard removed and nothing put in its place; `test_load_empty_dir` fails; four of four (C, with cycle 42's recollection reading).
- pylint-4604 ×2 — `variables.py` the gold's again, `IS_PYPY` never added; C, four of four.
- pylint-4661 ×2 — XDG data dir where the gold and the hidden test want `appdirs`; C, four of four.
- matplotlib-20676 ×2, scikit-learn-25747 ×2 — right file, other mechanism; C, four of four each.
- django-14534 ×1 — `self.data['attrs']['id']`, the same `KeyError` detail as cycle 39's failure; C.

Across cycles 43–45: **42 repeat failures, 41 at the same site and class**, one (cycle 44's 14170) in the same class at a different site. The per-instance reading is settled; the second-draw pool's remaining 34 are the many-draw regression members and the flippy tail, and the loop expects nothing new from them about *how* an instance fails. The pool's value now is reachability only, and cycles 43–45 put that at three flips in nineteen twice-failed instances.

Cumulative read: **395**.

Spend $10.30.

## Cycle 46 (2026-09-19) — second draws, the pool's next ten: the regression-era instances

**Conditions.** Kernel **`arbos-kernel 0.2.0 249ddb5f9f1e protocol 1`** (cycles 44–45's; `main` at 354413e0 with no engine or host change since). Jev off. Harness d1226d8c (#734). Network cut, no cap. The pool's next ten (`second10d.txt`: three two-draw instances from the cycle 12–28 reads and seven four-draw instances from the regression sets) at `-r 2`, pre-registered ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-46-preregistration.md)). Tenth host pause (eval log silent 16:13–17:01, and 17:18–17:33); one rollout died in it — the model stream came back dead after the pause, "did not answer (2 tries)", turn failed at step 12 with no patch — and was re-run alone. 21 rollouts, $25.98. **9 of 21** solved — a count on a pool selected for failure.

**Against the earlier draws.** The four instances with any earlier solve (django-11133, django-14017, django-14771, sympy-13878) solved both this time. Of the six that had never solved, one flipped: **django-14792**, 0000 then B (the consumers in four backends' `operations.py` fixed instead of `timezone._get_timezone_name`) in the first rollout, and on the re-run the gold's file and a full pass — its first solve in six draws. The other five stayed at zero. **All eleven read failures fail at the same site and in the same class as their earlier draws:**

- astropy-13398 ×2 — the direct ITRS↔AltAz/HADec module written as the issue asks, 10–15 KB; the hidden tests want refraction and the topocentric path the accepted PR grew into; C, the scope-grew shape, six of six.
- xarray-6992 ×2 — the same one-line `coord_names` fix, 477 bytes, 12 tool calls each; twelve hidden tests want the `indexes.py` redesign; C, four of four.
- sphinx-7590 ×2 — user-defined literals parsed in `cpp.py`/`cfamily.py`, `c.py` untouched and the AST shape not the gold's; `test_expressions` fails; C, four of four.
- django-15732 ×2 — **A, two halves of one fix**: the gold passes `primary_key: False` to `_delete_composed_index` *and* filters the primary-key constraint out in `_constraint_names`; one rollout did the first half, the other the second, each alone; the hidden test needs both. Six of six.
- django-16877 ×2 — `escapeseq` written; two of four hidden tests fail on how a non-string element is escaped; C, six of six.
- django-14792 ×1 — B, five of six (the sixth solved).

Across cycles 43–46: **53 repeat failures, 52 at the same site and class.** Reachability on twice-failed instances: four flips in twenty-five (43: 0/8, 44: 3/5, 45: 0/7, 46: 1/6).

Cumulative read: **406**.

Spend $25.98.

## Cycle 47 (2026-09-19) — second draws, the pool's next ten: the many-draw members, and a no-change verdict that follows the rule and still fails

**Conditions.** Kernel **`arbos-kernel 0.2.0 249ddb5f9f1e protocol 1`** (cycles 44–46's; `main` at 262044ee with no engine or host change since). Jev off. Harness d1226d8c (#734). Network cut, no cap. The pool's next ten (`second10e.txt`: four- to eleven-draw instances) at `-r 2`, pre-registered ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-47-preregistration.md)). $15.28. Eleventh host pause (18:04–18:57); walls not read. **10 of 20** solved — a count on a pool selected for failure.

**Against the earlier draws.** The instances with a prior solve behaved as their histories said: astropy-12907 (000011 → 11), django-11099, django-13449, sphinx-8265 solved both; pytest-10356 (01010 → 10) and sympy-18698 (010001 → 01) stayed flippy; sympy-15017 (7 of 11 → 00) went down. Of the three never-solved, none flipped (0 of 3; across cycles 43–47, four of twenty-eight). Ten failures, all at the same site and class as before:

- **django-13513 ×2 — a no-change verdict that meets the rule's predicate and is still wrong for the grade.** The issue says `explicit_or_implicit_cause()` ignores `__suppress_context__`; in this checkout it already reads it, and both rollouts **ran the issue's own example** (`raise ValueError from None` inside an `except RuntimeError`) against the unmodified tree and showed the rendered debug page no longer names `RuntimeError` — the asked output present, as d8344ff2 asks. They declared no change in 4 and 6 tool calls; the hidden test is `test_suppress_context_without_traceback`, an exception with no traceback, which the issue never mentions and which the gold reaches by restructuring frame assembly. Eleven of eleven draws fail on this instance, and this pair is the first the loop has read closely: **C, a case beyond the issue, with the no-change rule honestly satisfied.** It is the contrast to sympy-23950 — same verdict, opposite evidence — and it puts a ceiling on what the no-change rule can ever fix: when the asked output is present for the request's example and the hidden test wants a case the request does not name, no rule about evidence reaches it.
- sympy-23950 ×2 — no change on "already raises `NotImplementedError` (not `Contains`)", the "No change:" line written in form; **seven of seven** across three kernels. Recorded, not filed.
- django-15503 ×2 — the ambiguous-key JSON path built differently from the gold's; ten of ten; C.
- sympy-15017 ×2 — `__len__` in `ndim_array.py` in one, `_loop_size` across three files in the other, where the gold is one line in `dense_ndim_array.py`; C.
- pytest-10356 ×1, sympy-18698 ×1 — the same designs as their earlier failures; C.

Across cycles 43–47: **63 repeat failures, 62 at the same site and class.**

Cumulative read: **416**.

Spend $15.28.

## Cycle 48 (2026-09-19) — the second-draw pool's last fourteen: the regression members hold their shapes; the pool is spent

**Conditions.** Kernel **`arbos-kernel 0.2.0 bd50ec58eddc protocol 1`** = `main` head, built in the worktree, label proved; the one engine change since 249ddb5f (7ccaa99a: the third `await` ceiling in a row says the job is not finishing) is a tool behaviour, not a contract change. Jev off. Harness d1226d8c (#734). Network cut, no cap hit. The pool's last fourteen (`second14f.txt`, 14–33 draws each) at `-r 2`, 28 rollouts, pre-registered ([preregistration](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/swebench-cycle-48-preregistration.md)). $27.44. Two host pauses (19:36–20:35, 20:41–21:31); one rollout died in the second — the model stream came back dead mid-fix, "did not answer (2 tries)", a half-applied patch left in the tree (matplotlib-24870 `d87bab3b`, 53 PASS_TO_PASS failures from the partial edit) — not read as an agent failure. Walls not read. **18 of 28** solved — a count on a pool selected for failure; these members' long histories say what to expect and this pair matched them.

**Against the earlier readings.** The seven high-rate members (astropy-13236, astropy-14182, django-11728, django-16454, pytest-6197, scikit-learn-14629, sphinx-8035) solved both. The flippy ones flipped (xarray-6938, pylint-6386, pylint-8898, django-15252: one of two each; matplotlib-24870: 0 of 1 read). The two near-floor members stayed there (django-15022 now 1 of 26, sympy-17318 now 4 of 23). Nine failures read, all at the same site and class as their earlier readings:

- django-15022 ×2 — per-word `Exists`/lookup application in `get_search_results`, where the gold restructures the join; the three hidden tests still fail; C.
- sympy-17318 ×2 — `split_surds` guarded in `radsimp.py`, `sqrtdenest.py` untouched: **B, the producer**, the cycle-27 instance, nineteen of twenty-three now.
- xarray-6938 ×1 — `swap_dims` in `dataset.py`, `variable.py`'s `to_index_variable` returning `self` untouched: **B**, the other cycle-27 instance.
- django-15252 ×1 — `recorder.py` (where the table is made) rather than `executor.py` (where migration is decided): B, as read before.
- pylint-6386 ×1 — `nargs=0` on `verbose` alone against a four-file gold; the `-v` instance of cycle 20's E2 reading; C.
- pylint-8898 ×1 — the twin `_regexp_paths_csv_transfomer` named in the reply, `utils.py` untouched; C.
- matplotlib-24870 ×1 — both files, a `levels_are_default` flag where the gold checks dtype; `test_bool_autolevel` fails; C.

Across cycles 43–48: **72 repeat failures, 71 at the same site and class.**

**The second-draw pool is spent.** Sixty-four instances, 112 rollouts, $115 across cycles 43–48. What it gave: the per-instance reading of the account holds across draws and kernels (71 of 72), and reachability moved on four of twenty-eight twice-failed instances. What it did not give: any failure outside the account, on any draw. The loop has now read every instance in its order at least twice and every once-failed instance at least four times.

Cumulative read: **425**.

Spend $27.44.

## Hold (from 2026-09-19 22:10 UTC)

Both pools are spent and the coordinator's word is to hold: read new failures and landed steps as they arrive, invent no other source. Checked at 22:10 UTC: `main` at 833da250, 37 commits past the cycle-48 kernel, none under `crates/arbos-engine/src/` or `crates/arbos-core/src/host.rs` (the work is the mobile journey loop) — nothing for this loop to read. Keeper checks: 22:33 UTC, unchanged; 2026-09-20 00:11 UTC, `main` at cbb2907a, 50 commits past the kernel, still none under the engine or the host — nothing to read; 01:33 UTC, `main` at 5e501cef (#804, deploy/mobile), 53 commits past the kernel, none under the engine or the host — nothing to read; 03:09 UTC, `main` at 2364bcb8 (#805, deploy/mobile), 56 commits past the kernel, none under the engine or the host — nothing to read; 04:30 UTC, `main` at 1647b90a (#806, deploy/mobile), 59 commits past the kernel, none under the engine or the host — nothing to read; 06:00 UTC, `main` at 85ea2395 (#807, deploy/mobile), 62 commits past the kernel, none under the engine or the host — nothing to read; 07:30 UTC, `main` at dcf8313c, 64 commits past the kernel, none under the engine or the host — nothing to read; 09:00 UTC, `main` at a6817288, 70 commits past the kernel, none under the engine or the host — nothing to read; 10:30 UTC, `main` at 4ee41ee8, 73 commits past the kernel, none under the engine or the host — nothing to read; 12:00 UTC, `main` at aed86a0b, 76 commits past the kernel, none under the engine or the host — nothing to read. The kernel the loop would build next is whatever `main` head carries the first engine change; the harness stays at d1226d8c (#734). Jev off.

**Paused 2026-09-20 12:03 UTC** on Jacob's word to every Arbos worker, relayed by the coordinator: no checks of `main`, no cycle 49, idle until he says resume. Nothing was running; nothing is.

## Next (cycle 49, when there is something to read)

1. A landed step on a recorded observation is read where the observation was made (the pattern of cycles 38, 41, 42); a new failure class arriving from elsewhere is read against the account. Nothing else runs.
2. Any measured comparison: ceiling on its set stated first, at or above the band, or it does not run.
3. Jev stays off on this harness (coordinator, cycle 32). #734 stays as it is.
4. Observations recorded here, not filed anywhere: the quoted-reference mark is read as "test the example" (31); a generated artefact rode into a patch after the agent removed it (33); `run --timeout` does not interrupt a blocking tool (36); the server-reproduction refusal keys on a word (38); the no-change predicate change did not move the choice — seven of seven (42, 47); a recollection of upstream standing in for evidence (35, 40/42, 42); repeat failures repeat their class and site — 71 of 72 (43–48); a failing test that encodes the fix dismissed as "asserts the buggy behavior" (45); two halves of one fix, each rollout doing one (46); a no-change verdict with the rule's evidence honestly met and the hidden test beyond the issue (47).
