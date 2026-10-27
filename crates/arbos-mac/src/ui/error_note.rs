//! The one error element (Jacob, 2026-10-26, [feedback error-messages-mac-2026-10-26]): a soft
//! red block, a red "!" disc, **one plain sentence**, then the actions — **Try again** (or
//! **Sign in**) as a small capsule and **Details**, a disclosure that unfolds a mono block
//! (what · call · where / when / build) with a **Copy** capsule for a feedback report. Every
//! failure the user sees on the Mac is a [`UserError`] drawn through [`render`]; nothing else in
//! the app draws red failure text (`error_ui_lint` pins it).
//!
//! Placement is the caller's, by the rule in `docs/mac-design-rules.md` § "Error messages":
//! sheets → top of the body under the bar / tab pill; pane pages → under the pane header;
//! composer failures → directly above the composer; chat-level → the slot under the header;
//! toasts → the notice stack top-right. The note takes space (pushes content) everywhere but
//! the toast stack; it is never absolute over other controls.
//!
//! Tokens (`theme.rs`): fill `danger_soft` over the surface, radius 10, padding 10 / 12, disc
//! 15 in `danger` with a white "!", sentence `TEXT_SMALL` 13 / 17 in `text`, Try again a 24 pt
//! `surface_2` capsule (12.5 semibold), Details 12.5 `text_muted` + chevron, the Details block
//! `code_bg` radius 8 mono 11 / 15 `text_muted` with a 20 pt Copy capsule; a toast adds
//! `shadow_md` and a × top-right.

use gpui::{
    div, prelude::*, px, App, ClickEvent, ClipboardItem, Div, FontWeight, SharedString, Stateful,
    Window,
};

use crate::sync::user_error::{Action, UserError};
use crate::ui::icons::icon;
use crate::ui::theme::{Theme, TEXT_SMALL};

pub const RADIUS: f32 = 10.0;
pub const PAD_Y: f32 = 10.0;
pub const PAD_X: f32 = 12.0;
pub const DISC: f32 = 15.0;
pub const LINE: f32 = 17.0;
pub const PILL_H: f32 = 24.0;
pub const ACTION_TEXT: f32 = 12.5;
pub const DETAIL_TEXT: f32 = 11.0;
pub const DETAIL_LINE: f32 = 15.0;
pub const COPY_H: f32 = 20.0;
/// A toast's width (the notice stack's `max_w`).
pub const TOAST_WIDTH: f32 = 380.0;
/// The gap a surface leaves between the note and what follows it.
pub const GAP: f32 = 12.0;

/// An error a surface holds while it is up (sheets, pane pages, the composer slot).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    pub error: UserError,
    pub details_open: bool,
}

impl Slot {
    pub fn new(error: UserError) -> Self {
        Self {
            error,
            details_open: false,
        }
    }

    pub fn toggle_details(&mut self) {
        self.details_open = !self.details_open;
    }
}

/// Inline (takes space in its surface) or a toast (shadow, ×).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Inline,
    Toast,
}

pub type Handler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// What the note's controls do. `toggle_details` is always needed; the rest only where the
/// surface can act (a surface that cannot retry gets no Try again whatever the class says).
#[derive(Default)]
pub struct Handlers {
    pub try_again: Option<Handler>,
    pub sign_in: Option<Handler>,
    pub toggle_details: Option<Handler>,
    pub dismiss: Option<Handler>,
    /// One surface-specific capsule before Details — the chat note's **Unblock** when the
    /// relay refused a post because of my own block ([feedback blocked-state-mac-2026-10-26]).
    /// Its id is `{id}-{label}` lower-cased.
    pub extra: Option<(&'static str, Handler)>,
}

impl Handlers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn try_again(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.try_again = Some(Box::new(f));
        self
    }

    pub fn sign_in(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.sign_in = Some(Box::new(f));
        self
    }

    pub fn toggle_details(
        mut self,
        f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.toggle_details = Some(Box::new(f));
        self
    }

    pub fn dismiss(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.dismiss = Some(Box::new(f));
        self
    }

    pub fn extra(
        mut self,
        label: &'static str,
        f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.extra = Some((label, Box::new(f)));
        self
    }
}

struct HandlersWithFlags {
    try_again: Option<Handler>,
    sign_in: Option<Handler>,
    toggle_details: Option<Handler>,
    dismiss: Option<Handler>,
    extra: Option<(&'static str, Handler)>,
    toggle_details_present: bool,
}

impl From<Handlers> for HandlersWithFlags {
    fn from(h: Handlers) -> Self {
        Self {
            toggle_details_present: h.toggle_details.is_some(),
            try_again: h.try_again,
            sign_in: h.sign_in,
            toggle_details: h.toggle_details,
            dismiss: h.dismiss,
            extra: h.extra,
        }
    }
}

/// The element. `id` prefixes every control's id (`{id}-try-again`, `{id}-details`,
/// `{id}-copy`, `{id}-close`) and names the root for the headless tests.
pub fn render(
    id: &str,
    error: &UserError,
    details_open: bool,
    style: Style,
    theme: &Theme,
    handlers: Handlers,
) -> Stateful<Div> {
    let root_id: SharedString = id.to_owned().into();
    let sentence: SharedString = error.sentence().into();
    let action = error.action();
    let handlers = HandlersWithFlags::from(handlers);

    let mut actions = div().flex().items_center().gap(px(12.0));
    let capsule = |label: &'static str, cid: String, theme: &Theme| {
        div()
            .id(SharedString::from(cid.clone()))
            .debug_selector(move || cid.clone())
            .h(px(PILL_H))
            .px(px(10.0))
            .rounded_full()
            .bg(theme.surface_2)
            .flex()
            .items_center()
            .text_size(px(ACTION_TEXT))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.text)
            .cursor_pointer()
            .hover(|s| s.opacity(0.85))
            .child(label)
    };
    match action {
        Action::TryAgain => {
            if let Some(f) = handlers.try_again {
                actions = actions.child(
                    capsule("Try again", format!("{id}-try-again"), theme)
                        .on_click(move |e, w, cx| f(e, w, cx)),
                );
            }
        }
        Action::SignIn => {
            if let Some(f) = handlers.sign_in {
                actions = actions.child(
                    capsule("Sign in", format!("{id}-sign-in"), theme)
                        .on_click(move |e, w, cx| f(e, w, cx)),
                );
            }
        }
        Action::None => {}
    }
    let has_extra = handlers.extra.is_some();
    if let Some((label, f)) = handlers.extra {
        actions = actions.child(
            capsule(label, format!("{id}-{}", label.to_ascii_lowercase()), theme)
                .on_click(move |e, w, cx| f(e, w, cx)),
        );
    }
    if let Some(f) = handlers.toggle_details {
        let did = format!("{id}-details");
        let did2 = did.clone();
        actions = actions.child(
            div()
                .id(SharedString::from(did))
                .debug_selector(move || did2.clone())
                .flex()
                .items_center()
                .gap(px(3.0))
                .text_size(px(ACTION_TEXT))
                .text_color(theme.text_muted)
                .cursor_pointer()
                .hover(|s| s.opacity(0.8))
                .child("Details")
                .child(icon(
                    if details_open {
                        "chevron-up"
                    } else {
                        "chevron-down"
                    },
                    11.0,
                    theme.text_muted,
                ))
                .on_click(move |e, w, cx| f(e, w, cx)),
        );
    }

    // The sentence row (disc + sentence, wrapping), then the actions right-aligned on their own
    // row under it — one deterministic shape at every width, so a 273 pt sheet and an 876 pt
    // column read the same.
    let has_actions =
        !matches!(action, Action::None) || handlers.toggle_details_present || has_extra;
    let row = div()
        .flex()
        .items_start()
        .gap(px(8.0))
        .w_full()
        .child(disc(theme))
        .child(
            div()
                .id(SharedString::from(format!("{id}-sentence")))
                .debug_selector({
                    let s = format!("{id}-sentence");
                    move || s.clone()
                })
                .flex_1()
                .min_w_0()
                .whitespace_normal()
                .text_size(px(TEXT_SMALL))
                .line_height(px(LINE))
                .text_color(theme.text)
                .child(sentence),
        );
    let actions_row = has_actions.then(|| {
        div()
            .mt(px(8.0))
            .w_full()
            .flex()
            .justify_end()
            .child(actions)
    });

    let mut note = div()
        .id(root_id.clone())
        .debug_selector(move || root_id.to_string())
        .relative()
        .w_full()
        .flex()
        .flex_col()
        .rounded(px(RADIUS))
        .bg(theme.danger_soft)
        .px(px(PAD_X))
        .py(px(PAD_Y))
        .when(style == Style::Toast, |d| {
            d.pr(px(PAD_X + 18.0)).shadow_md()
        })
        .child(row)
        .children(actions_row);

    if details_open {
        note = note.child(details_block(id, error, theme));
    }

    if style == Style::Toast {
        if let Some(f) = handlers.dismiss {
            let cid = format!("{id}-close");
            let cid2 = cid.clone();
            note = note.child(
                div()
                    .id(SharedString::from(cid))
                    .debug_selector(move || cid2.clone())
                    .absolute()
                    .top(px(8.0))
                    .right(px(8.0))
                    .size(px(16.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.7))
                    .child(icon("xmark", 12.0, theme.text_muted))
                    .on_click(move |e, w, cx| f(e, w, cx)),
            );
        }
    }
    note
}

/// The red disc with the white "!" — the only red in the note.
fn disc(theme: &Theme) -> Div {
    div()
        .size(px(DISC))
        .flex_shrink_0()
        .mt(px((LINE - DISC) / 2.0))
        .rounded_full()
        .bg(theme.danger)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(10.5))
        .line_height(px(DISC))
        .font_weight(FontWeight::BOLD)
        .text_color(gpui::white())
        .child("!")
}

/// What · call · where, in mono, with Copy bottom-right.
fn details_block(id: &str, error: &UserError, theme: &Theme) -> Stateful<Div> {
    let lines = error.details();
    let copy_text = error.copy_text();
    let cid = format!("{id}-copy");
    let cid2 = cid.clone();
    let bid = format!("{id}-details-block");
    let bid2 = bid.clone();
    let mut block = div()
        .id(SharedString::from(bid))
        .debug_selector(move || bid2.clone())
        .mt(px(10.0))
        .ml(px(DISC + 8.0))
        .rounded(px(8.0))
        .bg(theme.code_bg)
        .px(px(10.0))
        .pt(px(8.0))
        .pb(px(6.0))
        .flex()
        .flex_col()
        .overflow_hidden();
    for line in lines {
        block = block.child(
            div()
                .font_family(theme.font_mono)
                .text_size(px(DETAIL_TEXT))
                .line_height(px(DETAIL_LINE))
                .text_color(theme.text_muted)
                .child(line),
        );
    }
    block = block.child(
        div().mt(px(6.0)).flex().justify_end().child(
            div()
                .id(SharedString::from(cid))
                .debug_selector(move || cid2.clone())
                .h(px(COPY_H))
                .px(px(8.0))
                .rounded_full()
                .bg(theme.surface_2)
                .flex()
                .items_center()
                .text_size(px(DETAIL_TEXT))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .cursor_pointer()
                .hover(|s| s.opacity(0.85))
                .child("Copy")
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(copy_text.clone()));
                }),
        ),
    );
    block
}

/// The drift guard (Jacob, 2026-10-26): no raw error string may reach a notice and no ad-hoc
/// red failure text may land outside this file. Source scan over `crates/arbos-mac/src` plus a
/// structure check that every surface's note is still where the rule puts it. Runs in the Rust
/// gate like any test; a line that must stay carries `// error-ui: exception <case>` on it or
/// on the line above.
#[cfg(test)]
mod error_ui_lint {
    use std::path::{Path, PathBuf};

    fn sources() -> Vec<(PathBuf, String)> {
        fn walk(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
            for entry in std::fs::read_dir(dir).expect("src dir") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).expect("read source");
                    out.push((path, text));
                }
            }
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut out = Vec::new();
        walk(&root, &mut out);
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    fn excepted(lines: &[&str], i: usize) -> bool {
        lines[i].contains("// error-ui: exception")
            || (i > 0
                && lines[i - 1]
                    .trim_start()
                    .starts_with("// error-ui: exception"))
    }

    fn rel(path: &Path) -> String {
        path.strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")))
            .unwrap_or(path)
            .display()
            .to_string()
    }

    /// Raw error values never go into a notice: `notice(format!("…{err}"))`,
    /// `push_notice(format!("…{e:#}"))`, `SyncEvent::Notice(format!("… {}", r.status()))`
    /// and friends are the shapes that put URLs, HTTP verbs and chains on screen.
    #[test]
    fn no_raw_error_strings_reach_a_notice() {
        let markers = [
            "{err",
            "{e}",
            "{e:",
            "{error",
            ":#}",
            ".status()",
            "people_error(",
            "{why",
            "{detail",
            "localized",
        ];
        let mut bad = Vec::new();
        for (path, text) in sources() {
            if path.ends_with("error_note.rs") {
                continue;
            }
            let lines: Vec<&str> = text.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                let notice = line.contains("notice(format!(")
                    || line.contains("Notice(format!(")
                    || line.contains("push_notice(format!(");
                let bare = line.contains("notice(err)")
                    || line.contains("notice(error)")
                    || line.contains("notice(why)")
                    || line.contains("Notice(err)")
                    || line.contains("Notice(error)")
                    || line.contains("push_notice(err")
                    || line.contains("push_notice(error");
                // A multi-line format!: read on to the line that closes the call.
                let end = (i..(i + 6).min(lines.len()))
                    .find(|&j| lines[j].contains("))"))
                    .unwrap_or(i);
                let tail: String = lines[i..=end].join(" ");
                if ((notice && markers.iter().any(|m| tail.contains(m))) || bare)
                    && !excepted(&lines, i)
                {
                    bad.push(format!("{}:{}: {}", rel(&path), i + 1, line.trim()));
                }
            }
        }
        assert!(
            bad.is_empty(),
            "raw error strings in notices (route them through sync::user_error / Shared::fail, or mark the line `// error-ui: exception <case>`):\n{}",
            bad.join("\n")
        );
    }

    /// Red failure text is drawn by this file alone. `.text_color(theme.danger)` elsewhere is a
    /// destructive action label or a status mark and says so with an exception comment.
    #[test]
    fn no_ad_hoc_red_failure_text_outside_error_note() {
        let mut bad = Vec::new();
        for (path, text) in sources() {
            if path.ends_with("error_note.rs") {
                continue;
            }
            let lines: Vec<&str> = text.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                let literal_red_text = line.contains(".text_color(theme.danger)")
                    || line.contains(".text_color(theme.danger.")
                    || line.contains(".text_color(self.theme.danger)");
                if literal_red_text && !excepted(&lines, i) {
                    bad.push(format!("{}:{}: {}", rel(&path), i + 1, line.trim()));
                }
            }
        }
        assert!(
            bad.is_empty(),
            "ad-hoc red text (use ui::error_note::render, or mark a destructive action / status mark `// error-ui: exception <case>`):\n{}",
            bad.join("\n")
        );
    }

    /// Every surface draws its note through `error_note::render` with its own id, in the place
    /// the rule names (`docs/mac-design-rules.md` § "Error messages"). A surface that loses its
    /// note, or a new one that invents its own, fails here.
    #[test]
    fn every_surface_has_its_one_note() {
        let surfaces: &[(&str, &str)] = &[
            ("avatar-generate-error", "ui/avatar_picker.rs"),
            ("composer-error", "ui/composer.rs"),
            ("machine-error", "ui/shell.rs"),
            ("delivery-error", "ui/shell.rs"),
            ("sign-in-error", "ui/shell.rs"),
            ("notice-", "ui/shell.rs"),
            ("people-sheet-error", "ui/people.rs"),
            ("server-sheet-error", "ui/servers.rs"),
            ("secrets-error", "ui/secrets.rs"),
            ("feedback-error", "ui/feedback.rs"),
            ("devices-error", "ui/profiles/me.rs"),
            ("update-error", "ui/profiles/me.rs"),
            ("routine-error", "ui/routines.rs"),
            ("question-card-error", "ui/widgets.rs"),
            ("secret-offer-", "ui/secret_card.rs"),
            ("connector-error-", "ui/connectors.rs"),
        ];
        let all = sources();
        let mut missing = Vec::new();
        for (id, file) in surfaces {
            let hit = all.iter().any(|(path, text)| {
                rel(path).ends_with(file)
                    && text.contains(id)
                    && text.contains("error_note::render(")
            });
            if !hit {
                missing.push(format!("{id} in {file}"));
            }
        }
        assert!(
            missing.is_empty(),
            "surfaces without their note through error_note::render: {missing:?}"
        );
        // And every render call names an id from the list (no ad-hoc surface).
        let mut unknown = Vec::new();
        for (path, text) in &all {
            if path.ends_with("error_note.rs") {
                continue;
            }
            for (i, line) in text.lines().enumerate() {
                if line.contains("error_note::render(") {
                    let window: String = text
                        .lines()
                        .skip(i + 1)
                        .take(3)
                        .collect::<Vec<_>>()
                        .join(" ");
                    let known = surfaces.iter().any(|(id, _)| window.contains(id))
                        || window.contains("&id,")
                        || window.trim_start().starts_with("id,")
                        || window.contains("&format!(\"{row_id}-error\")");
                    if !known {
                        unknown.push(format!("{}:{}", rel(path), i + 1));
                    }
                }
            }
        }
        assert!(
            unknown.is_empty(),
            "error_note::render with an id not in the surface list (add the surface to the rule and the list): {unknown:?}"
        );
    }
}
