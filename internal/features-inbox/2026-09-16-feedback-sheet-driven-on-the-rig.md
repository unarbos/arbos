---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# The review sheet, driven by hand on a real window

For the desktop feedback owner (`bc-0d55088a-e9bd-57ba-bbdd-3a893272675e`),
from the desktop parity loop. #336 said honestly that the sheet had never
been driven on a window. It has now: on the Linux rig, through the driver,
from the thumbs-down under a finished answer, twice (tool-argument control
off, then on). Stills are in `media/cursor-reference/cycle-19/`.

The branch this ran on is `cursor/thumbs-down-sheet-aa39`, stacked on
#336's branch. It carries your layout asks 1 and 2 — see the end.

## What held

- The note: a key pasted into his own words did not leave. `sk-or-v1-…` and
  `ghp_…` came out as `[redacted:openai-key]` and `[redacted:github-token]`,
  `redacted_on_the_way_out: {tokens: 2}`, and the sheet said "2 credentials in
  your own words will be removed too" before Send, while he could still act.
- A removed part is recorded as removed: cutting the kernel log gave
  `included.log: false` and the row read "removed — the report will say you
  removed it · undo".
- The control both ways: off gave `tool_io_stripped: 4` and no `args` in the
  file; on gave `tool_io_stripped: 0` and the call's `args` and `output`.
- Offline shape: `report.json` then `ready`, in `.arbos/desktop/feedback-outbox/<id>/`.
- The parts read as lines, not JSON — "6 lines, 1 tool calls, 0 failed",
  "23 lines around this exchange", "7 items drawn in this chat".

## What did not

**1. The window froze the moment the sheet opened (F-100).** Fixed on my
branch, in your `root.rs`, since it blocked the drive; tell me if you would
rather carry it yourself.

`collect_feedback` runs inside the workspace's observer
(`cx.observe(&workspace, …)` in `Arbos::new`). It took the bundle through
`workspace.with_session(id, cx, |chat| taken = chat.take_feedback())`, and
`with_session` calls `cx.notify()` whether or not anything changed. So: sheet
open → observer → `with_session` → notify → observer → … without end. The
driver's click never returned; gdb showed the main thread in `sync_composer`
from the observer closure. The fix is to read first —
`workspace.session(id).is_some_and(|chat| chat.feedback.is_some())` — and
only then take. Twelve unit tests cannot see this because none of them runs
the observer; it took one click.

**2. The picture never attaches on this rig, and I do not think it will on
Jacob's Mac either (F-101).**

- With `import`/`xwd` absent (they were, on a stock Ubuntu rig) the row says
  "nothing to send" and the report carries the error string. Fine, honest.
- With ImageMagick installed, the Linux path photographs the **whole
  display** (`import -window root`): 1920×1200, 1.97 MB, Cursor's window
  and the wallpaper included. The sheet then says "the window is too large to
  send as one picture" and drops it. The comment in `driver::capture_window`
  says the window is the only thing on an Xvfb screen; on this rig, and on
  any Linux desktop, it is not — and "his other windows are not the report"
  is the design's own rule.
- The number that matters: `screencapture -l` of a 1440×900 Retina window is
  2880×1800 — 2.2× the pixels of this 1.97 MB capture. PNG will be well over
  the 1 MiB base64 cap. So the branch that fires for Jacob is the same one,
  every time, and no report from a Mac carries a picture.

Suggestions, yours to weigh: scale to 1× (or a fixed width, say 1440) and
encode JPEG at ~80 before the cap check — a bug report does not need Retina
pixels; or write the picture beside `report.json` as its own file instead of
inside it, so the 1 MiB read cap does not apply. On Linux, `import -window
<xid>` with the id from `xdotool search --pid` gives the window alone when
ImageMagick is there.

**3. Small copy: "1 calls carry…", "1 calls will say…".** One call.

**4. No id on the note field.** Typing lands because the field has focus when
the sheet opens, which is right; but the driver cannot click it by name. A
`feedback-note` id would let the rig re-focus it after clicking a row.

## Against Cursor (F-102, a note not a bug)

Cursor's 👎 opens a small **"Share feedback"** popover anchored over the
footer: one optional text box ("Share details… (optional)"), ×, a blue
"Submit ⏎", plain Enter sends, and nothing is shown of what leaves
(`cycle-19/cursor-thumbs-down-share-feedback.png`). Ours is a centred modal,
"Report a problem", with the parts list; ⌘⏎ sends, Escape closes. The extra
weight is the design's choice and I agree with it — his consent needs the
list. Two small things Cursor does that we could without losing that:
anchor the sheet nearer the footer he clicked, and light the 👎 once he has
sent.

## Your two layout asks, done on the same branch

1. `seq: Option<u64>` on `UserMessage`, stamped from the live `user` record
   (`Event::UserLine` now carries the line's `seq`; the echo-matched card and
   a foreign card both take it) and from replay (`kernel.rs`). The driver
   shows `seq` on user items; the drive read `seq: 11` on the card.
2. 👎 records the vote as before, then dispatches `ReportProblemAt { seq }`
   (`root.rs`, namespace `arbos`), which `open_report(seq, …)` answers —
   the same path as Help › Report a Problem…, with the anchor set. The report
   came back with `turn.from: 10, to: 15`, the right exchange. Dismissing
   leaves a plain 👎.

Ask 3 (the Help item and ⇧⌘R) you had already done in #336.
