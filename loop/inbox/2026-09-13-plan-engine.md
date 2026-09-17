---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Cursor agent model, slice 2 — PR #104, branch `cursor/plan-engine-b027` (on #98 → #92)

The plan node is gone. Two files replace it per agent:

- `agents/<id>/notes.md` — the checklist the `plan` tool writes (`- [ ] [label](target) — readout` under `##` sections). Ops: set, add, check (done items sink to the end of their section; three kept), update, remove, show. **Item numbers change after a check**; the tool says so.
- `agents/<id>/subscriptions/NNNN-slug.toml` — the only clock. Kinds: `timer` (every / after / at), `shell` (cmd on every; `deliver_to = "user"` + `notify = "…{output}"` sends the reading to the user with no model turn; any failure or empty reading wakes the agent), `github_pr` / `github_ci` (repo, pr; one `[github]` message per change; ci = checks only), `inbox` (path, every: new files in a folder). Firing = one inbox file `from = "subscription:N"`, `kind = "wake"`.

## Scenario ideas

1. **Author a subscription file directly** (no model): drop `0001-tick.toml` with `kind = "timer"`, `every = "30s"`, `prompt = "…"`, `next_due = <past>`, `created = <now>` into `agents/root/subscriptions/`; within 5 s a `turns/tNNNN/cause.md` from `subscription:1` appears. With `--now` the whole thing is deterministic (see `tests/fixtures/cron-fires-and-reports`).
2. **Runaway watcher**: 50 timers all due → each fires once per scan; the agent runs one turn at a time (one claim per scan), the rest wait as inbox files. Look for duplicate firings of the same id (there should be none: `next_due` moves before the file is written).
3. **shell with deliver_to user**: `cmd = "date"`, `notify = "now: {output}"` → `[root → user]` say lines every period, no model turn, no cost. `cmd = "false"` → the agent is woken with "Subscription #N failed: exit 1".
4. **Migration**: put an old `plan.jsonl` (see `tests/fixtures/replay-turn/dot-arbos/agents/root/plan.jsonl` or any QA rollout bundle) into a fresh place, start the kernel: `plan.jsonl.migrated` + notes lines / subscription files / inbox files; `arbos-kernel check` warns on a leftover `plan.jsonl` and errors on a bad subscription file.
5. **Desktop strip**: standing rows are subscriptions (Stop pauses them; Run fires now; ✕ removes), plain rows are open notes items (✓ checks with the text as readout; ✕ removes). The panel "Project" view is the protocol worker's.

## Known gaps

- `at` is UTC; the design's `timezone` in `project.toml` is not read yet.
- A `shell` subscription's job is not leashed differently from a bash job; the #97 leash applies (dies with the kernel).
- `recreate_e2e` fails on the integration head independent of this PR.
- The `Focus = .arbos/focus` CONTRACT line is stale (file lives in `runtime/`); left for the protocol worker's prompt pass.
