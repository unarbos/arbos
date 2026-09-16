# qa-037: `spawn host=<machine> wait=true` returns on the remote's first notice, not on the worker's report

- Feature: remote places over ssh (#229), `main` @ `848e0039`
- Severity: medium: root answers the user before the remote worker has done anything ("It has only reported initial notices about syncing and setting up the environment. I will wait…"); the real report lands as a later `say` and costs another turn.
- Scenario: `rm-01-remote-install-and-spawn` (check `rm-01-wait-returned-early`); rollout `internal/qa/rollouts/20260915T161302Z-rm-01-remote-install-and-spawn/`

## Repro

machines.toml with a loopback machine; root: `spawn host=loop wait=true wait_secs=240 task="run hostname; whoami; pwd and report"`.

## Expected

The tool call blocks until the worker's report (its last words), as the local `spawn wait=true` does; the tool result carries the three lines.

## Actual

The tool returned as soon as the relay forwarded the remote's first messages (sync/setup notices). Root's reply: "…has only reported initial notices about syncing…". The worker's `say` with `hostname: cursor` arrived ~10 s later as a separate message.

## Suspected location

`crates/arbos-kernel/src/remote.rs` `spawn_ssh` / the relay: the wait receiver resolves on the first relayed line instead of the child's report.

## Fix

PR #239 (`cursor/remote-leash-wait-b027`, main). The remote branch of `spawn` ignored `wait`; it now blocks on the worker's report like the local branch (`wait_secs`, interrupt, "still working after Ns"), and the relay resolves the waiting parent with the report as the tool result (`KernelHooks::resolve_wait`) — no message on top, and the report is on the parent's transcript once (it used to land twice: once from the relay, once from the inbox claim). Test: `remote::lifecycle_tests::a_remote_report_answers_the_waiting_spawn_or_files_a_done`. Re-run `rm-01-wait-returned-early`: the tool result should carry the three lines.
