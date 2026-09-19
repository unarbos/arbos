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
| current cycle | **17**, open, started 14:00:35Z |
| last closed | 16, closed 13:22:48Z, 262 scenarios |
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
| `qal-j40` | deleting both lock files lets a second kernel serve one place | **open (product)**; guarded every cycle by `lk-04` (3.1 s) |
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
