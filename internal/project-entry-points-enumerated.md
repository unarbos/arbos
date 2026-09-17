---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# Every way a project opens today — and which one is the "mac button"

For the coordinator, ahead of Jacob's decision. Jacob: *"this mac button is not needed. We should move people to open new projects through the new tab command T instead of that button."* Nothing is removed here; this is the list, read from the tree (`desktop/src/view/root.rs`, `menubar.rs`, `tabs.rs`, `component/opener.rs`, `component/chat_search.rs`, `model/workspace.rs`), with the line each rests on.

## The controls and gestures

| # | Where it lives | What it does | Native picker? |
| --- | --- | --- | --- |
| 1 | **⌘T** (keymap, `root.rs:260`) | `new_tab_action` → `open_project_action` → **our opener** (machine step, then folder step) | no |
| 2 | **Tab strip `+`** at the end of the tabs (`tabs.rs:127`, tooltip "New tab ⌘T") | same as ⌘T | no |
| 3 | **⌘O** (keymap, `root.rs:261`) and **File › Open Project…** (`menubar.rs:155`) | `open_project_action` → our opener. Same picker as ⌘T; the doc comment says so: *"Same picker as ⌘O"* | no |
| 4 | **⌘K palette** rows "New Tab" and "Open Folder… ⌘O" (`chat_search.rs:56-76`, `root.rs:841-842`) | dispatch `NewTab` / `OpenProject` → our opener | no |
| 5 | **"No tab open" empty state → "New tab…" button** (`root.rs:2663-2686`, id `open-project-empty`) | `new_tab_action` → our opener | no |
| 6 | **Our opener's last row "Browse folders…"** (`opener.rs:282`, `Offer::Browse`, always appended after the machines) | `OpenerEvent::Browse` → `browse_local` → **`cx.prompt_for_paths`** (`root.rs:2158`): the OS folder dialog — **NSOpenPanel on macOS**, the GTK/portal dialog on Linux. The picked folder opens as a local project, then the tab-face sheet | **yes** |
| 7 | **Launch with nothing to show** (`workspace.rs:224-228`) | if no project can be restored and the machine has no home, the folder the app was started from opens | no (no control) |
| 8 | **`arbos://chat/…` links** in prose (`open_chat_link`) | select an existing chat; never opens a new project | no |
| 9 | Opener **`Create <path>`** row (`opener.rs:422`) | makes the folder and opens it — the "new project from nothing" path | no |

Not project entry points, for completeness: the panel's `+` (bottom right of the panel) is **New sub-chat ⌘N** (`panel.rs:1213`); the composer's `+` is attachments and uses the native *file* picker (`composer.rs:985`) — a different dialog, for files not folders.

## Which is "the mac button"

Only one control opens a native macOS dialog for a project: **#6, the "Browse folders…" row at the foot of our opener**, which opens Finder's folder sheet. Everything else — ⌘T, the tab strip `+`, ⌘O, the menu item, the palette rows, the empty-state button — already goes through our own opener. So the change Jacob asks for is the removal of one row and one function (`Offer::Browse`, `OpenerEvent::Browse`, `browse_local`), plus a decision on **#3**: ⌘O / File › Open Project… is a second name for the same opener; if "one way in" is the goal, ⌘O and the menu item go too, or stay as aliases of ⌘T (Cursor keeps ⌘O for folders in the IDE; the Agents window has no ⌘O).

A caveat worth telling him: with #6 gone there is no way to reach a folder the opener cannot list — one outside the home tree whose path he does not know by heart. The opener does take a typed absolute path (`/Volumes/…`), so nothing becomes unreachable, but the Finder-style browse does.

## Discoverability of ⌘T, if it becomes the way in

What a first-time user sees today that says "⌘T":
- The tab strip's `+` at the end of the tabs — its tooltip reads *New tab ⌘T* (`tabs.rs:127`), but only on hover.
- The "No tab open" empty state, only when nothing is open (a fresh install lands on the Home tab, so most users never see it); its button is labelled *New tab…* without the chord.
- The ⌘K palette's *New Tab* row shows ⌘T (`chat_search.rs:76`) — only to someone who already knows ⌘K.
- The tab-face sheet on first launch says nothing about tabs.

So today, nothing on the resting screen names the chord. Two places can carry it cheaply, both mine to change once he decides: the empty state's button can read *New tab  ⌘T* (the chord in the label, not the tooltip); and the tab strip's `+` can keep its tooltip and gain a one-time caption on a fresh install — *⌘T opens a folder as a tab* — the way Cursor's Agents window shows "New Chat ⌘N" in its sidebar as a permanent labelled row rather than a bare glyph. Cursor's answer to this is worth copying: its sidebar has a **labelled** "New Chat" row with the chord beside it, always visible; a bare `+` is what we have and it is not the same thing.

## What I did not touch

The tab strip itself: whether a labelled "New tab ⌘T" row belongs in it is the settings-tab worker's area now ([Make settings an inline tab](bc-6d9c3785-7bed-5cb3-9eb7-bca86aad4ee5) owns tab-strip rules for non-project tabs); if the decision lands on the strip, I will write to them in the inbox rather than edit `tabs.rs`.
