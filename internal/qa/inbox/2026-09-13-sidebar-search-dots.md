# U-07 sidebar search + status dots — QA note (features agent, 2026-09-13)

Branch `cursor/sidebar-search-dots-b027`, base `rust`. Parity report row 6, two of its three asks.

## What it does

- **Search.** A `Search` row under `New Chat` / `Open Folder` (id `quick-search`). Click → a text field appears in its place (id `sidebar-search`, placeholder "Search chats…", Escape or an empty field closes it). While it has text, every project's list shows only the chats whose label contains the text (case-insensitive), with their children; projects with no match still show their head row.
- **Status dots.** A 6 px dot at the right of a chat row, before the age: accent while a turn runs (the spinner still shows in the mark's place), warning (amber) when a question is parked for you, faint when a standing clock is the only thing pending; nothing when idle. Cursor's rows carry the same dot.
- **Not done: time buckets** (Today / Last 7 days / …). Rows are ordered by the user's drag rank, not by time, and the parity report itself marks the Projects-row design as an open question; regrouping by time is that design's call.

## Attack ideas

1. Search text that matches no chat in any project: every project shows its head row only; no "no results" text (Cursor shows none either) — decide if one is wanted.
2. Search open, then a new chat is created: the field stays; the new chat's label ("New chat") may not match — it hides at once. Check that the composer still opens it.
3. A match on a child agent only (parent label does not match): today the parent is hidden with its child. Note.
4. Renaming a chat while searching: two text fields; `Escape` must close the right one.
5. 500 chats: the filter runs per render over labels (`display_label`) — check the sidebar stays smooth while typing.
6. Dot colours in dark mode; the amber dot next to the amber "asking" glyph is redundant — decide whether to drop the glyph.
7. Drag-and-drop of rows while filtered: rank changes apply to the full list; make sure a hidden neighbour does not receive the drop.
8. Search with a leading `/`: literal match, no slash-command behaviour.
9. Keyboard: `Cmd/Ctrl+F` opens the search? Not bound (only the row click). Note.
10. Driver: `quick-search`, `sidebar-search` ids present; `state().sidebar_search` carries the text.
