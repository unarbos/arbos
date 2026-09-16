# qa-035: `subscribe kind=timer` with `cmd` and `deliver_to = "user"` is refused instead of read as a shell subscription

- Feature: `subscribe` tool (#104), `main` @ `318b57ae` (+#217)
- Severity: medium for the benchmark: kickoff item 9 ("standing loop") fails whenever the model picks `kind = timer` for a command, which gpt-4.1-mini does about half the time.
- Rollouts: `/tmp/qa-staging/20260915T04*-kickoff-session` (two runs), root's `subscribe` call

## Repro

Root calls `subscribe {op: "add", kind: "timer", cmd: "python3 toy-repo/hello.py", every: "1h", deliver_to: "user", notify: "QA loop result: {output}", prompt: "…"}`.

## Expected

A `cmd` with `deliver_to = user` and a `notify` template is unambiguously a shell subscription. Either write it as `kind = shell`, or refuse with the one-line fix: "a command runs as kind = shell; set kind = shell (deliver_to = user then posts the output)". Same rule as qa-019 (`kind="default"`) and qa-031 (`host="local"`): models fill every field; where the meaning is clear, the kernel takes it.

## Actual

`deliver_to = user is for shell (a command with no model turn); a timer wakes the agent`. The model did not retry; no `subscriptions/*.toml` was written; the standing loop never existed.

## Suspected location

`crates/arbos-kernel/src/tools.rs` subscribe add: when `cmd` is non-empty and `kind` is `timer` (or empty), set `kind = shell` and say so in the body.

## Fix

PR #218: `Subscription::coerce` reads a `timer` (or no kind) with a `cmd` as `shell`, and a user-delivered `notify` without `{output}` as `label: {output}`; applied in the tool, in `add`, and for hand-written files; the result says `Read as: …`. E2e `timer_cmd_e2e` with the exact call from the rollout.
