---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# For QA: the new window layout — tabs, right panel, no sidebar

From the layout agent. Branch `cursor/window-layout-tabs-right-panel` → `rust`, built on `cursor/release-integration-52cd` (PR #58). Spec: `docs/project-context.md`, "Desktop app layout (decided 2026-09-13)".

## What changed

- No left sidebar. A tab bar runs across the top; each tab is a project (a folder on a machine).
- Launch lands on a **Home** tab whose root is `~/.arbos` (created if missing). Tabs open last time follow it.
- `⌘T` (and `⌘O`) open the machine-then-folder picker; the folder becomes a tab. `⇧⌘]` / `⇧⌘[` and `Ctrl+Tab` / `Ctrl+Shift+Tab` cycle tabs. `⌘W` closes the tab in front; the window now closes on `⇧⌘W`. Middle-click a tab closes it. `⌘B` folds the panel.
- Each project has **one main chat**. `⌘N` makes a **sub-chat** nested under it (shown with the sub-agents), never a second root.
- The right panel is a live view of the project's `.arbos/`: **Agents** (main chat, sub-agents nested, click opens the chat, the header shows the path back as crumbs), **Processes** (terminals, jobs, standing plan nodes), **Resources** (browser pages, panels, store folder counts), **Goals** (`.arbos/GOALS.md`, else the root agent's `plan.md`), **Notes** (`.arbos/notes.md`). Settings gear bottom-left, `+` (sub-chat) bottom-right.
- Panel agent rows keep the sidebar's gestures: drag onto the composer (chip with the chat link), right-click menu (copy / fork / archive / delete), double-click renames in the header.

## How to exercise it

Linux: `cmd` is the Super key. Drive it with `desktop/driver/arbosdriver.py` (`ARBOS_DRIVER_SOCKET=/tmp/arbos.sock`); element ids: `tab-<ix>`, `tab-close-<ix>`, `new-tab`, `panel-agent-<id>`, `panel-surface-<id>`, `panel-set-goals`, `chat-header-crumb-<id>`, `toggle-panel`, `settings`, `new-subchat`.

Prompt that spawns sub-agents reliably: *You must spawn three parallel sub-agents: one reviews math_utils.py for edge cases, one writes docstrings for every function, one drafts a CHANGELOG.md. Then merge their results.*

## What could break — attack here

1. **Home tab**: delete `~/.arbos` while the app runs, then relaunch. Make `~/.arbos` a file, not a folder. `$HOME` unset.
2. **Tabs**: open 15 tabs (labels should truncate, `+` must stay reachable). Close the active tab, the first tab, the last tab; check which tab lands in front and that `state.toml` still lists the rest. Close every tab: the empty view offers "New tab…".
3. **Reopen restores**: run the sub-agent prompt, close the tab mid-turn, reopen the folder. Expected: main chat and sub-agents back, checks on the done ones. Watch the sub-agent order — after a relaunch it followed `delegate_number`, which came back reversed once (Draft, Add, Review instead of Review, Add, Draft).
4. **⌘N sub-chat before the main chat has a kernel id** (press ⌘N within a second of the tab opening), then relaunch. It must stay nested — fixed once, verify.
5. **Archive the main chat** (right-click → Archive). The project then has no main; ⌘N must create a new root, the panel must not go blank, `Set goals…` must still land in a composer.
6. **Goals file**: write `.arbos/GOALS.md` while the app runs (watch knock, ≤1 s). Empty file, 500-line file (only 12 lines show plus "N more"), a file with only `(no plan)`, `goals.md` lowercase. Same for `notes.md`.
7. **Narrow window**: under 900 px wide the panel hides; the header toggle must still work and nothing should overlap the composer.
8. **Remote place** (ssh host tab): panel shows no goals/notes/resources (sidecar only); no crash, no "Set goals…" link.
9. **Keyboard cycling**: `⌥⌘↓` / `⌥⌘↑` and bare ↑/↓ with focus on the window (not a field) step through the agents tree only within the active tab.
10. **Driver state**: `state()` no longer has `sidebar_open`/`sidebar_width`/`expanded`; it has `panel_open`. Anything in `internal/qa/desktop_scenarios.py` that clicked sidebar rows (`project-add-*`, "New Chat" at (163,139)) needs re-aiming at `new-subchat` / `new-tab`.

## Known, not fixed here

- `cargo test --lib` in `desktop/` does not compile on the integration base (`transcript.rs` tests call removed `work_bits` / `work_bit_index`); my `watch.rs` cases ride on the same target.
- Reopening a folder whose main chat is still finishing shows a spinner on the main row while it re-reads the transcript.
