//! The message bubble's menu — right-click or the hover rail's ⋯ on the Mac,
//! under the capsule on the iPhone's long-press (`MessageMenu.swift`):
//! **Copy, Reply, Forward, Reply in DM, Delete** and nothing else (Jacob,
//! #1370; Reply in DM joined with #1835, as on the iPhone; **Remove preview** joined with
//! #2065, only on a message that shows a link card the user may edit). Reactions live in
//! the capsule (`reactions.rs`), never in this menu. One builder for every
//! bubble, and one placement rule: the menu always sits fully inside the
//! window — flipped to the other side of the click near the right or bottom
//! edge, clamped to the edge margin when neither side fits.

use gpui::{div, point, prelude::*, px, size, Div, Pixels, Point, Size, Window};

use crate::model::Id;
use crate::ui::glass::{
    self, MENU_CARD_BORDER, MENU_CARD_PADDING, MENU_DIVIDER_HEIGHT, MENU_GAP, MENU_ROW_HEIGHT,
};
use crate::ui::shell::Shell;
use crate::ui::theme::Theme;

pub const MESSAGE_MENU_WIDTH: f32 = 180.0;
/// Space kept between the menu and every window edge.
pub const MENU_EDGE_MARGIN: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageMenuItem {
    Copy,
    Reply,
    Forward,
    /// Another person's message in a group / shared chat: answer them
    /// privately, with the quote and its chat name on the DM bubble (#1835).
    ReplyInDm,
    /// The message shows a link preview card and the user may edit the message: drop the
    /// card for everyone ([feedback link-embeds-mac-2026-10-27]).
    RemovePreview,
    Delete,
}

impl MessageMenuItem {
    pub fn label(self) -> &'static str {
        match self {
            MessageMenuItem::Copy => "Copy",
            MessageMenuItem::Reply => "Reply",
            MessageMenuItem::Forward => "Forward",
            MessageMenuItem::ReplyInDm => "Reply in DM",
            MessageMenuItem::RemovePreview => "Remove preview",
            MessageMenuItem::Delete => "Delete",
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            MessageMenuItem::Copy => "copy",
            MessageMenuItem::Reply => "reply",
            MessageMenuItem::Forward => "forward",
            MessageMenuItem::ReplyInDm => "bubble",
            MessageMenuItem::RemovePreview => "link",
            MessageMenuItem::Delete => "trash",
        }
    }

    pub fn danger(self) -> bool {
        matches!(self, MessageMenuItem::Delete)
    }
}

/// The menu's items, in order. One list for every bubble: the set is the
/// contract, not a per-row choice.
pub const ITEMS: [MessageMenuItem; 6] = [
    MessageMenuItem::Copy,
    MessageMenuItem::Reply,
    MessageMenuItem::Forward,
    MessageMenuItem::ReplyInDm,
    MessageMenuItem::RemovePreview,
    MessageMenuItem::Delete,
];

/// The items for one bubble: Copy, Reply and Forward always; Reply in DM only
/// on another person's message (`can_dm`: a human who is not me — my own rows
/// and an agent's have no peer to DM); Delete only when the relay would let
/// this user delete it (`model::may_delete_message`); Remove preview only on a message that
/// shows a link card (`has_preview`) the user may delete.
pub fn items_for(can_delete: bool, can_dm: bool, has_preview: bool) -> Vec<MessageMenuItem> {
    ITEMS
        .into_iter()
        .filter(|i| {
            (can_delete || !i.danger())
                && (can_dm || *i != MessageMenuItem::ReplyInDm)
                && ((can_delete && has_preview) || *i != MessageMenuItem::RemovePreview)
        })
        .collect()
}

/// Divider before the destructive item.
const DIVIDERS: usize = 1;

/// The menu's size, from the same constants `glass::menu_card` /
/// `menu_row` / `menu_divider` lay it out with.
#[cfg_attr(not(test), allow(dead_code))]
pub fn menu_size() -> Size<Pixels> {
    menu_size_for(ITEMS.len() - 1)
}

/// The size of a menu with `n` rows (the full set less the items a bubble is not offered).
pub fn menu_size_for(n: usize) -> Size<Pixels> {
    let rows = n as f32;
    let dividers = DIVIDERS as f32;
    let children = rows + dividers;
    let height = 2.0 * (MENU_CARD_PADDING + MENU_CARD_BORDER)
        + rows * MENU_ROW_HEIGHT
        + dividers * MENU_DIVIDER_HEIGHT
        + (children - 1.0) * MENU_GAP;
    size(px(MESSAGE_MENU_WIDTH), px(height))
}

/// Top-left of a menu of `size` opened at `anchor` (the click) in a window of
/// `viewport`: at the click when it fits, flipped to the other side of the
/// click when it would cross the right or bottom edge, and clamped to the
/// edge margin when neither side fits.
pub fn place(anchor: Point<Pixels>, size: Size<Pixels>, viewport: Size<Pixels>) -> Point<Pixels> {
    point(
        px(place_axis(
            anchor.x.into(),
            size.width.into(),
            viewport.width.into(),
        )),
        px(place_axis(
            anchor.y.into(),
            size.height.into(),
            viewport.height.into(),
        )),
    )
}

fn place_axis(anchor: f32, extent: f32, limit: f32) -> f32 {
    let margin = MENU_EDGE_MARGIN;
    let start = if anchor + extent + margin <= limit {
        anchor
    } else if anchor - extent >= margin {
        anchor - extent
    } else {
        limit - margin - extent
    };
    start.max(margin)
}

/// Rect of the menu as placed: `(origin, size)`, for the callers (and tests)
/// that check it against the window.
#[cfg_attr(not(test), allow(dead_code))]
pub fn placed_rect(anchor: Point<Pixels>, viewport: Size<Pixels>) -> (Point<Pixels>, Size<Pixels>) {
    let size = menu_size();
    (place(anchor, size, viewport), size)
}

/// `ARBOS_RIG_MESSAGE_MENU=<x>,<y>`: open the menu for the newest message of
/// the open chat at that window position (screenshot rigs; posted pointer
/// events never reach the app in the EC2 Mac's GUI session).
pub fn rig_anchor() -> Option<Point<Pixels>> {
    let spec = std::env::var("ARBOS_RIG_MESSAGE_MENU").ok()?;
    let (x, y) = spec.split_once(',')?;
    Some(point(
        px(x.trim().parse::<f32>().ok()?),
        px(y.trim().parse::<f32>().ok()?),
    ))
}

/// The open menu, appended straight to the shell's root (taffy places an
/// absolute child relative to its parent; a static wrapper would push it off).
pub fn render(shell: &Shell, window: &Window, cx: &mut Context<Shell>) -> Option<Div> {
    let (chat_id, message_id, at) = shell.message_menu.clone()?;
    let theme = Theme::of(cx).clone();
    let can_delete = {
        let state = shell.state.read(cx);
        let space_id = state.space_of_chat(&chat_id);
        match (
            state.project(&space_id),
            state
                .messages_of(&chat_id)
                .iter()
                .find(|m| m.id == message_id),
        ) {
            (Some(space), Some(message)) => {
                crate::model::may_delete_message(space, &state.me_id(), &state.me_handle(), message)
            }
            _ => false,
        }
    };
    let (can_dm, has_preview) = {
        let state = shell.state.read(cx);
        let me = format!("user:{}", state.me_id());
        let message = state
            .messages_of(&chat_id)
            .iter()
            .find(|m| m.id == message_id);
        (
            message.is_some_and(|m| {
                m.sender.starts_with("user:")
                    && m.sender != me
                    && m.sender_handle.as_deref().is_some_and(|h| !h.is_empty())
            }),
            message.is_some_and(|m| crate::ui::transcript::link_preview_of(m).is_some()),
        )
    };
    let items = items_for(can_delete, can_dm, has_preview);
    let origin = place(at, menu_size_for(items.len()), window.viewport_size());
    let mut body = div().flex().flex_col().gap(px(MENU_GAP));
    for item in items {
        if item.danger() {
            body = body.child(glass::menu_divider(&theme));
        }
        let (chat, msg) = (chat_id.clone(), message_id.clone());
        body = body.child(
            glass::menu_row(
                format!("message-menu-{}", item.label().to_ascii_lowercase()),
                item.glyph(),
                item.label(),
                item.danger(),
                &theme,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.message_menu = None;
                run(this, item, chat.clone(), msg.clone(), cx);
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
            .child(glass::frosted_menu(MESSAGE_MENU_WIDTH, &theme, body)),
    )
}

fn run(
    shell: &mut Shell,
    item: MessageMenuItem,
    chat_id: Id,
    message_id: Id,
    cx: &mut Context<Shell>,
) {
    match item {
        MessageMenuItem::Copy => {
            shell
                .transcript
                .update(cx, |t, cx| t.copy_message(&message_id, cx));
        }
        MessageMenuItem::Reply => {
            shell
                .state
                .update(cx, |s, cx| s.set_reply_to(chat_id, message_id, cx));
        }
        MessageMenuItem::Forward => {
            shell
                .state
                .update(cx, |s, cx| s.open_forward_message(chat_id, message_id, cx));
        }
        MessageMenuItem::ReplyInDm => {
            shell
                .state
                .update(cx, |s, cx| s.reply_in_dm(chat_id, message_id, cx));
        }
        MessageMenuItem::RemovePreview => {
            shell
                .state
                .update(cx, |s, cx| s.remove_link_preview(chat_id, message_id, cx));
        }
        MessageMenuItem::Delete => {
            // Deletes at once, for everyone (Jacob, feedback
            // remove-delete-confirms-2026-10-24: no "Delete message?" card). The
            // menu only offers Delete where #1526's rule holds.
            shell
                .state
                .update(cx, |s, cx| s.delete_message(chat_id, message_id, cx));
        }
    }
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
    fn items_are_exactly_copy_reply_forward_reply_in_dm_delete() {
        let labels: Vec<&str> = ITEMS.iter().map(|i| i.label()).collect();
        assert_eq!(
            labels,
            [
                "Copy",
                "Reply",
                "Forward",
                "Reply in DM",
                "Remove preview",
                "Delete"
            ]
        );
        // Forward and Reply in DM sit above the divider, with the plain actions; Delete alone is red.
        assert!(!MessageMenuItem::Forward.danger());
        assert!(!MessageMenuItem::ReplyInDm.danger());
        // No emoji row: every label is plain ASCII text.
        assert!(ITEMS.iter().all(|i| i.label().is_ascii()));
    }

    #[test]
    fn delete_and_reply_in_dm_are_offered_only_when_allowed() {
        let labels = |v: Vec<MessageMenuItem>| v.iter().map(|i| i.label()).collect::<Vec<_>>();
        assert_eq!(
            labels(items_for(true, true, false)),
            ["Copy", "Reply", "Forward", "Reply in DM", "Delete"]
        );
        assert_eq!(
            labels(items_for(false, true, false)),
            ["Copy", "Reply", "Forward", "Reply in DM"]
        );
        // My own row / an agent's: nobody to DM.
        assert_eq!(
            labels(items_for(true, false, false)),
            ["Copy", "Reply", "Forward", "Delete"]
        );
        assert_eq!(
            labels(items_for(false, false, false)),
            ["Copy", "Reply", "Forward"]
        );
        // A message with a link card the user may edit offers Remove preview above Delete;
        // a card on someone else's message (no delete right) does not.
        assert_eq!(
            labels(items_for(true, false, true)),
            ["Copy", "Reply", "Forward", "Remove preview", "Delete"]
        );
        assert_eq!(
            labels(items_for(false, true, true)),
            ["Copy", "Reply", "Forward", "Reply in DM"]
        );
    }

    #[test]
    fn menu_fits_at_a_right_edge_bubble() {
        // Jacob's own bubble hugs the right edge; the click is 12 pt from it.
        let viewport = size(px(WIN.0), px(WIN.1));
        let (origin, size) = placed_rect(point(px(WIN.0 - 12.0), px(300.0)), viewport);
        assert!(inside(origin, size, viewport), "{origin:?} {size:?}");
        // Flipped: the menu ends at the click, not clamped somewhere else.
        assert_eq!(origin.x + size.width, px(WIN.0 - 12.0));
        assert_eq!(origin.y, px(300.0));
    }

    #[test]
    fn menu_fits_at_a_bottom_edge_bubble() {
        let viewport = size(px(WIN.0), px(WIN.1));
        let (origin, size) = placed_rect(point(px(400.0), px(WIN.1 - 20.0)), viewport);
        assert!(inside(origin, size, viewport), "{origin:?} {size:?}");
        assert_eq!(origin.y + size.height, px(WIN.1 - 20.0));
        assert_eq!(origin.x, px(400.0));
    }

    #[test]
    fn menu_fits_in_the_bottom_right_corner() {
        let viewport = size(px(WIN.0), px(WIN.1));
        let (origin, size) = placed_rect(point(px(WIN.0 - 2.0), px(WIN.1 - 2.0)), viewport);
        assert!(inside(origin, size, viewport), "{origin:?} {size:?}");
    }

    #[test]
    fn menu_stays_at_the_click_when_it_fits() {
        let viewport = size(px(WIN.0), px(WIN.1));
        let (origin, _) = placed_rect(point(px(300.0), px(200.0)), viewport);
        assert_eq!(origin, point(px(300.0), px(200.0)));
    }

    #[test]
    fn menu_clamps_to_the_margin_at_the_top_left() {
        let viewport = size(px(WIN.0), px(WIN.1));
        let (origin, size) = placed_rect(point(px(0.0), px(0.0)), viewport);
        assert!(inside(origin, size, viewport));
        assert_eq!(origin, point(px(MENU_EDGE_MARGIN), px(MENU_EDGE_MARGIN)));
    }

    #[test]
    fn menu_clamps_when_the_window_is_short() {
        // A window shorter than twice the menu: neither side of a mid-height
        // click fits, so the menu hugs the bottom margin.
        let viewport = size(px(600.0), px(260.0));
        let (origin, size) = placed_rect(point(px(130.0), px(130.0)), viewport);
        assert!(size.height < viewport.height && size.height * 2.0 > viewport.height);
        assert!(inside(origin, size, viewport), "{origin:?} {size:?}");
    }
}
