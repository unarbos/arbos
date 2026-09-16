---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# For QA: worktree isolation for `spawn` (K-01)

From the features agent. Branch `cursor/spawn-worktree-isolation-b027` → `rust`.

## What I am building

`spawn` gets `isolate: "worktree"`. The kernel runs `git worktree add -b arbos/<child-id> <place>/.arbos/worktrees/<child-id> HEAD` and sets the child's `cwd` to that directory. The child edits, builds, and commits there; the parent's checkout is untouched. The child's first prompt tells it where it is and which branch it is on. The parent's receipt names the path, the branch, and the removal command. `.arbos/` is added to `.git/info/exclude` when the project does not already ignore it, so the worktree never shows up in the parent's `git status`.

## How to exercise it

Place must be a git repo with at least one commit. From root: "spawn a child with isolate=worktree and brief: *create hello.txt containing hi, commit it on your branch, report the branch name*". Expect `<place>/.arbos/worktrees/<id>/hello.txt`, a commit on `arbos/<id>`, no change in the parent checkout, `git worktree list` showing both. Then in the parent: `git branch --contains` shows the child's commit only on its branch.

## What could break — attack here

1. Place is not a git repo, or has no commits (unborn HEAD): `spawn` must fail with a clear error and create no folder and no half worktree.
2. Two children spawned in parallel with the same brief: distinct ids (`-2`), distinct worktrees, distinct branches.
3. Branch `arbos/<id>` already exists from an earlier run (kernel restart, folder deleted by hand): must not clobber; expect an error or a suffixed branch.
4. Child does `cd ..` in bash and edits the parent checkout: bash is not confined; file tools are confined to the place, and the worktree is under it, so `write ../../src/x.rs` from the worktree reaches the parent's tree. Known gap: file tools confine to the place, not to the child's cwd. Try it and file it.
5. Dirty parent checkout at spawn time: the worktree starts from HEAD, not from uncommitted edits. Confirm the child does not see them and the prompt says so.
6. `undo` / `changes` tools inside the worktree: `checkpoint` is written to `<cwd>/.arbos/checkpoint`, which does not exist in a worktree, so `undo` may do nothing there. Try it.
7. Delete the worktree folder by hand while the child runs; next bash call fails; next spawn with the same id must recover (`git worktree prune`).
8. grep: the kernel's tgrep index covers `.arbos/`, so a parent grep may return the same file twice (checkout + worktree). Check for duplicate hits.
9. Large repo: `git worktree add` copies nothing but checks out the tree; time the spawn on a repo with 50k files and see that the tool call does not time out.
