# qa-024: a job killed at `bash_wait_ms`/timeout reports "no exit recorded" instead of the kill

status: confirmed (low)
severity: low (the agent recovers, but the record does not say the kernel killed it)
scenario: swebench-nightly
rollout: /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa/rollouts/swebench/psf__requests-2317 (network tests ran past 600 s; 2 "no exit recorded" lines in run.jsonl)
fingerprints: none

## Expected

A job the kernel killed ends with an explicit status ("killed by the kernel after Ns") and an `exit` file, so `jobs`/`await` and the transcript say what happened.

## Actual

`await` reports "no exit recorded"; the agent has to guess and retries with `timeout 30`.

## Suspected location

`crates/arbos-engine/src/jobs.rs` `kill` / `status_line`: the kill path does not write the `exit` marker.
