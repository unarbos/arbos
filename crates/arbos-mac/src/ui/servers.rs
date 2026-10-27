//! The server sheets (D88 rules 7 and 8), the New Message sheet's shape: Cancel · title ·
//! Create, a field in a card, a line of help. **New server** asks for a name and lands on
//! `#general`; **Join a server** takes an invite link or its code; **New channel** (owner/admin,
//! from the `+` inside a server) takes a name — letters, digits and dashes — and the "Admins
//! only post" switch. A refusal from the relay is the shared error note at the top of the
//! sheet's body (Jacob, 2026-10-26).
//!
//! None of this appears when the relay answers `servers_off` (`ARBOS_MATRIX=off`, the remote
//! kill switch): the rows, the sheets and the server list stay hidden on every device until
//! the switch is back on and the app refetches `GET /servers`.

use gpui::{div, prelude::*, px, Context, Div, Focusable as _};

use crate::app::{AppState, ServerSheet};
use crate::ui::error_note;
use crate::ui::glass;
use crate::ui::highlight::{self, Shape};
use crate::ui::profiles::kit;
use crate::ui::shell::Shell;
use crate::ui::theme::{Theme, PANEL_RADIUS, TEXT_SMALL, TEXT_TINY};

/// The open sheet, over everything; a click on the dimmed area cancels it.
pub fn sheet(shell: &Shell, sheet: &ServerSheet, cx: &mut Context<Shell>) -> Div {
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let busy = state.server_busy;
    let error = state.server_error.clone();
    let typed = shell.server_input.read(cx).text().trim().to_string();
    let restricted = shell.new_channel_restricted;
    let (title, action, help) = match sheet {
        ServerSheet::NewServer => (
            "New server",
            "Create",
            "A server has channels and an invite link. You start as its owner, in #general.",
        ),
        ServerSheet::JoinServer => (
            "Join a server",
            "Join",
            "Paste the invite link someone sent you, or just its code.",
        ),
        ServerSheet::NewChannel { .. } => (
            "New channel",
            "Create",
            "Letters, digits and dashes; spaces become dashes.",
        ),
    };
    let is_channel = matches!(sheet, ServerSheet::NewChannel { .. });
    let can_go = !busy
        && match sheet {
            ServerSheet::NewChannel { .. } => !AppState::channel_name_clean(&typed).is_empty(),
            ServerSheet::NewServer | ServerSheet::JoinServer => !typed.is_empty(),
        };

    let field = div()
        .flex()
        .items_center()
        .gap(px(10.0))
        .px(px(14.0))
        .py(px(10.0))
        .child(
            div()
                .flex_1()
                .h(px(38.0))
                .px(px(12.0))
                .flex()
                .items_center()
                .rounded(px(10.0))
                .bg(theme.field)
                .border_1()
                .border_color(theme.field_border)
                .child(shell.server_input.clone()),
        );
    let mut group = glass::group(&theme)
        .flex_shrink_0()
        .mx(px(16.0))
        .child(field);
    if is_channel {
        group = group.child(kit::divider(&theme)).child(
            div()
                .id("new-channel-restricted")
                .flex()
                .items_center()
                .gap(px(12.0))
                .px(px(14.0))
                .py(px(10.0))
                .cursor_pointer()
                .map(|el| highlight::row(el, false, Shape::Nested(PANEL_RADIUS), &theme))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(TEXT_SMALL + 1.0))
                                .text_color(theme.text)
                                .child("Admins only post"),
                        )
                        .child(
                            div()
                                .text_size(px(TEXT_TINY))
                                .text_color(theme.text_muted)
                                .child("Everyone reads; owners and admins write."),
                        ),
                )
                .child(glass::switch(restricted, &theme))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.new_channel_restricted = !this.new_channel_restricted;
                    let handle = this.server_input.read(cx).focus_handle(cx);
                    window.focus(&handle, cx);
                    cx.notify();
                })),
        );
    }
    // The note (a refused create / join) goes first in the body, under the sheet header,
    // before the field — the shared error element and its one place in a sheet.
    let note = error.map(|slot| {
        div().mx(px(16.0)).child(error_note::render(
            "server-sheet-error",
            &slot.error,
            slot.details_open,
            error_note::Style::Inline,
            &theme,
            error_note::Handlers::new()
                .try_again(cx.listener(|this, _, window, cx| {
                    this.submit_server_input(window, cx);
                }))
                .toggle_details(cx.listener(|this, _, _, cx| {
                    this.state.update(cx, |s, cx| {
                        if let Some(slot) = s.server_error.as_mut() {
                            slot.toggle_details();
                        }
                        cx.notify();
                    });
                })),
        ))
    });
    let body = div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .pb(px(18.0))
        .children(note)
        .child(group)
        .child(
            div()
                .px(px(30.0))
                .text_size(px(TEXT_TINY))
                .text_color(theme.text_faint)
                .child(help),
        );

    let left = glass::sheet_button("server-sheet-cancel", "Cancel", &theme)
        .on_click(cx.listener(|this, _, window, cx| this.close_server_sheet(window, cx)));
    let label = if busy {
        match sheet {
            ServerSheet::JoinServer => "Joining…",
            ServerSheet::NewServer | ServerSheet::NewChannel { .. } => "Creating…",
        }
    } else {
        action
    };
    let right = glass::primary_button("server-sheet-go", label, &theme)
        .h(px(34.0))
        .when(!can_go, |el| el.opacity(0.45))
        .on_click(cx.listener(move |this, _, window, cx| {
            if !can_go {
                return;
            }
            this.submit_server_input(window, cx);
        }));
    let header = glass::sheet_header(title, Some(left), Some(right), &theme);
    glass::sheet(420.0, &theme, header, body).on_mouse_down(
        gpui::MouseButton::Left,
        cx.listener(|this, _, window, cx| this.close_server_sheet(window, cx)),
    )
}
