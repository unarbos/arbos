# Mac app — layout rules that must not drift

These rules were decided by Jacob and have been lost once already (#1134 built
the compact strip; #1491 replaced it with an always-open Search pill; this page
and the tests in `crates/arbos-mac/src/ui/sidebar.rs` keep them in the repo).
Design-store sources: `docs/group-chat-redesign.md` and the approved mocks in
`media/feedback/sidebar-top-strip/` of the project store.

## These rules outrank tester preference (Jacob 2026-10-24)

A tester report that asks for the opposite of a rule on this page does not change the rule. Since
2026-10-24 13:55 UTC no outside feedback merges to `main` without Jacob's per-item approval
(`docs/process-rules.md`); a fix that would move one of these rules holds for that approval and says
so in its summary.

## Three-pane rule (app-wide, Jacob 2026-10-20)

Exactly three panes.

- **Left** always lists chats, groups and servers. At its top only three
  controls: **+**, **search**, **minimize** (collapse). Entering a server slides
  the left pane over to that server's channel list (back arrow returns).
- **Centre** is always a chat (agent, group or channel). Nothing else, ever.
- **Right** shows every details / members / admin / settings screen (user
  profiles, group settings + members, agent details, server settings, app
  settings). It opens beside the chat, never over it, and closes with an **X**.
- Every right-pane screen uses one layout: editable picture, editable name, a
  row of tabs, then lists or settings beneath.

Code: `sidebar.rs` module docs and `strip_controls()` (pinned by
`strip_tests::the_strip_holds_exactly_search_plus_collapse`); the right pane's
shared layout is `profiles/pane.rs` (`layout()` + every kind's `TABS`).

## Chat-list top strip (approved mock 2026-10-24, [feedback sidebar-strip-filter-2026-10-24])

Points at 1×; the Mac draws at 2 px/pt.

| Item | Value |
|---|---|
| Strip height | 44 (`HEADER_HEIGHT`), controls centred on the traffic lights' line (20.75) |
| Buttons, left to right | search · + · collapse — three round discs, right-aligned |
| Disc | 32 (`STRIP_BUTTON` = `CONTROL`), fill `surface_2`, glyph 15–16 in `text` |
| Gap / right inset | 10 (`STRIP_GAP`) / 12 (`STRIP_INSET`) |
| Search | closed by default; click or **⌘F** expands it into a 32 pt capsule from the strip's left edge to the "+"; Escape or the clear dot fold it; a typed query keeps it open |
| Inside a server | no search (spacer); "+" only for owner/admin; collapse always |
| Folded sidebar | the chat header shows the same round disc beside the lights (`header::sidebar_toggle`) |

## Chat-list filter (the iPhone home's `PaneTabs`)

Directly under the strip: **All · Agents · People · Servers** — the iPhone
`HomeListFilter` with the same labels and order. Bare labels, the selected one
on a capsule, centred; no track, no border. It stays while the search field is
open: the query narrows the list, the segment still scopes it.

| Item | Mac | iPhone |
|---|---|---|
| Capsule height / gap | 32 / 4 | 32 / 4 |
| Label | `TEXT_BODY` 15, semibold when selected | `Theme.body` 17, semibold when selected |
| Side padding | **10** (`FILTER_PAD_X`; four segments at 16 overflow the 296 pt default sidebar) | 16 |
| Selected fill / text | `bubble_assistant` / `text` | `bubbleAssistant` / `text` |
| Other labels | `text_muted` | `muted` |
| Scope | All: everything · Agents: my agents (not shared-in) · People: DMs + groups · Servers: servers only | same (`HomeView.homeRows`) |

Server rows draw their picture as a rounded square (corner 11/36 of the size,
`SERVER_PICTURE_CORNER`) so servers read apart from the agent and people discs.

## Chat header (Jacob 2026-10-20, `topbar-unify`; #1130, #1336)

**Chat header: one centred pill (avatar + name) for every chat kind — #1130/#1336;
tester preference does not override.** The pill sits on the chat column's
centre line, its top 7 pt under the window edge, 34 pt tall, picture 22 pt
(`AVATAR_HEADER`), name 13 pt semibold (`TEXT_SMALL`); the only other things
in the bar are the sidebar expand disc (folded sidebar) and the connection
capsule under the pill. #1493 moved it top-left on a tester's "personal
preference" and was reverted by Jacob ([feedback header-pill-restore-2026-10-24]).

Code: `header::bar_frame` (equal side slots, `justify_center`) and
`header::metrics`; pinned by `header_tests::the_pill_is_centred_not_parked_at_the_left_edge`.
