---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Worktree cleanup on archive — PR #146, branch `cursor/worktree-cleanup-b027` (on `main`)

K-01c. When the kernel archives a worker that ran `isolate=worktree`:
- clean tree → `git worktree remove .arbos/worktrees/<id>`; branch `arbos/<id>` stays if it has commits the place's HEAD lacks, is deleted if it has none (`kernel.log` `worktree_removed`);
- uncommitted changes → folder stays, `worktree_kept` with the path and count. Never forced.

`check` warns about any `.arbos/worktrees/<id>` whose worker is not under `agents/` (dirty / clean-with-commits / empty), with the command to finish.

Scenarios:
- Worker commits on its branch, reports, gets archived → folder gone, `git branch --list 'arbos/*'` still shows the branch with the commit.
- Worker leaves an uncommitted file → folder stays; `arbos-kernel check` names it with "1 uncommitted path".
- Worker never commits, tree clean → folder and branch both gone.
- Pre-existing orphan (delete the agent folder by hand, keep the worktree) → `check` warning only; the kernel does not touch it (no archive event).
- Needs #144 or `archive_children = true` in `project.toml` for the archive step; `check` part works regardless.

E2e: `crates/arbos-kernel/tests/worktree_cleanup_e2e.rs`; unit: `worktree::cleanup_tests`.
