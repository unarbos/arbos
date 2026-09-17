---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Phase 1 of the file-system design: nested git for `.arbos/`, `runtime/` split, a commit per turn

Branch `cursor/nested-git-phase1-b027` (on the integration head). Jacob has not answered the design's open questions; the PR states the assumptions.

- `.arbos/runtime/` holds `kernel.json`, `lock`, `focus`, `checkpoint`, `kernel.log` (the kernel's own log), `kernel.out.log` (its stdout/stderr when a window or `run` starts it). `bootstrap` moves an old `focus`/`checkpoint` there. The kernel still writes the legacy `.arbos/kernel.json` for one release; readers (`Place::kernel_json_read`, the desktop, the ssh probe) fall back to it.
- `bootstrap` runs `git init -q -b main` in `.arbos/` (quiet if git is missing) and writes `.arbos/.gitignore`: `runtime/`, the legacy root files, `agents/*/trace/`, `agents/*/jobs/*/out.log`, `agents/*/results/`, `worktrees/`. If the project does not ignore `.arbos/`, `.git/info/exclude` gets it (local, uncommitted) so the project never tracks the nested repo as a gitlink.
- After every turn: `git add -A && git commit -m "<agent> turn L<line>: <last words>"`; after every mechanical node run: `"<agent> node #N: <goal>"`. One commit at a time, fixed identity `arbos <arbos@localhost>`, on the blocking pool; a failure is a `snapshot_failed` warning in `kernel.log`.
- `arbos-kernel log <place> [-n N]`; `arbos-kernel rewind <place> --commit SHA [--yes]` (kernel stopped): commits anything dirty as `pre-rewind`, `read-tree -u --reset SHA`, commits `rewind to <sha>` — linear history. Per-agent rewinds (#80/#82) now also commit `rewind <agent> to line N`. `rewind --list` shows the last 8 commits.

Verified: fresh place → two turns → `log` shows `root turn L4: one` / `root turn L8: two`; `git -C .arbos status` clean; project `git status` clean (exclude); `rewind --commit <first>` → transcript back to 4 lines, log gains `rewind to …`. Existing parity place → first commit takes the whole record; a desktop turn works with `runtime/kernel.json`. Kernel e2e tests pass except `recreate_e2e`, which fails on the integration head too (needs a model key).

## Attack surface

- A place whose project repo already tracks `.arbos/` (e.g. the old parity project) — the nested `.git` appears as an embedded repo to the outer `git add -A`; the exclude does not help once tracked. `check` should warn (follow-up).
- Big first commit on an old place (hundreds of MB of transcripts): time it; `git gc` is not scheduled yet (the design's weekly node is Phase 2+).
- Two kernels on the same place during the transition: old one holds `.arbos/lock`, new one `runtime/lock` — they will not see each other. Documented; the desktop kills/reuses by `kernel.json`, which both write.
- `git` not installed: bootstrap skips the repo silently, `log`/`rewind --commit` explain; everything else unchanged.
- The commit runs while another agent's turn writes its transcript: the commit captures a half-turn (by design; the next commit completes it).
- `rewind --commit` with `runtime/` untracked: `read-tree -u --reset` leaves it alone (verified: runtime files present after the rewind).
- Phone/iOS reads `.arbos/kernel.json` → still there for one release; tell the iOS worker to read `runtime/kernel.json` first.
