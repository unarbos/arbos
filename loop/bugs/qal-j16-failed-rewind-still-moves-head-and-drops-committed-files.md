# qal-j16: a rewind whose `read-tree` fails still moves HEAD and removes the person's later commits from the tree — `reset --hard` runs before the step that can fail

- Measured at: #419 @ `0f71cca4` (`arbos-kernel 0.2.0 0f71cca408f4`), control `main` @ `7e19f9e9`, each built in its own target directory (see the note at the end). Scenario `rw-08-failed-restore-leaves-the-tree-where-it-was`, rollouts `internal/qa/rollouts/20260917T091840Z-rw-08-…` (#419) and `20260917T092644Z-rw-08-…` (main); every git the kernel ran is in each rollout's `kernel-git.log`.
- Class: destructive, the eighth in `restore()`'s neighbourhood — the same shape as the seven before it: a step that destroys before the step that can fail. #419 moved `clean` after `read-tree`; `reset --hard` is still before it.
- Feature: rewind with `files: true` (`crates/arbos-engine/src/tools/git.rs` `restore`, #419).

## What #419 fixes, confirmed with a control

Set-up: three turns write `f1`, `f2`, `f3` (untracked). The person then commits `f2` (HEAD moves), edits `f1` by hand, writes `my-notes.txt`. The checkpoint for turn 3 has its work-tree commit's loose object removed (a corrupt repository; `git cat-file -t` fails). Rewind to turn 3 with files. `git read-tree` fails.

| | main `7e19f9e9` | #419 `0f71cca4` |
|---|---|---|
| git run by the kernel | `reset --hard` → `clean -fd` → `read-tree` (fails) | `reset --hard` → `read-tree` (fails); `clean` never reached |
| reported to the client | `error: … files not restored: git read-tree … failed` | same |
| `f1.txt` (edited), `f3.txt`, `my-notes.txt` | **GONE** | kept |
| `f2.txt` (the person's own commit) | GONE | **GONE** |
| HEAD | moved back to the checkpoint's | **moved back to the checkpoint's** |

So #419's claim holds: the later untracked files are no longer destroyed by a restore that then fails. And its second claim holds too (`rw-09-clean-that-fails-is-in-what-restored-says`): an untracked folder git cannot remove makes the restored text say `; untracked files from later turns may remain (git clean: warning: failed to remove later-dir/keep.txt: Permission denied)` on #419, where main says `restored <head> + working tree <work>` and nothing else.

## What is still wrong

The property the fix is for — *a failed restore leaves you where you were* — does not hold on #419. `reset --hard <cp.head>` runs first. When `read-tree` then fails, the person is told the files were not restored, and looks at a tree where HEAD has moved back and every file that only existed in their later commits is gone from the working tree (in the reflog, if they know to look). In the person's words: "it said it couldn't restore, and my commit's file disappeared anyway."

## What we expect

Nothing destructive until everything the restore needs has been checked:

1. Before `reset --hard`: `git cat-file -e <cp.head>^{commit}` and, when there is a work tree, `git cat-file -e <cp.work>^{tree}` (this is what `read-tree` needs). A missing or corrupt object is refused up front with the tree untouched — the same rule `knows_tree()` already applies to a checkpoint that never had a tree.
2. Better: take the current tree's own work commit first (the machinery exists: `work_commit`), keep its sha in the error, and on any later failure put it back with the same `read-tree -u --reset`, so even a failure after the checks leaves the tree where it was. The message then says what was undone.

## Regression check

`rw-08-failed-restore-leaves-the-tree-where-it-was` (no model, replay provider): breaks on #419 with `f2.txt: GONE; HEAD: … -> …`. Passes when the working tree, HEAD and index after the failed rewind equal what they were before it. Every `rw-*` scenario now also asserts the general property on any rewind that reports an error (`<name>-failed-restore-changed-the-tree`).

## A measurement note, so nobody repeats it

The first "control" run against `main` produced #419's behaviour with `main`'s version string: two worktrees built into one `CARGO_TARGET_DIR`, and cargo reused the first build's artifacts for the second. `strings <binary> | grep "may remain"` told them apart. A fix and its control get their own target directories, always (the loop's `cycle.sh` already does this per branch; a hand-built control must too).
