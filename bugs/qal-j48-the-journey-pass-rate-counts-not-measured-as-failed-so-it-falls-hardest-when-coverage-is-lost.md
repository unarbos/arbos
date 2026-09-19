# qal-j48 — the journey pass rate counts "not measured" as "failed", so it falls hardest exactly when coverage is lost

- **status**: open (loop reporting, not a product fault)
- **found**: 2026-09-19 04:45, taking a second reading on a "decline" I had flagged
- **measured on**: `loop/journey-history.jsonl`, the last ten journey runs

## The headline and the truth

`cycle.sh:314` computes the rate as `sum(1 for r in tail if r["steps"].get(s) == "pass")`. Only
`pass` counts, so **`fail` and `unverified` are indistinguishable in the number**. What the last ten
runs actually hold:

| step | headline | pass | unverified | **fail** |
|---|---|---|---|---|
| J1 | 5/10 | 5 | 4 | **1** |
| J2 | 6/10 | 6 | 4 | **0** |
| J3 | 5/10 | 5 | 4 | **1** |
| J4 | 5/10 | 5 | 4 | **1** |
| J5 | 6/10 | 6 | 4 | **0** |
| J6 | 4/10 | 4 | 4 | **2** |
| J7 | 4/10 | 4 | 4 | **2** |
| J8 | **0/10** | 0 | **10** | **0** |

Real failures over ten runs are **nought to two** per step. The headline reads as four to six, and
for J8 as a total collapse.

**J8 has never failed.** It has been unverifiable for the whole window:

```
J8: unverified — restart and second project fine; dropped connection:
    phone loop's last J8c is older than 24 h (2026-09-16T22:46:18Z)
```

That feed is now about **54 hours** stale. J8's `0/10` says nothing about Arbos; it says the phone
loop stopped posting two days ago and nobody noticed, because the number it produces looks like an
ordinary bad score rather than an absent one.

## Why every step fell together

**Four of the last ten runs verified nothing at all** — `pass 0, unverified 8, fail 0`. Those are
cycles where the journey never ran, and they count as a failure against every step at once. That is
why the whole row slid in step, which is also what made it look like a product-wide regression when
I first flagged it.

The journey lives in the desktop step, and the desktop step is what a spent budget takes first
(`qal-j46`). So the two findings compound: the budget removes the measurement, and the rate then
reports the missing measurement as failure. **The number is least trustworthy precisely in the
cycles where you most want to know.**

## The third instance of one mistake in one day

This is the same error in a third place:

| where | "could not ask" recorded as |
|---|---|
| `kf-01` | a **pass**, when the kickoff window was missed (fixed: retries, then records `inconclusive`) |
| `sq-02` | a **skip**, when the model would not hold a turn (fixed: retries, then breaks) |
| the journey rate | a **failure**, when the run never happened (this) |

Three different wrong answers to the same question, which suggests the rule is worth stating once
where everyone can see it rather than fixing case by case:

> Three outcomes, never two: it passed, it failed, or it was not asked. Any summary that folds the
> third into either of the others will mislead exactly when something has gone wrong with the rig
> or the budget, because that is when the third case happens.

## The fix is small

Report the three counts, or at minimum denominate the rate by the runs that actually ran:

```
J8 0/10          ->   J8 0 pass / 0 fail / 10 unverified   (or "J8 —/0, not measured")
J2 6/10          ->   J2 6 pass / 0 fail / 4 unverified
```

A step that has not been measured in ten runs should be *louder* than a step failing half the
time, not quieter. J8 has been dark for two days behind a number that reads like a known-bad test.

I have not changed `cycle.sh` — it is the loop's own reporting and a one-line change there alters
every cycle's summary format, which is worth a decision rather than my hand. The measurement above
is reproducible from `loop/journey-history.jsonl` in a few lines.

## Also worth someone's attention

The phone loop's J8c feed has been stale since 2026-09-16 22:46. Whatever posts it has stopped, and
nothing alarms on that — the only symptom is a score that looks like a failing test.
