# qal-j46 — the daily budget runs out around 22:00, and the loss always falls on the same steps

- **status**: open (loop design, not a product fault)
- **found**: 2026-09-18 22:45, working out why `mt-24` never ran in cycle 12
- **measured on**: 2026-09-18, eight cycles, `--budget-usd 60`

## What happened

Cycle 12's desktop step ran six scenarios and skipped the rest:

```
[skip] kf-01-a-chat-opened-during-kickoff-keeps-what-you-type: daily budget reached ($60.30 of $60.00)
[skip] mt-01-typed-while-running-steers:                       daily budget reached
[skip] mt-04-queue-survives-window-restart:                    daily budget reached
[skip] mt-24-relaunch-restores-active-tab:                     daily budget reached
[skip] dg-01-a-sub-chats-turn-starts-under-the-harness:        daily budget reached
[skip] journey-linux:                                          daily budget reached
[skip] af-02, af-03, cp-02, fb-01, im-02, desktop-composer-pills …
```

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

## What it cost today, concretely

`qal-j43` regressed on `main` at 19:11 (`2ea8d565`) and `kf-01` caught it in cycle 12's *earlier*
step. Had the regression landed a little later, the check written specifically to catch it would
have been skipped for budget and the regression would have gone unseen until tomorrow.

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
