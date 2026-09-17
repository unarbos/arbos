---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# Cursor's Settings interior, measured — for [Make settings an inline tab](bc-6d9c3785-7bed-5cb3-9eb7-bca86aad4ee5)

Measured 2026-09-17 18:00 UTC on the rig's live Cursor (Linux build, the
Agents window, signed in), display `:1` at 1x, window 1440×900 and then
1900×1000. Numbers are pixels read from the stills with a script, not
eyeballed; where 1x cannot separate two values (12 px from 13 px type) it
says so. Stills:

- `media/cursor-reference/cycle-35/cursor-settings/general-1440x900.png`
- `media/cursor-reference/cycle-35/cursor-settings/general-1900x1000.png`
- `media/cursor-reference/cycle-35/cursor-settings/rows-2x-crop.png` (the Startup group at 2x, for the eye)

The Linux window carries a 28 px menu bar (File Edit View Help) at the top;
every y below is measured from the window's top including it, so subtract
28 for content-relative values. macOS has no such bar.

## One thing to say first

The coordinator's brief said "rows rather than cards … hairlines between
rows inside a group and nothing around the group". **That is not what
Cursor draws today.** Each group of rows sits on a slightly lighter
rounded surface — `#1C1C1C` on the page's `#141414`, radius ≈ 6 px, no
border — with 1 px hairlines between the rows inside it. It reads as rows
because the lift is only 8 levels of grey; but it is a surface, and the
hairlines stop 12 px short of its edges. Copy the lift or not, but know it
is there.

## The page

| what | value |
| --- | --- |
| page background | `#141414` (20,20,20) |
| rail (left) width | **255 px**, background `#181818` (24,24,24); no border between rail and page — the two greys meet |
| content column width | **669 px** (probably 672 with its 1 px rounding), the same at 1440 and 1900 wide |
| column placement | **centred in the area right of the rail**, at both widths: at 1440 the column runs x 514–1182 (area 255–1439, centre 847, column centre 848); at 1900 it runs 744–1412 (area centre 1077, column centre 1078). So Cursor *does* recentre as the window widens — our centring matches it. What it does not do is grow: the column is a fixed max width |
| column offset from the window's left edge | 514 px at 1440; 259 px from the rail's right edge |
| top of content | a banner card at y 104–141 (the "moved to Customize" notice, dismissible), then the section heading |

## Section heading and group captions

| element | text | size | weight | colour | rhythm |
| --- | --- | --- | --- | --- | --- |
| section heading | "General" | cap height 12 px → **≈ 16–17 px** | semibold | `#F0F0F0` (240) | 23 px below the banner; **25 px** from its baseline row to the first card's top |
| group caption | "Startup", "Notifications", "Privacy" | cap height 9 px → **12–13 px** (1x cannot tell) | regular | `#B2B2B2` (178) — muted, the same grey as the descriptions | **29 px** from the previous card's bottom to the caption's top; **10 px** from the caption's bottom to its card's top |
| caption / heading left edge | x 522 — **8 px inside the card's left edge** (514), i.e. 4 px left of the row text (526). It aligns with nothing else; take it as "flush with the card, minus the row's inner inset" |

## Rows

| what | value |
| --- | --- |
| row height | **60 px** including its 1 px hairline (a single-row card is 60 tall: y 201–261) |
| row inner inset, left | text starts at x 526 = **12 px** from the card edge |
| row inner inset, right | controls end at x 1170–1171 = **11–12 px** from the card edge (1182) |
| padding above the label | 15–16 px to the label's cap top (14 px on rows whose label has no ascender overshoot) |
| padding below the description | 13–14 px from the description's descender line to the row's bottom |
| label → description | label cap top 328, description cap top 347: **19 px line pitch** (≈ 7 px from the label's baseline to the description's cap top) |
| label | cap height 9 px → 12–13 px, **medium** weight, `#F0F0F0` (240,240,240) |
| description | cap height 9 px → **the same size as the label** (the difference Jacob sees is colour and weight, not size — or one pixel of size the rig cannot resolve at 1x), regular, `#B4B4B4` (180,180,180) |
| hairline between rows | **1 px**, `#232323` (35,35,35) on the card's `#1C1C1C`; runs x 526–1170, i.e. **inset 12 px** from both card edges, matching the text inset — not full-bleed |
| between groups | no hairline; the card ends, 29 px of page, the next caption |
| hover | rows do not highlight on hover — checked with the pointer parked on the Tips row: the card stays `#1C1C1C` (`general-1440x900.png` vs the parked still); only controls react |

## Controls, hard right

| control | box | vertical placement | colours |
| --- | --- | --- | --- |
| toggle | **32 × 20**, right edge 11 px inside the card | **centred on the row** (toggle centre y 341.5, row centre 342) — not on the label's baseline, not on the description's | on: track `#3FA266` (63,162,102) green, white knob right; off: track `#3A3A3A` (58,58,58), white knob left |
| dropdown value ("Default ⌄") | **70 × 26**, 1 px border `#363636` (54,54,54), fill = card colour, radius ≈ 4 px | centred on the row (401.5 vs 402.5) | text `#F0F0F0`, cap 9 px (12–13 px), chevron 8 px muted, 8 px right padding |
| button ("Open ↗") | **62 × 22**, same border and fill as the dropdown | centred on the row | text `#F0F0F0` |
| the two questions asked | **A value control is a boxed control, right-aligned, vertically centred between the label and the description — never baseline-aligned to either.** I did not find a bare right-aligned text value (no box) in General; every "value" Cursor shows there is a dropdown or a button. If you draw a bare value, centre it on the row like the others rather than inventing a baseline rule Cursor does not have. **The column recentres** (see above) — theirs and ours agree; keep ours centred |

## The rail

| what | value |
| --- | --- |
| width | 255 px |
| "← Back" row | y 60 (cap), the first thing in the rail, 16 px type, muted |
| search field | y 90–115 (**≈ 26 px** tall), x 8–246, fill `#3B3B3B` (59,59,59) with a 1 px edge one step darker, placeholder "Search Settings" 12–13 px muted, magnifier icon 12 px |
| item height | **31 px pitch** (label cap tops at 144, 174/175, 205 …); the active item's highlight is **29 px** tall (y 134–162), `#242424` (36,36,36), radius ≈ 6 px, running x 8–246 (8 px inset each side) |
| item label | 12–13 px, regular; **active `#F0F0F0`**, inactive `#B3B3B3` (179); icon 12 px at x 14, same colour as its label |
| band separation | groups (General/Profile/Appearance · Plan & Usage … Worktrees · Browser & Network … Beta · Docs) are separated by **an extra 15 px**: 205 → 251 is 46 = 31 + 15. No rule between bands |
| account row | pinned to the rail's foot: avatar 20 px, name 12–13 px, a gear at the right (x 235); y 883 |

## Where Cursor contradicts what we wrote, and where it is worse

- **Surface, not rows.** Said above. Our design's "nothing around the group" is cleaner than what Cursor draws; if Jacob asked for Cursor's, this is the one place the words and the picture differ, and it is his call.
- **Caption inset.** Cursor's captions sit 4 px left of the row text. It looks like a bug (or a `padding: 0 8px` on the caption against `12px` on the rows). Align ours to the text at 12 px; nobody will miss the 4 px.
- **Same type size for label and description** (at 1x). It works because the greys are 60 levels apart. If ours uses a smaller description, it is a deliberate departure, not a match.
- **Toggle and value controls centred on the row.** This is the right call and cheap to copy; a control on the label's baseline reads wrong the moment the description wraps to two lines.
- **The dismissible banner at the top** pushes the section heading down 60 px and is not part of the pattern; ignore it.

## What the rig cannot see

1x on an Xvfb display: no sub-pixel weights, so "medium" vs "regular" is read from stroke width, and 12 px vs 13 px type is not separable (cap heights both round to 9). The Mac at 2x would settle both; the numbers above are the safe reading at 1x.
