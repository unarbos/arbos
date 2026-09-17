---
cursor:
  subagentId: "bc-0d55088a-e9bd-57ba-bbdd-3a893272675e"
---

# In-app feedback: what I need from the chat view

For the layout worker (`bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39`), from the
desktop feedback owner. Jacob's ask: report a problem from the app, and an
agent picks it up automatically — the loop his phone already has through
TestFlight.

**I am not inventing a control.** Cursor puts turn feedback in the footer
under a finished answer, you already matched it
(`docs/cursor-parity-report-2026-09-12.md` row 8), and the thumbs-down is
in `transcript.rs` ~4035–4078 writing a vote through
`workspace::vote_turn`. That is the control. It just stops short: 👎
records a number and asks nothing.

I will build the sheet, the screenshot, the redaction, the outbox and
delivery. Three things I need from your side, smallest first. **Tell me if
you would rather I wrote any of them in your files — I will not touch them
without your word.**

## 1. The transcript `seq` on the chat item (the one real blocker)

The kernel's `feedback` frame anchors on a transcript `seq`
([#328](https://github.com/unarbos/arbos/pull/328)). `ChatItem` carries
`ix` and a tool `call_id`, but no `seq` — `acp.rs` ~912 has `event.seq` on
the wire and drops it.

**Ask:** keep it. `seq: u64` on `ChatItem::User` (and on `ChatItem::Tool`
if it is free) as the event lands. `vote_turn` already walks back to the
`ChatItem::User` that began the footer's turn (`workspace.rs` ~1893), so
with a `seq` there I can name the exact turn without guessing from the
index.

Without this I would have to re-derive the turn by matching text against
the transcript on disk, which breaks on two identical prompts.

## 2. Thumbs-down opens the sheet

Cursor's 👎 asks what was wrong. Ours should too.

**Ask:** on 👎, after the vote is recorded as it is now, emit one event I
can subscribe to — turn index and `seq` — and I will open the review sheet
over it. Keep the vote working when the sheet is dismissed: a plain 👎
must stay a plain 👎 for anyone who does not want to write.

If you would rather own the sheet, say so and I will hand you the bundle
call and the redaction helper and take only the delivery half.

## 3. One way in when no answer is to blame

Half of Jacob's phone reports are not about an answer — the composer was
hidden, the list said the wrong thing, the window drew badly (F1, F2, F6,
F12, F16 in `internal/mobile-feedback-log.md`). There is no turn footer to
click for those.

**Ask:** a "Report a problem…" item in the menubar (`menubar.rs` ~137–209)
and a shortcut. `cmd-shift-r` and `cmd-alt-r` both read as free against
the global list in `root.rs` ~240–306 — your call which, or neither if one
of them is spoken for. It opens the same sheet with no turn anchored, and
the report carries the latest turn instead.

Not a status-bar icon: nothing in the parity ledger records one in Cursor,
and the bottom-left is the gear and the version badge.

## What the sheet is, so you can see the seam

A centred card on a scrim, the `permissions_sheet.rs` ~168–182 pattern:
his words in a text field, then a row per part — screenshot, trajectory,
kernel log, versions — each with a thumbnail or a line count and an ✕ that
drops it. A line reading "2 credentials were removed" when the kernel's
count is non-zero. Send, and nothing leaves before Send.

I own that file. The design is landing at `docs/desktop-feedback-design.md`
and I will link this note from it.
