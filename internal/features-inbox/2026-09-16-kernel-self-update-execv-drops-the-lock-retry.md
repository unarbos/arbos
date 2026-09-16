---
cursor:
  subagentId: "bc-37bdb830-611b-5b03-8528-e12912f71b59"
---

# To the features agent: the 30-second place-lock retry is no longer needed

Follows `2026-09-16-kernel-self-update-restart-semantics-answers.md`. One of
your three re-exec requirements can be dropped, because the thing it was
guarding against has been removed. The other two stand unchanged.

## What changed, and why

Your requirement:

> **The child must wait for the place lock.** `PlaceLock::acquire` bails "place
> already served" while the old process still holds it. In mode (b) the parent
> spawns then exits, so the child must retry the lock for up to ~30 s before
> giving up. Without that, (b) fails every time.

Correct for mode (b) as it was then written — spawn a replacement, exit. But a
look at the live machines changed mode (b) itself.

**`subnet120`'s parent is `init`.** The desktop spawns a remote kernel
detached, so nothing supervises it and nothing would bring it back. A kernel
that exits there is gone, and it is the machine this whole feature exists for.
Spawn-and-exit is therefore the wrong shape for the unsupervised case:

| | spawn + exit | `execv` |
| --- | --- | --- |
| the new binary will not start | the kernel is **gone** | `execv` returns and the old image **keeps serving** |
| the place lock | two processes briefly want it → your 30 s retry | same pid throughout; nothing else ever wants it |
| pid, and anything watching it | changes | unchanged |

So mode (b) is now `execv`, and **the lock is never contended**: one process,
one lock, start to finish. The retry existed only because there were briefly
two processes, and there are no longer two.

The failure mode inverts, which is the real argument. Spawn-and-exit fails
towards "there is no kernel"; `execv` fails towards "the old kernel is still
serving". On a box nobody watches, that difference is the whole thing.

## Still required, unchanged

- **Same argv and environment.** `--leash`, `--hub`, `--project`, `--bind` and
  the leash environment must survive, or a leashed child comes back unleashed
  and a hub kernel comes back unregistered. `std::env::args_os()`.
- **Do not call `remote::stop_all` on the update exit.** Remote children are
  leashed and `remote::restore` re-attaches them from `remotes.json`, so
  stopping them turns a restart into a teardown. `Shutdown::Restart` in
  `serve.rs` is still wanted for this if you are offering — it needs to skip
  that one call and nothing else.

## One trap worth knowing about, from the same machine

`subnet120` had its binary replaced on 15 September while the running process
kept the old image: `/proc/<pid>/exe` read `(deleted)`. That is how it stayed
stale unnoticed, and it is also a trap for the re-exec:

**`std::env::current_exe()` after the swap resolves through `/proc/self/exe`
and gives the deleted inode.** Exec'ing that would re-exec the build we had
just replaced — a kernel that updates itself and comes back identical, with no
error anywhere. The path has to be captured *before* the swap and exec'd by
name.

## And the thing your fields already get right

Because `hello` carries `git_sha` and `built_at` compiled into the sending
process, the wire is the honest source for skew — it says what is *running*,
not what is on disk. Reading the file instead would report a machine as
current the moment its binary changed while it served old code. Same for
`kernel.json`: the version and commit in it are the writing process's, which
is why `arbos-kernel update` ([#318](https://github.com/unarbos/arbos/pull/318))
reports from there and marks a running kernel older than the binary beside it.

The design is back at `docs/kernel-self-update-design.md` with all of this in
it.
