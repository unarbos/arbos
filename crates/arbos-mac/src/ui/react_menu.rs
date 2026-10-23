//! The quick-react emoji bar opened from the bubble's hover face-smile
//! glyph (Grok Mac). Right-click stays Copy / Reply / Delete (`message_menu`);
//! this bar is how people and agents get a 👍 on a message again after #1370
//! cut the emoji row from the context menu.
//!
//! Placement reuses [`message_menu::place`] so the bar never draws past the
//! window edge — the bug that killed the old right-click react bar.

use gpui::{div, prelude::*, px, size, Div, Pixels, Point, SharedString, Size, Window};

use crate::model::REACTION_PALETTE;
use crate::ui::message_menu;
use crate::ui::shell::Shell;
use crate::ui::theme::Theme;

/// One emoji cell.
const CELL: f32 = 32.0;
/// Gap between cells.
const GAP: f32 = 2.0;
/// Padding around the row of cells.
const PAD: f32 = 4.0;

/// Size of the six-emoji pill.
pub fn bar_size() -> Size<Pixels> {
    let n = REACTION_PALETTE.len() as f32;
    let width = 2.0 * PAD + n * CELL + (n - 1.0).max(0.0) * GAP;
    let height = 2.0 * PAD + CELL;
    size(px(width), px(height))
}

/// Top-left of the bar at `anchor` (the click), kept inside the window.
pub fn place(anchor: Point<Pixels>, viewport: Size<Pixels>) -> Point<Pixels> {
    message_menu::place(anchor, bar_size(), viewport)
}

/// The open bar, appended straight to the shell's root.
pub fn render(shell: &Shell, window: &Window, cx: &mut Context<Shell>) -> Option<Div> {
    let (chat_id, message_id, at) = shell.react_menu.clone()?;
    let theme = Theme::of(cx).clone();
    let origin = place(at, window.viewport_size());
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
    let mut bar = div().flex().items_center().gap(px(GAP)).p(px(PAD));
    for emoji in REACTION_PALETTE {
        let on = held.iter().any(|e| e == emoji);
        let (chat, msg) = (chat_id.clone(), message_id.clone());
        bar = bar.child(
            div()
                .id(SharedString::from(format!("react-{emoji}")))
                .size(px(CELL))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .when(on, |el| el.bg(Theme::with_alpha(theme.accent, 0.22)))
                .hover(|s| s.bg(theme.surface_2))
                .text_size(px(18.0))
                .child(emoji)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.react_menu = None;
                    let (chat, msg) = (chat.clone(), msg.clone());
                    this.state.update(cx, |s, cx| {
                        s.toggle_reaction(chat, msg, emoji.to_string(), cx)
                    });
                    cx.notify();
                })),
        );
    }
    Some(
        div()
            .absolute()
            .left(origin.x)
            .top(origin.y)
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::message_menu::MENU_EDGE_MARGIN;
    use gpui::point;

    const WIN: (f32, f32) = (1180.0, 780.0);

    fn inside(origin: Point<Pixels>, size: Size<Pixels>, viewport: Size<Pixels>) -> bool {
        let (x, y): (f32, f32) = (origin.x.into(), origin.y.into());
        let (w, h): (f32, f32) = (size.width.into(), size.height.into());
        let (vw, vh): (f32, f32) = (viewport.width.into(), viewport.height.into());
        x >= 0.0 && y >= 0.0 && x + w <= vw && y + h <= vh
    }

    #[test]
    fn palette_is_the_six_quick_reacts() {
        assert_eq!(REACTION_PALETTE, ["👍", "❤️", "😂", "😮", "😢", "🙏"]);
    }

    #[test]
    fn bar_fits_at_a_right_edge_bubble() {
        let viewport = size(px(WIN.0), px(WIN.1));
        let size = bar_size();
        let origin = place(point(px(WIN.0 - 12.0), px(300.0)), viewport);
        assert!(inside(origin, size, viewport), "{origin:?} {size:?}");
        assert!(f32::from(origin.x) >= MENU_EDGE_MARGIN);
    }

    #[test]
    fn bar_fits_in_the_bottom_right_corner() {
        let viewport = size(px(WIN.0), px(WIN.1));
        let size = bar_size();
        let origin = place(point(px(WIN.0 - 2.0), px(WIN.1 - 2.0)), viewport);
        assert!(inside(origin, size, viewport), "{origin:?} {size:?}");
    }
}
