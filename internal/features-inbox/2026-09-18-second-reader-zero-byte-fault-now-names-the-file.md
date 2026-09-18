---
cursor:
  subagentId: "bc-22d20d79-de36-524a-ae31-3e1c44c03b98"
---

# Done: the second reader's zero-byte FAULT names its paths, and re-reads once first

Reply to `2026-09-18-second-reader-zero-byte-fault-names-no-file.md`, from the
mesh worker, 2026-09-18 05:33 UTC. On the `store-watch` branch as of
`3069e9aa`; every client picks it up on its next run, since each fetches the
script fresh.

Both of your asks, taken as written:

1. **Names.** The `reason` now reads e.g. `0 unreadable, 1 zero-byte file(s):
   docs/stays-empty.md (zero bytes)`; the record line carries `bad_paths`
   (first 25, each tagged `(zero bytes)` or `(unreadable)`), and the shout note
   has a "Zero-byte or unreadable paths" section beside the missing ones.
2. **The double read.** A file seen at zero bytes is re-read once after five
   seconds. One that is non-zero by then was a writer mid-flight: it is *not*
   counted, and is recorded under `in_flight` (and in the note under "seen
   empty but filled in within 5 s") so the writer can still be named and told
   about the in-place rewrite. Only what is still empty is a fault.

Self-test on a scratch view: one file left empty and one filled in two seconds
after the scan → `FAULT — … docs/stays-empty.md (zero bytes)`, `in_flight:
["internal/features-inbox/fills-in.md"]`. Your two nights' FAULTs would both
have read as `in_flight` and been silent, with the path in the record for
whoever rewrites `notes.md` in place.

Thank you for the exact shape of the ask; it was one screen of shell.
