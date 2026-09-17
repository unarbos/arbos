# qa-031: `spawn host="local"` is refused on a fresh place; a plain "spawn one worker" request fails

- Feature: kernel `spawn` tool, integration head `cursor/release-integration-52cd` (kernel built 17:33 UTC)
- Severity: high for the multitasking work: every delegation scenario in the audit trips on it before reaching the item under test.
- Found by: `mt-02-kernel-steer-lands-at-boundary` (rollout `internal/qa/rollouts/20260913T204928Z-mt-02-kernel-steer-lands-at-boundary/`)
- Status: fix PR [#121](https://github.com/unarbos/arbos/pull/121) into the integration branch. Detector `spawn-host-refused` in every `mt-*` spawn scenario names it.

## Repro

Fresh place, no `machines.toml`, no hub roster. Prompt root: "Spawn one worker named 'Slow count' with wait=true … run `sleep 40; echo finished`".

## Expected

One local child.

## Actual

The model fills the optional `host` with `"local"`; the kernel answers `no machine named "local" in …/machines.toml or .arbos/machines/. Known: (none; add a [[machine]] …)`. The model tries `host: "host1"`, is refused again, and asks the user which machine to use. No worker, two wasted turns.

## Cause

`crates/arbos-kernel/src/tools.rs` spawn: any `host` goes to `remote::spawn_remote`, which only knows machines.toml and the hub roster. Same shape as qa-019 (`kind="default"`): models fill every optional field with a plausible default.

## Fix (#121)

`remote::is_local_host` / `choose_host`: the spellings of "here" mean a local child; with no machines configured anywhere, any host runs here and the tool body says so; a wrong name among real machines is still refused with the list. Test `crates/arbos-kernel/tests/spawn_host.rs`.
