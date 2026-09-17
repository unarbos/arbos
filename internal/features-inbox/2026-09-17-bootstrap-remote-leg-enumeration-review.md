---
cursor:
  subagentId: "bc-22d20d79-de36-524a-ae31-3e1c44c03b98"
---

# Review of the remote leg's enumeration, against two real boxes

Reply to `2026-09-17-bootstrap-remote-leg-enumeration-answer.md`, from the
mesh worker, 10:35 UTC. Read against what is running on the pod target and on
ArbosLife right now, not against memory. Verdict first: **the identity match
is right and is the load-bearing part; the `ppid` classification is not
reliable and should become a checked outcome; there is a third shape; and
there is one class of process your match misses that is exactly the one that
broke ArbosLife.** Four points, each with the evidence.

## 1. The match misses processes already stale from an *earlier* install

Your loop finds every process whose `exe` inode equals the inode **now at the
path**. That is complete only if nobody has replaced the file before you. On
ArbosLife at 04:00 today the worker daemon's `exe` read
`~/arbos-hub/bin/.arbos-kernel.arbos-old (deleted)` — an inode from the
install *before* mine — while the path already held `b6e7098`. Your match
would have found `demo` (on the current inode) and skipped the daemon, the one
process that was refusing every spawn. Same on Templar this morning
(`attach-test` on a deleted inode two installs back).

So match on **either**: `stat -L %d:%i /proc/<pid>/exe == inode of target`,
**or** `readlink /proc/<pid>/exe` with a trailing ` (deleted)` stripped equal
to the target path, the target's `.previous` (your `install_file` keeps that
name), or the older updater's `.arbos-kernel.arbos-old` beside it. The
`readlink` form works for a deleted file — the magic link still names the old
path — and it is what `~/sweep.sh` uses. Both tests are by identity or by
exact path; neither is by process name, so your confinement argument holds.

(Your "record first" reason is slightly off but the order is still right:
after the rename `/proc/<pid>/exe` still resolves for the stale processes —
`stat -L` on a deleted magic link works — what changes is the inode *at the
path*. Record the old inode before the rename and you can match after it too.)

## 2. `ppid == 1` does not tell your two shapes apart

Observed now:

| Process | `ppid` | parent | cgroup | Actually |
|---|---|---|---|---|
| pod `alpha`, `beta` | 1 | `start.sh` (**PID 1 of the container is a script, not init**) | `/init.scope` | detached — correct |
| pod `gamma` | loop pid | `bash` loop | `/init.scope` | supervised — correct |
| ArbosLife `subnet120`, QA's, parity's | 1 | `systemd` | `session-NNNNN.scope` | detached — correct |
| ArbosLife `demo`, `feedback`, worker, hub, phone | loop pid | `start.sh` / `bash` loop | `user@1001.service/tmux` | supervised — correct |
| **a kernel under a systemd unit** (the supervisor Jacob's box should get) | **1** | `systemd` | `…/<name>.service` | **supervised**, and `Restart=` may be unset — `ppid==1` says "relaunch it yourself", which double-starts or races the unit |
| **a kernel started in a shell or a tmux pane without a loop** | **≠ 1** (the shell) | `bash`/`sshd` | session or tmux | **nothing restarts it** — "stop and stop there" leaves it dead. This is the shape QA's and the parity loop's kernels had until they were relaunched detached today |

Two of the six real shapes are misclassified by `ppid`. `cgroup` is a
better hint (`.service` → unit; `session-*.scope` → detached), but no
classification is safe, so make the *outcome* the test instead:

> **Stop; then wait for a replacement to appear on the same place (new pid in
> its `kernel.json`, whose `exe` inode is the new target's) for a bounded
> window — 10 s covers every loop here, which sleep 2 — and only if nothing
> came back, relaunch it yourself from the recorded argv, cwd and fds under
> `setsid`.**

That handles the loop, `systemd` with `Restart=`, `systemd` without it, an
interactive parent, and a container whose PID 1 is a script, without knowing
which you have. It also removes the place-lock race you were worried about:
you relaunch only after confirming the old pid is gone and nothing else took
the lock. Use `ppid`/cgroup only to skip the wait when the answer is obvious
(session scope + parent 1 → relaunch at once).

One more thing your record must hold: **argv[0] may be relative.** The parity
loop's kernel on ArbosLife right now is `bin/arbos-kernel serve …` with cwd
`~/arbos-remote/parity-proj…`. A relaunch from argv alone fails; from argv +
recorded cwd it works. Your identity match is immune to this (a path match
would miss it entirely), which is one more reason it is the right primitive.

## 3. There is a third shape: the worker daemon's worktree kernels

`spawn host=<machine>` makes the daemon start `arbos-kernel serve
<checkout>/.arbos/worktrees/<claim-id>` from the same file. Right now on
ArbosLife: pid 694179, parent = the daemon. Facts about them:

- The daemon **never restarts one** (it starts a new one per claim) and does
  not reap finished ones (that pid is a zombie; `stat -L` on its `exe` fails,
  so your loop skips it — fine).
- A live one is **another machine's work in flight**: its turn is a remote
  parent's `spawn`, and its output is delivered to the parent's store. Stopping
  it mid-turn fails a spawn on a box you cannot see.
- When you stop the **daemon**, its live children are reparented to PID 1 and
  now look like your "detached" shape; a naive pass would then relaunch a
  finished child as a lingering kernel.

Rule: a kernel whose place is under `.arbos/worktrees/` is a child. **Busy →
leave it (and say so). Idle → stop it and do not relaunch; the daemon starts
fresh ones from the new file on demand.** Handle children *before* the daemon
so the reparenting never confuses the pass, then treat the daemon by the
rule in §2 (on ArbosLife it is under a loop; under `systemd` it will be a
unit).

## 4. "Stop and stop there" — what never comes back

With `ppid` as the test: any kernel whose parent is an interactive shell,
`sshd`, `nohup` without `setsid`, or a tmux pane with no loop. On a box where
"the supervisor only restarts on exit" is true, stopping is enough — but that
is the loop shape, and you already identify it. The dangerous cases are the
ones where the parent is *not* a supervisor at all, and `ppid ≠ 1` cannot
tell. §2's "wait, then relaunch" is the fix; without it you trade one outage
shape (deleted image) for another (stopped and gone), and the second is
louder but not better.

## What is good and should stay

- Identity, not path, not name; confinement by `EACCES` on another user's
  `exe` — I checked from `arbostest`: `stat -L /proc/1/exe` → `Permission
  denied`. Keep it exactly so.
- Record before replace; rename into place via `install_file`; busy check
  with a stale kernel winning. All three are the lessons of the night stated
  correctly.
- macOS by `lsof` inode. Right, and the note in the sweep document has the
  parsing that was verified on a replaced binary.

## Acceptance on the target, sharpened

Bootstrap while connected to `alpha`. Expect: `alpha` and `beta` relaunched
by you (parent 1, session-less container), `gamma` back via its loop **without**
your relaunch (its pid changes, its parent is still `loop-gamma`), all three
on the new inode, `~/sweep.sh` with no `GONE`, and — the §1 case — run
`~/reset.sh`, replace the file **twice** (install, then install again without
restarting), and confirm the pass still finds all three although two of them
now sit on an inode two generations old. If that last one passes, it would
have found ArbosLife's daemon.

Voice gateway before/after: `curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:8765/healthz` → 200.
