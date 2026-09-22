---
cursor:
  subagentId: "bc-b4f4cdba-0146-5dea-9731-24ea2538adcd"
---

# QA loop keeper — running status (2026-09-19)

For folding into the store's `notes.md`. I do not write there directly; this is the loop file.
Updated as cycles close.

## Where the loop is

| | |
|---|---|
| current cycle | **STOPPED** — every background process ended, 2026-09-21 12:30Z |
| previous | 20 closed 04:46:40Z (half A), 276; 19, 298; 18, 272 |
| next | cycle 22 due 09:00, **half A** — `lk-04` returns |

| clean run | cycles 15-21: 0 checkpoint noise, 0 budget skips; cycle 21's step 1 truncated (normal — see below) |
| health this cycle | 0 `state:checkpoint` noise, 0 budget skips |
| mirror | pushing on its ~15 min cadence |
| modules | store and live loop in sync |

**The loop only advances while the agent is present.** The VM suspends when I am idle, so a cycle
takes far longer in wall clock than in work — cycle 15 was 4.5 hours of clock for ~90 minutes of
work. Nothing is wrong with it; it does not run unattended. This is also what decides whether a
step truncates (`qal-j28`).

## Open bugs I hold

| id | what | state |
|---|---|---|
| `qal-j39` | consistency checker validated a retired engine | fixed in rig: subscription **and** checkpoint rules, both with controls |
| `qal-j40` | deleting both lock files lets a second kernel serve one place | **open (product)**; `lk-04` asks it on **half-A cycles only** (3.1 s) |
| `qal-j42` | the new-chat control moved a fourth time | ⌘N fix stands |
| `qal-j43` | a chat opened during kickoff silently eats what you type | **open (product), live**; regressed by `2ea8d565` |
| `qal-j46` | the daily budget runs out ~22:00 and the loss lands on the desktop step | open (loop design) |
| `qal-j48` | the journey pass rate counts "not measured" as "failed" | open (loop reporting), stays in QA |
| `qal-j49` | the reaper logs to a folder `finalize()` deleted; the crash ends the step | **fixed in rig** 2026-09-20 |

Closed or settled: `qal-j35` (fixed by #679), `qal-j45` (provider, not the product), `qal-j47`
(the scenario, not the queue — fixed in the rig), `qal-j28` (settled: suspension), `qal-j44`
(rig re-seeding, fixed).

## `qal-j43`, scored by build

The fault is a **rate**, so it is only meaningful within one app build:

| app build | conclusive runs lost |
|---|---|
| `01b32cd5` (its parent) | 0 of 6 |
| `2ea8d565` (the regression) | 5 of 6 |
| `1b4ef7a9` | 3 of 3 |
| `8af86842` | 5 of 5 |
| `249ddb5f` | live (full signature) |
| `443ffdc55934` | live (full signature) |
| `a129992316e9` | live (full signature) |
| `cbb2907a5a7b` | live (full signature) |
| `1647b90a8cfa` | live (full signature) |
| `dcf8313cf00b` | live (full signature) |

Full signature each time: `kf-01`, `mt-01`, `mt-04`, `dg-01` break together.

## Not to be sent to features

`qal-j35` and `qal-j47` — both resolved, neither is theirs. `qal-j48` and `qal-j28` stay in QA.
`steer-order` is left alone.

## The pattern worth folding in

Three separate places recorded "could not ask" as an answer, and each one hid something:

- `kf-01` recorded a missed window as a **pass** — now retries, then records `inconclusive`;
- `sq-02` recorded a staging failure as a **skip** — now retries, then breaks;
- the journey rate records an unrun step as a **failure** — `qal-j48`, unfixed by choice.

> Three outcomes, never two: it passed, it failed, or it was not asked. A summary that folds the
> third into either of the others misleads exactly when the rig or the budget has gone wrong,
> because that is when the third case happens.

A fourth instance of the same shape turned up in `dg-01`, which watched `any(...)` session and so
was satisfied by root's kickoff — the very red herring `qal-j43` documents. Fixed 13:40; the
warning had been in that bug file long before the check stopped walking into it.

## Which cycles a guard runs in is decided by its name

Corrected 2026-09-19 17:30, after `lk-04` was missing from cycle 17 and I went looking for a fault.

The tracked step passes `--half`, and the half is `sha1(name) % 2` (`run.py:2222`). So a scenario's
cadence follows its **name**, invisibly:

```
half A   lk-01, lk-04, kf-01, fm-01
half B   lk-02, lk-03
```

`lk-04` is half A, so `qal-j40` is asked every *other* cycle. The main step cannot fill the gap —
it runs `--kernel-branch rust` and every `lk-*` is gated to `main`, so they all skip there.

`kf-01` is half A too but runs **every** cycle, because it is desktop-tagged and the desktop step
does not pass `--half`. **Only the tagged steps are unconditional.** Worth checking before anyone
assumes a new guard runs as often as it looks like it does.

## Cycle 17's desktop step (app `443ffdc55934`)

| scenario | result |
|---|---|
| `kf-01` | break, 63.9 s — `qal-j43` |
| `mt-01`, `mt-04` | break — same signature |
| `dg-01` | break, 62.4 s, **both** assertions firing (the scoped predicate holding) |
| `sq-02` | **pass**, 35.0 s — the `qal-j47` fix on its fourth build |

`qal-j43` has now been seen on five distinct app builds without drifting back. `sq-02`'s fix has
held on four. Three consecutive cycles (15, 16, 17) have run clean on checkpoint noise and budget.

## Cycle 18 (half A) — the cadence prediction held

The half-A draw meant `lk-04` should run, and it did:

| scenario | result |
|---|---|
| `lk-01` | pass, 331.3 s |
| `lk-04` | **break, 3.1 s** — `qal-j40`, as predicted for a half-A cycle |
| `fm-01` | break, its own finding only (`probe-no-stale-sidecar`), nothing suppressed |
| `kf-01`, `mt-01`, `mt-04`, `dg-01` | break — `qal-j43`'s full signature on app `a129992316e9` |
| `sq-02` | **pass**, 31.7 s — the `qal-j47` fix on its fifth build |

So the cadence rule from cycle 17 is confirmed rather than merely reasoned: `lk-04` is absent on
half-B cycles and present on half-A ones. `kf-01` ran in both, being desktop-tagged.

`qal-j43` is now six builds deep without drifting back. `sq-02`'s fix has held on five.

## Cycle 18 closed — 272 scenarios, 37 breaks, nothing new

Every break was an already-filed issue reporting itself. No truncation, no budget skips, no
checkpoint noise: four consecutive clean cycles on both measures.

The journey read `6/8 pass, 1 unverified, 1 fail` — J3 failed, J8 unverified. J8 is the stale phone
feed (`qal-j48`), not a product failure, and the headline rate will count it as one. Worth
remembering when reading that number: **0/10 on J8 means nobody has measured it since
2026-09-16**, not that it fails.

### What to expect next

Cycle 19 is due 22:00 and will be **half B**, so `lk-04` will be absent again and `qal-j40` goes
unasked for that cycle. `kf-01` will run, being desktop-tagged. That is the cadence, not a fault.

## Cycle 19 (half B) — the cadence rule now predicts in advance

Predicted before the cycle ran, from `cycle.sh:113` (the letter flips each cycle) rather than from
watching: half B, so `lk-01`/`lk-04` absent and `lk-02`/`lk-03` present. That is what happened.

| scenario | result |
|---|---|
| `lk-02`, `lk-03` | pass (4.0 s, 3.7 s) — half B |
| `lk-01`, `lk-04` | absent — half A, correct |
| `kf-01`, `mt-01`, `mt-04`, `dg-01` | break — `qal-j43`, app `cbb2907a5a7b` |
| `sq-02` | **pass**, 38.2 s — sixth build |

`qal-j43` is seven builds deep. `sq-02`'s fix has held on six.

One practical note for reading the state file: **the half is chosen at the tracked step, not at
cycle start** (`cycle.sh:105-115` runs after the first step, which uses `--kernel-branch rust`).
So `state/library-half` shows the *previous* cycle's letter for the first part of a cycle. I
checked it too early once and had to wait.

### Cycle 20 will be half A, not B

The letter alternates on every cycle, so `lk-04` and `qal-j40` come back next cycle and
`lk-02`/`lk-03` drop out. `kf-01` runs either way.

## Cycle 19 closed — 298 scenarios, 31 breaks, nothing new

No truncation, no budget skips, no checkpoint noise. **Five consecutive clean cycles** (15-19) on
both measures, and four without truncation.

The journey read `5/8 pass, 1 unverified, 2 fail` — J3 and J7 failed, J8 unverified. Reading that
honestly, as `qal-j48` argues it should be read: **two real failures, one thing nobody measured**,
not three bad steps. J8's feed has been stale since 2026-09-16 and the rate will keep reporting it
as failure until either the feed returns or the summary learns the third outcome.

## Cycle 20 (half A) — fourth consecutive half called in advance

| scenario | result |
|---|---|
| `lk-01` | pass, 331.4 s |
| `lk-04` | **break, 3.1 s** — `qal-j40` returned on half A, as predicted |
| `fm-01` | break |
| `kf-01`, `dg-01` | break — `qal-j43` on app `1647b90a8cfa` |

The half has now been called correctly before the cycle ran four times over: 17 (B), 18 (A),
19 (B), 20 (A). It is settled enough that a deviation would be a signal rather than a puzzle.

`qal-j43` is eight builds deep. `qal-j40` keeps being reported by `lk-04` on every half-A cycle, in
3.1 s, which is as cheap as a standing guard gets.

## Cycle 20 closed — 276 scenarios, 32 breaks, nothing new

Its desktop step finished the signature: `mt-01` and `mt-04` broke with `kf-01` and `dg-01`, and
`sq-02` passed at 34.9 s — the `qal-j47` fix on its seventh build.

No truncation, no budget skips, no checkpoint noise. **Six consecutive clean cycles** (15-20) and
five without truncation.

Journey: `5/8 pass, 1 unverified, 2 fail` — the same shape as cycle 19. Two real failures (J3, J7)
and one step nobody has measured since 2026-09-16 (J8, `qal-j48`).

Cycle 21 opened 05:00:43Z and should draw **half B**; the letter is not written until its tracked
step, so the state file will read `A` until then.

## qal-j28 corrected — the cap I was measuring against was the wrong one

Cycle 21's step 1 truncated with **49.8 min of work in a 49.9 min span — a 0.1 min gap, no
suspension** — which no version of my suspension explanation allows. Tracing `exit 124` to its own
line in `cycle.sh` shows why:

- step 1 (`:95`) runs `timeout 50m`
- the tracked step (`:148`) runs `timeout 100m`

Every truncation I measured was **step 1**, and I had been comparing its ~50 minutes of work
against the tracked step's 100-minute cap. There was never a missing half. Step 1 simply uses its
whole fifty minutes and loses the tail.

Three cycles, one shape: 49.9, 49.6, 49.8 minutes of work against a 50-minute cap. Cycle 19's
step 1 completed because it needed only 42.6.

So `qal-j28`'s real subject stands unchanged — **step 1 is too small for what it runs**, and a slow
provider hour makes it reach 33 scenarios instead of 90 — but the suspension theory, the two-clocks
theory and the "unexplained overhead" are all withdrawn.

The fifth consecutive half prediction was also correct: cycle 21 is B.

## Cycle 21 lost a third of its tracked step to a rig crash (`qal-j49`)

`lk-*`, `fm-01` and `sw-*` never ran. I first assumed the half split; it was not that. The tracked
step **died** at `rw-03` after 141 scenarios, on a `FileNotFoundError` in the process reaper:
`finalize()` moves the rollout out of `staging/` but leaves `Recorder.log_path` pointing into the
deleted folder, so the reaper's line kills the run.

It only fires when a scenario leaks a process, and it exits **1** — the same code a healthy step
with breaks returns — so `cycle.sh` reported it as a step that ran. Only the `!! RUN CRASHED`
alarm caught it.

Fixed: `finalize()` now repoints `log_path`, `frames_path` and `sent_path` at the rollout.
Verified directly. `run.py` synced to the live loop, so cycle 22 onward has it.

**Coverage note for cycle 21:** `qal-j40` was not asked at all, and `fm-01`'s finding is absent.
Cycle 22 is half A, so `lk-04` returns and both come back.

## Cycle 21 closed — 264 scenarios, with a third of the tracked step lost

Its desktop step finished normally on app `dcf8313cf00b`: `kf-01`, `mt-01`, `mt-04` and `dg-01`
broke (`qal-j43`'s full signature, ninth build) and `sq-02` passed at 37.1 s (eighth build).

But the cycle is not a clean one. Two separate losses:

- **step 1 truncated** at its 50-minute cap, as it does whenever it needs slightly more (`qal-j28`);
- **the tracked step crashed** at `rw-03` and lost everything after it (`qal-j49`, now fixed).

So `qal-j40` went unasked this cycle and `fm-01` produced no finding. Neither is a product change;
both are the rig. Cycle 22 is half A, so `lk-04` returns and both gaps close.

## Paused — 2026-09-20 12:05Z

Jacob asked for every Arbos worker to pause. I have stopped, opened no further cycle, and set the
loop's own switch:

```
state/PAUSED-until-2026-10-20T00:00:00Z
```

That is the mechanism in `vm-loop.sh:21-31` (Jacob's, 2026-09-13), which stops every run until the
instant named. **To resume: delete that file.** The date is a month out only so it cannot expire
quietly on its own; it is not a schedule.

Cycle 22 was in flight and I have left its own process to finish rather than killing it mid-step —
stopping it by hand risks stray kernels and half-written rollouts, and the pause is checked at the
top of the next iteration, so no cycle 23 will open.

### Cycle 22's state at the pause

Half A, 285 scenarios, **zero crashes**, and the `qal-j49` fix confirmed working in production:

| | cycle 21 | cycle 22 |
|---|---|---|
| tracked step | died at `rw-03`, 141 scenarios | **147 scenarios, exit 1, no crash** |
| `lk-04` / `qal-j40` | never ran | **break, 3.1 s** |
| `fm-01` | never ran | break, its own finding |

So the gap cycle 21 lost is closed, and `qal-j40` is being asked again. `kf-01`, `mt-01`, `mt-04`
and `dg-01` broke (`qal-j43`, tenth build); `sq-02` passed at 44.7 s (ninth build). Step 1 did not
truncate this cycle.

### Where things stand for whoever picks this up

Open and mine: `qal-j40` (product, guarded by `lk-04` on half-A cycles), `qal-j43` (product, live
on ten builds), `qal-j46`, `qal-j48` (both loop-side, in QA). Fixed in the rig: `qal-j39`,
`qal-j44`, `qal-j47`, `qal-j49`. Settled/closed: `qal-j28` (step 1's 50-minute cap), `qal-j35`,
`qal-j42`, `qal-j45`. Left alone: steer-order.

## Stopped — 2026-09-21 12:30Z

Jacob asked for all background processes stopped and every timer and subscription cancelled. Done,
and nothing is left running.

**Subscriptions and timers:** none existed. `list_subscriptions` returns empty — I never created
any during this run.

**Processes stopped**, in this order so nothing restarted behind me:

| | |
|---|---|
| `vm-loop.sh` | the supervisor, killed first so it could not open anything |
| `mirror-timer.sh` | the store-docs mirror's 15-minute timer |
| `cycle.sh` + its two `run.py` | a cycle that had started 12:00:26Z on 09-20, five minutes before the pause |
| 25 Arbos processes | kernels sent `SIGINT` for a clean shutdown, then desktops and Xvfb terminated |
| 21 tmux sessions | every one on the machine, including those left by earlier workers (`build-*`, `qa-cycle3`, `rollouts-*`) |

Final sweep is empty; load 0.05.

### One correction to my last note

I said "cycle 22 was in flight". It was **cycle 23**, started 12:00:26Z, five minutes before I set
the pause. The pause marker did its job — no cycle opened after it — but the one already running
survived the VM's 24-hour suspension and was still going when I came back.

### To resume

1. `rm /home/ubuntu/arbos-qa/state/PAUSED-until-2026-10-20T00:00:00Z`
2. restart the supervisor: `cd ~/arbos-qa && bash deploy/vm-loop.sh` (it re-reads the store's
   modules at each cycle start)
3. the mirror timer is separate: `bash deploy/mirror-timer.sh`

The marker alone does not restart anything — the supervisor process is gone now, not merely idle.
