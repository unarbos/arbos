# K-13 attach replay + token deltas — QA note (features agent, 2026-09-13)

Branch `cursor/attach-replay-deltas-b027`, base `rust`. From the iPhone client work (PR #5).

## What it does

On every new connection to the kernel socket, in this order:

1. `{"type":"hello","protocol":1,"kernel":"<version>","tail":200,"focus":"<agent id>"}` — what the kernel speaks and how much history follows.
2. `snapshot` (tree, focus, budget) and one `plan` frame per agent — as before.
3. **Replay** of the focused agent's transcript tail: up to 200 `replayed` frames (`{"type":"replayed","agent":…,"event":{…,"seq":n}}`, each with its line number), then `{"type":"history_end","agent":…,"from":s,"to":e,"total":n}`.
4. Clients ask for more with `{"type":"history","agent":"root","since":120,"limit":500}` → `replayed` frames with `seq > since` (oldest first; `since: 0` = from the start; `limit` ≤ 2000), then `history_end`. Unknown agent → `history_end` with `total: 0`.

Live text: per streamed chunk the kernel now sends `{"type":"assistant_delta","agent":…,"text":…}` and `{"type":"thinking_delta",…}` instead of full `assistant` / `thinking` events with an empty `seq`. The per-step `assistant` event still arrives from the transcript tail (with `seq`), so an older client keeps working and a new one can ignore it (or replace its accumulated deltas with it).

`Event.seq` is on the wire whenever it is non-zero: a frame with `seq` is a transcript line; without, it is live.

## Attack ideas

1. Attach mid-turn: the replay tail ends before the live chunks; check no assistant text is doubled (deltas have no `seq`; the step's line has one; the phone's `ChatStore` should replace-or-skip on the seq'd line).
2. `history` with `since` beyond the end: `history_end` with `from == to == since`, no events.
3. `limit: 100000`: clamped to 2000.
4. A transcript of 50 000 lines: attach replays 200 (the last); `history since:0 limit:2000` pages from the start — measure the socket cost of a 2000-event page (some events carry 100 KB tool bodies).
5. Two clients attached: deltas reach both; `history` replies go only to the asker.
6. `history` for an agent whose transcript is being written right now: lines are read from disk; a half-written last line is skipped (`load_transcript` skips bad JSON).
7. Desktop: still streams (it maps `assistant_delta` → chunk); reopen a chat shows history from the file, not the socket — unchanged.
8. The tailed per-step `assistant` event arrives ~200 ms after the last delta: a client that appends both shows the text twice. That is the phone's decision; the PR body says what to key on (`seq` present).
9. `hello.focus` when `.arbos/focus` is missing → `root`.
10. Old kernel + new phone: no `hello` → the phone should fall back to `history since:0` (which an old kernel ignores) — nothing to do here, note for the phone.
