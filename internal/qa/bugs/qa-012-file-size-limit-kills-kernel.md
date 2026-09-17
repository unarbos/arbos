# qa-012: a write past the file-size limit kills the kernel (SIGXFSZ) and leaves half a line

status: pr-open — https://github.com/unarbos/arbos/pull/25 (branch `cursor/fix-qa-012-sigxfsz-partial-write-de28` -> `rust`)
severity: high (crash; damaged plan file; stale lock)
scenario: disk-full
rollout: /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa/rollouts/20260913T004841Z-disk-full
fingerprints: a9db899838 b35f84144f 38666df0f4 22537835bf

## Repro

Run the kernel with `RLIMIT_FSIZE = 256 KB` (`ulimit -f 256`), send a 200 KB prompt.

## Expected

The write fails with EFBIG, the kernel reports it and stays up, no partial line on disk.

## Actual

Kernel exit code -25 (killed by SIGXFSZ). `plan.jsonl` is exactly 262144 bytes, cut mid-line; `.arbos/lock` left behind; attempt a1 never ended. The same partial-line damage happens on ENOSPC without the crash.

## Suspected location

`crates/arbos-kernel/src/main.rs` (no SIGXFSZ handling); `crates/arbos-core/src/files.rs` `append_events` and `node.rs` `append_line` (no cleanup after a failed `write_all`).
