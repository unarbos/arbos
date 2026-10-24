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

## Own-bubble delivery (Jacob 2026-10-24, [feedback sent-state-opacity-2026-10-24])

**No "sent" / "seen" caption on my bubbles — the bubble's opacity is the
state.** Grok Bot's user bubble carries no delivery label (refs 03/web-01), and
Jacob asked for "coloring" only: "remove these 'sent' items, just use coloring
to make it clear, make the bubble less opaque."

| State | Meaning | Whole-bubble alpha | Extra |
|---|---|---|---|
| Pending | not yet ACKed by the relay | **0.50** (`BUBBLE_ALPHA_PENDING`) | — |
| Failed | the relay refused it | 1.00 | red ⓘ-style `exclamation` glyph (16 pt, `danger`) on the bubble's outer edge, always visible |
| Sent | ACKed, nobody opened past it yet | **0.72** (`BUBBLE_ALPHA_SENT`) | — |
| Seen | a peer's read watermark covers it, an agent answered after it, or read receipts are off | 1.00 | — |

Why 0.72 and not a lighter fill: whole-bubble alpha dims the text with the fill,
which is what makes the difference read in dark mode (fill L* 39 → 30, text
L* 100 → 76). Measured contrast of the composited text on its composited fill:
dark 4.9 : 1, light 7.7 : 1 (≥ 4.5 AA); pending 3.4 : 1 / 3.6 : 1 (transient).
The fill alone at 0.72 would move dark only 8 L* and read as "slightly
different grey". Same values on the iPhone (`Delivery.sentAlpha` /
`pendingAlpha`); its own bubble colours are the Mac's (`#5C5C5C` / `#111111`,
white text).

The "seen" caption from #1496 went with the "sent" one (same component); full
opacity carries it. With read receipts **off** every ACKed bubble is full at
once — there is nothing to wait for, and a transcript that is lighter forever
would read as broken.

Code: `transcript.rs` `Delivery` + `delivery_state()` + `own_bubble()` +
`failed_glyph()`, pinned by `delivery_tests`; iPhone `Transcript.swift`
`Delivery` / `ReadReceipts` / `DeliveryFade` + `DeliveryStateTests`. The iPhone
also sends the `seen` watermark in its presence header now (`SessionRoom
.newestMessageId`), so a Mac bubble read on a phone goes full.

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

## Right info pane chrome (Jacob 2026-10-24, [feedback info-pane-chrome-2026-10-24])

**The right pane is the window, not a panel: the chat's surface on both sides
of the join, no hairline between them, Grok Bot's two round discs (share ·
close) at its top right, a plain hero picture, inset cards under section
labels, and no "Delete agent" button.** Jacob's side-by-side of a fresh Grok
Bot and a fresh Arbos agent (2026-10-24, `media/feedback/grok-parity-test/` in
the project store; items 19–22, 26–27 of `docs/arbos-vs-grok-traits.md`).

Measured on `grok-right-pane-details.png` and `grok-chat-1.png` (2 px/pt,
window top at row 0; pane 639 px = 319.5 pt wide). Points.

| Item | Grok Bot | Arbos (`profiles/mod.rs`, `profiles/kit.rs`, `theme.rs`) |
|---|---|---|
| Pane fill | `#121212` — the chat's own colour; sidebar `#0E0E0E` | `Theme::pane()` = `chat_wash()` (nothing on matte / dark glass, the white wash on light glass); `theme.pane` is gone |
| Join | a half-point `#1F1F1F` line (+13) that reads as none | **no border** (`border_l_1` removed from the pane and the mid-slide blank) |
| Cards | `#1F1F1F` on `#121212` (+13), inset **16 / 16**, radius **12** | `pane_card` `#232323` on `#141414` (+15), `kit::INSET` 16, `kit::CARD_RADIUS` 12 (was `PANEL_RADIUS` 14); light: `#F2F2F2` on `#FCFCFC` (white on white was no card) |
| Rows | one-line **41**, two-line **51**, 13 pt title / 13 pt muted subtitle | `kit::ROW_MIN_H` 41 (was 48); a title + subtitle row lands on 51–52 by its own padding; row type unchanged (`TEXT_BODY` / `TEXT_SMALL`) |
| Section label | "Coding" / "Routines": text **28** from the pane edge, 13.5 pt `#A5A5A5`; glyphs 24 under the pill, 26 under the card before, 10 over the card after | `kit::section_title`: `INSET + SECTION_TITLE_INDENT` 28, `TEXT_SMALL` muted on a 16 pt line, padding 10 / 8 over `CARD_GAP` 14; a leading label pulls the fade spacer in by `pane::FIRST_TITLE_PULL` 7 |
| Share · close discs | **36** each, fill `#1F1F1F`, glyph `#E0E0E0`; **8** apart, **12** from the pane edge, centred on the strip discs' line (top 12 in the 60 band; `grok-chat-1.png` rows 28–99 for both sets) | `glass::control_disc` — the strip's disc (`CONTROL` 36, `surface_2`, `text`); `top_bar()` pads `PANE_DISC_INSET` 12, gaps `CONTROL_GAP` 8; close `xmark` at `PANE_CLOSE_GLYPH` 22 → 11 pt mark (Grok 11); share `share` at `PANE_SHARE_GLYPH` 21 → 14 pt (Grok 14). Before: a 30 pt `glass::circle` with a 6.5 pt × at top 17, inset 10 |
| Share disc action | — | the invite sheet (agent: let a person in on this agent; group: Add members). Pages with nothing to share (person, channel, server, own profile) draw the × alone |
| Top bar fill | none | none (was `card()`); the body still fades under the pinned hero + pill |
| Hero picture | plain bot mark, 58 pt, 73.5 under the window top | `HERO_TOP` 14 under the 60 band (was 4); the agent's leaf **without its pastel disc** (`avatars::project_hero_bare`: the hue's pastel as ink on dark, its ink on light) and **no presence dot** — presence stays on the sidebar row; `HERO_AVATAR` 72 kept (Jacob's mock b) |
| Delete agent | — | **gone from the pane** (footer and `Shell.agent_delete_armed` removed). Deleting an agent is the sidebar row's menu → Delete, at once (`sidebar::row_menu`, the Delete-is-immediate rule below) |

Pinned by `profiles::chrome_tests` (`the_pane_is_the_chats_surface_in_both_appearances`,
`the_constants_are_groks`, `the_share_and_close_discs_land_on_groks_pixels`,
`cards_are_inset_and_labelled_at_groks_margins`) and `pane::tests::every_kind_lays_out_hero_then_pill_then_options`.

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
