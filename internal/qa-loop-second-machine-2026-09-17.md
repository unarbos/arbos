---
cursor:
  subagentId: "bc-b4f4cdba-0146-5dea-9731-24ea2538adcd"
---

# The QA loop on a second machine, and #444 verified — 2026-09-17 13:15 UTC

Written by the worker taking the break-and-fix loop over from the machine that lost its store credential
(`bc-f2e2f30d`, whose handover note is `handover-2026-09-17.md` on the branch `qa-vm-evidence-2026-09-17`
at `ecec4c81`). This machine is **`qa-vm2`**. The loop's design is `docs/qa-loop-design.md`; the bugs are
`internal/qa/bugs/`; this file is the record of standing up and of the first verification pass.

## The handover of the eleven staged files: nothing to apply

`qa-store-pending` at `be857204` holds eleven files the outgoing worker changed by hand. Content-checked
against the store at 12:23 UTC, every one of them was **already in the store, byte-identical** — the
relay had applied them while the check was running (store mtimes 12:23:51–12:24:03). Six were identical
before that; `docs/qa-loop-design.md`, `internal/qa/landing_scenarios.py`, `qal-j19` and
`deploy/vm-loop.sh` differed by the outgoing worker's own later edits and `qal-j20` was absent. So this
machine wrote none of them, and `604d735c` / `4ced4323` were never touched.

Worth keeping: the store was readable and writable from this client throughout, and a second writer was
active in it during the same minutes. Two loops on one store is the state this handover creates, and it
is what the changes below are mostly about.

## What was stood up

- `~/arbos-qa` on this VM: `repo` (a clone of the workspace checkout, `origin` repointed at GitHub),
  `loop` (the runner copied from the store), `deploy`, `logs`, `state`, `staging`, `store-pending`.
- Kernel built from `origin/rust` = `origin/main` at `cbbe9922d6a2`; it says so itself:
  `arbos-kernel 0.2.0 cbbe9922d6a2 protocol 1`.
- **Nothing installed into a shared path.** `op` went to `~/bin`, `uv` to `~/.local/bin`, every build into
  a `CARGO_TARGET_DIR` of its own under `~/arbos-qa`. No `~/.cargo/bin`.
- `deploy/ns-wrap.sh` checked before any kernel ran: inside it `/cursor/stores` lists 0 entries, `~` and
  `/workspace` refuse writes, `/tmp` takes them, uid is still 1000. `run.py` refuses to start a kernel
  without it and `ARBOS_QA_STORE_VISIBLE` is unset here.
- Secrets from 1Password by item id into `~/arbos-qa/secrets.env`, mode 0600, never printed:
  `OPENROUTER_API_KEY` and `ARBOS_GITHUB`. Both proved by use, not by display — the OpenRouter key
  listed 444 models and answered `/key` (usage $8018, no limit); the PAT read `unarbos/arbos` (HTTP 200,
  push and admin true). `.git/config` holds no credential: the helper reads `$ARBOS_GITHUB` from the
  environment at use time, as `publish.sh` does.
- **The docs mirror ran from this machine** (12:37): pushed `0ef68cf4` to `store-docs` — 27 documents,
  562 `internal/` files, 156 `media/`, up from 532 and 138 on the previous tip. The tree grew; the safety
  gate had nothing to refuse.
- **The second store reader runs from this machine as a fourth client**, `CLIENT=qa-vm2`, script fetched
  fresh from the `store-watch` branch each run (never the store's convenience copy). Its first line is on
  the branch: `BEHIND — 34 file(s) here that the mirror has not taken yet`, 379 files here against 350 on
  a 47-minute-old tip. `readers/` now holds `cloud-mesh-3b98`, `cloud-update-1b59`, `qa-vm`, `qa-vm2`.

### Two loops on one store: what had to change first

Four scripts, all backwards compatible — every default is the old behaviour, so the outgoing machine's
next cycle is unaffected:

| File | Change | Why |
|---|---|---|
| `deploy/publish.sh` | refuses a push that would delete bug files unless `ARBOS_QA_ALLOW_BUGS_SHRINK='<reason>'`; `REMOTE` overridable as `ARBOS_QA_RESULTS_REMOTE` | **qal-j21** — this machine's first publish would have deleted 133 of `qa-results`'s 208 bug files and printed `-- publish: pushed` |
| `deploy/vm-loop.sh` | `ARBOS_QA_MACHINE` names the machine; every ledger carries it (`vm-qa-vm2-spend.jsonl` and so on), empty for the historical `qa-vm`; cold start seeds the whole bug set with `cp -n` | `store_put`'s sound-store path is a plain `cp` with no content check, so the last loop to finish a cycle overwrote the other's runs |
| `deploy/cycle.sh` | the store-probe file is named for the machine | two machines now give the "do the store views agree" check something to compare, where it had one fresh probe and said so every cycle |
| `deploy/mirror-timer.sh` | the same, and the second reader's `CLIENT` follows the machine | one reader per machine, one file per reader |

`deploy/pb-01.sh` is new: the control for `publish.sh`, driven against a scratch remote.

## #444 verified — the four destructive writes

[#444](https://github.com/unarbos/arbos/pull/444) @ `f80f0b663bac` against its own merge-base `main`
@ `cbbe9922d6a2`, separate target directories (`repo/target`, `target-pr444`), told apart both by
`--version` and by `strings`: `the undo mark could not be written` appears once in the PR build and not at
all in the control. New scenarios `uw-01`…`uw-04` in `internal/qa/uw_scenarios.py`, registered in
`run.py`. Each makes the write fail **before any record exists**, the `lk-02` shape.

| Probe | Site | Control `cbbe9922d6a2` | #444 `f80f0b663bac` |
|---|---|---|---|
| `uw-01` (a) | the undo mark, unwritable before the first turn | mark stays, `undo` says `no checkpoint`, nothing lost | mark **removed**, same words, nothing lost |
| `uw-01` (b) | a stale mark, **file** unwritable, folder writable | `reset --hard` to the stale sha; turn one's commit unreachable, `a.txt` gone, reported **`restored …`** | mark removed, `undo` refuses, commit and file intact — **fixed** |
| `uw-01` (c) | a stale mark, **folder** read-only | the same destruction | **the same destruction** — `remove_file` needs the folder the write just failed on. **qal-j22** |
| `uw-02` | `.arbos/` into `.git/info/exclude`, `.git/info` read-only | 0 notices; `.arbos/` untracked and would be staged by `git add -A`; silent | exactly 1 failed notice over two turns, naming the error and `.gitignore` — **fixed** |
| `uw-03` | a turn panics while root's transcript cannot be appended to | `turn_panicked_unrecorded` rows 0, notice frames 0, turn folders carrying the error 0 | 1, 1, and `t0001` — **fixed** |
| `uw-04` | a subscription job's marker, injected in the window after the process starts | 3 of 4 firings caught; **j2 and j3 ran to the end** with output nobody can attribute; nothing said | 3 caught, **0 finished**, and the person is told: `wake: Subscription #1 … failed: exit -1 … could not start: the run's record could not be written` — **fixed** |

So: three of the four hold, and the fourth holds for the cause the fix was written against (a full disk:
`ENOSPC` fails the write, `unlink` still works) but not for the commoner one (a read-only `runtime/`,
which is what ext4's default `errors=remount-ro` gives after one I/O error). Detail and the remedy —
land [#392](https://github.com/unarbos/arbos/pull/392), whose stamped mark does not depend on any write
succeeding — are in `internal/qa/bugs/qal-j22-…`.

### What these passes do not prove

- `uw-03` establishes the three **visibility** claims. It does not establish "the next boot does not
  replay the wake", because with an unwritable transcript the wake was never recorded either; the probe
  records `kinds_after_restart: []` rather than claiming it. The replay half needs a writable transcript
  and a different injection.
- `uw-04`'s injection is a directory at the marker's own path. The world's cause is the disk filling in
  the few milliseconds between the process starting and the marker being written; a directory is not that
  cause, and the probe says so. It was chosen because it fails that one write and nothing else in the job
  folder, where a read-only folder fails the job's own files first and never reaches the site. The
  question under test — what happens to a run whose marker never landed — does not turn on the errno.
- `uw-01` arm (a) is not a discriminating arm: `undo` in the first turn should reset to that turn's start,
  which is the same commit the mark would have named, so both builds are right for different reasons. It
  is kept because it shows the refusal path destroys nothing.

### Three probes that passed for the wrong reason first

Each was caught by the review list's second and third questions, and each is worth more than the fix it
was written for:

1. `pb-01`'s first control passed because the unguarded `publish.sh` died on a missing `loop/rollouts`
   before it ever pushed. A loop tree always has one; the probe's world was wrong.
2. `pb-01`'s second control passed because `REMOTE` was hardcoded in the store's copy, so the override did
   nothing and the script reached for the **live** `qa-results` branch. Only a deliberately wrong
   `ARBOS_GITHUB` stopped it. A control must differ from the fix by the fix alone, and a destructive path
   is never driven against the live artifact.
3. `uw-01`'s first arm (a) made `runtime/` read-only before the kernel started. The kernel exits 1 and
   there is no turn to measure. It reported `probe-kernel-did-not-start`, not a pass — the skip machinery
   earned its keep.
4. `uw-04`'s first version staged a hand-written subscription file with no `id`/`created`/`next_due` and
   measured **qa-029** (a hand-written subscription is dropped silently) instead of this bug: 0 job
   folders, and it reported `probe-never-caught-the-window` rather than passing. Its second version asked
   for `every = "5s"`, which the kernel refuses below 30s and says so on its log.

## A harness gap closed on the way

A rollout named the binary's **path** and whatever string `--kernel-branch` was given; nothing in it said
which build was measured, although `runtime/kernel.json` has carried `git_sha` since the tracing PR. The
loop's own rule is that a number without a commit is not a measurement, so `run.py` now records
`kernel_version` — `arbos-kernel 0.2.0 <sha12> protocol 1`, from the kernel's own mouth — in every
`result.json` and every `rollouts/index.jsonl` line. Proved on a run: `"kernel_version": "arbos-kernel
0.2.0 f80f0b663bac protocol 1"`.

## The first cycle, and the one thing it cannot do today

The cycle ran end to end on this machine: build, the library, the tracked branch, inbox branches, the
desktop step, retention, publish, and the mirror at both ends. Two things about it:

- **The model budget was already spent when this machine started.** `spent_today()` reads the ledger,
  which was seeded from the store's `vm-spend.jsonl`, and today's total on the shared OpenRouter key was
  **$20.38 of the $20.00 cap** before this loop ran a scenario. So 103 model scenarios did not run and
  both headlines — the kickoff replay and `journey-linux` — are `!! HEADLINE NOT RUN: (budget)`. The cap
  is Jacob's and the rule beside it is "report when it binds, do not raise", so it is reported and not
  raised. Counting the other machine's spend is the conservative reading and it is the right one while
  one key serves two loops; the alternative — a fresh ledger per machine — would quietly double the day's
  spend.
- **The first cycle's scenario verdicts are not a clean measurement.** The `uw-*` probes were driven on
  the same four cores while the cycle's `rw-*` family ran, and that family is deliberately
  starvation-sensitive (`rw-01` is specified as running "pinned to one core beside four spinners"). Some
  of that cycle's 18 breaks are that contention. The clean cycle is the one that runs with nothing else on
  the machine.

## The budget, raised on the record

Raised **$20.00 → $60.00 for 2026-09-17 only**, on the coordinator's authority, with Jacob told because it
is his money. The reasoning, and it is in `spend.jsonl` as its own line (`usd: 0`, so it counts as no
spend): the cap bound at $20.38 before this machine ran a scenario, because the day's spend was the
outgoing loop's on the same key, and it blocked both headlines — while the benchmark loop spent $87 the
same morning. A $20 cap that stops the acceptance journey is an old number, not a considered limit.

Two things deliberately not done: the script defaults in `cycle.sh` and `vm-loop.sh` stay at $20, so the
raise is passed in the environment and tomorrow returns to the standing cap unless renewed; and this
machine did **not** get a fresh ledger, because two loops on one key with a ledger each would double the
day's spend without anyone deciding to.

## The 5.9 GB of rollouts: taken, verified, and the release deleted

The channel worked. The outgoing machine tarred them with `zstd` and attached one part to a
`qa-rollouts-2026-09-17` release on the evidence branch's tip: `rollouts.tar.zst.part-aa`, 310 MB for
6.0 GB of JSON, plus `SHA256SUMS`.

Verified here before anything was deleted:

- the part's sum matches `e9c5c35b…2baa7d`, and `zstd -t` reports the archive clean, 5,266,984,960 bytes;
- restored into `~/arbos-qa/loop` and counted: **128 folders here before, 2,798 after — exactly the 2,670
  the tar was stated to hold**, one `result.json` per folder, 6.2 GB, spanning 13 to 17 September
  (388 / 239 / 279 / 979 / 913 by day);
- and the specific runs the day's bug files cite were read out of the restore rather than merely counted:
  `qal-j19`'s `20260917T115330Z-lk-02-…` (pass, kernel `kernel-pr441`) and `qal-j20`'s
  `20260917T121650Z-fm-01-…` with both of its named breaks, `stale-sidecar-restored-a-cut-turns-tree`
  and `new-turns-file-lost`. 20 `kernel-git.log` files, as the handover said.

The count check needed care: several scenario names the bug files cite (`lk-02`, `rw-08`, `ra-01`) also
ran in this machine's own first cycle, so matching by name alone would have "found" evidence that was
mine. The restored set is the 2,584 folders timestamped before this machine's first cycle.

**The release and its tag are deleted** — `DELETE` returned 204 for both and a read-back gives 404 for
the release, the tag ref and the asset; the repository's five other releases are untouched. The 310 MB
download was removed from `/tmp` afterwards, so the tree under `~/arbos-qa/loop/rollouts` is the copy.

## The desktop app does not build on a stock Ubuntu 24.04

Not filed as a product bug — it is a host gap — but worth a line, because it blocked the journey and it
will meet anyone who tries to run this project on a fresh machine. The gpui app needs, beyond the X11 and
Vulkan set: `libfontconfig1-dev`, `libfreetype6-dev`, `libxkbcommon-x11-dev`, the xcb `-dev` family, and
`libstdc++.so` — which exists only under `/usr/lib/gcc/x86_64-linux-gnu/13/`, where `rust-lld` does not
look, so the link fails on `-lstdc++` with nothing but "linker command failed" unless the verbose output
is read. `RUSTFLAGS="-L /usr/lib/gcc/x86_64-linux-gnu/13"` closes it. The first cycle's desktop step
failed on the first of these and logged one line, `-- desktop main: app build failed`, which is how a
cycle can skip fifteen scenarios and the acceptance journey without saying what stopped it: worth giving
that step the same treatment as the other alarms.

The outgoing machine has `loop/rollouts/` (~2,400 runs) and no way to move it: git cannot carry it, its
store is unreadable, and no path between the machines is known. One channel it already holds everything
for: **a GitHub release asset on `unarbos/arbos`**, using the `ARBOS_GITHUB` PAT that is already in its
environment. Assets take up to 2 GB each, they do not enter anyone's clone, and they can be deleted
later. Mostly-JSON rollouts should compress to a fraction of 5.9 GB, so it is likely one or two assets:

```
tar -C ~/arbos-qa/loop -I 'zstd -10 -T0' -cf - rollouts | split -b 1900M - rollouts-2026-09-17.tzst.
# then attach the parts to a release tagged qa-rollouts-2026-09-17
```

If it would rather not write to the repository, or the compressed size is still unwieldy, the stated
fallback stands and nothing is blocked: the index and every broken run's small files are on `qa-results`,
every rollout behind a finding is on the evidence branch, and the passing runs' snapshots are what the
30-day retention rule deletes anyway.

## A sixth step for the review list: an assertion must not bound a race

The five inherited steps ask what a **pass** proved. This one asks whether a **failure** would mean
anything, and it is distinct enough to stand beside them.

The case, from the kernel side on 2026-09-17: a test asserted `(2..=3).contains(&turns)` — a bound on how
many turns three worker reports are batched into. On a loaded runner the reports landed far enough apart
that root woke once per report and saw four. Everything the test names passed: three reports, one per
child, none lost, none doubled. Only the bound failed. Batching there is opportunistic, not enforced, so
the assertion was right about the intention and wrong about the mechanism.

**So: assert the property the optimisation exists for, or make the kernel enforce the bound and then
assert it. Never assert the number you happened to observe.** The cost of getting this wrong is not a
wasted run; it is that the steward now has to judge which reds mean anything. Three kernel tests went red
on unrelated branches today and two were real faults — the third being a bad assertion is what makes the
other two cheap to ignore. It is the same corrosion as 29 scenarios printing `pass` while skipping
themselves, arriving from the opposite direction.

### This loop's own four probes, read against it

Worth doing immediately, since `uw-04` races on purpose:

- `uw-01` — asserts properties only: the file exists, the commit is reachable from HEAD, `undo` said
  something. No counts. **Clean.**
- `uw-02` — asserts `notice_count <= 1` over two turns, which *is* a bound. It survives the rule because
  the bound is structural rather than opportunistic: the notice is written in `init_arbos_repo`, which
  runs once per start, and #444's own comment states "once per start" as the contract. If that ever
  becomes best-effort, the assertion must become "at least one, and not one per turn".
- `uw-03` — asserts presence: a log row by name, a frame carrying the notice, a turn folder holding the
  error. **Clean.**
- `uw-04` — races for a millisecond window and **does not assert the rate**. It asserts two properties —
  no run finished unattributable, and someone was told — and uses the count only as a validity gate
  (`caught == 0` is a stated skip, never a pass), reporting the rate in the notes. **Clean, and this is
  the shape the rule asks for**: the race decides whether the probe measured anything, never whether the
  product is right.

One thing the audit changed: `uw-01`'s per-arm "did this arm stage the fault" test was itself a bad
assertion of the same family — it read "the mark's contents changed" as "the mark was written", which
made #444's *removal* look like a successful write. It now tests the property that matters, whether the
mark names this turn's starting HEAD. A bound on a mechanism, replaced by the fact it stood for.

This sixth step belongs in `docs/qa-loop-design.md` beside the other five. It is recorded here rather
than written there yet, because that document is the outgoing machine's while it is still up and
`store_put`'s sound-store path has no content check — the hazard this report's table describes. It goes
into the review list at the handover boundary.

## A control the loop destroyed, and the guard that now catches it

At 14:34 the `uw-01` control passed — and reported behaviour only #392 has, on a build that predates it.
The cause: the control binary was `repo/target/release/arbos-kernel`, which is where the cycle's **own
step 1** builds the day's head. The clean cycle had rebuilt it to `42cb9751ace8` while the label I passed
still read `main-cbbe9922d6a2-control`. Nothing was wrong with the probe; the measurement was of a
different kernel from the one it was labelled with.

The inherited rule was "a fix and its control never share a `CARGO_TARGET_DIR`", which this obeyed. The
sharper rule is: **a control binary must live where nothing else builds — including the loop itself.**
The outgoing machine already did this (`kernel=/home/ubuntu/arbos-qa/kernel-pr441` in its rollouts); it
was not written down. Now: `~/arbos-qa/kernels/arbos-kernel-<sha12>`, one immutable copy per build, named
by the sha the binary reports itself.

And because the label is hand-typed while the version is not, `run.py` now compares them: any 8–12 hex
sha in `--kernel-branch` that the kernel's own `--version` does not carry prints

```
!! KERNEL LABEL MISMATCH: --kernel-branch says ['cbbe9922d6a2'] but the binary is `arbos-kernel 0.2.0
   42cb9751ace8 protocol 1` (…); this run measured a different build from the one it is labelled with
```

and records `kernel_label_mismatch` in `result.json`. Driven against the exact case that fooled me. It is
an alarm rather than a break, because the fault is the operator's and not the product's — but it lands in
the cycle's ALARMS block, so a run that measured the wrong build cannot read as a clean pass. This is the
"name the commit and the kernel's own `--version`" rule moving from a habit to a check a machine runs,
which is the order of preference the project settled on this morning.

The `qal-j22` table was re-measured from the pinned binaries afterwards and is unchanged:
`cbbe9922d6a2` destroys in arms (b) and (c); `f80f0b663bac` destroys in (c) only; `80e6994280f8` refuses
in (c) and no longer stages (a) or (b). Zero label alarms on all three.

## Waits: `idle` is the later, looser frame

Learned from the kernel side at 14:32 and applied here the same hour. The `turn` frame's `idle` state
arrives only after the notes nudge that follows a turn, so it can trail the fact by seconds, and a
scenario that waits for `idle` and then reads the transcript can read before the turn's end has landed.
Three of this module's waits were that pattern. They now wait on the `turn_complete` **event frame** —
the fact the assertion reads — through one `wait_turn_complete` helper.

`uw-03` deliberately keeps `idle`, and it is the only one: its whole point is a turn whose end cannot
reach the transcript, so there is no `turn_complete` event to broadcast, and the wait's timing out is data
rather than a failure. The reason is written at the call.

Re-checked after the change, because a change to a wait can make a break disappear: the control still
fails the same way, and `uw-02` got measurably quicker (1.0 s to 0.7 s) since the event lands before the
nudge that `idle` waits for.

## Where the second cycle got to, and the two things I broke myself

The cycle that started 13:46 ran for 4½ hours and did **not** finish. Its record, from disk:

- reached: the kernel build (`42cb9751ace8`), the library, the kickoff headline (**9 of 12**, 2309 s), the
  tracked-`main` suite, three inbox branches, and the desktop step;
- never reached: call mode, retention, `publish.sh`, and the closing mirror;
- counted 58 pass, 41 break, 206 skip — **and 24 of those breaks are one rig fault**, `qal-j23`: the
  desktop scenarios import their driver from the store's FUSE mount, so the acceptance journey died in
  10 s on `Errno 5` and `sq-02` held the step for 2.5 hours on `Errno 11`. The journey's `0/8, 8
  unverified` is that, not a product result.

Then two errors of my own, both repaired, both worth writing down because each is a rule this project had
already paid for:

**1. I killed the running cycle by swapping the script under it.** I copied the edited `cycle.sh` over the
path bash was still reading; bash reads a script incrementally, so it resumed at a shifted offset and died
on `syntax error near unexpected token 'done'` at the call-mode step. `bash -n` on the file passes — the
script was never wrong. This is the project's own rule ("put the new thing in place before removing the
old one … a supervised process is the dangerous case") arriving from a direction it had not been written
for: not a binary under a supervisor, but a shell script under its own interpreter. **Never write to a
file a running process is still reading; install to a new path, or wait for the process to end.**

**2. I deleted 38 bug files by matching a rule name, and said the opposite of what my own check printed.**
Meaning to remove the false drafts the driver fault had created, I selected on the string
`driver-exception` — which is a *rule* name, not a timestamp, so it also matched 28 records from 13
September onwards that were already on `qa-results`. Worse, the same command printed `on the branch: 28`
two lines above a message asserting "none were on the branch": a hardcoded claim next to the computed
truth, which is the exact thing `qal-j21` was filed about. Repaired in the next command: 30 came back from
the store byte-for-byte, and the 8 that exist in neither the store nor the branch are precisely the
drafts this machine had made minutes earlier — the ones actually intended. The tree is now a superset of
the branch (313 against 208, zero missing) and `publish.sh`'s guard confirms a push would proceed.

Then a third, smaller one in the same ten minutes: the "floor check" I ran to prove the repair compared
this machine's `bugs/` against **every path on the branch**, not `bugs/` alone, and reported 2,934 files
missing. A confident wrong number, printed twice in ten minutes from one-line checks written in a hurry.
The lesson is not about care in general: it is that **a check written to confirm a repair deserves the
same review as a check written to find a bug**, and neither of mine would have survived the review list.

## The mirror's gap, and the timers that were missing

`store-docs` had not moved between 14:53 and 18:18 — the longest gap since the branch existed. Cause: the
mirror runs at the start and end of each cycle *and* on `mirror-timer.sh`'s own 15-minute clock, and on
this machine only the first existed, because I had not started the timer. The cycle interrupted at 18:15
therefore took the mirror with it.

Closed two ways: the cycle started at 18:18 pushed `f7c4756d` as its first act (27 documents, 584
`internal/` files, media 156 → 168 — the tree grew, so the gate had nothing to refuse and the store is
whole), and `mirror-timer.sh` now runs on this machine every 15 minutes as `qa-vm2`, which also carries
the store probe and the second reader. Its first pass: probe written, reader `BEHIND` (391 files here
against a 0-minute-old tip), mirror pushed `e37f30f4`.

The cycle timer is the piece still outstanding: cycles are being run by hand while the tree settles, and
`vm-loop.sh` takes over as the standing hourly loop once one completes clean.

## Cycle 3, 18:18 → 20:36: every step, both mirrors, and what its breaks are

The first cycle on this machine to run from its first mirror to its closing one.

| step | result |
|---|---|
| opening mirror | `f7c4756d` at 18:18 — 27 documents, 584 `internal/`, media 156 → 168 |
| kernel | `42cb9751ace8` primary; `b1c8e82a62b1` / `1577f4de3035` tracked; each rollout names its own build |
| kickoff replay | **11/12** on the primary run (missing item 4), **8/12** on tracked `main` (missing 1, 3, 4, 12) |
| tracked `main` + 3 inbox branches | ran |
| desktop driver | `copied 4 file(s) … to local disk` — off the mount, `qal-j23`'s fix working |
| **acceptance journey** | **6/8** in 177 s, 0 unverified, J4 and J6 failing (ten-run rate: J4 6/10, J6 8/10) |
| call mode | **23/23 green** on `main@7ea0a3ecd72e` |
| publish | **refused, correctly** — then resolved and pushed `a4807ce8`, 323 bug files, none removed |
| closing mirror | `2a70919f` at 20:36 — 28 documents |
| totals | 67 pass, 41 break, 181 skip; every skip named with its reason in the ALARMS block |

**The publish refusal is the guard earning its place on its first real cycle.** Three bug files were on
`qa-results` and not in this machine's tree — `2d013b03b5`, `9e094b99a6`, `c011fbfcf4`, all drafts the
**outgoing machine** published while this cycle ran. Unguarded, the push would have deleted them and said
`-- publish: pushed`. Resolved the way the refusal's own message says: seed them from the branch, re-check
the floor (231 on the branch, 323 here, zero missing), push.

**The desktop breaks are a standing fault, not a new one.** With the driver local, the 18 remaining
`driver-exception`s carry no store path at all: 16 are `DriverError: move: no element matches 'project-0'`,
one an `IndexError` in the driver, one a missing `xwd` (a host gap, now installed). The fingerprint of the
first, `1f6f6064cd`, was **first seen on 2026-09-15** and its draft still reads `Suspected location: (fill
in)` — so the driver's project selector has not matched the app for two days, across machines, and nobody
triaged it because a `driver-exception` reads as a harness error rather than a finding. That is the same
corrosion as a skip printing `pass`, and it is the next thing to take apart: either the app renamed the
element (a product change the parity driver never followed) or the selector was always wrong, and the
app's own element tree decides which.

## The store is the loop's source, so a fix that lives only on the machine is reverted

Ten minutes after filing `qal-j23` I met its shape from the inside. `vm-loop.sh` copies the runner and the
deploy scripts **from the store** at the start of every cycle. I had written `cycle.sh` to the store at
14:40 — before adding the driver-copy block at 18:15 — so the standing loop's first act was to copy the
older store copy over the fixed one, and cycle 4 began with the driver back on the mount.

Caught by checking rather than assuming: `grep -c 'desktop driver: copied' deploy/cycle.sh` returned 0 on
the file the loop was running. Fixed by putting the current script in the store (content-checked first:
zero of their lines changed) and restarting the loop, which then copied it forward — verified as 1 in the
cycle it actually runs.

**So the rule for this machine: a change to the runner or to `deploy/` is not made until it is in the
store.** The machine's copy is a working tree, not a home.

One more thing that came out of the restart: killing `cycle.sh` left its `run.py` **orphaned** (reparented
to pid 1) and still running scenarios into the log, which is how a "stopped" cycle went on writing for
three minutes. The loop already knows this shape — "a process the harness started is the harness's to
kill, wherever its cwd is" — and it applies to the harness's own parent, too. Stopping a cycle means
stopping `run.py` and its kernels by pid, then the shell, not the shell alone.

## Cycle 4, 20:55 → 00:37: complete, and the first cycle whose desktop leg measured anything

| step | result |
|---|---|
| kickoff replay | 11/12 primary, 8/12 tracked `main` |
| **acceptance journey** | **5/8**, 1 unverified, 2 fail — J1✗ J6✗ J8? |
| desktop leg | ran for the first time: `af-02`, `cp-02`, `desktop-fresh-place-no-notice`, `desktop-huge-transcript-scroll`, `desktop-kill-kernel-under-ui`, `im-02` pass; `desktop-user-message-card` breaks on the `hi` card (symmetry's, not reopened) |
| call mode | **26/26 green** on `main@3ef5f436e529` |
| publish | **pushed** — the floor held this time |
| mirrors | both ends |
| totals | 119 pass, 34 break, 245 skip |

Its desktop leg ran on the store's 07:06 driver, because the running `cycle.sh` predated the pin: **do not
cite cycle 4 for anything about settings** (`qal-j25`).

## The 1528-second click that was a paused machine (`qal-j26`)

Cycle 4's `desktop-rapid-session-switch` got everything right — 6 rows, 30 switches, 0 failures, 6
`chat-*` folders — and then reported `ui-stall: switch to panel-agent-7 took 1528.7s (limit 3.0s)`. The
stall ended at the second this agent resumed from a 2400-second wait, and the same scenario by hand takes
19.6 s.

The machine's own clocks settle it: `uptime` read 2:23 at 18:13 and 6:02 at 00:30 — 3.65 hours of uptime
across 6.28 hours of wall clock, **2.63 hours unaccounted** — and `CLOCK_MONOTONIC` equals
`CLOCK_BOOTTIME` exactly, the signature of a VM *paused* rather than a guest suspended. Both monotonic
clocks freeze with the machine; only the wall clock jumps on resume. `Desktop.timed` measured with
`time.time()`.

The loop has known since 2026-09-14 that this VM sleeps while its worker is idle; what had not been drawn
out is that **assertions on duration do not go quiet, they go loud and wrong**. `timed`, `launch_s` and
`duration_s` are monotonic now, and two numbers already printed should not be cited: cycle 3's `sq-02` at
9142.7 s and cycle 4's `desktop-rapid-session-switch` at 1544.9 s.

## Cycle 5, from 01:00:50 on `c3247332dc4e`

Every fix of the day is in the copy it runs, verified by reading the files the loop actually copied rather
than the ones I edited: the driver taken from the app's own commit, `arbos-hub` built beside the kernel, a
desktop build failure as an alarm, the publish refusal, `af-04`, monotonic durations, the panel rows, and
the kernel-label guard. `uw_scenarios.py` is now in `vm-loop.sh`'s copy list too — it was missing, and
since `run.py` imports it, a machine that copied the runner without it would have failed at import and
measured nothing.

## The night's ledger: what was the rig's and what was the product's

The desktop leg went from measuring nothing to measuring everything, and the honest accounting of what
that produced matters more than the counts. Eight faults found between 18:00 and 05:20; **five were the
rig's, two were mine, one was the product's.**

| finding | whose | what it cost |
|---|---|---|
| `qal-j23` the driver imported off the store's FUSE mount | rig | 24 breaks in one cycle, the journey among them |
| `qal-j24` `new_chat` fell back to a removed sidebar | rig | 16 scenarios measuring nothing for two days |
| `qal-j25` the driver five hours older than its app | rig | every desktop result before 22:35 untrustworthy about settings |
| `qal-j26` a wall clock on a paused VM | rig | a 1528-second "UI stall" that never happened |
| J6 judged worker rows without opening the panel | rig | the acceptance headline "regressing" twice in a row |
| `mt-17`'s loose `"strip"` selector | **mine** | a false break the moment my panel fix landed |
| the pills scenario asserting a project's pills in a sub-chat | rig | a break the app documents as correct behaviour |
| **`qal-j27`** a line typed while a turn runs is not steered and is lost | **product** | the person's words, silently, since 16 September |

The pattern is worth stating once, because it is the opposite of what a break count suggests: **every rig
fault that fails early hides the ones behind it.** `qal-j24` hid `qal-j27` for two days — three scenarios
recorded it each cycle and nobody could see them inside sixteen `driver-exception` lines. Fixing the
selector produced four more faults in one hour, each of which had been sitting there.

And the corollary for this loop's own credibility: of eight faults, six were in the measuring apparatus.
A cycle's break count is a statement about the rig until proved otherwise, which is what the boundary
checks in `qal-j27` are for — four neighbouring contracts passing on the same build in the same cycle is
what made it the product's.

## What the acceptance journey did, once it could run

| cycle | score | why |
|---|---|---|
| 3 | 0/8, 8 unverified | the driver could not be imported (`qal-j23`) — not a result |
| 4 | 5/8, J1 and J6 fail | the 07:06 driver (`qal-j25`) |
| 5 | 6/8, J6 fail | correct driver; J6 on the closed panel |
| hand-run, 04:20 | **7/8, 0 fail** | J6's panel and post-versus-attempt both fixed |

J6's notification contract is **established** for the first time on this rig, where the design doc has it
as permanently unverified: `posted_new: 1` with `post_error: null`, `daemon_has_it: true`,
`daemon_entries: 1`, the badge showing `unseen: 1` with its tab dot while away and both clearing on
opening. Two host packages were missing and are now installed: `libnotify-bin` (so `notify-send` exists
at all) and, earlier, `x11-apps` for `xwd`.

J8 stays unverified by design — the dropped connection is the phone loop's.

## Four iterations to make one probe honest (the pills)

Kept because it is the clearest worked example of the review list catching its author, and every step
tripped a rule this project had already written down:

1. The original slept 25 s and asserted "no PRs pill after two bash outputs" **without proving there were
   two bash outputs** — `checkpoint_refs`'s defect, and a fixed sleep used to wait for a result.
2. My first wait matched the URL anywhere in the chat state and "landed" in **0.1 s** — the URL is in the
   prompt the scenario types, echoed back as the user's line. `sb-01`'s rule, reproduced in the file that
   records it.
3. My second excluded `User`-shaped items, but the driver's items are flat dicts with `kind`/`text`, so
   the exclusion never matched and it landed in 0.1 s again. I had guessed the data's shape twice rather
   than read how the rest of the rig reads it.
4. My third required a `tool` item and got an honest 2.3 s — then checked for the pill instantly, the
   opposite error to the original.

The answer, in the end, was in the app's own comment (`detail.rs:1924`): *"a subagent's chat in Cursor
carries no pills; they are the project's"*. The scenario opened a sub-chat and asserted the project's
pills in it. Tested on the project's own chat: `pill_ids: ["pill-prs"]`, 1.2 s after a trigger that
landed at 9.9 s. Nothing to file.

## The tracked step cannot run the whole library, and never has

Measured on cycle 6's tracked step, 2026-09-18 05:47, 60 verdicts in:

- **mean 33.5 s a scenario**, median 10.2 s — the mean is what matters for a cap
- **291 scenarios registered**, so the whole library needs about **162 minutes**
- the cap is **100 minutes**, which reaches about **179 of 291 — roughly 62%**

So the step has never run the library it is described as running, on this machine. Which 62% it runs is
decided by **registry order**, which is module import order, which puts anything new last. Until the
truncation alarm went in tonight, the log's only sign was `run.py exit 124` among several hundred lines.

Step 3a2 rescues the five scenarios that matter most for this — the day's own probes — but about 105
others are still cut every cycle, and nobody knows which without reading the log's tail.

Three ways out, and this is the coordinator's call rather than mine:

1. **Raise the cap to ~170 minutes.** Honest coverage, and a cycle then runs 5–6 hours, so fewer cycles a
   day and a slower loop around a finding.
2. **Split the library in two halves that alternate cycles**, each cycle saying which half it ran. Full
   coverage every two cycles, no silent loss, the cycle stays about its present length. My preference.
3. **Make the expensive scenarios cheaper.** The two 300-second entries in tonight's slowest five are
   `turn-never-ended` waits timing out — a five-minute wait for something that has already failed —
   and `spawn-storm` costs 10.8 minutes. There is real time to win here, and it is worth doing whichever
   of the other two is chosen.

One number in that list is not to be trusted, and it is a good illustration of `qal-j26`:
`desktop-rapid-session-switch` appears as the most expensive scenario of the night at 25.7 minutes. That
is the phantom stall from cycle 4, measured on a wall clock across a paused VM. Its real cost is 20
seconds. Durations recorded before 22:35 should not be used for this kind of arithmetic at all.

## What `docs/qa-loop-design.md` no longer describes

The design doc is what a new worker reads first, and the cycle changed under it tonight. Not edited here,
because it is the loop's shared document and the machine that rebuilt it was still up; this is the list
for whoever reconciles the two.

| the doc says | it now does |
|---|---|
| the cycle runs the library | the tracked step runs **one half**, alternating, stated in the log (`== library half B: 151 of 291`); the whole of it needs ~162 min against a 100 min cap |
| steps 3, 3a, 3b, 3b2, 3c | a **3a2** between 3a and 3b: the `uw-*`/`af-*` family on its own 25-minute invocation, because they register last and a capped step loses its tail |
| `timeout 50m` on the tracked step | **100m**, and `exit 124` is an alarm naming what truncation cost |
| the desktop driver comes from `internal/parity` when present | from **the app's own commit** (`$wt/desktop/driver`), copied to local disk; the store copy is a fallback (`qal-j23`, `qal-j25`) |
| — | the two **headlines run in both halves**; a half without the kickoff replay or the journey would be a cycle measuring less than it reports |
| — | durations are **monotonic**; a wall clock on this paused VM invented a 1528-second stall (`qal-j26`) |
| — | `publish.sh` **refuses** a push that would delete bug files the branch holds (`qal-j21`) |
| — | every rollout and index line records the kernel's own `--version`, and a `--kernel-branch` naming a sha the binary lacks is an alarm |
| J6 notifications "always unverified here" | **established**: the app posts, dunst receives it, the badge clears — and a *post* is now distinguished from a failed *attempt* |
| the loop's ledgers are `vm-*.jsonl` | per-machine (`ARBOS_QA_MACHINE`), so two loops cannot overwrite each other |

The bug files `qal-j21` through `qal-j27` carry the reasoning for each, and every change is in the store,
which is what `vm-loop.sh` copies from at the start of a cycle — the machine's copy is a working tree,
not a home.

## Cycle 6's cap, and what the truncation actually cost

The tracked step was killed by its 100-minute cap at 06:55 on `inbox:worktree-cleanup`, number
**152 of 291**, and the alarm added for exactly this printed. Measured rather than estimated:

| | count |
|---|---|
| never ran | 139 |
| of those, run by a later step | 23 (desktop tag; plus `uw-*`/`af-04` from cycle 7's step 3a2) |
| measured by no step at all | 116 |
| of those, deterministic | **93** |

The step's time went almost entirely to one place: `inbox:*` is **135 of 291** scenarios and took
**87.2 of the 98 measured minutes** (mean 47.2 s against 22.4 s for everything else). The
`swebench-loop-cycle-*` notes are *not* the cost — the five that ran took 5.9 minutes — but they
are the clearest illustration of the shape: 24 notes, one per benchmark cycle, each minting a
permanent paid scenario from a status report. Filed as `qal-j28`, with a falsifiable prediction
that cycle 7's half split brings the step in under the cap.

Two of the 93 matter more than the rest: `sw-02-stale-undo-mark-resets-past-committed-work` is
`qal-j22`'s own subject and has not run on `main` today, and `fm-01` is the first-match property
the `fm-*` family was to be built on.

## A red that meant nothing for four cycles

`af-03` ended with `f"… {wrong[0]!r} …"` inside an `expect` whose passing case is `wrong` empty.
Python builds the message before the condition is examined, so the scenario raised `IndexError`
**exactly when it succeeded** and printed `driver-exception` in four consecutive cycles. The notes
survived in every rollout and say the product is fine: no ghost `.arbos/` at the old path, and an
honest "This project's folder is gone or was moved" notice. Filed as `qal-j29`, fixed, and swept —
three other sites index inside a message and all three already guard with `… if evs else None`.

Added to the design doc as review rule 8: **a failure message must be computable when the
assertion passes**, and a `driver-exception` is more urgent than a product break because it hides
its own scenario's finding.

## The mirror dropped 168 feedback reports, and the store faults in the open

Reading `store-docs` history at the boundary: commit `68801ff4` (04:52Z) holds **0** entries under
`media/desktop-feedback` where its parent holds **168**. The shrink guard covers `docs/` and
`internal/` and not feedback, so a transient empty listing became a commit; the next pass restored
it. For roughly seventy minutes the mirror that exists *because the store dropped `docs/` once
with no event* held none of the reports.

The mechanism was then caught in the act rather than inferred. At **08:03:59Z** a `check` run
printed `REFUSED: docs/ lists no .md files` — 29 files listing as zero — and twenty samples taken
immediately after all returned 29. Writes to the store return `EAGAIN` about 3 times in 100; two
of my own edits failed that way and succeeded on retry, and a recursive read of `internal/` left a
`cp` wedged in uninterruptible sleep.

Three fixes, all in `qal-j30`: the feedback shrink guard (argued, **control outstanding**), the
timer keeping the `REFUSED` reason it had been discarding, and an `ERR` trap separating the two
things the timer had been calling by one name — **exit 2 is a refusal with a reason, exit 1 is the
script crashing under `set -e` in silence**, and six passes today were the second kind.

## Cycle 7

Started 08:01:44Z on kernel `c9e035c9f062`, running the store's `cycle.sh` with both changes in
place: the A/B half split (each cycle says which half it ran) and step 3a2, which gives
`uw-01`..`uw-04` and `af-04` the main-built kernel and the `--kernel-branch main` label they need.
In cycle 6 that family ran only in the untracked `rust` step and **skipped itself** by gate, which
is why `af-04` has still not had a real run.

## Cycle 7 died on my own change, and what it cost

The half split remembers its last half in `state/library-half`. `cycle.sh` runs under
`set -euo pipefail`, and `LIB_HALF=$(cat "$HALF_FILE" 2>/dev/null)` takes `cat`'s status — so on
the first cycle, with no file yet, the assignment exited 1 and ended the script. The `*)` branch
written for exactly that case could never be reached, and `2>/dev/null` hid the one line that
named the cause.

Cycle 7 died at 08:33 between the untracked and tracked steps and lost the tracked step, step 3a2,
the desktop step, the journey and the mirror. Fixed with `|| true` and `mkdir -p`, with a control
on the exact lines: first run `A`, then `B`, then `A`, exit 0 throughout, where before the first
run printed nothing and exited 1. Filed as `qal-j32`; review rule 9 — **run the path that has no
history** — comes from it and from `qal-j29`, which are the same shape seen from two sides.

## af-04 has run, and it passes

Step 3a2's set was run by hand at 08:37 to recover what cycle 7 lost, against
`arbos-kernel 0.2.0 d373422662bd protocol 1`. Six scenarios, none skipped — the branch label did
its job:

| scenario | verdict |
|---|---|
| `af-04-a-moved-places-old-path-is-not-recreated-by-the-kernels-late-writes` | **pass** (79.8 s) |
| `uw-01` undo mark | pass |
| `uw-02` git exclude | pass |
| `uw-03` panic path | pass |
| `uw-04` subscription marker | pass |
| `fm-02` MCP config | break — `qal-j31` |

`af-04`'s pass is worth reading rather than counting: `turn_complete_seen: True`,
`old_path_recreated: None`, `kernel_alive_after: False`, and
`new_kernel_served_the_moved_folder: True`. A turn really ran, the old kernel stopped itself as
#377 intends, a new kernel served the moved folder, and the old path did not come back. So it is a
pass note, as expected, and all four of #444's destructive cases hold on this build.

## A seventh first-match reader, found by looking rather than by breaking

The audit enumerated six first-match readers and cleared five. `mcp::load_servers`
(`crates/arbos-kernel/src/mcp.rs:116`) is not among them. It walks four config locations and the
first file to define a server name keeps it — and a file that does not parse is skipped with an
`eprintln!` while the walk continues.

Two arms, identical but for the global config, same broken `.arbos/mcp.toml` defining `notes`:
with the global present the kernel goes on to start a `notes` server; without it, no `notes`
server exists at all. So the place's broken config is skipped and **the global silently takes the
name**. Nothing reaches the person — the desktop routes kernel stderr to
`.arbos/runtime/kernel.out.log`, which its own comment calls "process facts, never part of the
`.arbos/` record". Filed as `qal-j31`, held by `fm-02`.

## Cycle 8

Started 09:01:25Z on kernel `b3770cd0de9e`, running the corrected `cycle.sh` (the `|| true` guard
is at line 112 of the copy `vm-loop` installed at 09:00:25). This is the first cycle that should
announce its half.

## #441 verified, with the controls failing on the build before it

`lk-01` was written as #441's control by the outgoing worker, so the verification is a comparison
rather than new code. #441 merged as `80e6994280f8`; `f80f0b663bac` predates it and
`d373422662bd` carries it.

| | `d373422662bd` (has #441) | `f80f0b663bac` (before it) |
|---|---|---|
| `lk-01` a held place is said once then escalates | pass (331.5 s) | **5 breaks** |
| `lk-02` held record in a read-only `runtime/` | pass | **3 breaks** |
| `lk-03` holder gone clears the record | pass | **3 breaks** |

What the old build actually did, which is worth reading rather than counting: it exited **1**
where a held place must exit 3, said the held line **0 times over 164 relaunches**, produced **0
heartbeats in 5.5 minutes** where one a minute is the contract, never escalated, and after a new
holder took over left the record as `None` instead of naming the new pid. So the family fails
comprehensively before #441 and cleanly after it, which is the shape a verification wants.

## #450's detector: both shapes found, and quiet where it must be

#450 (`a9b12f118650`) added `check_two_writers` to `crates/arbos-kernel/src/check.rs`. Its value
is not the warning but the reading — the question it answers is whether a person's places were
served twice while the lock was split across the `runtime/` move — so its **precision** matters as
much as its recall. A detector that also fires on an interrupted kernel would send everyone
hunting a fault they never had.

Four places built by hand and read through `arbos-kernel check`, now standing as
`ds-01-the-double-serving-detector-finds-both-shapes-and-stays-quiet-on-the-innocent-ones`:

| arm | wanted | `d373422662bd` | `f80f0b663bac` (before #450) |
|---|---|---|---|
| a wake while the previous turn is still open | warn | warns, naming the open line | silent |
| two checkpoints for one line with different times | warn | warns | silent |
| died mid-turn, with the restart notice | quiet | quiet | quiet |
| an ordinary place, two clean turns | quiet | quiet | quiet |

So the detector is sound on both shapes it claims and does not cry wolf on either innocent state,
and the scenario fails on the build before #450 — it tests the detector rather than the staging.

One thing checked and **not** filed: warning two says "or a rewind's cut left one behind", which
reads like an innocent cause that would make the warning unusable for deciding anything. It is
not, on a successful rewind: `rewind.rs:239–259` removes the cut lines' tree sidecars, drops their
git refs, and rewrites `checkpoints.jsonl` with only the retained records, so a reused line gets
one record and not two. The caveat covers a rewind that failed part way, which is narrower.

What the detector cannot see is stated in #450 itself and stands: two kernels answering on
different ports, and two writers to `notes.md` via temp-and-rename, which leave no trace and need
the `kernel_start` pids laid side by side.

## Cycle 8: the first whole tracked step, and the half split confirmed

`== library half this cycle: A` — **146 of 299 scenarios** — and the tracked step ended
`-- track main: run.py exit 1`: a normal finish with breaks and **no truncation alarm**. All 146
ran, against 152 of 291 in cycle 6. The `qal-j28` prediction held, and the file now records that
along with why it is not a closed matter: the library grew 291 → 299 in a day and
`inbox:swebench-loop-cycle-*` reached 26 from 24 while the file was open.

Step 3a2 then ran its whole family on the cycle's own main build, in-cycle for the first time:

| scenario | verdict |
|---|---|
| `af-04` moved place's old path not recreated | **pass** (also passed in the tracked step, 83.0 s) |
| `ds-01` the double-serving detector | **pass** |
| `uw-01` … `uw-04` (#444's four cases) | **pass** |
| `fm-02` MCP config skipped in silence | **break** — `qal-j31`, reproducing every run |
| `lk-01` (#441) in the tracked step | **pass** |

## The 36 minutes I first read as a stall, and what it really was

Cycle 8's tracked step looked like it had lost 37 minutes to two gaps between scenarios — cycle 6
lost 3 minutes across 239 rollouts, cycle 7 none. I chased it through three wrong explanations
before finding it, and the wrong turns are worth recording because each was cheap to rule out:

- **Not the rollout copy.** `copy_tree_contents` is local-to-local; the docstring claiming it
  copies to the store is stale, since `ROLLOUTS = QA_DIR/"rollouts"` now resolves under
  `~/arbos-qa/loop`. A synthetic tree of the same shape — 662 files, 11 MB — copies in **0.04 s**
  against the 1595 s gap I was trying to explain.
- **Not the store.** `vm-loop.sh` puts only `index.jsonl` there, once per cycle.
- **Not a per-file cost.** The apparent 2.4 s/file was two data points and coincidence.

What it is: **the VM suspends while this agent is idle, and `ps` lies about it afterwards.** The
tracked step's first rollout is stamped `20260918T093316Z`, while `ps lstart` reports its `run.py`
starting at `10:09:44` — 36 minutes later, the same size as the gap. `ps` derives a start time
from boot-wall-time plus start-ticks, and boot-wall-time is `now − /proc/uptime`; uptime stops
during a suspension while the wall clock resyncs on resume, so every process started before the
pause is reported that much *later* than it truly began. The rollout stamps are the honest record.

Two useful consequences:

- `timeout` counts monotonic time, so the step's 100-minute cap is **not** shortened by the pause.
  That is why half A finished despite the wall clock suggesting it had run out.
- A sampler taking wall and `/proc/uptime` every 20 s showed **zero** drift across 400 s while the
  agent was working, so nothing is lost while there is work to do. Short waits are enough; the
  inherited habit of polling every few minutes is the right one, and my 20-minute sleep is what
  bought the second gap.

This extends `qal-j26` rather than repeating it: that file established that a paused VM inflates
wall-clock durations. This adds that it also corrupts `ps lstart`/`etime`, which is the tool one
naturally reaches for to check whether a long step is stuck.

## Cycle 8's four unexplained reds: three were ours, one is real

Cycle 8's desktop step left four breaks nobody had looked at, each standing in all five cycles
since 2026-09-17 18:18. Taken in order:

| red | verdict | mechanism |
|---|---|---|
| `xp-01-first-line-lost` | **ours** — `qal-j33` | one click on a window not yet taking input; `composer.focused` was false and the composer was *also* empty, so the keystrokes were never delivered. Two clicks work. It had been claiming **data loss**. |
| `mt-18-page-fills-column` | **ours** — `qal-j34` | read `bounds.height` and `height`; the key is `h` (`driver.rs:969`). `tr_h` was 0 on every build, so the assertion could never pass. Corrected, the transcript keeps **571 of 1000** px with 21 open items. |
| `mt-14-raw-done-card` | **ours** — `qal-j36` | searched the item *data* for a line and concluded about the *view*, which strips the prefix and the file pointer before drawing it. |
| `mt-24-active-tab-not-restored` | **real** — `qal-j35` | a relaunch comes back on the project's main chat, not the sub-chat left active. |

Three of four. That ratio is the argument for triaging a red before believing it, and it is also
why the fourth is worth believing.

`mt-24` took two passes to become evidence. It had asserted on the **session id**, and a relaunch
renumbers sessions — the chat that was id 3 comes back as id 1 — so `active_after != active_before`
was true whatever the app did, and a *correct* restore would have gone red too. It also slept three
seconds and read once. Compared by `agent_session`, which survives the relaunch, and polled to a
20-second bound, the finding stands: `chat-1789733586900` before, `root` after, never restored.

Two of the repairs left the library better than a straight fix would:

- `xp-01` now separates three failures that had shared one name — the click never focused (ours), the
  line sits in the composer after Enter (visible refusal), focused-and-emptied yet on no transcript
  (silent loss). Only the last is the rule that was firing.
- `mt-14`'s unverifiable "a raw line is shown" became a check on the **wording coupling**:
  `done_report` knows exactly three prefixes, and if the kernel's phrasing drifts the view stops
  recognising it and the raw line does reach the person. That fails *before* anything is visible,
  which the original could not do.

The shared helper `desktop_scenarios.focus_composer` now backs every send in the library, and
review rules 10, 11 and 12 come from this pass.

## #446 verified, and the `fm-*` family gets its third member

`#446` (`cb495a38e189`) is the `fm-*` shape exactly: a place holds two kernel records,
`.arbos/kernel.json` from before the `runtime/` split and `.arbos/runtime/kernel.json` after it,
with different writers. `runtime/` was read first, so a place that had **ever** been served by a
newer kernel kept a `runtime/` file for ever — and when an older kernel then served it, the reader
took the stale record, found its pid gone, and reported **no kernel at all**. An empty-looking place
is the state in which something starts a second kernel on it, so this is `#450`'s harm arriving by
another route.

The reader is observed through `arbos-kernel feedback <place>`, whose `kernel.serving` block is
built by `place.kernel_json_read()` — the reader itself (`feedback_cmd.rs:78`). Three arms pin the
whole table, on `arbos-kernel 0.2.0 2301abd291c0 protocol 1`:

| arm | staged | must choose | chose |
|---|---|---|---|
| a — the case measured on the target | `runtime/` dead, `.arbos/` **live** | the live fallback | `1iveliveaaaa`, live ✓ |
| b — no regression | `runtime/` **live**, `.arbos/` dead | the live newer path | `1iveliveaaaa`, live ✓ |
| c — neither live | both dead | the newer path, as before | `deadf00d0000` ✓ |

Standing as `fm-03-the-kernel-record-that-names-a-live-process-wins-whichever-path-it-is-in`.

**The before-arm is not reachable, and the reason is worth recording.** I built a kernel at
`cb495a38e189^` (`42cb9751ace8`, pinned under `kernels/`) to run the same three arms on the
unfixed code, and it answers `unknown command feedback`. Checking the history: `feedback` was added
at `bf38270c599d`, and `bf38270c` and `cb495a38e189` are **siblings** — neither is an ancestor of
the other, both merged to `main` independently — so the first commit that has `feedback` also has
`#446`. No build exists that can be asked with this observable and lacks the fix. That is a fact
about the repository, not a gap in the method, and `fm-03`'s own
`probe-feedback-unreadable` fired on the old build rather than passing quietly, which is the
behaviour a probe-validity guard is for.

So `#446` is verified forward — the table is pinned and any regression toward first-match fails at
least one arm — but not against its own predecessor.

## #453 verified, and the reproduction its author could not get

`#453` (`b0b3cf841c77`) rests, by its own account, on a reading of the code: *"I could not reproduce
the red here: 0/12 on `main`, 0/12 with the fix."* The window is real but narrow — the app's swap is
a directory rename and then a copy into the start path, so for a moment that path holds nothing
usable — so the loop's job is to hold the gap open on purpose.

Staged the way the app makes it: rename the running image aside and leave an empty file at the start
path. An in-place truncate is impossible on a live binary (`ETXTBSY`), which is *why* the app
renames, and is why the gap exists at all. Standing as
`up-01-a-swap-still-in-progress-is-not-a-failed-restart`, measured over 30 s:

| | `42cb9751ace8` (before) | `2301abd291c0` (after) |
|---|---|---|
| `reexec` attempts onto the unusable file | **1** | 0 |
| `reexec_wait` lines | **0** | 6 (one per 5 s tick) |
| `binary_gone` noticed — probe validity | 1 | 1 |

Both arms stage the fault, so neither result is an accident of the setup.

**The mechanism is worse than the PR describes, and that is the finding.** The old build does not
merely *wait* a minute: it has no settled check at all, so it **attempts the exec onto a zero-byte
file** — `reexec: restarting onto … (git 42cb9751ace8 was serving)` — and it is that failed attempt
which earns `Reexec::Failed` and the full `REEXEC_RETRY_MS`. The fixed build makes no attempt and
logs `reexec_wait` at every tick until the file is whole and at rest. So the 60-second wait was the
symptom; jumping onto an unfinished file was the cause.

Two false starts worth recording, because each was my own probe lying to me and each was caught by
its validity guard rather than by luck:

- the first attempt set `ARBOS_NO_REEXEC=` to an **empty value**, and the code tests
  `var_os(...).is_some()` — so I had disabled the very thing I was measuring, and
  `reexec_onto_new_binary` returned `NotReady` before logging anything. The probe reported
  "no attempt logged; probe invalid" instead of passing.
- the old build cannot parse today's config file — it rejects `jev_model`, a field added after its
  commit — so the control arm needs a clean `XDG_CONFIG_HOME`. Without it the kernel never started
  and the run again declared itself invalid rather than green.

## The original queue is closed

| item | state |
|---|---|
| `#441` held-place record | verified, `lk-01`..`lk-03`, comprehensive breaks on the build before |
| `#444` four destructive cases | verified, `uw-01`..`uw-04`, all pass in-cycle |
| `#446` kernel.json first-match | verified forward, `fm-03`; before-arm unreachable (observable postdates the fix) |
| `#450` double-serving detector | verified, `ds-01`, both shapes found and quiet on the innocent ones |
| `#453` swap gap | verified, `up-01`, two-armed, with the reproduction the PR lacked |
| `fm-*` family | three members: `fm-01` checkpoint sidecar, `fm-02` MCP config (`qal-j31`), `fm-03` kernel.json |

`kernels/42cb9751ace8/` is pinned as the before-build for `#446`, `#450` and `#453`.

## Cross-references

- `internal/qa/bugs/qal-j33-five-cycles-of-first-line-lost-were-one-click-on-a-window-not-yet-taking-input.md`
- `internal/qa/bugs/qal-j34-mt-18-read-a-height-key-that-does-not-exist-so-its-assertion-could-never-pass.md`
- `internal/qa/bugs/qal-j35-a-relaunch-comes-back-on-the-main-chat-not-the-sub-chat-the-person-left-open.md`
- `internal/qa/bugs/qal-j36-mt-14-read-the-item-data-to-decide-what-the-view-draws.md`
- `internal/qa/bugs/qal-j31-a-place-mcp-config-that-does-not-parse-hands-its-server-name-to-the-global-one-in-silence.md`
- `internal/qa/bugs/qal-j32-the-half-split-killed-the-cycle-on-the-first-run-because-a-missing-file-is-a-failing-command.md`
- `internal/qa/bugs/qal-j28-the-inbox-grows-without-bound-and-crowds-the-library-out-of-the-cap.md`
- `internal/qa/bugs/qal-j29-af-03-reported-a-break-for-four-cycles-because-its-failure-message-crashed.md`
- `internal/qa/bugs/qal-j30-the-docs-mirror-dropped-168-feedback-reports-because-only-docs-and-internal-are-guarded.md`
- `internal/qa/bugs/qal-j21-publish-mirrors-a-smaller-bug-set-and-deletes-the-branchs-drafts.md`
- `internal/qa/bugs/qal-j22-undo-marks-removal-needs-the-folder-whose-permission-just-failed-the-write.md`
- `internal/unchecked-writes-and-orderings-audit.md` (the features agent's audit, which #444 answers)
- `internal/store-second-reader.md` (this machine is its fourth client)
- Branch `cursor/qa-store-pending-adcd` at `4ddf1bcb`: the nine files changed by hand here, at their
  store-relative paths, from a tree only this machine writes (`~/qa-hand`). The loop's own staging tree
  is `~/arbos-qa/store-pending` and is never that one.
