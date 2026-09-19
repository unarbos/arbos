# qal-j28 — the inbox grows without bound and crowds the deterministic library out of the cap

- **status**: open (loop defect, not a product defect)
- **found**: 2026-09-18, cycle 6, tracked `main` step
- **kernel**: `arbos-kernel 0.2.0 d373422662bd protocol 1` (tracked step build)
- **rollouts**: cycle log `logs/cycle-20260918T042839Z.log` lines 932–1726

## What happens

Every file in `loop/inbox/` becomes a scenario `inbox:<feature>`, minted with
`needs_model=True`. Nothing ever removes one. The directory is also used by other loops as a
**status channel**, so notes that describe no product behaviour still mint a paid model scenario
that runs in every future cycle.

The result, measured in cycle 6's tracked step:

| | count | share of library |
|---|---|---|
| `inbox:*` scenarios | 135 | 46% of 291 |
| deterministic scenarios | 156 | 54% |

| | ran | wall time | mean |
|---|---|---|---|
| `inbox:*` | 111 | 87.2 min | 47.2 s |
| everything else | 30 | 11.2 min | 22.4 s |

The inbox took **89% of the step's measured time**. The step was killed by its 100-minute cap on
`inbox:worktree-cleanup`, number 152 of 291.

## What the cut cost

**139 scenarios never ran.** Of those, 23 have a second home and are not a real loss — the
desktop step's 26-scenario set picks up `journey-linux`, nine `mt-*`, the `desktop-*` set and
`af-02`/`af-03`, and step 3a2 picks up `uw-01`..`uw-04` and `af-04` from cycle 7 onward.
(`kickoff-session` was never lost; it ran inside the first 152.)

That leaves **116 measured by no step at all, of which 93 are deterministic** — the part that
needs no model and carries today's findings:

| family | unmeasured | what it covers |
|---|---|---|
| `mt-*` | 20 of 29 | multitasking, steering, queue survival |
| `rw-*` | 17 | the whole rewind and restore family, including `rw-08*` (a failed restore leaves the tree where it was) and `rw-10*` (rewind against a concurrent `git`) |
| `bt-*` | 8 | batch and history paging |
| `fp-*` | 7 | file-plan subscriptions and plan pages |
| `sw-*` | 6 | the stale-write family, including **`sw-02-stale-undo-mark-resets-past-committed-work`**, which is `qal-j22`'s own subject |
| `co-*` | 5 | sleep and worker-report ordering |
| `rm-*`, `fr-*`, `lk-*` | 4, 3, 3 | remote spawn, first-run fallback, the held-record family |
| `ra-*` | 2 | **the root and home wipe refusals** — the attack the namespace wrapper exists for |
| the rest | 18 | one or two each across `re`, `rp`, `fm-01`, `fs`, `jl`, `pn`, `sb`, `st`, `sv`, `wt`, `xp`, `journey-j8a-headless` |

The two facts that matter most in that table: `sw-02` names the exact destruction `qal-j22` is
open about and has not run on `main` today, and `fm-01` is the first-match property the whole
`fm-*` family was to be built on.

A cap that always falls in the same place does not sample the library; it truncates it at a fixed
point. Everything after index 152 in registration order has not been measured on `main` by the
tracked step since the library passed that length.

## Why it is a defect and not just a slow suite

Two separate things are wrong.

1. **The inbox has no retirement.** A note is a prompt to look at something once. Nothing marks a
   note as looked-at, so the cost is permanent and the set only grows. 24 of the 135 are
   `swebench-loop-cycle-1` .. `-24`, one per benchmark cycle from `bc-bfb2cd63`, each a result
   report rather than a behaviour to probe. That family grows by one every benchmark cycle and
   will never stop growing.
2. **A model scenario is minted whether or not the note names a behaviour.** `needs_model=True` is
   set for every note unconditionally, so a status report costs a model turn forever.

The swebench notes are **not** the headline cost — the five that ran took 5.9 minutes between
them. The headline is the whole inbox family and the absence of any way out of it.

## Already in place for cycle 7

- **The half split.** Scenarios partition by `sha1(name) % 2` into halves A/B that alternate, with
  `kickoff-session` and `journey-linux` in both. Halves both parts, so roughly 68 inbox and 78
  deterministic per cycle.
- **The 45-second silence bound** on the inbox wait. Cycle 6 hit the 300-second ceiling three
  times (`inbox:agent-defs`, `inbox:attach-replay-deltas`, `inbox:job-streaming`) and there were
  8 `turn-never-ended` breaks; each now gives up after 45 seconds with no frame and says which of
  the two it was.

**Prediction, falsifiable at cycle 7's boundary:** half A is about 68 × 47 s + 78 × 22 s ≈ 82 min
before the silence bound, and under it once the ceiling hits are trimmed. Cycle 7's tracked step
should therefore **finish rather than truncate**. If it truncates anyway, the split is not enough
and the inbox needs retirement, not division.

### The prediction held, measured at cycle 8 (cycle 7 died before its tracked step, `qal-j32`)

Cycle 8 announced `library half A: 146 of 299 scenarios` and its tracked step ended
`-- track main: run.py exit 1` — a normal finish with breaks, **no truncation alarm**. All 146 ran,
against 152 of 291 in cycle 6. So the split is sufficient for now and the inbox does not need
retirement *today*.

Two things keep this from being a closed matter:

- the library grew from 291 to 299 in one day, and `inbox:swebench-loop-cycle-*` reached **26**
  from 24 while this file was open — one per benchmark cycle, exactly as described above. Halving a
  set that grows is a delay with a known end.
- the step used close to its whole cap. The first scenarios after the build are cheap and the
  `inbox:*` block in the middle is not; a handful more notes puts the last family back under the cut.

The budget control at the end of this file is therefore still the one worth having: assert that the
step's scenario count times its measured mean fits the cap, so the suite goes red when it outgrows
its cap rather than truncating quietly.

## The fix this needs

Halving is a delay, not a repair: the inbox grows and will pass the cap again. The repair is for a
note to be able to stop costing anything —

- mark a note handled once its scenario has passed on some named build, and stop minting it; or
- mint `needs_model=True` only when the note names a behaviour, and take status reports as reading
  material rather than as scenarios; or
- have the writing loop post status somewhere that is not the scenario generator's input.

The third is the cheapest and the most honest: `loop/inbox/` means "probe this", and a benchmark
result is not that.

## Control

`qal-j28-control`: assert that no note mints a `needs_model` scenario unless it names a behaviour
to probe. It fails today on at least the 24 `swebench-loop-cycle-*` notes, which are result
reports and mint 24 paid scenarios between them — that number is a floor, not a survey; the
remaining 111 notes have not been classified.

The control worth keeping afterwards is the budget one: assert the tracked step's scenario count
times its measured mean fits inside the cap, so the suite goes red when it outgrows its cap
instead of silently truncating. A truncation that prints per-scenario passes and no total is the
failure mode this whole file is about.

## Half the cap is not scenario work (measured 2026-09-19)

A thing this write-up assumed and I had not checked: that the 100-minute cap is spent *running
scenarios*. Summing the durations the loop itself records, per cycle's first step:

| cycle | scenarios | measured work | outcome |
|---|---|---|---|
| 15:01 | 70 | 27.8 min | completed |
| **17:01** | **33** | **49.9 min** | **TRUNCATED at 100 min** |
| 21:01 | 80 | 49.3 min | completed |
| 00:00 | 81 | 44.3 min | completed |

Two readings come out of it.

**The truncated cycle did no more work than the ones that finished** — 49.9 min against 49.3 and
44.3. It reached 33 scenarios instead of 80 because each cost more: that hour is `qal-j45`'s
provider slowdown (`ordinary-task` 361 s, `secrets-leak-hunt` 501 s against 21-29 s and 15-35 s).
So the crowding-out this bug describes is not the only way the tail goes unasked; a slow hour does
it with the same library.

**And only half the window was measured work at all.** Two truncated steps, the same shape:

| step | scenarios | measured work | cap |
|---|---|---|---|
| 17:01 | 33 | 49.9 min | 100 min |
| 04:37 | 77 | 49.6 min | 100 min |

About fifty minutes of each hundred is not scenario time. **I first blamed VM suspension and that
is wrong**, so the correction matters more than the guess:

- *Suspension is not it.* The 04:37 step spans **132 minutes of wall clock** between its first and
  last scenario and only then hits a `timeout 100m`. If the cap counted suspended time it would
  have fired at 100 wall-clock minutes. Both clocks pause, so the cap is not being eaten by the
  machine being asleep — my earlier note here said it was.
- *Snapshotting is not it.* A whole rollout is 124 files and copies in **6 ms**.
- *The obvious tail is already counted.* `duration_s` is taken at `run.py:2067`, after
  `cx.cleanup()`, after both provider checks and after the `state-after` snapshot — so cleanup,
  the transcript scans and the snapshot are all inside the number, not outside it.

### Per-scenario overhead, measured directly

Ten deterministic scenarios in one `run.py`, timed from outside:

```
sum of reported durations: 247.4 s
wall clock for the run:    248.0 s
unaccounted:                 0.6 s   (0.1 s per scenario)
```

So the loop's own numbers account for essentially all of a run's wall clock when nothing else is
going on. **Per-scenario overhead is not the missing half** — it is a tenth of a second.

Nor is it a hung scenario at the end: in both truncated steps the gap between the last finished
scenario and the next step's first is 1.2 min and 9.0 min, not fifty.

### What the two numbers actually are

The 04:37 step spans 132.9 min of *wall clock* and ~100 min of whatever clock `timeout` counts, and
I was idle for about 86 min of it. 132.9 − 49.6 ≈ 83, which is close enough to the idle window to
say the **wall-clock** gap is suspension. What I cannot square is the other side: within the
~100 min `timeout` counted, only 49.6 min was inside scenario functions, and the direct measurement
above says the gap is not overhead.

The honest reading is that I do not know which clock `timeout 100m` is really counting on a VM that
pauses, and the difference between the two candidate readings is the whole fifty minutes.

So roughly half of each step's **running** time goes somewhere I have not found, consistently, on
two independent measurements. I am not filing it as its own bug and not guessing again; isolating
it needs the loop instrumented between scenarios, which is a deliberate change rather than
something to do in passing.

What is established, and is enough to matter:

> Half the cap is not spent on scenarios, and it is not suspension, snapshots, or teardown. A step
> can be truncated having done fifty minutes of work in a hundred-minute window, and the summary
> reads as though the library was too big for the time.

Anyone deciding what to do about the cap (`qal-j46` lists the options for its budget twin) should
know that making the library more efficient cannot recover a window that was not spent on the
library — and that there is a 50% overhead worth finding before anyone raises the cap to cover it.

## Settled: it is suspension, and I retracted that too soon

I wrote that suspension explained the missing half, retracted it on one wall-clock argument, then
measured properly. The retraction was wrong. What decides it is two controlled runs with the
machine **continuously awake**, timed from outside `run.py`:

| run | scenarios | reported | wall clock | gap |
|---|---|---|---|---|
| deterministic | 10 | 247.4 s | 248.0 s | **0.6 s** (0.1 s each) |
| model-driven | 6 | 759.5 s | 760.0 s | **0.5 s** (0.1 s each) |

With nobody asleep, the loop's own numbers account for essentially all of the wall clock — for
model scenarios as much as deterministic ones. **There is no per-scenario overhead to find.** In
cycles that span my idle periods the same gap is 16–39 s per scenario. The only difference between
those two conditions is the machine being suspended, so that is what the gap is.

The objection that made me retract does not survive either: I thought the kernel build before
`run.py` might absorb the difference, but cycle start to first scenario is **0.8 min**, three
cycles running.

### What I still cannot say

The 04:37 step spans **133 min of wall clock** and its `timeout 100m` had not yet fired — so the
timeout's clock lost about 33 minutes to the suspension, while `time.monotonic()` (the scenario
durations) lost more. **The two clocks discount suspended time by different amounts**, and that
difference is the fifty minutes. I have not worked out the exact rule each follows on this
hypervisor, and it would take a deliberate experiment — suspend for a known interval, read both
clocks — rather than inference from cycle logs.

### What it means in practice

A step's cap is partly spent on time the machine was asleep. Cycles that run across an idle period
truncate having done less work, and the summary reads as though the library was too big for the
window. Cycle 16 is the control: it ran with me mostly present, its main step **completed** rather
than truncating, and it got through 89 scenarios where the 17:01 step managed 33.

So the loop's throughput depends on the agent being there — which is worth knowing before anyone
tunes the cap or trims the library to fit it.
