# qal-j13: a migration cut mid-way is kept as `plan.jsonl.migrating` and said only in kernel.log — the person is not told, and would not recognise the file

- **Closed 2026-09-17 06:16 UTC against #392 @ `ac831935`** (`arbos-kernel 0.2.0 ac831935e1de`, built and run here). The author went further than asked: a cut migration is now *finished*, not left — what it already wrote is recognised, the rest carried over once — and the person is told on the transcript: *"An earlier start began carrying over the old plan (plan.jsonl) and was cut before it finished. Finished now: 1 standing subscription(s) carried over this time; 0 were already in place and were not written again. The old file is kept as plan.jsonl.migrated."* `sw-06`: exactly one cron, the notice present, the source kept under a name that says what it is. Controls the same minute: #392 @ `92f6eb59` silent with `plan.jsonl.migrating` left; `main` @ `7f6a6b9a` silent. `sw-05` still green on `ac831935`.
- Measured at: #392 @ `92f6eb59` (`arbos-kernel 0.2.0 92f6eb59a5bd`); the cut-case handling is new in this commit, so there is no older kernel to compare (on `a0f2a92d` and `main` @ `7f6a6b9a` a `.migrating` file is simply ignored — also silently).
- Class: misreport, not destructive. Filed because the coordinator asked for exactly this check: the author's judgement that visibly losing the unwritten tail beats silently doubling every cron is right — but the loss is not visible.
- Scenario: `sw-06-migration-cut-leaves-something-a-person-can-finish`; rollout `internal/qa/rollouts/20260917T053502Z-sw-06-…`.

## Repro

`.arbos/agents/root/plan.jsonl.migrating` holding one legacy standing cron, no `plan.jsonl`, no new records — the state after a start moved the file aside and died. Start the kernel; send a line.

- Not migrated again: zero cron files. Good.
- The source is kept, as `plan.jsonl.migrating`. Good.
- `kernel.log`: `"event":"migrate_cut"` — *"an earlier migration of plan.jsonl was cut mid-way; the nodes it wrote stand, and its source is kept at … for a person to read. It is not run again: a second pass would double …"*. Good words.
- **The transcript: nothing.** The window shows an ordinary place. The standing cron the person had is gone from what runs, and the only trace is a file with an extension nobody has seen, findable only by someone who reads `kernel.log`.

## Expected

The same words that went to `kernel.log` go to the transcript as a failed notice, once — *"An earlier migration of your old plan was cut mid-way. What it had migrated stands; the rest is kept at `.arbos/agents/root/plan.jsonl.migrating` for you to read. Tell me what in it you still want and I will set it up."* — and the kept file could carry a first line saying what it is and why it is there, so a person opening it in an editor knows.

## Actual

`Claim::Cut` → `klog::warn("migrate_cut", …)` only; `Claim::Blocked` writes a transcript notice, `Cut` does not.

## Suspected location

`crates/arbos-kernel/src/migrate.rs`, the `Claim::Cut(source)` arm in `run()` (both the per-agent and the `subscriptions.json` one): append the notice beside the log line, as `Blocked` does.

## Fix

#392 @ `ac831935` (see the closing line). Regression check: `sw-06` (a transcript notice naming `plan.jsonl.migrating`).
