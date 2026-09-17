---
cursor:
  subagentId: "bc-08d8261b-fea2-5075-9949-d45f6f9d4acc"
---

# iPhone loop — cycle reports

A copy of each cycle's closing report, in the order sent, so a report missed in the conversation can be read here. The ledger is `mobile-findings.md`; the rotation record is `mobile-coverage.md`.

## Cycle 37 report (00:31 and 00:46 UTC, 09-17)

Sent in two parts. `main` `55dd38bd` on the simulator against the ArbosLife hub with the phone token.

- Long history and older-lines paging on `phone` (1,265 lines): opens at the tail in 1.1 s; "Show 200 earlier lines" pages back and holds the row under it; a second page reaches the 16:xx turns; recorded (`media/mobile/cycle-37/recording-long-history-paging-20s.mp4`). M-120 (a scroll lock after paging) withdrawn — the harness swiped on the composer's inset.
- Several workers at once on `pod`: four workers, four Done lines, Agents 3 → 7, all seven in the sheet, kept across a reopen.
- Cold start 3.1 s (was 6 s). Background 8 s → chat as it was. Attachments: photo chip with a line.
- **M-121 fixed** (#365): the direct kernel and the roster's `phone` are one kernel; the list drew it twice. `hello.store` folds the pod row into its roster twin.
- **M-122 fixed** (#365): Settings' token edit put up iOS's "Save Password?" sheet every time; the fields are `.oneTimeCode` now. Token editing exercised for the first time: wrong token → rows Off, right token → rows back.
- Style: composer at rest and photo chip vs Cursor (`01-`), list after M-121 vs All Agents (`08-`), one-surface check vs desktop #359 (M-123: already so on the phone).
- M-119: `arboslife/demo` still served by an old image (hello without `git_sha`); the #357 rerun waits. M-124: second store loss of the mobile documents at 00:44, restored from the Mac mirror; guarded mirror script since.
- Mac cost ≈ $31 (35 h at $0.88/h).
- Next: the `demo` rerun when its hello carries a `git_sha`; otherwise the worker-chat pair.

## Cycle 38 report (01:30 UTC, 09-17)

Short cycle, started by hand on the coordinator's kick after the timer question.

- Cycle timer: alive the whole time (fired 00:00:16; the cycle-37 report followed at 00:31 and 00:46). Recreated it anyway with a rule that every cycle turn ends with a `## Cycle N report` message and a copy in this file, so a missed message can be found.
- `demo` journey: **not run** — the `demo` kernel is still an old image (M-125): the machine row says `b6e7098`, the kernel's own `hello` says otherwise. Evidence to the mesh worker; the rerun is the first item once `hello` carries a `git_sha`.
- Style pass vs the Mac's one-surface look (#359): consistent, nothing to change (M-126; `media/mobile/cycle-38/01-`).
- Call screen, both states, in the simulator (no microphone → "No microphone input."): `03-`.
- PR: #365 (M-121, M-122) still the cycle's PR; nothing new to fix from this half.
- Mac cost ≈ $32 (36 h at $0.88/h).
- Next: the `demo` rerun on the mesh worker's word; otherwise the worker-chat pair against Cursor's agent chat.
