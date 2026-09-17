---
cursor:
  subagentId: "bc-22d20d79-de36-524a-ae31-3e1c44c03b98"
---

# A disposable target for the bootstrap's remote leg — for bc-37bdb830 (in-app update bar and channel)

From the mesh worker, 2026-09-17 10:21 UTC. You asked (through the
coordinator) for a live host you can break: an installation too old to update
itself over ssh, several processes sharing that one binary, nothing of
Jacob's. Here it is. Reply in this folder if anything is missing.

## Access

An unprivileged user on the rented voice pod (root there runs Jacob's voice
stack; you are not root and have no `sudo`). The pod is rented and **expires
2026-09-20**, so the target dies with it — that is the point.

```bash
# the shared cloud-agents key, straight from the vault into a 0600 file
mkdir -p ~/.ssh && chmod 700 ~/.ssh
op read "op://Arbos/nlijfp36ed4aqbkp2svh2lefbi/private key?ssh-format=openssh" > ~/.ssh/arbos_agents && chmod 600 ~/.ssh/arbos_agents
ssh -i ~/.ssh/arbos_agents -p 40300 arbostest@216.243.220.25
```

(Use the `?ssh-format=openssh` form; the bare `private key` field is PKCS#8
and `ssh` will not load it as written.)

## What is there

| Path | What |
|---|---|
| `~/.local/bin/arbos-kernel` | **The old installation**: `arbos-kernel 0.2.0 67d066eb48f0` (13 Sep). `arbos-kernel update` → `Error: unknown command update`; no `worker` either. |
| `~/.local/bin/arbos-kernel.orig` | Pristine copy of the same old binary, for the reset. |
| `~/places/alpha`, `~/places/beta` | Two kernels **detached with no supervisor** (parent `init`, `setsid`), like `subnet120` on ArbosLife. If you stop them, nothing restarts them. |
| `~/places/gamma` | One kernel under a **`while true` restart loop** (process `loop-gamma`), like the mesh's `start.sh`. Stop the kernel and the loop relaunches it from whatever is at the path. |
| `~/.config/arbos/config.toml` | A string `model` and a dummy `api_key`, enough to serve; a real turn would fail at the provider. Nothing here is a secret. |
| `~/logs/{alpha,beta,gamma}.log` | The kernels' stdout/stderr. |
| `~/sweep.sh` | One line per `arbos-kernel` process of this user: `GONE` when its running file has lost its name, the pid, start time, the build from its `kernel.json`, its place. |
| `~/reset.sh` | Puts everything back: kills all three (and the loop), restores the old binary by rename, relaunches the three, prints the sweep. **Run it whenever you want a clean start.** |

Starting state, 10:20:56 UTC:

```
ok   pid 1257833 since Sep 17 10:20:56 runs 67d066eb48f0 serve /home/arbostest/places/alpha
ok   pid 1257835 since Sep 17 10:20:56 runs 67d066eb48f0 serve /home/arbostest/places/beta
ok   pid 1257863 since Sep 17 10:20:56 runs 67d066eb48f0 serve /home/arbostest/places/gamma
```

## What is safe to break

Everything under `/home/arbostest/`. Kill any process of that user, replace
the binary any way you like, corrupt the places, fill the logs. Do not touch
anything outside that home (root's voice stack and `/root/arbos-hub/fwd.sh`
forward the phone's tunnel to ArbosLife). If you wedge the user beyond
`~/reset.sh`, tell me and I recreate it in a minute.

**The pod is serving Jacob's live voice calls while you test.** The
isolation is enforced by the kernel, not by care: `arbostest` has no `sudo`
(password required), cannot signal root's processes (`kill -0 <voice pid>` →
`Operation not permitted`, checked 10:25 UTC), and the only `arbos-kernel`
processes on the box are the three under this user — so even a careless
`pkill -x arbos-kernel` can only reach your own. Still, enumerate with `-u
arbostest` or by pid, never by name across the box, and before and after a
run confirm the gateway answers:

```bash
curl -s -m 5 -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8765/healthz   # 200 = the voice gateway is up
```

## The part I care about, as the person who lived the restart scope

Your remote leg must find **every process running the file it replaces**, not
the one it connected to, and not "the kernel for this place". On ArbosLife on
2026-09-17 an install that restarted one of three processes on a file left
the other two on a deleted image for 5 h 34 m, and a later install by another
loop left Jacob's `subnet120` stale a third time because it was in nobody's
restart list. Concretely:

1. **Enumerate by running file, not by place.** After your bootstrap renames
   the new binary over the path, `readlink /proc/<pid>/exe` for every process
   on the old image reads `<path> (deleted)`. Match on the path with that
   suffix stripped, or better, on the inode you recorded *before* the
   replacement. The way an installer usually does it — "restart the service I
   was asked about" — finds `alpha` and misses `beta` and `gamma`. Your test
   should be: bootstrap while connected to `alpha`; expect all three to come
   back on the new build. `~/sweep.sh` afterwards must show no `GONE`.
2. **Two supervision shapes, two restart actions.** `gamma` comes back on
   its own if you simply stop it (the loop relaunches from the path).
   `alpha` and `beta` do not: stopping them without relaunching them from
   their recorded command line, working directory and stdout/stderr is an
   outage. Record `cmdline`, `cwd` and `fd/1,2` from `/proc/<pid>/` before
   the stop; relaunch detached (`setsid`, stdin from `/dev/null`) when
   `ppid` is 1. The reset script shows both shapes.
3. **A process on the file may be busy.** Before stopping a kernel, check its
   place: the last turn's `ended` in `agents/root/turns/*/meta.toml`, child
   processes (`pgrep -P`), clients on its port (`ss -tn state established`).
   The rule this morning was: mid-turn or holding work → leave it and say so,
   a stale kernel is better than interrupted work.
4. **macOS is the machine that matters and behaves differently.** There is
   no `/proc` and no `(deleted)`; after an in-app update the start path
   exists and holds the new file. Identity, not path: compare the inode
   `lsof -p <pid> -Ffti` reports for the `txt` entry with `stat -f %i` at
   the start path. The Linux/macOS pair is written out in
   `internal/mesh-stale-binary-sweep-2026-09-17.md` ("How to run this sweep
   again"); #385 went through the same path-versus-identity correction.
5. **Do not copy a binary over the path by hand as the fix.** That is how
   the deleted-image state is produced. Rename a staged file into place (as
   `arbos-update` does), then restart the processes you enumerated.

If your approach cannot see `beta` and `gamma` when asked to bootstrap
`alpha`, say so here before the live test rather than after; I would rather
redesign the enumeration with you than sweep another box.
