# qal-j45 — model turns got ten to thirty times slower, and the cycle's first step now covers a quarter of what it did

- **status**: open; the slowdown is measured and reproducible, the cause is **not yet isolated** — the first attempt to isolate it was invalid (see "The first alternating run was invalid")
- **found**: 2026-09-18 19:05, checking whether cycle 11 would finish inside its cap
- **kernel**: slow on `arbos-kernel 0.2.0 fba8688d92d2`; fast on `cea8b902eecf` and everything before
- **cost so far**: cycle 11's first step stopped at **25 scenarios**; cycle 10's reached **62**

## What happened

Cycle 11's first step hit its 100-minute cap and was killed (`-- run.py exit 124`) after 25
scenarios. The same step in cycle 10 completed 62. Nothing about the step changed; the scenarios in
it got slower.

The same scenarios, every cycle today, in seconds:

| scenario | 08:01 | 09:01 | 12:00 | 15:01 | **17:01** |
|---|---|---|---|---|---|
| `ordinary-task` | 21.8 | 29.2 | 29.0 | 26.6 | **361.7** |
| `secrets-leak-hunt` | 35.4 | 17.4 | 21.3 | 15.5 | **501.3** |
| `kickoff-session` | 184.8 | 360.2 | 157.2 | 119.4 | **489.4** |
| `spawn-storm` | 32.6 | 31.2 | 649.8 | 49.8 | **556.8** |
| `steer-storm` | 9.0 | 15.4 | 8.4 | 7.6 | **4.9** |

`ordinary-task` — one turn, write a FizzBuzz file, say what it did — went from **27 seconds to six
minutes**. `secrets-leak-hunt` went from about 20 seconds to over eight minutes.

`steer-storm` is the control that makes this readable: it is the one row that did **not** move,
and it is the one that leans least on model turns. So this is not the machine being slow, the disk
being slow, or the harness being slow. It is turns.

## Why it matters more than it looks

The cycle's value is coverage per cycle. At cycle 10's rate the first step asked 62 questions; at
cycle 11's it asked 25. Everything it did not reach is not "slower to find" — within a cycle it is
**not asked at all**, and the library's ordering means the same tail goes unasked every time. That
is `qal-j28`'s problem arriving by a different road: there, the model-driven inbox family crowded
out the deterministic tail; here, every model turn costs ten times more.

## Which of the two

Two candidates, and I am not yet calling it:

1. **The Jev controller commits.** Between the fast 15:01 cycle and the slow 17:01 one, `a47c5104`
   ("Jev fail ends the turn; the chat model does not run", 15:05) and `2d5cad97` ("Jev posts
   Decisions, not chat completions", 15:34) landed. Every turn now makes a controller call before
   the chat model, so a turn costs two round trips where it cost one, and a slow controller call
   stalls the turn. The correlation is exact: the last fast cycle built its kernel before those
   commits, the first slow one after.
2. **Provider rate limiting.** Demonstrably present today — `qal-j45`'s sibling finding, the
   `env:provider-rate-limited` detector, exists because two rollouts died on a 429 with
   *"Jev did not choose the next step: 429 rate limited"*. A throttled provider slows every turn
   whatever the kernel does.

These are not exclusive, and they interact: the controller doubles the calls per turn, which
doubles the exposure to a throttle.

**Running all of one build and then all of the other cannot separate them**, because the provider's
mood changes across the hour that takes — which is exactly how my first attempt went (`fba8688d`
221 s and 361 s, then `cea8b902` 30 s and 91 s, with the older build's own failure text naming rate
limiting). Alternating the builds run for run does separate them: both meet the same conditions.
That is `deploy/ab-kernel-speed.sh`.

### The first alternating run was invalid, and why

It compared `fba8688d92d2` against `00cc5ba89968` — and **both carry the Jev commits**
(`git merge-base --is-ancestor` says yes for `a47c5104` and `2d5cad97` on each). The loop rebuilt
`target-track-main` partway through cycle 11, overwriting the older binary I had aimed at while the
test was running. So the test compared two post-change builds and can say nothing about candidate 1.

Two things are still worth taking from it:

- the `old` arm — itself a current build — ran `ordinary-task` in **193 s, 362 s, 186 s**, against
  21–29 s for the four cycles before today's change. Both current builds are slow.
- the `new` arm produced no verdict in any of four rounds, because each exceeded the probe's
  12-minute ceiling. That is a stronger statement than any number in the table.

The lesson for the rig: **a probe must not point at a binary the loop owns.** `target-track-main`
and `target-desktop-main` are rebuilt every cycle; a comparison against "the old build" has to hold
its own copy.

### The valid test, set up

The fast 15:01 cycle built its kernel before `a47c5104` (15:05) and was quick; the slow 17:01 cycle
built after and was not. So the before-build is `232518c2` (14:59), the commit `a47c5104` sits on
top of. That is building now into `~/arbos-qa/target-probe-prejev` from its own worktree
`~/arbos-qa/repo-probe-kernel` — **directories the loop does not touch** — and the alternating run
against it is what decides candidate 1. Nobody should act on candidate 1 before that number is in
this file.

## What is already done about it

Nothing that fixes the slowdown — these are containment:

- `env:provider-rate-limited` (in `run.py`) stops a turn killed by a 429 being drafted as a product
  bug. Two rollouts had already filed three `wrong-output` breaks each for work the model was never
  asked to do.
- `spawn-storm`'s teardown is bounded (one 60 s deadline instead of 60 s per child), which returned
  about five minutes per cycle. Confirmed: `settle_seconds: 60.0`, `settle_gave_up_at: worker-4`.

Neither touches the cause. If the alternating test finds candidate 1, this is a product performance
regression and the cap conversation is a distraction; if it finds candidate 2, the loop needs to
decide whether to slow its own request rate rather than lose coverage to a throttle.
