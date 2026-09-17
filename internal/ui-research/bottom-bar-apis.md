---
cursor:
  subagentId: "bc-a9e31e46-414e-55fe-911e-f56c41ba7408"
---

# Bottom-left bar UI APIs — Theme, buttons, icons, animation, driver

Research for a persistent bottom-left bar (settings gear + filled blue "Update" button). Read-only audit of `/workspace` (`unarbos/arbos`).

---

## 1. The `Theme` struct

**Definition:** `bezel-theme` crate (dependency `0.1.8`), **not vendored** — source at:

`/usr/local/cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bezel-theme-0.1.8/src/theme/mod.rs`

Re-exported as `bezel::theme::Theme` / `theme::Theme` from `bezel-ui`.

All colour fields are `gpui::Hsla`. Full struct (colour fields only):

```157:348:/usr/local/cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bezel-theme-0.1.8/src/theme/mod.rs
/// The app theme. Two concrete instances — [`Theme::dark`] and [`Theme::light`].
#[derive(Debug, Clone)]
pub struct Theme {
    /// Which appearance these tokens were built for.
    pub appearance: Appearance,

    // ---- paint: neutral surfaces ----
    /// Main content panel. Dark: the deepest plane (#060606). Light: pure white —
    /// long-form content reads best on an unbroken white field.
    pub bg: Hsla,
    /// Shell / sidebar surface. Dark: one step *up* from `bg`. Light: one step
    /// *down* (grey) — chrome recedes from the content plane in both, which is
    /// the direction a naive invert gets backwards.
    pub surface: Hsla,
    /// Raised surface: opaque pills and chips that sit proud of the panel.
    /// Dark: lighter than `surface`. Light: white, separated by `border` +
    /// shadow rather than by lightness.
    pub surface_raised: Hsla,

    // ---- paint: elevation ladder ----
    pub surface_card: Hsla,
    pub surface_dialog: Hsla,
    pub surface_overlay: Hsla,
    /// Hover wash for interactive rows and buttons, on glass and off it alike:
    /// `../desktop`'s `--color-hover`.
    pub element_hover: Hsla,
    /// Active/selected wash, one rung over the hover — `--color-active`.
    pub element_active: Hsla,
    /// Hairline border.
    pub border: Hsla,
    /// Stronger border for focused/raised edges.
    pub border_strong: Hsla,

    // ---- paint: text ----
    /// Primary text. ~17.5:1 on its own background in both appearances.
    pub text: Hsla,
    /// Muted text: timestamps, secondary labels. ~7.5–8:1.
    pub text_muted: Hsla,
    /// Faint text: placeholders, disabled. ~4.5:1 — AA for body copy.
    pub text_faint: Hsla,
    /// One notch below `text_muted` — the diff file-path tone.
    pub text_dim: Hsla,

    // ---- paint: high-contrast solid (primary buttons) ----
    /// The maximum-contrast solid fill: near-white on dark, near-black on light.
    /// This is the primary button plate.
    pub solid: Hsla,
    /// Label/icon color on top of [`Self::solid`] — its inverse.
    pub on_solid: Hsla,

    // ---- paint: accents ----
    /// Accent — the emphasis weight for text and icons.
    /// **Neutral by default, and deliberately.** … The default is now the same lightness
    /// with the chroma at zero. This is the token to brand.
    pub accent: Hsla,
    /// Stronger accent for fills that carry [`Self::on_accent`] text. Neutral by
    /// default, it is the maximum-contrast plate — a mid grey would not carry a
    /// label the way the indigo it replaced did.
    pub accent_strong: Hsla,
    /// Label color on top of [`Self::accent_strong`].
    pub on_accent: Hsla,
    /// Danger — red (errors, stop button).
    pub danger: Hsla,
    /// Softer danger for secondary/inline error copy.
    pub danger_muted: Hsla,
    /// Warning — amber (offline notices, awaiting-input).
    pub warning: Hsla,
    /// Softer warning for secondary copy.
    pub warning_muted: Hsla,
    /// Success / online — emerald.
    pub success: Hsla,
    /// Working / streaming indicator — pink.
    pub busy: Hsla,
    /// Softer success for text on a success-tinted chip.
    pub success_muted: Hsla,

    // ---- paint: components ----
    /// Hover tone for an *opaque* raised pill.
    pub surface_raised_hover: Hsla,
    /// Recessed band behind a palette/picker header or footer strip.
    pub band: Hsla,
    /// The composer pill and other input plates.
    pub input_bg: Hsla,
    /// Text-selection highlight in the composer and inputs.
    pub selection: Hsla,
    /// Terminal block cursor.
    pub cursor: Hsla,
    /// Text caret — [`Self::accent`]'s lightness.
    pub caret: Hsla,
    /// Keyboard focus ring — a hairline.
    pub ring: Hsla,
    /// Destructive-action button fill (danger plate, carries [`Self::on_accent`]).
    pub danger_strong: Hsla,

    // ---- paint: code & diff ----
    pub code_text: Hsla,
    pub code_wash: Hsla,
    pub syntax: SyntaxPalette,
    pub diff_add: Hsla,
    pub diff_del: Hsla,
    pub diff_hunk_bg: Hsla,
    // … glass, fonts (non-colour) …
}
```

### Accent / primary blue — answers

| Question | Answer |
|----------|--------|
| **Which field is the accent/primary blue?** | **`theme.accent`**. In Arbos dark mode this is overridden to Cursor blue `#86aee4` (see §3). Default bezel dark is achromatic grey at indigo-400 lightness. |
| **On-accent foreground?** | **`theme.on_accent`** — doc says "Label color on top of `accent_strong`". **`theme.on_solid`** is for the high-contrast **`solid`** / **`ButtonStyle::Prominent`** plate. |
| **Accent hover/active variants?** | **No dedicated tokens.** Use `element_hover` / `element_active` for ghost washes, or chain `.hover(|s| s.opacity(0.9))` like `ButtonStyle::Prominent`. Link-style accent hover in the app uses `.hover(|el| el.text_color(theme.accent))` or `.text_color(theme.accent_strong)`. |
| **`accent_strong`?** | Intended for filled accent plates with `on_accent` text. **`palette.rs` does not override it** — at runtime it stays the default dark value `color::neutral(0.922)` (near-white). Only `accent` and `caret` get Cursor blue. |

Default dark palette values (before Arbos override):

```35:37:/usr/local/cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bezel-theme-0.1.8/src/theme/palettes.rs
            accent: color::neutral(0.673),        // indigo-400's lightness, no chroma
            accent_strong: color::neutral(0.922), // the solid plate
            on_accent: color::grey(0x0e),         // its inverse label
```

---

## 2. Button-like widgets on `Theme`

Catalog traits in `desktop/vendor/bezel-ui/src/widgets/`. Import groups, call on `Theme`:

```rust
use bezel::ui::widgets::{Buttons, Controls, Content, Layout, Scaffolding, Status};
let theme = Theme::of(cx);
theme.button(...); theme.ghost(...); theme.progress_bar(...);
```

### `Buttons` — `desktop/vendor/bezel-ui/src/widgets/buttons.rs`

**`ButtonStyle` enum:** `Ghost`, `Prominent`, `Destructive`.

**`button(label, style, fade)` — full body:**

```49:82:desktop/vendor/bezel-ui/src/widgets/buttons.rs
    fn button(
        &self,
        label: impl Into<SharedString>,
        style: ButtonStyle,
        fade: Option<Fade>,
    ) -> Div {
        let theme = self.theme();
        let label = label.into();
        match style {
            ButtonStyle::Ghost => match fade {
                Some(fade) => {
                    let mut btn = frame()
                        .text_color(motion::hover_blend(&fade, theme.text_muted, theme.text))
                        .bg(motion::hover_blend(&fade, ink(0.0), theme.element_hover))
                        .child(label);
                    btn.interactivity().on_hover(motion::hover_listener(fade));
                    btn
                }
                None => frame().text_color(theme.text_muted).child(label),
            },
            ButtonStyle::Prominent => frame()
                .bg(theme.text)
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.on_solid)
                .hover(|s| s.opacity(0.9))
                .child(label),
            ButtonStyle::Destructive => frame()
                .bg(theme.danger_strong)
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(gpui::white())
                .hover(|s| s.opacity(0.9))
                .child(label),
        }
    }
```

**`icon_button(icon, style, fade)` — full body:**

```94:125:desktop/vendor/bezel-ui/src/widgets/buttons.rs
    fn icon_button(&self, icon: &'static str, style: ButtonStyle, fade: Option<Fade>) -> Div {
        let theme = self.theme();
        let square = frame()
            .px(px(0.0))
            .w(px(Theme::BUTTON_HEIGHT))
            .justify_center();
        let glyph = |tint| crate::icons::icon(icon).size(px(GLYPH)).text_color(tint);
        match style {
            ButtonStyle::Ghost => match fade {
                Some(fade) => { /* … hover_blend … */ }
                None => square.child(glyph(theme.text_muted)),
            },
            ButtonStyle::Prominent => square
                .bg(theme.text)
                .hover(|s| s.opacity(0.9))
                .child(glyph(theme.on_solid)),
            ButtonStyle::Destructive => square
                .bg(theme.danger_strong)
                .hover(|s| s.opacity(0.9))
                .child(glyph(gpui::white())),
        }
    }
```

**`control_group()` — full body:**

```143:155:desktop/vendor/bezel-ui/src/widgets/buttons.rs
    fn control_group(&self) -> Div {
        let theme = self.theme();
        div()
            .self_start()
            .flex()
            .flex_row()
            .gap(px(GROUP_PAD))
            .p(px(GROUP_PAD))
            .rounded(px(Theme::button_radius() + GROUP_PAD))
            .bg(theme.surface_raised)
            .border_1()
            .border_color(theme.border)
    }
```

**`ghost(id)` — full body:**

```160:170:desktop/vendor/bezel-ui/src/widgets/buttons.rs
    fn ghost(&self, id: impl Into<ElementId>) -> Stateful<Div> {
        let tint = self.theme().element_hover;
        div()
            .id(id)
            .flex()
            .flex_row()
            .items_center()
            .rounded(px(Theme::control_radius()))
            .cursor_pointer()
            .hover(move |el| el.bg(tint))
    }
```

### Does a filled accent/primary **blue** button exist?

**No.** `ButtonStyle::Prominent` uses **`theme.text`** (white in dark Cursor palette) as fill, **`theme.on_solid`** as label — the "send plate" look, not accent blue.

Closest built-in accent fill is **`pagination::page_button`** when `current: true`:

```116:125:desktop/vendor/bezel-ui/src/pagination.rs
    if current {
        button
            .bg(theme.accent)
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(theme.on_accent)
    } else {
        button
            .text_color(theme.text_muted)
            .hover(|s| s.bg(theme.element_hover).text_color(theme.text))
    }
```

For a filled blue "Update" button, build explicitly, e.g.:

```rust
theme.button("Update", ButtonStyle::Ghost, None) // wrong — not blue
// Instead:
div()
    .id("update")
    .control_size(ControlSize::Regular) // or mirror frame() from buttons.rs
    .flex().items_center().cursor_pointer()
    .px(px(12.)).py(px(6.))
    .rounded(px(Theme::button_radius()))
    .bg(theme.accent)
    .font_weight(FontWeight::MEDIUM)
    .text_color(theme.on_accent) // or theme.solid / white if contrast needs tuning
    .hover(|s| s.opacity(0.9))
    .child("Update")
```

### `Controls` — `desktop/vendor/bezel-ui/src/widgets/controls.rs`

- **`progress_bar(fraction)`** — full body at lines 113–128 (track `ink(0.12)`, fill `theme.text`).
- **`toggle`**, **`checkbox`**, **`radio_button`**, **`slider`**, **`select_trigger`**, **`toggle_group`**, **`toggle_group_item`**.

### `Content` — `desktop/vendor/bezel-ui/src/widgets/content.rs`

- **`badge`**, **`badge_active`**, **`avatar`**, **`tag`**, **`breadcrumb`**, **`breadcrumb_item`**, **`breadcrumb_separator`**, **`empty_state`**.

### `Scaffolding` — `desktop/vendor/bezel-ui/src/widgets/scaffolding.rs`

- **`page_column`**, **`page_header`**, **`page_subtitle`**, **`field_label`**, **`option_card_row`**, **`option_card`**, **`group_box`**, **`card_row`**, **`row_icon`**, **`row_title`**, **`meta_line`**.

### `Layout` — `desktop/vendor/bezel-ui/src/widgets/layout.rs`

- **`disclosure`**, **`collapsible_header`**, **`nav_row`**, **`split_handle`**, **`tab_bar`**, **`tab`**.

### `Status` — `desktop/vendor/bezel-ui/src/widgets/status.rs`

- **`step_row`**, **`step_output`**, **`error_strip`**, **`warning_strip`**.

---

## 3. `desktop/src/view/palette.rs` — runtime colours

Full file (104 lines). Every colour set in `cursor_dark`:

```40:103:desktop/src/view/palette.rs
fn cursor_dark(t: &mut Theme) {
    // Panels.
    t.bg = grey(0x161514);
    t.surface = grey(0x1a1a16);
    t.surface_card = grey(0x212121);
    t.surface_raised = grey(0x212121);
    t.surface_raised_hover = grey(0x2a2a2a);
    t.surface_dialog = grey(0x1e1e1e);
    t.surface_overlay = grey(0x242424);
    t.input_bg = grey(0x212121);
    t.border = hsla(0.0, 0.0, 1.0, 0.11);
    t.border_strong = hsla(0.0, 0.0, 1.0, 0.18);
    t.text = grey(0xf0f0f0);
    t.text_muted = grey(0xbbbbbb);
    t.text_faint = grey(0x999898);
    t.text_dim = grey(0x6b6b6b);
    t.code_wash = grey(0x272625);
    t.code_text = grey(0xf0f0f0);
    t.accent = grey(0x86aee4);   // ← Cursor blue
    t.caret = grey(0x86aee4);
    t.selection = hsla(0.6, 0.6, 0.7, 0.33);
    t.solid = grey(0xf0f0f0);
    t.element_hover = hsla(0.0, 0.0, 1.0, 0.035);
    t.element_active = hsla(0.0, 0.0, 1.0, 0.05);
    t.popover_surface = SurfaceStyle::Material(Material::UltraThick);
    t.syntax = SyntaxPalette { /* VS Code Dark+ tokens */ };
}
```

**Not overridden:** `accent_strong`, `on_accent`, `on_solid`, `danger*`, `success*`, etc. — those remain bezel defaults from `Theme::dark()`.

`grey(value)` is `rgb(value).into()` → opaque sRGB hex as `Hsla`.

---

## 4. Icon rendering

**Crate:** `bezel-icons` (`0.1.8`), re-exported as `bezel::ui::icons` (desktop: `use bezel::ui::icons`).

**`icons::icon(path)`** returns `gpui::Svg`:

```294:298:/usr/local/cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bezel-icons-0.1.8/src/lib.rs
pub fn icon(path: &'static str) -> Svg {
    svg().path(path).flex_none()
}
```

- **Size:** `.size(px(14.))` (or any `Pixels`).
- **Colour:** `.text_color(theme.text_muted)` — gpui tints SVG as alpha mask; colour must be on the `Svg`, not the parent button.

### Settings gear in `panel.rs` (lines 1150–1206)

```1150:1206:desktop/src/view/panel.rs
            .child(
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
                            .when(wants_permission, |el| {
                                el.child(
                                    div()
                                        .id("settings-dot")
                                        .absolute()
                                        .top(px(-2.))
                                        .right(px(-3.))
                                        .size(px(6.))
                                        .rounded_full()
                                        .bg(theme.warning),
                                )
                            }),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if wants_permission {
                            this.show_permissions(window, cx);
                        } else {
                            this.open_settings(Section::General, cx);
                        }
                    })),
            )
            .child(
                theme
                    .ghost("search-chats")
                    /* … icons::system::MAGNIFER … */
            )
            .child(
                theme
                    .ghost("new-subchat")
                    /* … icons::system::PLUS … */
            )
```

### Download / arrow-down / refresh constants (`bezel-icons`)

| Constant | Module | Lucide source |
|----------|--------|---------------|
| **`icons::files::DOWNLOAD`** | `files` | `LuDownload` |
| **`icons::arrows::ARROW_DOWN`** | `arrows` | `LuArrowDown` |
| **`icons::arrows::ALT_ARROW_DOWN`** | `arrows` | `LuChevronDown` (disclosure chevron) |
| **`icons::system::REFRESH`** | `system` | `LuRefreshCw` |
| **`icons::system::RESTART`** | `system` | `LuRotateCcw` |
| **`icons::arrows::ARROW_UP`** | `arrows` | `LuArrowUp` |

Other useful `system::*`: `SETTINGS_MINIMALISTIC`, `MAGNIFER`, `PLUS`, `CLOSE`, `SUN`, `MOON`, `CHAT_ROUND_LINE`.

---

## 5. Tooltips and hover

### `Tooltip` — `desktop/vendor/bezel-ui/src/tooltip.rs`

```28:53:desktop/vendor/bezel-ui/src/tooltip.rs
    pub fn text(text: impl Into<SharedString>, _window: &mut Window, cx: &mut App) -> AnyView {
        let text = text.into();
        cx.new(|_| Self { text, keystroke: None }).into()
    }

    pub fn with_keystroke(
        text: impl Into<SharedString>,
        keystroke: impl Into<SharedString>,
        _window: &mut Window,
        cx: &mut App,
    ) -> AnyView {
        let (text, keystroke) = (text.into(), keystroke.into());
        cx.new(|_| Self { text, keystroke: Some(keystroke) }).into()
    }
```

**Plain tooltip** (`project_page.rs`):

```94:94:desktop/src/view/project_page.rs
                    .tooltip(|window, cx| Tooltip::text("Open notes.md as a document", window, cx))
```

**With keystroke** (`project_page.rs`):

```115:117:desktop/src/view/project_page.rs
                    .tooltip(|window, cx| {
                        Tooltip::with_keystroke("Back to the chat (Esc)", "⌘1", window, cx)
                    })
```

Chain on any element: `.tooltip(|window, cx| Tooltip::text("…", window, cx))`.

### `.hover` / `.active` idioms

**Ghost wash (buttons, rows):**

```643:644:desktop/src/view/component/transcript.rs
                        el.hover(|el| el.bg(theme.element_hover))
                            .active(|el| el.bg(theme.element_active))
```

**Copy control:**

```3716:3717:desktop/src/view/component/transcript.rs
                .hover(|el| el.bg(theme.element_hover))
                .active(|el| el.bg(theme.element_active))
```

**Accent text on hover:**

```76:76:desktop/src/view/settings/general.rs
                                    .hover(|el| el.text_color(theme.accent))
```

**Accent link with stronger hover** (`panel.rs`):

```931:933:desktop/src/view/panel.rs
                            .text_color(theme.accent)
                            .cursor_pointer()
                            .hover(|el| el.text_color(theme.accent_strong))
```

**Prominent button hover:** `.hover(|s| s.opacity(0.9))` inside `Buttons::button`.

**Note:** gpui panics on a **second** `.hover()` on the same element — use `motion::Fade` + `hover_blend` for nested hover (see `nav_row`, `ButtonStyle::Ghost` with `fade: Some(...)`).

---

## 6. Animation / repaint

### `Painter` — `bezel-motion` (`bezel::motion::Painter`)

```128:159:/usr/local/cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bezel-motion-0.1.8/src/lib.rs
pub struct Painter(EntityId);

impl Painter {
    pub fn of<T: 'static>(cx: &Context<T>) -> Self {
        Self(cx.entity_id())
    }

    pub fn notify(self, cx: &mut App) {
        cx.notify(self.0);
    }

    /// Claim `fps` redraws a second, lapsing `until` from now unless something
    /// renews it. … Renew it from `render` — that is what makes a claim self-cancelling.
    pub fn lease(self, fps: f32, until: Duration, cx: &mut App) {
        lease(self.0, fps, until, cx);
    }
}
```

A global **`PulseClock`** wakes views on schedule; each `lease()` call renews while the spinner/progress UI is mounted in `render`.

### Spinner constants and function — `transcript.rs`

```66:69:desktop/src/view/component/transcript.rs
const BRAILLE: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const BRAILLE_TICK_MS: u128 = 80;
const BRAILLE_FPS: f32 = 12.5;
const BRAILLE_LEASE: Duration = Duration::from_millis(300);
```

```4895:4911:desktop/src/view/component/transcript.rs
fn spinner_frame(since: Duration, reduce_motion: bool) -> &'static str {
    if reduce_motion {
        "···"
    } else {
        BRAILLE[(since.as_millis() / BRAILLE_TICK_MS) as usize % BRAILLE.len()]
    }
}

pub fn spinner<V: 'static>(since: Duration, color: Hsla, cx: &mut Context<V>) -> AnyElement {
    Painter::of(cx).lease(BRAILLE_FPS, BRAILLE_LEASE, cx);
    div()
        .text_style(TextStyle::Callout)
        .text_color(color)
        .child(spinner_frame(since, cx.reduce_motion()))
        .into_any_element()
}
```

**Pattern for download progress:**

1. Store `started: Instant` + `fraction: f32` in view state.
2. In `render`, call `Painter::of(cx).lease(fps, lease_duration, cx)` while downloading.
3. Paint `theme.progress_bar(fraction)` or `spinner(elapsed, theme.text_muted, cx)`.
4. On background completion: `this.update(cx, |this, cx| { this.fraction = 1.0; cx.notify(); })`.

### `cx.spawn` — representative examples

**`workspace.rs` — background fetch, then UI update:**

```282:294:desktop/src/model/workspace.rs
    fn load_agent_icons(&mut self, cx: &mut Context<Self>) {
        let configured = self.settings.agents.clone();
        cx.spawn(async move |this, cx| {
            let icons = cx
                .background_executor()
                .spawn(async move { agent::icons(&configured) })
                .await;
            let _ = this.update(cx, |workspace, cx| {
                workspace.agent_icons = icons;
                cx.notify();
            });
        })
        .detach();
    }
```

**`root.rs` — periodic poll loop:**

```1515:1545:desktop/src/view/root.rs
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(400))
                    .await;
                let lines = crate::voice_ws::drain_mirror();
                /* … */
                let keep = this.update(cx, |this, cx| {
                    /* mutate state, cx.notify via workspace updates */
                    live
                });
                if !matches!(keep, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
```

**`transcript.rs` — timer then clear flash state:**

```3723:3734:desktop/src/view/component/transcript.rs
                    cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(COPY_FLASH).await;
                        let _ = this.update(cx, |this, cx| {
                            this.with_session(id, cx, |chat| {
                                if chat.transcript.copy_flash.get(&turn)
                                    .is_some_and(|at| at.elapsed() >= COPY_FLASH)
                                {
                                    chat.transcript.copy_flash.remove(&turn);
                                }
                            });
                        });
                    }).detach();
```

---

## 7. Element probes (test driver)

**File:** `desktop/src/driver.rs`

### ID convention

- Set with `.id("kebab-case-name")` on the interactive element (often the same element that gets `.on_click`).
- Examples in panel footer: `"settings"`, `"search-chats"`, `"new-subchat"`, `"settings-dot"`.
- Use **stable, unique, kebab-case** strings; driver matches by full path, tail, or wildcards.
- **Put `.id` on the hit target**, not a wrapper (bezel docs warn for `step_row`, `option_card`).

### Matching

```909:916:desktop/src/driver.rs
/// Whether `pattern` names the element at `path`. A pattern is a full path
/// (`a.b.c`), a tail of one (`c` or `b.c`), or either with `*` wildcards.
fn matches(pattern: &str, path: &str) -> bool {
    if pattern.contains('*') {
        return wild(pattern, path) || wild(&format!("*.{pattern}"), path);
    }
    path == pattern || path.ends_with(&format!(".{pattern}"))
}
```

### Click by id

Wire request:

```json
{"id": 1, "method": "click", "params": {"target": "settings"}}
```

Or nested in `at`:

```600:628:desktop/src/driver.rs
        "click" => {
            let MouseParams { at, button, count, modifiers } = parse(params)?;
            let probe = at
                .target
                .as_deref()
                .map(|target| find(window, target))
                .transpose()?;
            let position = match &probe {
                Some(probe) => probe.bounds.center(),
                None => locate(window, &at)?,
            };
            /* mouse_move + mouse_down/up at position */
            if let Some(probe) = probe {
                out["element"] = describe(&probe, window);
            }
            Ok(out)
        }
```

Also accepts top-level `"target": "settings"` via `find` in `locate()`.

**Suggested ids for new bar:** `"bottom-bar-settings"`, `"bottom-bar-update"`, `"update-progress"` (if separate).

---

## Quick recipe: bottom-left bar

Mirror existing panel footer (`panel.rs` ~1140–1220): `flex_row`, `theme.border` top hairline, `theme.ghost("…")` for gear, custom accent fill for Update, `.id(...)` on each control, `Tooltip::with_keystroke` optional.
