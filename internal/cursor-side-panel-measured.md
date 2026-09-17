---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# Cursor's side panel, measured — for [Design side panels for desktop](bc-32dc7892-de92-5f0e-801c-05628fa6bde4)

Live Cursor Agents (stable Linux AppImage, 2026-09-17 12:17–12:32 UTC) at 1x, window 1440×900, sidebar open (256 px). Screen pixels are CSS pixels here; on Jacob's Retina Mac every number below is drawn at 2x. Every number was read off a still with a pixel scan, not estimated; the stills are in `media/cursor-reference/side-panel/` (named `cursor-sp-NN-*.png`, numbers below refer to them). Where I could not make Cursor show a state, I say so.

## What the panel is

A second tab set on the right of the chat, opened by the **⊟ toggle at the top-right of the chat header** (also the panel's own top-right icon). Its tabs are *kinds of thing*, one per kind mostly: Project, Browser, Files, Changes, Desktop, Subscriptions, Context, Terminal. It is **per project**: switching to another project (Ctrl+Tab / sidebar) shows that project's own panel state.

## The tab row (`sp-13`, `sp-20`)

| measure | value |
| --- | --- |
| row height | **40 px** (y 152–191 in the window), 1 px bottom border `#252525`, row fill `#141414` (the panel's fill) |
| active tab | a **pill**: fill `#202020`, height **26 px**, top/bottom inset 6 px, horizontal padding ≈ 8 px, radius ≈ 6 px. Icon 14 px + 6 px gap + label. No underline, no accent colour |
| inactive tab | icon + label, no fill; label `#969696` (150,150,150); active label `#F0F0F0` |
| type | **13 px**, regular weight (cap/ascender rows 165–175: 11 px), same face as the chat |
| spacing | ≈ 16 px from one tab's label end to the next tab's icon; the first pill sits 6 px from the panel's left edge |
| after the tabs | a `+` (14 px), 16 px after the last tab |
| right of the row | three icons, right-aligned: **plug** (Ports — a popover "Port number / No Ports Detected"), **⤢ expand**, **⊟ close panel** |
| hover on a tab | a faint pill and an **×** that appears *over the label's last letters* — the tab does not widen (`sp-03-crop`: "Proje×") |
| close | × on hover, one click. Closing the **last** tab closes the panel and **empties its set** (`sp-05`) |

**New tab placement:** a new tab opens **immediately to the right of the active tab**, not at the end (Files opened between Browser and Changes, `sp-15`; Desktop between Files and Changes, `sp-18`).

**Duplicates:** the chord for a kind that is already open **focuses the existing tab** (Ctrl+Shift+B twice → one Browser tab, `sp-14`). The `+` menu drops kinds that are open and singular (Changes, Desktop disappeared from it once open, `sp-19`) but keeps Project/Browser/File/Terminal listed even when open — I did not confirm whether the menu path can open a second Browser; the chord path cannot.

**Overflow (`sp-20`, `sp-22`, `sp-24`):** no ellipsis, no overflow menu, no shrinking. The row **scrolls horizontally**: tabs are clipped hard at both edges ("Cha" / "oject"), the active tab is scrolled into view when it changes, and the **mouse wheel over the row scrolls it**. The `+` and the three right icons never scroll.

**Reorder:** **yes, by drag** — Context dragged from the end to before Desktop (`sp-26`). No drop indicator I could capture at 1x; the tab moves with the pointer.

## The `+` menu (`sp-09`, `sp-17`)

A popover under the `+`: a search field *"Open any file, URL, …"* then rows with the chords beside them:

```
Project
File            Ctrl+G      (⌘G on the Mac)
Terminal        Ctrl+J      (⌘J)
Browser         Ctrl+Shift+B (⌘⇧B)
Changes         Ctrl+E      (⌘E)
Desktop
Subscriptions
Context
```

The search field takes a file path or a URL and opens the matching tab kind — one entry point for "open anything in the panel".

## Shortcuts (`sp-32` … `sp-41`) — the risk you named

- **Opening** a panel tab is by **kind-specific chords** (above), which work with focus anywhere in the window — from the chat's composer they opened panel tabs. There is **no generic "new panel tab" chord**; ⌘T is not used by the panel.
- **Cycling** uses the **same chord as the project strip** — Ctrl+Shift+] / [ (⌘⇧] / [ on the Mac) — and **which set moves follows focus**: focus in the chat → the project strip cycles (`sp-40`); focus inside the panel → the panel's tabs cycle (`sp-41`). With **one** tab in the strip the chord falls through to the panel even from the chat (`sp-32`).
- **Ctrl+Tab** is neither: it switches **chats/projects** (the sidebar's list), and the panel of the project you left is not shown until you return (`sp-34`).
- **What signals which set will act: nothing but focus.** The active pill looks identical whether or not its set has focus; there is no focus ring on the panel or the chat. This is the weakest part of Cursor's design and the place I would not copy — say it in ours (a hairline or a brighter active pill on the focused set), or give the panel its own cycling chord.
- **Panel closed:** the kind chords **reopen the panel** with that tab; the cycling chord goes to the strip.

## Width and layout (`sp-28`, `sp-29`)

| measure | value |
| --- | --- |
| default width | **592 px** of the 1184 px right of the sidebar — the panel takes **half** of the space beside the sidebar; the chat keeps 591 |
| divider | 1 px `#252525`; the drag handle is **inside the panel's edge** (a press 1–3 px right of the line drags; on the line or left of it does not) |
| maximum | **766 px** — the chat stops at **418 px** and the panel cannot grow further (a drag to 700 clamped at 874) |
| minimum | **377 px** (a drag to 1600 clamped at 1263); at that width the tab row clips to two visible tabs |
| remembered | **yes** — closing with ⊟ and reopening restored the width (1263) **and all seven tabs with the active one** (`sp-31`). Only closing the last tab forgets the set |
| the chat below the minimum | never: the divider clamps, the chat is never squeezed under 418 |

**Expand (⤢, `sp-43`, `sp-45`):** the panel takes the whole content area; the chat collapses; the panel's tabs **move into the top strip** after the project tab (`Subtensor · Project · Browser +`), the composer floats at the bottom over the panel content, ⤢ becomes ⤡. Escape does **not** collapse it; the ⤡ icon does. So Cursor has a *merged* single-strip mode as well as the split.

## Empty state and a single tab (`sp-05`, `sp-07`, `sp-08`)

- **Empty** (panel open, no tabs): the tab row holds only `+` and the three icons; the body is a centred **2×2 grid of tiles** — Project, Browser, Terminal, File — each an icon over a label, ~110×80 px, 16 px apart, placed at about 60 % of the panel's height (low, near the composer's line), not centred vertically.
- **One tab**: the pill alone with `+` after it. The Project tab's body: the project's icon and name as a heading (`Cold pair`, 15 px semibold) with a note icon at the right, then the page's own empty state (*Nothing to Track Yet — Give your project an assignment and track its progress here*).
- Kind-specific empty states: Browser has a URL field and Recents (`sp-14`); Files has a search field, *New File*, Recents, and a tree at the right (`sp-15`); Changes has *All Commits ▾ main ▾* (`sp-13`); Context *No Context Yet*.

## Where Cursor contradicts our design, and what I would not copy

1. **Closing the last tab throws the set away**, while closing with ⊟ keeps it. Two different results for "the panel went away" — I would keep the set in both cases (ours: keep).
2. **New tabs open beside the active one**, not at the end. Fine to copy; say it, since the strip's `+` in our design appended.
3. **Overflow is hard clipping plus wheel-scroll**, no ellipsis, no menu. Cheap to build and honest, but a clipped "Cha" is not a label; if we copy the scroll, fade the edges.
4. **Which tab set a chord acts on is invisible.** Focus decides, and nothing shows focus. Copy the follow-focus rule if Jacob wants the same chords, but mark the focused set — this is the one place I judge Cursor worse than ours.
5. **The expand mode merges the two tab sets into one strip.** Elegant for reading a file large, but it means the strip's tabs change meaning depending on a toggle. Worth a decision rather than a default.
6. **The `+` menu doubles as an open-anything search** ("Open any file, URL, …"). Ours planned a menu; the field is the better half of theirs.
7. **The drag handle is inside the panel, not on the divider line** — a person aiming at the line misses. Ours should take both.

## What I could not measure here

Retina metrics (the Mac draws all of this at 2x); the Terminal tab (Ctrl+J did nothing in a Start-from-scratch cloud project); tab **drag between the two sets**; whether a second Browser can be opened from the `+` menu; keyboard focus movement *into* the panel without the mouse (I found no chord that moved focus there; Tab from the composer did not).
