# qa-003: one large prompt makes the kernel re-parse megabytes every 200 ms; SIGINT is ignored for >10 s and the lock is left behind

status: pr-open — https://github.com/unarbos/arbos/pull/10 (branch `cursor/fix-qa-003-tail-offset-de28` -> `rust`)
severity: high for long-running agents (the main loop cost grows with transcript size, and transcripts are meant to grow without limit)
scenario: huge-input
rollout: /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa/rollouts/20260912T223613Z-huge-input
fingerprints: fbda14788b 16977971ff

## Repro

1. Start the kernel, attach, send a `user` frame whose text is ~4 MB.
2. Wait for `turn idle`, send a normal prompt, wait for `turn idle`.
3. Send SIGINT.

## Expected

- The kernel prints `arbos-kernel stopping` and exits within a second; the `lock` file is removed.
- The prompt text is stored once.

## Actual

- No `arbos-kernel stopping` in `kernel.stdout.log`; the kernel was still running 10 s after SIGINT and had to be SIGKILLed, leaving `.arbos/lock` behind (`driver.log`, `state-after/lock`).
- `state-after/agents/root/plan.jsonl` is 12.6 MB: the 4 MB goal is written three times (pending, active, done). `transcript.jsonl` is 8.4 MB (wake + user both carry the text).
- Reproduced in both runs of the scenario (the first with a slow driver, the second with a fast one).

## Suspected location

- `crates/arbos-kernel/src/serve.rs:146-147,223-288`: the `tail` interval (200 ms) calls `load_transcript` on every agent's whole transcript every tick, and `plan::scan` re-reads every `plan.jsonl` on every kick and 5 s tick (`crates/arbos-kernel/src/plan.rs:112-120`, `hooks.rs:271-273`). All of it is synchronous file I/O and JSON parsing inside the `tokio::select!` loop, so the `ctrl_c` branch (`serve.rs:290-293`) is starved.
- `crates/arbos-core/src/node.rs:348-351` `save_node` appends the full node (goal included) on every status change; `hooks.rs:423-443` `inbox` stores the whole prompt as the node goal.

## Fix ideas

- Tail by byte offset (remember file length, read only new bytes) instead of re-parsing.
- Store the prompt once on the transcript and keep the node goal clipped, or reference the transcript line.
- Move file scanning off the main loop, or cap the per-tick work.

## Note

Reproduced three times: twice with `target/debug/arbos-kernel`, once with `target/release/arbos-kernel` (rollout `20260912T225811Z-huge-input`: same two breaks, `kernel-hang-on-sigint` and `lock-leftover`). The release build shrinks the constant but not the O(size per 200 ms) shape.
