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

## Sidebar footer (Jacob 2026-10-24, [feedback footer-avatar-grok-size-2026-10-24])

**The footer is the top strip mirrored at the window's bottom edge: a 60 pt
band whose 36 pt discs are centred 30 from the bottom.** Jacob: "Make the
profile picture at the bottom here the same size as it is on grok."

Measured on Jacob's Oct-2026 Grok Bot Mac refs
(`media/group-chat-redesign/refs/1-agent-chat-topbutton-sidebar.png` and
`2-agent-chat-info-pane.png` in the project store; 2520 × 1670 px at 2 px/pt,
lights 28 px / discs 72 px as in the strip measurement, window bottom at the
image's last row, sidebar divider at x 553 px → 276.5 pt wide). Points from
the sidebar's left edge and the window's bottom edge.

| Item | Grok Bot | Arbos (`sidebar.rs` `FOOTER_*`) |
|---|---|---|
| Band | avatar, "Connect apps" pill and update disc all on rows 1574–1645 px → 36 tall, 12 above the bottom, centre 30 | `FOOTER_HEIGHT` = `HEADER_HEIGHT` 60; everything centred → centre `CONTROL_LINE` 30 |
| My avatar | 72 px disc at x 26–97 → **36**, left edge **13**, bottom **12** | `FOOTER_AVATAR` = `CONTROL` 36 at `FOOTER_INSET` 13, `FOOTER_BOTTOM` 12 (was 28 at x 18, 16 above the bottom) |
| Machine dot | — (Grok has none) | scales with the avatar: 9 pt dot in a 13 pt ring of the sidebar colour, 2 pt past the bottom-right edge (`FOOTER_DOT` / `FOOTER_DOT_RING` / `FOOTER_DOT_OVERHANG`; was 7 / 11 / 2) |
| Hover card | — | 44 tall, inset 8 like the rows, pad 5 so the avatar lands at 13; name `TEXT_SMALL` + 1, 10 after the avatar |
| "Connect apps" pill | 36 tall, x 114–432 px, 8 after the avatar, 8 before the update disc, fill `#1F1F1F`, 1 px `#2C2C2C` rim, text `#B8B8B8` | not built (no connectors pill in Arbos's footer; the name sits there) |
| Update disc | 72 px solid circle at x 449–520 px (the same x as the strip's "+" disc), same rows → **36**, right inset 16.5 in a 276.5 sidebar, fill **`#326CCB`**, glyph `#F5F5F5` cloud-with-arrow boxed 26 × 28 px → **13 × 14**, ~2.5 px stroke | `FOOTER_DISC` = `CONTROL` 36 at `FOOTER_DISC_INSET` = `STRIP_INSET` 12 (under our strip's last disc, as Grok's sits under its "+"), `STRIP_GAP` 8 after the hover card; `theme.update_disc` / `on_update_disc`; icon `cloud-download` at `FOOTER_DISC_GLYPH` 20 (traced path, 15.6 × 16.2 on the 24 grid at a 1.5 stroke) |

Pinned by `sidebar::footer_tests::the_footer_avatar_is_groks_size_inset_and_line`
(constants vs the table) and `the_footer_avatar_lands_on_groks_pixels` (gpui
layout of `footer_frame` + `footer_row_frame`: avatar left 13, bottom 12,
centre 30, dot overhang 2). `row_bounds_tests` builds the same frame under the
list, so the list still ends above it.

### Update button (Jacob 2026-10-24, [feedback footer-update-button-grok-2026-10-24])

**The only update control in the app's chrome is Grok Bot's disc at the
footer's right end** — "Give us the same update button as grok also." It
replaced the full-width "Update" pill under the chat list (D42 / #1458's
rail button). Settings → Updates (version, check, automatic toggle, install)
stays as the detail page.

| Item | Rule |
|---|---|
| Drawn when | `UpdateState::disc_action() != Hidden`: automatic updates **off** (the default) → the moment the feed has a newer build (one click downloads, verifies, installs, relaunches; the disc pulses at `DISC_BUSY_ALPHA_MIN`…1 while it works); automatic updates **on** → only once the build is installed and waiting for its safe relaunch (one click restarts now); after a failed install (click = retry); from a translocated / read-only copy (click = move to Applications) |
| Never drawn | up to date, checking, or a background download / install with automatic updates on |
| Click | `AppState::install_update` → `click_action()` — the same path the automatic install and Settings → Install use |
| Hover / pressed | the fill lifted `DISC_HOVER_LIFT` 0.06 / `DISC_PRESSED_LIFT` 0.12 in lightness (brighter under the hand, brighter still while down — the chip rule); busy = inert |
| No label, no countdown text | the notice line carries the words ("… press the blue update button at the bottom of the chat list …") |
| Rig | `ARBOS_UPDATE_STATE=available\|downloading\|installed\|failed` starts the fixture in that phase for a fake 0.6.999 build (the worker stays off) |

Pinned by `update::disc_tests` (hidden until ready; click → install /
relaunch; the click action is never gated by the automatic toggle) and
`sidebar::footer_tests::the_update_disc_is_groks_size_colour_and_placement`
+ `the_update_disc_lands_on_groks_pixels` (36 at the strip's inset, on the
footer's line, 8 after the card, fills `#326CCB` / `#F5F5F5`).

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

Results are **app-wide whatever chat is open** ([feedback
search-palette-app-wide-2026-10-24], Jacob): `crate::palette`, matching through
#1477's `search` module (display name, @handle, title, member, chat title; case
and diacritics folded) plus the local **`crate::search_index`** — every chat doc
on this Mac (agent chats, DMs, groups, server channels) indexed as it arrives
and re-indexed on every snapshot, upsert and channel page: message text,
attachments (name + title) and `http(s)` links, folded once and scanned in
memory on each keystroke, never a relay round trip.

**People & agents** (DMs and agents by name; people met in a group, a shared
project or the directory who have no DM yet — opening one starts the DM) →
**Chats** (groups; member and chat-title hits with the reason as subtitle) →
**Servers & channels** (a server and each matching channel as its own row) →
**Messages** (message text anywhere; row = the chat's name over "who · …snippet…";
opening it jumps to the message in its chat or channel) → **Files, media &
links** ("Kind · size · who in where"). Inside a section: a name that starts
with the query, then one that contains it, then indirect hits; newest first;
8 per section. Empty query: **Recent** — chats and servers by activity, no
files or messages — with the ⌘ badges. Rig hooks: `ARBOS_SEARCH=<text>`,
`ARBOS_PALETTE=1`, `ARBOS_PALETTE_SELECT=<n>`, `ARBOS_PALETTE_OPEN=1` (open
the highlighted row once the rows are there).

### iPhone: one bottom search bar, nothing else (Jacob 2026-10-25, [feedback iphone-bottom-search-bar-2026-10-25])

The iPhone's bottom tab bar (Contacts · Calls · Chats · Settings, a tester item
of 2026-10-23) is gone. Home's bottom edge is **one search capsule and no other
control** — no grid/+ button, no tabs, no Cancel beside it (`HomeChrome.bottom
== [.searchBar]`, pinned by `BottomSearchBarTests`). It sits in the bottom
safe-area inset (rides the keyboard, clears the home indicator); the × inside
the capsule clears and closes, dragging the results drops the keyboard, opening
a row closes it. The header keeps the account circle (→ Settings sheet) and the
`+` circle only — the search disc moved to the bar.

Results are the palette's, app-wide (`GlobalSearch` = `palette.rs`, `SearchIndex` =
`search_index.rs`, lazy against `messagesVersion`): the same five sections in the
same order with the same headings and the 8-per-section cap, Recent on the empty
query; a message row opens its chat scrolled to that row (`pendingReveal`), a
file row opens the Library item in the viewer, a DM-less person starts the DM.
The home list is no longer narrowed by typed text (as on the Mac).

Where the four tabs' destinations live now: Settings → the account circle;
Contacts → `+` (New message: people + directory) and the bar's People & agents;
Calls → a chat's header call buttons (no call-history list existed; the tab was
empty); the unread count → the app icon badge and each row's dot / count pill;
new chat → `+`.

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

Code: `transcript.rs` `hover_time()` (returns nothing at opacity 0), pinned by
`layout_tests::the_time_is_hidden_at_rest_and_beside_the_bubble_on_hover`;
iPhone `Transcript.swift` `RowView` + `SenderNameRowTests`.

## Agent bubble width (Jacob 2026-10-24, [feedback bubble-width-2026-10-24])

**An agent bubble is at most 560 pt wide, and never closer than 84 pt to the
column's right edge.** Measured on Jacob's Grok Bot shots of 2026-10-24
(`media/feedback/grok-parity-test/grok-chat-1.png`, `grok-chat-2-research.png`,
3456 × 2172 px = 2 px/pt, a 1728 pt window with the right pane open): Grok's
chat column is 1128 pt, its composer 960 pt, and every wrapped agent bubble
(the three-line answer, the table, the research bullets) ends at **1120 px =
560 pt** — 58 % of the composer, 50 % of the column; the list bubble is
content-sized at 531. Arbos in the same window: column 1041, composer 876
(`COLUMN_MAX_WIDTH`), bubble 792 = column − 84 (90 %). Grok's older narrow-pane
refs (57: 612 in a 696 pill; 58: 310 in 392) show the fixed 84 pt gap governing
below the ceiling, so the rule is the smaller of the two:
`assistant_cap(slot) = min(slot − 84, 560)`; the crossover is a 644 pt slot.
At our 876 pt column the bubble is 560 = 64 % of the column (Grok's composer is
wider than ours; matching the absolute width is what reads as "the same
bubble" side by side). Question / approval cards share the slot and the cap
(#1374 / #1440); my own bubbles keep their 72 % rule.

Code: `transcript.rs` `ASSISTANT_MAX_WIDTH`, `assistant_cap()`, the inner
`chat-bubble-cap` frame of `assistant_slot()`; `onboarding::bubble_target`
predicts with the same function. Pinned by
`layout_tests::the_agent_bubble_cap_is_the_column_minus_the_grok_gap_at_most_grok_width`
and `cards_share_the_bubbles_left_edge_and_cap`.

## Markdown inside bubbles (Jacob 2026-10-24, [feedback bubble-markdown-2026-10-24])

**Lists and headings read like Grok Bot's: bold headings, real bullet discs,
Grok's line, item and block rhythm.** Measured on Jacob's `grok-chat-1.png`
(2 px/pt) against Arbos v0.6.221 (`arbos-chat-right-pane-1.png`):

| | Grok Bot | Arbos before | Rule |
|---|---|---|---|
| Text line (wrapped paragraph, agent and user bubbles) | 44 px = **22 pt** | 20 pt | `CHAT_LINE_HEIGHT` 22 |
| List item pitch | 52 px = **26 pt** | 23 | `LIST_ITEM_GAP` 4 over the 22 line |
| Block pitch (paragraph → heading → list → paragraph) | 64 px = **32 pt** | 24–26 | `BLOCK_GAP_HALF` 5 above and below every block row (10 between blocks) |
| Bullet | **4.5 pt disc**, muted, centre 8.5 pt from the block's left; item text at **20 pt** | the font's "•" (a 3 pt dot 2 pt in), text at 24 | `BULLET_DOT` 4.5, `BULLET_CENTRE` 8.5, `LIST_INDENT` 20 (26 for two-digit numbers) |
| Heading / `**bold**` | bold (700); "Surf gear" renders 3 % wider than the same semibold string | semibold (600) | `STRONG_WEIGHT` = `FontWeight::BOLD` for `#` headings and strong spans |

Jacob's "headings" in the surf list are `**bold**` paragraphs (same glyph
height as the body), so the weight rule covers both. `#` heading sizes (20 /
17 / 15) are unchanged. Copy text keeps its "• " markers.

Code: `transcript.rs` `render_block` (lists, headings), `inline_runs`,
`markdown_bubble` block padding; `theme.rs` `CHAT_LINE_HEIGHT`. Pinned by
`markdown_tests` (rhythm constants, bold runs, a headless render of a bullet
list: disc size / place, indent, 26 pt pitch, dot on the first line's centre).

## Bubble hover rail (Jacob 2026-10-24, [feedback bubble-hover-rail-2026-10-24])

**The ☺ ↩ ⋯ rail is vertically centred beside the bubble and shows while the
pointer is anywhere on the bubble.** Measured on Jacob's Grok Bot shot
(`grok-chat-2-research.png`, 2 px/pt): the three glyphs sit at the mid-height
of a 440 pt table bubble, the first glyph's centre 18 pt past its right edge,
glyph centres 26 pt apart — the rail's own geometry (`RAIL_BUTTON` 24 + `RAIL_GAP`
2, 6 pt off the bubble) already matched; only the placement did not. Before
this rule the rail hung off the *last markdown block* of a reply — at the
bottom of a long bubble — and appeared only while that last block was hovered.

An agent reply is one transcript row per markdown block, so "the bubble" means
all of those rows: hover is tracked per message (`HoverFade` keyed by
`{message_id}#bubble`), and the last block places the rail at the middle of
every block row (`Transcript::bubble_rail_centre` → `rail_centre()` from the
list's item bounds), clamped so a bubble taller than the pane keeps its rail on
screen under the header fade band. One-row bubbles (mine, other people's,
media) centre the rail on their own row as before.

Code: `transcript.rs` `HoverFade::set(key, row, …)`, `rail_centre()`,
`hover_rail_frame()`; pinned by `rail_tests`.

## Mac bubble text selection (Jacob 2026-10-24, [feedback bubble-selection-2026-10-24])

**A press on bubble text starts a selection, whichever way the pointer then
moves — up, down, or into another message; dragging a message never turns it
into a reply.** Grok's behaviour, as Jacob checked side by side:

- Bottom-up works: the old axis lock handed a vertical pan with no range yet
  to drag-scroll, so a drag that started at the end of a message and went up
  selected nothing. Now the selection owns the pointer from the press
  (`TextSelection::is_dragging`), and the transcript arms its click-drag
  scroll only for presses that are not on text (bubble padding, empty rows).
- Across messages: the selection is a pair of positions, each in one bubble
  with that bubble's transcript order (`select::BubbleAt`, `Span`). The first
  bubble is selected from the pressed index, every bubble between whole, the
  last up to the pointer; ⌘C copies them in transcript order whichever way the
  drag went, one blank line between messages (`transcript::copy_across`).
  Shift-click extends into another bubble the same way. One-bubble gestures
  (#1389: drag through paragraphs, double-click word, triple-click paragraph)
  are unchanged.
- No swipe-to-reply on the Mac: the Telegram / iPhone right-swipe
  (`ReplySwipe`) is removed from the Mac transcript; Reply is the rail's ↩,
  the message menu and the right-click menu. The iPhone keeps its own gesture
  under its own rule (above).

Rig: `ARBOS_RIG_SELECT=across` selects from the second word of the bubble
before the newest agent bubble to that bubble's second-to-last word.

Code: `select.rs` (`BubbleAt`, `Span`, `Model::begin_at` / `extend_in` /
`propose_in`), `transcript.rs` (`message_order`, `bubble_texts`,
`copy_across`, the root mouse-down guard). Pinned by `select::tests::
a_drag_across_bubbles_spans_them_in_transcript_order`,
`a_bottom_up_drag_selects_across_two_rendered_bubbles` (a simulated pointer
drag straight up from one rendered bubble into the one above), and
`transcript::cross_bubble_copy_tests`.

## Own-bubble delivery (Jacob 2026-10-24 + 2026-10-25, [feedback sent-state-opacity-2026-10-24], [feedback sent-full-when-delivered-2026-10-25])

**No "sent" / "seen" caption on my bubbles, and no dimming after delivery.** A
delivered bubble is the full own-bubble style the moment the relay ACKs it;
only a pending bubble is dimmed; a failed one is full with a red "! Retry".
Grok Bot's user bubble carries no delivery label and no delivery dimming beyond
pending (refs 03 / web-01).

| State | Meaning | Whole-bubble alpha | Extra |
|---|---|---|---|
| Pending | not yet ACKed by the relay | **0.50** (`BUBBLE_ALPHA_PENDING`) | — |
| Failed | the relay refused it | 1.00 | red `exclamation` glyph + "Retry" (`danger`) on the bubble's outer edge, always visible; click re-sends the text |
| Delivered | ACKed | 1.00 | — |

Why no "sent-but-unseen" state (the 0.72 of v0.6.223–v0.6.226, #1667): read
receipts cannot be expected — agents never send one, members on older builds
never send one, a member who has not opened the chat sends none, and the relay
stores none — so in a group a delivered bubble sat at 0.72 indefinitely and
Jacob read it as "not sent" (Arbonites, 2026-10-25 22:17: fill measured
`#484848` = 0.72 × `#5c5c5c`, with an agent reply already under it). A 1:1-only
variant would still dim forever against a peer on an older build, so the state
is gone on both platforms. Read receipts are still *sent* (Settings →
Notifications on the Mac; the iPhone sends `seen` while a people chat is
focused) but nothing is drawn from them.

Pending at 0.5: white text on the composited fill 3.4 : 1 dark / 3.6 : 1 light,
transient until the ACK. Same values on the iPhone (`Delivery.pendingAlpha`).

Code: `transcript.rs` `Delivery` + `delivery_state()` + `own_bubble()` +
`failed_glyph()` / `Transcript::failed_retry` → `AppState::resend_failed`,
pinned by `delivery_tests`; iPhone `Transcript.swift` `Delivery` (`.user(text:,
delivery:, quote:)`) + `AppModel.resendFailed`, pinned by `DeliveryStateTests`.
