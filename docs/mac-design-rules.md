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

## Chat-list top strip (approved mock 2026-10-24, [feedback sidebar-strip-filter-2026-10-24]; sized to Grok Bot [feedback search-palette-2026-10-24]; placed like Grok Bot [feedback topbar-grok-parity-2026-10-24])

Points at 1×; the Mac draws at 2 px/pt. Vertical numbers are from the
window's top edge.

| Item | Value |
|---|---|
| Strip height | **60** (`HEADER_HEIGHT` = 2 × `CONTROL_LINE`); the chat header and the right pane's header are the same band |
| Traffic lights | macOS's own, at `TRAFFIC_LIGHT_POSITION` 15 / 15: 14 pt discs centred **22** from the top (position + 7, measured), 23 apart — Grok's spot (was 14 / 21) |
| Discs' line | centred **30** (`CONTROL_LINE`): disc top 12, bottom 48 — Grok's; 8 pt under the lights' line (they are not one line in Grok either; Jacob's "too high" was the discs centred at 21 in a 44 band) |
| Under the strip | the filter capsules start at **64** (`FILTER_TOP` 4 after the 60 strip) — where Grok's first chat card starts, 16 under the discs; the capsule row ends at 104 (`FILTER_BOTTOM` 8) |
| Buttons, left to right | search · + · collapse — three round discs, right-aligned |
| Disc | **36** (`STRIP_BUTTON` = `theme::CONTROL`), fill `surface_2`, glyph in `text` |
| Glyphs | `CONTROL_GLYPH` 19 for search and + (visible marks 14.3 / 12.7 at a 1.6 stroke); `CONTROL_GLYPH_WIDE` 17 for the collapse rect (visible box 14.2) |
| Gap / right inset | **8** (`STRIP_GAP` = `CONTROL_GAP`) / 12 (`STRIP_INSET`: the discs end 4 past the rows' card edge, as Grok's) |
| Search | a round disc only; click, **⌘⇧F** or **⌘K** open the command palette (below). No inline field or capsule any more. **⌘F** is unbound — reserved for in-chat search (not built yet) |
| Inside a server | no search (spacer); "+" only for owner/admin; collapse always |
| Folded sidebar | the chat header shows the same round disc beside the lights (`header::sidebar_toggle`) |

### One button family (Jacob 2026-10-24)

The sidebar's three discs, the folded-sidebar toggle in the chat header **and
the chat header's pill** (avatar + name, every chat kind) are one family at
Grok Bot's size: all `theme::CONTROL` = 36 tall (`header::metrics::PILL_HEIGHT`
= `CONTROL`, picture `AVATAR_HEADER` = `CONTROL − 8` = 28, pill and toggle
centred on the same line, `PILL_TOP` = 12 = Grok's disc top). Pinned by
`sidebar::strip_tests::the_three_discs_are_groks_size`,
`sidebar::strip_tests::the_strip_is_groks_height_and_the_discs_sit_on_groks_line`,
`sidebar::strip_tests::the_discs_and_the_filter_land_on_groks_pixels` and
`header::tests::every_kind_has_the_same_bar_height_and_pill_position`.

Measured on `media/feedback/search-palette/ref-grok-round-search-plus.png`
(project store, 2 px/pt, window top at image row 2): disc 72 px, glyph stroke
3 px, magnifier box 28 px, plus box 26 px, 16 px between discs, disc fill
`#1F1F1F` on `#0E0E0E`, glyph `#E0E0E0`; discs at rows 24–96 px (12–48 pt),
lights at rows 32–59 px (centre 22 pt, red at x 30–57 px → left edge 15),
first chat card at row 130 px (64 pt). The before shots: v0.6.206 had 64 px
discs 20 px apart; v0.6.212–0.6.214 had the right discs centred at 21 pt
(top 3) in a 44 pt band with the filter capsules at 50 — 9 pt too high
against Grok, Jacob's "the top bar on mac is now too high".

## Command palette ([feedback search-palette-2026-10-24], Grok Bot's)

Search never opens inline. The Search disc, **⌘⇧F** and **⌘K** open one
centred modal over the window (`ui::palette` on the shared `ui::modal`
backdrop): the chat behind gets no wheel, no click and no key while it is up;
Esc, a click outside, or opening a row closes it. Send Feedback moved from
⌘⇧F to **⌘⌥F** (Help menu unchanged).

| Item | Value (measured on `ref-grok-command-palette.png`, 2 px/pt) |
|---|---|
| Panel | 558 wide (`palette::WIDTH`), radius 12, `card()` fill, hangs at 12 % of the window height (≥ 48), at most 72 % tall |
| Field row | 49 tall, magnifier 17 (12.5 visible) inset 16.5, "Search" at 17 pt; 1 pt hairline under it |
| List | inset 10, 8 over the first row; rows 49 tall, 2 apart, radius 12, highlight `surface_2` |
| Row | picture 24 (chat disc / server square / `#` tile / thumbnail or glyph tile), name 13.5 medium, subtitle 12.5 muted |
| Shortcut badge | ⌘1–⌘9 on the first nine rows: 27 × 19, radius 6, 8 from the row's right edge |
| Keys | ↑↓ move the highlight (`ComposerInput::on_arrow`), ⏎ opens, ⌘1–9 open that row, Esc closes |

Results (`crate::palette`, matching through #1477's `search` module — display
name, @handle, title, member, message; case and diacritics folded): **People &
agents** (DM or agent by name) → **Chats** (groups; member, chat-title and
message hits with the reason as subtitle) → **Servers & channels** (a server
and each matching channel as its own row) → **Files, media & links** (every
attachment and `http(s)` link in loaded chats, "Kind · size · who in where").
Newest first inside a section, name hits before indirect ones, 8 per section.
Empty query: **Recent** — chats and servers by activity, no files — with the
⌘ badges. Rig hooks: `ARBOS_SEARCH=<text>`, `ARBOS_PALETTE=1`,
`ARBOS_PALETTE_SELECT=<n>`.

## Chat-list filter (the iPhone home's `PaneTabs`)

Directly under the strip: **All · Agents · People · Servers** — the iPhone
`HomeListFilter` with the same labels and order. Bare labels, the selected one
on a capsule, centred; no track, no border. The list is no longer narrowed by
typed text (search is the palette); the segment alone scopes it.

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
centre line at the button family's size ([feedback search-palette-2026-10-24],
Jacob 13:52): **36 tall** (`PILL_HEIGHT` = `CONTROL`), its top **12** pt under
the window edge so it shares the sidebar discs' line (`CONTROL_LINE` 30, in
the 60 pt band — [feedback topbar-grok-parity-2026-10-24]), picture **28**
(`AVATAR_HEADER` = `CONTROL − 8`), name 13 pt semibold (`TEXT_SMALL`); the only other things
in the bar are the sidebar expand disc (folded sidebar) and the connection
capsule under the pill. #1493 moved it top-left on a tester's "personal
preference" and was reverted by Jacob ([feedback header-pill-restore-2026-10-24]).

Code: `header::bar_frame` (equal side slots, `justify_center`) and
`header::metrics`; pinned by `header_tests::the_pill_is_centred_not_parked_at_the_left_edge`.

## Message timestamps (Jacob 2026-10-24, [feedback timestamps-hover-only-2026-10-24])

**Message timestamps are hover-only, never always-on.** A bubble never carries
its time — not in a footer, not after the text, in no chat kind (1:1, group,
channel; mine, another person's, an agent's). The time ("9:41 PM") appears only
while the pointer is over the row, beside the bubble on its outer edge (left of
my bubble, after the ☺ ↩ ⋯ rail of everyone else's), fading with the rail; at
rest it is not drawn at all. On the iPhone the same rule is the iMessage
drag-to-reveal (`timeReveal`): no inline or footer time in any bubble.

Lost twice before this page existed: v0.6.183 (#1437, group bubbles got the time
after the text) and v0.6.203 (#1496, a tester's request for an always-visible
bottom-right footer time). The sent/seen label on my own bubbles is a separate
element and may stay in the footer.

Code: `transcript.rs` `hover_time()` (returns nothing at opacity 0) and
`bubble_footer()` (receipt only), pinned by
`layout_tests::the_time_is_hidden_at_rest_and_beside_the_bubble_on_hover`;
iPhone `Transcript.swift` `RowView` + `SenderNameRowTests`.

## iPhone bubble gestures (Jacob 2026-10-24, [feedback iphone-open-scroll-block-2026-10-24] #1645 + #1629)

**A pan that starts on a bubble always scrolls the chat.** Bubble gestures are
`simultaneousGesture`s, never exclusive: the long-press (reactions + menu) must
not delay or swallow a vertical drag. **Bubble text is not selectable on the
iPhone** — `.textSelection(.enabled)` stays off in every bubble (user, other
person, agent, code); Copy is on the long-press menu. Approved by Jacob as the
trade-off in #1629 (@allan), which supersedes #1389's drag-to-select on the
iPhone. The Mac keeps selection (`select.rs`, mention chips included).

The other half of the same P0 is performance: while a room catches up or a
burst of live rows lands, the transcript is re-read at most once per window
(600 ms catching up, 150 ms live), off the main actor, and a bubble never reads
`theme` or `composerInset` from the environment (plain inputs), so a publish
does not re-create the window of bubbles (#1645; first-scrollable after open
+7.8 s → +0.74 s with a 72-row catch-up on Arbonites).

Code: `MessageActions.swift` `messageRowGestures`, `Transcript.swift` `RowView`,
`AppModel.swift` `requestSnapshot` / `applySnapshot`.

## Full screen (Jacob 2026-10-24, [feedback fullscreen-traffic-lights-2026-10-24] → [feedback topbar-grok-parity-2026-10-24])

**The traffic lights are macOS's own standard window buttons, always. The
app never draws, imitates or re-parents them** (Jacob, 18:3x UTC: "this
should be real mac os feature not custom buttons"; private-API tricks such
as Chrome's `NSThemeFrame` overrides count as custom).

| Item | Rule |
|---|---|
| Windowed | The real buttons at `TRAFFIC_LIGHT_POSITION` (centre 22 from the top, Grok's spot) |
| Full screen | Stock macOS: the window grows to the screen, the lights hide with the title bar and come back — real buttons, in macOS's own bar — when the pointer reaches the top edge |
| Strip and filter | Exactly where they are in windowed mode: no band above, no layout jump (`top_strip(true) == top_strip(false)`, pinned by `sidebar::strip_tests::full_screen_adds_no_band_above_the_strip`) |

v0.6.213 painted three lights of its own in full screen (`ui/traffic_lights.rs`,
#1637); they changed shape and glyphs against the real ones and were removed
on Jacob's ask. Supported AppKit has no way to keep the real buttons on
screen in full screen: an `NSToolbar` keeps the title bar on screen but the
buttons in it at alpha 0, an `NSTitlebarAccessoryViewController` shows its
own view there but not the buttons, and the AppKit-owned alpha / view tree is
reset on every reveal (the #1637 probes, `media/feedback/fullscreen-traffic-lights/appkit-probes/`).

## Delete is immediate — no confirm (Jacob 2026-10-24, [feedback remove-delete-confirms-2026-10-24])

**Delete in the message menu and Delete in the chat-list row menu act at
once; neither opens a "Delete …?" card, sheet or inline Cancel / Delete
step.** The guard is *who sees the item*, not a second click: a bubble's
Delete appears only on your own messages or where you moderate (#1526's
relay rule, `model::may_delete_message`), and still deletes for everyone;
a row's Delete removes a DM or an agent (with its chats) from your list,
deletes a group you own for everyone, and leaves a group you do not own.
The "Delete message?" card from #1502 and the row menu's "Delete “Name”?"
prompt (#1532) were removed on Jacob's ask; the iPhone's long-press Delete
and home-list Delete behave the same. Other sheets keep the shared modal
layer (`ui::modal`) — this rule is about these two deletes only.

Code: `message_menu::run` (Delete → `AppState::delete_message`),
`sidebar::row_menu` (Delete → `delete_project` / `delete_people_chat` /
`leave_group`); pinned by `message_menu::tests::delete_is_offered_only_when_allowed`
and `sidebar::row_menu_tests::delete_fires_at_once`.
