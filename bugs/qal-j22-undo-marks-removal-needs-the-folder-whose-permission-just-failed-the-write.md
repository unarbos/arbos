# qal-j22: #444 removes an undo mark it could not write — but the removal needs the folder the write just failed on, so on a read-only `runtime/` the stale mark survives and `undo` still resets past committed work

> **CLOSED on `main` @ `80e6994280f8` (verified 13:27 UTC), by [#392](https://github.com/unarbos/arbos/pull/392) rather than by #444.** The stamped mark decides it: arm (c) now answers *"no checkpoint for this turn: the mark on disk is from the turn at line 24, this turn started at line 30 (its own mark was not written); nothing reset"*, with `a.txt` in place and turn one's commit reachable. The control still fails — #444 alone (`f80f0b663bac`) destroys the commit in arm (c) — so the pass names the mechanism rather than inheriting it. Detail in "The verdict" below; the general rule the bug was filed for stands.

- Measured at: [#444](https://github.com/unarbos/arbos/pull/444) @ `f80f0b663bac` (`arbos-kernel 0.2.0 f80f0b663bac protocol 1`), control `main` @ `cbbe9922d6a2` (`arbos-kernel 0.2.0 cbbe9922d6a2 protocol 1`, #444's own merge-base), separate `CARGO_TARGET_DIR`s (`repo/target`, `target-pr444`), told apart by `strings | grep "the undo mark could not be written"` — 0 in the control, 1 in the PR. Scenario `uw-01-unwritable-undo-mark-refuses-rather-than-using-an-older-turns`, rollouts `internal/qa/rollouts/20260917T1252{40,53}Z-uw-01-…`.
- Class: destructive with a false "restored" — qal-j10's shape, one layer out. The fix's own repair depends on the permission that caused the fault.
- Feature: `arbos-engine tools/git.rs snapshot_turn_tree` / `undo`, the turn-start mark `.arbos/runtime/checkpoint`.

## What #444 fixes, and what it does not

#444 replaces `let _ = std::fs::write(&mark, …)` with: on failure, `remove_file(&mark)` and return the error, so `undo` finds no mark and refuses. Three arms, each a fresh kernel on the same repository, the mark made unusable **before any record existed** for its arm:

| arm | how the mark's write fails | control `cbbe9922d6a2` | #444 `f80f0b663bac` |
|---|---|---|---|
| (a) no mark has ever been written; the mark file is an empty, unwritable file from before the first turn | `EACCES` on the file | mark stays (empty), `undo` says `no checkpoint`, nothing destroyed | mark **removed**, `undo` says `no checkpoint`, nothing destroyed |
| (b) a stale mark from turn one; **the mark file** unwritable, its folder writable (a permissions accident, and what a full disk amounts to — `ENOSPC` on write, `unlink` still works) | `EACCES` on the file | mark stays at `461de789`; `undo` runs `reset --hard 461de789`; turn one's commit `a43b19af` unreachable, `a.txt` gone; tool reports **`restored 461de789…`** | mark **removed**; `undo` says `no checkpoint`; `a.txt` there, turn one's commit reachable — **fixed** |
| (c) the same stale mark; **the folder** read-only | `EACCES` on the file *and* on the removal | mark stays at `461de789`; `undo` resets to it; commit `8b7c7ee9` unreachable, `a.txt` gone; **`restored 461de789…`** | mark **stays** (`mark_exists_after: true`); `undo` resets to it; commit `b5f3b505` unreachable, `a.txt` gone; **`restored ab47fa45…`** — **unchanged** |

So arm (c) is a destructive path with a success message on both builds. The mechanism is plain: `remove_file` needs write permission on the containing directory, which is the same permission whose absence failed the write.

## Why arm (c) is the case to care about

- **It is the common cause, not the exotic one.** `errors=remount-ro` is ext4's default: one I/O error and the filesystem carrying the place is remounted read-only under the running kernel. Every write in `runtime/` then fails, including the unlink. A container layer turning read-only, or a `chmod` by a person or a sync tool, gives the same state.
- **A full disk is the case the fix does cover**, because `ENOSPC` fails the write and leaves `unlink` working. That is a real win and arm (b) proves it.
- The place cannot be *started* with a read-only `runtime/` — the kernel exits 1, measured at 12:51 on `cbbe9922d6a2` — so this is a mid-session change, which is exactly what a remount is.

## What we expect

The durable fix is not a repair after the fact but a mark that cannot be believed unless it belongs to this turn:

1. **Stamp the mark with its turn's line and check it on read** — which is what [#392](https://github.com/unarbos/arbos/pull/392) already does. `undo` then refuses a mark whose line is not the current turn's, whatever the filesystem did. With #392 landed, arm (c) is covered without depending on any write succeeding. **Landing #392 closes this; #444's removal is a second layer, not the floor.**
2. Failing that, `undo` must treat "a mark I cannot prove is mine" as no mark: read the mark's mtime against the turn's start, or write the mark through a temp file and rename so a failed write leaves the *old* mark visibly older than the turn.

Either way, the general rule from this one is worth keeping: **a repair that runs on the failure path must not need the resource that failed.** Removal needs the directory; if the directory is why you are here, you have no repair.

## The verdict, measured on three builds (13:27 UTC)

`#392` merged at 12:59 UTC and `main` @ `80e6994280f8` carries both PRs. The same three arms, run on three
builds, each build told apart by its own `--version` and by phrases only it holds:

| build | arms that staged the fault | (b) mark file unwritable | (c) mark folder read-only |
|---|---|---|---|
| `cbbe9922d6a2` — neither PR | a, b, c | **destroyed committed work**, said `restored …` | **destroyed committed work** |
| `f80f0b663bac` — #444 only | a, b, c | refused with a reason — fixed | **destroyed committed work** |
| `80e6994280f8` — `main`, #392 + #444 | **c only** | the arm no longer stages the fault | **refused with a reason** — closed |

Two things that table says, and only the middle column says the second one:

1. **#392 closes it, and does so without depending on any write succeeding.** The mark now carries
   `line:<n>`, and `undo` compares it with the turn it is in. A stale mark that cannot be removed is
   simply not believed. That is the repair this bug asked for.
2. **On `main` the file-permission arm can no longer be staged at all**, because #392 writes the mark with
   `record::write_atomic` — a temp file and a rename, which replaces an unwritable file without writing
   to it. So the reachable worlds for an unwritten mark narrowed to two: the folder cannot be written
   (arm c, now refused by the stamp) or the disk is full. The probe reports this per arm rather than
   passing quietly: `arms_that_staged_the_fault: ["c"]`.

The second point is the one that would have been missed by reading. A build where two of three arms pass
because the injection stopped working is not the same as a build where they pass because the fault is
handled, and only the arm-level record tells them apart.

## Regression check

`uw-01`, three arms, tagged `undo`, `destructive-order`. Green on `main` @ `80e6994280f8`; red on
`cbbe9922d6a2` and on #444 alone, which is what makes the green mean something.

Two guards inside it, both earned:

- **Each arm says whether it staged the fault**, and the scenario refuses to be green if none did
  (`probe-no-arm-staged-an-unwritten-mark`). Without that, `main` would have read as three passes.
- **"Staged" is measured as "the mark does not name this turn's starting HEAD"**, not as "the mark's
  contents changed". The first version used the latter and called #444's *removal* a successful write —
  reporting a staged arm as unstaged on the one build where the fix worked. Caught by running all three
  builds and reading a result that disagreed with the run twenty minutes earlier.

The arm that had to be thrown away, and why it is worth recording: the first version of arm (a) made `runtime/` read-only *before* the kernel started. The kernel exits 1 and there is no turn to measure — a world this bug cannot live in. The probe reported `probe-kernel-did-not-start` rather than a pass, which is how it was caught.

## One thing #444 does not appear to cost

`snapshot_turn_tree` now returns an error where it used to swallow one, so a turn whose mark cannot be written now has a failing step inside it. Measured in arm (a): the turn still ended (`turn_ended: true`) and root's transcript carried no notice about it (`turn_notices: []`). The person's turn is not taken from them — but nobody is told the turn has no undo point either, which is worth a line on the transcript the next time this code is touched.
