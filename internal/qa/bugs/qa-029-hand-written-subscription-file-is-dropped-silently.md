# qa-029: a hand-written subscriptions/*.toml without id/created/next_due is dropped silently

- Feature: plan engine replacement, PR #104 (`cursor/plan-engine-b027`), seen on #106 @ `271e9b4`
- Severity: medium. The file system is the interface; a file the kernel cannot use must be said somewhere.
- Scenario: `fp-shell-subscription` (check `bare-subscription-silently-ignored`; first rollout `internal/qa/rollouts/20260913T172637Z-fp-shell-subscription/`)

## Repro

Write `agents/root/subscriptions/0001-tick.toml` the way the inbox note and the design describe a subscription:

```toml
kind = "shell"
cmd = "echo shell-tick >> ticks.txt"
every = "30s"
prompt = "tick ran"
```

Start the kernel. Wait a minute.

## Expected

Either it runs every 30 s (the kernel fills `id` from the file name, `created` = now, `next_due` = now), or `kernel.log` / stderr says `0001-tick.toml: missing field id` and the user is told once.

## Actual

Nothing. 0 runs in 40 s, no line anywhere. `Subscription` has `id: u32` and `created: String` without serde defaults, so `toml::from_str` fails, and `load()` does `.filter_map(|p| read(&p).ok())` — the error is thrown away. A file with `id` and `created` but no `next_due` parses and also never fires (`is_due` needs `next_due`), again silently.

## Suspected location

`crates/arbos-core/src/subscription.rs`: `load()` (swallowed error), `Subscription` (no defaults for `id`, `created`; `next_due` not derived from `created` + `every` when absent). Suggest: `#[serde(default)]` for `id` (fall back to the `NNNN-` file prefix) and `created`; in `load()`, log parse failures with `klog` once per file (dedupe by mtime) and write a `kernel.log` line; when `next_due` is absent and `every` is set, treat the file as due now.
