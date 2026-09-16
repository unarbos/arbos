---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# For QA: one remote place per child, and a macOS-ready start (K-02b)

From the features agent. Branch `cursor/remote-per-child-b027` → `rust`, stacked on #33 (`cursor/remote-spawn-b027`).

## What I am building

1. **Per-child remote places.** #33 gave every remote child of a project the same place (`<dir>/<project>/`), so two children on one machine shared one remote root and the second brief became a follow-up turn to the first. Now each child gets `<dir>/<project>--<child-id>/` with its own kernel, root, and tunnel. `remotes.json` records the path per child, as before.
2. **Cleanup on demand**: `spawn` receipts and the child's first notice name the remote path; a new `remote` action on the `say`… no — a `remote_cleanup` is not a tool. Instead the parent's receipt names the exact `ssh … rm -rf` line, and the design's archive step is where automatic removal belongs (noted in the PR).
3. **macOS-ready remote start**: the start script no longer assumes `setsid` (absent on macOS): it uses `setsid` when present, plain `nohup … &` otherwise. When the machine's `uname -sm` differs from this kernel's, the error names the fix: build there and set `kernel = "<path>"` in `machines.toml`. The build recipe and cross-check plan for Jacob's Mac are in `internal/macos-worker-recipe.md`; nothing in this PR touches his machine.

## How to exercise it

Two `spawn host=arboslife` in one turn with different briefs → two folders under `/home/const/arbos-remote/` named `<project>--<id>`, two kernels (`pgrep -af arbos-remote/bin`), two tunnels locally, two replies. Then spawn a third with the same brief as the first → id suffix `-2`, its own folder.

## What could break — attack here

1. Disk: each child is a full copy of the project tree on the machine. Ten children of a 500 MB tree = 5 GB. The receipt says where; nothing removes them yet.
2. rsync of the same tree twice in parallel (two spawns in one parent step) — both must succeed; watch for the "one spawn at a time" lock only covering the id, not the sync.
3. Kernel start race: two remote kernels starting at once must write distinct `kernel.json` (distinct dirs, so yes) — confirm distinct ports.
4. `remotes.json` written by two relays at once (mirror positions): last writer wins; check neither record is lost.
5. A machine whose `uname -sm` is `Darwin arm64`: the error message, and that `kernel = "…"` in `machines.toml` makes the spawn proceed to the sync step.
