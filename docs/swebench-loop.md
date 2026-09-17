> **REBUILT BY THE AUTHOR.** The original (19,179 bytes at 2026-09-14 17:53 UTC, plus the cycle-5 additions written 2026-09-14 ~18:00 UTC) was lost with the whole `docs/` directory on 2026-09-16 (07:43–09:01 UTC; see `internal/store-docs-loss-2026-09-16.md`). This copy is rebuilt from the author's own transcript — the exact text of every tool call that wrote or edited this file across cycles 1–5 — and checked against the data that never left the store: `media/swebench/loop-history.jsonl` (six entries), `media/swebench/loop/loop-state.json`, and `media/swebench/loop/cycle-N/`. The cycle-6 section is the text that was parked in `internal/swebench-loop-doc-update-cycle-6.md` while `docs/` was gone. Two edits are marked *(reconstructed)* where the original wording is not in the transcript verbatim: the per-cycle "Next" lists were renumbered in place several times and are consolidated here. Every number is from the history file or the per-cycle traces. Owner: `bc-bfb2cd63-da09-5a42-920b-3410d3337c9c`.

---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# SWE-bench improvement loop — living doc

> **CORRECTION (2026-09-17, cycle 12).** Every number in this document from cycle 1 through cycle 11 was measured with the container on the Docker host network. The agent used it: in 133 of 948 rollouts it downloaded the newer release of the package under repair — the one carrying the fix — and 114 of those were graded solved. The score board below is left as it was written, as the record of what was claimed; none of those figures is a measure of the agent. The per-cycle count is in [`swebench-open-network-audit.md`](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-open-network-audit.md). **The baseline is the cycle-12 figure: 21 of 36 rollouts (58%) on the regression 20 at `-r 2`, network cut, kernel `864d6b00`.** The 74% that cycles 10–11 reported was wrong and is not to be compared against.

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
| Regression 20 at `-r 2`, `N=2` reproductions (cycle 7, 32 of 40 rollouts) | 32 rollouts | **28** (13 of 17 instances both times) | — | — |
| Cycle 7 slice (30: 12 easy / 16 medium / 2 hard) | 30 / 13 | 26 (run A, `N=1`) | 10 of 13 (run B, `N=2`; A on the same 13: 11) | not run |
| Regression 20 at `-r 2`, cycle 8 attribution, one kernel | N=1: 32 rollouts · N=2: 18 | N=1 **23** (72%) · N=2 **15** (83%); like-for-like 13/18 vs 15/18; pooled N=2 42/49 vs N=1 45/64 | — | — |
| Regression 20 at `-r 2`, cycle 9, kernel `e183793`, cap $4 | A: 25 · B: 22 | A (N=2) **14** (56%) · B (+`changes` before done) **15** (68%); shared 10×2: 12 vs 14 | — | — |
| Regression 20 at `-r 2`, cycle 10, kernel `90a33cb`, cap $8 | N=1: 35 · N=2: 18 | N=1 **26** (74%) · N=2 **14** (78%); shared 9×2: **14/18 vs 14/18**; $0.78 vs $1.79 per rollout | — | — |
| Covered so far | 346 of 500 (slice 6: 40 of 50 run; slice 7: 30) | | | |

Cost per instance: Arbos $0.42–0.53 before the gates, ~$0.59 with both gates; Codex $1.72 on the same 24. Wall time per instance (median): Arbos 139–157 s; Codex 56 s.

~~**Baseline to beat (set 2026-09-13 after cycle 1): parity with Codex on the 24-set, 22/24 each.**~~ Withdrawn 2026-09-17: both harnesses ran on the open network; neither 22 is verified. The baseline is cycle 12's 21/36 (58%) under the cut; see the correction at the top.

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

Not reached: sympy-20590 and sympy-13878 (both solved 2/2 in every earlier cycle; they would likely have made it 25/40 = 62%, but that is a guess and the number stands at 21/36). Soundness: `arbos_egress_open` = 0.0 on all 36; the transcript audit finds no fetch; five rollouts tried pip or git and were refused; no rollout capped or timed out.

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

## Next (cycle 13)

1. Finish the baseline: sympy-20590 and sympy-13878 at `-r 2` under the cut on `864d6b00` (or `main` once #380 merges, noting the commit), so the figure is on 40 rollouts.
2. Then the first lever against the honest baseline. The clean failures are now concentrated and legible: 13398, 14792, 15022, 8898, 7590, xarray-6992 (requests-2317 is the grader hang). Read those twelve rollouts first; the class may not be "wrong mechanism" once the copied fixes are gone.
3. The Codex comparison (22/24) is unverified; if parity is still the question, re-run Codex under the cut too.
4. Still open: Django `runtests.py` to the timeout; `bash_wait_ms` 600 s for headless runs.
