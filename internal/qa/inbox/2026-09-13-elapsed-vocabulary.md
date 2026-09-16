# U-06 elapsed time and thought vocabulary — QA note (features agent, 2026-09-13)

Branch `cursor/elapsed-vocabulary-b027`, base `rust`. Parity report row 3.

## What it does

- The settled fold line above an answer reads **"Worked 21s"** (Cursor's format; was "Worked" alone, or "Worked for 15m 20s" when tool seconds were known — they usually summed to 0).
- The number is the turn's wall time: live from the flight clock, stored on the user message (`UserMessage.worked_secs`) when the turn ends, and on replay computed from the transcript's event timestamps (`turn_complete.ts − user.ts`).
- **"Thought Ns"** on replayed transcripts: the thought's seconds are the gap to the next event, so reopened chats show it too (live chats already did).
- Verb vocabulary unchanged: Exploring/Explored with file and search counts already matched Cursor.

## Attack ideas

1. A turn that ended with an error (TurnDone Err): still stamps `worked_secs`; the fold line shows it above the error strip.
2. A turn interrupted by the user: `worked_secs` stamped at stop; check it is not stamped twice by a late `turn_complete` replay.
3. Replay of a transcript written by an older kernel with `ts: 0`: no number (guard: any non-positive delta → None), fold shows "Worked".
4. Very long turn (2h): `since()` prints "120m 5s"; Cursor prints "2h 0m". Note; fix if the reviewer wants hours.
5. Sub-second turn: "Worked" alone, no "0s".
6. A thought followed at once by a tool: "Thought 0s" — hidden when 0 (thought() already hides `Some(0)`? verify).
7. Chat reopened while a turn is running: the live clock resumes from `flight.at` — check the replay does not stamp the open turn.
8. History files saved by the desktop (`Record`) now carry `worked_secs` on user items; older records lack it → `None` → the fold falls back to summed tool seconds as before.
9. Driver: `items[].kind == "user"` gains `worked_secs`; assert `Worked Ns` matches within ±1 s of the driver's own timing.
10. Parity suite screenshots `p1-question-end.png` etc. should now show "Worked Ns" above every answer.
