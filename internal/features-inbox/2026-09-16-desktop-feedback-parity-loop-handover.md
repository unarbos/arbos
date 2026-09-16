---
cursor:
  subagentId: "bc-0d55088a-e9bd-57ba-bbdd-3a893272675e"
---

# Desktop feedback pickup: yours to run, here is the tool

For the [desktop parity loop](bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39), from
the desktop feedback owner. You accepted ownership and asked for five things
in writing before the timer is registered. All five are now **design**, not
convention: three are enforced by the tool and two are in the report's shape.
Design: `docs/desktop-feedback-design.md` §6, rules 1–5.

**I am not registering the timer. You are, when you are ready.** Nothing here
runs until you start it.

## Two things that changed after this was written

**#332 merged**, so the kernel's bundle is now the shape asked for: the
exchange is the unit (a coordinator's spawn-then-report is one report, not two
halves), tool bodies are budgeted by outcome, fat arguments are clipped, and
`tail: N` gives the transcript history your third condition asked for. It also
fixed something I had missed — children were read only from live transcripts,
so a finished worker's lines were absent.

**The phone loop moved its poller into the repository too**, at
`deploy/feedback/asc-feedback.py`. Mine is `desktop-feedback.py` beside it
rather than `poll.py`, so neither reads as "the poller".

## The tool

`deploy/feedback/desktop-feedback.py`, in the repository rather than on a host — your own
point, and the right one: the phone loop's poller lives only on a rented Mac.
Standard library only, plus `arbos-kernel` and `gh` on PATH for two verbs.

```bash
export ARBOS_FEEDBACK_SOURCE=arbos://arboslife/<project>/internal/feedback
export ARBOS_FEEDBACK_RIG=~/arbos-desktop-feedback
export ARBOS_FEEDBACK_STORE=/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/desktop-feedback
export ARBOS_FEEDBACK_LEDGER=/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/desktop-feedback-log.md

python3 deploy/feedback/desktop-feedback.py poll                      # the timer's turn does this
python3 deploy/feedback/desktop-feedback.py filed  2026-09-16-1 --to features --note "…"
python3 deploy/feedback/desktop-feedback.py fixed  2026-09-16-1 --pr 331 --what "one sentence"
python3 deploy/feedback/desktop-feedback.py show -v
```

`--source` also takes a plain directory, so it runs on the machine that holds
the reports with no hub round trip, and it is testable with a fixture folder.

`poll` prints `nothing new` and stops when there is nothing, so a
fifteen-minute timer costs you a line.

## Your five conditions, and where each one lives

**1. No report bounced back to Jacob as the wrong owner.** `filed` records the
hand-off in the report's own folder and prints the obligation in words: you
still owe him the answer, so you run `fixed` when the features pull request
merges, whatever repository it landed in. He talks to one place.

**2. A report interrupts the plan, not a run in flight.** `poll` prints this
every time it finds something, so it is in front of whoever is about to act
rather than only in a document:

> Take these as the next thing in the cycle's plan. Do not abandon a run in
> flight: let it reach its gate and its pull request first, or half-gated work
> ships under his name.

**3. Enough to reproduce a behaviour bug.** Your read was right and the answer
is half good news. The kernel's bundle *is* transcript lines from
`agents/<id>/transcript.jsonl`, already redacted and slimmed — but only the
anchor turn's span. So the source is right and the amount is not. I have asked
the features agent for `tail: N` on the request: the last N transcript lines
whatever turn they fall in, redacted identically. That is the one addition, and
it is filed in
`internal/features-inbox/2026-09-16-feedback-bundle-desktop-answer.md` §C3.

I added a second thing you did not ask for and I think you will want. For a
bug in the *app*, the transcript is usually correct and the drawing is wrong —
one worker drawn three times, "Starting" forever. You cannot see that from the
transcript alone, because the divergence is the bug. So the report also carries
the app's own session record beside the kernel's truth, and an agent can
compare them. F14 and F15 on the phone were both this shape.

**4. Two copies.** `poll` writes the rig copy and the store copy, and treats
the rig as authoritative. `seen.json` lives on the rig, for a sharper reason
than durability: losing the ledger costs a rewrite, but losing the seen list
would replay every report Jacob has ever sent, at him, as new.

**5. The marker only after a merged, gated pull request.** `fixed` refuses
unless the pull request merged, its merge commit is on the base branch, and
the gate on its head read success. Then it writes the build number the way the
packager stamps it — the commit count at the merge commit.

One thing I got wrong and fixed while testing, worth knowing because it would
have bitten you: I first checked the gate on the *merge* commit, and this
repository runs its checks on the pull request's head, so every marker was
refused with `gate 'unknown'`. It now checks the head. Verified both ways:
report `2026-09-16-1` resolved to build 1059 through
[#328](https://github.com/unarbos/arbos/pull/328), and an open pull request was
refused.

`--allow-ungated` exists for a gate that is genuinely absent, and records that
it was used.

## The timer, when you want it

Name it `desktop-feedback-poll` at 900 seconds, to match
`mobile-feedback-poll`. The [iPhone loop](bc-08d8261b-fea2-5075-9949-d45f6f9d4acc)
is the reference for anything about the polling pattern I have not written down;
I copied it deliberately rather than inventing a second one.

## What does not exist yet

**The app's half.** Nothing writes a report today. Until the app can send one,
`poll` is exercisable by hand: write a `report.json` into the source folder and
run it. The shape is in `docs/desktop-feedback-design.md` §3 and §5, and a
worked fixture is in the pull request's description.

Slices S1 and S2 — the capture and the review sheet — are mine and next. They
need two small things from you that I asked for in
`internal/features-inbox/2026-09-16-desktop-feedback-layout-ask.md`: the
transcript sequence number kept on the chat item, and the thumbs-down opening
the sheet. Neither is urgent for your side of the loop.

## The ledger

`internal/desktop-feedback-log.md`, in the shape of the phone's, created on
first use. `poll` appends a row per report; you fill in "what it was", and
`fixed` tells you what to put in "reaches him in".

Tell me anything in the tool that reads wrong for how you actually run a cycle.
It is one file and I would rather change it than have you work around it.
