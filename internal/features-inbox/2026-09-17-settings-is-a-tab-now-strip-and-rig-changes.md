---
cursor:
  subagentId: "bc-6d9c3785-7bed-5cb3-9eb7-bca86aad4ee5"
---

# Settings is a tab now — what changed in the strip, the driver and your rig

To: the desktop parity loop. From: the settings-surface worker.
Branch: `cursor/settings-as-inline-tab-4ee5` on `unarbos/arbos`.

Jacob, 09-17: "settings should be a tab that opens rather than a floating panel,
it should be a full inlined tab." The settings window is gone. It is a tab in
the strip now, beside the project tabs, filling the window's middle.

You own the chat rendering, the opener, and the enumeration of project entry
points. I own the settings surface and the tab-strip rules a non-project tab
needs. This note is the part of my change that lands in your area, so you do not
find it by surprise.

## What the strip does now

- `Arbos::settings_tab: Option<SettingsTab>` holds the pane and whether it is
  the tab in front, in one field. `Arbos::front() -> Front` (`Project` |
  `Settings`) says which kind of tab the middle draws.
- The Settings pill draws **after every project tab and before the `+`**. It is
  `tab-settings`, its close mark is `tab-settings-close`.
- A project tab is shaded only when a project tab is in front:
  `selected = active == Some(ix) && self.front() == Front::Project`.
- `cycle_tab` (⌃Tab, ⇧⌘] / ⇧⌘[) rings over the projects **and** the Settings
  slot when it is open. With two projects and Settings open there are three
  slots, so ⌃Tab twice from Settings lands on the second project, not back on
  Settings.
- `select_project` leaves Settings first, so a tab click is always a full
  switch.
- `close_project_action` (⌘W) closes whichever tab is in front, so with Settings
  in front it closes Settings and leaves every project alone.
- With Settings in front the right-hand panel is not drawn: the panel is a view
  of a project's `.arbos/`, and Settings has none.

If you need something else of the strip, say so here rather than reshaping
`front()` — one field is what keeps "Settings is in front" and "a Settings tab
exists" from disagreeing.

## What changed in the driver you drive it with

- There is **no `"settings"` window kind** any more. `windows()` returns
  `main` and `other` only, and `use_window("settings")` fails by design.
- `state()` gained `front` (`"project"` | `"settings"`) and
  `settings_section` (`"general"`, `"model"`, `"permissions"`, `"appearance"`,
  `"performance"`).
- `settings_open` survives with a changed meaning: **the tab is open**, not a
  window is open. It stays true while the tab sits behind the chat, so a test
  that means "the settings surface is on screen" must read `front == "settings"`
  as well. That is the one way an old assertion can quietly keep passing.
- New ids: `tab-settings`, `tab-settings-close`, `settings-back-to-chat`.
  `settings-back-to-chat` is absent when no project is open, because there is no
  chat to go back to.

## What I changed in `qa/parity/ui_pass.py` (your file)

I edited it rather than leaving it broken, because every settings row in it
drove the second window and would have recorded `not-reachable` and returned —
a phase that silently stops testing rather than failing.

- `phase_settings` drives the tab: the gear opens it, the pill and rail are
  asserted, and the control walk runs in one window (`main_state()` and every
  `use_window` juggle deleted).
- The `bionic-reading` weight probe steps out to the chat with ⌘1, photographs
  the window, and comes back by clicking `tab-settings`. No window masking: the
  chat's prose is behind the tab rather than under a floating window.
- New `phase_settings_ways_out`: Escape, the pill, ⌘1, the rail's row, the
  pill's ×, ⌘, and ⌘W, one check each.
- The first-launch block now expects Escape to leave the tab rather than to
  close a window, and closes the tab by its ×.
- `close_second_windows()` is deleted (it `wmctrl -c Settings`'d a window that
  cannot exist) and the `ImageDraw` import with it.
- `driver/examples/sweep.py` skips `tab-settings-close` instead of `settings`.

I drove all of that on Linux with Xvfb — 29 checks, all passing — but I did not
run your whole `ui_pass` suite end to end (it wants a model key and a long
journey). If a settings row in your next cycle reads oddly, that is the first
place to look, and the change is mine to fix: say so here.

## Two things I did not touch

- The window's own chrome and traffic lights. One window fewer, no chrome
  added.
- The one-surface rule. The pane paints `root::content_bg`, the rail
  `root::chrome_bg`, and I checked the pixels rather than my eye: strip,
  rail, body and bottom bar are all `(22, 21, 20)` in the dark palette, the
  same value the chat shows at the same points.
