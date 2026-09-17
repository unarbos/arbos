---
cursor:
  subagentId: "bc-6d9c3785-7bed-5cb3-9eb7-bca86aad4ee5"
---

**For the desktop parity loop**, from the settings-surface worker. An ask, not a
blocker: I am building to estimates now and will fold your numbers in when they
arrive.

# Measurements wanted: Cursor's Settings interior

Jacob sent a screenshot of Cursor's own Settings with "settings should feel more
like this". The tab shell we shipped in
[#451](https://github.com/unarbos/arbos/pull/451) is right; the interior is
wrong in three ways — a centred reading column with a logo and tagline,
information in bordered cards, and values inside the cards rather than hard
right. I am reshaping it into Cursor's rows.

You have live Cursor. I have a description of a screenshot. Six numbers would
turn my estimates into measurements.

## What I need, from Cursor's Settings (any section; General is the reference)

1. **Row height** for a two-line row (label plus one-line description), and for
   a one-line row. Padding above and below the text, if you can separate it from
   the line boxes.
2. **The description line**: size, weight, colour (hex or the token name), and
   its gap under the label.
3. **The group caption** — the small muted words that head a run of rows
   (`Startup`, `Notifications`, `Privacy`): size, weight, letter case, colour,
   the space above it and below it.
4. **Hairlines**: colour and opacity, whether they run the full width of the
   content column or inset to the label, and whether the last row in a group
   carries one.
5. **The content column**: its width, its left offset from the rail, and how it
   behaves when the window is much wider — does it stay put, or move? (From the
   screenshot it looks left-aligned and left of centre; whether it is a fixed
   left gutter or a max-width that never centres is the thing I cannot tell.)
6. **The rail**: width, row height, icon size, and the gap between bands.

## Two smaller ones, if they are cheap

7. Does the **right-hand control** column line up at a fixed right edge, or does
   each control simply sit at the end of its row?
8. Is the **section heading** (`General`) the same size as a page title
   elsewhere in Cursor, and how much space sits under it before the first
   caption?

## What I am building against until then

Provisional, all in the one place — `desktop/src/view/settings/mod.rs`, as
`row`, `caption` and `group` used by every section, so a number arriving changes
one file rather than five:

| what | my estimate |
| --- | --- |
| row padding | 10px top and bottom, hairline between rows, none after the last |
| description | Caption, `text_muted`, 2px under the label |
| caption | Caption, `text_faint`, 20px above, 8px below |
| content column | 640px max, left-aligned, 32px in from the rail, never centred |
| rail | 200px, unchanged from the tab work |

If any of those is visibly wrong in Cursor, say so even without a number —
"the description is dimmer than that" is worth more than nothing.

## Two decisions I am making rather than copying, for the record

- **No bands in the rail yet.** Cursor's rail has about thirteen items in four
  bands. We have five sections, which reads as one band already; bands would be
  space between arbitrary splits. When a sixth and seventh arrive, they split
  naturally.
- **No search field yet.** In Cursor it filters a long list. Over five sections
  it is decoration, and a field that searches section names while a person types
  a setting's name is worse than no field. It earns its place when it can search
  row labels rather than section names, or when the sections outgrow one screen.

Reply here or in your own note; I will read it either way.
