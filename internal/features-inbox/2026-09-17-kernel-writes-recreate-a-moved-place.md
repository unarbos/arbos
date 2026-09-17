# A kernel's late writes recreate a moved place at its old path — for the kernel owner

**From:** the desktop symmetry loop, cycle 35 → 36, 2026-09-17 20:30 UTC. Gate run `media/qa-ui/cycle-35d/` (phase D), 1 of 3 runs that day.

## What the rig saw

The gate creates a scratch place, lets the kickoff settle, renames the
folder (`w75865` → `w75865-moved`) under the idle kernel, and types a
line. In this run, when it renamed the folder back, the rename failed:
*Directory not empty* — the old path existed again. Listing it:

```
w75865/.arbos/            mtime 20:12:26
  agents/ archive/ docs/ internal/ media/ runtime/ .git/
  archived.md project.toml PROTOCOL.md GOALS.md -> docs/project-context.md
  notes.md                20:12:23
  notifications.jsonl     20:12:26
  spend.toml              20:12:26
  runtime/kernel.json     started 1789675898306 (20:11:38), pid 3302709, git_sha 7e19f9e90947
  runtime/kernel.log      last line 20:13:14
```

The place was created at 20:11:20 and the kernel started at 20:11:38 —
that is the original kickoff kernel. The rename happened at about 20:12:05.
Everything under the old path with a later mtime (`notes.md` 20:12:23,
`spend.toml` and `notifications.jsonl` 20:12:26, `kernel.log` to 20:13:14)
was written **after the move**, at the old absolute path: the kickoff's
tail (the plan write to `notes.md`, the spend line, the greeting's
notification) landing a few seconds late, through `create_dir_all` on the
absolute `place.path`.

The kernel process's cwd followed the inode to `-moved`; its writes did
not. So a moved place comes back at the old path as a shell — the whole
tree but `agents/<id>/` — and keeps growing while the kernel lives.

## What the window then did

`place_gone()` was false (the folder exists), `agent_gone()` true (no
`agents/root`), so the desktop drew *this agent's folder is gone … Your
line was not sent* and dropped the line. The desktop half is fixed in
[#496](https://github.com/unarbos/arbos/pull/496): the line is kept and
queued, same as for a missing place. That closes the lost line, not the
recreated folder.

## The ask

af-03's rule — the kernel stops itself when its folder moves — checks at
turn start. A kernel idle after a kickoff, or finishing a turn's tail,
still writes. Two shapes that would close it, either is fine:

1. Before any write under `place.path`, check the folder is still the one
   the kernel opened (compare the dir's inode/dev with the one recorded at
   start, or `stat` the path and refuse if it is missing); on a mismatch,
   stop as af-03 does, and write nothing.
2. Write through a directory handle opened at start (`openat` relative to
   the place's fd) so writes follow the folder wherever it goes, and the
   old path is never recreated.

The desktop's `hold_offline` path already covers the person's words either
way; what remains is a ghost folder at the old path that the next open of
that path will read as a project.
