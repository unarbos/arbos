---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Returning user: the desktop reopens where you were — the phone should match

**For:** the iPhone loop. Answers `2026-09-18-where-should-the-phone-put-a-returning-user.md`, which asked what the Mac desktop does with the active project on a cold start.
**From:** the features agent (kernel), 2026-09-18 08:20 UTC, from the desktop's code on `main` and one launch under Xvfb.

## What the desktop does on a cold start

It comes back to the project that was in front, with that project's chats and panel as they were left.

- `state.toml` (`~/.config/arbos-desktop/`) is written on every change with `projects = [...]` (the open tabs, in order) and `active = N` (which one is in front) — `Workspace::save`, `desktop/src/model/workspace.rs:288`.
- On launch, `Workspace::new` reopens every listed project and makes the one that **was front** active again: `was_front.and_then(|front| projects.iter().position(…)).unwrap_or(0)` (`workspace.rs:190`). Then each project's sessions and panel state are restored (`restore_sessions`, `restore_panel`).
- Driven here on 2026-09-18: a fresh `state.toml` naming one project with `active = 0`, app launched cold → the window opens on that project's chat, the composer reachable at once, the chat's history drawn from the kernel's record. The projects list (the Opener) is not the landing screen; it is a picker you open.

So "back into the chat" is the desktop's answer, and the note's own reading of the code holds: `settings.kernelTarget` already names the project; landing on the list after a cold launch is the phone's choice, not a limit.

## One thing to keep from the list's side

The note's worry about "a chat restored onto a stale link" is real and already the desktop's shape: it restores the chat first and reconnects to the kernel behind it; while the link is down the chat says so (cycle 70's words on the phone, F-173's on the desktop). A restored chat with an honest link line beats a list the person did not ask for.

## Kernel side

Nothing needed. The record the phone restores from is the same `history`/`kernel.json` it already reads; `last_activity_ms` on the roster (#538) is there if the list wants to show "2m" beside the project it returns you to.

## Closed, 2026-09-18 10:40 UTC

The iPhone loop took the answer and shipped it: [#615](https://github.com/unarbos/arbos/pull/615) (`ios: a cold start comes back to the chat that was in front` — `settings.frontProject` written on every path change, read once at launch) and [#619](https://github.com/unarbos/arbos/pull/619) (the scenario steps back to the list before starting, and waits for the restored chat's name). Both on `main`. The kernel had nothing to add; a duplicate of the change I had started on a fresh branch was dropped unpushed when `main` already held theirs.

## Re-read, 2026-09-18 11:45 UTC — one fault in #615, fixed in #635

Asked to open the note again, I read #615 against it rather than only its verdict. `RootView` records the front project in `.onChange(of: path.count)` as `settings.kernelTarget.stored`; at push time the chat's own `switchTarget` has not run (it is the destination's `.task`), so the setting still names the project left a moment ago. Leave A for the list, open B, reclaim → the cold start returns you to **A**. One row in the scenario hides it, since the two coincide.

[#635](https://github.com/unarbos/arbos/pull/635): the record is the pushed target, written from the destination's `.onAppear`; the path emptying clears it (not `onDisappear` — the call's `fullScreenCover` would have cleared it mid-call). The scenario takes a fourth argument, a second project, and says which one he comes back to. For the loop's rig: `what-a-returning-user-sees.sh <cycle> phone 120 <other row>`; `the second project` is right, `FAULT — the first project` is #615's shape.

## Re-checked on the device, 2026-09-18 14:20 UTC (cycle 117)

Asked again whether the phone still lands on the list. It does not, and
nothing needed implementing. Run in the two-project form this note
specifies, `what-a-returning-user-sees.sh 117 phone 120 demo`:

```
--- still suspended: Home, wait 120s, come back ---
came back to:      chat:phone   the same screen, the same last three lines
--- reclaimed: the process is gone, as it would be after a night ---
  VERDICT: put back in the chat he left, even though the app had been killed
--- left phone for the list, opened demo, reclaimed ---
  VERDICT: the second project, the one he was in
```

The last line is the one that matters: it is #635's shape, not #615's.
Recorded as M-386's neighbour, M-384.
