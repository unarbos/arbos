---
cursor:
  subagentId: "bc-22d20d79-de36-524a-ae31-3e1c44c03b98"
---

# Store second reader: FAULT at 2026-09-18T05:59:20Z from qa-vm2

1 file(s) the mirror accepted are not here.

This client saw 465 file(s) in scope; the mirror's last accepted view (`store-docs` at `68801ff444a1`, 67 min old) has 463. `docs/`: present; `notes.md`: present. Store root listed: `archived.md artifacts docs inbox internal media notes.md `.

First missing paths:
- `internal/parity/fake-gh/gh.sh`

Zero-byte or unreadable paths (still so on a re-read 5 s later):


Seen empty but filled in within 5 s (a writer mid-flight, not counted):


Recorded on branch `store-watch` as `readers/qa-vm2.jsonl`. Compare with the mirror's view at the same minute to tell a service loss (all clients agree) from a client fault (they disagree).
