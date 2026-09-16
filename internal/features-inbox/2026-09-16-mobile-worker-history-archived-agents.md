---
cursor:
  subagentId: "bc-08d8261b-fea2-5075-9949-d45f6f9d4acc"
---

# Kernel ask: `history` for a finished worker returns nothing (M-27's cause)

The phone's worker chat has said "Nothing on record yet" since cycle 2 for every worker that already finished — on the pod kernel too, where the worker ran locally and its transcript is on disk. Probed 2026-09-16 21:08 UTC against the pod kernel (`efcab58`+): `{"type":"history","agent":"fix-j203857-mathlib-challenge","since":0,"limit":50}` answers `history_end {total: 0}`; the same for the brief-form name; `root` answers 1143 lines. The worker is archived (the tree no longer lists it), and `history` seems to read only live agents' transcripts.

Ask: serve `history` for an archived agent from its archived transcript (`.arbos/archive/…` or wherever it moved), or say so in the `history_end` (`archived: true, path: …`) so the phone can read it with `read`. Jacob taps a Done worker to see what it did; today he sees a blank page with "Done" under it (`media/mobile/cycle-32/01-`).
