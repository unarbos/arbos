---
cursor:
  subagentId: "bc-37bdb830-611b-5b03-8528-e12912f71b59"
---

# Answer before the live test: no, my enumeration cannot see `beta` and `gamma`

Reply to `2026-09-17-bootstrap-remote-leg-disposable-target.md`, from the
update-channel worker. Written before running anything on the pod, as asked.

## The answer

**No.** Asked to bootstrap `alpha`, everything I have shipped would find
`alpha` and nothing else.

Every enumeration in the update path today is **by place**, which is the exact
failure you named:

| where | what it does |
| --- | --- |
| `desktop/src/update.rs` `stop_kernels(places)` | iterates *places*, reads each `kernel.json`, signals that one pid |
| `desktop/src/kernel.rs` `restart_kernel(place)` | one place, one pid from its `kernel.json` |
| `arbos-core` `remote_kernel::stop_script(place_dir)` | reads **that place's** `kernel.json`, kills that one pid |

So the remote leg as it stands would bootstrap the binary, rename the new file
into place, restart `alpha`, and leave `beta` and `gamma` on the deleted
image. That is your 5 h 34 m, reproduced exactly, on a box built to catch it.

I would rather say that now than show you a green test that only proves the
easy third of it.

## What I intend to build instead, for review before I run it

**Enumerate by running file, before anything is replaced.** The order matters:
after the rename the stale processes hold the *old* inode, so matching on "the
inode at the path now" would find none of them. Record first, replace second.

```sh
# Identity, not path. Taken before the replacement, when all three still
# share it.
want=$(stat -L -c '%d:%i' "$target")
for p in /proc/[0-9]*; do
    # readlink on another user's exe fails with EACCES for an unprivileged
    # user, so this cannot see root's voice stack even by accident. No
    # process name is matched anywhere — there is no `pgrep arbos-kernel`.
    id=$(stat -L -c '%d:%i' "$p/exe" 2>/dev/null) || continue
    [ "$id" = "$want" ] || continue
    # …record pid, ppid, cmdline, cwd, fd/1, fd/2 …
done
```

That satisfies your constraint by construction rather than by care: **the
confinement is the kernel's permission check, not a filter I wrote.** I never
enumerate by name, and I cannot read the `exe` link of a process I do not own.

**Two shapes, two actions**, from the recorded `ppid`:

- `ppid == 1` → detached, nothing will bring it back. Relaunch it myself:
  `setsid`, stdin from `/dev/null`, the recorded argv, the recorded `cwd`,
  stdout and stderr appended to the recorded `fd/1` and `fd/2` targets.
- `ppid != 1` → supervised. Stop it and stop there; relaunching it too would
  race the loop for the place lock.

**Busy first, and a stale kernel wins.** Before stopping any of them, the
checks from your note — the last turn's `ended`, `pgrep -P <pid>`, established
clients on its port. Mid-turn or holding work means leave it, say so, and
report the box as partly done rather than quietly interrupting it. The old
binary predates `update_gate`, so `/healthz` is not available here; on a
kernel new enough to answer, that is the better question and I will prefer it.

**Rename, never copy over the running file.** `arbos-update`'s `install_file`
([#430](https://github.com/unarbos/arbos/pull/430)) stages beside the target
and renames, keeping the replaced build as `<bin>.previous`. The remote leg
calls that rather than `mv -f`, which is what produces the deleted-image state
in the first place.

**macOS by identity too.** `lsof -p <pid> -Ffti` for the `txt` entry against
`stat -f %i` at the start path — no `/proc`, no `(deleted)`, and after an
in-app update the path exists and holds the new file, so the path proves
nothing. I have been bitten by exactly this twice: the cross-build warning
compared commits and missed a same-build deleted image, and #385's first
detector compared the file at `current_exe()` with the one seen at start,
which after a directory rename is the same file.

## On the pod's other tenant

Understood and treated as the hard limit. Nothing outside
`/home/arbostest/`; no enumeration or signalling by process name at any point;
and I will confirm the voice gateway still answers before I report. If any
step of my design turns out to need something I cannot confine to that user's
own processes, I will stop and say so here rather than widen it.

## What I would like from you

Nothing blocking — but if the enumeration above looks wrong to the person who
swept the real box, say so before I run it. You have seen the failure and I
have only read about it.
