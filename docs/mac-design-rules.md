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

## Principle: never a coloured status dot (Jacob 2026-10-27, [feedback neon-dots-mac-2026-10-27]; iPhone [feedback neon-dots-ios-2026-10-27])

Jacob, on the amber glow after a chat tab's name: "replace all neon glimmer
icons like this in the app with braille spinners always, on a matter of
principle never use colored dots." App-wide, both platforms:

| A dot used to mean | Now |
|---|---|
| **activity** — an agent working, a tab streaming, a machine waking, a link reconnecting / syncing, a memo recording, the voice agent thinking | the **braille spinner** (`ui::working` / `WorkingSpinner`, § Working indicator), `text_muted`, one shared ticker |
| **unread** | the **count pill** (`marks::unread_pill` / `UnreadBadge`: accent capsule, white digits, "99+") beside the bold title; a lit flag with no count reads "1" |
| **a fault** — relay down, machine error | the red **`exclamation` glyph** (`marks::fault_glyph`), the error note's mark |
| **a resting state** — awake, asleep, offline, read | **nothing**; the words say it ("Awake · 5 h", "Offline") |
| **a person typing** | nothing in the status cell; the preview names them in a group ("Mira and Sam are typing", "3 people are typing") and reads the bare "typing…" in a DM, where the row's own title is already that person; the status row over the composer names them too. The spinner is never a human's sign. The group forms carry no ellipsis and the DM line keeps its own: Scout asked for the group wording and the 1:1 line was never reported, so it was left as it shipped (coordinator ruling, 2026-10-30) |

Exemptions carry `// status-dot: exception <why>` on the `.bg(` line (Mac)
or the `Circle().fill(` line (iPhone): a chart legend's series swatch, a
pager's page dots, a 30 pt placeholder disc — colour as data or as a surface,
not as a state. Lints: `marks::lint_tests::no_coloured_status_dot_is_drawn_in_this_crate`
(Mac: a `.size(px(≤ 12))` + `.rounded_full()` div filled with `online` / `warn` /
`danger` / `accent` / a machine colour) and `StatusDotLintTests` (iPhone: a
`Circle().fill(theme.<status colour> | machineDotColor | dot)`).

Not in this sweep, for Jacob's call: the green **online presence dot on people's
avatars** (another owner's area, bc-3bafe44c; the one remaining coloured dot,
Telegram / Grok Bot draw it too) and the incoming-call card's pulsing green
border (`callview`, a ring, not a dot).

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
| Folded sidebar | collapses to the **icon rail** (section below), never away; the toggle is the rail's first disc under the traffic lights (window and full screen alike, [feedback sidebar-toggle-in-rail-2026-10-27]); the chat header shows the round disc beside the lights (`header::sidebar_toggle`) only in the narrow band, where the sidebar hides outright |

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

## Sidebar pinning (Jacob 2026-10-28, [feedback sidebar-pinning-mac-2026-10-28]; relay [feedback sidebar-pinning-relay-2026-10-28]; iPhone [feedback sidebar-pinning-ios-2026-10-28])

Jacob (Arbonites, 16:44 UTC): "the ability to pin chats/servers/agents on the side bar
with the control click." Mockup approved 17:10 UTC (project store
`media/feedback/sidebar-pinning/sheet-sidebar-pinning-mock.png`, design
`docs/sidebar-pinning-design.md`) with two decisions: **cap 5, no tiers**; iPhone gets the
same header, pins servers too and migrates its device-local pins into the synced list.

| Item | Rule | Code |
|---|---|---|
| Pin / Unpin | the **first item** of every row menu (agent, person, group, server), above today's items, which do not move; SF `pin` / `pin.slash`, the iPhone's names. A new pin lands at the **top**; Unpin returns the row to its place by activity. At the cap the click answers with the shared error element ("You can pin up to 5 chats.") | `sidebar::row_menu` (`row-menu-pin`), `AppState::set_pinned` |
| Pinned section | "Pinned" `TEXT_SMALL` 13 medium `text_muted` at the rows' text inset (list 8 + row 12 = **20**) in a **26** pt band (`PINNED_HEADER_H`) under the filter row; the pinned rows **in the user's order only** — never re-sorted by unread or activity, unread stays the count pill / bold name; then a 1 pt `border` hairline inset 12 in a 9 pt band (`PINNED_DIVIDER_H` = `glass::MENU_DIVIDER_HEIGHT`); then the rest in today's order, no second header | `sidebar::list_sections`, `pinned_header`, `pinned_divider` |
| Pinned row | a normal row plus **`pin.fill` 11 pt semibold** in `text_muted` before the time (`row-pin-<id>`). No colour, no dot | `pin_mark` |
| Zero pins | **nothing is drawn** — no header, hairline or marks; the list is exactly as before | pinned by `pin_tests::zero_pins_draws_no_section` |
| Filter | All · Agents · People · Servers: the section shows only the pins the segment admits; header and hairline hide when none passes | `list_sections` over the filtered rows; `pin_tests::the_filter_shows_only_the_pins_it_admits` |
| Reorder | **drag** a pinned row within the section: the row lifts (the highlight token's pressed step, `highlight::lifted`), the others slide to open the slot under the pointer, the drop — or a mouse-up anywhere — saves the shown order; pins the filter hid keep their places (`AppState::reorder_pins`). No drag across the hairline; pin / unpin is the menu | `PinDrag` / `pin_draggable` / `pin_drop_target`, `Shell::{begin_pin_drag, pin_slot_at, finish_pin_drag}` |
| Rail | the collapsed rail draws the same order (pins first); its metrics are unchanged — no hairline tile | `rail` reads `sidebar::list_entries` |
| Archive / delete | unpin (a pin is a row you see); pinning an archived row unarchives it first | `set_space_archived`, `delete_project`, `delete_people_chat`, `leave_group` |
| Data | one relay list per user, top first: row keys `agent:<project id>` / `chat:<people chat id>` / `server:<server id>` (`AppState::pins`, `pin_key`); `GET /me/pins` at launch after the cached copy (`store::KEY_PINS`), `PUT /me/pins {pins}` replaces the whole list (`ClientCommand::SetPins`; the relay's answer is the truth, a refusal re-reads), `pins_changed` from another device replaces it (`SyncEvent::Pins`). Cap from the relay (`pin_cap`, default 5) | `sync/relay.rs` `refresh_pins` / `apply_pins`, `fixture.rs` `ARBOS_RIG_PINS=<name>[,…]` |

Pinned by `sidebar::pin_tests::{zero_pins_draws_no_section,
pinned_rows_lead_in_the_users_order_under_a_header, the_filter_shows_only_the_pins_it_admits,
the_row_menu_leads_with_pin_then_unpin, pins_land_on_top_stop_at_the_cap_and_go_to_the_relay,
reordering_keeps_hidden_pins_in_place, a_pin_drag_previews_the_order_and_commits_on_drop}`;
relay `pins::tests`.

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
| Lights' slot | the top **44** pt of the rail (`LIGHTS_SLOT` = `theme::TRAFFIC_LIGHT_SLOT` = button 14 + 2 × `TRAFFIC_LIGHT_POSITION` 15): the title-bar container GPUI sizes around AppKit's three buttons (x 15–75, y 15–29, centred at 22 = Grok's line, hover tracking areas inside it; re-laid out by GPUI on resize, full-screen exit, activation, screen change — `gpui_macos::window::move_traffic_light`). Reserved: none of our controls inside it; all window drag. As the expanded strip, the rail moves nothing in native full screen: macOS hides the buttons, the slot stays. Pinned by `rail::tests::no_control_shares_the_traffic_lights_frame_in_any_state` (window, full screen, morph path, key / non-key) |
| Toggle | **inside the rail**, the first disc under the lights' slot: x **24**, top **52** (`TOGGLE_TOP` = `LIGHTS_SLOT` 44 + `STRIP_GAP` 8), the same 36 pt `control_disc` and column as + and Search, in a window and in full screen alike (Jacob 2026-10-27, [feedback sidebar-toggle-in-rail-2026-10-27]: "The tab button should also be in the side bar here" — supersedes the 2026-10-26 rule that a plain window kept the toggle in the chat header at x 96 and that full screen put it on the lights' line at top 12, where a window state read as full screen left it half hidden under the buttons; `rail::toggle_spot` is gone). The chat header draws a toggle only for the narrow band's hidden sidebar (`header_toggle_shown`) |
| + and Search | stacked under the toggle, `STRIP_GAP` 8 apart: + top **96**, Search top **140** (36 pt `control_disc`s, the strip's own handlers) — the strip's search · + · collapse read top-down from the toggle |
| Pictures | every chat the current filter admits — agents, people, groups, servers (rounded squares, `SERVER_PICTURE_CORNER`) — in the list's order, **36** pt on a **58** pitch, first tile top **192** (16 under Search; picture top 201, centre 219; was 152/161/179 before the toggle joined the column); the same `ListEntry::picture` builders as the rows (one avatar resolver, #1912). Selected = **54** tile, radius **12**, the highlight token's selected step; hover = its hover step + a name tip 8 right of the rail (`pane_card`, `border_strong`, 13 pt) |
| Badges | one spot at the picture's bottom-right corner (never a coloured dot, 2026-10-27): **working** = the braille spinner in the 13 pt ring of the rail's colour, else **unread** = the count pill (`marks::unread_pill`, a 2 pt rim in the rail's colour); the selected chat shows no unread pill; nothing for a person typing |
| My picture | alone at the bottom: 36 at x 24, **12** above the bottom (centre 30 = `CONTROL_LINE` mirrored; Grok 31), with the machine dot; opens Settings like the footer row |
| Scrolling | the pictures scroll (`LIST_FADE_BAND` edge fade) when more fit than the window holds (`capacity`: 14 in a 1079 pt window, 8 in a 760 one) |
| Filter | the rail follows All · Agents · People · Servers; the segment persists in `<config>/sidebar_filter` and the collapsed state in `<config>/sidebar_collapsed` (`SidebarCollapsed`), so the app reopens as it was left |
| Servers | a server's picture opens its **newest channel** (`#general` when none has spoken) and marks the server open, so expanding shows its channel list |
| Expanded sidebar | **unchanged** — strip, filter, rows, footer exactly as above |
| Composer / column | the pill's cap is Grok's **960** (`COMPOSER_WIDTH` 904 + 2 × 28; was 876), the transcript column shares it (`COLUMN_MAX_WIDTH`); both centred in the chat area right of the rail / sidebar, as before |
| Collapse / open | a **morph**, not a clip, on the pane-slide curves (170 ms ease-out quad closing, 200 ms ease-out cubic opening): the three discs glide from the strip into the rail column (the toggle with them, into its slot under the lights), the rows' pictures shrink into their rail slots as the text and filter fade out by half-way, the selected card shrinks into its tile, my footer picture slides to the centre, the chat column re-centres. The travelling pictures keep the lists' `LIST_FADE_BAND` top and bottom fade the whole way, in a list frame that moves from the expanded list's to the rail's with them (`rail::morph_list_top`), so the rail's edge fade comes in with the fold and goes with the unfold — never added the frame the fold ends or dropped the frame an unfold begins ([feedback mac-sidebar-slide-jank-2026-10-29], Scout: "the second it is folded in it just overlays the blur"; `rail::tests::the_morph_fades_its_pictures_in_a_frame_that_moves_with_them`). `rail::render_morph` (the column) + `rail::render_overlay` (the travelling pieces, from the resting bounds the expanded sidebar records each frame — `Shell::morph`). Reduce Motion snaps |
| Narrow band | below `NARROW_WINDOW` the sidebar still hides and the header toggle floats it as an overlay (Q7 b); the rail is for the docked widths |
| iPhone | no equivalent (no sidebar; the home list + the single bottom search bar) |

Rig: `ARBOS_SIDEBAR_COLLAPSED=1|0` (start collapsed), `ARBOS_SIDEBAR_FILTER=…`,
`ARBOS_RIG_FULLSCREEN=1` (lay out as full screen where the platform cannot say;
since [feedback sidebar-toggle-in-rail-2026-10-27] full screen changes no
layout), `ARBOS_RIG_RAIL_T=0.4` (freeze the morph at a progress for still
frames).

Pinned by `rail::tests` (metrics vs the measurement table, the rail's layout
on the mock's pixels, the toggle's rail slot under the lights, the lights'
frame clear of every control in every state, header-toggle rule, badge
priority, capacity, morph maths), `slide::tests::collapsing_slides_the_sidebar_down_to_the_rail_and_back`
(+ reduce-motion / width-drag), `theme::sidebar_state_tests` (collapsed state
restore) and `sidebar::strip_tests::the_filter_is_restored_from_the_config_dir`.

## Narrow sidebar: the slider's snap point (Jacob 2026-10-27, [feedback sidebar-narrow-wrap-2026-10-27])

**Dragging the chat list narrower never draws a broken middle state.** Jacob
(v0.6.296): "fix wrap for when closing the side bar with the slider" — at 200 pt
the filter capsules overflowed (one clipped on the left, "Servers" off the
right), rows read "Arbo… 10:35 AM" with the dot crowding the name, and the
strip's discs no longer fit beside the lights. Rules (`ui/sidebar_fit.rs`):

| Width (pt) | Rule |
|---|---|
| ≥ 288 (`HANDLE_MIN`) | the full row: name, `@handle` (DMs), time at `TEXT_TINY` 11.5 |
| 238–287 (`TIME_FULL_MIN`) | the `@handle` goes first; time still 11.5 |
| 226–237 (`TIME_SMALL_MIN`) | the time shrinks to **10** pt (`TIME_SMALL_SIZE`) |
| 220–225 | the time is hidden; the name has the line |
| **220** (`SNAP_WIDTH` = `SIDEBAR_WIDTH_MIN`) | the narrowest layout: the strip's search · + · collapse exactly fill the band right of the lights (72 + 12 + 3 × 36 + 2 × 8 + 12 = 220, `strip_fits`); a row keeps a `NAME_MIN` 96 pt name (`text_column` = width − 86 chrome). The handle holds here while the pointer is within 16 pt below |
| < 204 (`SNAP_WIDTH − SNAP_HYSTERESIS`) | **snaps to the icon rail** (the rail morph, as the toggle) and keeps the width the gesture started from as the column's saved width (`EdgeTarget::Rail { keep }`) |
| rail → out past 220 | **restores that width** (`NSSplitView` collapse behaviour): the column re-expands at it, the handle holds until the pointer passes it, then follows (`EdgeTarget::Hold` → `Expanded`). The rail's right edge is the same handle (`rail-resize`) |
| rail → out, short of 220 | **the rail morph follows the pointer** (`EdgeDrag::track` → `slide::Want::sidebar_track`), the mirror of the fold following it down to 220. Crossing 220 settles at the restored width on the open curve from where the hand is; letting go short of it settles back into the rail on the close curve; a width change under a running slide retargets it, never drops it. Drag open and drag close are symmetric with each other and with the toggle (Jacob 2026-10-29, [feedback mac-sidebar-drag-open-2026-10-29]) |
| Filter capsules | never clipped: the row scrolls horizontally under a 12 pt side fade (`FILTER_FADE_BAND`, `Shell::filter_scroll`) when the four do not fit — Grok Bot's iPhone home filter row — and is centred when they do (first / last capsule carry `auto` side margins); the selected capsule is scrolled into view. No icons, no overflow menu |
| Strip | unchanged at every width ≥ 220 |

Breakpoints derive from `ROW_CHROME` 86 (8 + 8 list, 12 + 12 row, 36 picture,
10 gap) + `NAME_MIN` 96 + the time's footprint (56 full / 44 small) + the
handle's 50. Pinned by `sidebar_fit::tests::{the_width_breakpoints_keep_every_part_readable,
the_rows_time_shrinks_then_hides_before_the_name_gives,
dragging_past_the_snap_point_collapses_to_the_rail_and_back_restores_the_width,
a_drag_from_the_rail_tracks_the_pointer_until_the_snap_width}` and
`slide::tests::dragging_the_rail_open_follows_the_pointer_then_settles_like_the_toggle`.
Rig: `<config>/sidebar_width` (the saved width) + `ARBOS_SIDEBAR_COLLAPSED=0`.

## Split handles: a wide grab, a hairline look (Jacob 2026-10-28, [feedback sidebar-slide-handle-minimal-2026-10-28])

**Every split / resize handle on the Mac is macOS's split-view divider: what
the pointer grabs is wide and invisible, what the eye sees is at most a 1 pt
low-contrast hairline that brightens a little on hover and a little more while
dragging. Never a thick bar, never a coloured fill.** Jacob (2026-10-28 15:15 UTC, v0.6.331): "the
slide highlight on the side bar is wrong. Should make this much more minimal"
— the sidebar's handle lit its whole 10 pt band in `primary` at 45 % on hover,
a fat light-grey bar down the sidebar's edge. #2082's tester note ("fidgety")
was about the grab, not the look; the grab stays generous, the look goes.
Rules (`ui/split_handle.rs`):

| Part | Rule |
|---|---|
| Hit band | `HIT_WIDTH` **10** pt inside the pane's right edge, full height, `col-resize` cursor, **never drawn** (8–10 pt is the range: easy to start, does not eat the list) |
| Idle | nothing of the handle's own; the pane's separator is the only line — the sidebar's and the rail's `border_r_1` in `theme.border` (white 8 % / black 10 %), the right info pane's `pane_edge` hairline |
| Hover | a `LINE_WIDTH` **1** pt hairline flush with the edge, over the separator, text colour at `HOVER_ALPHA` **0.18** — a step above the border, still low contrast |
| Dragging | the same hairline at `DRAG_ALPHA` **0.30** — one step more, held for the whole gesture (GPUI suppresses hover styles during a drag; `split_handle::DragState` carries the state, raised with the drag's ghost view and lowered when GPUI drops it at mouse up) |
| Where | one builder, `split_handle::right_edge(id, theme, dragging)`; the sidebar's `sidebar-resize` and the rail's `rail-resize` are that element with the drag wiring (`sidebar::resize_handle`). The right info pane has a fixed width (`Shell::pane_width`) and so no handle — its hairline rule is the one above. Any new divider uses the same builder |
| Snap / width rules | unchanged: `SIDEBAR_WIDTH_MIN` 220, hysteresis 16, rail snap and restore (`ui/sidebar_fit.rs`, § above) |

Pinned by `split_handle::tests::{the_hit_band_is_generous_and_the_line_is_a_hairline,
the_line_is_dark_at_rest_and_brightens_in_two_steps,
the_drag_state_lowers_itself_when_the_ghost_drops,
the_rendered_hit_band_is_wide_and_the_drawn_line_is_one_point}` (a headless
render measures the hit band at 10 × full height and the drawable line at
1 × full height, both flush with the pane's edge). The `// hover-fill:
exception handle` marker in `highlight`'s lint is this hairline's.

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
| Hover / pressed | the highlight token's two steps washed over the blue (`highlight::washed`; § Rows: hover / selected / pressed) — brighter in dark, darker in light; busy = inert |
| No label, no countdown text | the notice line carries the words ("… press the blue update button at the bottom of the chat list …") |
| Rig | `ARBOS_UPDATE_STATE=available\|downloading\|installed\|failed` starts the fixture in that phase for a fake 0.6.999 build (the worker stays off) |

Pinned by `update::disc_tests` (hidden until ready; click → install /
relaunch; the click action is never gated by the automatic toggle) and
`sidebar::footer_tests::the_update_disc_is_groks_size_colour_and_placement`
+ `the_update_disc_lands_on_groks_pixels` (36 at the strip's inset, on the
footer's line, 8 after the card, fills `#326CCB` / `#F5F5F5`).

## Working indicator (Jacob 2026-10-27, [feedback working-braille-spinner-mac-2026-10-27]; iPhone [feedback working-braille-spinner-ios-2026-10-27])

**Wherever a list row says "this agent is working", the indicator is the
braille spinner ⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ in the row's muted text colour — never a green
(or amber) dot.** The chat's own working line (the status row over the
composer) is unchanged; this rule is for the small status glyph on rows.

| Item | Rule (`ui/working.rs`) |
|---|---|
| Frames | the classic ten, in order: `FRAMES` = ⠋ ⠙ ⠹ ⠸ ⠼ ⠴ ⠦ ⠧ ⠇ ⠏; `FRAME_MS` **90** (80–100 ms band, ≈ 11 fps) |
| Cell | a fixed box **8 × 12** pt (`CELL_W` = the unread dot's 8, `CELL_H` 12), the glyph centred in it in the monospace face (`theme.font_mono`, `GLYPH_SIZE` = `TEXT_TINY` 11.5, line height 12). The ten frames differ only in which dots are set, so the cell never changes size: the row never re-flows while it turns |
| Colour | `theme.text_muted` — the time label's colour above it. Not `online` green, not `warn` amber, not the accent |
| Where | **sidebar chat rows**: the status cell at the right end of the preview line, under the time (`sidebar::RowStatus`: Working ▸ Unread ▸ Typing ▸ Quiet — a person typing shows **nothing** in the cell, so it hides neither the spinner nor the pill (the spinner is the agent's sign, never a human's — coordinator veto 2026-10-27; the green pulse went with the no-coloured-dots principle the same day, the preview carries the words instead), the spinner while the chat's agent streams, else the unread **count pill**, else nothing). Where a count pill shows it wins the slot — information over animation. **Header tab segment**: the spinner after the tab name while that tab streams (was a 6 pt amber dot). **Settings › Machine › Per-agent usage**: the spinner before "Working · …" on a running workspace. The collapsed rail's picture badges and the avatar presence dot are *not* this rule (avatar badge code, another owner) |
| Clock | one `WorkingClock` global: frame = elapsed ÷ 90 ms mod 10, read by every spinner at render → all spinners turn in step; no row owns a timer |
| Ticker | one task for the whole app. Drawing a spinner marks the clock `wanted`; the ticker sleeps 90 ms, and if nothing was drawn since the last tick it parks (no frames requested at all) — it starts again on the next spinner drawn. It also parks while the window is in the background (`Shell`'s activation observer → `WorkingClock::set_window_active`) and resumes with the window. A tick landing on a frame the window draws anyway (a scroll) is coalesced by GPUI — the ticker never adds frames to a scroll. The sidebar row's old `PulseClock::request` for a working row (30 fps whole-window redraws that nothing read since the tree animation retired) is gone: a working row costs the 11 fps ticker and nothing else |
| Reduce Motion | `cx.reduce_motion()` → the still glyph **⠿** in the same cell, and no frame requested (the ticker parks) |
| Trace | `ARBOS_WORKING_TRACE=1` prints once a second: `working spinner: ticks=N starts=M spinners=K draws/tick=D frame=F` — `starts` is the number of ticker tasks ever spawned (1 per idle→busy transition, never one per row), `spinners` how many distinct cells were on screen in the last tick, `draws/tick` how many window frames drew them (1 = only the ticker's) |

Pinned by `working::tests::{the_frames_are_the_classic_cycle_at_eighty_to_a_hundred_ms,
reduce_motion_draws_the_still_glyph_and_asks_for_nothing,
one_ticker_for_every_visible_row_and_it_parks_when_none_is_left,
the_cell_keeps_its_box_across_frames}` and
`sidebar::status_cell_tests::{the_cell_shows_one_thing_in_this_order,
the_spinner_sits_under_the_time_where_the_dot_was (real Shell: right edge =
the time's right edge, centred on the preview line, 8 × 12; unread row keeps
its dot; quiet row nothing; typing DM keeps its pulse), rows_share_one_ticker_and_the_cell_never_moves}`.
iPhone twin: `WorkingSpinner` in `UI/WorkingSpinner.swift` (same frames, 90 ms,
one `WorkingSpinnerClock`, Reduce Motion ⠿), pinned by `WorkingSpinnerTests`.

## Command palette ([feedback search-palette-2026-10-24], Grok Bot's)

Search never opens inline. The Search disc, **⌘⇧F** and **⌘K** open one
centred modal over the window (`ui::palette` on the shared `ui::modal`
backdrop): the chat behind gets no wheel and no key while it is up; Esc,
⌘⇧F / ⌘K again, a click outside (which also lands on what is under it —
"Search in chat" § Click outside), or opening a row closes it. Send Feedback moved from
⌘⇧F to **⌘⌥F** (Help menu unchanged).

| Item | Value (measured on `ref-grok-command-palette.png`, 2 px/pt) |
|---|---|
| Panel | 558 wide (`palette::WIDTH`), radius 12, hangs at 12 % of the window height (≥ 48), at most 72 % tall |
| Material | **one plate** ([feedback search-popup-single-layer-2026-10-28], Jacob 2026-10-28 "double transparency on search pop up is annoying"; Scout's report of the same): `glass::popup_plate` — on **Glass** the window frosted under the panel at `glass::POPUP_BLUR` 24 (the sheet's), the card tint at the popup floor `theme.popup()` (`POPUP_WASH_FLOOR` 0.80, adaptive above it), the glass family's 1 pt rim + top highlight, the sheet's lift shadow; on **Matte** the flat `surface_1` card with a 1 pt `border_strong` hairline. **Nothing else between the chat and the rows**: the backdrop does not dim (`modal::PASSTHROUGH_BACKDROP_ALPHA` 0 — Spotlight does not dim the desktop), the rows have no fill of their own, the field row and the list sit straight on the plate. Readability: a 15 pt line of transcript under the panel must not be legible — the blur erases it, the tint flattens what is left (`PopupMaterial::backdrop_bleed` ≤ 0.2) |
| Field row | 49 tall, magnifier 17 (12.5 visible) inset 16.5, "Search" at 17 pt; 1 pt hairline under it — the panel's one inner line |
| List | inset 10, 8 over the first row; rows 49 tall, 2 apart, radius 12; rows transparent, the highlighted / hovered row is the `ui::highlight` tint on top (`Shape::Own`) |
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
| Keys | **⌘F toggles** (Jacob 2026-10-27, [feedback search-toggle-click-outside-mac-2026-10-27]): closed → opens and focuses the field; open → closes exactly like ✕ / Esc (marks go, the composer types again), also with the field focused — the chord is a root action, the text input never swallows it. Enter / › / ⌘G next (newer); ⇧Enter / ‹ / ⇧⌘G previous (older); both wrap; ⇥ pins the offered sender; Esc / ✕ close. Inside the field the "ChatSearch" context overrides the composer's Send / Newline / AcceptMention. **⌘⇧F / ⌘K toggle the palette** the same way |
| Click outside | a mouse-down anywhere outside the bar closes it, and anywhere outside the palette's panel dismisses the palette — **without swallowing the click**: the message, sidebar row or button under it answers in the same click (`Shell::click_outside`, capture phase against the bar's / panel's last-painted bounds; the palette's backdrop is see-through for the mouse — `modal::modal_overlay_top_passthrough` — does not dim (§ Command palette, Material) and still swallows the wheel). A click inside the bar / panel keeps it. The Forward picker and the media viewer stay true modals (#1632) |
| Beyond the loaded rows | ‹ on the oldest loaded hit of a **server channel** asks the relay for a page (`ChannelLoadOlder`), shows "searching older…", moves on to the page's older hit when it lands, keeps paging while rows come without a hit, and wraps once the room says there is nothing above (`SyncEvent::ChannelHistoryEnd`). A chat doc (agent, DM, group) is whole on this Mac, so ‹ wraps at once |
| Not | no second field in the header, no dropdown of senders, no match counter in the pill, no sidebar field |

Pinned by `chat_search::tests` (hits, AND, offer, wrap, paging step, count,
byte ranges), `ui::chat_search::tests` (⌘F opens the bar at 56 / 36 / the
composer's width and focuses the field, Esc closes; ⌘F / ⌘⇧F toggle closed
again from inside the field; a click outside closes bar / palette and the
"+" disc under it still opens; typing counts, steps, wraps, pins; a channel
pages older rows then wraps) and `keymap_tests` (⌘F / ⌘G / ⇧⌘G; Enter /
⇧Enter / ⇥ inside the bar).

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

## Composer — caret shortcuts (Jacob 2026-10-27, [feedback composer-caret-shortcuts-2026-10-27])

The chat composer answers the standard macOS caret chords exactly as an
`NSTextView` does. "Line" for ⌘← / ⌘→ / ⌘⌫ is the **visual (wrapped) row**,
never the paragraph; "document" is the whole draft, every paragraph. Code:
`ui::composer` (`Home` / `End` / `DocumentStart` / `DocumentEnd` and their
`Select…` twins, `DeleteToLineStart`, `visual_line_range`, `caret_position`),
bound in `main.rs` `bind_keys` in the `"ComposerInput"` context only.

| Keys | Does | NSTextView selector |
|---|---|---|
| ⌘↑ / ⌘↓ | caret to the start / end of the whole draft | `moveToBeginningOfDocument:` / `moveToEndOfDocument:` |
| ⌘← / ⌘→ (also Home / End) | caret to the start / end of the caret's **visual** row; the end is after the space the row wraps on and paints at the row's end | `moveToLeftEndOfLine:` / `moveToRightEndOfLine:` |
| ⇧⌘↑ / ⇧⌘↓ / ⇧⌘← / ⇧⌘→ (also ⇧Home / ⇧End) | the same moves extend the selection from the anchor; the caret is the moving end, a selection flips around the anchor | `…AndModifySelection:` |
| ⌘⌫ | deletes the selection, else the row back to its start; at a row's start one step back like ⌫ | `deleteToBeginningOfLine:` |
| ⌥← / ⌥→ (⇧ selects), ⌥⌫ / ⌥⌦ | word moves / word deletes — unchanged | `moveWordLeft:` … |
| ↑ / ↓ (⇧ selects) | one visual row, column kept; past the ends → draft start / end — unchanged | `moveUp:` / `moveDown:` |

Rules: (1) **routing** — all of these live in the `"ComposerInput"` key
context, so they reach the focused composer first and fall through to any
app-level binding only when no text field has focus (today nothing else binds
⌘↑ / ⌘↓ / ⌘← / ⌘→; the chat list and the transcript must never take them
ahead of a focused field). (2) **Affinity** — a caret that ⌘← / ⇧⌘← put on a
soft-wrap boundary paints at the start of *its* row (downstream), every other
way of reaching that byte paints at the end of the row above (upstream, the
AppKit default); a selection that starts on a boundary is drawn from the row
below's start, so no empty sliver is highlighted at the end of the row above.
(3) Before the first paint (no layout yet) the row is the paragraph.

Pinned by `ui::composer::caret_tests` (every move and extend on a wrapped
two-paragraph draft in a 360 pt field, ⌘⌫, ⌥ arrows, focused vs blurred
routing — all through the real keymap) and `keymap_tests::caret_chords_route_to_the_focused_composer_only`.

### Composer — floating layers (Scout 2026-10-29, [feedback mention-list-overlay-2026-10-29])

The `@` mention list, the + menu and the emoji / sticker / GIF picker float
over the transcript, `MENTION_MENU_GAP` (8) above the field, left-aligned to
it. Opening, filtering or closing one moves nothing: the pill, the composer
block and every transcript row keep their frames. Rule: **a floating layer is
never an in-flow child of the composer column** — it is absolute on the
field's own box (`ui::composer::above_field`). The column stacks its children
with an 8 pt gap, and the old in-flow `glass::popover` wrapper (zero height,
its card absolute inside it) still took one gap, so every open layer grew the
composer block by 8 pt and lifted the pill and the chat with it. Pinned by
`ui::composer::overlay_tests` (the real `Shell`, a group with people and an
agent: frames before / with each layer open / after).

### Composer — keyboard layouts (Scout 2026-10-28, [feedback mention-hotkey-layout-2026-10-28])

There is no `@` hotkey: the mention picker opens when an `@` is **typed** at a
word start, and every layout types it differently (US ⇧2, German ⌥L **and**
⌃⌥L, Spanish ⌥G, Swiss ⌥G, French/Italian/Norwegian a plain key, Finnish /
Portuguese ⌥2 — read off the real layouts with `UCKeyTranslate`). GPUI's macOS
layer drops the character of any chord with Control held and AppKit never
hands such a chord to `insertText:`, so German ⌃⌥L reached the composer as a
bare `ctrl-alt-l` and typed nothing (on German the whole ⌃⌥ column repeats
the ⌥ column — Q `«`, F `ƒ`, Y `¥` — AltGr by another name). Rule: **a
Control+Option chord no binding took types what the layout says it types**,
when that is printable — the composer's `on_key_down` asks
`ui::key_layout::typed_text` (Carbon TIS + `UCKeyTranslate`, the table
`NSTextView` uses) and inserts the result. Nothing is special-cased to `@` or
to German; US ⌃⌥L is still a control character and types nothing, as in
TextEdit; plain Control chords are left alone (US ⌃2 reports "2" but is not
typed). Bindings run first, so ⌃⌥F stays Send Feedback on German rather than
typing `ƒ`. Chords that arrive with their character (⌥L = `alt-l` + `@`) are
the platform's to type and are not typed twice.

Shortcut audit: GPUI reports every letter chord under ⌘ by the character the
layout gives that key with ⌘ held (non-ASCII layouts fall back to the ⌘ table),
so ⌘B / ⌘I / ⌘⇧X / ⌘F / ⌘⇧F / ⌘K / ⌘G and the caret chords (named keys) work
on QWERTZ, AZERTY and friends. Only `cmd-{` / `cmd-}` (tabs) are unreachable
where braces need ⌥; **⌃⇥ / ⌃⇧⇥** are bound as the layout-independent tab
chords. Pinned by `ui::key_layout::tests` (layout tables read off the real
German / US / French / Spanish layouts), `ui::composer::layout_chord_tests`
(the chords dispatched exactly as the platform builds them, through the real
keymap, into a focused composer) and
`keymap_tests::non_us_at_sign_chords_are_nobodys_and_tabs_have_layout_free_chords`.

### Composer — formatting chords and live styling (Jacob 2026-10-27, [feedback chat-markdown-composer-2026-10-27])

The draft shows the chat Markdown of § Message formatting **as it is typed**,
and three chords write the markers for you. Code: `ui::composer` (`Bold` /
`Italic` / `Strike` → `toggle_marker`, `live_styles`, the quote bars in
`InputElement::prepaint`), `main.rs` `bind_keys`.

| Keys | Does |
|---|---|
| ⌘B | wraps the selection in `**…**`; on an already wrapped selection (markers just outside it, or selected with it) removes them; with no selection inserts `****` with the caret in the middle, and removes an empty pair on the next press |
| ⌘I | the same with `*…*` |
| ⌘⇧X | the same with `~~…~~` |

Rules: (1) **Routing (Jacob's default, vetoable)** — the three chords live in
the `"ComposerInput"` context: with the composer focused ⌘B is bold; with it
blurred ⌘B keeps toggling the sidebar (`shell::ToggleRail`, bound at the
root), and ⌘I / ⌘⇧X are nobody's. (2) The markers hug the words: spaces at the
ends of the selection stay outside, so the grammar's close-against-a-non-space
rule always holds. (3) After a wrap the words stay selected (inside the
markers), so ⌘I after ⌘B nests (`***both***`) and a second ⌘B undoes the wrap;
every toggle is one ⌘Z step. (4) **Live styling** never changes the layout: the
typed bytes stay where they are, only their paint changes — marker bytes
(`**`, `*`, `~~`, `||`, `` ` ``, `[`, `](…)`) in the faint ink, the words
between them bold / italic / struck / mono on the code wash / on the open
spoiler wash; a `> ` line's markers faint, its words muted, and a 3 pt bar
(`transcript::QUOTE_BAR`, the muted ink at 55 %) in the field's left padding
5 pt from the text, the height of that paragraph's rows (`>>` nests the same
way: one bar, deeper text). Plain text (`snake_case`, `2*3*4`, `>5`) is one
plain run — the same never-mangled rule as the bubble.

Pinned by `ui::composer::format_tests` (wrap / unwrap / hug / strip / empty pair
/ nesting / ⌘Z / blurred, all through the real keymap; `live_styles` run by
run) and `keymap_tests::format_chords_are_the_focused_composers_and_cmd_b_toggles_the_rail_outside`.

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

### Connection capsule under the pill — shown for every chat kind, whole (Jacob 2026-10-22 [feedback connection-pill]; 2026-10-27 [feedback offline-chip-group-header-2026-10-27])

The small dark capsule under the pill — a 6 pt dot and two words, "Offline" ·
"Relay down" · "Reconnecting…" · "Syncing…" · "Reconnecting shortly…" — is the
**connection capsule** (#1339): it reports *this Mac's link to the relay for
the open chat* (`AppState::link_pill` = the chat's own room socket and the
relay's registry poll, highest verdict wins, shown once a state has held
1.2 s). It is not a member's presence, not typing, not the agent status row.

| Chat kind | Capsule | Why |
| --- | --- | --- |
| Agent chat / job tab | shown | the agent's room is a relay room; sends queue while the link is down |
| DM | shown | same |
| People group | shown | same — "Offline" under Arbonites was a true reading of the group's room link |
| Server channel | shown | same (the channel's room id is the chat id) |

Rule in code: `header::ChatKind::shows_link_pill` (an exhaustive match, so a
new kind must decide). Geometry (`header::metrics`): top `LINK_PILL_TOP` =
pill top 12 + pill 36 + gap 3 = **51**, height **22**, centred on the pill;
its box reaches `LINK_PILL_OVERHANG` **13** pt below the 60 pt bar, so the bar
does not clip its children and the shell's floating header overlay is
`OVERLAY_HEIGHT` **73** tall (the capsule has no hitbox, and the drag
catchment under the bar — `DRAG_OVERHANG` 24 — already covers that band). It
steps aside while the ⌘F bar is open (that bar owns the slot under the pill,
`chat_search::metrics::TOP` 56). What Jacob saw on 2026-10-27 (Arbonites, Dark
Glass, v0.6.296): a4e0031ed (2026-10-24) had put `overflow_hidden` on the bar
and on the overlay at `HEIGHT`, so the capsule (51–73) showed only its top
9 pt — the upper half of "Offline". Pinned by
`header::link_pill_tests` (the real `Shell`, one chat of each kind: capsule
whole under the pill, inside the overlay's clip box, pill and bar unmoved;
gone with the link live; gone while ⌘F is open) and
`every_kind_shows_the_capsule_and_the_header_leaves_room_for_it`.

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
(#1374 / #1440); my own bubbles keep their 72 % rule. Agent 1:1 chats only:
people chats take 80 % of their side (§ "Chat density in people chats").

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

## Message formatting — the chat Markdown grammar (Jacob 2026-10-27, [feedback chat-markdown-core-2026-10-27], Mac [feedback chat-markdown-mac-2026-10-27], iPhone [feedback chat-markdown-ios-2026-10-27])

**One parser, every bubble.** `arbos_core::markdown` splits a message into
blocks and inline runs; the Mac (`ui/markdown.rs` is a thin adapter) and the
iPhone (`markdownBlocks` / `markdownSpans` over the FFI, `Markdown.blocks` /
`Markdown.inline`) render the same tree. Typed messages and agent replies go
through the same grammar: what Jacob types as `> hello` is the quote block Grok
Bot shows, in his own bubble and in the agent's. Plain text is never changed.

### Blocks (a line decides at its start; up to three leading spaces are fine)

| Typed | Block | Rule |
|---|---|---|
| `> text` | **Quote** — 3 pt bar in the muted ink at 55 %, 10 pt gap, the quoted blocks in the muted ink | A line is a quote line when it starts with `>` followed by **a space**, another `>` (nesting, up to four deep) or nothing. The quote is the run of such lines; it ends at the first line without `>`. The first line must have text after its markers: a lone `>` is prose, `>>` alone is prose, `>5 means more than five` (no space) is prose; `> 5 means five` is a quote. A `>` line with nothing after it inside the run is a paragraph break. Any block may be quoted (headings, lists, fences, tables, another quote). |
| `# text` · `## text` · `### text` | **Heading** — bold, 20 / 17 / 15 pt on the Mac, 24 / 21 / 19 pt on the iPhone; never an anchor | `#` × 1–6 then **a space** then text; four or more `#` render like three. `#hashtag`, `#1 fan` and a lone `#` are prose. |
| `- a` · `* a` · `+ a` · `• a` | **Bullets** (the disc from "Markdown inside bubbles") | marker + space + text; `-5 degrees` and `*bold* start` are prose. |
| `1. a` · `1) a` | **Numbered** — renumbered from 1 | 1–9 digits + `.` or `)` + space + text; `3.14 is pi` is prose. |
| ```` ```lang ```` … ```` ``` ```` | **Code card** (the existing card: mono, copyable, highlighted by the tag) | contents verbatim, markers inside it never apply; an unclosed fence runs to the end (streaming). |
| `\| a \| b \|` + `\|---\|---\|` | **Table** | unchanged (two pipe rows without the separator degrade to one mono block). |
| `$$ … $$` · `\[ … \]` | **Display math** | unchanged ([feedback math-render-mac-2026-10-26]). |
| anything else | **Paragraph** | a single Enter is a line break (soft newlines are kept), a blank line is a paragraph break. |

### Inline (inside paragraphs, headings, list items and table cells)

| Typed | Renders | Rule |
|---|---|---|
| `**bold**` | bold (700) | A marker **opens** only at a word start — after a space, punctuation or the line start — and never when glued to the same marker; it **closes** only at a word end (before a space, punctuation or the end), with a non-space just before it. The run must be exactly the marker: `****` is prose. |
| `*italic*` · `_italic_` | italic | same rule, so `snake_case`, `__init__`, `2*3*4`, `a*b`, `trailing*` and `*leading` are prose. `__bold__` is not a marker (Python dunders stay). |
| `***both***` | bold italic | same rule. |
| `~~gone~~` | strikethrough | same rule; a single `~` is never a marker. |
| `\|\|spoiler\|\|` | **Spoiler** — a solid pill in the muted ink at 90 % with the glyphs invisible; one click / tap opens it for **this viewer only** (a faint 16 % wash stays); nothing is stored or sent | same rule, so `a \|\| b` and `x\|\|y` are prose. A spoiler may span lines; copies carry the words. A link inside a hidden spoiler opens on the next click, once it can be read. |
| `` `code` `` | mono chip (the existing chip) | a backtick run closes with a run of the same length; `` `` a`b `` `` holds a backtick; one space of padding each side is dropped; an unmatched backtick is prose. Markers inside code never apply. |
| `[text](https://…)` | link in the accent, underlined, with its site icon ([feedback link-icons-mac-2026-10-25]); opens through the same click path as every link (`select.rs` → `open_url` / the iPhone's `OpenURLAction`, so the link-open confirm of #1522 applies when it ships) | the target must be `http(s)://`, `mailto:` with an `@`, `arbos://` or `www.`; `javascript:`, `data:` and relative targets leave the text as typed. Balanced parentheses inside the target are kept (Wikipedia), an optional `"title"` is dropped. The label may carry styles. |
| `https://…` · `http://…` · `www.…` | link (no icon) | up to whitespace or `<` `>` `"`; trailing `.,;:!?` and an unbalanced `)` stay prose. E-mail addresses are **not** linked and never styled. |
| `$…$` · `\(…\)` | typeset math | unchanged (Pandoc's rule: `$5 and $10` are prices). |
| `\*` `\_` `\~` `\|` `` \` `` `\[` `\#` `\>` `\\` … | the character itself | a backslash before any ASCII punctuation escapes it (`\> not a quote` at a line start is prose); before a letter or a space it is a backslash. |
| `<b>html</b>`, `<script>` | the text as typed | HTML is never interpreted. |
| `@name`, emoji, `:shortcode:` | unchanged | mentions are resolved after parsing as before (chips); emoji pass through. |

Emphasis stays on one line of a paragraph (a `*` on one line and a `*` on the
next are prose); an unterminated marker is literal, so a half-streamed
`**bold` never flickers.

### Never mangled — pinned by `arbos_core::markdown::tests`

`snake_case_word`, `__init__`, `2*3*4 = 24`, `5 * 3 = 15`, `a_b`, `a*b`, `a~b`,
`a|b`, `x||y`, `a || b`, URLs with `_` or `*` in the path, `jacob_s@example.com`,
`5 > 3`, a lone `>` / `*` / `_` / `` ` `` / `#`, `** spaced **`, `*leading`,
`trailing*`, `#hashtag`, `#1`, `C++`, `C#`, `100%`, `3.14`, `$5 and $10`,
`<b>html</b>`, `[not a link]`, `[x](y`, emoji and skin tones — each renders as
typed. Two property tests back the list: 4 000 random texts built from such
words (plus at most one marker word) round-trip through the parser unchanged,
and 20 000 random marker-heavy strings parse without a panic and never show a
character that was not typed.

Code: `crates/arbos-core/src/markdown.rs` (grammar), `crates/arbos-ffi/src/markdown.rs`
(Swift surface, quotes flattened to `quote_depth`), Mac `ui/markdown.rs`
(adapter: typeset math runs, icon cells, URL wrap hints) + `transcript.rs`
`render_block_in` (quote bar, ink colour per bubble) + `inline_runs`
(strike, spoiler pill) + `select.rs` (spoiler click, per-viewer reveal set),
iPhone `Transcript.swift` `Markdown` / `BlockView` / `TypedBody` / `SpoilerState`.
Rig: `ARBOS_FIXTURE_REPLY=markdown` (the sample sheet), `=quote` (Jacob's
"> hello").

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

## Menus and every other glyph: standard SF Symbols

### Standard SF Symbols, same names as iPhone (Jacob 2026-10-27, [feedback menu-sf-symbols-2026-10-27]; every surface [feedback sf-symbols-everywhere-mac-2026-10-28]; supersedes [feedback reply-in-dm-icon-2026-10-27])

**Every glyph the Mac app draws — menus, toolbar and strip discs, the rail,
chat header, composer, transcript marks, hover rail and reactions, footer,
settings rows and Devices tiles, people and profile panes, connectors,
library and media viewer, call view, search surfaces — is Apple's system
symbol, by its SF Symbols name, the same name the iPhone passes to
`Image(systemName:)`; never a glyph of our own.** Jacob, on the redrawn
speech bubble of v0.6.302: "use standard versions instead of making them."
The hand-drawn pack (`ui/icons.rs`) is frozen and now serves only as the
fallback for a symbol the running macOS lacks (and the Linux dev build);
its one living custom path is the activity spinner (marked
`// icon: custom` — SF has no fading-spoke dial). Brand art (the leaf
avatars, the black-and-white tree) is not an icon and is not in this rule.

| Surface rule | Detail | Where |
|---|---|---|
| Drop-in | `sf_symbols::glyph(name, size, color)` replaces `icons::icon(name, size, color)` one for one: same `size` box in the layout, the symbol at **`size × 0.85` points** (`GLYPH_POINT_RATIO`, the menu precedent 13-in-15), **medium** weight (the pack's 2 pt stroke on the 24 grid ≈ SF Medium), semibold for the marks the pack drew heavier (`checkmark`, `arrow.up`, `arrow.down`, `waveform`, `number`, `chevron.left.forwardslash.chevron.right`, `exclamationmark.circle`) | `sf_symbols::glyph`, `HEAVIER` |
| Discs | strip / rail / header discs keep `CONTROL_GLYPH` 19 and `CONTROL_GLYPH_WIDE` 17 boxes; `magnifyingglass` inks ≈ 14.5 of Grok's 14, `plus` ≈ 12.9 of 13, `sidebar.left` ≈ 14 — pinned by `sidebar::strip_tests::the_three_discs_are_groks_size` | `StripControl::glyph`, `rail::disc`, `glass::control_disc` |
| Names | the full pack → symbol map is `sf_symbols::FALLBACKS` (79 names, every one present on macOS 26.6 — probed): `search` → `magnifyingglass`, `person` → `person.fill`, `people` → `person.2.fill`, `gear` → `gearshape`, `check` → `checkmark`, `hash` → `number`, `code` → `chevron.left.forwardslash.chevron.right`, `camera` / `camera-off` → `video.fill` / `video.slash.fill`, `mic-off` → `mic.slash`, `phone-down` → `phone.down.fill`, `share` → `square.and.arrow.up`, `leave` → `rectangle.portrait.and.arrow.right`, `cloud-download` → `icloud.and.arrow.down`, `window` → `macwindow`, `desktop` / `laptop` / `smartphone` / `bot` → `desktopcomputer` / `laptopcomputer` / `iphone` / `server.rack`, `circle-half` → `circle.lefthalf.filled`, `info` → `info.circle`, `exclamation` → `exclamationmark.circle`, `face-smile` → `face.smiling`, `puzzle` → `puzzlepiece`, `antenna` → `antenna.radiowaves.left.and.right`, `expand` → `arrow.up.left.and.arrow.down.right`, `download` → `arrow.down.to.line`, `text-align-left` → `text.alignleft`; identical names (`plus`, `xmark`, `trash`, `link`, `globe`, `photo`, `doc`, `key`, `lock`, `video`, `eye`, `mic`, `bell`, …) stay | `sf_symbols::FALLBACKS` |
| Lint | `icons::tests::every_surface_draws_system_symbols` walks the crate's sources: no `icons::icon(` call outside `icons.rs` / `sf_symbols.rs` without a `// icon: custom <why>` line above it, and every literal handed to `sf_symbols::glyph(` / `symbol(` is a registered symbol; `no_new_hand_drawn_glyph_without_a_custom_marker` freezes the pack's names | `icons.rs` |

| Item | Rule | Where |
|---|---|---|
| Name | the SF Symbols name, registered in `sf_symbols::FALLBACKS` with the pack glyph that stands in for it; message menu = `doc.on.doc` · `arrowshape.turn.up.left` · `arrowshape.turn.up.right` · `bubble.left` · `trash` (`MessageMenu.swift`'s `systemImage`); chat-list menus `archivebox` / `tray.and.arrow.up`, `bell` / `bell.slash`, `arrow.triangle.branch`, `pencil`, `person.crop.circle`, `trash`; member menu `person.crop.circle`, `xmark`; library menu `photo`, `arrowshape.turn.up.right`, `arrow.down.to.line`, `globe`, `link`; tab menu `plus`; connectors `globe`, `doc.on.clipboard` | `message_menu::MessageMenuItem::glyph`, `sidebar.rs`, `member_menu.rs`, `profiles/library.rs`, `header.rs`, `connectors.rs` |
| Size and weight | point size = the label's (`TEXT_SMALL` 13, `glass::MENU_SYMBOL_PT`), weight regular, scale medium — what a SwiftUI `Label` does; each symbol keeps Apple's natural canvas (a `trash` at 13 pt is 15 × 17) centred in a fixed **20 pt slot** (`MENU_SYMBOL_SLOT`) so labels align, 8 pt gap to the label | `glass::menu_row` |
| Rendering | `NSImage(systemSymbolName:)` + `NSImageSymbolConfiguration(pointSize, weight, .medium)`, drawn as the template into an `NSBitmapImageRep` of natural size × the window's backing scale (2× Retina, no resampling), tinted with the row's text colour by a `sourceIn` fill, handed to GPUI as a premultiplied BGRA `RenderImage`; one raster per (name, size, weight, scale, colour), cached. Why not text or paths: the system font does not carry the symbols under their names (`CTFontGetGlyphWithName` knows `plus`, `minus`, `ellipsis` only — probed on 26.6), so AppKit's own raster of the system artwork is the crisp, weight-faithful route, and it is how NSMenu draws symbol images too | `ui/sf_symbols.rs` |
| Fallback | a name `NSImage` does not know (older macOS) and every non-mac build draw the registered pack glyph at the slot size; nothing else in a menu may draw from the pack | `sf_symbols::symbol`, `FALLBACKS` |
| Message pill | the person pane's Message pill draws `bubble.left` at 13 pt **semibold** (the label's weight) in an 18 pt slot | `kit::pill_button`, `PILL_GLYPH*` |
| New glyphs | a new hand-drawn `icon!` entry is refused unless a `// icon: custom <why>` line sits right above it (why no symbol fits); the pack's current names are frozen in the test | `icons::tests::no_new_hand_drawn_glyph_without_a_custom_marker` |

Pinned by `icons::tests::menus_draw_system_symbols_only` (every enum-driven
menu glyph and every literal `menu_row(id, "…", …)` in `sidebar.rs`,
`header.rs`, `connectors.rs` must be a registered symbol; every fallback is a
real pack glyph), `no_new_hand_drawn_glyph_without_a_custom_marker`, and
`no_glyph_closes_with_a_stub` (the pack's own shape check, kept for the
fallbacks). Rig: `ARBOS_RIG_MESSAGE_MENU=<x>,<y>` +
`ARBOS_RIG_MESSAGE_MENU_FROM=someone` opens the menu on another person's
newest row; `/Users/Shared/arbos-tools/bin/cgclick x y right` is a real
right-click.

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

### iPhone bottom bars: rows stay readable behind the glass pills, a soft fade toward the edge (Jacob 2026-10-28, [feedback iphone-composer-see-through-2026-10-28]; supersedes [feedback iphone-composer-bottom-fade-2026-10-28])

Jacob on TestFlight 202610281715985037: "yes this is not it. we should actually
see the text behind the search bar." Grok Bot (his dark chats-list shot): the
next row scrolls visibly UNDER a glass round button and a glass "Search"
capsule — text and avatar dimmed but readable through and between the pills,
no opaque band anywhere, a soft fade darkening the content toward the bottom
edge. The opaque band shipped earlier that day is gone; this rule holds for the
chat composer and Home's search capsule alike, light + dark, Matte + Glass,
keyboard open.

| Item | Rule (pt / alpha) | Code |
| --- | --- | --- |
| Layer | one layer at the canvas's bottom, over the content, under the controls — the page colour masked by a ramp; the bar has no background of its own | `BottomBarFadeView`, the canvas `.overlay(alignment: .bottom)` in `ChatView` / `HomeView` |
| Ramp | clear **28** above the bar's top edge → **0.25** at that edge → **0.4** at the controls' bottom edge → **0.75** at the screen's (or keyboard's) edge (0.3 / 0.5 / 0.8 until the pills became clear glass); never 1 — no band, no hard edge; readable over every control (≤ **0.6**) | `BottomBarFade.fade / edgeWash / controlsWash / bottomWash / readableMax`, `.chat` / `.home` |
| Controls | **real, clear glass on Glass** (Jacob 2026-10-28, [feedback iphone-composer-glass-2026-10-28]: "Grok chat is actual transparent glass. Ours isn't."): Liquid Glass `.clear` on iOS 26, plain `.ultraThinMaterial` before — no fill, no `field` tint, no wash of ours on the pill, so the text reads through it with only the system's frost; the `+` disc and the field in the chat, the search capsule on Home; Home's `+` takes the chat header's glass. Matte: the field colour. Sizes are the control family's | `ChatHeaderGlass.pillGlass`, `ControlSurface`, `HeaderGlassBackground(clear:)` |
| Lists | the last row / result at rest sits above the fade: transcript tail = Home's trailing clear row = the fade | `Theme.transcriptTail`, `HomeChrome.listBottom` |
| Keyboard | the keyboard is the bar's safe area: the layer ends on it and moves with it | `BottomBarFadeView` (GeometryReader reads the inset) |
| System edge effect | none under the bar on iOS 26+ (`scrollEdgeEffectHidden(for: .bottom)`): one fade, ours | `NoBottomScrollEdgeEffect` |

Pinned by `ArbosTests/BottomBarFadeTests` and `PillGlassTests`. Records: project store
`internal/feedback-loop/iphone-composer-see-through-2026-10-28.md` and
`iphone-composer-glass-2026-10-28.md`, shots in `media/feedback/iphone-composer-fade/` and
`media/feedback/iphone-composer-glass/`.

### iPhone Home: the bars float over the list, rows fade out under them (Jacob 2026-10-28, [feedback iphone-home-bar-fades-2026-10-28])

Grok's chat list: the top buttons and the bottom search bar float over the list;
rows stay visible and fade out under them while scrolling. Ours cut the list
hard — an opaque block through the discs and the whole tab bar at the top
(#1988's answer to rows behind the status bar) and an opaque block behind the
search capsule at the bottom. Supersedes #1988's opaque-through-the-tabs rule;
nothing still reads behind the status bar itself.

| Item | Rule (pt / alpha) | Code |
| --- | --- | --- |
| Top layer | opaque through the status bar + **4**, then one eased ramp: **0.75** at the disc row's bottom, **0.55** at the tab bar's bottom, **0** where the first row rests (`listTop`, unchanged); search mode has no tabs: 0.75 at the discs, 0 at `barOnlyListTop` | `ListTopFade.Screen.home/.homeSearch`, `HomeChrome.topSolid / discsWash / tabsWash` |
| Bottom layer | the shared see-through fade of the rule above (`BottomBarFade.home`): clear 28 above the capsule, 0.3 at its top edge, 0.5 at its bottom edge, 0.8 at the home indicator / keyboard — rows readable behind the capsule | `BottomBarFadeView`, `BottomBarFade.home` |
| Lists | leading clear row = the top layer's height (first row reachable at rest), trailing clear row = the bottom fade (last row / result reachable) | `HomeChrome.listTop`, `listBottom` |
| Controls | unchanged: glass discs / capsules of the control family (`GlassCircle`, `controlField()`) | — |
| Material | top layer — Matte: the page colour, Glass: `.ultraThinMaterial` under the 0.94 wash; bottom layer — the page colour on both; same geometry | `ListTopFade`, `BottomBarFadeView` |

The chat bar floats the same way (rule above). Pinned by
`ArbosTests/HomeBarFadeTests` and `ListTopFadeTests`. Record: project store
`internal/feedback-loop/iphone-home-bar-fades-2026-10-28.md`, shots in
`media/feedback/iphone-home-bar-fades/`.

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

### iPhone sheet header: the confirm control is never squeezed (Jacob 2026-10-26, [feedback iphone-add-members-2026-10-26])

Jacob's 17:52 CR shot: on "Add members to Arbonites" with @arbos picked, the system inline
navigation bar gave the title the room and cut the Add pill to "A…" — and the tap target with
it ("Add members button broken on iPhone"). Rule, for every sheet whose title can be long
(Add members to <group / server>, Give a person access, New group, Edit group, New channel,
Add agent, the Secrets sheets, Share <agent>): the sheet draws its own bar, `SheetHeaderBar`
(`UI/SheetHeader.swift`, `.sheetHeader(title:…)`), in place of the system bar.

| Part | Rule |
| --- | --- |
| Bar | `Controls.height` (48) tall, `Controls.inset` (16) side insets, `Controls.top` gap, `bg` fill, 0.5 pt `separator` under it — one more `Controls.Screen` (`sheetHeader`). |
| Controls | Cancel (or a back chevron on a pushed step, or nothing) leading; the confirm trailing: a `primary` capsule `Controls.sheetAction` (36) tall with `Controls.textInset` sides for Add · Add (n) · Create · Save, plain semibold text for the Secrets sheets' Save / Replace / Connect and Done. Both are `fixedSize()` with layout priority: **never compressed, never truncated**, full hit area. |
| Title | `Theme.body` semibold, centred on the bar, in the room left symmetric around the centre (`SheetHeaderLayout.titleRoom`); shrinks to 70 % (`minimumScaleFactor`) and then cuts with an ellipsis. |
| Label | `SheetAction.label`: "Add" for one pick, "Add (n)" for several, "Adding…" while the call runs; "Create" / "Creating…" for a new group. |

Pinned by `ArbosTests/SheetHeaderLayoutTests` (every label × the longest titles × 320–430 pt:
the control keeps its width and its place, the title never runs under it) and
`AddMembersActionTests` (the Add tap posts one batch, replaces the roster on 200, reports a
refusal, falls back per item on an old relay).

## Chat density in people chats: Discord-size pictures, wider bubbles (Jacob 2026-10-26, [feedback chat-density-ios-2026-10-26], Mac [feedback chat-density-mac-2026-10-26])

Jacob (17:54 CR, iPhone Arbonites group): "In chats the pfp chat bubble takes
up too much space. Go for Discord standard size and use more of the screen both
ways, left and right per chat bubble. So reduce the empty space on the left and
right." Discord's message avatar is **40 pt in a 72 pt gutter** (desktop, cozy:
avatar at 16, content at 72, 48 free on the right) and **32 pt on mobile**.
The rule covers **people chats** — DM, group, server channel, a project with
people in it: every chat whose bubbles carry a sender disc. The 1:1 agent chat
keeps its Grok-measured widths (screen − 76 on iPhone; the 560 pt ceiling and
my 72 % on Mac): Grok has no group chats to measure, so people chats follow
Discord's density instead.

### iPhone (402 pt screen, 3 px/pt; the list pads 16 each side)

| Item | Rule (pt) | Was | Code |
| --- | --- | --- | --- |
| Sender disc | **36** — Discord's size (between its 40 desktop and 32 mobile); centred on the composer's `+` as before, so its left edge sits **6 inside the `+`** = the status picture's edge (`presenceAvatarOffset`): one left edge for every picture in the chat | 42 | `Theme.memberAvatar` |
| Disc → bubble | **10** (= `Controls.gap`, the `+` → field gap) | 13 | `Theme.memberAvatarGap` |
| Disc / bubble body from the screen | keyboard up **24–60 / 70**; down **37–73 / 83** (the column follows the `+`, whose inset is Grok's 18 / 31) | 21–63 / 76; 34–76 / 89 | `Theme.avatarColumnInset`, `memberLane` |
| Far-side margin | **32** on both sides: my bubble's left edge never passes 32 from the screen, an incoming bubble's right edge never passes 402 − 32 | 60 | `Theme.bubbleOppositeMargin`, `peopleBubbleInset` = 16 + 32 |
| My bubble cap | **354** (right edge 16, as Grok) | 326 | `RowView.maxBubble(width:people:)` |
| Incoming bubble cap | **300** keyboard up / **287** down (354 − lane); runs to 370 | 253 → 342 | same, minus `memberLane` |
| Question cards, pictures with a lane | the incoming cap; a photo + caption bubble stays ≤ 280 (#1840) | — | `QuestionCard`, `ImageBubble` |
| Status row, composer `+`, system notices ("Const added Aureum") | unchanged: the `+` is Grok's; the status picture (25⅓, 6 inside the `+`) now shares the disc's left edge; notices stay centred | — | `PresenceRow`, `.system` |

Pinned by `ArbosTests/ChatDensityMetricsTests` (disc, gap, column, caps,
far margin, agent-chat numbers untouched) and `PresenceRowTests`.

Not moved, on purpose: the composer's side inset (Grok's 31 keyboard down /
18 up, #1841) — it is the left gutter with the keyboard down; moving the disc
off the `+` would break Jacob's avatar-column rule, moving the `+` would break
Grok parity. If the keyboard-down gutter still reads wide, that inset is the
lever, and it needs a Grok keyboard-down reference first.

### Mac

The Mac column is already denser than Discord's: a **28 pt disc** (= the `+`,
Jacob's column rule) in a **50 pt gutter** (disc 8 inside the pill's edge, 14 to
the body, the body on the composer's text start — `transcript::SENDER_LANE`)
against Discord's 40 in 72. Disc, gap and the 16 pt column margin (Grok's
composer edge, #1374) stay. The widths move:

| Item | Rule | Was | Code |
| --- | --- | --- | --- |
| Incoming bubble in a people chat (a person's or an agent's), cards after a lane | **80 % of the slot** after the sender lane; 20 % stays free on the right | person 72 % of the slot; agent `min(slot − 84, 560)` | `transcript::PEOPLE_BUBBLE_FRACTION`, `people_cap`, `people_slot` |
| My bubble in a people chat | **80 % of the column** from the right; 20 % free on the left | 72 % | `own_bubble_fraction(true)` |
| Agent 1:1 chat | unchanged — agent `min(slot − 84, 560)`, mine 72 % | — | `assistant_cap`, `OWN_BUBBLE_FRACTION` |
| Reference window (1180 wide, 296 sidebar → 852 column) | incoming 66 → 708 (641.6 wide, 160 free); mine 682 wide (170 free on the left) | incoming agent 560 (242 free), person 577; mine 613 (239 free) | — |
| Grok's 960 column | incoming 728 wide (182 free), mine 768 (192 free) | agent 560 (350 free), person 655; mine 691 | — |

One share at every width: below a 750 pt column an agent's group bubble is
narrower than Grok's 560 agent-chat ceiling (517 at ref 57's 696 column); a
person's beside it is wider than its old 72 % everywhere, and the two are now
the same width. The hover rail lives in the free 20 %. Pinned by
`transcript::layout_tests::people_chat_bubbles_take_four_fifths_of_their_side`,
`cards_share_the_bubbles_left_edge_and_cap`, `bubbles_stay_inside_the_column_and_wrap_their_text`
and the `people_chat` rule in `names_senders_follows_the_chat_kind`. Record:
project store `internal/feedback-loop/chat-density-2026-10-26.md`, shots in
`media/feedback/chat-density/`.

## Grouped messages: one name and picture per run (Jacob 2026-10-28, iPhone [feedback grouped-messages-ios-2026-10-28], Mac [feedback grouped-messages-mac-2026-10-28])

Asked by the Arbonites (DaWizzaa / Scout / Affinity), approved by Jacob 17:56
UTC: "consecutive messages from one sender within ~2 min form a run — name and
avatar once, phone gaps inside a run about half of the normal, Mac grouped the
same with its spacing otherwise unchanged." Grok Bot's stacked agent bubbles
(`internal/refs/grokbot-ios/web-09`, `ref-01`) are the visual reference.

### The run (both platforms, one rule)

| | Rule |
| --- | --- |
| A run | Consecutive messages from one sender (`sender` key: a person, an agent, me) each within **2 min** (`RUN_WINDOW_MS` = `Transcript.runWindowMs` = 120 000 ms) of the run's last message. The window is measured from the previous message, so a steady stream chains. A row with no clock never groups. |
| Breaks | The sender changes · the window passes · a date stamp · a system line (call lines, membership notices) · a bubble with a **reply quote** (`replyTo` / `replyRef`; an agent's `inReplyTo` in a people chat) — the quoted bubble starts a run of its own, and the next unquoted line from the same sender joins it. A hidden fold ("3 messages hidden …") also ends the run. |
| First message of a run | Name inside the bubble (in chats that name their senders) and the sender's picture on the avatar column — on the message's last row, as before (the disc at the bottom of the message; redesign mocks a/f). |
| Later messages of a run | No name, no picture; the avatar column stays **empty at the picture's width**, so the bubble aligns under the first one's text. Reply quotes, reactions, attachments, the hover rail (Mac) / swipe-left time (iPhone) and the per-row menus are unchanged: every row is still its own message. |
| Who groups | People, agents (in groups and in their own 1:1 chat) and my own messages alike. Supersedes #1447's "name and gap on every agent reply": an agent's reply that quotes the ask is still named (the quote breaks the run); its unquoted follow-up bubbles group. |

The two row builders live in the clients (`crates/arbos-mac/src/ui/transcript.rs`
`continues_run` / `carries_quote`; `apps/ios/…/UI/Transcript.swift`
`Transcript.continuesRun` / `carriesQuote`), not in `arbos_core`, so the window is
one constant per client, pinned by a test on each side (`grouping_tests::
the_window_is_two_minutes_and_the_gaps_are_groks`, `GroupedMessagesTests.
testTheWindowIsTwoMinutesAndTheGapsAreTighter`). Change both or neither.

### Spacing

| Gap | iPhone | Was | Mac | Was |
| --- | --- | --- | --- | --- |
| Inside a run (between its messages, and between the rows of one message) | **4** (`Transcript.runGap`) | 8 | **6** (`2 × ROW_GAP_HALF`) | 6 |
| A new run (sender change, window passed, quoted reply) | **16** (`Transcript.turnGap`; Grok Bot's iPhone turn gap, ≈ 15–16 pt on `ref-01` at 1.17 px/pt) | 24 | **20** (`TURN_GAP_EXTRA + 2 × ROW_GAP_HALF`) | 20 |
| Around a centred system / date line | 20 (`Transcript.systemGap`) | 20 | 20 / 24 (stamp pads) | same |

The Mac's rhythm is Grok's Mac measurement and stays; only *where* the 20 pt
opens changed (at run starts, not at every agent message). On the phone the two
gaps fall from 8 / 24 to 4 / 16 — "overall phone spacing tighter".

Pinned by `transcript::grouping_tests` (the rule, agents, own messages, the row
cache's run starts) and `transcript_smoke_tests::a_run_shows_one_disc_and_opens_no_turn_gap`
(the real transcript drawn: one disc, same left edge, 6 / 20 pt) on the Mac;
`ArbosTests/GroupedMessagesTests` (rule, gaps, avatar once per run, agents, own,
DM) and the updated `SenderNameRowTests` on the iPhone. Record: project store
`internal/feedback-loop/grouped-messages-2026-10-28.md`, shots in
`media/feedback/grouped-messages/`.

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
| Window material | A real system material, installed once under GPUI's content view; its `appearance` follows the theme (dark material under a dark theme, whatever macOS is set to), so an appearance switch re-tones it and never drops it. **macOS 26: Liquid Glass (`NSGlassEffectView`), adaptive by design; before 26: AppKit's under-window material with its tint intact** (Jacob 2026-10-26 15:12, [feedback mac-glass-adaptive-2026-10-26]; #1922 had used `Sidebar`). GPUI's own background is `Transparent` on Glass so the two do not stack. | `ui::vibrancy` (`BackdropPlan`, `apply`, `liquid_available`), `GlassMaterial` (`ARBOS_GLASS_MATERIAL=liquid|sidebar|underwindow|…|none` for the rig) |
| Opacity slider | Still 0 … 100 %, floor **0 %** (#1425). It sets the shell wash only; legibility never depends on it. | `GlassOpacity`, `Theme::shell()` |
| Adaptive washes | Every Glass region — sidebar, chat column, right pane, floating cards — is washed with its own tint at the alpha **the text on it needs over the desktop that is actually there**: body ≥ 4.5:1, muted ≥ 3:1 (WCAG, sRGB compositing), never under the region's floor (chat / pane 0.25, sidebar 0.25, cards 0.55). A black desktop under a dark theme, or a white one under a light theme, gets the floor and the picture shows through; a white desktop under white text gets what it takes (≈ 0.8). The desktop is read by `ui::backdrop` — `CGWindowListCreateImage` of the screen region below the window (our own window excluded, nominal resolution), reduced to three band luminances (left quarter / middle half / right quarter), every 1.5 s and at once on a move or resize, repainting only on a change ≥ 0.02. A blank capture (no Screen Recording grant for other apps' windows) is not a sample. Without a sample the washes assume the worst desktop — #1922's fixed values. The window material is never credited: it can only add. | `Theme::adaptive_wash` / `wash_alpha` / `needed_alpha`, `Theme::backdrop`, `GlassRegion`, `CHAT_WASH_FLOOR` / `CHROME_WASH_FLOOR` / `CARD_WASH_FLOOR`, `ui::backdrop` |
| Floating cards | Menus, sheets, pickers: the adaptive card wash with the header's 1 pt rim and top highlight on Glass. | `Theme::card()`, `glass::plate` |
| Text on glass | Still GPUI's flat `text` / `text_muted`; the adaptive washes carry the contrast. Vibrant label colours (plusD / plusL over the sampled backdrop) are the next item, Jacob-tier, with a lint that every Glass container uses them. | — |
| Untouched | Traffic lights native; sizes and placements; bubbles, discs and hairlines keep their colours (they sit on the washed surfaces above); Matte, except the header pill rule. | — |

Pinned by `theme::glass_contrast_tests` (white / black / photo × light / dark × 0 / 50 / 100 %, sampled and unsampled; the floors over the friendly desktop), `backdrop::tests` (throttle, change gate, bands, coverage) and `vibrancy::tests` (Liquid on 26, under-window before, none on Matte, kept across dark → light → dark).

### Right pane components on Glass (Jacob 2026-10-28, [feedback right-pane-glass-cards-2026-10-28]; approved 18:17 UTC on the Xvfb sheet)

Jacob's Members-tab shot: the pane wore the adaptive wash but its cards
("Add members / Description", the member list), the Members · Library
capsule, the glyph tiles and the fields were still painted with their Matte
tokens — `pane_card`, `surface_2`, `field`, `bubble`, opaque hex colours on
every surface — so they sat as solid slabs on a see-through pane while the
sidebar beside them was frosted. "We can still have the glass for basically
every component in the pane when we go glass."

**On Glass every pane component is a wash of the text colour** (white-alpha
in dark, black-alpha in light — the material's own plusL / plusD idiom), so
the desktop shows through it as through the pane and it rides the adaptive
wash; **Matte returns the old tokens untouched.**

| Component | Matte | Glass dark | Glass light | Code |
| --- | --- | --- | --- | --- |
| Grouped card (`kit::card`: members, settings, Connection / Key cards, Library lists) | `pane_card` | white 8 % + 1 pt rim white 10 % (inset shadow: no layout cost) | white 55 % + rim black 6 % | `Theme::pane_card_fill` / `pane_card_rim`, `PANE_CARD_GLASS_*` |
| Small controls: the round-button family — the pane's share / close / back discs, the sidebar strip's search · + · collapse and the collapsed rail's discs (Jacob 2026-10-28 16:58: "they should be glass"; hover and pressed stay `ui::highlight`'s two wash steps over the fill, the glyph the text colour) — the tab capsule, glyph tiles, Library thumbnails, hero glyph tile, Message pill, tag chips | `surface_2` | white 14 % | black 7 % | `Theme::control_fill` |
| Sidebar filter's selected capsule (All · Agents · People · Servers) | `bubble_assistant` | white 14 % | black 7 % | `Theme::filter_on_fill` |
| Fields in the pane (description / label / name editors, the handle field, Unarchive) | `field` | white 6 % | white 60 % | `Theme::field_fill` |
| Settings icon tile | `#5c5c60` / `#6e6e73` | white 22 % | black 42 % (white glyph ≥ 3:1) | `Theme::tile_fill` |
| Selected On / Off / Dark / Light segment | `bubble` | white 16 % | black 72 % | `Theme::segment_on_fill` |
| Secondary text (subtitles, details, section titles, footnotes, unselected tab, empty lines) | `text_muted` | text at 72 % (vibrant) | text at 62 % | `Theme::label_muted` |
| Tertiary (chevrons, placeholders) | `text_faint` | text at 45 % | text at 42 % | `Theme::label_faint` |
| Hairlines between rows, the pane's left edge | unchanged | unchanged | unchanged | `kit::hairline_below`, `pane_edge` |

Pinned by `theme::glass_contrast_tests::every_pane_component_reads_over_every_desktop_on_glass`
(card, field on a card, tile on a card, control on the pane: body ≥ 4.5:1,
vibrant muted ≥ 3:1 over white / black / photo × light / dark × 0 / 50 /
100 %, sampled and unsampled; the selected segment's label ≥ 4.5:1, the
icon tile's glyph ≥ 3:1), `…::on_glass_the_pane_components_are_translucent`
and `…::on_matte_the_pane_components_keep_their_solid_tokens`.

Rig: `ARBOS_MOCK_WALLPAPER=<pre-blurred png>` + `ARBOS_MOCK_BACKDROP_LUM=<0…1>`
(`ui::mock_backdrop`) paint a stand-in desktop under the window and seed the
sampler where there is no vibrancy (Linux / Xvfb previews); inert without the
variables (`mock_backdrop::tests`). The iPhone's `PaneCard` takes the same
card token (`Theme.paneCardFill` / `paneCardRim`, [feedback
right-pane-glass-cards-ios-2026-10-28]) so both panes match on Glass.

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
| Hover | the shared inset pill (§ Rows: hover / selected / pressed; the armed row draws it lit from state); the time gives way to a **24 pt ⋯ disc** (no layout shift) that opens the row's menu at the click: Open · Copy · Forward · Download for attachments (Copy puts the picture or file itself on the pasteboard, [feedback copy-image-messages-mac-2026-10-27]), Open link · Copy link for links | `MORE_DISC`, `menu_items` |
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
| Shape | one of the Grok button family: `theme::CONTROL` **36** tall, a capsule, side padding 18; `surface_2` fill (the discs'), the system `bubble.left` symbol at 13 pt semibold (§ Menus) and "Message" 13 pt semibold in `text`, 7 apart | `kit::pill_button`, `PILL_*` |
| Pointer | hover and pressed are the highlight token's two steps over the fill (`highlight::washed`), as the footer's update disc | `kit::pill_button` |
| Click | `AppState::start_dm(handle)`: the existing people space with that person is selected; else `POST /chats/dm` (the New Message sheet's path) and the DM is selected when the registry lists it. A relay refusal ("this person is not available" — they blocked you) shows as a notice | `user.rs`, `app.rs` |
| Hidden | on your own pane; once *you* blocked them (`GET /blocks`); the reverse direction is not in any payload the client reads, so the relay's answer covers it | `user::shows_message` |

Pinned by `profiles::user::tests::the_message_pill_is_for_other_unblocked_people`
and the headless `profiles::user::render_tests` (under the @handle at 36,
centred; click opens the DM we have / starts one; none on my own pane or a
blocked person's, back once unblocked). Rig: `ARBOS_OPEN_PROFILE=user`
(the DM peer's pane), `user:message` (the pill pressed), `group:member`
(a member with no DM yet).

### Link icons: one tile, bundled brand marks, no stretched favicons (Jacob 2026-10-27, [feedback link-icons-mac-tile-2026-10-27])

**Every site icon on a tile — the person pane's Links tab, the Library's
Links rows, the search palette's links, the link-embed cards — is
`ui::link_tile::tile`.** Jacob: the Spotify row had a crisp green mark, the
github.com row "a blurry, low-res, dark-on-dark favicon on a muddy grey
square". The iPhone's `LinkTile` follows the same table.

| Item | Rule | Where |
|---|---|---|
| Tile | a rounded square of the row's size (pane rows 36, Library 32, inline 14); corner 28 % of the side (36 → 10 = the kit's corner, 14 → 4); fill `surface_2` in both themes | `link_tile::tile`, `radius` |
| Known host | a bundled **vector** mark, 56 % of the side (36 → 20; 22 % clear space all round), never a fetched picture: GitHub, X, YouTube, Spotify, Apple Music, SoundCloud, Vimeo, Loom, Bluesky, Reddit, Google Docs / Sheets / Slides / Maps, Figma, Notion, Linear, Wikipedia, Arbos | `link_tile::Brand`, `assets/brands/*.svg` |
| Mark colour | a brand with its own colour keeps it on both tiles (Spotify `#1ED760`, YouTube `#FF0000`…); a black-and-white brand (GitHub, X, Notion, Wikipedia, Arbos) is `text` — white on the dark tile, near-black on the light | `Brand::ink`, `Brand::color` |
| Unknown host | the relay's icon (`GET /link-icon?domain=`: apple-touch-icon or the largest declared icon, resampled to the tile's device pixels) edge to edge; a transparent **dark** mark gets a light fill in the dark theme, a light mark a dark fill in the light theme (`Tone`, measured at decode) | `link_tile::Icon::Picture`, `app::link_icon` |
| No icon | the host's first letter, semibold, `text_muted`, on the plain tile — never a coloured disc, never `https://<host>/favicon.ico` | `link_tile::Icon::Initial` |
| Brand sources | each mark's press page is in `Brand::source`; shapes unaltered, used mono or in the published brand colour with clear space, per the owners' guidelines | `link_tile.rs` |

Pinned by `link_tile::tests` (host → brand incl. the three Google editors by
path and `google.com/maps`; every brand has one filled SVG path and a source;
tones; fills flip only across the mark's own tone; the headless render: 36 →
mark 20 at 8/8, 32 → 18 at 7/7). Rig: `ARBOS_FIXTURE_LINKS=1
ARBOS_FIXTURE_OPEN=01J9DMSAM00000000000000000 ARBOS_OPEN_PROFILE=user` —
Sam's Links tab with github.com, open.spotify.com, x.com and nypost.com.

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

### Blocked state (Jacob 2026-10-26, [feedback blocked-state-mac-2026-10-26])

**Once I block a person, every place that shows them says so, every Unblock
asks first, and a DM with them has no input.** Jacob: "Today you can't tell in
the app that you've blocked someone." The block list is the relay's
(`AppState::blocked`, `GET /blocks`); no surface keeps a local flag. The iPhone
has the same surfaces ([feedback blocked-state-ios-2026-10-26]).

| Surface | Rule | Where |
|---|---|---|
| Person pane | a **Blocked** chip (`nosign` glyph + word, `danger` on `danger_soft`, 22 pt) under the @handle; the hero's one action is **Unblock** (the Message pill is gone); the footer row reads **Unblock** (accent) | `profiles/user.rs`, `ui/blocked.rs` `chip` |
| Unblock confirm | the row (pane) or the bar (DM composer) becomes "Unblock @x?" · "They'll be able to message you and see you in groups again." · Cancel · **Unblock** (the kit's confirm buttons); Unblock sends `DELETE /blocks/{id}`; nothing changes until the relay's list comes back without them | `Shell::{arm_unblock, cancel_unblock, confirm_unblock}`, `blocked::confirm` |
| DM header pill | a **Blocked** tag (16 pt capsule, 10.5 semibold) after the name | `header.rs`, `blocked::tag` |
| Member lists (group / shared project) | the same tag on the member's row, before the owner's remove × | `profiles/group.rs` |
| DM composer | "You blocked X · Unblock" — a `COMPOSER_HEIGHT` bar on `surface_2`, the composer's radius, in the composer's place (transcript and empty-state layouts alike) | `Shell::blocked_composer`, `blocked::composer_bar` |
| Refused push (chat-level note) — **DMs** | the shared `error_note` with `PushRefusal::sentence()`: "<name> has blocked you in this chat." · "Couldn't post: you've blocked <name>, who is in this chat. Unblock to post." (+ an **Unblock** capsule → the confirm) · the mute line · else the generic delivery line. The relay's code stays `not_available`; the words come from its `reason` | `Shell::render_delivery_note`, `sync/mod.rs` |
| Groups: hidden for the blocker alone (Jacob 2026-10-26 17:28 CR) | a group block never refuses the room. The blocked person's rows fold, per run, into one muted stamp-style line "**N messages hidden because you blocked <name> · Unblock**" (`nosign` glyph; Unblock opens their pane on the confirm) — for the blocker only; everyone else sees the messages. The blocker's sidebar preview, unread dot, notifications, search hits and Library skip those rows. Senders come from the relay's `blockedSenders` hint on the chat plus my block list (`AppState::hidden_senders_in`, `hides_message`) | `transcript::build_rows_hiding`, `RowKind::Hidden`, `blocked::hidden_row`, `sidebar::preview`, `AppState` arrivals / search, `profiles::collect` |

Pinned by `blocked::hidden_tests` (runs fold into Hidden rows; the real shell: only the
others' bubbles, the preview skips the blocked sender, Unblock on the fold opens the pane
on the confirm, unblock reveals every row; a hidden arrival leaves the dot dark while a
third member's lights it) and `blocked::render_tests` (the real shell: pane chip / pill / row and
the confirm's Cancel and Unblock with the one command; the DM pill's tag and
the bar, the confirm in its place, the field back once unblocked; the member
list's tag on the blocked member only; the note's sentences and its Unblock
capsule) and `sync::push_refusal_tests` (the sentences). Rig:
`ARBOS_RIG_FAILURE=blocks-member|blocked-by` (the note, every chat) with the
fixture's `ARBOS_FIXTURE_BLOCKED=<handle>` (my block list + the groups' `blockedSenders`).

## Rows: hover / selected / pressed (Jacob 2026-10-27, [feedback row-hover-style-2026-10-27])

Jacob (dark, v0.6.29x): "on hover, rows get a hard full-bleed rectangle fill with square corners
flush to the container edges" — the Members card's **Add members**, the agent pane's Routines rows,
the add-agent picker. Every list drew its own `surface_2` box over the whole row. Required, Grok
Bot's: **a subtle inset highlight — a soft low-contrast rounded pill inset a few points from the
container edges, its corner radius the container's inner radius, never a hard rectangle; selected =
a slightly stronger fill of the same shape; pressed = one step more.** One token for the whole app,
`crates/arbos-mac/src/ui/highlight.rs`; nothing outside it sets a hover, pressed or selected fill.

| | Rule | Code |
|---|---|---|
| Shape in a card | the pill is a layer **under** the row's content, inset **`INSET` 4** from the row's box on all four sides (so 4 from the card's edges and 4 clear of the kit divider), radius **card − 4** (`inner_radius`: kit card 12 → **8**, `glass::group` / sheet 14 → **10**, dropdown 10 → floor **6** = `MIN_RADIUS`). The row's own box never gets a fill | `highlight::row(el, selected, Shape::Nested(card_radius), theme)`; `kit::row` / `kit::row_state` carry the layer, interactive callers join `.group(kit::HOT)`; `glass::hot_row` |
| Shape when the row is the pill | rows measured on Grok that already sit inset with their own radius keep it and take the fills on their box: sidebar chat / server / channel rows (inset 8, radius `PANEL_RADIUS − 4` 10), the rail tile (54, radius 12), the Hidden-chats and account rows, palette rows (12), menu rows (**`MENU_ROW_RADIUS` 8** = plate 14 − padding 6, was 9), the header's tab dropdown rows, mention / forward / skills / tasks rows | `highlight::row(el, selected, Shape::Own, theme)` |
| Fills | washes of `text` so one number reads the same on the chat, sidebar, card, frosted plate and Glass. Matte dark **0.055 / 0.09 / 0.13** (hover / selected / pressed), Matte light **0.045 / 0.08 / 0.12**, Glass dark **0.08 / 0.12 / 0.16**, Glass light **0.06 / 0.10 / 0.14**. A selected row keeps its step under the pointer; pressed is the third step for every row | `highlight::alphas`, `highlight::fills` |
| Transparent buttons | an icon disc, a text button, a tab segment, a small ×: the same hover / pressed washes on its own rounded shape | `highlight::button`, `button_with` (extra hover styling), `segment` (the on segment is a filled control) |
| Filled controls | the fill at rest, the wash composited over it under the pointer and while down — `surface_2` discs and pills, `field` pills, the glass circle, the blue update disc. Dark: the old chip rule (a shade lighter); light: a shade darker (lifting `#E8E8E8` toward white vanished on the `#F6F6F6` sidebar). `DISC_HOVER_LIFT` / `DISC_PRESSED_LIFT` are gone | `highlight::washed(el, base, theme)`, `highlight::over` |
| Exceptions | a line that must set its own fill says why: `// hover-fill: exception <kind>` on it or the line above — `handle` (a split handle's hover hairline, `ui/split_handle.rs`), `overlay` (white / black washes over a photo or video), `code-card` (the copy pill's four states), `danger` (a Delete label's `danger_soft`), `card-option` (the approval card's own palette), `choice` (a chosen chip / segment in a form, not a list selection), `removed-by-sibling`. The "Add agent" / "Add your agent" rows and the add-agent popover are untouched while [feedback add-agent-row-removal] removes them | `highlight::tests::EXCEPTION_KINDS` |
| Focus | the keyboard focus ring is not part of this token and did not change | — |

Pinned by `highlight::tests::no_hover_or_pressed_fill_outside_the_token` (scans `src/ui` for
`.hover(|s| s.bg(`, `.active(…bg(`, `group_hover(…bg(`, `.when(selected | is_selected | hovered |
open | i == selected | menu_here, |el| el.bg(`, `.bg(if selected / is_selected / is_active / hovered
/ open / menu_here` — an unmarked one, or a marker with an unknown kind, fails the Rust gate),
`the_nested_pill_sits_inset_from_every_edge_of_the_row` (headless render: the layer is 4 from the
row's box on every side and never flush with the card), `the_pill_is_concentric_with_its_card_and_never_sharper_than_the_floor`,
`selected_is_one_step_over_hover_and_pressed_one_more_on_every_surface`,
`the_fills_are_washes_of_the_text_colour`. iPhone: the tap highlight is checked separately (see the
feedback report); nothing there used a full-bleed fill outside SwiftUI's own list row highlight.

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
| Chat column | a flex sibling, so its slot follows every frame: the header pill and the composer re-centre live. The **transcript keeps the width it had when the slide began** and is clipped/offset — centred when the rows are capped at both widths (`COLUMN_MAX_WIDTH`: then nothing moves at the end), else, for the right pane, pinned to the edge away from it — and is laid out once more at the end. (Grok re-lays its list out every frame; its list is short rows, ours is a markdown transcript whose `list()` drops every item height on a width change) |
| Chat column, sidebar moving | **the bubbles move with the sidebar, at any width** ([feedback mac-sidebar-slide-jank-2026-10-29], Scout on v0.6.350: "the text bubbles teleport … chat messages lag behind"). The held transcript is kept only while its rows are capped at every width the column passes (start, this frame, and the narrowest the move can end at — a drag from the rail may still settle at the full width): it glides centred, exactly where the live layout would put it. Where the rows fill the column (the default 1180 pt window: 884 pt expanded) the transcript is laid out at the live width every frame, as under a width drag — a held layout there sat still while the sidebar moved and jumped the whole distance when it settled. Pinned by `slide::layout_tests::every_frame_of_a_sidebar_slide_lays_the_rows_out_at_the_live_width` |
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
| Where (takes space, never absolute over controls) | **Sheets** (avatar picker, Create sheets, New Message / group / Add members, server sheets, feedback): the top of the body under the bar / tab pill, before the first control; the sheet grows by the note. **Pane pages** (Secrets, Devices, Updates, connectors, routines): under the pane header, before the first card / field. **Composer** failures (send refused, offline, attachment, recording): directly above the composer, the composer's width; the failed bubble keeps its red mark (the word "Retry" moved into the note). **Chat-level** ("Messages in this chat aren't being delivered right now.", "Can't reach your machine."): the slot under the header where the machine banner lives, the column's width. **Toasts** (download / save / forward / connect failures with no surface up): **the one toast slot** — see the row below (Jacob 2026-10-27: no longer the top-right stack). **Sign-in**: under the button row and the hint, 420 wide. Field validation ("Pick someone…", "Give the routine a name.") is the same element with the words as a refusal |
| The toast slot (Jacob 2026-10-27, [feedback toast-stack-mac-2026-10-27]; iPhone [feedback toast-stack-ios-2026-10-27]) | **One fixed slot just below the current chat's header profile-picture pill, top centre of the chat column** (`header::metrics::TOAST_SLOT_TOP` = pill bottom + the connection pill's band + 8, centred between the sidebar and the pane; 380 wide), the same on the iPhone (under the chat header pill; the root slot under the status bar only when no chat is up). **Stacked compactly, newest at the bottom, at most 3 visible**; older ones fold into one "N more notices" line above the stack whose × clears the slot. **Identical failures never stack**: the same sentence again while its toast stands bumps a small **×N** after the sentence and restarts the clock — one note per failure, however often it repeats (eight "Couldn't load your secrets. You're offline." toasts down the right side on v0.6.296 are the reason). 10 s (info 6 s), the pointer on a toast holds it for at most 60 s unless Details is open, × dismisses, Try again re-issues the command. **Background polls never toast**: a 15 s refresh that fails (secrets, registry) backs off (30 s → 1 → 2 → 4 → 5 min, one fetch in flight at a time) and says nothing — the connection pill is the one place a link fault shows; the pane's own note speaks when a person asked. |
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
`a_failure_event_is_an_error_toast_with_details_try_again_and_close`,
`the_same_failure_eight_times_is_one_toast_and_the_slot_shows_at_most_three`,
`a_held_toast_is_let_go_after_the_hold_cap_unless_details_are_open`; `AppState::notice_stack`,
`NOTICE_VISIBLE_MAX`, `NOTICE_HOLD_MAX_MS`, `relay::secrets_poll_skip_after`). Rig:
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

## Composer: staged attachments (Jacob 2026-10-27, [feedback composer-attachment-removal-2026-10-27]; iPhone [feedback composer-attachment-removal-ios-2026-10-27])

**Every pending attachment in the composer can be removed, with the mouse or
the keyboard, and removing one never touches the others or the typed text.**
With files pending the pill opens into the card (redesign mock e, 2026-10-20):
the strip above the text, "+" bottom-left, mic / send bottom-right.

| Item | Value |
|---|---|
| Strip | one horizontal row (`Composer::staged_row`, selector `composer-staged-strip`), tiles **8** apart (`STAGED_GAP`), 12 in from the card's edges; it **scrolls sideways** past the card's width, never wraps into a second row |
| Tile | **64** tall (`STAGED_TILE`), radius **10** (`STAGED_RADIUS`), fill `surface_2`, a **1 pt `border_strong` hairline drawn as an overlay ring** — one rounded rect per attachment, no shadow, nothing outside it. Pictures are 64 × 64 (thumbnail decoded at ≤ 256 px, `ObjectFit::Cover`); other files are the icon tile · name · size chip, at most 320 wide |
| Remove disc | **always drawn** (trackpad and keyboard users — never hover-only), a **20 pt** circle (`STAGED_REMOVE`, the hit target) **inset 4** from the tile's top and right (`STAGED_REMOVE_INSET`), black 62 % with a white ✕ and a faint white ring so it reads on dark and light pictures alike; it sits fully inside its tile, so it never overlaps the neighbour. Role `Button`, accessible label and tooltip **"Remove attachment"**; selector `composer-remove-attachment-<i>` |
| Click | removes that attachment only; the others, the typed text and the caret stay |
| ⌫ / ⌦ | with an **empty draft** the field has nothing to delete and lets the key through; the composer removes the **last** staged attachment. With text in the draft the keys edit text only — even with the caret at the start, where ⌫ is a no-op — and never the strip. The rule lives on the composer (`delete_key_on_strip`), not in the field's key routing |
| Empty strip | the card folds back to the 44 pt pill |

Code: `composer::staged_tile`, `remove_attachment`, `delete_key_on_strip`;
`ComposerInput::backspace` / `delete` propagate when there is nothing to
delete. Pinned by `composer::staged_tests::{removing_one_attachment_keeps_the_others_and_the_text,
backspace_on_an_empty_draft_removes_the_last_attachment, the_strip_lays_the_tiles_out_as_pinned}`.
iPhone: the chip's ✕ has a 24 pt hit frame and the same label; ⌫ on an empty
field removes the last pending attachment (`MentionUITextView.deleteBackward`);
pinned by `ComposerMetricsTests` (pending attachments).

### iPhone: a confirm is a centred alert (Jacob 2026-10-27, [feedback confirm-alerts-ios-2026-10-27])

**Where the iPhone does ask first — Block / Unblock, Leave / Delete a group, server
or channel, Remove / Ban a member, Delete / Revoke / Scrub a secret, Disconnect a
vault, Sign out, Reset an invite link — the question is a centred alert: title, a
one-line note, Cancel · action (red when it removes something), the Mac's two-button
confirm. Never a sheet or dialog anchored at the screen.** On iOS 26 a
`.confirmationDialog` presents anchored to the view it is attached to; every one of
ours hung off a screen's root, so the Unblock question floated in the middle of the
transcript over someone's messages with no Cancel (`media/feedback/blocked-state/
after-iphone17-sim-6231fdd8c-2-composer-unblock-confirm-floats-3x.png`). A
`.confirmationDialog` stays only where it is a *menu on its own control* — the
mute-duration list on its row, a tapped member's actions on their row, a connector's
Disconnect on its row — and says so with `// confirm: anchored`.

Code: `UI/Confirm.swift` — `ConfirmSpec` (title · message · action · destructive ·
cancel) and `View.confirm(_:isPresented:perform:)` / `confirm(item:spec:perform:)`
over `.alert`; `BlockedState.unblockConfirm`, `BlockRow.blockConfirm`. Pinned by
`ConfirmFormTests`: the spec's shape, no `.confirmationDialog(` outside the helper
without the marker (count held at 3), no hand-built destructive-plus-Cancel `.alert(`.

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
| Add agent | the same 30 capsule on `surface_2` with `plus` 13 + "Add agent" 13 semibold — the chip hover/pressed are the highlight token's steps over the fill (`highlight::washed`); held at the pressed step while open | 34 capsule on `bubbleAssistant`, `plus` + "Add agent" 15 semibold |
| Empty | the button alone + "No agent can use it yet." (`TEXT_TINY` muted) | the same |
| Picking | a dropdown **6** under the chips row, left-aligned, **308** wide (the pane's content width), `hoverpick`'s frame (`card()` fill, `border`, radius 10, shadow; flips above the row when the window has no room below): search row 34 (magnifier 13, `ComposerInput` at 13 pt, "Search agents"), rows 36 (picture 20, name 13, `@handle` 12.5 muted), the highlighted row on the token's inset pill (radius 6 in the radius-10 frame) with ↩, a 22 pt footer "↑↓ move · ↩ add · esc close"; ≤ 6 rows before it scrolls | a stacked sheet "Add agent" (Cancel): a search field of the control family (`.controlField()`: `Controls.height` **48**, the composer's rim — the iPhone control-size token, not the secrets forms' old 42) focused with the keyboard up, "YOUR AGENTS" `PaneCard` of 48 pt `PaneRow`s (36 pt picture, name, `@handle`, a + disc) |
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

## Link previews: one card under a link, the code card's frame (Jacob 2026-10-27, approved mock; relay [feedback link-embeds-relay-2026-10-27], Mac [feedback link-embeds-mac-2026-10-27], iPhone [feedback link-embeds-ios-2026-10-27])

**The relay unfurls, the apps draw.** A finished message with a bare `http(s)://` URL (not a
markdown-labelled link, not inside code) gets one `link_preview` part from the relay — fetched
once with the relay's address, never a viewer's — and the apps draw it as the code / chart card's
sibling inside the bubble: `code_bg`, hairline, radius 10, 10 pt padding, at most 400 pt wide.
Two densities, chosen by the relay (`layout`): **compact** = 56 pt art left, title (13 semibold,
one line) · subtitle (12 muted) · footer (11 faint: tile slot · provider · facts); **expanded** =
the picture on top (225 pt tall at full width, cover-cropped), ▶ and the duration chip on a
video, title (two lines) · description (two lines) · footer under it. X / Bluesky posts: a
32 pt initials disc, name · @handle, the post text (eight lines), the date. GitHub: repo · #n
over the title, a state pill (Open green · Merged purple · Draft muted · Closed red), the diff
(`+612` in `online`, `−38` in `danger`) in the footer.

**One card per message; the card is never the only copy of the link.** Text + URL keeps the
text exactly as typed and puts the card under it; a message that was only the URL shows the card
alone with a 6 pt inset (iMessage's rule) — the full URL stays one hover away on the card and
`Copy` copies the text. No part → today's plain link, silently: no "preview unavailable", no
error text, nothing fetched by the app.

**Every card opens out.** A click anywhere on the card is the link's click
(`AppState::open_link`, the one seam every chat link goes through, where #1522's confirm slots
in): own bubbles open directly, others' ask. Nothing plays inside a bubble in v1.

**Remove preview, for everyone.** On a message the user may delete (#1526's rule, shared with
the bubble menu), the card shows an × top-right on hover and the ⋯ menu gains **Remove
preview** above Delete; both write `preview: "off"` on the row through the relay
(`DELETE …/messages/{id}/preview`) and the card goes for every member. Viewers have one global
switch — Settings → Appearance → **Link previews** On · Off (this Mac) — and no per-message
collapse.

**The site-icon tile.** The footer's small square is the shared link-icon tile
(`ui::link_tile`, 14 pt): the host's bundled brand mark, else the relay's `siteIcon` picture on
the part, else the relay's host icon, else the host's initial. The card itself fetches nothing
from the site.
## One add row per info pane: "Add members" (Jacob 2026-10-27, [feedback add-your-agent-row-removal-mac-2026-10-27], iPhone [feedback add-your-agent-row-removal-ios-2026-10-27])

**Every info pane that adds members has exactly ONE add row — "Add members" —
and its picker adds people and agents. No "Add agent", no "Add your agent", no
sparkle promo row anywhere: not in the group pane, not in the server pane, not on
a person's pane opened from a chat, not in an empty chat.** Jacob's shots
(Mac, 2026-10-27): the group's Members card drew "Add members" and under it
"Add agent · One of yours, so everyone here can ask it"; a DM's info pane drew
"Add your agent · Bring one of yours into this chat". Cut, not relocated
(product principle 2026-10-20 / 2026-10-22). Candidates for a return, as
candidates only: adding an agent to a 1:1 DM from the Mac now has no path (the
person pane never had Add members — the iPhone never had one either).

| Item | Mac | iPhone |
|---|---|---|
| Group / shared project, Members tab | the first card is "Add members" (`profile-add-members`, `person-plus` glyph) alone; a people group's description follows it in the same card as a labelled row (`profile-description`, `text-align-left` glyph, "Description", the text right-aligned in `TEXT_SMALL` muted, ≤ 180 wide, truncated) — it used to be a bare text card above the add rows, which with "AR" alone read as a stray block; the Mac has no editor for it (the iPhone's Edit group sheet is the one writer) | "Add members" alone, then the Description row (tap → Edit group), then Share invite link / Activity |
| Server, Members tab | "Add members" (`server-add-members`) alone, admins only; people only | the same |
| Person pane (from a DM or a group) | Links \| Groups, the Message pill, Block — no add row | the same |
| Empty people chat | picture, name, hint, composer — no "Add agent" control | the same |
| Add members picker | title "Add members to <group>"; chips; search; **Agents** section first — my agents (pickable; a greyed check when already in), then the other members' agents already in the chat (greyed, `@handle · run by @owner`, not pickable: only their owner adds or removes them) — then **People** (`add-members-agents-section`, `group-agent-<id>`, `group-person-<id>`); servers' and secrets' pickers list people only | "AGENTS" then "PEOPLE" in the same order (`NewGroupView`) |
| Dead code gone | `Shell.add_agent_open`, `people::add_agent_picker`, `ClientCommand::AddAgent` (the picker's batch `AddMembers` carries agents; the `ARBOS_RIG_ADD_AGENT` recording hook goes through it), `groups::project_color` | — |

Pinned by `profiles::group::render_tests` (the real Shell, headless: every kind's
pane — people group, shared project, server, person from a DM and from a group —
draws one Add members row and neither `profile-add-agent` nor `empty-add-agent`;
the Description row sits under Add members in the same card and vanishes when
blank; Add members opens the picker aimed at the group with the Agents section
above my agents, the others' agents after mine, people below) and
`profiles::group::tests` (which description makes a row).

## Copy copies the message, pictures included (Jacob 2026-10-27, [feedback copy-image-messages-mac-2026-10-27], iPhone [feedback copy-image-messages-ios-2026-10-27])

Copy — the bubble menu's Copy, control-click, ⌘C with a whole bubble selected, the media
viewer's Copy pill / ⌘C, the Library row's Copy — puts **the message** on the pasteboard,
read through the one accessor Forward uses too (`Message::body()` / `Message.body`):

- every picture is one pasteboard item carrying its **original bytes under its own
  type** (PNG `public.png`, JPEG `public.jpeg`, GIF `com.compuserve.gif`, WebP, HEIC),
  fetched from the relay as uploaded — never the thumbnail — plus a `public.png`
  re-encode when the native type is not PNG and (Mac) a `public.tiff` representation, all
  on the same item, so Preview, Notes, Messages, Slack and Finder each take the one they
  read and a GIF stays a GIF;
- the message's words ride on the **first** item as `public.utf8-plain-text`: a text
  field receives the text, an image well the picture — one rich item, never two;
- several pictures → several items in message order; a file that is not a picture → a
  `public.file-url` item (Mac, staged under the temp dir) / its own data type (iPhone);
- the Arbos composer reads every item back (⌘V / Paste): the pictures become attachment
  chips, the words land at the caret;
- a message whose picture is still uploading says "Still uploading — try again in a
  moment"; a picture the relay would not give back says so in the notice and the rest
  still copies.

Mac: `pasteboard.rs` (`plan` → `write`/`read`, NSPasteboard on macOS, gpui's clipboard
elsewhere), `AppState::copy_message` / `copy_attachment`, `ClientCommand::FetchCopyBlobs`.
iPhone: `MessagePasteboard` (`items` → `UIPasteboard.setItems`), `AppModel.copyMessage` /
`copyAttachment`, the composer's `paste` override. Pinned by `pasteboard::tests` and
`MessagePasteboardTests` (text · picture only · mixed · several · GIF kept · file ·
round trip).
