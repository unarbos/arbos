//! The reaction capsule (Grok Bot's shape, Jacob 2026-10-23): a dark rounded
//! pill with 😂 👍 👎 ❤️ 🎉 and an add-emoji control that opens the system
//! emoji picker. Opened from the smiley on a bubble's hover rail; a pick
//! toggles my reaction on that message (`AppState::toggle_reaction`, a
//! reaction row in the chat doc, synced like any other op) and closes it.
//! The chips under the bubble (`RowKind::Reactions`) show the result.

use gpui::{
    div, point, prelude::*, px, size, Context, Div, Entity, Focusable, Pixels, Point, Size, Window,
};

use crate::model::{Id, REACTION_PALETTE};
use crate::ui::composer::ComposerInput;
use crate::ui::highlight;
use crate::ui::icons::icon;
use crate::ui::shell::Shell;
use crate::ui::theme::Theme;

/// Capsule geometry (pt): one emoji cell, the pill's padding, the gap to the
/// anchor (the smiley) and the window margin.
pub const CELL: f32 = 36.0;
pub const CELL_GAP: f32 = 2.0;
pub const PAD: f32 = 4.0;
pub const EMOJI_SIZE: f32 = 22.0;
pub const ANCHOR_GAP: f32 = 8.0;
pub const EDGE_MARGIN: f32 = 8.0;
/// Width of the "pick any emoji" field that replaces the palette while the
/// system picker is open.
const PICK_FIELD_WIDTH: f32 = 160.0;

/// The capsule's size: the palette cells plus the add-emoji cell.
pub fn capsule_size() -> Size<Pixels> {
    let cells = REACTION_PALETTE.len() as f32 + 1.0;
    size(
        px(2.0 * PAD + cells * CELL + (cells - 1.0) * CELL_GAP),
        px(2.0 * PAD + CELL),
    )
}

/// Top-left of a capsule of `size` for a smiley clicked at `anchor`: centred
/// over the click and sitting `ANCHOR_GAP` above it; dropped below the click
/// when there is no room above; clamped to the window margin left / right.
pub fn place(anchor: Point<Pixels>, size: Size<Pixels>, viewport: Size<Pixels>) -> Point<Pixels> {
    let (ax, ay): (f32, f32) = (anchor.x.into(), anchor.y.into());
    let (w, h): (f32, f32) = (size.width.into(), size.height.into());
    let (vw, vh): (f32, f32) = (viewport.width.into(), viewport.height.into());
    let x = (ax - w / 2.0).clamp(EDGE_MARGIN, (vw - EDGE_MARGIN - w).max(EDGE_MARGIN));
    let above = ay - ANCHOR_GAP - h;
    let y = if above >= EDGE_MARGIN {
        above
    } else {
        (ay + ANCHOR_GAP).min((vh - EDGE_MARGIN - h).max(EDGE_MARGIN))
    };
    point(px(x), px(y))
}

/// The first emoji in `text`, if the text starts with one: what the system
/// picker (or a keyboard) typed into the pick field.
pub fn picked_emoji(text: &str) -> Option<String> {
    use unicode_segmentation::UnicodeSegmentation;
    let first = text.trim_start().graphemes(true).next()?;
    let is_emoji = first.chars().any(|c| {
        let u = c as u32;
        (0x1F000..=0x1FAFF).contains(&u)
            || (0x2600..=0x27BF).contains(&u)
            || (0x2B00..=0x2BFF).contains(&u)
            || (0x1F900..=0x1F9FF).contains(&u)
            || (0x2190..=0x21FF).contains(&u)
            || (0x3030..=0x303F).contains(&u)
            || u == 0x200D
            || u == 0xFE0F
    });
    is_emoji.then(|| first.to_owned())
}

/// `ARBOS_RIG_REACTION_CAPSULE=<x>,<y>`: open the capsule for the newest
/// message of the open chat at that window position (screenshot rigs).
pub fn rig_anchor() -> Option<Point<Pixels>> {
    let spec = std::env::var("ARBOS_RIG_REACTION_CAPSULE").ok()?;
    let (x, y) = spec.split_once(',')?;
    Some(point(
        px(x.trim().parse::<f32>().ok()?),
        px(y.trim().parse::<f32>().ok()?),
    ))
}

/// Opens macOS's character palette (Edit → Emoji & Symbols). It types into
/// the focused text input, so the caller focuses the pick field first.
#[cfg(target_os = "macos")]
fn open_system_emoji_picker() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    NSApplication::sharedApplication(mtm).orderFrontCharacterPalette(None);
}

#[cfg(not(target_os = "macos"))]
fn open_system_emoji_picker() {}

/// The open capsule, appended straight to the shell's root (like the message
/// menu: taffy places an absolute child relative to its parent).
pub fn render(shell: &Shell, window: &Window, cx: &mut Context<Shell>) -> Option<Div> {
    let (chat_id, message_id, at) = shell.reaction_capsule.clone()?;
    let theme = Theme::of(cx).clone();
    let origin = place(at, capsule_size(), window.viewport_size());
    let me = format!("user:{}", shell.state.read(cx).me_id());
    let held: Vec<String> = shell
        .state
        .read(cx)
        .messages
        .get(&chat_id)
        .and_then(|list| list.iter().find(|m| m.id == message_id))
        .map(|m| {
            m.reaction_list()
                .into_iter()
                .filter(|(_, s)| s.contains(&me))
                .map(|(e, _)| e)
                .collect()
        })
        .unwrap_or_default();
    let mut bar = div().flex().items_center().gap(px(CELL_GAP)).p(px(PAD));
    if shell.reaction_pick_open {
        // The system picker is up: it types into this field.
        bar = bar
            .child(
                div()
                    .w(px(PICK_FIELD_WIDTH))
                    .h(px(CELL))
                    .flex()
                    .items_center()
                    .px(px(10.0))
                    .rounded_full()
                    .bg(theme.field)
                    .child(shell.reaction_pick.clone()),
            )
            .child(cell(
                "react-pick-close",
                icon("xmark", 14.0, theme.text_muted),
                false,
                &theme,
                cx.listener(|this, _, _, cx| {
                    this.close_reaction_capsule(cx);
                }),
            ));
    } else {
        for emoji in REACTION_PALETTE {
            let on = held.iter().any(|e| e == emoji);
            let (chat, msg) = (chat_id.clone(), message_id.clone());
            bar = bar.child(cell(
                format!("react-{emoji}"),
                div().text_size(px(EMOJI_SIZE)).child(emoji),
                on,
                &theme,
                cx.listener(move |this, _, _, cx| {
                    this.react(chat.clone(), msg.clone(), emoji.to_string(), cx);
                }),
            ));
        }
        bar = bar.child(cell(
            "react-more",
            div()
                .relative()
                .size(px(20.0))
                .child(icon("face-smile", 20.0, theme.text_muted))
                .child(
                    div()
                        .absolute()
                        .right(px(-5.0))
                        .top(px(-5.0))
                        .size(px(12.0))
                        .rounded_full()
                        .bg(theme.card())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon("plus", 10.0, theme.text_muted)),
                ),
            false,
            &theme,
            cx.listener(|this, _, window, cx| {
                this.open_reaction_pick(window, cx);
            }),
        ));
    }
    Some(
        div()
            .absolute()
            .left(origin.x)
            .top(origin.y)
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .rounded_full()
                    .bg(theme.card())
                    .border_1()
                    .border_color(theme.border_strong)
                    .shadow_lg()
                    .child(bar),
            ),
    )
}

fn cell(
    id: impl Into<gpui::ElementId>,
    content: impl IntoElement,
    on: bool,
    theme: &Theme,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .size(px(CELL))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .when(on, |el| el.bg(Theme::with_alpha(theme.accent, 0.22)))
        .map(|el| highlight::button(el, theme))
        .child(content)
        .on_click(on_click)
}

impl Shell {
    pub(crate) fn open_reaction_capsule(
        &mut self,
        chat_id: Id,
        message_id: Id,
        at: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.close_popovers(cx);
        self.reaction_capsule = Some((chat_id, message_id, at));
        self.reaction_pick_open = false;
        cx.notify();
    }

    pub(crate) fn close_reaction_capsule(&mut self, cx: &mut Context<Self>) {
        if self.reaction_capsule.is_some() || self.reaction_pick_open {
            self.reaction_capsule = None;
            self.reaction_pick_open = false;
            self.reaction_pick.update(cx, |input, cx| input.clear(cx));
            cx.notify();
        }
    }

    fn react(&mut self, chat_id: Id, message_id: Id, emoji: String, cx: &mut Context<Self>) {
        self.close_reaction_capsule(cx);
        self.state.update(cx, |s, cx| {
            s.toggle_reaction(chat_id, message_id, emoji, cx)
        });
    }

    fn open_reaction_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reaction_pick_open = true;
        self.reaction_pick.update(cx, |input, cx| input.clear(cx));
        let handle = self.reaction_pick.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        open_system_emoji_picker();
        cx.notify();
    }

    /// The pick field changed (system picker or keyboard): the first emoji
    /// typed is the reaction.
    pub(crate) fn reaction_pick_changed(&mut self, cx: &mut Context<Self>) {
        if !self.reaction_pick_open {
            return;
        }
        let text = self.reaction_pick.read(cx).text().to_owned();
        let Some(emoji) = picked_emoji(&text) else {
            return;
        };
        if let Some((chat, msg, _)) = self.reaction_capsule.clone() {
            self.react(chat, msg, emoji, cx);
        }
    }
}

/// The pick field: one line, no placeholder noise, submits nothing.
pub(crate) fn new_pick_input(cx: &mut Context<Shell>) -> Entity<ComposerInput> {
    let input = cx.new(ComposerInput::new);
    input.update(cx, |input, _| {
        input.set_placeholder("Pick an emoji…");
        input.set_font_size(EMOJI_SIZE - 4.0);
    });
    cx.observe(&input, |this, _, cx| this.reaction_pick_changed(cx))
        .detach();
    input
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIN: (f32, f32) = (1180.0, 780.0);

    fn inside(origin: Point<Pixels>, size: Size<Pixels>, viewport: Size<Pixels>) -> bool {
        let (x, y): (f32, f32) = (origin.x.into(), origin.y.into());
        let (w, h): (f32, f32) = (size.width.into(), size.height.into());
        let (vw, vh): (f32, f32) = (viewport.width.into(), viewport.height.into());
        x >= 0.0 && y >= 0.0 && x + w <= vw && y + h <= vh
    }

    #[test]
    fn palette_is_groks_five() {
        assert_eq!(REACTION_PALETTE, ["😂", "👍", "👎", "❤️", "🎉"]);
    }

    #[test]
    fn capsule_sits_centred_above_the_smiley() {
        let viewport = size(px(WIN.0), px(WIN.1));
        let s = capsule_size();
        let o = place(point(px(600.0), px(400.0)), s, viewport);
        assert_eq!(o.x + s.width / 2.0, px(600.0));
        assert_eq!(o.y + s.height + px(ANCHOR_GAP), px(400.0));
        assert!(inside(o, s, viewport));
    }

    #[test]
    fn capsule_fits_at_the_right_edge() {
        let viewport = size(px(WIN.0), px(WIN.1));
        let s = capsule_size();
        let o = place(point(px(WIN.0 - 10.0), px(400.0)), s, viewport);
        assert!(inside(o, s, viewport), "{o:?} {s:?}");
        assert_eq!(o.x + s.width, px(WIN.0 - EDGE_MARGIN));
    }

    #[test]
    fn capsule_drops_below_near_the_top() {
        let viewport = size(px(WIN.0), px(WIN.1));
        let s = capsule_size();
        let o = place(point(px(300.0), px(20.0)), s, viewport);
        assert!(inside(o, s, viewport), "{o:?} {s:?}");
        assert_eq!(o.y, px(20.0 + ANCHOR_GAP));
    }

    #[test]
    fn capsule_fits_in_the_bottom_left_corner() {
        let viewport = size(px(WIN.0), px(WIN.1));
        let s = capsule_size();
        let o = place(point(px(2.0), px(WIN.1 - 2.0)), s, viewport);
        assert!(inside(o, s, viewport), "{o:?} {s:?}");
    }

    #[test]
    fn picked_emoji_takes_the_first_grapheme_only_when_it_is_an_emoji() {
        assert_eq!(picked_emoji("🎉").as_deref(), Some("🎉"));
        assert_eq!(picked_emoji("❤️ love").as_deref(), Some("❤️"));
        assert_eq!(picked_emoji("👍🏽").as_deref(), Some("👍🏽"));
        assert_eq!(picked_emoji("ok"), None);
        assert_eq!(picked_emoji(""), None);
    }
}
