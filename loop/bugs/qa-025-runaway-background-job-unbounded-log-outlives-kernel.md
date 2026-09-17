# qa-025: a runaway background job writes an unbounded out.log (8.7 GB in an hour) and outlives the kernel

status: confirmed on the feature branch `cursor/job-streaming-b027` (inbox note job-streaming); the job model is the same on `rust`
severity: high (disk exhaustion; a CPU-bound process left running after the kernel is gone; the turn that started it never ends)
scenario: inbox:job-streaming (the note's exercise starts `yes`, `yes > /dev/null &` and an infinite printf loop as background jobs)
rollout: /home/ubuntu/arbos-qa/loop/rollouts/20260913T120732Z-inbox:job-streaming (job logs removed: jobs/j1/out.log had reached 8.7 GB)
feature: job-streaming
fingerprints: 271d0c9a32 886ce71e03 b234886744 0724b505e4 2940065caf

## What happened

The model ran `bash background:true command:"yes"`. The kernel's job wrote every line to `jobs/j1/out.log` with no cap. The turn awaited the stream and never ended (300 s scenario timeout). On SIGINT the kernel did not stop cleanly (lock left behind, the driver killed it). The `yes` process was still alive 60 minutes later at 100% CPU; its log was 8.7 GB.

## Expected

- A job's `out.log` is capped (rotate or truncate to the last N MB; the transcript already shows only head/tail).
- A job that outlives its kernel is a choice, not an accident: at least the kernel's graceful stop should kill jobs the current turn started, or the job should be capped in wall time when nothing awaits it.
- A turn awaiting a stream that never ends must end at its own timeout with an interrupted line.

## Suspected location

`crates/arbos-engine/src/jobs.rs` (out.log writer, no cap; jobs deliberately survive the kernel), `crates/arbos-kernel/src/serve.rs` stop path (jobs are not killed), `crates/arbos-engine/src/tools/bash.rs` await/streaming on the job-streaming branch.
