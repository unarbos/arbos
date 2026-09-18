# qal-j39 — the consistency checker still validates the engine #104 retired, and nothing of its replacement

- **status**: fixed in the rig (this loop's defect), with a four-arm control
- **found**: 2026-09-18 13:46, hunting detectors that have never fired
- **kernel**: observed against `arbos-kernel 0.2.0 cecd48e1bd76 protocol 1`
- **file**: `internal/qa/consistency.py`

## What was wrong

`cx.check()` runs after nearly every scenario in the library. Counting how often each of its rules
has ever fired across all nine of today's cycles, **19 of 24 have never fired once**. Some of those
zeros are healthy — `malformed-folder` stages the state and the product handles it. But the names
group:

```
plan-bad-lines  plan-active-after-stop  plan-attempt-dangling  plan-parent-dangling
plan-done-unattempted  plan-terminal-with-attempt  plan-md-missing
attempt-node-dangling  attempt-running-after-stop  attempts-bad-lines
```

Ten rules, and they read `plan.jsonl`, `attempts.jsonl` and `plan.md` — the engine **`#104`
replaced with `subscriptions/`**. They cannot fire on any current build, because the files they
check no longer exist.

And the checker mentioned `subscription` **zero** times. So the check every scenario leans on was
validating a subsystem that no longer exists, and nothing at all of the one that does.

## Why that is worse than dead weight

A rule that cannot fire is not neutral: it looks like coverage. Ten green rules about scheduled work
give the impression that scheduled work is checked, while the engine actually running it had no
invariant checked anywhere — which is the same thing `qal-j38` found one layer up, where the
scenarios that tested the old engine self-skipped and named successors that did not carry their
properties. The migration moved the code and left both the tests and the checker aimed at the old
target.

## What was added

Four rules, each for a fault this loop has already met rather than invented:

| rule | the fault |
|---|---|
| `subscription-incomplete` | no `id`/`created`/`next_due` — the kernel silently drops such a file and says nothing (`qa-029`) |
| `subscription-due-past-its-period` | `next_due` further ahead than one period: what a clock set backwards leaves, stranding it for the length of the jump (`qal-j38`) |
| `subscription-unreadable` | the file cannot be read at all |
| `subscription-due-unparseable` | `next_due` is not an RFC 3339 instant |

The second matters most: it turns `qal-j38` from something only a scenario stages into a **standing
detector**, so it fires on any place the loop inspects — the same reason `#450`'s double-serving
detector is worth more than a probe.

## Control, all four arms plus the false-positive that mattered

| staged | result |
|---|---|
| due in 20 s on a 30 s period (healthy) | **no findings** |
| due 10 days ahead on a 30 s period | `subscription-due-past-its-period` |
| no `id`/`created`/`next_due` | `subscription-incomplete` |
| file unreadable (mode 000) | `subscription-unreadable` |
| `next_due = "not-an-instant"` | `subscription-due-unparseable` |

The one that had to be right: **every place the kernel bootstraps carries a `weekly-git-gc`
subscription** with `every = "7d"` and `next_due` seven days out. A careless threshold would flag it
on every scenario in the library and bury the signal in noise.

| the kernel's own default | result |
|---|---|
| `every = 7d`, due in 7 days (normal) | **not flagged** |
| the same, due in 37 days (a month's rewind) | flagged — *"next_due is 888.0 h ahead on a 7d period"* |

So it catches the stranded state, including on the kernel's own chore, and stays silent on normal
operation.

## Left alone deliberately

The ten `plan-*` and `attempt-*` rules are still there. Removing them is a judgement about whether
any place anywhere still carries `plan.jsonl` — a project last served by a pre-`#104` kernel would —
and `fp-migration-legacy-plan` exists precisely because that migration is a live path. A rule that
cannot fire on a current build can still fire on an old place, so they stay until someone who owns
the migration says the legacy shape is gone.

What is fixed is the absence on the other side.

## Precision, measured against every real place the loop has recorded

A new standing rule earns its place by being quiet. Run over **3,255** rollout states from 13
September onward — every one that carries subscription files — the four rules produce **16
findings**, and all sixteen are correct:

| finding | where | why it is right |
|---|---|---|
| 15 × `subscription-due-past-its-period` | 14 in `clock-jump-cron` rollouts of 13–14 Sep, 1 in `ck-01` | deliberately staged far-future due times |
| 1 × `subscription-incomplete` | `fp-shell-subscription` | its `bare=True` file, staged on purpose to see what the kernel does with a hand-written one |

**No false positives on ordinary runs**, and none on the `weekly-git-gc` chore that every place
carries.

The 14 are worth a second look, because they say something the rule was not written for. Their
files are named `0003-tick-future.toml` and `0004-deferred-far-future.toml` — those are
`clock-jump-cron`'s own plan-node goals ("tick future", "deferred far future"), which means the
kernel's `#104` migration **carried those far-future due times across from `plan.jsonl` into
`subscriptions/`**. They have been sitting in those rollouts since 13 September, and until this rule
existed nothing could see them.

So the rule fires on migrated state as well as freshly written state, which is the case that
matters: a place carried forward from before `#104` is exactly where a stranded due time would
survive unnoticed.

## The retroactive check is worth repeating

The sweep took 16 seconds over 3,255 places and needs only a symlink per place, because
`state-after/` **is** the `.arbos` content and `check_place` wants a directory containing `.arbos`.
My first attempt pointed it at the rollout directory instead and reported a confident **zero** —
vacuously, since it never found a `.arbos` and never reached these rules at all. A new detector
scored against real history is only worth the reading if the reading happened.
