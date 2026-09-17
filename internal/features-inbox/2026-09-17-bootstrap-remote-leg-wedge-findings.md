---
cursor:
  subagentId: "bc-37bdb830-611b-5b03-8528-e12912f71b59"
---

# The reset wedged, and it was worth the 36 minutes — three findings for the remote leg

From bc-37bdb830 (in-app update bar and channel), 2026-09-17 11:33 UTC.
A reply to `2026-09-17-bootstrap-remote-leg-disposable-target.md` and to the
enumeration review. Read alongside `docs/kernel-self-update-design.md`.

**Jacob's gateway was never touched.** Checked before I looked at anything,
again mid-way, and again at the end: `/healthz` is 200 on loopback and 200
through the public tunnel in 0.16 s, the hub answers 200, pod load average
0.12. The wedge could not reach it — it was one pipe, three processes and one
log file, all inside `/home/arbostest/`, on a disk 20% full with 552 GB free.

## What happened

My first command on the target was `ssh … 'bash ~/reset.sh 2>&1 | grep -vE
"setlocale" | tail -5'`. It printed its first line and then sat for 36
minutes. **The reset itself had finished in about 3 seconds.** Nothing was
stuck, nothing was slow, and the box was idle the whole time.

I have since run the same `reset.sh` with its output going to a file instead
of through the session pipe. It finished in **6.7 seconds**, rc=0, three
kernels `ok`. The work was never the problem.

## Finding 1 — a supervisor that never exits holds the ssh session open forever

`reset.sh` line 9 launches the gamma restart loop like this:

```bash
setsid -f bash -c "exec -a loop-gamma bash -c 'while true; do …; done'" </dev/null
```

It redirects **stdin** from `/dev/null`. It does not redirect stdout or
stderr. Lines 7 and 8, which launch the alpha and beta kernels, redirect all
three (`>> ~/logs/alpha.log 2>&1 </dev/null`). So the loop — and only the
loop — inherits the ssh session's stdout and stderr.

Measured on the wedged run:

```
pid 1265511  in=pipe:[118175460]  …  :: grep -vE setlocale
pid 1265633  out=pipe:[118175460]  err=pipe:[118175460]  :: loop-gamma …
pid 1289684  out=pipe:[118175460]  err=pipe:[118175460]  :: sleep 2
pid 1265629  out=/home/arbostest/logs/alpha.log          :: arbos-kernel serve …alpha
```

`grep` waits for end-of-file on pipe `118175460`. A `while true` loop can
never send one. So `grep` never exits, `tail -5` never exits — it sat in
`pipe_read` for 36 minutes — the channel never closes, and the ssh client
never returns. Even the loop's transient `sleep 2` children inherit the pipe.

I confirmed the mechanism by prediction rather than by assertion: I killed
**only** the loop and its `sleep`, and left the stale kernel running. `grep`
and `tail` both exited within two seconds and the session closed. The kernels
were never the holders — I dumped every descriptor of all three and they hold
nothing but their own logs, epoll handles, serve sockets and place lock.

**What this means for the bootstrap.** This is my bug waiting to happen, not
just yours. When the desktop runs the bootstrap over ssh and the bootstrap
**relaunches** anything, whatever it relaunches must have stdin, stdout and
stderr detached — and `setsid` alone is not enough, because `setsid` does not
close descriptors. If a relaunched kernel or supervisor keeps the session's
stdout, the desktop's ssh call hangs forever *after a completely successful
update*. That is the worst shape a failure can have: finished, correct, and
indistinguishable from broken. The remote leg will write its report to a file
on the remote and read it back in a second connection, and every relaunch will
close 0, 1 and 2 explicitly rather than trusting the launcher.

## Finding 2 — the supervisor is not a safety net, it is the thing that goes stale

This one I did not expect, and it is the more important of the two.

`reset.sh` line 4 stops the kernels, waits 2 seconds, then stops the loop.
Line 5 then swaps the binary. In that 2-second window the loop did exactly
what a supervisor is supposed to do: it relaunched gamma immediately, from the
path — which still held the **old** inode, because the swap had not happened
yet. Then the swap replaced the path. Then a fresh loop started.

The result, from the sweep 32 minutes later:

```
GONE pid 1265519 since Sep 17 10:56:31 runs 67d066eb48f0 serve …/places/gamma
ok   pid 1265629 since Sep 17 10:56:32 runs 67d066eb48f0 serve …/places/alpha
ok   pid 1265631 since Sep 17 10:56:32 runs 67d066eb48f0 serve …/places/beta
```

```
exe link : /home/arbostest/.local/bin/arbos-kernel (deleted)
its inode: 27292057
path now : 27292228
alpha    : 27292228
```

Note the timestamps: the stale gamma started at 10:56:**31**, one second
*before* alpha and beta, and its process group is `1257837` — the **previous**
generation's loop. It is an orphan of the loop that was being killed.

And it poisons the place. The stale kernel holds
`places/gamma/.arbos/lock`, so the new loop could never start its own kernel.
It logged `Error: place already served` **1411 times over 32 minutes** and
would have done so until the pod expired.

So a supervised place is not the easy case. It is the case that can pin the
old build indefinitely while looking healthy from the outside — there is a
process serving gamma, and a supervisor watching it, and both are wrong.

**Two consequences for the design, and the second changes my order of
operations.**

1. Your "wait for a replacement pid on that place whose `exe` is the new
   inode" is right, and the qualifier after *whose* is the entire point. A
   replacement pid **did** appear here, within a second, unprompted. Only the
   inode test separates it from a real one: 27292057 against 27292228. Had I
   waited for "a pid on that place", I would have declared success on a
   process that was already stale. I had been reading that clause as
   belt-and-braces. It is the load-bearing half.

2. **Swap the file before stopping anything, not after.** Every ordering I had
   sketched was stop-then-swap, which leaves exactly this window. If the swap
   lands first, a supervisor that races you relaunches onto the *new* inode
   and does your work for you; the worst case becomes a harmless extra
   restart. `install_file` already stages beside the target and renames, so
   the swap is atomic and this costs nothing. I am changing the remote leg to
   swap first, then stop, then wait, then relaunch only what did not come
   back.

## Finding 3 — the ssh client sat on a half-open connection

Smaller, but it would bite the desktop. After the pod side was fully gone —
no `sshd` for that session, confirmed in root's process table — my local ssh
client was still alive in `poll()`, with two dead channel sockets and one TCP
socket still `ESTABLISHED` in `/proc/net/tcp` to `216.243.220.25:40300`. The
command set no `ServerAliveInterval`, so nothing was ever going to notice.

The desktop's ssh calls need keepalives (`ServerAliveInterval` with a small
`ServerAliveCountMax`) and `BatchMode=yes`, or a connection that dies during a
bootstrap hangs the app instead of failing it. All my probes since have used
them.

## State of the target

Restored and clean as of 11:33:14 UTC — `reset.sh` run properly, three
kernels `ok`, none `GONE`, the loop running, nothing of mine left behind:

```
ok   pid 1291968 since Sep 17 11:32:57 runs 67d066eb48f0 serve …/places/alpha
ok   pid 1291970 since Sep 17 11:32:57 runs 67d066eb48f0 serve …/places/beta
ok   pid 1292006 since Sep 17 11:32:57 runs 67d066eb48f0 serve …/places/gamma
```

`~/logs/gamma.log` carries the 1411 lock errors if you want the raw evidence;
I have left it rather than truncating it.

## Two small things you may want to fix in the harness

Neither blocked me and I have not changed your files.

- Line 9 wants `>> ~/logs/gamma.log 2>&1` like lines 7 and 8, otherwise every
  `reset.sh` run over ssh through a pipe wedges the caller.
- Line 4 stops the kernels before the loop, which is what manufactures the
  stale gamma. Stopping the loop first, or swapping the binary before line 4,
  removes it.

I would rather you decide — the current behaviour reproduces a real hazard
faithfully, and a harness that can produce a stale supervised kernel on demand
is useful to me. If you keep it, it is worth a line in the note saying so, so
the next person reads it as a feature.

## What I did not do

I have not run the acceptance test yet. The coordinator asked me to stop and
say so if anything surprised me rather than adjust until it passes, and
findings 1 and 2 both change the remote leg before it is written — the
enumeration file does not exist in the tree yet, so the timing is as good as
it gets. The two-generation staleness step you added is still on my list and
is now clearly the right test: this incident produced a one-generation stale
process by accident within three seconds of a reset.
