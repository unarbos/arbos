# U-08 thumbs + relative time under a turn — QA note (features agent, 2026-09-13)

Branch `cursor/turn-feedback-b027`, base `rust`. Parity report row 8.

## What it does

Under every settled answer, after the copy and fork icons: **thumbs up**, **thumbs down**, and a faint relative time ("Just now", "2m ago", "3h ago", "Yesterday", "3d ago", then the date).

- A vote is stored on the turn's prompt (`UserMessage.feedback: Some(1|-1)`), so it survives reopening; the chosen thumb paints in the accent colour; clicking it again clears it.
- Each vote is also appended to `.arbos/agents/<id>/feedback.jsonl` in the place (`{ts, turn, vote, answer}` with the answer's first 200 chars) so QA and the kernel can read it — the kernel does nothing with it yet.
- The time is the prompt's `sent_at` (unix ms, set when the message is created; on replay from the transcript's `user` event `ts`). Older records without it show no time.

Element ids: `vote-up-<chat>-<turn>`, `vote-down-<chat>-<turn>`, `turn-time-<chat>-<turn>`.

## Attack ideas

1. Vote, restart the desktop: the thumb is still lit (saved record); `feedback.jsonl` has one line, not two.
2. Vote up then down: the record flips; the file gets both lines (an audit trail, by design).
3. Remote place (host set): the file write is skipped (no local `.arbos`); the vote still shows.
4. Relative time never refreshes on its own: a chat left open shows "Just now" until something repaints. Note; a 60 s lease would fix it.
5. Transcript with `ts: 0` (old kernel): no time shown, no "56y ago".
6. Very old turn: the date ("Sep 3") — Cursor shows dates the same way.
7. Driver: `items[].kind == "user"` gains `feedback` and `sent_at`.
8. Fork from a voted turn: the vote does not carry to the fork (the record copies items — check whether it should).
9. Vote on a turn that ended with an error strip: the footer still shows (thumbs make sense for a failure too).
10. Dark mode: the lit thumb uses `accent`; the resting ones `text_faint` — contrast check.
