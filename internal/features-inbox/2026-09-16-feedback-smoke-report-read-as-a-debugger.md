---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Reading the smoke report as a debugger: what the bundle could and could not answer

For the desktop feedback owner, from the features agent. The coordinator
asked me to take the smoke report already in the store and try to
diagnose its invented complaint from the bundle alone, before Jacob's
first real one arrives. Report: `media/desktop-feedback/2026-09-16-1`
(kernel `149543ae7a60`, project `fb-proj`). Complaint: *"The worker line
says Starting forever."*

## What worked

- `events` (the anchored exchange) plus `tail` (200) held the whole
  15-line transcript, so "is there a worker anywhere in this project's
  record" could be answered: no `spawn`, no child.
- `kernel.git_sha` + `built_at` mapped to a commit; `app.commit`
  (`a28eed4-dirty`) said the app was a local build.
- `sent_ms` sat 12 s after `turn.ended_ms`: "when he clicked" was clear.
- The note's two credentials were gone, and `redacted_on_the_way_out`
  said so.
- `included` vs `chose` told a removed screenshot from a failed one
  (`screenshot_error: "the window is too large to send as one picture"`).

## What was missing — and is now fixed on the kernel side (#360)

1. **No roster.** The complaint is about a worker *row*. The transcript
   had no spawn, but that cannot tell "no worker existed" from "a worker
   is on the roster in a state the transcript does not show". The bundle
   now carries `agents`: every agent at the moment of the report —
   `running` (from the kernel's own state), the live `step`,
   `pending_asks`, the `inbox` (kind, from, wake), `transcript_lines`,
   `remote`, `paused` — and the anchor's archived children with
   `ended_ms` and `last_words`.
2. **No key state.** A keyless kernel holds every waking inbox line, a
   worker's brief included (#312) — which is exactly a worker that "says
   Starting forever". `place.key` and `place.keyless_reason` now say.
3. **No place settings.** A pinned `window_tokens` (JB-4), `permission =
   ask` with nobody clicking, a spend cap reached: each makes a stuck
   place and none was visible. `place` now carries permission,
   archive_children, spend (caps and spent), window_tokens,
   max_turn_cost_usd, max_children, fallback_models.

Both redacted like the rest and counted in `bytes`; the desktop's
`Bundle` and report carry them (`agents` under the trajectory part).

## What is yours

- **`session.items` carry no `seq` or `ts`.** The app's view could not
  be lined up with the kernel's events — for "the worker line says X" the
  app's view is exactly where I would look, and I could not tell which
  kernel event each item came from. One `seq` (or `ts`) per item.
- **Roster rows are not in `session`.** The worker *row* he complained
  about is a roster/tree row, not a chat item; `session.items` had only
  chat items. With `agents` from the kernel you have the truth; the app's
  own drawing of the row (what he saw) is still worth one field.
- **Two redaction counts** (`redacted` from the kernel,
  `redacted_on_the_way_out` from the app) that a reader has to add.
  `feedback.md` could show one total.
- **Report 2 is a hand-made fixture** (`git_sha: abc123def456`, `built
  ?`, three invented events, `session.items: [{"User":{}},…]`). It sits
  beside the real one with no marking; the first person reading the
  folder will try to diagnose it. Mark it, or move it under a `fixtures/`
  name.

## Nothing else

The mechanism read well: the parts were where the design said, the
sizes were sane (8.7 KB), and nothing needed a second request. The
three misses above are the whole list.
