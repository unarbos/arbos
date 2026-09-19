# qal-j46 — the daily budget runs out around 22:00, and the loss always falls on the same steps

- **status**: open (loop design, not a product fault)
- **found**: 2026-09-18 22:45, reading cycle 12's desktop step
- **measured on**: 2026-09-18, eight cycles, `--budget-usd 60`

## What happened

Cycle 12's desktop step ran six scenarios and skipped the rest:

```
28 run, 2 with breaks, 18 skipped
!! SKIPPED (budget): 18 — af-02-two-windows-on-one-place, af-03-desktop-folder-renamed-under-the-window,
   cp-02-desktop-turn-ends-on-cap, desktop-composer-pills, dg-01-a-sub-chats-turn-starts-under-the-harness,
   fb-01-feedback-report-written-delivered-picked-up, im-02-desktop-no-quiet-line-while-streaming,
   journey-linux, kf-01-a-chat-opened-during-kickoff-keeps-what-you-type, mt-01-typed-while-running-steers,
   mt-04-queue-survives-window-restart, mt-14, mt-18, mt-19, mt-20, mt-23, sq-02 …
```

**`mt-24` is not in that list — it ran and passed (7.2 s) at 22:19:15.** I first reported it as
skipped, on a grep of `^\[(pass|break)\]` that cannot match the log's `[pass ]` with its trailing
space. The scenario was fine; my pattern was not. What follows stands on the eighteen that really
were skipped and on cycle 13's count, both re-checked with a pattern that matches.

Not a desktop fault, not a missing binary — the money ran out. The cap is a **UTC day**
(`run.py:2185`), and today it was reached at **21:57**, eighteen minutes before the desktop step
started.

## The spend was even; the loss was not

```
$10 by 04:47    $20 by 06:40    $30 by 10:12
$40 by 13:08    $50 by 17:52    $60 by 21:57
```

Eight cycles across the day at a steady rate. Nothing spiked — the biggest single scenario is
`kickoff-session` at $7.24 across all its runs, then `spawn-storm` at $2.80.

What is uneven is **who pays for it**. A cycle runs its steps in a fixed order, and the desktop
step and the acceptance journey are near the end. So when the cap lands mid-cycle, it always lands
on them. Every later cycle in the same UTC day is in the same position before it starts.

This is `qal-j28`'s shape in a second currency. There, a fixed 100-minute step cap meant the
library's tail went unasked; here, a fixed daily budget means the cycle's tail goes unasked. Both
times the cost is invisible in the summary — the step reports green on what it managed — and both
times it falls on the same work, because the order never changes.

**The desktop leg is the most fragile part of the library** (`qal-j24`, `qal-j27`, `qal-j33`,
`qal-j42`, `qal-j43`) and it is the part that stops being measured first.

## A prediction, so this is falsifiable

Cycle 13 starts at 23:00 UTC, still inside the exhausted day. It should skip essentially every
model scenario, including all of the desktop leg. The first cycle after 00:00 UTC gets a fresh $60
and should be normal.

### Prediction confirmed

Cycle 13 started 23:01 and by 23:07 had logged **344** `daily budget reached` skips — every model
scenario in it, including the whole desktop leg and the acceptance journey. Its deterministic
scenarios (`boot-idle`, `second-serve`, `concurrent-sessions`, `malformed-frames` …) run normally,
so the cycle is not idle; it is half-blind, and blind on the same half as cycle 12.

That is two consecutive cycles — a quarter of the day's eight — with no desktop coverage at all.

## What it cost today, concretely

`qal-j43` regressed on `main` at 19:11 (`2ea8d565`). `kf-01`, the check written specifically to
catch it, **was one of the eighteen skipped for budget** in cycle 12's desktop step — so the cycle
did not catch the regression; I did, by hand, earlier in the evening. That is the concrete cost:
the guard existed, the regression was live, and the budget kept them apart.

## What I checked before blaming the obvious thing

I did a great deal of model-driven probing today — two bisects over the desktop app, and repeated
runs of `mt-24`, `kf-01`, `mt-04`, `dg-01`, `mt-01`, `ordinary-task` and `spawn-storm`. The
obvious story is that I ate the budget. Measured, it is **$3.91 of $60.30 — about 6%**, and
`spawn-storm` is $2.80 of that:

```
mt-24  15 runs $0.06    kf-01  7 runs $0.03    mt-04 13 runs $0.23
dg-01   7 runs $0.06    mt-01 14 runs $0.28    ordinary-task 13 runs $0.46
spawn-storm 13 runs $2.80
```

Desktop scenarios are cheap because they spend on one short turn; the expensive work is the inbox
family and `kickoff-session`. So my probing is worth keeping honest about but is not the cause, and
cutting it would buy back four dollars of sixty.

## Worth someone's decision

Not mine to set, but the options are visible from here:

- **Move the fragile, cheap work earlier.** The desktop leg costs cents and is the most likely to
  regress. Running it before the inbox family would cost almost nothing and stop it being the thing
  that starves.
- **Reserve a slice of the budget per step**, so the last step cannot be zeroed by the first.
- **Raise the cap or cut cycles per day** — eight cycles against $60 is the arithmetic underneath
  all of this.

The first is the cheapest and fixes the specific harm: the loss currently falls on the work least
able to afford it.

## What the re-run found (2026-09-19 00:30)

Re-running the eighteen after the UTC reset was not bookkeeping. Two of them had something to say:

- `kf-01`, `mt-01`, `mt-04` and `dg-01` broke on `qal-j43`'s regression — the guard that was
  skipped for budget is the one that would have caught it in the cycle;
- `sq-02` self-skipped with a reason never seen before, which turned out to be a **new kernel-side
  regression** in the queued-follow-up path (`qal-j47`), invisible to every cycle summary because
  the scenario records it as a skip.

Eight of the eighteen passed; the other four breaks (`mt-14`, `mt-18`, `mt-20`, `journey-linux`)
were already breaking earlier today and are not new.

So the budget did not merely delay coverage — it hid a live regression for one cycle and would have
hidden a second indefinitely.

## It also corrupts the headline (qal-j48)

The journey lives in the desktop step, so a spent budget removes it — and the journey pass rate
counts an unrun step as a **failure**, not as "not measured". Four of the last ten journey runs
verified nothing at all, and those four count against every step at once.

So the budget does not just cost coverage; it makes the loop's own summary report that lost
coverage as product failure. `qal-j48` has the numbers: real failures are 0-2 per step where the
headline reads 4-6.