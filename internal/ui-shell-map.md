---
cursor:
  subagentId: "bc-86f3e5dd-f6d8-5f43-84f3-4c03852b8b01"
---

# Desktop UI shell map (for bottom-left bar work)

## 1. Window layout and the panel (not a left sidebar)

**Important:** `desktop/src/view/panel.rs` is the **right-hand** panel (Agents / Project tree), not a left sidebar. The main window has **no left column**. Layout is: custom titlebar band → tab bar → `[ detail (main) | panel (right, optional) ]`.

### Root render chain (`Arbos::render`)

```2199:2248:desktop/src/view/root.rs
impl Render for Arbos {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_terminal(window, cx);
        let theme = Theme::of(cx).clone();
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            // ...
            .map(|root| self.commands(root, cx))
            .child(div().key_context(WINDOW_CONTEXT).track_focus(&self.focus))
            // The strip of tabs across the top, then the chat column with
            // the panel on its right.
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
            // ...
    }
}
```

**Regions:**

| Region | File | Size / behaviour |
|--------|------|------------------|
| Titlebar | AppKit + `TitlebarOptions` in `root::open` | Transparent; traffic lights at `(TRAFFIC_LIGHT_X=12, TRAFFIC_LIGHT_Y=11)` in a `HEADER_HEIGHT=36` band |
| Tab bar | `tabs.rs` → `tab_bar()` | `h(36)`, full width, `chrome_bg`, border-bottom |
| Main column | `detail.rs` → `detail()` | `flex_1`, `content_bg` |
| Right panel | `panel.rs` → `panel()` | `PANEL_WIDTH=280`, hidden if `!panel_open` or window `< PANEL_MIN_WINDOW (1000)` |

Default window: `1100×761` (`WINDOW_WIDTH` / `WINDOW_HEIGHT` in `root.rs`).

### Tab bar root (`tabs.rs`)

```92:109:desktop/src/view/tabs.rs
        div()
            .id("tab-bar")
            .flex_none()
            .h(px(root::HEADER_HEIGHT))
            .w_full()
            .bg(root::chrome_bg(&theme))
            .border_b_1()
            .border_color(theme.border)
            .flex()
            .flex_row()
            .items_center()
            .pl(px(root::TOOLBAR_INSET))
            .pr(px(root::HEADER_INSET))
            .gap(px(4.))
            .children(tabs.into_iter().map(|tab| { ... }))
            .child(/* + new tab ghost button */)
```

### Right panel root chain (`panel.rs`)

```426:440:desktop/src/view/panel.rs
        Some(
            div()
                .id("panel")
                .flex_none()
                .w(px(PANEL_WIDTH))
                .h_full()
                .bg(root::chrome_bg(&theme))
                .border_l_1()
                .border_color(theme.border)
                .flex()
                .flex_col()
                .child(body)          // scrollable sections
                .child(self.panel_foot(&theme, cx))
                .into_any_element(),
        )
```

Scroll body (`#panel-scroll`): `flex_1`, `overflow_y_scroll`, `px(PAD_X=10)`.

### Existing bottom anchor (panel foot only)

The **only** persistent bottom strip today is `panel_foot` — **inside the right panel**, not window-level bottom-left:

```1135:1222:desktop/src/view/panel.rs
    /// The bottom strip: settings on the left, a sub-chat on the right.
    fn panel_foot(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex_none()
            .h(px(40.))
            .px(px(PAD_X))
            .border_t_1()
            .border_color(theme.border)
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(/* settings ghost — left */)
            .child(/* search-chats ghost */)
            .child(/* new-subchat ghost — right */)
            .into_any_element()
    }
```

Settings gear is the **first** child (left within the foot). Search and new sub-chat are to its right; `justify_between` spreads the three clusters.

There is **nothing** anchored to the bottom-left of the full window or the main `detail` column today.

---

## 2. How Settings opens today

**Action type:** `OpenSettings` — gpui action in the `arbos` namespace (`root.rs` `actions!` macro, line 55).

### Entry points

| Entry | File | Dispatch |
|-------|------|----------|
| **Menubar** | `menubar.rs:142` | `MenuItem::action("Settings…", OpenSettings)` |
| **Keyboard ⌘,** | `root.rs:257` | `KeyBinding::new("cmd-,", OpenSettings, None)` |
| **Panel gear icon** | `panel.rs:1184-1189` | `.on_click` → `open_settings(Section::General)` or `show_permissions` if permission dot |
| **Root action handler** | `root.rs:1322-1328` | `open_settings_action` → `open_settings(Section::General, cx)` |
| **Global fallback** | `menubar.rs:112-115` | When settings window is front, forwards to workspace window |

**Not present:**

- No Settings entry in `ChatSearch` (⌘K palette — `chat_search.rs`)
- `palette.rs` is **not** a command palette; it registers Cursor colour overrides (see §4)
- No other gear icon at window bottom-left

### Dispatch code (core path)

Action definition and keybind:

```46:77:desktop/src/view/root.rs
actions!(
    arbos,
    [
        // ...
        OpenSettings,
        // ...
    ]
);
```

```256:257:desktop/src/view/root.rs
        KeyBinding::new("cmd-,", OpenSettings, None),
```

Handler → opens separate Settings window:

```1322:1504:desktop/src/view/root.rs
    pub(crate) fn open_settings_action(
        &mut self,
        _: &OpenSettings,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings(Section::General, cx);
    }

    pub(crate) fn open_settings(&mut self, section: Section, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        let had = self.settings_window.is_some();
        self.settings_window = settings::open(workspace, self.settings_window, section, cx);
        // ... observe_release to refocus main window when settings closes
    }
```

Panel gear click:

```1184:1189:desktop/src/view/panel.rs
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if wants_permission {
                            this.show_permissions(window, cx);
                        } else {
                            this.open_settings(Section::General, cx);
                        }
                    })),
```

Menubar wiring on root (for `is_action_available` greying):

```269:271:desktop/src/view/menubar.rs
            .on_action(cx.listener(Self::open_settings_action))
```

**Separate:** `permission_center::open_settings` opens **macOS System Settings** URLs for permissions — not the in-app Settings window (`permission_center.rs:285`).

---

## 3. Settings model and persistence

Two TOML files under **`~/.config/arbos-desktop/`** (via `settings::dir()`):

| File | Purpose | Format |
|------|---------|--------|
| `settings.toml` | Machine config (features, cover_memory, watch_bounce, legacy agents) | Serde + `toml_edit` for surgical writes |
| `state.toml` | Session/UI prefs (appearance, projects, frame, text_size, …) | Serde + pretty TOML |

### `settings.toml` — struct and load

```9:31:desktop/src/model/settings.rs
#[derive(Debug, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "cover_memory")]
    pub cover_memory: u64,
    #[serde(default = "watch_bounce")]
    pub watch_bounce: u64,
    #[serde(default)]
    pub features: Features,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<Agent>,
}
```

```197:217:desktop/src/model/settings.rs
pub fn load() -> Result<Settings> {
    let dir = dir()?;
    let path = dir.join("settings.toml");
    if !path.exists() {
        let settings = Settings::default();
        std::fs::create_dir_all(&dir)?;
        let body = format!(
            "# generated by arbos-desktop — edits are kept, deleting regenerates defaults\n\n{}",
            toml::to_string_pretty(&settings)?
        );
        std::fs::write(&path, body)?;
        return Ok(settings);
    }
    // ...
}
```

### Pattern for adding an enum-valued (or discrete) setting

**Model layer** — define field + default + `set_*` using `toml_edit` (preserves comments):

Example: `Feature` enum + `set_feature` (`settings.rs:59-95`, `225-237`).

**Workspace layer** — write file first, then update in-memory cache:

```464:499:desktop/src/model/workspace.rs
    pub fn set_feature(&mut self, feature: Feature, on: bool, cx: &mut Context<Self>) {
        if settings::set_feature(feature, on).is_err() {
            return;
        }
        feature.set(&mut self.settings.features, on);
        cx.notify();
    }

    pub fn set_watch_bounce(&mut self, ms: u64, cx: &mut Context<Self>) {
        let ms = ms.clamp(watch::BOUNCE_RANGE.0, watch::BOUNCE_RANGE.1);
        if settings::set_watch_bounce(ms).is_err() {
            return;
        }
        self.settings.watch_bounce = ms;
        cx.notify();
    }
```

**UI layer** — segmented control in a `card_row` (see `performance.rs` watch delay).

### End-to-end example: `watch_bounce`

**1. Struct field + default** (`settings.rs`):

```16:23:desktop/src/model/settings.rs
    #[serde(default = "watch_bounce")]
    pub watch_bounce: u64,
```

```128:131:desktop/src/model/settings.rs
fn watch_bounce() -> u64 {
    watch::BOUNCE
}
```

**2. Persist** (`settings.rs`):

```251:259:desktop/src/model/settings.rs
pub fn set_watch_bounce(ms: u64) -> Result<()> {
    let path = dir()?.join("settings.toml");
    let body = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut =
        body.parse().context("settings.toml is not valid toml")?;
    doc["watch_bounce"] = toml_edit::value(ms as i64);
    std::fs::write(&path, doc.to_string())?;
    Ok(())
}
```

**3. Workspace apply** — quoted above (`set_watch_bounce`).

**4. Settings UI row** (`performance.rs`):

```22:26:desktop/src/view/settings/performance.rs
const BOUNCES: [(u64, &str); 3] = [
    (50, "Instant"),
    (watch::BOUNCE, "Balanced"),
    (500, "Relaxed"),
];
```

```87:144:desktop/src/view/settings/performance.rs
    fn watch_bounce_row(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        // ...
        theme.card_row(false)
            .child(/* title + note */)
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .gap(px(2.))
                    .p(px(2.))
                    .rounded(px(Theme::button_radius()))
                    .border_1()
                    .border_color(theme.border)
                    .children(BOUNCES.into_iter().map(|(value, label)| {
                        // selected / unselected styling
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.workspace.update(cx, |workspace, cx| {
                                workspace.set_watch_bounce(value, cx)
                            });
                            cx.notify();
                        }))
                    })),
            )
    }
```

**Appearance prefs** (theme mode, tint, transparency) live in **`state.toml`**, not `settings.toml` — via `Workspace::save()` → `state::save` (`workspace.rs:249-268`, `state.rs:119-121`).

---

## 4. Visual idiom / design tokens

### Colour palette

- **`desktop/src/view/palette.rs`** — installs Cursor-sampled dark palette via `set_palette(build, cx)`; called from `bin/main.rs:68` before `appearance::init`.
- Light mode uses bezel defaults; dark overrides `bg`, `surface`, `text_*`, `accent`, `element_hover/active`, syntax colours (see `cursor_dark` in `palette.rs:40-102`).
- Runtime theme: `Theme::of(cx)` from bezel; chrome uses `root::chrome_bg` / `content_bg` with optional vibrancy material.

### Settings “theme” module

**`desktop/src/view/settings/theme.rs`** is the **Appearance settings page** (mode segmented control, tint sliders, transparency toggle) — not a global token registry.

### Typography / spacing

- `bezel::theme::TextStyle` (`Body`, `Caption`, `Callout`, `Title3`, …) + `Typeset` trait on elements.
- Layout constants in views: `HEADER_HEIGHT`, `PAD_X`, `ROW_HEIGHT`, `GROUP_GAP`, etc.
- Theme helpers: `theme.ghost()`, `theme.card_row()`, `theme.group_box()`, `theme.nav_row()`, `theme.badge()`, `theme.toggle()`, `theme.slider()`, `Theme::button_radius()`, `Theme::control_radius()`.

### Icons

- **Bezel icon font:** `icons::icon(icons::system::SETTINGS_MINIMALISTIC)` etc.
- **Custom SVGs:** `desktop/src/view/*.svg` embedded via `assets.rs` → `AssetSource::load`; rendered with `svg().path(crate::assets::PHONE_ICON)...` (`panel.rs:524-528`).
- Registration: `Assets` impl in `assets.rs:46-77`.

### Reusable pill / button components

- **`theme.ghost(id)`** — primary chrome control (settings gear, panel buttons, tab close). Example from panel foot:

```1151:1170:desktop/src/view/panel.rs
                theme
                    .ghost("settings")
                    .px(px(8.))
                    .py(px(6.))
                    .tooltip(|window, cx| {
                        Tooltip::with_keystroke(
                            format!("Settings — {}", crate::build::badge()),
                            "⌘,",
                            window,
                            cx,
                        )
                    })
                    .child(
                        div()
                            .relative()
                            .child(
                                icons::icon(icons::system::SETTINGS_MINIMALISTIC)
                                    .size(px(14.))
                                    .text_color(theme.text_muted),
                            )
```

- **`chips.rs`** — markdown link chip glyphs in prose, not UI buttons.
- **`desktop/src/view/component/`** — feature widgets (composer, transcript, menu); no generic `Button` module in-tree (uses bezel `ui::widgets::Buttons` / `Theme` methods).

### Row pill pattern (panel list rows)

```1427:1445:desktop/src/view/panel.rs
fn row(id: (&'static str, u64), depth: u8, selected: bool, theme: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .h(px(ROW_HEIGHT))   // 26
        .pl(px(8. + TREE_STEP * f32::from(depth)))
        .pr(px(8.))
        .rounded(px(5.))
        // hover / selection washes
}
```

---

## 5. Status / progress affordances

### Frame meter (`meter.rs`)

Floating **FPS/stats overlay** — not download progress:

```20:31:desktop/src/view/component/meter.rs
pub(crate) fn panel(
    id: &'static str,
    at: &Floating,
    meter: &Entity<Stats>,
    window: &Window,
) -> AnyElement {
    let home = point(
        px(f32::from(window.viewport_size().width) - stats::WIDTH - INSET),
        px(INSET),
    );
    floating::panel(id, at, home, meter.clone()).into_any_element()
}
```

Toggled from Settings › Performance; rendered on root when `workspace.meter` (`root.rs:2238-2242`).

### Determinate progress bar

`Theme::progress_bar(fraction)` in bezel controls:

```111:127:desktop/vendor/bezel-ui/src/widgets/controls.rs
    fn progress_bar(&self, fraction: f32) -> Div {
        let theme = self.theme();
        let fraction = fraction.clamp(0.0, 1.0);
        div()
            .w_full()
            .h(px(4.0))
            .rounded_full()
            .bg(ink(0.12))
            .child(
                div()
                    .h_full()
                    .w(gpui::relative(fraction))
                    .rounded_full()
                    .bg(theme.text),
            )
    }
```

Used for **context token usage** in composer (`composer.rs:2061-2075`), not downloads.

### Indeterminate spinner

Braille animation in `transcript.rs`:

```4905:4911:desktop/src/view/component/transcript.rs
pub fn spinner<V: 'static>(since: Duration, color: Hsla, cx: &mut Context<V>) -> AnyElement {
    Painter::of(cx).lease(BRAILLE_FPS, BRAILLE_LEASE, cx);
    div()
        .text_style(TextStyle::Callout)
        .text_color(color)
        .child(spinner_frame(since, cx.reduce_motion()))
        .into_any_element()
}
```

Used throughout panel (working agents, call connecting), tabs, detail header, etc.

---

## 6. App version

### Compile-time sources

| Source | Value |
|--------|-------|
| `desktop/Cargo.toml` | `version = "0.2.0"` → `env!("CARGO_PKG_VERSION")` |
| `desktop/build.rs` | `ARBOS_COMMIT` (git short SHA + `-dirty`), `ARBOS_KERNEL_VERSION` from sibling crate |
| `desktop/src/build.rs` | `COMMIT`, `KERNEL_VERSION`, `badge()` → `"layout {COMMIT} · kernel {KERNEL_VERSION}"` |
| `desktop/changelog.json` | Release notes JSON; latest entry `0.2.0` |

`build.rs` commit logic:

```33:40:desktop/build.rs
fn commit() -> String {
    let Some(short) = git(&["rev-parse", "--short=7", "HEAD"]) else {
        return "unknown".into();
    };
    match git(&["status", "--porcelain"]).is_none_or(|tree| tree.is_empty()) {
        true => short,
        false => format!("{short}-dirty"),
    }
}
```

Runtime badge (`src/build.rs`):

```12:15:desktop/src/build.rs
pub fn badge() -> String {
    format!("layout {COMMIT} · kernel {KERNEL_VERSION}")
}
```

Also used in panel settings tooltip (`panel.rs:1157`) and driver JSON (`driver.rs:796` uses `CARGO_PKG_VERSION`).

### About / version in Settings UI

`Settings › General` (`general.rs`):

```10:17:desktop/src/view/settings/general.rs
const VERSION: &str = env!("CARGO_PKG_VERSION");
const COMMIT: &str = build::COMMIT;
```

```51:82:desktop/src/view/settings/general.rs
            .child(
                theme
                    .group_box()
                    .child(
                        theme.card_row(true)
                            .child(/* "Version" */)
                            .child(theme.badge(VERSION)),
                    )
                    .child(
                        theme.card_row(false)
                            .child(/* "Build" */)
                            .child(theme.badge(build::badge())),
                    )
                    .child(
                        theme.card_row(false)
                            .child(/* "Commit" */)
                            .child(/* badge, clickable to GitHub if known */),
                    ),
            )
```

No separate About window (menubar comment: `menubar.rs:135-136`).

---

## Implications for a Cursor-like bottom-left bar

1. **Anchor point:** Add at **window root** (`Arbos::render`) or **`detail()` column** — not in `panel_foot`, which disappears when panel is closed or window `< 1000px`.
2. **Reuse:** `theme.ghost()` + `icons::icon()` matches existing settings control styling; place settings + update side-by-side in a `flex_row` with fixed height (~40px) mirroring `panel_foot`.
3. **Settings open:** Call `this.open_settings(Section::General, cx)` or dispatch `OpenSettings`.
4. **Progress for downloads:** Reuse `theme.progress_bar(fraction)` for determinate; `transcript::spinner` for indeterminate.
5. **Version label:** `build::badge()` or `env!("CARGO_PKG_VERSION")`.
