---
cursor:
  subagentId: "bc-b4f4cdba-0146-5dea-9731-24ea2538adcd"
---

# Is steer order a contract? One inversion in eight runs, nothing lost

**For:** the kernel owner. **From:** the QA loop (second machine), 2026-09-18 13:26 UTC.
**Not a bug file on purpose** — I cannot name the mechanism, and an unexplained intermittent red is
the thing that makes people stop reading reds.

## What happened

`steer-storm` (`run.py:984`) sends 25 steer frames during one running turn and asserts four things:
the kernel lives, the transcript parses, all 25 arrive, and they are **recorded in order**
(`steers == sorted(steers)`).

On `arbos-kernel 0.2.0 a8678ac16636 protocol 1`, in cycle 9, the order was:

```
00 01 02 03 04 05 06 07 08 09 10 11 12 13 14 15 16  23 24  17 18 19 20 21 22
```

`23` and `24` ahead of `17`–`22` — one contiguous block moved, not a shuffle. **All 25 arrived**;
`steer-lost` passed.

## How often

| | |
|---|---|
| passes | 7 — five earlier cycles, plus two fresh runs on `f97bb3487540` |
| breaks | 1 |
| vacuous passes | none: the two fresh runs had 5 and 15 tool events, so the steers really did span several boundaries |

So the property holds most of the time and the passes are not empty.

## Why I am asking rather than filing

`inbox::list`'s doc says *"Every message waiting, oldest first"*, and it resolves order by file
name — `{stamp}-{who}-{seq:03}.md`, sorted. So an inversion means the **names** did not reflect the
order the frames were sent. I could not find a mechanism for that:

- `deliver`'s seq allocation is check-then-write (`if path.exists() { continue }`), which would race
  if two deliveries ran at once — but I found no per-frame `spawn`/task around `inbox::deliver` in
  `serve.rs`, so deliveries look sequential, and sequential delivery gives monotonic names.
- the stamp comes from `msg.sent_ms()`, which one client sending 25 frames in a loop should also
  make monotonic.

Either I have missed the concurrency, or the reordering happens after the listing — between taking
a batch at a tool boundary and appending it.

## The question

**Is steer order a guarantee, or is it incidental?** The answer decides which of two things is
wrong, and they need opposite fixes:

- if ordering **is** a contract, the inversion is a real fault in the naming or the append, narrow
  but real: steers are corrections, and a reader — or the model, which reads the transcript — seeing
  "actually do Y" before "do X" gets them backwards;
- if ordering is **incidental**, then `steer-order` is an assertion bounding something the product
  does not promise, and the honest repairs are the ones that rule taught us: assert the property the
  ordering exists for (nothing lost, nothing doubled — `steer-lost` already does the first), or make
  the kernel enforce the order and then assert it.

I have left the assertion as it stands rather than soften it on a guess. If the answer is
"incidental", say so and I will repair the scenario; if it is "a contract", the rollout
`20260918T125737Z-steer-storm@steer-running-child` has the failing order and
`20260918T132552Z-steer-storm` a passing one on the same build for comparison.

## What would settle it cheaply

A probe that captures the inbox file names as they are created, rather than reading the transcript
after the fact. That separates "the names were out of order" from "the names were fine and the
append reordered them" in one run. I can build it as a model-free scenario — the ordering question
is about delivery and reading, not about the model — if the answer above does not make it moot.
