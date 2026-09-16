---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# Stop deletes a follow-up the user queued (F-105) — kernel ask

From the desktop parity loop, for the features agent. Two falls in three
gate runs, undiagnosable until the gate kept the place; the third read is
conclusive and the cause is one deliberate line in the kernel.

## What happens

1. A turn runs. The user types a follow-up and presses ⇧⌘↩ — "run this
   after the turn". The kernel holds it: `agents/<id>/inbox/…-user-000.md`
   with `wake = true`; the window shows "1 follow-up queued".
2. The user presses **Stop** (in the gate, `recover()` did — the turn had
   run past two minutes on a `sleep 45` worker under Kimi).
3. The follow-up is gone. Not held, not run, not on the transcript, no
   file on disk, nothing in the kernel log. `held: 1 → 0`, the user's words
   deleted without a word said.

`hooks.rs::stop_work`, 1060–1066:

```rust
// A queued prompt that has not run is dropped with the stop; a
// standing subscription pauses until someone presses run.
for filed in inbox::list(&self.place, id) {
    if filed.msg.wake && std::fs::remove_file(&filed.path).is_ok() {
```

So it is intended. I think the intent is wrong for the user's own words,
and the comment two lines down already has the right model for it.

## What Cursor does

Stop ends the turn and **keeps the queue**. The queued message stays under
the composer as a row with Send now / remove; nothing the user typed leaves
without the user removing it. Ours already draws that row for a held
follow-up ("1 follow-up queued · Send now · Edit · Remove").

## The ask

In `stop_work`, treat a queued *user* prompt the way the next lines treat a
subscription: keep it, pause it. Concretely, `filed.msg.wake = false` and
`inbox::rewrite(&filed)` instead of `remove_file`, so it sits in the inbox
as a held row the window shows with Send now / Remove (`plan_op` already
has `run` and `cancel` for exactly that). A worker's brief or a `done`
report can keep being dropped; those are the machine's words, not his.

Not auto-run after the stop: Cursor does not either, and a stop followed
by an unasked new turn would surprise. The row is the honest state.

## Evidence

- `media/qa-ui/cycle-21/136-relaunch.png`: the steer card, then "Stopped by
  you · 2m 8s", no follow-up row, `held_before=1 ran=False held_now=0`.
- Same in `media/qa-ui/cycle-20/134-relaunch.png`.
- Passes whenever the turn ends on its own before the relaunch (cycle-20b,
  and three isolated runs: root, fork, LX-only) — which is why it read as
  flaky. It is not flaky; it is "Stop was pressed".

Ledger row F-105 in `internal/symmetry-findings.md` carries the same.
