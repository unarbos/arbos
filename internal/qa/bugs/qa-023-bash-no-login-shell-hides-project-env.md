# qa-023: `bash` runs without a login shell, so the image's conda env is off PATH and 3-8 calls per task go to finding the interpreter

status: confirmed (harness side mitigable; kernel side is the root)
severity: medium (cost and time: 6 of 8 easy SWE-bench instances; astropy compiled twice)
scenario: swebench-nightly
rollout: /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa/rollouts/swebench/astropy__astropy-13398 (17 conda.sh/activate lines in run.jsonl), /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa/rollouts/swebench/pylint-dev__pylint-8898, /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa/rollouts/swebench/psf__requests-2317
fingerprints: none

## Repro

A container whose Python lives in a conda env activated by `/etc/profile.d` or `~/.bashrc`. Ask the agent to run the tests.

## Expected

`bash` sees the same PATH an interactive user would (login shell, or the harness exports the env), so `python -m pytest` works on the first call.

## Actual

`python` is the system one; the agent probes `which python`, `conda env list`, `source .../conda.sh && conda activate testbed` for several calls before every test run.

## Suspected location

`crates/arbos-engine/src/tools/bash.rs`: the child runs `sh -c`/`bash -c` with the kernel's environment. Options: `bash -lc` (login shell; slower, side effects), or honour a place-level `.arbos/env` file the harness writes (PATH, VIRTUAL_ENV) and the kernel merges into every job's environment.
