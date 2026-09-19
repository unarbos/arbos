# qal-j40 — deleting both lock files lets a second kernel take a served place, in silence

- **status**: open (product)
- **found**: 2026-09-18 13:58, taking after-failure states nobody had staged
- **kernel**: `arbos-kernel 0.2.0 cecd48e1bd76 protocol 1` (today's `main`)
- **probe**: `deploy/af05c-runtime-and-lock-removed-probe.sh`
- **control**: `lk-04-removing-both-lock-files-does-not-let-a-second-kernel-in` (added 2026-09-18 17:21; breaks in 3.1 s). It **runs every other cycle**, not every cycle — see below.

## What happens

A kernel serves a place. Remove `.arbos/runtime/` **and** `.arbos/lock` while it runs. A second
kernel then serves the same place, and neither says anything:

```
A serving, pid 810871
removed .arbos/runtime and .arbos/lock; A alive: yes
kernel.json now names pid: 811157
A alive: yes   B's pid alive: yes
distinct live kernels on this place: 2
B said nothing about the place being held
```

Reproduced on every attempt. This is the harm `#450` exists to prevent, reached by deletion rather
than by staleness.

## Why it works

The place lock is `flock` on two files — `.arbos/lock` (legacy) and `.arbos/runtime/lock`
(`place.rs:132–148`). A lock lives on the open descriptor's **inode**, not on the path. Deleting
the files leaves the holder's locks on unlinked inodes, still held and now unreachable; the second
kernel creates fresh files at the same paths and locks *those*, which are different inodes, so it
succeeds. Nothing in the protocol makes the first kernel notice its lock has been orphaned.

## #450 is what stops the ordinary version of this

Removing **only** `.arbos/runtime/` is refused, and the refusal is good:

```
arbos-kernel: place already served — another kernel already serves …: pid 806610,
build cecd48e1bd76, url tcp://…
```

That is `#450`'s design working exactly as intended: because `PlaceLock::acquire` takes **both**
files, deleting one directory leaves the other holding. A script that knows only the new path — or
only the old one — cannot open the place. It takes removing both.

So the ordinary shapes are covered. What is not is the thorough cleanup: `rm -rf .arbos/runtime
.arbos/lock`, which is what someone resetting a place they believe is stuck would reach for, having
read that the lock is in both places.

## What this probe does **not** establish, and I want to be exact about it

After the two-kernel period, `arbos-kernel check` reported **0** double-serving warnings. **That is
not evidence the detector is wrong.** Neither kernel ran a turn in this staging — I started them and
never sent a prompt — so there was no wake to land inside an open turn and no checkpoint to conflict
at a transcript line. `check_two_writers` had nothing to work from, which is the correct outcome for
a period in which nothing was written.

Testing the detector against a genuine concurrent-turn period needs both kernels driven with a
provider and prompts at once, which this probe does not do. `ds-01` verifies the detector against
both shapes it claims, staged directly, and that remains the evidence on it.

I am flagging the gap rather than the detector: a double-serving period with **no turns** leaves no
trace, which is consistent with `#450`'s own stated limits — it cannot see two kernels answering
different ports, nor two writers to `notes.md`.

## Suspected fix

The lock's identity should not be the path alone. Two candidates, either enough:

- hold the lock and re-check that the file at the path is still the same inode (`fstat` on the held
  descriptor against a `stat` of the path) on the same tick that already re-reads the record — a
  kernel whose lock file has been unlinked is in a state it should say something about;
- have the arriving kernel compare `kernel.json`'s pid for liveness *before* trusting a fresh lock,
  which is `#446`'s `names_a_live_pid` question asked of the lock rather than of the record. Here
  `kernel.json` was gone with `runtime/`, so there was nothing to check — which suggests the pid
  belongs somewhere the cleanup does not reach.

## Related

- `#450` — the both-files lock that stops the one-directory version, verified here incidentally.
- `#446` — the same harm by a stale record outranking a live one; `fm-03`.
- `ds-01` — the detector for a double-serving that has already happened.
- `af-02` — two windows on one place, the supported concurrent case.
- `internal/qa-after-failure-probes-that-found-nothing-2026-09-18.md` — the complement of this bug,
  measured: a record that **lies** about its pid is harmless, because the lock is the gate and the
  lock still tells the truth. Here the lock was moved out from under itself and a second kernel got
  in. One mechanism, two directions.

## Now guarded by the library

The probe reproduced this on demand but only when someone ran it. `lk-04` stages the same shape as
a scenario, so every cycle asks the question and a fix is noticed rather than waited for. It holds
one line: while the holder is alive, a second kernel must not end up serving the same place.
Refusing out loud is the good outcome; what must not happen is two servers.

Measured on `arbos-kernel 0.2.0 fba8688d92d2`: breaks in **3.1 s**, auto-draft `06c30e0315`.

## The guard runs every other cycle, not every cycle

I wrote that `lk-04` asks this question every cycle. It does not, and the reason is worth knowing
for any guard anyone adds.

Only the **tracked** step runs the lock family, and it passes `--half`. The half is
`sha1(name) % 2` (`run.py:2222`), so which cycles a scenario appears in is decided by its **name**:

```
half A   lk-01, lk-04, kf-01, fm-01
half B   lk-02, lk-03
```

`lk-04` is in half A, so it runs on half-A cycles only. Cycle 16 ran it (break, 3.1 s); cycle 17
is half B and did not, which is correct rather than a fault — I went looking for a broken guard and
found a working split.

The main step cannot cover the gap: it runs `--kernel-branch rust` (`cycle.sh:95`), and every
`lk-*` is gated to `main`, so all four skip there every cycle.

`kf-01` is in half A too but still runs **every** cycle, because it is desktop-tagged and the
desktop step (`cycle.sh:298`) does not pass `--half`. That is the difference: a guard's cadence
depends on which step picks it up, and only the tagged steps are unconditional.

**If this needs asking every cycle, `lk-04` wants a tag a tagless step selects — not a rename.**
Renaming to flip its hash would work once and then rot the moment anyone renames it again.
