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

**139 scenarios never ran**, of which **116 are the deterministic library** — the part that does
not need a model and that carries today's findings:

- the whole rewind family: `rw-01`..`rw-10d`, including `rw-08*` (failed restore leaves the tree
  where it was) and `rw-10*` (rewind against a concurrent `git`)
- the whole stale-write family: `sw-01`..`sw-06`, including **`sw-02-stale-undo-mark-resets-past-committed-work`**, which is `qal-j22`'s own subject
- all 29 `mt-*` multitasking scenarios
- `lk-01`..`lk-03`, `fm-01`, `af-01`..`af-04`, `uw-01`..`uw-04`
- `ra-01-root-and-home-wipes-are-refused` — the attack the namespace wrapper exists for

Two of the lost names are covered elsewhere and are **not** a real loss: `journey-linux` and
`kickoff-session` run again in the desktop step, which reached `journey-linux` at 07:06 in this
same cycle. Counting them as cost would overstate the damage. The 116 deterministic scenarios
above exclude nothing else that a later step picks up: `af-*` and `uw-*` have step 3a2 from cycle
7 onward, and the rest have no second home.

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
