//! The copy sheet over Settings → Agent access link: the one screen a new
//! link's URL exists on.
//!
//! Geometry is cloned to the point from `connect_agent::reveal_sheet` — same
//! 54.5 pt header, same uppercase section label, same radius-12 card inside a
//! radius-16 frosted panel, same Copy capsule that flips to a green "Copied",
//! same Done pill — because this is the same job as that sheet and the two
//! must not drift.
//!
//! Three things differ, all from Jacob's one-copy decision
//! (`internal/settings-agent-access-link-one-copy.md`):
//!
//! 1. **One copy row, not two.** The relay never hands the owner's device a
//!    bearer (`spec` §6.1), so there is no token row to draw. The link is the
//!    credential.
//! 2. **No leading icon tile on that row.** In the precedent the tile told
//!    Link from Key; with one row it identifies nothing, and its 50 pt was
//!    what truncated the URL.
//! 3. **Done waits for the copy.** A closed sheet is unrecoverable, so Done
//!    stays in `create_entity`'s disabled tokens until Copy has been pressed.
//!
//! The sheet does not follow the Glass / Matte surface setting, because
//! `create_entity::palette` branches on the appearance only. That is the
//! app's behaviour in every sheet of this family today.

use gpui::{div, prelude::*, px, ClipboardItem, Context, Div, FontWeight, MouseButton};

use crate::model::AgentLinkReveal;
use crate::ui::agent_links::{self, Scope};
use crate::ui::create_entity::{self, hex, palette};
use crate::ui::frost::{frosted, PANEL_BLUR};
use crate::ui::highlight;
use crate::ui::sf_symbols;
use crate::ui::shell::Shell;
use crate::ui::theme::Theme;

const WIDTH: f32 = create_entity::WIDTH;
const RADIUS: f32 = 16.0;
const HEADER: f32 = 54.5;
const INSET: f32 = 16.5;

/// The sheet: the link to copy, what opening it does, what the link may do,
/// and Done.
pub fn sheet(reveal: &AgentLinkReveal, cx: &mut Context<Shell>) -> Div {
    let theme = Theme::of(cx).clone();
    let p = palette(&theme);
    let scope = Scope::from_wire(&reveal.scope);
    let copied = reveal.copied;

    let header = div()
        .relative()
        .flex_none()
        .w_full()
        .h(px(HEADER))
        .border_b_1()
        .border_color(p.line)
        .child(
            div()
                .absolute()
                .left(px(17.5))
                .top(px(16.0))
                .text_size(px(14.2))
                .line_height(px(20.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(p.title)
                .child("New agent access link"),
        )
        .child(
            div()
                .id("agent-link-reveal-close")
                .absolute()
                .right(px(16.5))
                .top(px(15.0))
                .size(px(22.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .map(|el| highlight::button(el, &theme))
                .child(sf_symbols::glyph("xmark", 10.0, hex(0xD2D2D2)))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state.update(cx, |s, cx| s.close_agent_link_reveal(cx));
                })),
        );

    let section = |text: &str| {
        div()
            .px(px(INSET + 12.0))
            .pt(px(14.0))
            .pb(px(6.0))
            .text_size(px(11.5))
            .text_color(p.placeholder)
            .child(text.to_uppercase())
    };
    let card = |children: Vec<gpui::AnyElement>| {
        div()
            .mx(px(INSET))
            .rounded(px(12.0))
            .bg(p.cancel_bg)
            .border_1()
            .border_color(p.line)
            .flex()
            .flex_col()
            .children(children)
    };
    let footnote = |text: &str| {
        div()
            .px(px(INSET + 12.0))
            .pt(px(8.0))
            .text_size(px(11.5))
            .line_height(px(15.0))
            .text_color(p.placeholder)
            .child(text.to_owned())
    };

    let full = reveal.url.clone();
    let copy_row = div()
        .id("agent-link-copy")
        .debug_selector(|| "agent-link-copy".into())
        .min_h(px(46.0))
        .flex()
        .items_center()
        .gap(px(12.0))
        .px(px(14.0))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_size(px(11.0))
                        .line_height(px(13.0))
                        .text_color(p.placeholder)
                        .child("Link"),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .line_height(px(16.0))
                        .font_family("Menlo")
                        .text_color(p.title)
                        .truncate()
                        .child(agent_links::short_url(&reveal.url)),
                ),
        )
        .child(
            div()
                .id("agent-link-copy-button")
                .h(px(26.0))
                .px(px(10.0))
                .rounded_full()
                .bg(if copied {
                    Theme::with_alpha(theme.online, 0.18)
                } else {
                    p.ring
                })
                .flex()
                .items_center()
                .gap(px(5.0))
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if copied { theme.online } else { p.title })
                .cursor_pointer()
                .hover(|s| s.opacity(0.85))
                .active(|s| s.opacity(0.7))
                .child(sf_symbols::glyph(
                    if copied { "checkmark" } else { "doc.on.doc" },
                    13.0,
                    if copied { theme.online } else { p.title },
                ))
                .child(if copied { "Copied" } else { "Copy" })
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(full.clone()));
                    this.state.update(cx, |s, cx| s.mark_agent_link_copied(cx));
                })),
        );

    let scope_ink = if scope.is_danger() {
        theme.danger
    } else {
        p.title
    };
    let scope_row = div()
        .min_h(px(46.0))
        .flex()
        .items_center()
        .gap(px(12.0))
        .px(px(14.0))
        .py(px(10.0))
        .child(
            div()
                .size(px(28.0))
                .flex_shrink_0()
                .rounded(px(8.0))
                // The tile stays here, unlike on the copy row: it is not an
                // identifier, its fill is what carries the warning's colour.
                .bg(if scope.is_danger() {
                    theme.danger_soft
                } else {
                    p.ring
                })
                .flex()
                .items_center()
                .justify_center()
                .child(sf_symbols::glyph(
                    if scope.is_danger() {
                        "arrowshape.turn.up.left"
                    } else {
                        "eye"
                    },
                    15.0,
                    scope_ink,
                )),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(12.5))
                        .line_height(px(16.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(scope_ink)
                        .child(scope.title()),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .line_height(px(15.0))
                        .text_color(p.placeholder)
                        .child(agent_links::restatement(scope, &reveal.expires_in)),
                ),
        );

    let body = div()
        .flex()
        .flex_col()
        .w_full()
        .pb(px(16.0))
        .child(section("Give this to your agent"))
        .child(card(vec![copy_row.into_any_element()]))
        .child(footnote(agent_links::ONCE_NOTE))
        .child(footnote(agent_links::RACE_NOTE))
        .child(section("Scope"))
        .child(card(vec![scope_row.into_any_element()]));

    let done = div()
        .id("agent-link-reveal-done")
        .debug_selector(|| "agent-link-reveal-done".into())
        .h(px(27.0))
        .w(px(67.0))
        .rounded_full()
        .bg(if copied { theme.primary } else { p.create_bg })
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(14.2))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if copied {
            theme.on_primary
        } else {
            p.create_text
        })
        .child("Done")
        .when(copied, |el| {
            el.cursor_pointer()
                .hover(|s| s.opacity(0.88))
                .active(|s| s.opacity(0.76))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state.update(cx, |s, cx| s.close_agent_link_reveal(cx));
                }))
        });
    let footer = div()
        .flex_none()
        .w_full()
        .h(px(56.5))
        .border_t_1()
        .border_color(p.line)
        .flex()
        .items_center()
        .justify_end()
        .pr(px(12.5))
        .child(done);

    let sheet = div()
        .id("agent-link-reveal-card")
        .debug_selector(|| "agent-link-reveal-card".into())
        .relative()
        .w(px(WIDTH))
        .max_w_full()
        .max_h_full()
        .flex()
        .flex_col()
        .rounded(px(RADIUS))
        .bg(p.bg)
        .border_1()
        .border_color(p.line)
        .shadow_lg()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(header)
        .child(body)
        .child(footer);
    // No click-outside dismissal, unlike the connect sheet: this is the only
    // place the URL exists, so it comes down by × or Done alone.
    div()
        .absolute()
        .inset_0()
        .occlude()
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .flex()
        .items_center()
        .justify_center()
        .p(px(24.0))
        .bg(Theme::with_alpha(theme.bg, 0.55))
        .child(frosted(RADIUS, PANEL_BLUR, sheet))
}
