---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# `archive_children` on by default — PR #144, branch `cursor/archive-children-default-b027` (on `main`)

Audit item 23 / fix 12. `[root] archive_children` is now `Option<bool>`: absent = on, `false` = off; new places write `true`. Once root has read a worker's done (batched), a worker that is not live moves to `.arbos/archive/agents/<id>/`; tree frame follows; `kernel.log` `child_archived`.

Kept working across the move:
- `grep scope=history` walks `archive/agents/` too.
- A `.arbos/agents/<id>/…` path that no longer exists resolves into the archive, so the path in the done message still reads in root's done turn (`read`, `ls`, `grep path=`).

Scenarios to run:
- Audit scenario 9 (eight spawns, then a ninth) with the desktop attached: worker tabs close (or stay closed) when their folders move; no reconnect loop, no ghost `transcript.jsonl` under `.arbos/agents/<id>/` (that was the #122 blocker). `consistency.py` may need to learn `archive/agents/` as a legal home for `agent.md`.
- `mt-*` scenarios that read `.arbos/agents/<w>/` after the done: write `archive_children = false` in the scenario's `project.toml`, or read from `.arbos/archive/agents/<w>/`.
- `say to=<archived worker>` is refused ("no agent") — expected; a follow-up needs a fresh spawn. Say if you want the kernel to unarchive on `say`.

E2e: `crates/arbos-kernel/tests/archive_children_e2e.rs`.
