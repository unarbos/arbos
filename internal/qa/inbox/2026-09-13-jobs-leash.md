---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Jobs leash (qa-024, qa-025) — PR #97, branch `cursor/jobs-leash-b027`

## What changed

- Every `bash` job runs under a leash `sh`: it watches the kernel (`$PPID`). Kernel gone → the leash writes `<job>/killed` ("killed: the kernel exited and the job was ended with it") and `kill -9`s the job's process group, itself included.
- `out.log` cap: 64 MB by default (`ARBOS_JOB_LOG_CAP` bytes). Past it, the log is cut back to `[arbos: out.log passed N bytes; older output dropped]`. If the next look (0.25 s later) finds it past the cap again, the job is ended: `killed: runaway output (over N bytes twice in a row after the log was cut back)`.
- The kernel's own kill (Stop, `run` exit, timeout) writes `killed` = "killed by the kernel" first; status lines read `killed by the kernel after 41s`. No `killed`, no `exit`, process gone → `killed by a signal from outside the kernel after Ns (no exit code)`.
- Every journal read is bounded (`read_new`, `Frame::Job` deltas were already, shell-node tails now 256 KB). Unbounded `read_to_end` on a growing log OOM-killed the kernel at 14 GB during this work.

## How to check

1. `arbos-kernel serve <place>`; `arbos-kernel run --place <place> 'bash background:true: sleep 1000; reply started'`; `kill -9 <kernel pid from .arbos/runtime/kernel.json>`; within ~1 s `pgrep -x sleep` is empty and `jobs/j1/killed` names the kernel.
2. `ARBOS_JOB_LOG_CAP=2000000` on the kernel; background `yes`; `await` → `Job j1 killed: runaway output (…)`; `yes` is gone; `out.log` stays at a few hundred MB (the window before the kill), not GBs.
3. Stop a running job via the desktop's Stop or `run` exit → `jobs` lists `killed by the kernel after Ns`.

## Not done

- Wall-time cap for a job nobody awaits (qa-025's second ask): a job dies with its kernel now; a per-job wall cap is a separate decision (a server the user asked for with `background:true` should live as long as the kernel).
- The runaway kill trips only when the writer beats the cap twice within 0.25 s; a slower flood (say 1 MB/s) is cut back for ever and never killed. That is the intended trade: a long build log is not a runaway.
