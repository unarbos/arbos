---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# For QA: steer a running agent (K-03)

From the features agent. Branch `cursor/steer-running-child-b027` → `rust`. PR link follows in the backlog's Shipped table.

## What I am building

`say` gets a third mode, `steer`. It means "act on this now, whatever state you are in":

- Target **running**: the text is injected into its live turn at the next tool boundary, as a `Say` event from the sender. No new turn, no hop budget spent.
- Target **idle**: behaves like `request` (a turn is queued now).
- A steer that arrives as the turn is ending is not lost: the kernel drains any steer still queued when the turn finishes and turns it into an inbox node (a queued turn).
- The receipt tells the sender which of the three happened.
- Same for the user: a `Frame::User{steer:true}` that lands in the last instant of a turn is re-queued instead of dropped.

## How to exercise it

Two agents: root spawns a child with a long brief (e.g. "count to 100 with a bash sleep 2 between each ten"). While the child runs, from root call `say to=<child> text="stop at 30 and report" mode=steer`. Expect: child's transcript gets a `say` line from root at the next tool boundary, the child changes course in the same turn, receipt at root says "injected into its current turn".

Kernel only; drive it over the TCP frames or via the desktop. Read transcripts with `rg '"kind":"say"' .arbos/agents/<child>/transcript.jsonl`.

## What could break — attack here

1. Steer to an agent that finishes between the check and the push (race) — must become a queued turn, never vanish.
2. Steer to a paused agent, to yourself, to an unknown id, with empty text, with `mode=STEER` (case), with `wake:true` legacy flag.
3. Ten steers in one parent step (parallel tool calls) — dedupe is per `(to,text)` per turn; identical texts are refused, distinct ones must all land in order.
4. Steer during the child's compaction or while it waits on `ask` — the injection happens at the next loop iteration; confirm nothing is appended mid-compaction.
5. Kernel restart with a steer queued in memory — the drain only runs on a normal turn end; a hard kill loses in-memory steers (known, F-01 fixes it). Confirm it does not corrupt anything.
6. User steer from the desktop composer on a child chat still works (it uses the same path).
