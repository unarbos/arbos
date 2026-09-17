---
cursor:
  subagentId: "bc-08d8261b-fea2-5075-9949-d45f6f9d4acc"
---

# Kernel ask: `history` for a finished worker returns nothing (M-27's cause)

The phone's worker chat has said "Nothing on record yet" since cycle 2 for every worker that already finished — on the pod kernel too, where the worker ran locally and its transcript is on disk. Probed 2026-09-16 21:08 UTC against the pod kernel (`efcab58`+): `{"type":"history","agent":"fix-j203857-mathlib-challenge","since":0,"limit":50}` answers `history_end {total: 0}`; the same for the brief-form name; `root` answers 1143 lines. The worker is archived (the tree no longer lists it), and `history` seems to read only live agents' transcripts.

Ask: serve `history` for an archived agent from its archived transcript (`.arbos/archive/…` or wherever it moved), or say so in the `history_end` (`archived: true, path: …`) so the phone can read it with `read`. Jacob taps a Done worker to see what it did; today he sees a blank page with "Done" under it (`media/mobile/cycle-32/01-`).

---

## Counted, 2026-09-17 (iPhone loop, cycle 47)

The phone's side is settled and it is not the phone. On `arboslife/qa-cycle-11-demo`:

- the live agent tree carries **one** agent, `root`, with no children;
- the transcript names **12** finished workers, which is where the app's
  "Agents 12" sheet comes from;
- asked over the attach socket for each of the first eight, the kernel
  answers `history_end` with **`total: 0`** — eight out of eight.

```
Run J152618 verification command   -> 0
Fix area bug in J154418            -> 0
Add requester line J154418         -> 0
Run J154418 verification command   -> 0
Write hello m10 file               -> 0
Write hello m100 file              -> 0
Write hello m100c file             -> 0
Rewrite hello m100c file           -> 0
```

The app asks under the same identifier the sheet and the worker-chat header
show, so we are asking the same question the user is. "Nothing on record yet"
is the honest answer to what the kernel returns; there is nothing for the
phone to draw differently.

What Jacob loses is the ability to see what a worker actually did, which is
most of the value of a worker list. Root's own transcript has the report;
the worker's own record is what is missing.
