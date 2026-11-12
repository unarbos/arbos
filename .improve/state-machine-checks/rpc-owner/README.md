# Main Pi child ownership

The actual `PiSlot<T>` owns Vacant, Starting, Checking(child), Ready(child),
Exited(child), Parked or Stopped. Its single transition function guards every
slot write by an opaque boot ticket. Effects run after the slot lock is released.
Boot guards own spawned/removed children before their next cancellable wait;
terminal shutdown owns checking children as well as published children. Cleanup
uses captured handles and preserves the existing five-second grace. Runtime
teardown has a best-effort nonblocking handle fallback, without a new runtime or
PID-derived authority; synchronous final reaping after runtime loss is not claimed.

Before the change, six actual Agent tests had1 positive control and5 semantic
failures: initial/respawn publication after shutdown, aborted unpublished-child
leak, old Ready overwrite and stale abort clearing new boot admission. The final
selected native suite passes30 distinct cases (7ownership,4slot,11crash/supervisor,
2model-switch,4turn-end,2compaction). Existing Linux `/proc` RSS measurement is
unsupported on Darwin: the unchanged full crash suite had11P/1F there. That case
remains enabled for final fenced Linux CI. `NATIVE-GATES.json` records exact
sources/counts/commands; `CALIBRATION.json` pins13 unchanged diagnostic/timing/RPC
methods and existing Duration calls. Full workspace verification is separate.

From the repository root, with cached harness dependencies and Rust/Node22:

```sh
node --test harness/arbos-pi-ext/rpc-owner-laws.test.ts
python3 .improve/state-machine-checks/rpc-owner/check_mutants.py --output /tmp/arbos-rpc-mutants-1
cargo test --locked --offline -p arbos-agentd shutdown_boot_regressions -- --test-threads=2
```

Use synthetic HOME/store and the established loopback-only OS fence for the Cargo
child/relay fixtures. No real pi, provider, account or user process is needed.
The std-only law bridge has no runtime or process effects. Three Effect Schema
laws exercise768 seeded traces: stale ownership refusal with current progress,
terminal absorption, and exactly-once return of attached local resource handles.
A fourth test checks bounded malformed-frame rejection and recovery. Three actual
function mutations falsify and fail exact canonical replay, with original hashes
unchanged. Their first failing inputs were already canonical; accepted shrink
count is zero. This is finite testing, not an exhaustive proof.

## Review and remaining integration

Review intentional terminal/cancel behavior before merge. Root also identified
late SwitchSession/SetModel metadata completions as a related ownership lead;
actual response-barrier pins and any necessary narrow repair are in progress.
The current30 native/768 law results do not certify that unfinished follow-up.
