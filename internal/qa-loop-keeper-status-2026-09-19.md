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
| current cycle | none open; **18 closed 21:02:25Z** (half A), 272 scenarios, 37 breaks |
| previous | 17 closed 17:44:10Z, 292; 16 closed 13:22:48Z, 262 |
| next | cycle 19 due 22:00, half B (so no `lk-04`) |
| clean run | cycles 15-18: 0 checkpoint noise, 0 budget skips, no truncation in 16-18 |
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
