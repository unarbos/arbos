# P-09 background job streaming — QA note (features agent, 2026-09-13)

Branch `cursor/job-streaming-b027`, base `rust`.

## What it does

- Kernel: every 200 ms tick, for each **detached** job (a bash command that outlived its `wait_ms`, or `background:true`), the kernel reads the new bytes of `jobs/jN/out.log` and broadcasts `Frame::Job { agent, id, delta, running, exit }`. A delta is capped at 16 KB per tick; when more arrived, the frame carries the last 16 KB and a `[… N bytes skipped]` marker. When the job ends, one last frame has `running: false` and the exit code (`null` = killed).
- Desktop: `Bind::Process` gains `live` (the streamed tail, capped at 64 KB) and `done`. The process surface shows `live` when frames have arrived, so remote places (no file access) stream too; the local file tail is the fallback for old kernels. The footer reads `running` / `exited N`.
- Nothing changes for the model: `await` and `jobs` are as before.

## Attack ideas

1. A job printing 10 MB/s (`yes`): frames capped at 16 KB per tick → ~80 KB/s over the socket; the surface keeps 64 KB. Check the desktop stays responsive and the skip marker appears.
2. A job that prints no newline for a minute (progress bar with `\r`): delta arrives; the surface renders carriage returns literally — cosmetic, note it.
3. Two detached jobs at once: frames interleave by id; each surface only takes its own.
4. Kernel restart while a job runs (jobs survive as processes): offsets restart at the current file size → no replay of old output; the surface shows only new bytes plus the file tail fallback for local places. Check the two do not both show.
5. Job ended between ticks: the final frame must still carry the last bytes and `running:false`. Check `exit` matches the `exit` file.
6. Remote place via ssh tunnel (#33): the surface must show output without file access. That is the point of the feature; verify on ArbosLife if time.
7. Non-UTF-8 output (binary): lossy conversion; no panic.
8. A journal over 256 KB before the first frame: only the new bytes stream; the local fallback tail shows the old part. Fine.
9. Frame::Job for an agent whose surface is not open: dropped, no memory kept. Then `jobs` → the row opens → the file tail fills it (local).
10. Desktop older than kernel: unknown frame type must be ignored (serde `other`?) — check the desktop's Frame enum tolerates unknown variants.

## How to run

Prompt: `bash: for i in $(seq 1 20); do echo tick $i; sleep 1; done` with `background: true`. Open the process row under the chat: lines appear one per second; footer flips to `exited 0`.
