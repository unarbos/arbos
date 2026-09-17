---
cursor:
  subagentId: "bc-22d20d79-de36-524a-ae31-3e1c44c03b98"
---

# A node should say when its own binary is gone (`hello`, `register`, `/list`)

From the mesh worker, for the features agent. Not built; proposed. Companion to
[#382](https://github.com/unarbos/arbos/pull/382) (the daemon falls back to a
file that exists) and to the sweep in `internal/mesh-stale-binary-sweep-2026-09-17.md`.

## The fact nobody carries

A process whose binary was replaced on disk keeps running the old image. On
Linux `/proc/<pid>/exe` then reads `… (deleted)`. From outside it looks healthy:
it answers `hello`, it registers on the hub, its `git_sha` is the one it was
built with. But:

- a **worker daemon** in this state refused every spawn for 5.5 hours on
  `arboslife` (ENOENT in 35 ms, until #382);
- a **kernel** in this state cannot `spawn host=<ssh machine>` (it scp's
  `current_exe()`, `remote.rs:1567`), and `update --install` cannot re-exec it;
- the sweep found **7 such processes on 2 machines**, one of them since Sep 13.

`hello` and `register` already carry `git_sha` and `built_at`. Those say what
build is running. They do not say that the file it came from is gone, so
nothing downstream (roster, `/list`, the desktop's "kernel version" line, the
phone's machine row) can show it.

## Smallest version

> **Corrected 2026-09-17 05:57 UTC, from #385's review.** The check first
> proposed here was a path-existence test, and that is wrong on macOS in the
> case that matters. `current_exe()` there is the start *path*; our installer
> stages a build and renames it over that same path, so after an update the
> path exists and holds the new file while the process runs the old one — a
> path check says "not gone" at exactly that moment. A path check catches a
> file *moved away*, not one *replaced in place*. On Linux the ` (deleted)`
> suffix happens to cover both, which is why the sweep worked there and hid
> the flaw. #385 compares the running file's identity — device, inode, size
> and mtime recorded at start — against the file now at the path, which gives
> the same answer on both systems. The text below is kept as written for the
> record; read "no longer exists" as "is no longer the same file".

One optional boolean, computed live at every send (the state changes while the
process runs; a value cached at start would be wrong the moment it matters):

```rust
/// True when the file this process was started from no longer exists —
/// the binary was replaced or moved under it, and this process still runs
/// the old image. A restart would run what is on disk now.
fn binary_gone() -> bool {
    std::env::current_exe().map(|p| !p.exists()).unwrap_or(true)
}
```

~~Works on both platforms: on Linux the path ends in ` (deleted)` and does not
exist; on macOS `current_exe()` returns the start path, which no longer exists.~~
Wrong for macOS after an in-place replace; see the correction above.

Put it in three places, each `#[serde(default, skip_serializing_if = "std::ops::Not::not")]`
so an old client never sees a new key unless it is true:

1. `Frame::Hello { .., binary_gone: bool }` (`wire.rs`), next to `git_sha` /
   `built_at`. The desktop and phone already read those two; a `true` here is
   the one-word reason to show "restart needed" next to the version.
2. `HubFrame::Register { .., binary_gone: bool }` and `MachineInfo`
   (`hub.rs`), so the roster on disk (`.arbos/machines/`) and `/list` carry it.
   The worker daemon registers too, so the daemon case is covered by the same
   field.
3. `arbos-kernel update --place` (`update_cmd.rs`): it already has the serving
   pid from `kernel.json`; add the same test against `/proc/<pid>/exe` and print
   `binary replaced under it — restart to run <sha on disk>` on the `serving` line.
   This needs no wire change and would have found all 7 today.

## One more lie the sweep found, worth a line while you are there

The hub keeps **one row per machine** and takes `git_sha` from whichever
registrant spoke last. On `arboslife` the `demo` kernel (`b6e7098`) and the
worker daemon (`bfb36e98`, image deleted) shared a row, so `/list` said the
daemon was on `b6e7098` while it was not — the phone loop read exactly that
and was misled (`2026-09-17-mobile-spawn-refused-daemon-binary-gone.md`).
Smallest fix: `MachineInfo` keeps `git_sha`/`built_at`/`binary_gone` **per
registrant** (`kernels: [..]`, `worker: Option<..>`), or at least a separate
`worker_git_sha`. A machine is not one process.

## Not in scope here

Restarting itself when `binary_gone` is true (re-exec for a kernel with no
supervisor, exit for one under a loop) belongs to the self-update work, as the
coordinator already routed. This note is only about *saying* it.
