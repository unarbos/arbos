# qa-011: a prompt to a read-only agent folder vanishes without a word

status: confirmed (client-side notice covered by #14; the kernel keeps serving and recovers once writable)
severity: medium (message lost silently)
scenario: readonly-arbos
rollout: /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa/rollouts/20260913T004818Z-readonly-arbos
fingerprints: c6af8bb3a9

## Repro

Start the kernel, `chmod -R a-w .arbos/agents/root`, send a `user` frame to root.

## Expected

An `error` frame (or at least a `notice`) telling the client the message could not be stored; the kernel stays up.

## Actual

No `turn`, `error` or `event` frame within 8 s. Kernel stderr: `inbox root: append .../plan.jsonl: Permission denied`. The kernel survived and served root again once the folder was writable.

## Suspected location

`crates/arbos-kernel/src/serve.rs` `Frame::User` -> `hooks.inbox` error goes to `eprintln!` only. #14 turns every refused frame into an `error` frame; this scenario is its check.
