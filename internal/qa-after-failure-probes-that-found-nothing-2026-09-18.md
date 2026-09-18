---
cursor:
  subagentId: "bc-b4f4cdba-0146-5dea-9731-24ea2538adcd"
---

# Two after-failure states I staged that turned out to be well defended (2026-09-18)

Negative results, written down so the next person does not spend the afternoon I spent. Both were
staged against `arbos-kernel 0.2.0 1b4ef7a93fe6`, in the same lane as `qal-j40` (locks and place
records) and `#446` (which record wins).

Neither is a bug. Both are places the defence holds, and the *reason* it holds is worth knowing,
because it is not the reason I expected in the second case.

## A place whose `.arbos` is a symlink

**Why stage it.** Moving the store to another disk and leaving a symlink behind is an ordinary
thing to do with a folder that grows. Nothing in the library covers it, and `qal-j40` had just
shown that lock identity is about **inodes, not paths** — so a path that is not what it appears to
be is the obvious next question.

**What happens.** It works.

```
.arbos -> /tmp/symstore.RBwfIy/elsewhere
A served, pid 1187239; records landed through the link
files under the real folder: 16
.arbos is still a symlink: yes        (nothing replaced the link with a directory)
B refused out loud: yes
kernel.json still names A: yes
```

The kernel serves through the link, writes its records to the real folder, leaves the link alone,
and the place lock still refuses a second kernel. Probe:
`deploy/symlink-store-probe.sh`.

## A record naming a live process that is not a kernel (pid reuse)

**Why stage it.** `#446` settled that *the record naming a live process wins*. Pid reuse makes that
premise interesting: a machine crashes with a `kernel.json` naming pid 4242, reboots, and something
else gets 4242. The record names a live process, the liveness test passes — and there is no kernel
behind it. If the arriving kernel then refuses, a person is locked out of their own place by a
number that means nothing.

**What happens.** The kernel serves anyway.

```
impostor pid 1187930 is a `sleep`, alive: yes
rewrote runtime/kernel.json to name pid 1187930
rewrote .arbos/kernel.json    to name pid 1187930
the arriving kernel refused: no
kernel.json now names: 1187942
```

Both records — the runtime one and the legacy one — were rewritten to name a live `sleep`, and the
arriving kernel took the place regardless.

**Why it holds, which is the useful part.** Not because anything checked that the pid was a kernel.
The gate is the **lock**, not the record: no process held `flock` on the lock files, so the arriving
kernel acquired them and served. The record's pid is used to *describe* a holder, not to decide
that there is one.

That is a better design than the one I went looking for, and it is the exact complement of
`qal-j40`: there, deleting the lock files stranded the real holder's locks on unlinked inodes and a
second kernel got in, because the lock is the gate and the lock had been moved out from under it.
Here the record lies and nothing bad happens, because the lock still tells the truth. **One
mechanism, two directions.** Probe: `deploy/pid-reuse-probe.sh`.

## Why these are not scenarios

Both probes pass, and a scenario that can only pass earns its cycle time only if the thing it holds
is likely to break. The symlink case is cheap enough to reconsider if the lock code is ever
reworked; the pid-reuse case is really a restatement of "the lock is the gate", which `lk-01`
through `lk-04` already hold from four directions. I have left both as scripts rather than adding
them to the library.

If the lock ever stops being the gate — if a future change makes the record's pid decide — the
pid-reuse probe becomes the check that catches it, and it is already written.
