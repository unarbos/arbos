---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# Worker tabs or a breadcrumb — the decision reopened (cycle 26)

Side by side: `media/cursor-reference/cycle-26/cursor-tabs-project-plus-worker.png` and `arbos-tabs-and-breadcrumb-worker.png`.

## What Cursor does

One strip, left-aligned, no Home tab: the project tab (glyph + name), then an *italic* tab per opened worker ("Build todo.py CLI"), then `+`. Opening a worker from its line opens a tab beside the project's; closing it returns to the project. On relaunch the project tab comes back and the worker tabs do not (cycle 23, `cursor-12-reopened`).

## What Arbos does (decision from cycle 9, kept in F-111)

One tab per project. A worker opens *inside* the project's tab, with a breadcrumb over the chat: `Use exactly one worker › create and modify files`. Back is the breadcrumb's first crumb, Escape, or the panel.

## Why reopen now

The bars are one surface (F-118), so the tab strip is the only band left that says where you are. A worker tab is a visible way back that survives scrolling, and lets two chats stay open side by side in the mind (project and worker) the way Cursor's italic tab does. The breadcrumb disappears as soon as the chat scrolls.

## Cost, honestly

Tabs are projects today: `tabs.rs` draws one per `Workspace.projects[ix]`; `tab-N` ids, ⌘1…9, ⌘W, the restore of `state.active`, the tab spinner and the unseen dot all index projects. A worker tab needs a tab model of `(project, session)` — new type in `workspace.rs`, `tabs.rs` rewritten around it, ⌘W semantics (close the worker tab, not the project), restore (do not restore worker tabs, as Cursor does), the gate's `tab-*` rows and the journey's J11a. Invasive across three files and the rig; a cycle of its own, not a fix.

## Recommendation

Do it, as cycle 27's headline, **if Jacob wants the Cursor shape here**. If he is content with the breadcrumb, the one cheap improvement is to pin the breadcrumb (it scrolls away with the chat today) so the way back is always visible. This is his call: both are defensible, and the cost of the first is real.
