---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# For QA: file tools confined to a worktree child's cwd (K-01b)

From the features agent. Branch `cursor/worktree-cwd-confinement-b027` → `rust`. Closes the gap named in PR #9's "attack here" list.

## What I am building

Today every file tool (`ls read find grep write edit apply_patch`) is confined to the **place**. A child spawned with `isolate=worktree` has its cwd under `<place>/.arbos/worktrees/<id>/`, so `write ../../src/x.rs` reached its parent's checkout. Now the confinement root is the child's worktree when its cwd is under `.arbos/worktrees/`, else the place (`RunCx::root`, `confinement_root`). `grep` results for such a child are filtered to its worktree and shown relative to it, so a file no longer appears twice. `bash` stays unconfined (that is the sandbox item P-07).

## How to exercise it

Spawn a worktree child (needs PR #9's `isolate=worktree`; on this branch alone, set `cwd: <place>/.arbos/worktrees/x` by hand in the child's `agent.md` after making that folder a checkout). From the child: `read ../../README.md` must fail with "outside the workspace <worktree path>"; `write hello.txt` succeeds inside the worktree; `grep` for a word present in both trees returns one hit with a worktree-relative path.

## What could break — attack here

1. Symlink from inside the worktree to the parent tree (`ln -s ../../src src-link`): `confine` canonicalises the deepest existing ancestor, so it should refuse. Try both a symlinked file and a symlinked directory.
2. A cwd that *looks* like a worktree path but is not a git worktree (a plain folder under `.arbos/worktrees/`): still confined there. Fine, but confirm no panic when the folder is deleted mid-turn.
3. Root agent (cwd = place) unchanged: full place access, `.arbos/` reachable as before.
4. Child with explicit `cwd` elsewhere in the place (not a worktree): confined to the place as before, not to that cwd.
5. `grep` with `path` scope inside a worktree child: the scope resolves against the worktree; hits are relative to it.
6. `apply_patch` with `*** Move File` to a path outside the worktree: refused.
7. `last_touched_path` inference (edit without path) in a worktree child: it scans the agent's transcript; make sure the inferred path resolves inside the worktree.
