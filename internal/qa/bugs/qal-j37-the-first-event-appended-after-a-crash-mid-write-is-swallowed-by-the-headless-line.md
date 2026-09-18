# qal-j37 — the first event appended after a crash mid-write is swallowed by the headless line

- **status**: **closed — fixed on `main` by [#646](https://github.com/unarbos/arbos/pull/646) (`cecd48e1`), verified 2026-09-18 13:38 with both arms.**
- **found**: 2026-09-18 12:55, taking after-failure states nobody had staged
- **kernel**: `arbos-kernel 0.2.0 a8678ac16636 protocol 1` (today's `main`)
- **control**: `pl-01-a-turn-after-a-crash-mid-append-is-not-swallowed-by-the-partial-line`
- **rollout**: `20260918T125558Z-pl-01-…`

## What happens

`append_events` builds one buffer ending in a newline and writes it with `O_APPEND`
(`arbos-core/src/files.rs:537`). If that write fails part-way it calls `drop_partial_line` to cut
the headless remainder back off, and the comment there names the hazard exactly:

> A write that failed part-way (disk full, size limit) leaves the head of a line with no newline.
> Every reader skips it, but it also swallows …

**That repair runs only in the process whose own write failed.** `drop_partial_line` has exactly
one caller — `files.rs:581`, the error arm of that same `write_all` — and nothing repairs a
transcript at startup. A crash leaves the identical partial line with nobody to run it: SIGKILL, a
lost machine, a power cut, the OOM killer.

The next kernel then appends onto the headless line. `load_transcript` skips unparseable lines
silently (`files.rs:629`, an `if let Ok(...)` with no `else`), so the combined line — and the event
inside it — is gone.

## Staged and measured

Three whole events, then a fourth line cut off mid-string with no trailing newline, which is what a
crash leaves. Then one turn runs.

| | before the kernel started | after the turn |
|---|---|---|
| readable events | 3 | 6 |
| unparseable lines | 1 | 1 |
| bytes | 231 | 617 |

The surviving unparseable line, in full shape:

```
{"ts": …, "kind": "assistant", "text": "hal{"ts":…,"kind":"wake","wake":"user","text":"AFTER-CRASH-25125: …
```

The new `wake` event has been run onto the headless line. Every reader skips the whole thing, so
that event does not exist as far as the product is concerned.

## Precisely what is lost, because the first reading was too generous

`append_events` writes a **batch** in one call. The headless line therefore takes the batch's
**first** event and the rest land on their own lines whole. My first assertion looked for the typed
marker *somewhere* readable and passed — because the `user` line of the same batch survived even
though the `wake` did not. The honest property, which the control now asserts, is that **no** event
appended after the crash ends up inside an unparseable line.

So the loss is one event per crash, not a whole turn — but which event depends on what is appended
first, and two of the candidates matter:

- a **`wake`**, as measured here. `check_two_writers` — #450's double-serving detector — tracks open
  wakes to decide whether two kernels served a place. A missing wake is a missing open, so a crash
  can quietly make that detector read a place as sound. The detector `ds-01` verifies is only as
  good as the record it reads.
- a **`user`** line, when the person's words happen to be first in the batch. Then it is their words
  that are gone.

## Suspected fix

Cut a headless last line before appending, not only when this process's own write failed. The
mechanism already exists and is already correct — `drop_partial_line` does exactly this — so the
change is where it is called from: once on opening the transcript for append, rather than only in
the error arm. A note on the transcript saying a partial line was dropped would also turn a silent
repair into a visible one.

## How it was found

Not from a break. `run.py:855` has a check — `transcript-corrupt: partial line(s) left in the
transcript` — and the place checker has `state:transcript-bad-lines`. The loop has detected this
residue for days and **nothing in the library ever created it**: a detector with no probe. Surveying
which failure modes the 303 scenarios actually inject turned up 53 uses of `chmod` and 13 of
`SIGKILL`, and zero that leave a file half-written. The gap between what the loop can notice and
what it ever causes is where this was sitting.

Two other injections are still absent and worth the same treatment: a write that fails for want of
space (`ENOSPC`, which is the *other* half of `drop_partial_line`'s own comment and reaches it by
the supported path), and a shifted clock — the kernel reads `ARBOS_NOW` for exactly that purpose and
no scenario sets it.

## The guard is correct; it only covers one of the two ways this state arises

Added 2026-09-18 13:20, after staging the other cause `drop_partial_line`'s comment names.
`en-01-a-write-that-runs-out-of-room-does-not-swallow-the-next-event` sets `RLIMIT_FSIZE` on the
kernel before exec and pads the transcript to just under it, so the next append crosses the ceiling
mid-write — the "size limit" half of *"disk full, size limit"*. On
`arbos-kernel 0.2.0 f97bb3487540 protocol 1`:

| | |
|---|---|
| kernel met the ceiling | yes — stderr names it, file stopped at 65,451 of a 65,536-byte cap |
| kernel still serving afterwards | yes |
| readable events before / after | 145 / **148** |
| unparseable lines afterwards | **0** |

So on the path it was written for, the guard does exactly its job: the failed write is cut back, the
next append lands whole, and nothing is lost. `en-01` passes.

That makes this bug narrower and more actionable than first written. It is **not** that
`drop_partial_line` is wrong — it is right, and proven right. It is that the state it repairs has
two causes and the guard sits on only one of them:

| how the headless line appears | who repairs it |
|---|---|
| this process's own `write_all` fails (disk full, size limit) | `drop_partial_line`, at `files.rs:581` — works, `en-01` |
| the process is gone mid-write (SIGKILL, power, OOM) | **nobody** — `pl-01` |

The fix therefore needs no new mechanism, only a second call site: cut a headless last line when
opening the transcript for append, not only when this process's own write failed.

## A note against myself

`pl-01`'s own failure message indexed `swallowed[0]` while asserting `not swallowed` — so it would
have raised `IndexError` on the day this bug was fixed, and the green would have arrived looking
like a fresh `driver-exception`. That is review rule 8, which this loop wrote this morning out of
`qal-j29`, broken in a scenario written hours after it. `en-01` did it too and was caught on its
first run by the same rule's symptom. Both are guarded now, and the two other `[0]`/`[-1]` sites in
this module were checked: `fm-02`'s sits inside an `if said:`, and `uw-04`'s comprehension yields
nothing on an empty list, though it is now also guarded against a blank line.

Writing the rule is not the same as having it.

## Both causes now staged, and what the second one cannot tell you

`drop_partial_line` names two causes; `en-01` reached the arm by `RLIMIT_FSIZE` and
`deploy/en02-enospc-probe.sh` reaches it by a genuinely full filesystem — a 512 KiB tmpfs mounted in
a user namespace, the place on it, the transcript padded to the ceiling, then a turn. On
`arbos-kernel 0.2.0 f97bb3487540 protocol 1`:

| | |
|---|---|
| filesystem | 512 KiB, 0 bytes free before the turn |
| readable events before / after | 766 / **766** |
| unparseable lines before / after | 0 / **0** |
| kernel said no space | yes |
| kernel alive afterwards | no — it exited |

So a full disk leaves the transcript exactly as it was, says why, and corrupts nothing. The property
holds on this cause too.

**What it cannot distinguish, and I would rather say so than imply otherwise.** With zero bytes free
the write can fail at byte 0, in which case there is no headless line for the guard to cut — the
easy case, not the guard working. I tried to leave a sliver smaller than one event and could not:
tmpfs allocates by page, so "200 bytes remaining" is zero *available blocks* and `statvfs` reports 0
either way. From outside the process, "the write never started" and "the write was cut back" look
identical: the file length is unchanged in both.

`en-01` is therefore the load-bearing evidence that the guard actually cuts — there the ceiling fell
inside the buffer, three events landed whole afterwards, and no unparseable line remained. `en-02`
adds that the other cause reaches the same place without damage, and that a kernel which cannot
write says so before it goes.

One thing worth a second look by someone who owns that area: **the kernel exits when the filesystem
is full.** It reports the reason, so it is not silent, and a kernel that cannot record anything
arguably should stop. But it is a place stopping on a condition a person can fix, and whether the
window says so usefully is a question this probe does not reach.

## Closed: #646 verified, both halves

`#646` (`cecd48e1`) added exactly the missing call site — `drop_headless_tail` in `files.rs`, swept
by `repair_headless_tails` at kernel start "before anything appends" — and made the cut speak.
Verified with `pl-01` on `cecd48e1bd76` against `f97bb3487540`, which is the commit line just before
it:

| | `f97bb3487540` (before) | `cecd48e1bd76` (with #646) |
|---|---|---|
| readable events, before → after the turn | 3 → 6 | 3 → **8** |
| unparseable lines afterwards | **1** | **0** |
| the turn's event swallowed | **yes** | no |
| the cut said on the transcript | no | **yes** |

The notice is worth quoting, because the last clause is the part that makes the record accountable:

> The kernel that wrote this record before ended in the middle of a line (55 bytes, the head of one
> event). That half line was dropped so everything from here on reads whole; the event it began was
> lost with that kernel, not now.

It names the byte count, says what was dropped and why, and puts the loss on the crash rather than
on the repair. `pl-01` now asserts both halves — nothing swallowed, and the cut said — so it breaks
twice on the build before and passes on the build after.

## Two mistakes of mine in this control, for the record

Adding that second assertion took two tries, and both failures were mine rather than the product's:

- the first referenced `evs`, which this scenario does not define — it reads through `read_lines()`
  into `good1`. `NameError`, caught on the first run. Reading the surrounding code before naming a
  variable would have cost nothing;
- the break message then read *"a half-written line was cut back at start and root's transcript does
  not say so"*, which is untrue on the build before #646, where **no cut happens at all**. A message
  that asserts the wrong thing about the build it fired on is `qal-j34`'s and `qal-j36`'s fault in
  miniature, and it was in a file I wrote after filing both. It now says which of the two states it
  cannot tell apart and points at the break above for the answer.

Earlier in the day this same scenario's failure message indexed `swallowed[0]` while asserting
`not swallowed` — it would have raised `IndexError` on this very run, the moment the fix landed, and
the green would have arrived disguised as a `driver-exception`. That was guarded a few hours before
#646 merged, by luck of ordering rather than by discipline.
