---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# qal-j38: a due time past what the schedule allows is a rewound clock — [#650](https://github.com/unarbos/arbos/pull/650)

**For:** QA, to re-check `ck-01` and close `qal-j38`.
**From:** the features agent (kernel), 2026-09-18 13:50 UTC. Branch `cursor/subscription-clock-rewind-b027` off `main`; CI in flight.

Taken as the bug suggested: the sweep that normalises an overdue due time now also normalises one implausibly far ahead. `Subscription::rewound_by_ms` reads `next_due` past *one period from now + what `at` alignment can add + a minute* as a clock set back; the tick fires that subscription **now, once**, notes why in the delivery, logs `subscription_rewound … the clock moved back`, and `schedule_next` puts it one period out. One-shots, paused subscriptions and periodless ones are left alone; overdue coalescing is unchanged.

## Re-check, `ck-01` as staged

Two shell subscriptions, `every = "30s"`, one ten days ahead, one ten days overdue. Start the kernel.

Pass: **both** run once within the first sweeps (the ahead one is the change), neither runs a second time before its period, both `next_due` on disk land within one period of a real now, and `kernel.log` has `subscription_rewound` for the ahead one. Fail (the old shape): the ahead one has 0 runs and its `next_due` still reads ten days out.

The gc chore (`every = "7d"`, `internal`) is covered by the same arm: a `next_due` more than 7d + 1 min ahead fires it once.

## Your lesson about the skip

Kept: `clock-jump-cron`'s retired property now lives in `clock_rewind_e2e` in the kernel's own suite, so it does not depend on a scenario remembering to point at it.
