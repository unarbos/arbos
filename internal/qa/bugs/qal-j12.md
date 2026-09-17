# qal-j12: when the migration's rename of `plan.jsonl` fails, the next start migrates again — the standing cron exists twice and fires twice, the pending task is queued twice

- Measured at: `main` @ `7f6a6b9a` (`arbos-kernel 0.2.0 7f6a6b9a06bc`), #390 @ `9ade320f` (`9ade320f958a`), #392 @ `a0f2a92d` (`a0f2a92dbf3a`) — fails on all three the same way. Replay provider, no model.
- Family: qal-j08's, from the census's "misreport-only" list on a second, adversarial look. `migrate.rs` writes the new records (subscriptions, inbox files, notes lines) and then `let _ = std::fs::rename(plan.jsonl → plan.jsonl.migrated)`. The rename is the only thing that says "done". When it fails, nothing is misreported — the next start simply does it all again.
- Severity: **high, destructive by repetition.** A standing shell cron migrated twice runs its command twice per period for ever (a deploy, a `git push`, a notification — doubled); a pending user task migrated twice is queued and run twice. Nothing on the transcript says why. It happens on the first start after an upgrade of every place that still has a `plan.jsonl` — Jacob's older places — and it needs only the agent folder to refuse one rename (a permissions slip, a partial view on a mount) while its subfolders accept writes.
- Scenario: `sw-05-failed-migration-rename-doubles-crons-and-tasks`; rollouts `internal/qa/rollouts/20260917T052236Z-sw-05-…` (#392), `…052245Z` (#390), `…052253Z` (`main`).

## Repro

`.arbos/agents/root/plan.jsonl` in the legacy form: one shell node (`every_ms: 30000`, `echo legacy-tick >> ticks.txt`) and one pending agent node ("Reply with the single word MIGRATED."). Injector: the agent folder itself read-only (`chmod 555`), its `subscriptions/` and `inbox/` subfolders writable. Start the kernel, stop it, start it again.

After two starts: `plan.jsonl` still present; `subscriptions/` holds **two** files carrying `legacy-tick`; `inbox/` holds **two** files (`…-user-000.md`, `…-user-001.md`) for the one pending task.

## Expected

Migration is idempotent or it says so: write the new records, then rename; if the rename fails, either undo the records just written and say "migration could not complete: plan.jsonl could not be moved aside (why); nothing migrated", or mark the migration done another way (a `migrated` record beside the new files, checked before migrating again) — and never create a subscription that already exists by identity (`legacy plan node 1`) or an inbox file for a node already queued.

## Actual

`run()` migrates every node, `let _ = rename(...)`, returns a summary line; the summary is logged even when the rename failed. The next start finds `plan.jsonl` and repeats.

## Suspected location

- `crates/arbos-kernel/src/migrate.rs::migrate_plan` (the `let _ = rename` at the end) and `run()` (the two `let _ = rename` for `attempts.jsonl` / `plan.md`); the same idempotence question for `.arbos/subscriptions.json` below it.
- The class fix's rule applies: a step that changes state acts only on a record whose write was confirmed — here, the migration should confirm the rename before it can be said to have happened, and the new records should carry the legacy node id so a second pass is a no-op.

## Fix

Not started (features agent). Regression check: `sw-05` (exactly one cron file and one inbox file after two starts, or a notice that the migration did not complete and nothing written).
