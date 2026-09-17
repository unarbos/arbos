---
cursor:
  subagentId: "bc-6d9c3785-7bed-5cb3-9eb7-bca86aad4ee5"
---

# Proposal: what fills Settings › General now that it is a full-height tab

For Jacob, through the coordinator. Nothing built. Asked for after
[PR #451](https://github.com/unarbos/arbos/pull/451) made Settings an inline
tab: the reading column is right at full width, but General ends after six rows
and the rest of a 1000px-tall tab is blank, which a 620px window hid.

The rule I am proposing to follow: **fill it, do not stretch it.** The column
stays capped at 860px and left-aligned to the same measure; what changes is that
the page has something to say all the way down.

## What General holds today

The mark, "Arbos", the tagline. Then two boxes: Version / Build / Commit, and
Updates (Stable · Dev) / Last checked / This build.

## What I would add

Two new groups under the existing two, in this order. Top-down: what this copy
of the app **is**, then how it **updates**, then what it is **talking to**, then
where to **read more**.

### Group 3 — This machine

The point of the group: a person can answer "what is my app actually running
against" without a terminal.

| row | what it shows | where the data is |
| --- | --- | --- |
| **Kernel** | the version and build of the kernel this window is talking to, the path of the binary it started, and whether that build agrees with the app's own | the app already prints `kernel 0.2.0` inside `build::badge()`, and the bottom bar already knows a *stranger* kernel when it meets one (`status-bar-stranger`) — the row is those two facts written down where no pointer is needed |
| **Serving** | one line per open place: the place, the kernel serving it, and whether it is the build this app ships | `Workspace::projects` plus the same stranger check the bar runs |
| **Store** | `~/.arbos`, as a path, with a click to reveal it | known |
| **Config** | `~/.config/arbos/config.toml`, path plus reveal | the Model section already carries a `config-reveal`; General gets the plain path |
| **App state** | `~/.config/arbos-desktop/state.toml` | known |

The **Kernel** row is the one that earns its place today. We spent the morning
making it honest that a kernel can be a stranger, 223 commits behind, rejecting
frames it has never heard of and losing work while looking healthy. The bar
shouts when it catches one. This row is the quiet version of the same fact:
readable before anything goes wrong, by a person who is merely curious, and
readable by the driver too, so a test can assert it.

Two rules I would hold it to:

- It answers **present, absent, or unknown**, and says which. "No kernel
  running for this place yet" is a state, not a blank.
- It reports the kernel it **is talking to**, read off the connection, not the
  binary it would launch. Those two differ in exactly the case that matters —
  the stranger — so a row that reads the path and calls it the answer would be
  the same class of bug we have been clearing all day.

### Group 4 — About

| row | what it shows |
| --- | --- |
| **Licence** | MIT, the text a click away |
| **Source** | `github.com/unarbos/arbos`, as a link (the Commit badge already links a commit; the repository itself is not written anywhere) |
| **Website** | `arbos.life` |
| **Report a problem** | the same sheet ⇧⌘R opens, so the way to complain sits where a person goes looking for "about this app" |

## What I would not add

- **No About window or panel.** The menu deliberately has no About item,
  because an about panel is a window this app does not have. This group is that
  content, in the place a tab can hold it.
- **No changelog.** The update channel owns what is new; a second telling of it
  goes stale on its own.
- **No model or key rows.** Those are the Model section's, and duplicating them
  is two places to read one truth.
- **No filling for its own sake.** If Jacob wants General to stay short, the
  honest alternative is to leave the space empty rather than pad it: an empty
  lower half is a smaller cost than a page of rows nobody needs.

## Size of the change

Contained: `desktop/src/view/settings/general.rs` for the rows, plus one read
for the kernel a connection is actually on. The **Serving** row is the only part
that needs anything new from the model, and it needs a read, not a write. No
change to the tab, the strip, or the panes.
