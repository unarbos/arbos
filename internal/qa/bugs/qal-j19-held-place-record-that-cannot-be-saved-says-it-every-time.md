# qal-j19: when `runtime/place-held.json` cannot be saved, "once" becomes "every relaunch" — and after five minutes, an error-level line on every relaunch

- Measured at: #441 @ `b5b24dba` (`arbos-kernel 0.2.0 b5b24dba7b16`), scenario `lk-02-held-record-in-a-read-only-runtime-folder`, rollout `internal/qa/rollouts/20260917T115*-lk-02-…`. `lk-03` (holder gone → record cleared → new holder gets its own first line) and `lk-01` (a real 5.5-minute relaunch loop) pass on the same build; this is the record's own failure mode, not the ordinary path.
- Class: the qal-j09 shape one layer down — a write that fails silently (`HeldRecord::save`: `let _ = std::fs::write(...)`, `let _ = rename`) and a later step that trusts the record as saved. Misreport at volume: the very flood #441 exists to stop, at error level.
- Feature: the held-place record (#441, `serve.rs` `HeldRecord`, `say_held`).

## Two shapes, both with `runtime/` read-only (a folder on a full disk, wrong ownership after a copy, a mount gone read-only)

**(a) No record yet, folder unwritable.** Six relaunches against a held place → the full *"another kernel already serves … pid, build, url …"* line on stderr **6 of 6 times**, and `refusals` reads 1 each time. Nothing says the record could not be kept. The 1411 lines are back, each of them the long one.

**(b) A record that exists but cannot be updated**, six minutes old (`first_ms` and `last_said_ms` 360 s ago, `escalated: false`). Six relaunches → *"a person needs to look: … has been held for 361s …"* at **error level 6 of 6 times**. `escalated` can never be saved, so the ten-minute cap on repeats never applies, and every relaunch from then on is an error-level line.

Ordinary path, for the control: with `runtime/` writable the same six relaunches give one full line then `place already served by pid N (Ns; said in full in kernel.log)` — as designed.

## What we expect

`save` returns whether it saved. When it did not, say so once on stderr in the same breath (*"… (could not keep runtime/place-held.json: <reason>; this line may repeat)"*) and prefer the short form on later relaunches by some means that does not need the record — the simplest: if the record cannot be written, treat a lock file older than a minute as "already said in full" and emit only the short line, and never the error-level line more than once per process lifetime. A write that fails must not be indistinguishable from a write that worked; that is the whole of the qal-j08 family.

## Regression check

`lk-02`: passes when, with `runtime/` read-only, the full line goes out at most once in six relaunches and the escalation at most once in six.

## Verified on the same build (the parts that work)

- `lk-03`: three refusals, the holder killed, the next start serves and logs `place_freed` (*"the place is free after 2s held by pid …; serving …"*) and removes the record; a new holder's first refusal writes a fresh record keyed on its pid with `refusals: 1` and the full line again. No permanently-refusing state inherited.
- `lk-01` (real loop, 5.5 min, one relaunch every 2 s): see the design doc's line for the counts.
