---
cursor:
  subagentId: "bc-37bdb830-611b-5b03-8528-e12912f71b59"
---

# I stopped your `p1` and `p2` at 12:29:07, and three findings from the run

From bc-37bdb830 (in-app update bar and channel), 2026-09-17 12:35 UTC.
Follows `2026-09-17-bootstrap-remote-leg-wedge-findings.md`.

## First, the damage, because it is yours and it is still down

**`~/mesh/places/p1` and `p2` have no kernel. I stopped them, and I have
not restarted them.** `p3` came back by itself two seconds later because
`loop-p3` was watching it.

What happened: I ran `~/reset.sh` at 12:29:07 to restore my own fixture.
Its `stop_all` is

```bash
pkill -u "$USER" -x arbos-kernel
```

which matches **by process name**, across the whole user — so it reached
your `~/mesh/bin/arbos-kernel` kernels as well as mine. The evidence is
in your own logs: `p1.log:14` and `p2.log:7` both end `arbos-kernel
stopping`, written at 12:29:07.766, and `p3` restarted at 12:29:09.

I did not start them again on purpose. Your `mesh/xdg` and `mesh/cache`
say those kernels run with environment I would be guessing at, and
guessing it wrong seemed worse than telling you quickly. `p3`'s loop
shows the argv shape (`mesh/bin/arbos-kernel serve <place>`) if that is
all you need.

I have stopped using `reset.sh` on that box until you say otherwise.

The irony is not lost: this is the lesson you gave me — match by identity,
never by name — arriving from the other direction. **My pass never touched
your kernels**, in the same runs, because it matches on the running file:
your three are on `mesh/bin/arbos-kernel` (inode 27292764) and my target
is `.local/bin/arbos-kernel` (inode 27292539), so they were never
candidates. The harness that cleans up after the pass is the part that
reached across.

Worth fixing in `reset.sh` whichever way you prefer — confining to the
installation (`-f "$HOME/.local/bin/arbos-kernel serve"`) or matching by
inode as the pass does.

## Finding 1 — `reset.sh wedge` does not currently wedge

It produces three healthy kernels, not a pinned place. I ran it twice and
checked by running file rather than by `sweep.sh`; both times all three
were `ok` on the inode the path held.

The cause is one character. Line 33 stops the kernels and waits `sleep 1`
before the swap, but the supervisor's own cycle is `sleep 2`. So the swap
lands at t+1 and the loop relaunches at t+2 — *after* it, onto the new
file. The original 10:56 incident had a 2-second gap, which is why its
relaunch landed inside the window.

Proved by changing that one thing and nothing else:

```
sed '33s/sleep 1/sleep 3/' ~/reset.sh
```

| gap | result |
|---|---|
| `sleep 1` (as shipped) | `ok` gamma on inode 27292243 = the path. No stale kernel, twice. |
| `sleep 3` | `STALE` gamma pid 1367384 on inode 27292252, path on 27292254, holding the lock. |

So the fixture wants a gap longer than the loop's own sleep. With that,
it reproduces the hazard exactly and I used it for the acceptance test.

One note on its steady state, which differs from the incident in a way
that does not matter for the test: here the loop that spawned the stale
kernel is still alive and supervising it, so nothing spins. In the
original, the *old* loop was killed and its orphan survived, so the *new*
loop logged `place already served` 1411 times. Both are "a place pinned by
a kernel on a deleted image under a live supervisor", which is the part
the pass has to clear.

## Finding 2 — the place lock does not hold across the `runtime/` split

This one I think is yours to decide on, and it is the most serious thing
I found today.

An old kernel takes `<place>/.arbos/lock`. A current kernel takes
`<place>/.arbos/runtime/lock`. `Place::lock_path()` has no fallback to the
legacy name, unlike `kernel_json_read()` which does. So they do not see
each other.

Checked rather than reasoned, on a fresh place:

```
1. old build (67d066eb48f0) serves ~/places/delta   -> takes .arbos/lock
2. new build (69280dc36f92) serves the same place   -> takes runtime/lock, starts happily
3. TOTAL SERVING delta: 2
```

Two kernels of different builds, one place, one store. I stopped both and
removed the directory immediately.

This matters for any mixed-version window: a supervisor relaunching onto
a new build while an old kernel still holds the place will double-serve
rather than be refused. It also means my acceptance test proved slightly
less than I first wrote — the supervisor's relaunch onto the new build
would have succeeded whether or not my pass had freed the lock. What the
pass does guarantee is that the old one is *stopped first*, and that when
it will not stop the pass refuses rather than starting a second one.

I have not changed `lock_path()`. It changes serving behaviour and it is
the kernel's, not mine.

## Finding 3 — `kernel.json` reported the wrong build, and I have fixed the read

`kernel_json_read()` preferred `runtime/kernel.json` whenever it existed,
falling back to the legacy path only when it was absent. But a pre-split
kernel writes only the legacy file, so a place that has ever run a newer
kernel keeps a `runtime/` file for ever, and an older kernel serving it
afterwards is invisible.

Measured on the target, all three places at once:

```
alpha: REALLY pid 1398974 on 67d066eb48f0
    runtime/kernel.json: pid 1391101 sha 69280dc36f92  alive=NO   <- read first
    .arbos/kernel.json : pid 1398974 sha 67d066eb48f0  alive=yes
```

The visible effect, from `update --place` against that place:

- **without the fix**: no `serving` line at all. It read the dead record,
  found the pid gone, and concluded nothing was serving the place.
- **with the fix**: `serving 0.2.0 67d066eb48f0 (pid 1398974) in
  .../alpha ← binary replaced under it; restart to run 7a231cf0df18`.

So the old behaviour was worse than reporting the wrong build: it
reported an *empty place*, which is the state in which something starts a
second kernel on it — and by finding 2, nothing would stop it.

The read now asks "which of these names a process that is still there"
rather than "which path is newer", and keeps the old answer when neither
does. That is on its own branch rather than in the bootstrap PR, since it
is a different subsystem and should not wait on that review.

It also explains `sweep.sh` reporting `cbbe9922d6a2` for kernels that are
really on `67d066eb48f0` — it reads `runtime/` first for the same reason.
Reading the running process's `/proc/<pid>/exe --version` is the honest
answer if you want the sweep independent of the fix.

## Where the target stands

`alpha`, `beta`, `gamma` on the old build as the clean fixture intends,
nothing stale, no leftovers of mine except `~/newer-arbos-kernel`. The
voice gateway answered 200 before, during and after every run, inside and
outside, and the box never went above 0.21 load.

`p1` and `p2` are down and waiting for you.
