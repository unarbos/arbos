---
cursor:
  subagentId: "bc-2a9237f8-931e-50f0-a873-48a5c79d1aef"
---

# Report-a-problem UI — exploration (read-only)

Exploration of `/workspace` (unarbos/arbos) and the Project store for adding an in-app "report a problem" control. No files were modified.

---

## 1. Cursor parity / reference — feedback control placement

### What is recorded (verbatim)

**Primary documented placement — under a finished assistant answer**, not a global "Report a bug" entry:

From `docs/cursor-parity-report-2026-09-12.md` row 8:

> | 8 | Under a finished answer | Thumbs up, thumbs down, copy, and a relative time (`Just now`, `2m ago`). `public-video-frame-working-2-prs-5-pills.png` | Copy and fork (branch) icons only; no time. `suite-2026-09-12/p2-multifile-end.png` | No feedback, no time | Add thumbs and the relative time; keep fork | U-08 |

From `docs/cursor-parity-report-2026-09-13.md` row 8 (closed on integration build):

> | 8 | Under a finished answer | 👍 👎 copy, `2m ago` | **copy · fork · 👍 · 👎 · `Just now`**. `p2-multifile-end.png` | Same. `p3-subagents-live-panel.png` | Closed | Check that `Just now` ages to `2m ago` **[from memory: not verified]** |

From `docs/features-backlog.md` U-08:

> | U-08 | Thumbs up/down + relative time under a turn | Parity row 8 | copy + branch only | S | P3 | — |

**Bottom-left chrome (settings + version), not feedback:**

From `desktop/src/view/status_bar.rs` module doc (lines 1–18):

> Cursor's is the shape being matched. It sits at the bottom left, it is always there, and it is quiet until it has something to say. Resting, it is the version in faint text and nothing else. When a build is waiting it becomes a filled blue control that reads `Update` — not a tinted pill or a dot on an icon, because the whole point is that it cannot be missed.

From `internal/ui-shell-map.md` (Settings entry points):

> | **Panel gear icon** | `panel.rs:1184-1189` | `.on_click` → `open_settings(Section::General)` or `show_permissions` if permission dot |
> | **Keyboard ⌘,** | `root.rs:257` | `KeyBinding::new("cmd-,", OpenSettings, None)` |

From `docs/cursor-parity-report-2026-09-13.md` row 16:

> | 16 | Settings | Gear at the bottom of the sidebar opens settings in place **[from memory: Cursor uses a settings page, not a window]** | Gear at the bottom of the sidebar opens a separate **Settings window**. `idle-settings-window.png` | Gear at the bottom of the right panel, same window. `idle-settings-window.png` | Different | Keep the window; it is not a look-and-feel blocker |

**Planned in-app feedback (not yet a shipped desktop control)** — kernel bundle + desktop design notes in `internal/features-inbox/2026-09-16-feedback-bundle-desktop-answer.md`:

> The desktop owns: the control and where it sits, the window screenshot, the review sheet that shows every part and lets him cut any of it, the offline outbox, and delivery to the hub.

> **`call_id` on the request is the best of these and cheap.** When Jacob clicks a tool line in the transcript before pressing Send, he is pointing at the failure. That beats every heuristic, and the desktop already knows which line he clicked. Please add `call_id: Option<String>` next to `seq`.

> Design lands at `docs/desktop-feedback-design.md`; I will link this file from it.

(`docs/desktop-feedback-design.md` was **not found** in the store at exploration time.)

### What is **not** recorded in parity material

Searched: `docs/cursor-parity-report-*.md`, `internal/symmetry-findings.md`, `internal/ui-shell-map.md`, `internal/ui-research/bottom-bar-apis.md`, `internal/desktop-feedback-inventory.md`, `docs/features-backlog.md`, `media/cursor-reference/` (only `cycle-17/f94-root-record-after-relaunch.json`).

**No store record** of any of these for Cursor:

- Help menu → "Report a bug" / "Give feedback"
- Command palette entry for feedback
- Status-bar bug icon
- Per-message flag separate from thumbs down
- Global keyboard shortcut for reporting

The closest Cursor pattern captured for *negative* feedback is **thumbs down on the turn footer**, beside copy and relative time, visible after a turn finishes (hover-revealed copy/fork in Arbos; thumbs always faint-visible per parity row 8).

---

## 2. Arbos desktop UI shell

### Window layout (top → bottom)

```
Arbos::render (root.rs)
├── commands + WINDOW_CONTEXT focus sink
├── tab_bar (tabs.rs)           — 36px, full width
├── flex_row (flex_1)
│   ├── detail (detail.rs)      — main chat column, flex_1
│   └── panel (panel.rs)        — optional right rail, 280px
├── status_bar (status_bar.rs)  — 28px, full width, bottom-left gear + version/update
└── overlays (siblings): opener, tab_sheet, permissions_sheet, chat_search
```

**Root render chain** — `desktop/src/view/root.rs` **2270–2317**:

```2270:2317:desktop/src/view/root.rs
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            // ...
            .map(|root| self.commands(root, cx))
            .child(div().key_context(WINDOW_CONTEXT).track_focus(&self.focus))
            .child(self.tab_bar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .flex_row()
                    .child(self.detail(window, cx))
                    .children(self.panel(window, cx)),
            )
            .child(self.status_bar(cx))
            // ...
            .child(self.opener.clone())
            .child(self.tab_sheet.clone())
            .child(self.permissions_sheet.clone())
            .child(self.chat_search.clone())
```

### Title band / tab bar

- Constants: `HEADER_HEIGHT = 36` — `root.rs` **115–118**
- Tab bar root: `tabs.rs` **101–118** (`id("tab-bar")`, chrome bg, border-bottom, tabs + `new-tab` ghost)
- macOS titlebar/traffic lights: `TRAFFIC_LIGHT_X/Y`, `TitlebarOptions` — `root.rs` **189–215** (used in `open`, not quoted here)

### Status bar

- Module + Cursor parity intent: `status_bar.rs` **1–21**
- Render: `status_bar()` — **42–63** (`id("status-bar")`, settings ghost + update/version)
- Settings gear click: **97–103** → `open_settings(Section::General)` or permissions sheet
- Version label at rest: `build::version_label()` — **247–284**

### Menu bar

- File: `desktop/src/view/menubar.rs`
- Menu tree: **137–209** — Arbos (Settings, Permissions, Quit), File, Edit, View, Window
- **No** Help menu, **no** feedback/report item
- Settings: **142** `MenuItem::action("Settings…", OpenSettings)`
- Keybindings registered here: **50–58** (`cmd-q`, `cmd-h`, `cmd-shift-w`, etc.)

### “Command palette” vs palette

| File | Role |
|------|------|
| `desktop/src/view/palette.rs` | **Colour palette** only (`set_palette`, Cursor dark tokens). **Not** a command UI. |
| `desktop/src/view/component/chat_search.rs` | **⌘K search** over chats (`CommandPalette` from bezel). **No** feedback action. |

### Detail column (transcript + composer stack)

- Entry: `detail()` — `detail.rs` **633–729**
- Structure: optional `chat_header` → body (`conversation` / project / surface) → bottom stack with pills, queue, live view, **composer**, context row
- Composer block: **696–724** (max width `CHAT_MAX_WIDTH`, gutter, `composer_height()`)
- Transcript rendered inside `conversation` via `transcript::render(...)` — **~1891**

### Composer

- File: `desktop/src/view/component/composer.rs`
- Init/keybindings: **68–91** (`enter` send, `cmd-shift-enter` queue, `/` menu navigation)
- Glass card surface: **48–50** (`SURFACE`)

### Transcript + single message/tool line

- Main render: `transcript::render` — **2929–2936**
- Turn footer (thumbs/copy/fork/rewind/time): `turn_footer` — **3929–4079**
- User prompt card + hover pencil: `user_prompt` — **1017–1129**
- Tool line render: `tool()` — **4760–4876**; element id `("tool", ix)` at **4863**
- Fold/work lines: `fold_row` — **4373–4437**

### Sheet / modal patterns to copy

**Permissions sheet** — `permissions_sheet.rs`:

- Centred modal, dimmed full-window scrim — **168–182** (`absolute().inset_0()`, `bg` alpha 0.45)
- Escape bound in `ArbosPermissionsSheet` context — **37–42**
- Footer with Ghost + Prominent buttons — **123–167**

**Tab sheet** — `tab_sheet.rs`:

- Same overlay pattern — **190–199** (`absolute().inset_0()`, scrim, centred card `WIDTH = 340`)
- Enter/Escape — **35–38**
- Glass surface card — **25**, render continues **~200+**

Both are mounted as root siblings in `Arbos::render` (see root.rs **2315–2316**).

---

## 3. Existing per-message affordances

### Finished turn footer (assistant answer)

`turn_footer` — `transcript.rs` **3901–4078**:

- Shown only when turn **not running** and `footer_at[position]` — **3905–3911**
- Order comment: **4035** — `// Cursor's order: thumbs up, thumbs down, copy, fork, then "Just now".`
- **Thumbs up/down**: ids `vote-up-{id}-{turn}` / `vote-down-{id}-{turn}` — **4048–4061**; `vote_turn` in `workspace.rs` **1887–1934**
- **Copy**: always visible (faint) — **3956–3989**
- **Fork / rewind**: hover-revealed via `group_hover(msg-{turn})` — **3991–4034**
- **Relative time**: `turn-time-{id}-{turn}` — **4069–4077**

Votes attach to the **user prompt** that started the turn (`turn` index = turn range start), not per assistant paragraph.

### User prompt

- Hover **pencil** (edit/resubmit): `replay-prompt-{id}-{ix}` — **1104–1125**
- No context menu

### Tool lines

- Rendered by `tool()` — **4760+**
- **No** hover action row (copy/thumbs/report)
- Click toggles expand / opens child session — **4864–4876**
- Identifiers at render:
  - **`ix`**: index in `chat.items` (used in `.id(("tool", ix))`)
  - **`id`**: tool `call_id` string on `ChatItem::Tool { id, ... }` — `session.rs` **83–105**
- **No `seq`** on `ChatItem`; kernel `Event.seq` is used in `acp.rs` to distinguish recorded vs delta lines (**912**, **939–941**) but is **not stored** on desktop chat items

### Assistant prose / thinking

- Code blocks: copy-on-hover (configured in `root::init` **220–238**, implemented in transcript markdown path)
- Thinking/tool rows: fold chevrons on hover for nested runs — **4430–4436**

### Implication for attaching a report to a transcript line

| Line type | Index at render | Kernel seq | call_id |
|-----------|-----------------|------------|---------|
| User prompt | `ix`, turn index | No | N/A |
| Tool | `ix` | No | `ChatItem::Tool.id` |
| Assistant paragraph | `ix` | No | N/A |
| Turn (for bundle) | turn index from `turns()` | Must map from file offset or send `turn` + optional `call_id` | From tool row if selected |

The feedback-bundle desktop answer explicitly plans **`seq` + `call_id`** on the wire frame; desktop render today has **`ix` + Tool.id** only.

---

## 4. Keyboard shortcuts

### Registration pattern

- Global app: `Arbos::init` → `cx.bind_keys([...])` — `root.rs` **240–306**
- Per-view contexts: `composer.rs` **68–91**, `settings/mod.rs` **33–38**, `permissions_sheet.rs` **37–42**, `tab_sheet.rs` **32–38**, `opener.rs` **69–80**, `terminal.rs` **28–32**, `menubar.rs` **50–58**
- Text fields: `view/mod.rs` `bind_field_editing` — **33–99** (duplicates cmd-c/x/v/z per context)

### Global shortcuts already taken (`root.rs` **240–306**)

| Chord | Action |
|-------|--------|
| `cmd-n` | NewSession |
| `cmd-t` / `cmd-o` | NewTab / OpenProject |
| `cmd-w` | CloseProject |
| `cmd-shift-]` / `[`, `cmd-}` / `{`, `ctrl-tab`, `ctrl-shift-tab` | Tab cycling |
| `cmd-,` | OpenSettings |
| `cmd-b` | TogglePanel |
| `cmd-shift-c` / `cmd-shift-m` | StartCall / ToggleMute |
| `cmd-1` / `cmd-2` | ShowChat / ShowProject |
| `cmd-k` | SearchChats |
| `cmd-=`, `cmd-+`, `cmd--`, `cmd-0` | Zoom |
| `alt-cmd-up/down` | Prev/NextEntry |
| `cmd-c` | CopySelection (fallback) |
| `ctrl-c` / `ctrl-v` | CopyChat / PasteChat |
| delete/backspace | DeleteChat (ArbosWindow context) |
| up/down | Prev/NextEntry (ArbosWindow) |
| escape | DismissMenu / DismissName |

Menubar-only: `cmd-q`, `cmd-h`, `alt-cmd-h`, `cmd-shift-w`, `cmd-m`, `ctrl-cmd-f`.

**No shortcut** is bound for feedback/report. **`cmd-shift-r`** and **`cmd-alt-r`** appear free at global scope (verify before binding — composer uses context-scoped keys only).

---

## 5. Settings window + version info

### Where

- Separate window (not in-tab sheet): `settings/mod.rs` **1–6**, opened via `root::open_settings` (see `ui-shell-map.md` **170–186**)
- Sections enum: **64–70** — General, Model, Permissions, Appearance, Performance

### Version display

**Settings › General** — `settings/general.rs`:

| Row | Lines | Value |
|-----|-------|-------|
| Version | **60–63** | `env!("CARGO_PKG_VERSION")` |
| Build | **67–71** | `build::badge()` → `"layout {COMMIT} · kernel {KERNEL_VERSION}"` |
| Commit | **73–86** | `build::COMMIT`, link to GitHub when known |
| This build | **202–229** | **`build::version_label()`** — e.g. `0.2.0 (879)` |

Also shown in:

- Status bar resting text — `status_bar.rs` **247–284**
- Panel/settings gear tooltip — `ui-shell-map.md` cites `build::badge()` at panel foot

`build::version_label()` defined — `desktop/src/build.rs` **27–37** (uses `ARBOS_BUILD` / git rev-count).

---

## 6. Recommendations aligned with recorded Cursor pattern

1. **Primary affordance**: extend or sit beside the existing **turn footer** (thumbs/copy/time) for answer-level feedback; thumbs-down already exists and writes `feedback.jsonl` — a "report a problem" flow may want a **separate** action (sheet with note + bundle) rather than overloading vote.
2. **Reachability (~1s)**: **status bar** gear area or a persistent bottom-left control matches Cursor chrome (`status_bar.rs`); alternatively a **global chord** (none taken) + footer when the user is already looking at a line.
3. **Line attachment**: use **`ChatItem::Tool.id` (`call_id`)** when a tool row is selected; map **`ix` → kernel `seq`** via transcript file or new field if the bundle requires `seq`.
4. **Review dialog**: copy **`permissions_sheet.rs`** / **`tab_sheet.rs`** overlay pattern (scrim + centred card + Escape).
5. **Do not** put feedback in `palette.rs` (colours) or confuse with `chat_search.rs` (⌘K).

---

## Key file index

| Topic | Path |
|-------|------|
| Window layout | `desktop/src/view/root.rs` |
| Tab bar | `desktop/src/view/tabs.rs` |
| Status bar | `desktop/src/view/status_bar.rs` |
| Menubar | `desktop/src/view/menubar.rs` |
| Detail / composer stack | `desktop/src/view/detail.rs` |
| Composer | `desktop/src/view/component/composer.rs` |
| Transcript | `desktop/src/view/component/transcript.rs` |
| Chat search (⌘K) | `desktop/src/view/component/chat_search.rs` |
| Colour palette | `desktop/src/view/palette.rs` |
| Modal sheets | `permissions_sheet.rs`, `tab_sheet.rs` |
| Settings + version | `settings/mod.rs`, `settings/general.rs`, `build.rs` |
| ChatItem schema | `model/session.rs` 65–137 |
| Thumbs persistence | `model/workspace.rs` 1887–1934 |
| Parity (Cursor thumbs) | `docs/cursor-parity-report-2026-09-12.md` row 8 |
| Feedback bundle plan | `internal/features-inbox/2026-09-16-feedback-bundle-desktop-answer.md` |
| UI shell map | `internal/ui-shell-map.md` |
