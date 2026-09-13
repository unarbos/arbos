---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# For QA: sub-agent status lines + right task rail (U-01)

From the features agent. Branch `cursor/subagent-task-panel-b027` → `rust`. Parity report row 5 (Jacob's called-out gap).

## What I am building (desktop)

1. **Finished sub-agents stay.** A rust-kernel child used to be closed (archived) 6 s after its turn ended (`settle_delegate` → `reap_delegate`), which is why they vanished from the sidebar. Now only Go one-shot delegates are reaped; kernel children stay nested under the parent, marked done.
2. **Inline lines under the parent's status**, on the latest turn: one row per sub-agent — spinner + shimmering "Working  <title>", chat glyph "Asking", hollow circle "Waiting", green check "Done" (faint). With more than one child a caption says "2 of 3 sub-agents working". Click a row → the child's chat opens.
3. **Right rail "Tasks" section** (the existing context rail that showed browser/terminal surfaces): same rows with the same glyphs, header "Tasks · N working", the focused child highlighted. The rail now appears when the chat has surfaces **or** sub-agents; it is hidden under 1000 px window width as before.
4. **No raw ids in labels**: `[chat-1789… → user]` and `[draftachangelog…]` message headers now show the child's title (`ChatSession::who_label`), resolved through the parent's child list.

Data path: `Workspace::child_summaries(id)` → `ChatSession::children` (runtime only; refreshed before every transcript draw) → `transcript::children_lines`, `detail::context_panel`.

## How to exercise it

Parity suite prompt p3: "Use parallel sub-agents: one reviews math_utils.py for edge cases, one writes docstrings for every function, one drafts a CHANGELOG.md. Then merge their results." Watch: lines appear under the parent's status as the kernel spawns; the rail lists them; when each child finishes its line turns to a check and does not disappear; clicking a line or a rail row opens that child; the child's messages back to the parent are headed with its title.

## What could break — attack here

1. Ten children: the inline block gets tall; check the transcript still scrolls to the bottom and the composer stays put.
2. Nested grandchildren: only direct children are listed on each level. A grandchild's report to its parent (a child) is titled by that child's list, not the root's.
3. A child that is `Asking` (parks on `ask`): glyph and colour; answering it from its chat; the parent line updates within the 2 s poll.
4. Child renamed by the user (F2 / double-click in the sidebar): the line and rail must follow the new name.
5. Reap regression: a Go-kernel (legacy gateway) delegate should still vanish after its turn as before; a kernel child must not.
6. Window under 1000 px wide: the rail hides, the inline lines remain.
7. Focus: clicking a rail row while the parent is streaming — the transcript switches to the child; Back via the sidebar.
8. Restart the app with finished children on disk: they load as Done (turn_ended is runtime-only, so a restored idle child shows Waiting until the poll probes its tail — check what it shows and for how long).
