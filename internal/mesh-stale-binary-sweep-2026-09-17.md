---
cursor:
  subagentId: "bc-22d20d79-de36-524a-ae31-3e1c44c03b98"
---

# Mesh sweep: processes running a binary that was deleted under them (2026-09-17 04:00–04:10 UTC)

What was checked: every `arbos-kernel` / `arbos-hub` process on every machine
the mesh can reach, its `/proc/<pid>/exe` (the file the process is running;
Linux appends ` (deleted)` when that file is gone), the build it reports in its
own `kernel.json`, and the mtime of the file now at its path. "Gone since" is
the last replacement of that file, so it is a lower bound.

Machines: ArbosLife (`const@204.12.171.6`), Templar (`const@204.12.168.71`),
the voice pod (`root@216.243.220.25:40300`), this cloud VM. Jacob's Mac and
`chakanaone` are not reachable from here. The hub (`ws://127.0.0.1:7010` on
ArbosLife) has exactly one registered machine, `arboslife`.

## Found in the state (7 processes, 2 machines)

| Machine | Process | Started | Running build | File replaced (gone since) | Gone for | On the hub? | Effect |
|---|---|---|---|---|---|---|---|
| ArbosLife | **worker daemon** `~/arbos-hub/bin/arbos-kernel worker` pid 1612928 | 09-16 19:11 | `bfb36e98` | 09-16 22:34 (`b6e7098`, my `update --install` for #357) | **5 h 34 m** (restarted 04:08) | yes | **every spawn refused** — 2 claims, 23:07 and 01:43 UTC |
| ArbosLife | `feedback` service kernel pid 1612848 | 09-16 19:11 | `bfb36e98` | 09-16 22:34 | 5 h 34 m (restarted 04:08) | yes | none visible; served turns on the old image |
| ArbosLife | `~/arbos-qa/cycle-11/demo` kernel pid 3238515 (QA loop's) | 09-15 13:09 | `5bff3ec9` | 09-16 17:21 (`~/.cargo/bin`, `efcab58f`) | 10 h 40 m, still running | no (own place, QA's) | stale; cannot ssh-spawn or re-exec |
| ArbosLife | `~/arbos-remote/parity-proj--reply-with-hostname` kernel pid 2739473 (parity loop's) | 09-13 18:24 | too old to write a sha | 09-14 13:10 | **2 d 15 h**, still running | no | stale test kernel |
| Templar | `~/attach-test` kernel pid 669043 | 09-16 08:21 | `44bae9b4` | 09-16 11:45 (`~/.cargo/bin`) | 16 h 20 m, still running | no (Templar has no `hub.toml`) | stale test kernel |
| Templar | `~/arbos-qa/cycle-11/demo` kernel pid 3935016 (QA loop's) | 09-15 13:26 | `1fb5f776` | 09-16 11:45 | 16 h 20 m, still running | no | stale |
| ArbosLife | 7 zombie children of the worker daemon (19:36–22:34) | — | — | — | — | — | the daemon never `wait()`s finished worktree kernels; cleared by the restart |

Not in the state: `demo` (`b6e7098`, restarted 22:37), `subnet120` and the
phone kernel (`efcab58f`, restarted 17:24), the hub (`01:35`), `k17-test`
(runs a build moved aside as `arbos-remote/old/arbos-kernel.67dcb85`, so its
file exists — old but honest). The voice pod runs no `arbos-kernel` or
`arbos-hub` at all now (hub and phone kernel forwarded to ArbosLife). This VM
runs none.

## How long the mesh was actually dead

**5 h 34 m, not two days.** The worker log (`~/arbos-hub/logs/worker.log`)
goes back to the first quick tunnel on Sep 13 and holds exactly two
`No such file or directory` claims, both after 22:34 UTC Sep 16. Before that,
8 claims between 19:11 and 22:34 each started a worktree kernel. The mobile
loop's earlier "four identical failures" were #357's empty-worktree bug, a
different cause.

The cause was mine: at 22:34 I ran `arbos-kernel update --install` on
`~/arbos-hub/bin/arbos-kernel` for #357 and restarted the `demo` kernel only.
The worker daemon and the `feedback` kernel share that file and were left on
the deleted image. Restarting a binary's *every* process is now on my
checklist for any install; the note below asks the node to say it itself.

## What I did

- 04:08 UTC: `SIGTERM` to the worker daemon and the `feedback` kernel; their
  `start.sh` loops relaunched both from the file on disk (`b6e7098`). The
  worker re-registered (`id 13`, checkouts `demo, feedback, subnet120`); the
  zombies were reaped with the parent.
- 04:09 UTC: one real spawn from `demo`'s root, `spawn host=arboslife`, through
  the hub and the worker. Claim `c161060-1` started a worktree kernel at
  `projects/demo/.arbos/worktrees/c161060-1`; the child answered
  `computeinstance-u00sd1yvtcwsc091pb` / `2026-09-17T04:09:23Z` and root
  reported it verbatim. Turn complete in about 50 s.
- Subscribed to #382. When it merges I update `~/arbos-hub/bin/arbos-kernel`
  again, restart **all three** of its processes (kernel, feedback, worker), and
  repeat the spawn.
- 04:19 UTC, on the coordinator's decision: Templar's `~/.cargo/bin/arbos-kernel`
  (`3940aac3`, a day behind `main`, too old to have `update`) was brought to
  the dev channel's `7f6a6b9a06bc` (build 1232) with the kernel's own
  `update --install` run from a staged copy of ArbosLife's `b6e7098` binary
  (`/tmp/arbos-kernel-updater`, deleted after). Both kernels backed by that
  file — `arbos-qa/cycle-11/demo` (idle since Sep 15 13:34) and `attach-test`
  (idle since Sep 16 11:59, still on the deleted image) — were checked idle
  (no turn, no child, no client), stopped with `SIGTERM`, and relaunched
  detached with their original command lines and log files. Sweep afterwards:
  two `ok` lines, both `runs 7f6a6b9a06bc`; `arbos-kernel --version` on disk
  says the same. Nothing else on Templar touched. The old `update` report
  confirmed the trap: it flagged `attach-test` as "older than the binary on
  disk" while the sweep alone would have shown a second process on the file.
- Left alone: the four stale test kernels that belong to the QA and parity
  loops (`arbos-qa/cycle-11/demo` on both boxes, `arbos-remote/parity-proj…`,
  `attach-test`). They are not on the hub and not mine; their owners should
  restart or stop them. Names and pids are above.

## How to run this sweep again

**This is a stopgap with an end date.** It exists only until `binary_gone`
lands on `hello`, on `register`, and on the hub roster
(`internal/features-inbox/2026-09-17-running-binary-gone-in-hello.md`). From
then on `/list` and the desktop's version line carry the fact for every node
on the hub, `arbos-kernel update --place` carries it for local places, and
these scripts should be deleted from this document rather than maintained.

**The rule that would have prevented tonight's outage, on its own line:**

> **One file can back several processes. After any install over a binary,
> restart every process that was started from that file — not the one you
> were thinking about.** On ArbosLife, `~/arbos-hub/bin/arbos-kernel` backs
> the `demo` kernel, the `feedback` kernel and the worker daemon; restarting
> `demo` alone left the other two on a deleted image for 5 h 34 m. Before
> restarting, run the sweep below and count the lines that share the path.

One script per operating system, one line per Arbos process. Each marks `GONE`
when the process runs a file that is no longer the one at its start path,
reads the build the process wrote about itself into its place's `kernel.json`
(not the file on disk), and shows the mtime of the file at the start path so
"gone since" is on the same line.

### Linux (ArbosLife, Templar, the pod, cloud VMs)

Needs bash, `pgrep`, and `/proc`; the kernel appends ` (deleted)` to
`/proc/<pid>/exe` when the running file is gone, so that is the test. Save as
`/tmp/sweep.sh` and pipe it over ssh.

```bash
# Linux. One line per arbos process: state, pid, since, running build, the file it started from, and what is on disk now.
for p in $(pgrep -x arbos-kernel; pgrep -x arbos-hub); do
  exe=$(readlink /proc/$p/exe); file=${exe% (deleted)}
  case "$exe" in *"(deleted)") st=GONE;; *) st=ok;; esac
  set -- $(tr '\0' ' ' < /proc/$p/cmdline); role=$2; place=$3
  case "$1" in /*) file=$1;; esac
  sha=-; for j in "$place/.arbos/runtime/kernel.json" "$place/.arbos/kernel.json"; do
    [ -f "$j" ] && sha=$(sed -n 's/.*"git_sha": *"\([0-9a-f]*\)".*/\1/p' "$j" | head -1) && break; done
  disk=$( [ -f "$file" ] && stat -c %y "$file" | cut -c1-16 || echo missing)
  printf '%-4s pid %-8s since %-16s runs %-12s %s %s  disk:%s\n' "$st" "$p" "$(ps -o lstart= -p $p | awk '{print $2,$3,$4}')" "${sha:--}" "$role" "$place" "$disk"
done
```

```bash
ssh arboslife 'bash -s' < /tmp/sweep.sh
ssh -i ~/.ssh/arbos_agents const@204.12.168.71 'bash -s' < /tmp/sweep.sh   # Templar: no alias, key named explicitly
ssh voicepod 'bash -s' < /tmp/sweep.sh
```

Tested on ArbosLife and Templar at 04:15 UTC; it reproduced the table above
in two seconds, and showed the parity loop had already relaunched its kernel
(`52cb63a0` at 04:14).

### macOS (Jacob's Mac, the AWS Mac)

This is the machine where the fault hurt a person: the desktop's in-app
updates replaced the kernel binary under running kernels several times on
2026-09-16, which left five workers hanging and the feedback sheet empty.
macOS has no `/proc` and does not mark a deleted executable anywhere, so the
test is different: **compare the inode of the file the process is running
(`lsof`, the `txt` entry) with the inode of the file now at its start path
(`stat`)**. An update that unlinks and rewrites, or renames a new file into
place, always changes the inode; the same inode means the same file.

Run it as the user who owns the kernels (Jacob's, for his), because `lsof`
shows only your own processes without `sudo`. Not yet run on a Mac: the AWS
Mac was being rebuilt when this was written. The `lsof -F` parsing was
checked on Linux against a binary replaced while it ran (`run` inode 1925423,
`disk` inode 1925425 → `GONE`; same file → `ok`). The macOS-only parts are
`ps -o comm=` giving the full start path and `stat -f %i`/`%Sm`; both are
standard BSD `ps`/`stat`. Whoever first runs it on a Mac: start a copy of any
binary (`cp /bin/sleep /tmp/t && /tmp/t 300 &`), replace it (`cp /bin/sleep
/tmp/t.new && mv -f /tmp/t.new /tmp/t`), and check the copy reads `GONE`
before trusting the Arbos lines.

```bash
# macOS. One line per arbos process: state, pid, since, running build, start path, and what is on disk now.
# GONE = the inode the process runs is not the inode now at its start path (replaced, moved, or deleted).
for p in $(pgrep -x arbos-kernel; pgrep -x arbos-hub); do
  path=$(ps -o comm= -p $p)                       # on macOS this is the full path the process was started from
  set -- $(ps -o args= -p $p); role=$2; place=$3
  run_ino=$(lsof -p $p -Ffti 2>/dev/null | awk '/^ftxt$/ {t=1; next} t && /^i/ {print substr($0,2); exit} /^f/ {t=0}')
  disk_ino=$(stat -f %i "$path" 2>/dev/null)
  if [ -z "$disk_ino" ]; then st=GONE; elif [ "$run_ino" != "$disk_ino" ]; then st=GONE; else st=ok; fi
  sha=-; for j in "$place/.arbos/runtime/kernel.json" "$place/.arbos/kernel.json"; do
    [ -f "$j" ] && sha=$(sed -n 's/.*"git_sha": *"\([0-9a-f]*\)".*/\1/p' "$j" | head -1) && break; done
  disk=$( [ -n "$disk_ino" ] && stat -f %Sm -t '%Y-%m-%d %H:%M' "$path" || echo missing)
  printf '%-4s pid %-8s since %-16s runs %-12s %s %s  path:%s  disk:%s\n' "$st" "$p" "$(ps -o lstart= -p $p | awk '{print $2,$3,$4}')" "${sha:--}" "$role" "$place" "$path" "$disk"
done
```

The desktop app starts kernels from inside its bundle
(`…/Arbos.app/Contents/MacOS/`), so `path:` will point there; an in-app update
that swapped the bundle leaves every kernel started before it `GONE`. If the
desktop ever names its kernel process something other than `arbos-kernel`,
change the two `pgrep -x` names to `pgrep -f arbos-kernel`.

### Reading either output

- Any `GONE` line is a process to restart from the file now at its start path.
  `disk:` is when that file last changed, so that is how long the process has
  been gone. `disk:missing` (Linux) means the deleted file was the updater's
  `.arbos-old` copy; the start path in the command line is what to relaunch.
- A `runs` sha older than the newest build on that machine, with `ok`, is
  merely old, not dead.
- The worker daemon and the hub write no `kernel.json`, so they show `runs -`;
  their build is the `disk:` file's when `ok`, unknown when `GONE`.
- Several lines sharing one path: restart all of them (the rule above).

## Closing the class

Proposal filed for the features agent:
`internal/features-inbox/2026-09-17-running-binary-gone-in-hello.md` — one
live-computed `binary_gone: bool` on `hello`, `register`/`MachineInfo`, and the
`update --place` serving line; plus per-registrant `git_sha` on the hub row,
because the shared row reported the daemon as `b6e7098` while it ran
`bfb36e98`.
