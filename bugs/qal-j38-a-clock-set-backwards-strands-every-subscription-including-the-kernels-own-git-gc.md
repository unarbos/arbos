# qal-j38 — a clock set backwards strands every subscription, including the kernel's own git gc

- **status**: open (product)
- **found**: 2026-09-18 13:03, taking after-failure states nobody had staged
- **kernel**: `arbos-kernel 0.2.0 a8678ac16636 protocol 1` (today's `main`)
- **control**: `ck-01-a-subscription-survives-a-clock-jump-without-a-storm-or-being-stranded`
- **rollout**: `20260918T130308Z-ck-01-…`

## What happens

A subscription whose `next_due` is further ahead than its own period — which is exactly what a
clock corrected **backwards** leaves behind — is never pulled back. It does not run, its due time
on disk is not touched, and nothing is said.

Staged with two shell subscriptions, `every = "30s"`, one due ten days ago and one due ten days
ahead:

| | runs in 28 s | `next_due` on disk afterwards |
|---|---|---|
| ten days **overdue** | **1** | pulled to `2026-09-18T13:03:38Z` — one period out |
| ten days **ahead** | **0** | still `2026-09-28T13:03:08Z`, untouched |

The overdue arm is the reassuring half: 28,800 missed periods coalesced into a single run, so the
engine does not fire a backlog. The scan plainly happened — it corrected the overdue one in the
same sweep — and it left the future one alone.

## Why ten days ahead is an ordinary state

Nothing needs to go wrong in Arbos to reach it. An NTP correction on a machine whose clock ran
fast, a timezone or manual clock fix, a VM restored from a snapshot, a dead RTC battery. The clock
moves back; every `next_due` the kernel wrote is now in the future by the size of the jump.

## Its own default subscription is in the blast radius

Every place the kernel bootstraps carries one:

```toml
id = 1
kind = "shell"
prompt = "Weekly git gc of the .arbos repository (kernel chore)."
every = "7d"
cmd = "git -C .arbos gc --auto --quiet"
deliver_to = "none"
internal = true
```

`deliver_to = "none"` and `internal = true`, so its silence is by design and nobody would notice it
stop. It is also the chore that keeps `.arbos` from growing without bound — the checkpoint refs
that `drop_checkpoint_refs` exists to prune are what a long project accumulates, and its own
comment describes a repository that "kept every turn's working tree for ever". A clock rewind of
a month strands the one chore that cleans that up, for a month, silently.

## This property had a test, and it was retired with its engine

`clock-jump-cron` (`run.py:866`) held exactly two properties over the old `plan.jsonl` engine:

- **coalesce** — a node ten days overdue fires once, not once per missed period;
- **`cron-clock-rewind`** — a node whose `next_due` is ten days ahead "should n[ot]" stay there.

`#104` replaced that engine with `subscriptions/`, and the scenario now sets itself aside with a
note naming `fp-shell-subscription` and `fp-timer-subscription` as where to look instead. Neither
inherits either property: both test the cadence of a subscription that is due now, and the shared
helper `write_subscription` defaults `next_due` to one second ago, so nothing in the library had
ever put a subscription far out of date in either direction.

So the expectation here is not invented — it is the product's own prior contract, and this file is
only noticing that half of it was lost in a migration. Coalesce survived the change on its own
merits; rewind recovery did not.

**The lesson is about the skip, not the clock.** A scenario that retires itself politely and names
successors is the most trustworthy-looking thing in a suite, and these successors do not carry what
it was for. Worth asking of every self-skip that points elsewhere: does the thing it points at
assert what it asserted?

## Suspected fix

Treat a `next_due` further ahead than one period as a rewound clock rather than a schedule, and
pull it back — which is what the plan engine did. The coalescing path already normalises a due time
in the past; the same sweep can normalise one implausibly far in the future.
