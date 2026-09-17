---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Multitasking audit, kernel items — PR #122, branch `cursor/multitask-kernel-b027` (base: integration head)

Fixes 4, 5, 6, 8 of `docs/multitasking-audit-2026-09-13.md`. Your scenarios 3, 9, 12, 13, 15 from `2026-09-13-multitasking-audit.md` are in `crates/arbos-kernel/tests/multitask_e2e.rs` on the replay provider; the same flows work through `run.py` with a real model.

## What changed

- **Cap (scenario 9).** `live_children` = children with a turn in flight, a waking inbox message, or a `waiting/` file. Finished workers no longer count. Spawn 8 short workers, let them finish, spawn a ninth: it starts. Also after a kernel restart (nothing in memory).
- **Archive (opt-in).** `project.toml`:

```toml
[root]
role = "coordinator"
archive_children = true
```

  Once root has read a worker's done message and the worker is not live, its folder moves to `.arbos/archive/agents/<id>/` and a tree frame goes out. Off by default because the desktop reconnects forever to a missing agent (audit item 23). Try it with the kernel alone (`kclient.py`): the child vanishes from `snapshot.tree`, `kernel.log` has `child_archived`.
- **One report per child (12, 13).** The brief no longer says "report with say". Expect exactly one `say` line per child on root's transcript (the done file). `spawn wait=true`: the words come back as the tool result and no done file follows; root has one turn.
- **Batching (15).** Several done files waiting for an idle root go into one turn: `turns/tNNNN/cause.md`, `cause-2.md`, …; one `say` line each; one root model call; `kernel.log` `done_batched`. Five workers ending within 15 s should give two root turns at most (one if all land while root's spawn turn is still running).
- **Steer (3).** A steer no longer skips the tool calls the model already decided on. `skipped: user steered` is gone; the only skip text left is `skipped: user said stop` (a steer file whose text is a stop word, e.g. a peer's `say mode=steer text=stop`). A stop word typed by the user is still the Stop button (turn interrupted).

## To try to break

- A child that `say`s to root *and* ends: root should get the say and then the done file (two lines) — the template no longer asks for the say, so a model that does it anyway still produces two; that is the model, not the kernel. Report if the *kernel* ever writes two done files for one turn.
- Batching with a mix: two done files and one user message waiting. The user message opens its own turn (only done files batch). Check no file is lost: every claimed file is in a turn folder.
- Cap with parked asks: a child parked on `ask` counts as live. Eight parked children block the ninth spawn — intended; the error names the cap and `max_children`.
- Archive on, then the desktop opens the archived worker's chat: expect the reconnect loop from audit item 23 until the desktop learns the archive. Do not turn it on for a place a window is showing.
- Steer while a *long* batch runs (ten edits): all ten complete, the steer lands after. If the user wanted to abort, "stop" does it.

## Not done here (other lanes)

Desktop: steer by default while busy, kernel-held queue, done-file card, plan strip trimmed (audit fixes 1, 2, 3, 7, 9, 11, 12). Kernel fix 10 (notes nudge at turn end) and the `gc` chore hidden as `internal = true` are next on my list.
