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
| Search | a round disc only; click, **⌘⇧F** or **⌘K** open the command palette (below). No inline field or capsule any more. **⌘F** is in-chat search — the Search-in-chat bar under the header pill (below), never the sidebar field |
| Inside a server | no search (spacer); "+" only for owner/admin; collapse always |
| Folded sidebar | collapses to the **icon rail** (section below), never away; in a plain window the chat header shows the same round disc beside the lights (`header::sidebar_toggle`), in full screen the rail's top-left slot holds it |

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

## Collapsed sidebar = icon rail (Jacob 2026-10-26, [feedback mac-sidebar-rail-2026-10-26])

**The sidebar toggle collapses the chat list to Grok Bot's 84 pt icon rail; it
never closes it.** Jacob: "instead of closing the side bar all the way when
pressing the tab button I want you to do the same as what grok does but with
the + and search button at the top and our user icon at the bottom." Approved
on the round-2 mock (`media/feedback/mac-sidebar-rail/mock-mac-sidebar-rail-*.png`
in the project store, 10:47 CR).

Measured on Jacob's `jacob-ref-grok-collapsed-rail-2.png` (3440 × 2158 px =
a 1720 × 1079 pt window at 2 px/pt): hairline at x 167 px → 83.5; selected tile
107 × 108 px → 54 square, radius ≈ 12; pictures 72 px → 36; pitch 116 px → 58;
the user's picture centred 31 from the bottom; blue unread dot 16 px → 8 at the
picture's bottom-right 45° point; composer 849–2766 px → 959 wide, centred in
the chat area (not the window). Points from the window's edges.

| Item | Rule (`ui/rail.rs` `metrics`) |
|---|---|
| Rail | **84** wide incl. the 1 pt `border` hairline (`WIDTH`), `theme.chrome()`; one column centred at **42** |
| Toggle | **a plain window:** stays where today's lone toggle is — the chat header's left slot, 12 after the rail (x 96), top 12 on `CONTROL_LINE`. **Native full screen** (the lights are hidden): the rail's top-left slot, x 24, top 12. `rail::toggle_spot(fullscreen)`; never both (`header_toggle_shown`) |
| + and Search | stacked under the toggle's slot, `STRIP_GAP` 8 apart: + top **56**, Search top **100** (36 pt `control_disc`s, the strip's own handlers). In a window the toggle's slot stays empty under the lights; + and Search never move between the two modes |
| Pictures | every chat the current filter admits — agents, people, groups, servers (rounded squares, `SERVER_PICTURE_CORNER`) — in the list's order, **36** pt on a **58** pitch, first tile top **152** (16 under Search; picture top 161, centre 179); the same `ListEntry::picture` builders as the rows (one avatar resolver, #1912). Selected = **54** tile, radius **12**, `surface_2`; hover = `surface_2` at 55 % + a name tip 8 right of the rail (`pane_card`, `border_strong`, 13 pt) |
| Badges | one spot at the picture's bottom-right corner: **typing** = the 9 pt `online` dot in a 13 ring (pulsing), else **unread** = the 8 pt `accent` dot in a 12 ring; the selected chat shows no unread dot — the rows' own `!typing && unread` rule |
| My picture | alone at the bottom: 36 at x 24, **12** above the bottom (centre 30 = `CONTROL_LINE` mirrored; Grok 31), with the machine dot; opens Settings like the footer row |
| Scrolling | the pictures scroll (`LIST_FADE_BAND` edge fade) when more fit than the window holds (`capacity`: 14 in a 1079 pt window) |
| Filter | the rail follows All · Agents · People · Servers; the segment persists in `<config>/sidebar_filter` and the collapsed state in `<config>/sidebar_collapsed` (`SidebarCollapsed`), so the app reopens as it was left |
| Servers | a server's picture opens its **newest channel** (`#general` when none has spoken) and marks the server open, so expanding shows its channel list |
| Expanded sidebar | **unchanged** — strip, filter, rows, footer exactly as above |
| Composer / column | the pill's cap is Grok's **960** (`COMPOSER_WIDTH` 904 + 2 × 28; was 876), the transcript column shares it (`COLUMN_MAX_WIDTH`); both centred in the chat area right of the rail / sidebar, as before |
| Collapse / open | a **morph**, not a clip, on the pane-slide curves (170 ms ease-out quad closing, 200 ms ease-out cubic opening): the three discs glide from the strip into the rail column (the toggle to the header's slot in a window), the rows' pictures shrink into their rail slots as the text and filter fade out by half-way, the selected card shrinks into its tile, my footer picture slides to the centre, the chat column re-centres. `rail::render_morph` (the column) + `rail::render_overlay` (the travelling pieces, from the resting bounds the expanded sidebar records each frame — `Shell::morph`). Reduce Motion snaps |
| Narrow band | below `NARROW_WINDOW` the sidebar still hides and the header toggle floats it as an overlay (Q7 b); the rail is for the docked widths |
| iPhone | no equivalent (no sidebar; the home list + the single bottom search bar) |

Rig: `ARBOS_SIDEBAR_COLLAPSED=1|0` (start collapsed), `ARBOS_SIDEBAR_FILTER=…`,
`ARBOS_RIG_FULLSCREEN=1` (lay out as full screen where the platform cannot say),
`ARBOS_RIG_RAIL_T=0.4` (freeze the morph at a progress for still frames).

Pinned by `rail::tests` (metrics vs the measurement table, the rail's layout
on the mock's pixels, toggle spot windowed vs full screen, header-toggle rule,
badge priority, capacity, morph maths), `slide::tests::collapsing_slides_the_sidebar_down_to_the_rail_and_back`
(+ reduce-motion / width-drag), `theme::sidebar_state_tests` (collapsed state
restore) and `sidebar::strip_tests::the_filter_is_restored_from_the_config_dir`.

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

## Search in chat — ⌘F (Jacob 2026-10-26, [feedback search-in-chat-mac-2026-10-26]; approved mock 14:18 CR)

"Bring back command F to find things in a chat by key word, user name etc."
The Mac never had it (⌘F opened the sidebar field in v0.6.197 and was unbound
in v0.6.212 for this); the iPhone's bar is #1829. **⌘F opens the
Search-in-chat bar for the open chat** — the palette (⌘⇧F / ⌘K) stays
app-wide, the bar is this chat only. Code: `crate::chat_search` (rules),
`ui::chat_search` (bar), `AppState::chat_search` (state).

| Item | Rule |
|---|---|
| Place | the first thing under the header: top **56** = `PILL_TOP` 12 + `PILL_HEIGHT` 36 + **8** (the iPhone's spacing); the pill does not move; the first row starts **12** under the bar — the transcript moves down **44** (`metrics::PUSH`) |
| Size | **36** tall (`CONTROL`, the button family), radius **12** (the iPhone bar's / `BUBBLE_RADIUS`), **820** wide (`COMPOSER_WIDTH`), centred — bar, rows and composer share their edges; narrower panes shrink it with the composer |
| Material | the header's (`glass::header_surface` + `header_frost`): frosted on Glass and Matte alike, 1 pt rim, top highlight, the rows blurred through — the pill's own spec, so the bar follows whatever the pill does |
| Inside, left → right | inset 12; magnifier 15 muted; pinned sender chip (24 tall, picture 18, "From Name" 12.5 medium, accent 18 %, ✕ unpins); the field at `TEXT_BODY` 15, placeholder "Search in chat"; an offered chip while a name matches (⇥ pins); right: "searching older…" while a page is in flight, the count 13 pt muted, ‹ › 24 pt hit targets (faint with nothing to step to), ✕ 16 |
| Matching | keyword = folded substring over the chat's loaded rows (`arbos_core::search::fold`, the palette index's text); sender = the shared matcher of #1918 (`match_entity` on display name / @handle of everyone who posted: exact > prefix > word-start, name before handle); **chip AND keyword**; a pinned chip with no text = all of that sender's rows. A hit is a **message**, not a word occurrence |
| Count | "n of m" over the loaded rows, 1 = the oldest loaded (Find's order); the cursor starts on the newest hit; "No matches"; nothing while nothing is typed |
| Marks | every hit's words on the accent at 30 %; the revealed hit's words in Find's yellow `#ffd60a` with dark text; the revealed message's row washed and scrolled to through **the palette's reveal path** (`highlight_message` + `reveal_message` → `Transcript::reveal_hit`) — one reveal mechanism |
| Keys | ⌘F open / refocus (never closes); Enter / › / ⌘G next (newer); ⇧Enter / ‹ / ⇧⌘G previous (older); both wrap; ⇥ pins the offered sender; Esc / ✕ close — marks go, the composer types again. Inside the field the "ChatSearch" context overrides the composer's Send / Newline / AcceptMention |
| Beyond the loaded rows | ‹ on the oldest loaded hit of a **server channel** asks the relay for a page (`ChannelLoadOlder`), shows "searching older…", moves on to the page's older hit when it lands, keeps paging while rows come without a hit, and wraps once the room says there is nothing above (`SyncEvent::ChannelHistoryEnd`). A chat doc (agent, DM, group) is whole on this Mac, so ‹ wraps at once |
| Not | no second field in the header, no dropdown of senders, no match counter in the pill, no sidebar field |

Pinned by `chat_search::tests` (hits, AND, offer, wrap, paging step, count,
byte ranges), `ui::chat_search::tests` (⌘F opens the bar at 56 / 36 / the
composer's width and focuses the field, Esc closes; typing counts, steps,
wraps, pins; a channel pages older rows then wraps) and `keymap_tests`
(⌘F / ⌘G / ⇧⌘G; Enter / ⇧Enter / ⇥ inside the bar).

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

### Material: frosted glass on both surfaces (Jacob 2026-10-25 + 2026-10-26, [feedback mac-header-glass-2026-10-25], [feedback mac-header-pill-glass-matte-2026-10-26])

The pill and the folded-sidebar disc are **frosted glass whatever the
surface** — Grok Bot's header pill over its flat black transcript is the
reference, so Matte (no window vibrancy) is not an excuse for a flat fill.
#1830 (v0.6.256) frosted them on Glass only; Jacob's Mac is on Matte (the D90
default) and he saw the solid `bubble_assistant` pill on his AffineTeam group
(v0.6.268). Recipe (`glass::header_surface` + `glass::header_frost`,
`Theme::header_glass*`): the list blurred through the control's rounded
bounds at a **light** blur (`glass::HEADER_BLUR` 5 pt — a line of transcript
passing under the pill stays legible; the menus' 16 pt turned it to mush), a
see-through fill (dark `#3b3f45` @ **30 %**, light white @ 60 % — the same on
both surfaces; the backdrop blur composites ≈ 20 % darker than its source on
Metal and wgpu alike, so over flat `#fcfcfc` the light pill lands near the
`#ededed` of the solid pill it replaces), a 1 pt rim (white **30 %** / black
10 %), a top highlight (white **45 %** / 90 %) and a 1 pt lift. The dark pill
is read as glass the way Grok Bot's is over its black page: ≈ +12/255 above
the page, its shape carried by the rim, the transcript visible through it —
not as a plate. (v0.6.275 shipped 56 % / 18 % / 30 % / 16 pt and Jacob's
14:15 CR shot still read "opaque dark gray"; [feedback
mac-header-pill-see-through-2026-10-26].) White text on it keeps ≥ 4.5:1
because the pill always lies over the chat column — the page on Matte, the
85 % page wash on Glass. Sizes and placements are untouched; the rest of the
glass restyle stays paused. Pinned by `header::glass_tests` (every kind ×
surface × appearance; `the_dark_pill_is_see_through_and_its_text_still_reads`).

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
At the then 876 pt column the bubble was 560 = 64 % of it; since
[feedback mac-sidebar-rail-2026-10-26] the column is Grok's 960 (`COMPOSER_WIDTH`
904 + the two 28 pt circles), so the bubble is 58 % of the column as in Grok
(matching the absolute width is what reads as "the same bubble" side by side). Question / approval cards share the slot and the cap
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

## iPhone composer and status row = Grok Bot's (Jacob 2026-10-25, [feedback iphone-composer-grok-parity-2026-10-25] #1841)

**One composer and one status row for every chat kind** — agent 1:1, DM, group,
server channel, job tab — laid out from `Theme`'s composer constants and
pinned by `ArbosTests/ComposerMetricsTests`. The numbers are Jacob's Grok Bot
shot of 2026-10-25 (iPhone Air, 420 pt, 3 px/pt, keyboard up), aligned on the
composer's bottom line and the `+`; every value is a whole pixel at 3x. Grok
changed its composer between the 2026-10-15 refs and this shot; this shot wins.

| Item | Rule (pt) | Code |
| --- | --- | --- |
| `+` disc and field height | **48** (was 42), field edge on a true circle; radius stays 24 as the field grows | `Theme.plusButton`, `Theme.composerHeight` |
| Rim on the `+` and the field (dark) | edge highlight fading in over 1 pt: 58 / 47 / 40 luminance over `#181818` — three nested inner strokes ⅓, ⅔, 1 pt; light keeps the 1 pt `fieldBorder` | `ComposerRim`, `Theme.composerRimLayers` |
| `+` glyph | SF `plus` **22.5 pt regular** (54 px arms, 5 px stroke) | `Theme.plusGlyph` |
| `+` → field | **10** (was 12) | `Theme.plusFieldGap` |
| Side inset | 31 keyboard down (unchanged), **18 keyboard up** (was 19) | `Theme.composerInset`, `composerInsetKeyboard` |
| Text start | caret centre **17** from the field's edge; one 22 pt line centred (13 top / bottom) | `Theme.composerTextInset`, `composerTextPadY`, `MentionTextView.insets` |
| Placeholder / caret | 17 pt regular `#58585B`; 2 × 22 pt accent caret — unchanged (identical in the shot) | — |
| Mic | **filled mic without a base bar**, 12 × 16⅓, `#96969C`; glyph centre **24 before the pill** (44 pt hit frame + 2) | `MicGlyph`, `Theme.micHitWidth`, `micPillGap` |
| Right action | white **capsule 40 × 32** (was a 36 circle), 8 from the field's top and bottom, **8⅔ (26 px)** from its right edge | `Theme.actionPillWidth/Height/Trailing` |
| Right action glyph, idle | Grok's **voice-wave**: five round bars 2 wide, 3.25 apart, 6.5 / 14.5 / 8.5 / 14.5 / 6.5 tall; **a tap still starts the call** (Arbos has calls, Grok has voice mode). With text or a recording: `arrow.up` 16 bold. Channels: the arrow, dimmed until there is text | `VoiceWaveGlyph`, `ComposerField.actionGlyph` |
| Field → keyboard | **18** with the keyboard up (was 8); 2 above the home indicator otherwise | `Theme.composerBottomKeyboard`, `composerBottom` |
| Status picture | **25⅓ (76 px)**, left edge **6 inside the `+` disc's left edge** (24 from the screen keyboard up, 37 down) — no longer the bubble's 42 pt disc, no longer centred on the `+` | `Theme.presenceAvatar`, `presenceAvatarOffset`, `presenceInset(composerInset:)` |
| Status words | **17 pt body**, muted, shimmer unchanged; **14** after the picture (words at 190 px keyboard up) | `PresenceRow.textSize`, `Theme.presenceGap` |
| Picture bottom → composer top | **22⅔ (68 px)**; the row is the picture tall, ChatView pads below it | `Theme.presenceBottomGap` |
| Last bubble → picture top | **14 (42 px)** — the row rises 22 pt into the transcript's 28 pt tail instead of starting under the tail and the bar's 8 pt pad (was 36); bubble → composer with the row = Grok's 62⅓; the bar grows 26 pt (was 48) when the row appears ([feedback status-row-gap-ios-2026-10-26]) | `Theme.presenceTopGap`, `presenceLift`, `transcriptTail`, `chatBarTopPad` |
| Tasks chip (#1696) and job-tab words (#1801) | ride the same row unchanged (20 pt chip inside the 25⅓ row) | `PresenceRow.tasksChip`, `onWords` |

Not moved, on purpose: the people-chat **bubble picture stays 42 pt centred on
the `+`** (the ask covered the composer and the row, not the bubbles — the `+`
grew, the disc did not; `Theme.memberAvatar` is no longer `= plusButton`); the
Settings / search / secrets fields keep `Theme.fieldHeight` 42; the status
words' colour (both sides shimmer — one still cannot settle it); the light
theme's rim colour (no Grok light reference for this composer — shots on the
sheet for Jacob's yes). Measurement record and the before/after sheet:
project store `docs/grokbot-design-parity.md` § "Composer and status row vs
Jacob's 2026-10-25 Grok Bot shot" and `media/feedback/iphone-composer-parity/`.

## Mac status row sits inside the last bubble's 54 pt (Jacob 2026-10-26, [feedback status-row-gap-mac-2026-10-26])

Jacob: "Fix this odd spacing that arbos has between the last message and the
chat." Grok Mac keeps the last bubble **54 pt** from the composer pill whether
or not its status row ("Charting the weather") shows, and draws the row *in*
that gap (Jacob's shot, 2 px/pt: bubble → pill 107 px = 53.5; icon 23 pt,
19.5 over the pill; refs 57/58 without a row: 54). Arbos stacked the row's
whole height under the 54 pt: bubble → disc 57 pt, bubble → pill 92.5 pt, and
the list shrank and grew by 38 pt each time the row came and went.

| Item | Rule (pt) | Code |
| --- | --- | --- |
| Last bubble → pill, with or without the row | **54** (one trailing list row, `BOTTOM_INSET`; never container padding) | `transcript::LAST_BUBBLE_TO_COMPOSER` |
| The row's place | floats over that clearance from a zero-height anchor at the composer block's top; the shell draws it only while the list is pinned at its end (scrolled into history the bubbles pass through the band and ↓ sits there) | `shell::render_center`, `Transcript::is_pinned_to_bottom` |
| Disc bottom → pill top | **20** (Grok 19.5) | `presence::COMPOSER_GAP`, `presence::LIFT` = 20 − 3 |
| Last bubble → disc top | **6** = one same-sender bubble gap (54 − 20 − 28); Grok's smaller icon sits 11 under the bubble, centres 2.5 apart | follows from the two above |
| Disc, words, left edge, chip | unchanged: the `+` disc (28) on the `+` column, words at the composer's text start, tasks chip / job-tab link on the row | `presence::row`, `tasks::presence_row` |
| When the row comes or goes, or a reply lands | nothing else moves; the reply takes the next slot under the ack (the 20 pt turn gap of #1447) and the pill stands still | `presence::placement_tests` |

Pinned by `presence::placement_tests` (the real `Shell`, headless: idle →
working → reply landed; last bubble → pill 54 in every state, bubble → disc 6,
disc → pill 20, identical bubble and composer frames across the toggle, the
ack rises by exactly the reply's row; scrolled into history → no row). Record:
project store `internal/feedback-loop/status-row-gap-2026-10-26.md`, shots in
`media/feedback/status-row-gap/`.

## iPhone control family — one size for every round button, pill, field and bar (iPhone only; Jacob 2026-10-26, [feedback iphone-header-composer-sizing-2026-10-26])

**iPhone only.** The Mac keeps its own `CONTROL` family (36 pt discs and pill, 44 pt
composer) unchanged — Jacob 08:52 CR: no Mac size or placement moves with this rule.

Jacob's 08:49 shot (Bubble agent chat): the glass back / search discs and the name
pill (#1829) were 42 pt over a 48 pt composer (#1841) — two families on one screen.
There is now one: `Controls` in `apps/ios/Arbos/Arbos/UI/Theme.swift` is the single
source; `Theme.control`, `Theme.fieldHeight`, `Theme.composerHeight`, `Theme.plusButton`,
`plusFieldGap` and `composerTextInset` are aliases of it. The numbers are the composer's
(Grok Bot's 2026-10-25 shot).

| Token | Value (pt) | Used by |
| --- | --- | --- |
| `Controls.diameter` — round icon button | **48** (= composer `+`) | chat header back / search, home account + `+`, hidden-chats back, server back / `+`, call collapse, composer `+` |
| `Controls.height` — pill, field, bar | **48** (= composer field) | header name pill, Search-in-chat bar, bottom search bar (#1810), settings / profile / secrets / sign-in / join-server fields, sheet search fields, pane Message pill, secrets action buttons |
| `Controls.radius` | **24** (capsule; a growing field keeps 24, the composer's rule) | every field and pill (was 12 / 14 / 25 rounded rects) |
| Outline | `ComposerRim` — #1841's thin light rim (⅓ / ⅔ / 1 pt inner strokes, dark); `fieldBorder` 1 pt in light | every member, Glass and Matte; on iOS 26 glass it overlays `glassEffect` |
| `Controls.glyph` — SF symbol in a disc / bar | **19** (17 × 48 / 42) | chevrons, magnifiers, `+` outside the composer |
| `Controls.inset` — top bar → screen edge | **16** (home was 20) | home, hidden chats, chat, server headers; the Search-in-chat bar |
| `Controls.top` — bar → safe-area top | **6** | the same bars (`.controlBar()`) |
| `Controls.gap` — adjacent controls | **10** (= `+` → field) | composer |
| `Controls.textInset` — text start in a pill / field | **17** (= composer caret) | `.controlField()`, `.controlPill()`, pill trailing pad |
| `Controls.avatar` / `avatarInset` — picture in a title pill | **32 / 8** (concentric with the capsule end; was 28 / 10 in 42) | chat and server header pills |

Build with the helpers, never by hand: `GlassCircle` (disc), `GlassPill` (title pill),
`.controlField()` (one-line field / bar), `.controlFieldGrowing()` (text area that grows
from one line), `.controlPill()` (text pill button), `ControlSurface` (the fill + rim),
`.controlBar()` (bar insets). Surfaces: the chat header's discs, pill and Search-in-chat
bar take the Glass material (#1829); everything else is the `field` fill — both wear
`ComposerRim`.

**Drift guard.** `ArbosTests/ControlTokenLintTests` (and `apps/ios/scripts/control-size-lint.sh`)
fails the iOS gate on any line outside `Theme.swift` that writes `GlassCircle(size: <number>)`,
`.background(Capsule()/Circle()/RoundedRectangle(…).fill(theme.field))` or
`strokeBorder(theme.fieldBorder` without `// control-size: exception <case>` naming one of
`Controls.Exception`. `ArbosTests/ControlMetricsTests` pins the token relations and walks
`Controls.Screen` (home, hidden chats, chat header, Search-in-chat, composer, bottom search,
server header, settings fields, Message pill, sheet search, sign-in, secrets, call header):
every disc is `diameter`, every pill / field / bar is `height`, every bar inset is `inset`.
A new screen declares itself there.

**Exceptions (keep their own size, on purpose):**

| `Controls.Exception` | What | Why |
| --- | --- | --- |
| `composerSideInset` | composer side inset 31 keyboard down / 18 up | follows the keyboard; Grok's measurement (#1841), the avatar column hangs off it |
| `plusGlyph` | composer `+` glyph 22.5 regular | Grok-measured (#1841); the family glyph is 19 |
| `callControls` | call screen 64 pt action discs, 108 pt avatar disc, 150 pt ring, 44 pt chips | Grok's call UI, a different surface |
| `inCallStrip` | in-call strip chip and voice button 36 | the strip is 48 tall |
| `filterChips` | 30 pt All · Agents · People · Servers chips, pane tab pills | Grok's home filter |
| `rowIconTiles` | 36–44 pt leading tiles in rows and sheets, the hero camera badge, avatar-cluster placeholder | row content, not a floating control |
| `illustrationDiscs` | 72 / 92 / 112 pt discs (primer, feedback done, join link, hero) | illustration |
| `transcriptJumpChip` | Grok's 36 pt scroll-to-bottom circle, the "New messages" chip | Grok-measured transcript furniture |
| `inlineChips` | ≤ 32 pt chips and segments inside cards | not controls of the family |
| `primaryCTA` | 50 pt filled capsules (Sign in, Pair, Create) | primary buttons, `primary` fill, no rim |
| `textAreas` | feedback note / description areas, 18 pt corners | free-form, grow |

## Glass surface: readable content, translucent chrome (Jacob 2026-10-26, [feedback mac-glass-contrast-2026-10-26])

Jacob (09:55 CR): "the glass appearance is broken on the mac." Reproduced on
v0.6.268 over white, black and busy desktops at Opacity 0 / 50 / 100 %: GPUI's
`Blurred` window is an `NSVisualEffectView` with every tint layer stripped — a
colourless gaussian of the desktop — so the only tone was the shell wash at
the slider's value. Dark at 0 % over white: sidebar names white on `#afafaf`
(1.9:1), previews gone, discs and hairlines flat; light at 0 % over black the
mirror image. The blur itself survived an appearance switch (System pref
toggled dark → light → dark, no flat frame); that risk did not reproduce.

Rules (ARB-271, the wider Liquid / frosted / tinted restyle, stays paused —
this is the correctness of the existing surface):

| Layer | Rule | Code |
| --- | --- | --- |
| Window material | A real AppKit semantic material with its tint intact, installed once under GPUI's content view; its `appearance` follows the theme (dark material under a dark theme, whatever macOS is set to), so an appearance switch re-tones it and never drops it. Default `Sidebar` (Finder's). GPUI's own background is `Transparent` on Glass so the two do not stack. | `ui::vibrancy` (`BackdropPlan`, `apply`), `Theme::window_background_for`, `GlassMaterial` (`ARBOS_GLASS_MATERIAL=…` for the rig) |
| Opacity slider | Still 0 … 100 %, floor **0 %** (#1425). It sets the shell wash only; legibility never depends on it. | `GlassOpacity`, `Theme::shell()` |
| Chat column | Page-colour wash at **0.85** on both appearances (dark `bg`, light white; light had it since 2026-10-21). Body text ≥ 4.5:1 and muted ≥ 3:1 over white / black / mid grey at every slider value, counting the app's paint alone. | `CHAT_WASH_ALPHA`, `Theme::chat_wash()` / `pane()` |
| Sidebar | Tint **0.60** both appearances (dark was 0.35): names ≥ 4.5:1 over any desktop by the app's paint alone; previews lean on the material below the default Opacity. | `CHROME_TINT_ALPHA`, `Theme::chrome()` |
| Floating cards | Menus, sheets, pickers at **0.85** (dark was 0.50, light 0.72) with the header's 1 pt rim and top highlight on Glass. | `FROST_TINT_ALPHA`, `Theme::card()`, `glass::plate` |
| Untouched | Traffic lights native; sizes and placements; bubbles, discs and hairlines keep their colours (they sit on the washed surfaces above); Matte. | — |

Pinned by `theme::glass_contrast_tests` (the matrix above with a WCAG model of
the compositing) and `vibrancy::tests` (material on Glass, none on Matte,
kept across dark → light → dark).

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

## Right info pane chrome (Jacob 2026-10-24, [feedback info-pane-chrome-2026-10-24])

**The right pane is the window, not a panel: the chat's surface on both sides
of the join, Grok's one-device-pixel hairline on the pane's left edge, Grok
Bot's round close disc (plus the share disc on the agent pane only) at its top
right, a plain hero picture, inset cards under section labels, and no "Delete
agent" button.** Jacob's side-by-side of a fresh Grok
Bot and a fresh Arbos agent (2026-10-24, `media/feedback/grok-parity-test/` in
the project store; items 19–22, 26–27 of `docs/arbos-vs-grok-traits.md`).

Measured on `grok-right-pane-details.png` and `grok-chat-1.png` (2 px/pt,
window top at row 0; pane 639 px = 319.5 pt wide). Points.

| Item | Grok Bot | Arbos (`profiles/mod.rs`, `profiles/kit.rs`, `theme.rs`) |
|---|---|---|
| Pane fill | `#121212` — the chat's own colour; sidebar `#0E0E0E` | `Theme::pane()` = `chat_wash()` (nothing on matte / dark glass, the white wash on light glass); `theme.pane` is gone |
| Join | one device pixel (0.5 pt at 2 px/pt) of `#1F1F1F` on the `#121212` page (+13 ≈ 5 %), full height, in every Grok shot (`grok-chat-1/2`, column 2816 / 2814) | `border_l(PANE_EDGE)` 0.5 pt in `theme.pane_edge` — dark `#212121` on `#141414`, light `#EFEFEF` on `#FCFCFC` (the same 5 % step) — on the pane and the mid-slide blank. History: v0.6.224 (#1675) drew none, reading Jacob's trait 20 as "Grok has none"; Jacob re-checked Grok on 2026-10-25 and asked for the hairline ([feedback info-pane-left-hairline-2026-10-25]). It is a colour, not `border`'s alpha, so glass does not change it; never the 1 pt `border` line of the old panel |
| Cards | `#1F1F1F` on `#121212` (+13), inset **16 / 16**, radius **12** | `pane_card` `#232323` on `#141414` (+15), `kit::INSET` 16, `kit::CARD_RADIUS` 12 (was `PANEL_RADIUS` 14); light: `#F2F2F2` on `#FCFCFC` (white on white was no card) |
| Rows | one-line **41**, two-line **51**, 13 pt title / 13 pt muted subtitle | `kit::ROW_MIN_H` 41 (was 48); a title + subtitle row lands on 51–52 by its own padding; row type unchanged (`TEXT_BODY` / `TEXT_SMALL`) |
| Section label | "Coding" / "Routines": text **28** from the pane edge, 13.5 pt `#A5A5A5`; glyphs 24 under the pill, 26 under the card before, 10 over the card after | `kit::section_title`: `INSET + SECTION_TITLE_INDENT` 28, `TEXT_SMALL` muted on a 16 pt line, padding 10 / 8 over `CARD_GAP` 14; a leading label pulls the fade spacer in by `pane::FIRST_TITLE_PULL` 7 |
| Share · close discs | **36** each, fill `#1F1F1F`, glyph `#E0E0E0`; **8** apart, **12** from the pane edge, centred on the strip discs' line (top 12 in the 60 band; `grok-chat-1.png` rows 28–99 for both sets) | `glass::control_disc` — the strip's disc (`CONTROL` 36, `surface_2`, `text`); `top_bar()` pads `PANE_DISC_INSET` 12, gaps `CONTROL_GAP` 8; close `xmark` at `PANE_CLOSE_GLYPH` 22 → 11 pt mark (Grok 11); share `share` at `PANE_SHARE_GLYPH` 21 → 14 pt (Grok 14). Before: a 30 pt `glass::circle` with a 6.5 pt × at top 17, inset 10 |
| Share disc action | — | **agent pane only** (owner: the invite sheet — let a person in on this agent). Every other pane — group, server, person, channel, own profile — draws the × alone on the discs' line (`pane::share_disc_allowed`). Jacob 2026-10-25, [feedback info-pane-no-share-disc-groups-2026-10-25]: "remove this button from side bar on group/" — the group's share disc (Add members) of v0.6.224 is gone; Add members stays the group's first card |
| Top bar fill | none | none (was `card()`); the body still fades under the pinned hero + pill |
| Hero picture | plain bot mark, 58 pt, 73.5 under the window top | `HERO_TOP` 14 under the 60 band (was 4); the agent's leaf **without its pastel disc** (`avatars::project_hero_bare`: the hue's pastel as ink on dark, its ink on light) and **no presence dot** — presence stays on the sidebar row; `HERO_AVATAR` 72 kept (Jacob's mock b) |
| Delete agent | — | **gone from the pane** (footer and `Shell.agent_delete_armed` removed). Deleting an agent is the sidebar row's menu → Delete, at once (`sidebar::row_menu`, the Delete-is-immediate rule below) |

Pinned by `profiles::chrome_tests` (`the_pane_is_the_chats_surface_in_both_appearances`,
`the_constants_are_groks`, `the_share_and_close_discs_land_on_groks_pixels`,
`cards_are_inset_and_labelled_at_groks_margins`, `the_pane_edge_is_groks_hairline`),
`pane::tests::every_kind_lays_out_hero_then_pill_then_options` and
`pane::tests::only_the_agent_pane_may_have_a_share_disc`.

### Grouped-card dividers: inset 16 at both ends (Jacob 2026-10-26, [feedback mac-settings-card-dividers-2026-10-26])

**Every divider between two rows of a grouped card stops 16 pt short of the
card's left edge and 16 pt short of its right edge — the row's own content
padding — on every Mac grouped list: the agent pane's Settings card, group /
server / person / channel info, the Settings pages, profile cards, the
Library list, the New message / New channel sheets.** Jacob's v0.6.268 shot of
the agent Settings card (Model · Reasoning · Voice): the lines started under
the labels but ran into the card's right edge, so the card read lopsided.
Before, the kit hung the line "under the label" (16 past an icon tile → 58,
past an avatar → 62) and to the right edge, Apple's iOS 6 habit; Grok Bot's
cards do not do this either.

| Item | Rule | Code |
|---|---|---|
| Leading inset | **16** (`kit::DIVIDER_INSET` = `kit::INSET`, the row padding), whatever leads the row — nothing, a tile, an avatar, a thumbnail | `kit::divider_insets()` → `(16, 16)` |
| Trailing inset | **16**, the same number | `kit::hairline_below` (hung 1 pt under a row, `.relative()` on the row) and `kit::divider` (in flow, for a card that places its own rows) both draw from it |
| Height / colour | 1 pt, `theme.border`; none above the first row, none under the last (the card's clip swallows the hung one; `section_parts` places `rows − 1` in-flow ones) | unchanged |
| Who draws it | **only the kit.** `glass::row` (Settings pages) hangs `kit::hairline_below`; the New message / New channel sheets and the Add-members lists use `kit::divider` / `kit::hairline_below` instead of their own 0.5 pt lines and `border_b_1()` rows. A card that needs a line between two custom children calls `kit::divider(theme)` | `kit::tests::no_card_outside_the_kit_draws_its_own_divider` scans `src/ui` for a left-margin-only hairline, a `left(..)/right(0)` one, a 0.5 pt one, or a list row's own bottom border |

Pinned by `kit::tests::the_divider_is_inset_equally_at_both_ends_by_the_row_padding`
(pure) and `kit::tests::every_shared_divider_stops_at_the_row_padding_on_both_sides`
(headless render: tile row, avatar row, `glass::row`, `section` — each line's
left and right gaps to its card measure 16). iPhone unchanged.

### Agent pane: Details · Library · Computer (Jacob 2026-10-24, [feedback agent-pane-details-tab-2026-10-24])

**The agent pane's tabs are Grok Bot's — Details · Library · Computer — and
the first tab shows live content first.** Items 23–25 of Jacob's list:
Grok shows the name over "Add a label"; its Details tab opens with the
coding project (spinner while the bot works) and the routines with their
switches; ours opened with Name / Handle / Description rows and fine print.
This supersedes the 2026-10-22 tab names (Profile · Settings · Library) for
the Mac agent pane; the structure every pane shares — hero, then the tab
pill, then the options — is unchanged, and every option the old tabs held is
still on the pane. The iPhone agent pane has the same three tabs and the same
content model since [feedback iphone-agent-pane-tabs-2026-10-25] (#1694:
`PaneKinds.agent`, `AgentProfileView`, `Model/AgentPane.swift` — the pure
order / copy twins of this file's functions, pinned by
`ProfilePaneStructureTests`); where the Mac geometry does not translate it
keeps the iPhone's shared pill and 48 pt rows, uses the system spinner, and
edits the label in place but the name through an alert.

| Item | Grok Bot (measured, `grok-right-pane-details.png`) | Arbos (`profiles/agent.rs`) |
|---|---|---|
| Subtitle | "Add a label", 12 pt muted, under the name | the agent's **description is its label**: shown under the name (`HERO_SUBTITLE` 12, `text_muted`), `LABEL_PLACEHOLDER` "Add a label" in `text_faint` while empty (owner); a click edits it in place (the hero's `label_editor`, Done / Return commits). Someone else's agent: its label, else "Run by @owner". "working" is no longer a subtitle — the Coding row spins |
| Tab pill | capsule **26** tall, label **13** pt, 9 pt beside it, 2 between capsules | `TAB_HEIGHT` 26 / `TEXT_SMALL` / `TAB_PAD_X` 9 / `TAB_GAP` 2 (shared `tab_bar`; was 30 / body / 14 / 4) |
| Details, first | "Coding": the project with a 15 pt blue spinner, 41 pt row | **Coding** card — **hidden for now** (see the 2026-10-25 trim below): the agent's workspace — `cards::activity_spinner` (`CODING_SPINNER` 15, `accent`, eight spokes stepping every 100 ms) while a turn runs, the `code` glyph when idle; subtitle "Working · 42 s · 7 tool calls" / "Idle" / "Machine asleep" (`coding_status`); a click opens the Computer tab |
| Details, second | "Routines": name over schedule, a 32 × 20 switch | **Routines** card (own agents) — **hidden for now**: one `kit::row` per routine, subtitle "trigger · next in …" / "Paused" (`routine_status`), a `glass::switch` that flips `routine_set_enabled`; "No routines yet" when empty. Routines still load when the pane opens (`RoutinesRefresh`) |
| Details, then | — | **Settings** (Model · Reasoning · Voice, the hoverpick rows; Model alone on someone else's agent). No footnotes. The **Who may message X**, **Permissions** and **Profile** cards that followed it are gone (below) |
| "Run by …" (someone else's agent) | — | a **link**, not a label (Jacob 2026-10-25, [feedback agent-pane-run-by-link-mac-2026-10-25]): "Run by Sami Kassab" (display name) or "Run by @sami" (`run_by_label`), drawn as a mention-chip pill under the name (`run_by_link`: accent text, accent pill at 14 % rest / 22 % hover / 30 % pressed with `mention::ChipPointer`'s 120 ms ease-out, pointing hand); a click pushes the owner's person page over the agent's (`Shell::push_profile` → `ProfilePage::User`), the top bar's `‹` disc brings the agent back on its tab and scroll. With no label the link is the subtitle; under a label it is the line below. Rig `ARBOS_OPEN_PROFILE=agent:foreign[:owner]` with `ARBOS_FIXTURE_FOREIGN_AGENT=1` |
| Library | Grok's second tab | the one list by day with the hover card ("Library tab" below) |
| Computer | Grok's third tab (the bot's computer) | the agent's **box**: **Machine** card — Status (dot + "Awake · 5 h 12 m"), Where it runs, Machine spec, Image, Spend today (`GET /machine`); **Resources** — Disk / Memory "x free of y", Load, Up for; **Running now** — every workspace with a turn running (spinner, name, "42 s · 7 tool calls") or "Nothing running · N workspaces". The last two come from the box's own `/box/state` through **`GET /machine/debug`** (`ClientCommand::MachineDebugRefresh` → `SyncEvent::BoxReport` → `AppState.box_report`), fetched when the pane opens and when the tab is picked; an asleep box says "The machine did not answer · <reason>". Someone else's agent: Status + Machine class from its project |
| Delete agent | — | not on the pane (see above) |

Owner link pinned by `agent::run_by_tests` (label; headless click pushes the owner's page and `‹` returns).
Order pinned by `agent::tests` (`details_is_settings_alone_until_the_live_cards_are_switched_back_on`,
`computer_order_follows_the_report`, `coding_status_…`, `routine_status_…`,
`box_report_reads_the_relay_and_the_box`) and `pane::tests::kinds_keep_their_own_tab_sets`.
Rig: `ARBOS_OPEN_PROFILE=agent` (Details), `agent:library`, `agent:computer`;
the fixture answers `MachineDebugRefresh` with an awake 4-core box and one
workspace per agent (the streaming one mid-turn).

**Details trimmed to Settings (Jacob 2026-10-25, [feedback agent-details-remove-sections-mac-2026-10-25], iPhone [feedback agent-details-remove-sections-ios-2026-10-25]).**
Five blocks left the Details tab on both platforms, the same release each:

| Block | Mac | iPhone | Status |
|---|---|---|---|
| "Who may message X" (grant rows, "Give a person access…") | `DetailsBlock::Access`, `message_grant_rows` removed | `.access`, `messageGrantRow`, `AddMessageGrantSheet` removed | **gone.** The access list itself is untouched: `message_grants` on the project, `PUT/DELETE /projects/{id}/message-access`, the state methods and the relay calls stay |
| "Permissions" (Shell approvals row, its caption, one switch per grant) | `DetailsBlock::Permissions`, `shell_grant_sections` removed | `.permissions` removed | **gone.** Standing answers from the destructive-shell card (#1544) still apply on the box; `ShellGrantsRefresh` / `shell_grant_revoke` and the relay endpoints stay. The card's footer no longer points at the pane ("Standing choices are per agent: X stops asking for what you allow here") |
| "Profile" ("Edit profile" → Name · Handle · Description) | `DetailsBlock::Profile`, `profile_rows`, `Shell.agent_edit_open` removed | `.profile`, `profileRows`, `editOpen` removed | **gone.** Name and label are edited in the hero as before (Mac: click the name / the label). iPhone: the label was already in place; the name had no other way in, so the hero's name is now a tap → the same rename alert (`PaneHero.nameAction`, nil on every other pane — a second inline editor would stack on the label's) |
| "Coding" | `DetailsBlock::Coding` | `.coding` | **hidden, temporary** — behind `AGENT_DETAILS_SHOW_CODING_ROUTINES` (Mac, `profiles/agent.rs`) / `AgentPane.agentDetailsShowCodingRoutines` (iPhone, `Model/AgentPane.swift`), compile-time, **off** by default. The Computer tab, `coding_status` and the box report are unchanged |
| "Routines" | `DetailsBlock::Routines` | `.routines` | **hidden, temporary** — the same flag; `RoutinesRefresh` / `routine_set_enabled` and the relay calls stay |

What is left on Details: the **Settings** card alone (Model · Reasoning · Voice;
Model alone on someone else's agent) under the hero (picture, name, label).
`details_order(mine, show)` / `AgentPane.detailsOrder(mine:showCodingRoutines:)`
are pure and pinned for both flag states. Rig: `ARBOS_SHOW=agentrename` opens
the iPhone rename alert (replaces `agentedit`).

### Library tab: one list by day, hover preview (Jacob 2026-10-25, [feedback library-list-hover-mac-2026-10-25])

**The Library tab is Grok Bot's list, not a grid: everything shared — images,
videos, files and links together — newest first, grouped by day inside the
pane's cards; a row is a small thumbnail or type tile, the name on one line
and the relative time; the pointer swaps the time for a ⋯ menu; a day shows
five rows then "Show more"; resting on a row floats a preview card over it.**
Jacob: "make library list based with hover pop out". Built from the approved
mock (`media/feedback/library-list/` in the project store) — Jacob sent no
Grok Library shot, so the list's own numbers below are the mock's on Grok's
pane metrics, not measurements. Same on the group pane (Members · Library) and
the agent pane (Details · Library · Computer); `profiles/library.rs`.

| Item | Rule | Where |
|---|---|---|
| Grouping | one list, `when` descending, split by local day: **Today · Yesterday · weekday (last 7 days) · "Oct 18"** (the sidebar's words; "Oct 18, 2025" in another year); each day = `kit::section_title` + one `kit::card` | `group_days`, `day_label` |
| Row | the kit's one-line row: **41** tall, inset 16, 10 right; **32** pt thumbnail (aspect-fill, radius 7) — a play glyph over a video, the extension ("PDF") on a `surface_2` tile for files, the link glyph for links; name 13 pt one line, tail-truncated; time 11.5 pt muted, compact ("now", "42m", "13h", "2d", "3w", "4mo"); the kit's divider (16 / 16, below) | `ROW_H`, `THUMB`, `rel_time` |
| Hover | row fill `surface_2`; the time gives way to a **24 pt ⋯ disc** (no layout shift) that opens the row's menu at the click: Open · Forward · Download for attachments, Open link · Copy link for links | `MORE_DISC`, `menu_items` |
| Show more | after **5** rows a 36 pt "Show more ⌄" row unfolds the rest of that day in place ("Show less ⌃" folds it); per opened pane | `SHOW_MORE_AFTER`, `library_toggle_day` |
| Preview card | after **300 ms** on a row: **300** wide, left **20** from the pane's edge, **8** above the row (under it when it would reach the window's top 8 pt band); padding 10, radius 12, a step over the card's fill (white in light) with the strong border and shadow; picture **278 × 156** radius 8 aspect-fill (files: the extension at 22 pt on the tile; links: a globe); type line 11.5 muted ("Image" / "Video" / "PDF" / "Link"); the full name 13 semibold on 17, up to 3 lines; the sender's 18 pt picture, name, "· Yesterday 10:06 PM" (`sidebar::stamp_label`) | `HOVER_DELAY_MS`, `CARD_*`, `card_above` |
| Click | a row opens the viewer at its place in the chat-order album (files included: the viewer's Open / Download); a link opens in the browser — as before | `open` |

The iPhone has the same list since [feedback library-list-ios-2026-10-25]
(`apps/ios/Arbos/Arbos/UI/LibraryList.swift`: `LibraryList` is the pure twin —
`days`, `dayLabel`, `relTime`, `visibleRows`, `kindLabel`, `menuItems` — on
the iPhone kit's 48 pt rows with a 36 pt thumbnail; the long press is the
system context menu with the preview card on top: Open · Forward · Save /
Open link · Copy link; `LibraryListTests` holds the twin tests).

Pinned by `library::tests` (times, labels, grouping, the five-row fold, the
card's height and flip, the menus) and the headless `library::render_tests`
on the real shell (one list by day, Show more, ⋯ in the time's place with no
layout shift, the card 8 pt over the row after the delay, the menus). Rig:
`ARBOS_OPEN_PROFILE=group:library` / `agent:library`; `ARBOS_LIBRARY_HOVER=<row>`
draws that row hovered with its card up.

### Person pane: the Message pill (Jacob 2026-10-25, [feedback profile-message-button-mac-2026-10-25])

**Another person's pane carries one primary action — a Message pill under
the name and @handle — that opens the 1:1 DM you have or starts one; your
own pane and a person you blocked show no pill.** Jacob: "Add a message
button to a user." The iPhone has the same button in the same place
([feedback profile-message-button-ios-2026-10-25]).

| Item | Rule | Where |
|---|---|---|
| Place | centred in the hero, 12 under the "@handle" line (the hero's 6 pt gap + `HERO_ACTION_GAP` 6), before the tab pill | `Hero.action`, `pane.rs` |
| Shape | one of the Grok button family: `theme::CONTROL` **36** tall, a capsule, side padding 18; `surface_2` fill (the discs'), the `bubble` glyph at 17 and "Message" 13 pt semibold in `text`, 7 apart | `kit::pill_button`, `PILL_*` |
| Pointer | hover lifts the fill `DISC_HOVER_LIFT` 0.06 in lightness, pressed `DISC_PRESSED_LIFT` 0.12 — the chip rule, as the footer's update disc | `kit::pill_button` |
| Click | `AppState::start_dm(handle)`: the existing people space with that person is selected; else `POST /chats/dm` (the New Message sheet's path) and the DM is selected when the registry lists it. A relay refusal ("this person is not available" — they blocked you) shows as a notice | `user.rs`, `app.rs` |
| Hidden | on your own pane; once *you* blocked them (`GET /blocks`); the reverse direction is not in any payload the client reads, so the relay's answer covers it | `user::shows_message` |

Pinned by `profiles::user::tests::the_message_pill_is_for_other_unblocked_people`
and the headless `profiles::user::render_tests` (under the @handle at 36,
centred; click opens the DM we have / starts one; none on my own pane or a
blocked person's, back once unblocked). Rig: `ARBOS_OPEN_PROFILE=user`
(the DM peer's pane), `user:message` (the pill pressed), `group:member`
(a member with no DM yet).

### Profile targets toggle the pane (Jacob 2026-10-26, [feedback profile-pane-toggle-mac-2026-10-26])

**Every picture or name of a person or an agent in the chat is one profile
target: a click opens their page in the right pane; a click on that target —
or on any other target for the same person or agent — while their page shows
closes the pane; a click on someone else swaps the page in place, with no
back stack.** Jacob (2026-10-26, Allan's bubble with the @Affine chip):
"clicking the agent or user in the chat should open their user profile right
bar. Clicking it again should close it. Same if you click the user profile
picture to open the side bar clicking it again should close it." Same in
agent chats, DMs, groups, server channels and job tabs; the iPhone's sheet
follows the same single action ([feedback profile-pane-toggle-ios-2026-10-26]).

| Target | Before | Now |
|---|---|---|
| Sender name in a person's bubble (group / channel) | pushed the person's page over whatever was open; a second click did nothing | toggle |
| Sender name in an agent's bubble | pushed the agent's page; second click nothing | toggle |
| Disc beside a person's / an agent's bubble | as the name (push, no close) | toggle, the same target as the name |
| @mention chip, people and agents (bubble text and the composer) | opened and swapped, never closed; a person's chip page carried no `space_id`, so the DM pill did not see it as "open" | toggle; a person is the same person whichever chat the page came from |
| Chat header pill — agent chat / job tab | toggled (after the 250 ms double-click window on an owned agent) | unchanged, now labelled |
| Chat header pill — DM | toggled, but only against a page with its own `space_id` | toggles against the person, from any target |
| Status-row pictures (typing people, working agents) | nothing: no click, arrow cursor | toggle; a face no card can name (`AgentIdentity::known` false) stays inert |
| Sidebar footer / rail: my picture | toggled `Me` | unchanged |
| Pane rows (members, "Run by …" owner link) | push, `‹` returns | unchanged — a page's own rows still stack |

One action: `AppState::toggle_profile(page)` — close when
`profile_shows(&page)` (`ProfilePage::same_subject`: a person by handle,
case-insensitive, whatever the `space_id`; everything else by id), else
`open_profile` (the stack clears). `ui::profile_target::attach` gives a
target the pointing hand, the `Button` role, the accessible label — **"Open
<name>'s profile"** / **"Close profile"**, from the pane's state at draw
time — and the click; `profile_target::mark` is the same without the click
for the header pill, whose listener keeps the rename double-click. The
status-row pictures carry `Participant.page` (person by handle, agent by
project) and `presence::row(.., Some(&state), ..)` attaches them. The DM
pill's ring (`header::render` `pane_open`) and `pane::is_open` read
`profile_shows` too. Pictures come from the shared resolver as before
(`people::avatar`, `avatars::agent_picture` / `agent_identity`).

Pinned by `profile_target::tests` (label; same-subject; a source scan that
`transcript.rs`, `presence.rs`, `mention.rs` and `header.rs` call no
`open_profile` / `push_profile` of their own) and the headless
`profile_target::render_tests` on the real shell — Allan and Affine in a
group, Allan typing, his bubble with the @Affine chip, a DM with Allan:
`every_target_opens_and_a_second_click_closes` (name, disc, chip, status-row
face, for a person and an agent), `any_target_for_the_same_profile_closes_it`,
`a_different_person_swaps_the_page_in_place` (no ‹, empty stack),
`the_header_pill_toggles_the_same_person_as_the_bubbles`,
`the_label_tracks_whose_page_is_showing`. The test platform exposes neither
the cursor nor the AccessKit tree, so hand and label are pinned by
construction (every target goes through `attach` / `mark`; the scan guards
the road).

## Pane animations (Jacob 2026-10-24, [feedback right-pane-slide-2026-10-24], [feedback sidebar-slide-2026-10-24])

**The right info pane and the left chat sidebar slide; they never pop.**
Jacob: "on the mac the side bar appears when you click the top button. I
want it to slide animate out like it does on grok. Same for the left bar
when you click the close side bar button." Grok Bot's desktop app could not
be run on the rig (no install), so its right pane was measured frame by
frame on xAI's own 30 fps screen recording (the "How I run multiple teams
of Grok Bots" guide video, t = 51.8 s open / 55.1 s close; strips and CSV in
the project store's `media/feedback/pane-slide/grok-reference/`): open
≈ 200 ms (6 moving frames, soft start, long tail — ease-out cubic RMS 0.022,
a 0.21 s / 0.9 spring 0.015), close ≈ 150–175 ms (4 frames, no slow start —
ease-out quad RMS 0.008), the pane **translates** with its content. Its
sidebar toggle is in no public video; it gets the same motion. Numbers,
recorded here so they do not drift:

| Item | Value |
|---|---|
| Duration | **200 ms open, 170 ms close** (`slide::OPEN_MS` / `CLOSE_MS`); a click mid-slide turns round from where the pane is, in the time the remaining distance deserves |
| Curve | open **ease-out cubic** (`motion::ease_out_cubic`, the composer drop's curve), close **ease-out quad** (`slide::Motion::ease`): fast off the mark, soft landing, never symmetric ease-in-out |
| Right pane | its left edge comes in from the window's right edge and goes back out the same way; the pane keeps its layout and moves with its edge (not a reveal, not a fade) |
| Left sidebar | its right edge slides to the window's left edge and back; the strip's discs leave with it, and the header's expand disc appears at its resting spot (72 + 12) as soon as the sidebar no longer covers it — never two discs at once |
| Chat column | a flex sibling, so its slot follows every frame: the header pill and the composer re-centre live. The **transcript keeps the width it had when the slide began** and is clipped/offset — centred when the rows are capped at both widths (`COLUMN_MAX_WIDTH`: then nothing moves at the end), else pinned to the edge away from the moving pane — and is laid out once more at the end. Never re-measure every bubble per frame. (Grok re-lays its list out every frame; its list is short rows, ours is a markdown transcript whose `list()` drops every item height on a width change — the one deliberate difference) |
| Frame rate | one redraw per display refresh (`request_animation_frame`), 12 / 10 frames at 60 Hz |
| Reduce Motion | macOS Accessibility → Display → Reduce motion (`cx.reduce_motion()`): no slide, both panes snap as before |
| At rest | the layout is today's static layout — the clips exist only while a slide runs (`slide::layout_tests::at_rest_the_clipped_layout_is_the_static_layout`); strip, filter, folded toggle and header pill geometry unchanged |

Code: `ui/slide.rs` (`PaneSlides::step` derives the motion from
`AppState::profile` / `rail_collapsed` each frame; `sidebar_clip`,
`pane_clip`, `transcript_clip`), wired in `Shell::render` and
`Shell::chat_frame`; pinned by `slide::tests` (state machine, reduce-motion
snap, mid-slide reversal, no slide on a page swap or a width drag) and
`slide::layout_tests`.

## Create sheets: Upload | Generate, neutral initial (Jacob 2026-10-25, [feedback create-sheet-avatar-upload-generate-mac-2026-10-25])

**The Create New Agent / Group / Server sheets offer exactly two picture
options — the agent avatar picker's own Upload and Generate — and never a
leaf grid or pastel swatches. An entity created with neither gets the
name's first letter on a neutral grey disc; no leaf is ever auto-assigned.**
Existing agents, groups and servers keep the avatars they have (stored or
automatic leaf, picture): nothing is migrated or re-rendered.

| Item | Rule |
|---|---|
| Options | `avatar_picker::tab_pill` with `Tab::for_target(&Target::Draft)` = `[Upload, Generate]` (Upload first, Jacob's order); the bodies are the picker's `upload_body` / `generate_body` on the shell's `create_picker` (a `Picker` on `Target::Draft`) |
| Where | capsules at **246** from the sheet top (where Grok's first icon row began), body 16 under; footer 12 under the taller body; the card is 461.5 tall (was 602.5 with the 4 × 4 grid and two swatch rows) |
| Preview | the hero disc (114) shows the chosen file, the generated picture, or the neutral initial of the typed name (an empty grey disc until a name is typed) — `avatars::neutral_element` |
| Generate before the entity exists | `POST /me/avatar/generate` into my own slot `users:{me}` (#1789); on Create the bytes are copied into the new entity's slot (`sync::relay::adopt_draft_picture`); a relay without the route says "Generate is not on this relay yet; upload a picture instead." |
| Stored value | `avatar_leaf` = `avatar_leaf_color` = **255** (`arbos_core::avatar::leaf::NEUTRAL`), accepted by the relay's `check_leaf_pick`; `null` stays the automatic leaf, so older agents are untouched. An older client's `pick_for` ignores 255 and draws the automatic leaf |
| Neutral disc | dark `#3A3A3A` with the letter in `#E5E5E5`; light `#E6E6E6` / `#4A4A4A` (`leaf::NEUTRAL_DARK` / `NEUTRAL_LIGHT`); one letter, semibold at `INITIALS_SCALE`; bubble-bar accent a mid grey (`leaf::neutral_accent`) |
| Everywhere | sidebar rows, header pill, pane hero, bubbles, presence row, mention chips and the composer picker all go through `avatars::agent` / `project` / `agent_picture` / `mention::Picture::Neutral`, so the one stored value draws the one disc |

Pinned by `create_entity::tests` (options, stored pair, geometry) and
`create_entity::render_tests` (headless render of both sheets: the two
capsules, no Bot tab, no leaf element, the footer where the constants say,
nothing picked → no picture sent), `avatar_picker::tests::the_draft_offers_upload_and_generate_only_and_hands_over_its_pick`,
`avatars::tests::neutral_pick_is_the_core_value_and_identity_carries_it`,
`arbos_core::avatar::leaf::tests::neutral_is_its_own_value_…` and the relay's
`avatar::tests::neutral_pick_is_accepted_and_the_rest_still_validates`.
Rig: `ARBOS_OPEN_NEW_AGENT=1` / `ARBOS_OPEN_CREATE=group|server` with
`ARBOS_NEW_AGENT_NAME=…` and `ARBOS_CREATE_PICTURE=upload|generate[:<prompt>]`.

## Error messages: one element, one place per surface (Jacob 2026-10-26, [feedback error-messages-mac-2026-10-26]; iPhone [feedback error-messages-ios-2026-10-26])

**Every failure the user sees is one shared element — `ui::error_note` on the Mac, `ErrorNote`
on the iPhone — drawn in one fixed place per surface, with one plain sentence; the raw words
live behind Details / Copy and never in the sentence.** Jacob's ask ("fix and universalize nice
error messages everywhere so that they look professional and are always in the same place")
after a raw `POST …/avatar/generate: error sending request … operation timed out` ran across
the avatar picker's footer over Cancel / Set avatar (his shot:
`media/feedback/error-messages/jacob-before-mac-avatar-generate-timeout-raw-error-over-buttons.png`;
the approved mocks: `media/feedback/error-messages/mock-errors-*.png`).

| Part | Rule |
|---|---|
| The element | a `danger_soft` block (radius 10, padding 10 / 12), a 15 pt `danger` disc with a white "!" — **the only red** — the sentence `TEXT_SMALL` 13 / 17 in `text`, then the actions: **Try again** (a 24 pt `surface_2` capsule, 12.5 semibold; **Sign in** on 401 / 403) and **Details** (12.5 `text_muted` + chevron) at the end of the sentence's line when they fit, else right-aligned on a row under it. Details unfolds a `code_bg` mono block (11 / 15 `text_muted`): *what · call · where / when / build*, ids shortened in the middle, with a 20 pt **Copy** capsule (the full raw error for a feedback report). A toast adds `shadow_md` and a × |
| The words | `sync::user_error::UserError` — a **verb** (what the user tried: "generate the image", "send your message", "save the secret") and a **class**: Timeout "The server took too long." · Offline "You're offline." / "Arbos can't reach the server right now." · Refused (4xx) the relay's own sentence, else "That wasn't accepted." · NotFound "It isn't there any more." · Server (5xx) "The server had a problem." · RateLimited (429) "Too many requests — wait a moment." · ContentRefused "That prompt was refused — try different words." · Upload "The file couldn't be read." / "That file is too large (25 MB max)." / "That file type isn't supported." · AuthExpired "You were signed out." · Unknown "Something went wrong." Sentence = "Couldn't <verb>. <why>". Try again only where a retry can help (not on content refused, 404, size / type, a 4xx with the relay's reason); Sign in replaces it on 401 / 403 |
| Where (takes space, never absolute over controls) | **Sheets** (avatar picker, Create sheets, New Message / group / Add members, server sheets, feedback): the top of the body under the bar / tab pill, before the first control; the sheet grows by the note. **Pane pages** (Secrets, Devices, Updates, connectors, routines): under the pane header, before the first card / field. **Composer** failures (send refused, offline, attachment, recording): directly above the composer, the composer's width; the failed bubble keeps its red mark (the word "Retry" moved into the note). **Chat-level** ("Messages in this chat aren't being delivered right now.", "Can't reach your machine."): the slot under the header where the machine banner lives, the column's width. **Toasts** (download / save / forward / connect failures with no surface up): the notice stack top-right under the header, 380 wide, 10 s (info notices 6 s), the pointer holds it, × dismisses, Try again re-issues the command. **Sign-in**: under the button row and the hint, 420 wide. Field validation ("Pick someone…", "Give the routine a name.") is the same element with the words as a refusal |
| Never | raw URLs, HTTP verbs, status numbers, ids, Rust / Swift error chains or `localizedDescription` in a sentence; red body text; a failure as a plain notice card; a failure drawn at a sheet's footer or over buttons |

Code: `sync/user_error.rs` (`UserError::classify / from_relay / from_status / from_text / of`,
`Fault`), `Shared::fail` / `fail_with` in `sync/relay.rs` (every relay-side failure; `notice` is
for things that worked), `SyncEvent::Failure { error, retry }`, `AppState::push_failure`,
`send_errors` (the composer slot), `failure_claim` (a pane form up takes its verbs' failures),
`ui/error_note.rs` (`render`, `Slot`, `Handlers`, `Style`). Pinned by
`error_note::error_ui_lint` (source scan: no `notice(format!("…{err}"))`, no
`.text_color(theme.danger)` outside the element without `// error-ui: exception <case>`, every
surface's note present through `error_note::render` with its id) and the headless
`avatar_picker::me_picker_tests` (`a_timed_out_generation_shows_the_shared_note_above_the_body`,
`a_failure_event_is_an_error_toast_with_details_try_again_and_close`). Rig:
`ARBOS_RIG_FAILURE=toast|composer|delivery|machine|secrets|people`; a Generate prompt containing
"timeout" fails after 2 s.

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

## Secrets agent picker: chips + "Add agent" (Jacob 2026-10-26, [feedback secrets-agent-picker-mac-2026-10-26], iPhone [feedback secrets-agent-picker-ios-2026-10-26])

**Wherever Secrets picks agents, the agents already granted are chips (picture ·
name · ×) and one "Add agent" button opens a searchable list of the rest; an
agent is never a toggle chip.** Jacob's shot (Mac v0.6.268, 09:14 CR): the New
secret form drew his 16 agents as a wall of chips. Approved as mocked 09:48 CR
(`media/feedback/secrets-agent-picker/` in the project store). One component per
platform, used by all four pickers — New secret, Connect 1Password, "Give another
agent access…" on a secret, the same on a vault: Mac `ui::agent_picker` (state
`SecretsPanel::agent_picker`), iPhone `AgentPickerField` + `AddAgentSheet`
(`UI/AgentPicker.swift`). A new agent picker in Secrets uses it or does not ship.

| Item | Mac | iPhone |
|---|---|---|
| Chip | **30** tall capsule, `field` fill + `field_border` rim, picture **20** (`avatars::project_with`, the shared resolver's newest copy — never `project()` alone), name 13 medium, × 9 pt in `text_muted` (hover `text` on a `surface_2` disc); × removes the grant at once | 34 tall, `field` + `fieldBorder`, picture 20 (`AgentAvatar` of the resolver's project), name 15 medium, × 11 muted |
| Add agent | the same 30 capsule on `surface_2` with `plus` 13 + "Add agent" 13 semibold — the chip hover/pressed lifts (`DISC_HOVER_LIFT` / `DISC_PRESSED_LIFT`); lifted while open | 34 capsule on `bubbleAssistant`, `plus` + "Add agent" 15 semibold |
| Empty | the button alone + "No agent can use it yet." (`TEXT_TINY` muted) | the same |
| Picking | a dropdown **6** under the chips row, left-aligned, **308** wide (the pane's content width), `hoverpick`'s frame (`card()` fill, `border`, radius 10, shadow; flips above the row when the window has no room below): search row 34 (magnifier 13, `ComposerInput` at 13 pt, "Search agents"), rows 36 (picture 20, name 13, `@handle` 12.5 muted), the highlighted row on `surface_2` with ↩, a 22 pt footer "↑↓ move · ↩ add · esc close"; ≤ 6 rows before it scrolls | a stacked sheet "Add agent" (Cancel): a search field of the control family (`.controlField()`: `Controls.height` **48**, the composer's rim — the iPhone control-size token, not the secrets forms' old 42) focused with the keyboard up, "YOUR AGENTS" `PaneCard` of 48 pt `PaneRow`s (36 pt picture, name, `@handle`, a + disc) |
| Keys | typing filters; ↑ / ↓ move the highlight (`ComposerInput::on_arrow`); ↩ adds the highlighted agent (`on_submit`); esc or a click elsewhere folds the list (`Shell::escape` / `close_popovers` → `SecretsPanel::close_agent_picker`); the chips stay | ↩ on the keyboard adds the first match; a tap adds that row |
| One per visit | ↩ / a click adds one agent and folds the list (the button reopens it) | a tap adds one agent and dismisses the sheet |
| Matcher | **no filter of its own**: `agent_picker::candidates` → `people::agent_picker_rows` → `arbos_core::search` ([feedback shared-search-matcher-mac-2026-10-26]): name, `@slug` and description; exact, prefix, word-start; case, diacritics and a leading `@` folded; best first, registry order in a tie. The site is listed in `search::picker_sites_tests::SITES` | `PeoplePicker.agents` (the same core matcher through the FFI, [feedback shared-search-matcher-ios-2026-10-26]) |
| Hidden from the list | guests' agents, people spaces, agents already chips, agents with access on the page (`exclude` = the enabled rows of the secret / vault) | the same (`exclude`; the Give sheets prefill `picked` with the enabled rows, so × there removes access on Save) |

Pinned by `agent_picker::tests` (candidates through the shared matcher) and the headless
`secrets::picker_tests` (the real Shell on the four pages: Add agent → dropdown →
"ar" narrows to three → ↓ ↩ adds a chip → × removes it; esc folds; a picked agent
leaves the list; the agents with access on a secret's / a vault's page are not
offered). iPhone: `AgentPickerTests` (candidates through `PeoplePicker.agents`, chip order, pick / ×,
and a source check that the four sheets draw `AgentPickerField` and `AgentChips`
is gone). Rig: `ARBOS_OPEN_SETTINGS=secrets-add` (the New secret form) or
`secrets-connect`, with `ARBOS_RIG_AGENT_PICKED=<name>,<name>` (these agents start
as chips) and `ARBOS_RIG_AGENT_PICKER=<query>` (the dropdown starts open with that
query typed; empty = open).

## Code cards: Grok's six colours, icon-only copy, indent guides (Jacob 2026-10-26, approved mock; [feedback code-highlighting-mac-2026-10-26], iPhone [feedback code-highlighting-ios-2026-10-26])

Fenced ``` blocks in a reply are Grok Bot's code card. One highlighter for both
apps: `arbos_core::highlight` (syntect + fancy-regex, the default Sublime pack —
one 360 KB dump loaded on first use, `warm()` off the main thread at launch;
Swift through `arbos-ffi`'s `highlight_code`), so a block is coloured the same on
the Mac and the iPhone. Languages: Python, JavaScript (+ TypeScript on the JS
grammar), Swift (table lexer), Rust, Go, shell, JSON, SQL, C, C++, YAML, HTML,
CSS, Markdown. Language = fence tag → alias map (`py`, `ts`, `sh`, `yml`, `c++` …)
→ shebang / `<!DOCTYPE` → keyword score (two clues and a one-point lead) → plain.
Blocks over 64 KB stay plain.

| Token | Dark (sampled from Grok's card) | Light (derived, ≥ 4.5:1 on the light card) | Mac `Theme` / iPhone `CodePalette` |
|---|---|---|---|
| plain — identifiers, operators, brackets, and `in` / `and` / `or` / `is` / `as` | `#D6D6DD` (the body colour) | `#1F1F23` | `code_plain` / `plain` |
| keyword, storage type, `true` / `false` / `None` / `nil` | `#95D0CD` | `#1F7F7A` | `code_keyword` / `keyword` |
| function & macro name (definition and call), type name, tag attribute, YAML key | `#E6B387` | `#B8641A` | `code_function` / `function` |
| string (quotes and escapes included) | `#D898D8` | `#A63FA4` | `code_string` / `string` |
| number | `#F1C972` | `#9A6F00` | `code_number` / `number` |
| comment — **italic** | `#626262` | `#8C8C8C` | `code_comment` / `comment` |
| indent guide | white 7 % | black 9 % | `code_guide` / `guide` |

Card rules (both platforms):

- **No header row** — no language label, no "Copy" pill. The tag only drives the grammar.
- **Copy** is a 24 pt square with a 6 pt radius and the card's hairline (`border` /
  `separator`), the `copy` glyph 13 pt in `text_muted`, inset 8 pt from the card's
  top-right corner, floating over the code (the body keeps a 40 pt right inset so no
  text hides under it). Hover `surface_2`, pressed the chips' accent tint, and after a
  click a **check in `online`** for 1.5 s (`#copy-check`; the Mac's `CodeCopy`, the
  iPhone's `copied` state). Copies the block verbatim.
- **Indent guides**: 1 pt lines, one per completed indent step on every non-blank
  line, at column *k·step + 0.65·step* (Grok: col 2.6 of a 4-space step); the step is
  the block's smallest indent (tabs = 4), at least 2. None on blank lines.
- **Long lines scroll sideways** inside the card (`sideways_scroll` /
  `ScrollView(.horizontal)`); no wrapping.
- Mono 12.5 pt / 18 pt lines on the Mac (`CODE_TEXT`, `CODE_LINE_HEIGHT`, glyph
  advance 7.5 → `CODE_GLYPH_ADVANCE`); 14 pt SF Mono on the iPhone (`CodeHighlight.fontSize`,
  advance measured). Radius 10 (Mac) / 12 (iPhone), `code_bg` fill, `border` hairline.

Pinned by `arbos_core::highlight::tests` (Jacob's bubble-sort card token by token,
landmarks in all 15 languages, aliases and fallbacks), the Mac's
`the_code_card_paints_grok_colours_and_indent_guides`,
`a_code_card_with_long_lines_scrolls_sideways` and the copy-button tests, and the
iPhone's `CodeHighlightTests`. Rig: `ARBOS_FIXTURE_REPLY=math` (Mac) /
`ARBOS_SHOW=math` (iPhone) is Jacob's bubble-sort turn; `ARBOS_RIG_CODE_COPY=hover|pressed|copied`
paints the copy square in that state for a capture (the check needs `copied`).
## Chart bubbles: a native chart card, the code card's frame (Jacob 2026-10-26, approved mock; [feedback chart-bubble-mac-2026-10-26], iPhone [feedback chart-bubble-ios-2026-10-26], agentd [feedback chart-post-tool-agentd-2026-10-26], core [feedback chart-core-2026-10-26])

**An agent posts a chart as data, not a picture, and the apps draw it natively.**
`post_chart(chart, caption)` (agentd) writes one row: `text` (the caption) →
`attachment` (a 1200 × 675 light PNG agentd renders on the box, `chartPart`
naming the chart part — what an app that cannot draw the chart shows, and what
push previews and Library use) → `chart` (`specVersion`, the validated
`arbos_core::chart::Chart`, its markdown `table`, `fallback` = the attachment
id). The data lives in the part (≤ 32 KB), never in a blob. Both apps skip
unknown part kinds, so an older build shows the picture; a build that knows the
part draws the chart and hides the attachment that names it.

One source of numbers: `arbos_core::chart` validates (8 series · 500 points per
series · 2 000 total · 100 categories · pie 12 slices, one series · 12
annotations), resolves scales (linear / log / time / category), nice ticks,
stacking, value formats (`%` glued, other units spaced, currency in front,
compact `1.2k`, durations) and the readout columns; `layout(w, h, style)` gives
the pixel geometry the Mac canvas and the picture paint; `hit` the readout under
the pointer; `table_markdown` / `table_tsv` the data. The iPhone draws Swift
Charts from the same model through `arbos-ffi`.

| Item | Mac | iPhone |
|---|---|---|
| Frame | the code card's: inside the bubble, `code_bg`, `border` hairline, radius **10**, padding 12; one block of the bubble (`Block::Chart`), so corners, the hover rail and selection treat it like a code card | radius 12, the same tokens |
| Header | title 13 semibold, subtitle 11.5 `text_muted`; the axis title sits above the plot (horizontal), never rotated | 15 / 13 |
| Icon squares | **24** pt, 6 pt radius, hairline, 13 pt glyph in `text_muted`, inset 8 from the card's top-right: **Copy data** (`copy` → `check` in `online` for 1.5 s, the code card's `#copy-check`), **Save PNG** (`download`), **Expand** (`expand`) | the same three |
| Series colours | the code-card token palette in order — keyword `#95D0CD`, function `#E6B387`, string `#D898D8`, number `#F1C972`, then `accent`, `online`; light: the derived light set. A `color` the agent gave is kept in both themes | the same (`ChartPalette`) |
| Plot | canvas 210 pt (bar 200, pie 220): gridlines = `border`, ticks 11 pt `text_faint`, lines 2 pt round-joined, areas 18 % fill, bars 62 % slot width with 3 pt top radii, pie slices seamed in the bubble colour with the share inside slices ≥ 6 % | Swift Charts marks from the core model's domains and ticks |
| Values | **none at rest**. Hover: a 1 pt crosshair at the nearest column, a dot per series ringed in the bubble colour, one readout chip (x label, then dot · name · value per series; `surface_2` / `surface_1`, hairline, radius 8) beside the crosshair at the plot's top, flipped left near the right edge. Bars: the hovered category stays full, the others dim to 45 % | touch-and-hold: the same readout centred above the thumb |
| Copy data | TSV with a header row (units in the header) | the same |
| Save PNG | the core picture renderer in the **current theme at 2x** (`chart_card::png_bytes`), to Downloads with the "Saved to …" toast and the Finder reveal every saved attachment gets; a failure is the shared error toast ("couldn't save the chart") | `ImageRenderer` at 3x, to Photos |
| Expand | an **in-window overlay** (`ui::chart_viewer`): scrim 58 % / 40 %, a panel ≤ 1140 × 706 (radius 16, `bg`, hairline, shadow) with the title · subtitle, **Copy data** · **Save PNG** pills and the close disc top-right, the plot at the panel's width with the same hover readout, and a Chart \| Data switch whose Data view is the table in the markdown table's style; Esc, the disc or the scrim closes; focus follows `viewer_focus` like the media viewer | a full-screen cover with ×, title, share; Chart \| Data; Save image · Copy data pills |
| Fallback | the PNG as an inline picture + the caption (older builds); push body = the caption | the same |

Code: `ui/chart_card.rs` (`card`, `plot`, `paint_plot`, `save_png`), `ui/chart_viewer.rs`,
`model::Part::Chart` + `Attachment::chart_part`, `markdown::Block::Chart`, the
transcript's row builder (a chart part is a `Markdown` row; an attachment whose
`chart_part` names a chart part of the same message is skipped). Pinned by
`transcript::chart_tests` (part parse, rows + the hidden picture, the headless card:
title / legend / plot height / the three 24 pt squares at the top-right, Copy data
→ TSV + check, Save / Expand wired) and `chart_card::tests` (palette per theme,
file names, the 2x themed PNG); the numbers by `arbos_core::chart::tests`.
