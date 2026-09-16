# qa-038: every remote spawn leaves an `arbos-kernel serve` running on the machine after the parent kernel exits

- Feature: remote places over ssh (#229, #232), `main` @ `848e0039`
- Severity: medium: one kernel process and one `place--<child>` folder per spawn accumulate on the remote machine; nothing stops them when the child is done or when the parent kernel exits. Two scenarios left two kernels on the loopback "machine"; the scenarios now kill their own.
- Scenario: `rm-01-remote-install-and-spawn` (check `rm-01-remote-kernel-left-running`); rollout `20260915T161302Z-rm-01-remote-install-and-spawn`

## Repro

`spawn host=loop wait=true` a worker that runs one command; let it report; stop the parent kernel (SIGINT). On the machine: `pgrep -af '<dir>/bin/arbos-kernel serve'`.

## Expected

The remote kernel exits when its worker's turn ends, or when the parent kernel that launched it exits (the #97 leash for local jobs) — or one remote kernel serves all of a parent's children and `remotes.json` says which are live.

## Actual

`<dir>/bin/arbos-kernel serve <dir>/place--run-host-whoami-pwd-loop` keeps running after the parent stopped; a second spawn starts a second one.

## Suspected location

`crates/arbos-kernel/src/remote.rs` `spawn_ssh` (launch) and the relay's end-of-child handling; `RemoteHub::restore` re-attaches on restart, so the processes are treated as long-lived by design — then a stop path is missing.

## Fix

PR #239 (`cursor/remote-leash-wait-b027`, main). Three layers: `serve --leash 10m` on every kernel a parent starts for a child (ssh route and the hub worker's worktree kernels) — it exits by itself once no client has been attached for the span and nothing runs; the parent's SIGINT/SIGTERM stops its ssh children's kernels at once (`remote::stop_all`, the desktop's stop script); a finished remote child's reply is a `done` message now, so archiving it stops the kernel and drops its `remotes.json` record (`remote::forget`), and `restore` drops records whose agent is gone instead of starting them again. Tests: `idle::leash_tests`, `remote::lifecycle_tests`, `leash_e2e`. Re-run `rm-01-remote-kernel-left-running`: after SIGINT on the parent, `pgrep` should find nothing within ~5 s (the stop script) — and even after a crash, nothing after 10 minutes.
