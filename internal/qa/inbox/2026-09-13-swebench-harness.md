---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: Arbos as a SWE-bench harness (Prime Intellect verifiers)

Branch `cursor/swebench-harness` (on the integration branch) → PR against `rust`. Results doc: [swebench-run-2026-09-13](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-run-2026-09-13.md). Raw run: `media/swebench/2026-09-13/`.

## What it is

- `harness/Dockerfile` → image `arbos-harness`: static (musl) `arbos-kernel` + `arbos-swe-run`.
- `arbos-swe-run` (in `harness/arbos_harness/`): headless entry. Writes `config.toml` from env, writes a root `agent.md` (mode `auto`, no `ask`/browser/screen tools), runs `arbos-kernel run --json --timeout`, denies dangerous-command approvals (exit 3 → `answer --deny --follow`, max 3 rounds), kills the kernel, exports the rollout bundle, writes `patch.diff` (`git add -A && git diff --cached <base>`), `result.json`, `kernel.log`, `run.jsonl` to `/logs/artifacts/arbos`. Exit 0 whenever a transcript exists; the kernel's own exit lives in `result.json.kernel_exit`.
- `arbos_harness` Python package: verifiers v1 `Harness` plugin, id `arbos-harness`. Uploads the kernel into each task's own container (SWE-bench instance image), points it at the interception endpoint (`ARBOS_API_BASE`, `ARBOS_API_KEY`, provider `custom`), records `arbos_*` metrics, copies the artifacts to the host.
- New kernel env vars: `ARBOS_MODEL` (over `config.toml`), `ARBOS_MODE` (mode of a root agent minted fresh).

## How to run

```bash
# one time
docker build -f harness/Dockerfile -t arbos-harness .
uv venv .venv && source .venv/bin/activate
uv pip install "verifiers[harbor]>=0.3.1" prime -e harness
prime env pull primeintellect/swebench-verified && uv pip install -e swebench_verified

# one instance, cheap model, through the real grader
export OPENROUTER_API_KEY=...   # op item get xbirrctuljw2m6aieoway2szom --vault Arbos --fields label=credential --reveal
.venv/bin/eval swebench-verified \
  --env.agent.harness.id arbos-harness --env.agent.harness.timeout 900 \
  --env.agent.harness.artifacts ./outputs/arbos-artifacts \
  --env.agent.runtime.type docker \
  --env.taskset.tasks astropy__astropy-12907 -n 1 \
  -m deepseek/deepseek-v4.1-flash \
  --client.base-url https://openrouter.ai/api/v1 --client.api-key-var OPENROUTER_API_KEY \
  --no-push -o outputs --run.dir smoke --rich.show-logs true
```

Notes: the task filter takes the folder name (`astropy__astropy-12907`), not `swe-bench/...`. If your user is not in the `docker` group, wrap in `sg docker -c '...'` and call the eval by full path (`eval` is a shell builtin). `traces.jsonl` in the run dir is the sample: every model call (messages, tool calls, usage, cost) plus `rewards.solved` and `metrics.arbos_*`. `-c 3` runs three containers at once; 4 CPUs / 15 GB was enough.

Standalone (no verifiers), any repo:

```bash
docker run --rm -e OPENROUTER_API_KEY -e ARBOS_MODEL=anthropic/claude-sonnet-5 \
  -v "$PWD/repo:/testbed" -v "$PWD/out:/logs/artifacts/arbos" arbos-harness "<task text>"
```

Replay a bundle without a model: `arbos-kernel rollout replay out/rollout/<stamp>-root`.

## Run result (16 instances, Sonnet 5, $8.02)

12/16 solved (7/8 easy, 5/8 hard). Failing bundles in `internal/qa/rollouts/swebench/` (`rollout.tar.gz` = the Arbos bundle; `arbos-kernel rollout replay` runs it without a model):

- `pydata__xarray-6992` — stopped after 10 calls; one-line fix; hidden tests cover more of the issue.
- `pylint-dev__pylint-8898` — rewrote the existing test the grader uses (`test_csv_regex_error`).
- `astropy__astropy-13398` — relaxed an existing test's tolerance; missed refraction handling.
- `psf__requests-2317` — grader artefact (gold patch scores 0 here too; network tests).

## What breaks or wastes — attack here

0. **Edits existing tests to make its change pass** (2 of 3 real failures). Nothing in the contract forbids it. Try: a task whose existing test contradicts a tempting fix; the agent should treat the test as the spec or say the test is wrong — not edit it silently. Also try: "the issue implies more than the MVCE" (xarray style) and see whether the agent looks for the full scope before stopping.
1. **Python env discovery (biggest waste).** The kernel's `bash` runs without a login shell, so the image's conda env `testbed` is not on PATH. In 6 of 8 easy instances the agent spent 3–8 bash calls finding the interpreter and, on astropy, `pip install`ed numpy/cython/pyerfa/pytest into the *base* python before finding `/opt/miniconda3/envs/testbed`. Try: a repo whose tests only run inside a venv/conda env; count wasted calls. Fix idea: run bash as `bash -lc`, or probe `conda env list` / `.venv` at bootstrap and put it in the instance prompt.
2. **No user, but the prompt still talks to one.** The contract says "ask the user", "commit on a branch", "deliver an image". Headless runs get a standing `instructions.md` from the wrapper telling the agent not to commit or ask. Try: remove `ARBOS_INSTRUCTIONS`/the default and see whether the agent commits (git guard blocks unverified identity → wasted turns) or asks (exit 3).
3. **Bash jobs that never end.** `pytest test_requests.py` (network tests) ran past `bash_wait_ms` (600 s), became a job, and was killed with "no exit recorded". The agent recovered with `timeout 30`. Try: a test suite that hangs; check the kernel's `wait_ms` handling and whether the agent loops on `await`.
4. **Exit 3 path.** No instance triggered an approval (sudo, `rm -rf /`, mkfs). Force one: a prompt that needs `sudo`. Expected: denied, turn continues, `approval_rounds` > 0 in `result.json`.
5. **Kernel lifetime.** `run` spawns `serve` in its own process group; the wrapper kills it by pid from `kernel.json`. If the kill fails, the grader's tests run beside a live kernel with idle jobs. Check `ps` in the container after a run.
6. **`.arbos/` inside the repo.** Excluded via `.git/info/exclude`; the patch uses `git add -A`. A repo without `.git` gets no patch at all (`patch_bytes 0`). Try a non-git workdir.
7. **Timeout accounting.** `ARBOS_TIMEOUT` covers only the kernel turn; `rollout export` and `git diff` add seconds after it. verifiers has its own `--env.timeout.*`; if it fires first, the artifacts are not written.
8. **Cost visibility.** The kernel's own `turn_complete.usage.cost` (e.g. 0.2297 for requests) matched OpenRouter within a cent, but only for the last turn; per-call costs come from the trace, not from Arbos.
