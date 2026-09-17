---
cursor:
  subagentId: "bc-37bdb830-611b-5b03-8528-e12912f71b59"
---

# #403's self re-exec against the app's update path — review

Asked for by the coordinator: a kernel that re-execs itself when its binary is
replaced, versus the desktop's update flow, which stops kernels deliberately
and warns when it is talking to a stranger. Do they cooperate or race?

**Short answer: they cooperate, with one real defect that is specific to how
the app's updater swaps, and one dependency worth naming.** The defect is a
small fix and does not touch the author's constraint.

## The three mechanisms, and why there is no conflict of intent

| | when | what it does |
| --- | --- | --- |
| the app's update | before the swap | `SIGTERM`s the kernels of every place it knows |
| the app's attach check (#372/#388) | at attach | a stranger that is idle is stopped and respawned; busy, it attaches and the bar warns |
| **#403** | the serve loop's 5 s tick | its own binary replaced *and* the gate idle → exec onto the file at its path |

They are three answers to one question — "this process is running code nobody
wants any more" — at three different vantage points. The app's stop is
best-effort and cannot enumerate every kernel on the machine; my own design
says so and asks for a second layer. #403 is effectively a third, running
inside the only process that always knows.

## The gate question, answered: it cannot defeat it

The coordinator's sharpest question was whether a kernel restarting itself
could get around the desktop's "this kernel is busy, ending it would end that
work" warning.

**No, and by construction.** #403 gates on
`idle::update_verdict_quiet(&hooks, REEXEC_HORIZON_MS)` — the *same* verdict
the desktop asks for over `/healthz`. Both refuse for a turn in flight, a
pending approval, a subscription run, or a remote child mid-turn. There is no
state in which the bar says "busy, this would end that work" and #403 execs
anyway, because they are reading one function.

#403 is in fact stricter: a 60-second horizon, so a subscription due within
the minute also holds it, where the desktop passes a horizon of its own.

## Ordering: the app's stop comes first, so they do not overlap

The update flow is: verify → unpack → **stop known kernels** → swap → commit →
relaunch. Kernels the app knows about are already dead before the binary moves,
so `binary_gone` can never become true for them. What #403 acts on is exactly
the survivors — the ones my stop provably misses.

A rename is atomic, so an exec landing mid-swap gets either the old directory
or the new one, never a half-written file.

## The one race, and it is benign

If the attach check `SIGTERM`s a skewed-but-idle kernel at the same moment
#403 execs it: signal dispositions reset at exec, so a `SIGTERM` arriving just
after could kill the fresh image. But `attach_or_spawn` then spawns one anyway,
so it converges on a single kernel from the new build. Untidy for a second, no
work lost — the gate guaranteed there was none.

## The defect: `choose()` prefers where the inode *is*, not where it was started

`binary::choose()` takes `current_exe()` whenever it "is a file and does not
end in ` (deleted)`", and only then falls back to the path the process was
started with.

That is right for the case #403 was written for — a binary replaced in place by
unlink-and-write, where `current_exe` reads `(deleted)`. **It is wrong for how
the app updates**, which renames a directory rather than unlinking a file:

1. `Swap::begin` renames `/Applications/Arbos.app` →
   `/Applications/.Arbos.app.arbos-old`, then moves the new app in.
2. A surviving kernel's inode travels with the directory. `current_exe()`
   follows the inode and returns
   `/Applications/.Arbos.app.arbos-old/Contents/MacOS/arbos-kernel`.
3. That **is a file** and does **not** end in ` (deleted)`. So `choose()`
   returns it, and the kernel execs onto **its own old build**.

It escapes only because `Swap::commit` unlinks the backup milliseconds later,
after which `current_exe` is a dead path and the fallback gives
`/Applications/Arbos.app/Contents/MacOS/arbos-kernel` — the new build. So the
right outcome depends on winning a race by a wide margin rather than on
anything designed.

Two reasons not to leave it there:

- **It is accidental.** If the app ever kept its backup — which I considered
  for rollback, and which `arbos-kernel update` already does as
  `<bin>.previous` — the kernel would exec onto the old build indefinitely.
- **It may be self-silencing.** If `gone()` compares the current path's
  identity against what was remembered at start, a kernel that has exec'd onto
  the old inode at its new path may come back with `gone()` false. It then
  stops retrying and is stale for ever, with the notice already said once — the
  exact failure the feature exists to prevent, arrived at through the feature.

### What I would change

**When `binary_gone()` is true, prefer `STARTED_AS` over `current_exe()`.**
The question a re-exec asks is "what is at the path I was started from now",
not "where has my inode been moved to". For unlink-and-write both answers
agree; for a directory rename only the first is right.

That is a few lines in `choose`, keeps every fallback, and does not weaken the
detached case the author cares about.

## The dependency worth naming

`binary_identity::gone()` is not in `main` or on #403's branch — it comes from
#385, still open. So **#403 cannot land before #385**, and I could not read
`gone()` to confirm the self-silencing concern above. It is a question for the
author rather than a finding: *after a kernel execs onto an inode that has been
moved to a new path, does `gone()` go back to false?*

## What the bar shows while a kernel re-execs

Correct, in all three moments, without changes:

- **during the exec** — the port is closed, `alive()` is false, so no skew is
  reported and the bar stays quiet. Right: nothing is wrong.
- **after it comes back** — commits match the bundle, no warning. Right.
- **if the exec fails** and the old image serves on — `binary_gone` stays true,
  and #388 makes that a stranger on its own whatever the commits say, so the
  bar says *"Kernel running a deleted build"*. The bar is the backstop for a
  failed re-exec, which is the behaviour I would want.

## Verdict

Cooperate. Land #403 after #385, with `choose()` preferring the started-from
path when `binary_gone` is true. Without that change it is still an
improvement on today — the race is usually won — but it is won by accident,
and the losing branch is silent.
