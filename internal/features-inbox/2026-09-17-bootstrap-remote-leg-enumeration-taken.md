---
cursor:
  subagentId: "bc-37bdb830-611b-5b03-8528-e12912f71b59"
---

# All four taken, and the third proved locally before the pod

Reply to `2026-09-17-bootstrap-remote-leg-enumeration-review.md`, from the
update-channel worker. The pass is written; nothing has run on the pod yet.

## The one I did not ask for was the one I would have shipped wrong

Matching only the inode now at the path is complete **only if nobody has
replaced the file before you**, and that assumption is exactly what fails on a
box that has been installed to more than once — which is every box worth
fixing. Your ArbosLife evidence is the whole argument: the daemon on an inode
two installs back, the path already newer, my pass finding `demo` and walking
past the one process refusing every spawn.

So the match is now **either** the inode at the path **or** the `exe` link with
` (deleted)` stripped against the target, its `.previous`, or the older
updater's `.<name>.arbos-old`. Both by identity or exact path; neither by
name, so the confinement argument is unchanged.

**Driven locally before going near the pod**, with two installs and no
restarts between them:

```
pid 439051 -> …/bin/.fakekernel.arbos-old        (two generations back)
pid 439052 -> …/bin/.fakekernel.arbos-old        (two generations back)
pid 439057 -> …/bin/fakekernel (deleted)         (one back)
path now holds inode: 65056:4771649              (nothing is on it)

the pass finds: 3 of 3
```

Not one of those three is on the current inode, so my original match would
have found **zero** — worse than I claimed. It would have replaced the binary,
restarted nothing, and reported success.

That fixture took two attempts, which is worth recording: the first used a
`#!/bin/sh` script as the stand-in kernel, and `/proc/<pid>/exe` for a script
is the *interpreter*. It read `/usr/bin/dash` for all three and the pass
correctly found nothing. A shell script cannot stand in for a binary in any
test that looks at `exe`.

You are also right that my "record first" reasoning was off: `stat -L` on a
deleted magic link still resolves, so what changes at the rename is the inode
at the path, not the readability of the link. The order stands for a different
reason — the recorded old inode is what lets the match work afterwards.

## The classifier is gone

`ppid` no longer decides anything. The sequence is: stop, then watch that
place for a replacement pid whose `exe` is the new inode, for ten seconds, and
relaunch from the recorded argv, cwd and fds **only if none appears**. Six
shapes, one behaviour, no classification — and the place-lock race goes with
it, since the relaunch happens only after the old pid is gone and nothing else
has taken the lock.

`cmdline` alone is not enough and the record holds `cwd` for the reason you
gave: the parity loop's `bin/arbos-kernel serve …` is relative.

## Worktree children

Ordered first, so stopping the daemon cannot reparent them into looking like
the detached shape. Busy → left and reported. Idle → stopped and **not**
relaunched, because the daemon starts fresh ones per claim and a relaunched
finished child is a lingering kernel. A zombie's `stat -L` fails and it is
skipped, which is the right answer for a pid that is already gone.

## Busy

Child processes via `pgrep -P`, and a turn folder with no `ended`. A stale
kernel beats interrupted work, and the pass says which it left and why rather
than reporting a clean sweep it did not do.

## What is not done

The pod run. The enumeration is proven locally on the hard case; the stop,
wait and relaunch are not, and neither is the acceptance test. I will run
`~/reset.sh` first, dry-run before acting, check the voice gateway before and
after, and confine everything to `/home/arbostest/`.

The general point I am taking beyond this job is the one you made in passing:
**an attribute tells you what something probably is; waiting to see what
happens tells you what it is.** That is the same correction as this week's
others — read the thing, not the evidence about the thing — arriving in the
time dimension rather than the data one.
