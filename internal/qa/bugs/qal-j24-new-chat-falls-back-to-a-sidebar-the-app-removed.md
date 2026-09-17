# qal-j24: the desktop rig's `new_chat` looks for `new-subchat` before the panel it lives in is open, then falls back to a sidebar the app removed — so 16 scenarios a cycle have died since 15 September on an error naming the fallback

- Measured at: `arbos-kernel 0.2.0 b1c8e82a62b1` / the desktop app built from `main` `b1c8e82a62b1`, cycle `20260917T181835Z` on `qa-vm2`; the same failure in cycle `20260917T134605Z` and, by fingerprint `1f6f6064cd`, back to **2026-09-15 04:09**. Probes: `/tmp/handover/probe-project-0.py`, `probe-panel.py`, `probe-sheet.py` (kept with the rollouts).
- Class: a rig fault that hides the product. Every one of the 16 failures is *before* the scenario measures anything, and the message names the last thing tried rather than the reason — so it read as a harness error and nobody triaged it for two days.
- Feature: `internal/qa/desktop_scenarios.py` `Desktop.new_chat`, against the desktop layout decided 2026-09-13.

## What happens

`new_chat` tested for the leaf `new-subchat` and, if absent, did `hover("project-{ix}")` then
    10|`click("project-add-{ix}")` — the old left sidebar. The 2026-09-13 layout decision removed the sidebar
("No left sidebar at all"), so that branch cannot succeed on any current build. Its error,
`DriverError: move: no element matches 'project-0'`, is what the cycle log shows.

The first branch fails for a reason nobody had looked for. `new-subchat` is real — it is in the app at
`desktop/src/view/panel.rs:1204` — but it lives in the **right-hand panel**, and the panel is closed in a
fresh window. Measured by watching one window rather than reasoning about it:

| moment | elements | `new-subchat` |
|---|---|---|
| as launched (0.09 s after `launch()` returned) | 33 | absent |
| through 30 s of polling | 33–34 | **never appears** |
| after `click("permissions-skip")` — the first-run permissions sheet | 26 | absent |
    20|| after `click("toggle-panel")` | **42** | **present, reachable** |

So it is not a readiness race: the tree is fully populated at 0.09 s and the fallback fired at 1.7 s. The
control simply is not mounted until the panel is open, and nothing in the rig opened it.

## Why it stayed invisible for two days

1. **The error named the second branch.** "No element matches `project-0`" points at a sidebar, so the
   reader looks for a sidebar problem. The cause was a closed panel.
2. **A `driver-exception` reads as a harness error**, so a cycle showing 16 of them looks like a broken
   rig rather than 16 unmeasured scenarios. The draft `1f6f6064cd` still says `Suspected location: (fill
   in)` two days on.
3. **A fallback that cannot succeed is worse than no fallback.** It converts "the control is not where I
    30|   look" into a confident, wrong statement about a different control. With the branch removed, the
   helper now raises a message naming `desktop/src/view/panel.rs` and asking whether the control moved.

## The fix

`new_chat` opens the panel when `new-subchat` is not mounted (`click("toggle-panel")`, then
`wait_element("new-subchat", reachable=True)`), clicks it, and has no sidebar branch. If neither leaf is
there it raises with what to check, rather than trying something that cannot work.

Verified on `desktop-rapid-session-switch`, one of the 16, at `80e6994280f8`: the `project-0` break is
gone and the scenario creates **six chats** — where it had previously not created one. It now fails on two
questions about the product that nobody had reached:

    40|- `driver-gap: no clickable session row` — the six sub-chats have no `tab-<n>` row; `session_element`
  looks in the tab bar, and after the layout change sub-chats belong to the panel. Harness, almost
  certainly; needs the same treatment as `new_chat`.
- `state:chat-folders: 6 chats created but folders are ['root']` — the window holds six sessions while
  `.arbos/agents/` holds only `root`. Two readings and the evidence does not yet choose: the kernel may
  create a chat's folder lazily on its first turn (the scenario never sent to them), or the window is
  showing rows the kernel has no record of — **which is the shape of Jacob's own 2026-09-17 report**, and
  is why it is written down here rather than guessed at.

Those two are the next thing to take apart, and they exist as questions only because the selector was
fixed. That is the cost of a rig fault that fails early: it is not 16 red scenarios, it is 16 scenarios
    50|that measured nothing for two days.

## Regression check

The 16 scenarios themselves, and `new_chat`'s own raise if the control moves again. The property worth
adding beside it, from the probe: **a helper that has two ways to reach a control asserts that at least
one of them exists**, rather than trying them in order and reporting the last failure — the same rule as
`qal-j19`'s first-match family, in the harness.

One thing the probes taught about themselves: the second probe clicked candidate elements in sequence and
let an opened settings sheet change the window under it, which produced `move: no element matches
'toggle-panel'` — an apparent driver bug that was my own ordering. The third probe did one action at a
time from one window and the effect vanished. A script that depends on which of two things happens first
    60|invents faults as readily as it hides them.
