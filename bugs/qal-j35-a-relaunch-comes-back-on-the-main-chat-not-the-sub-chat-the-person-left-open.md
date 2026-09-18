# qal-j35 — a relaunch comes back on the main chat, not the sub-chat the person left open

- **status**: open (product)
- **found**: 2026-09-18 12:13, triaging cycle 8's desktop-step breaks
- **kernel**: `arbos-kernel 0.2.0 d2a807e48423 protocol 1`; app at `2301abd291c0`
- **control**: `mt-24-relaunch-restores-active-tab`
- **rollout**: `20260918T121304Z-mt-24-relaunch-restores-active-tab`

## What happens

Open two sub-chats, leave the second active, quit the window, relaunch it. The window comes back
with the **project's main chat** active, not the sub-chat that was open.

```
agent_before:      chat-1789733586900     (the sub-chat the person left active)
agent_after:       root                   (the project's main chat)
restored_after_s:  None                   (never matched, across 20 s of polling)
```

Present in every cycle from 2026-09-17 18:18 to now — five in a row, same outcome each time.

## What made this hard to see, and why the old red was not evidence

The scenario had been asserting on the **session id**:

```python
active_before = d.app.state().get("active_session")   # 3
... quit, relaunch, sleep 3 ...
active_after = d2.app.state().get("active_session")   # 2
expect(str(active_after) == str(active_before))
```

Two faults in that, and both had to be cleared before the red meant anything.

**A relaunch renumbers sessions.** Recorded from the window's own list:

| before quit | after relaunch |
|---|---|
| id 1 → `root` | id 0 → `root` |
| id 0 → `root` | id 1 → **`chat-…679`** |
| id 2 → `chat-…087` | id 2 → `root` |
| id 3 → **`chat-…679`** (active) | id 3 → `chat-…087` |

The chat that was id 3 comes back as id 1. So `active_after != active_before` was true whatever
the app did, and the assertion would have gone red on a **correct** restore too. It sat beside a
real fault and named the wrong thing — the `qal-j27` lesson asked of identity rather than of whose
inbox to read.

**And it slept past the question.** `time.sleep(3)` then one read asserts the window finishes
restoring within three seconds, which nothing promises (review rule 7). Replaced with a poll to a
20-second bound that records how long the restore took, so a genuine slowdown stays visible.

With both cleared the finding survives: compared by `agent_session`, which is the chat's own
identity and does survive the relaunch, the active chat is `root` where it was
`chat-1789733586900`, and 20 seconds of polling never changes it.

## Suspected location

The active session looks to be restored by **position** while the positions are rebuilt in another
order — the renumbering table above shows the list is not reconstructed in its previous order, so a
stored index points at a different chat. Storing the agent id instead of the index would fix both
the restore and the renumbering surprise.

## Why it matters

A person with a project open and several sub-chats leaves the one they were working in. Every
relaunch puts them back in the main chat, and on a busy project the sub-chat they want is not
obvious from the tab strip. Nothing is lost, so this is irritation rather than damage — but it is
irritation on every single launch.

## Cross-reference

Found in the same pass as `qal-j33` and `qal-j34`, both rig faults in cycle 8's break list. This
one is the product's, and it is worth noting that it was the *third* of three reds examined that
afternoon: two dissolved on inspection and one survived. The two that dissolved are why this one is
worth believing.
