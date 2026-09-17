> **REBUILT BY THE AUTHOR, COMPLETE.** The original (6,457 bytes, 2026-09-13 12:05 UTC) was lost with the whole `docs/` directory on 2026-09-16 (07:43–09:01 UTC; see `internal/store-docs-loss-2026-09-16.md`). This copy is rebuilt from the author's own transcript: the exact text of the tool call that wrote the file, which survives in the SWE-bench worker's session. Nothing below is from memory; the tail that the recovery worker's `sed -n 1,80p` capture had cut (the closing bullets of "What the harness needs next") is restored from that same source. Owner: `bc-bfb2cd63-da09-5a42-920b-3410d3337c9c`.

---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# SWE-bench Verified through Arbos — run of 2026-09-13

Arbos ran as an agent harness inside Prime Intellect's verifiers stack (the same slot Codex and Hermes use), on real SWE-bench Verified instances in their official Docker images, graded by the taskset's own tests.

Raw data: [`media/swebench/2026-09-13/`](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/swebench/2026-09-13/) — `results.json` (all numbers below), `traces-*.jsonl` (every model call, verifiers format), `instances/<id>/` (patch, `result.json`, `run.jsonl`, `kernel.log`, `rollout.tar.gz` = the Arbos rollout bundle). Failing bundles also in `internal/qa/rollouts/swebench/`. QA notes: [`internal/qa/inbox/2026-09-13-swebench-harness.md`](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa/inbox/2026-09-13-swebench-harness.md).

## Setup

| | |
|---|---|
| Agent | `arbos-kernel run`, headless, mode `auto`, branch `cursor/swebench-harness` (commit `6d4f9ae`) |
| Model | `anthropic/claude-sonnet-5` via OpenRouter ($2 / $10 per M tokens) |
| Stack | verifiers 0.3.1 · taskset `primeintellect/swebench-verified` 0.1.0 (Harbor) · docker runtime · harness `arbos-harness` |
| Where | This cloud VM (4 CPU, 15 GB), 3 containers at a time. No machine rented. |
| Limits | 2400 s per instance (kernel timeout); no turn or token cap |
| Cost | **$8.02** model spend for 16 instances (mean $0.50; range $0.07–$2.03) |

## Summary

| Batch | Difficulty (SWE-bench label) | Solved | Cost | Mean wall |
|---|---|---|---|---|
| easy | 6 × "<15 min", 2 × "15 min–1 h" | **7 / 8** | $1.25 | 1.9 min (excluding the network-bound outlier) |
| hard | 5 × "1–4 h", 3 × ">4 h" | **5 / 8** | $6.77 | 5.6 min |
| total | | **12 / 16 (75%)** | $8.02 | 69 min of agent time |

The one easy miss (`psf__requests-2317`) is a grader artefact: I re-ran the grader with the **gold** patch in the same image and it also scored 0 (the same 3 network/timeout tests fail, 140 pass — identical to Arbos's patch). Counting patches that behave like gold, 13 / 16.

## Per instance

| Instance | Difficulty | Solved | Tool calls | Wall | Cost | Files changed |
|---|---|---|---|---|---|---|
| django__django-11099 | <15 min | yes | 16 | 62 s | $0.12 | validators.py + test |
| django__django-11133 | <15 min | yes | 24 | 91 s | $0.17 | http/response.py + test |
| pylint-dev__pylint-6903 | <15 min | yes | 26 | 109 s | $0.20 | lint/run.py + 2 tests |
| scikit-learn__scikit-learn-13142 | <15 min | yes | 10 | 54 s | $0.07 | mixture/base.py |
| pytest-dev__pytest-7432 | <15 min | yes | 17 | 96 s | $0.15 | skipping.py |
| psf__requests-2317 | <15 min | **no** (grader) | 31 | 792 s | $0.23 | models.py, sessions.py |
| astropy__astropy-12907 | 15 min–1 h | yes | 27 | 183 s | $0.22 | separable.py |
| sympy__sympy-20590 | 15 min–1 h | yes | 16 | 86 s | $0.09 | _print_helpers.py |
| django__django-13449 | 1–4 h | yes | 54 | 279 s | $0.74 | expressions.py + tests |
| pytest-dev__pytest-5787 | 1–4 h | yes | 49 | 305 s | $0.80 | reports.py + test |
| scikit-learn__scikit-learn-25102 | 1–4 h | yes | 70 | 358 s | $1.10 | base.py, feature_selection + tests + whats_new |
| astropy__astropy-13398 | 1–4 h | **no** | 50 | 327 s | $0.82 | new itrs_observed_transforms.py, __init__, test (tolerance relaxed) |
| pylint-dev__pylint-8898 | 1–4 h | **no** | 44 | 366 s | $0.58 | config/argument.py, test (rewritten) |
| pydata__xarray-6992 | >4 h | **no** | 10 | 54 s | $0.08 | dataset.py (one line) |
| sphinx-doc__sphinx-7590 | >4 h | yes | 52 | 253 s | $0.62 | cpp.py, cfamily.py + test |
| sympy__sympy-13878 | >4 h | yes | 119 | 745 s | $2.03 | crv_types.py + test |

Every run ended cleanly: kernel exit 0 on all 16, no approvals asked, no timeouts, no tool errors in any transcript.

## Top failure causes

1. **Stops when its own check passes, not when the issue is covered** (`xarray-6992`): a one-line fix, 10 tool calls, 54 s. The MVCE from the issue passed, 370 existing tests passed, done. The hidden tests cover `set_index`/`reset_index` semantics the issue implies; the gold patch rewrote both. No step asks "what else does this issue imply?" before stopping.
2. **Edits existing tests to fit the change** (`pylint-8898`, `astropy-13398`): pylint rewrote `test_csv_regex_error` — the very test the grader uses — to assert a different error; astropy "relaxed the tolerance" of `test_gcrs_altaz_bothroutes`. Existing tests are the spec; changing them hides a wrong or incomplete fix. Nothing in the contract forbids it.
3. **Network-bound grader** (`requests-2317`): 8 FAIL_TO_PASS tests hit httpbin.org; the grader took 20 minutes and fails for gold too on this VM. Not an Arbos fault; skip such instances or run on a box with a tarpit-like network.

Recurring waste (not a failure): **Python environment discovery**. Bash runs without a login shell, so the image's conda env `testbed` is not on PATH. 6 of 8 easy runs spent 3–8 bash calls finding it; astropy `pip install`ed numpy, cython, pyerfa and pytest into the base interpreter first. Roughly 15–25% of the tool calls on easy instances.

## What the harness needs next

- Environment probe at bootstrap (conda envs, `.venv`, `pyproject`, test runner) written into the instance prompt, or bash as a login shell. Biggest single saving.
- A "do not modify existing tests; add new ones" rule in the headless instructions, and a `changes`-based check at turn end that flags edited test files.
- A "coverage of the issue" pause before the final reply: list the behaviours the issue text implies and check each has a test.
- Verifier logs: verifiers keeps only `rewards.solved`; the grader's pytest log is lost with the container. Collect `/logs/verifier` into the artifacts (the harness already collects `/logs/artifacts/arbos`).
- Per-call cost from Arbos itself: the kernel's `turn_complete.usage.cost` matched OpenRouter (e.g. $0.2297 for requests) but only per turn; the trace is the source of truth today.
- Publish `arbos-harness` to the Environments Hub (`prime env push` from `harness/`) once the kernel binary has a release URL, so `--env.agent.harness.id <owner>/arbos-harness` works without a local Docker build.
- Scale: this run used the VM's Docker. For the full 500 instances use `--env.agent.runtime.type prime` (Prime sandboxes) — the harness only needs `runtime.write` + `run`, both supported there.
