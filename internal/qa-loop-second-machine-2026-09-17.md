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

## The 5.9 GB of rollouts: one channel worth trying

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

## Cross-references

- `internal/qa/bugs/qal-j21-publish-mirrors-a-smaller-bug-set-and-deletes-the-branchs-drafts.md`
- `internal/qa/bugs/qal-j22-undo-marks-removal-needs-the-folder-whose-permission-just-failed-the-write.md`
- `internal/unchecked-writes-and-orderings-audit.md` (the features agent's audit, which #444 answers)
- `internal/store-second-reader.md` (this machine is its fourth client)
- Branch `cursor/qa-store-pending-adcd` at `4ddf1bcb`: the nine files changed by hand here, at their
  store-relative paths, from a tree only this machine writes (`~/qa-hand`). The loop's own staging tree
  is `~/arbos-qa/store-pending` and is never that one.
