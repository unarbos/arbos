# qal-j24: the desktop rig's `new_chat` looks for `new-subchat` before the panel it lives in is open, then falls back to a sidebar the app removed — so 16 scenarios a cycle have died since 15 September on an error naming the fallback

- Measured at: `arbos-kernel 0.2.0 b1c8e82a62b1` / the desktop app built from `main` `b1c8e82a62b1`, cycle `20260917T181835Z` on `qa-vm2`; the same failure in cycle `20260917T134605Z` and, by fingerprint `1f6f6064cd`, back to **2026-09-15 04:09**. Probes: `/tmp/handover/probe-project-0.py`, `probe-panel.py`, `probe-sheet.py` (kept with the rollouts).
- Class: a rig fault that hides the product. Every one of the 16 failures is *before* the scenario measures anything, and the message names the last thing tried rather than the reason — so it read as a harness error and nobody triaged it for two days.
- Feature: `internal/qa/desktop_scenarios.py` `Desktop.new_chat`, against the desktop layout decided 2026-09-13.

## What happens

`new_chat` tested for the leaf `new-subchat` and, if absent, did `hover("project-{ix}")` then
`click("project-add-{ix}")` — the old left sidebar. The 2026-09-13 layout decision removed the sidebar
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
| after `click("toggle-panel")` | **42** | **present, reachable** |

So it is not a readiness race: the tree is fully populated at 0.09 s and the fallback fired at 1.7 s. The
control simply is not mounted until the panel is open, and nothing in the rig opened it.

## Why it stayed invisible for two days

1. **The error named the second branch.** "No element matches `project-0`" points at a sidebar, so the
   reader looks for a sidebar problem. The cause was a closed panel.
2. **A `driver-exception` reads as a harness error**, so a cycle showing 16 of them looks like a broken
   rig rather than 16 unmeasured scenarios. The draft `1f6f6064cd` still says `Suspected location: (fill
   in)` two days on.
3. **A fallback that cannot succeed is worse than no fallback.** It converts "the control is not where I
   look" into a confident, wrong statement about a different control. With the branch removed, the
   helper now raises a message naming `desktop/src/view/panel.rs` and asking whether the control moved.

## The fix

`new_chat` opens the panel when `new-subchat` is not mounted (`click("toggle-panel")`, then
`wait_element("new-subchat", reachable=True)`), clicks it, and has no sidebar branch. If neither leaf is
there it raises with what to check, rather than trying something that cannot work.

Verified on `desktop-rapid-session-switch`, one of the 16, at `80e6994280f8`: the `project-0` break is
gone and the scenario creates **six chats** — where it had previously not created one. It now fails on two
questions about the product that nobody had reached:

**Second: the scenario open-coded a selector the helper already owned.** `session_element` exists to find
a session's row; `desktop-rapid-session-switch` ignored it and did its own `startswith("session-")` — the
old sidebar's naming — so it reported `no clickable session row` for every sub-chat. The rows are
`panel-agent-<session id>` in the panel (measured: two sub-chats in a fresh window give `panel-agent-1`,
`panel-agent-2`, `panel-agent-3`). `session_element` now looks there, opens the panel when it is shut,
and has lost its old second pass — a loose match on any element whose id merely *contained* the session
number, which could have returned an unrelated element sharing a digit. The scenario calls the helper.
The project's duplication rule with a two-day receipt: the copy rotted while the function it copied from
was there to be called.

**Third, and it was mine rather than the product's: `state:chat-folders: 6 chats created but folders are
['root']`.** That looked like the shape of Jacob's own report — a window row the kernel has no record of
— and it was not. `Desktop.__init__` sets `PATH` to `dirname(cx.binary)`, which assumes a binary named
literally `arbos-kernel` sits there. I had pinned control builds as `kernels/arbos-kernel-<sha12>`, so
nothing on `PATH` was called `arbos-kernel`, every sub-chat failed to connect, and no agent was created.
The app said so, in the chat, where a user would read it:

    Notice(failed): connection failed: no kernel binary at arbos-kernel on this machine

Re-pinned as `kernels/<sha12>/arbos-kernel` — still immutable, now correctly named — and the same run
reports **six `chat-<ms>` agent folders, one per sub-chat, beside `root`**. So a ⌘N sub-chat gets its
kernel agent at once; the app's "nested here and nowhere the kernel can see" describes the moment before
it binds, not a lasting state. Two lessons: **pin a build by directory, not by filename**, because the
rig depends on the name; and a notice the app writes into the chat is evidence — it named my own fault
while I was preparing to file it against the product.

## What the 16 were worth

With both helpers fixed, five of the previously-dead scenarios at `80e6994280f8`:

| scenario | before | now |
|---|---|---|
| `desktop-rapid-session-switch` | `project-0` | **pass** — 6 rows, 30 switches, 0 failures |
| `desktop-fresh-place-no-notice` | `project-0` | **pass** |
| `desktop-kill-kernel-under-ui` | `project-0` | **pass** |
| `desktop-huge-transcript-scroll` | `project-0` | **pass**, 58.8 s |
| `desktop-user-message-card` | `project-0` | **break, and a real one**: `card-short-too-wide: the 'hi' card takes 0.84 of the column; it should hug its text` |

That last row is the point. A message card taking 84% of the column instead of hugging two characters is
a visible defect in the app, and it sat behind a stale selector for two days while the cycle log said
`driver-exception`. The cost of a rig fault that fails early is not 16 red scenarios; it is 16 scenarios
that measured nothing, and a real fault nobody could see.

## Regression check

The 16 scenarios themselves, and `new_chat`'s own raise if the control moves again. The property worth
adding beside it, from the probe: **a helper that has two ways to reach a control asserts that at least
one of them exists**, rather than trying them in order and reporting the last failure — the same rule as
`qal-j19`'s first-match family, in the harness.

One thing the probes taught about themselves: the second probe clicked candidate elements in sequence and
let an opened settings sheet change the window under it, which produced `move: no element matches
'toggle-panel'` — an apparent driver bug that was my own ordering. The third probe did one action at a
time from one window and the effect vanished. A script that depends on which of two things happens first
invents faults as readily as it hides them.
