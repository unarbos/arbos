//! Connect an agent (#2141, design `docs/outside-agents-spec.md` §11; mockups
//! `media/feedback/outside-agents/mock-mac-connect-{reveal,profile}.png`): the key reveal
//! sheet after the Create screen, and the Connection / Key cards the agent pane shows for an
//! outside agent. The key is shown once; Copy puts it on the clipboard; the sheet polls the
//! relay and flips to "Connected" when the agent first checks in. Waiting uses the app's one
//! working indicator (`cards::activity_spinner`), never a coloured status dot.

use gpui::{div, prelude::*, px, ClipboardItem, Context, Div, FontWeight, MouseButton};

use crate::app::ConnectReveal;
use crate::model::{now_ms, Id, OutsideInfo};
use crate::ui::create_entity::{self, hex, palette, Transport};
use crate::ui::frost::{frosted, PANEL_BLUR};
use crate::ui::highlight;
use crate::ui::motion::PulseClock;
use crate::ui::profiles::kit::{self, Leading};
use crate::ui::profiles::pane::Section;
use crate::ui::sf_symbols;
use crate::ui::shell::Shell;
use crate::ui::theme::Theme;

const WIDTH: f32 = create_entity::WIDTH;
const RADIUS: f32 = 16.0;
const HEADER: f32 = 54.5;
const INSET: f32 = 16.5;

/// A short form of a long key for the row: `arbk_14788fc5…54bcd`.
pub fn short_key(key: &str) -> String {
    if key.len() <= 22 {
        return key.to_owned();
    }
    format!("{}…{}", &key[..13], &key[key.len() - 5..])
}

/// `https://relay.arbos.life/asp/connect/01M…` → `relay.arbos.life/asp/connect/01M…`.
fn short_link(link: &str) -> String {
    link.trim_start_matches("https://")
        .trim_start_matches("http://")
        .to_owned()
}

/// The reveal sheet: name, the Link and Key rows with Copy, the one how-to line, the live
/// status, Done.
pub fn reveal_sheet(_shell: &Shell, reveal: &ConnectReveal, cx: &mut Context<Shell>) -> Div {
    let theme = Theme::of(cx).clone();
    let p = palette(&theme);
    let transport = Transport::from_wire(&reveal.transport);
    let connected = reveal.status.as_ref().is_some_and(|s| s.connected);
    let title = if reveal.rotated {
        format!("New key for {}", reveal.name)
    } else {
        format!("Connect {}", reveal.name)
    };

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
                .child(title),
        )
        .child(
            div()
                .id("connect-reveal-close")
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
                    this.state.update(cx, |s, cx| s.close_connect_reveal(cx));
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
    let copy_row = |id: &'static str,
                    glyph: &'static str,
                    label: &'static str,
                    shown: String,
                    full: String,
                    which: &'static str,
                    copied: bool,
                    cx: &mut Context<Shell>| {
        div()
            .id(id)
            .min_h(px(46.0))
            .flex()
            .items_center()
            .gap(px(12.0))
            .px(px(14.0))
            .child(
                div()
                    .size(px(28.0))
                    .flex_shrink_0()
                    .rounded(px(8.0))
                    .bg(p.ring)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(sf_symbols::glyph(glyph, 15.0, p.title)),
            )
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
                            .child(label),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .line_height(px(16.0))
                            .font_family("Menlo")
                            .text_color(p.title)
                            .truncate()
                            .child(shown),
                    ),
            )
            .child(
                div()
                    .id(gpui::ElementId::Name(gpui::SharedString::from(format!(
                        "{id}-copy"
                    ))))
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
                    .child(sf_symbols::glyph(
                        if copied { "checkmark" } else { "doc.on.doc" },
                        13.0,
                        if copied { theme.online } else { p.title },
                    ))
                    .child(if copied { "Copied" } else { "Copy" })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(full.clone()));
                        this.state.update(cx, |s, cx| {
                            if let Some(r) = s.connect_reveal.as_mut() {
                                r.copied = Some(which);
                            }
                            cx.notify();
                        });
                    })),
            )
    };
    let link_row = copy_row(
        "connect-link",
        "link",
        "Link",
        short_link(&reveal.link),
        reveal.link.clone(),
        "link",
        reveal.copied == Some("link"),
        cx,
    );
    let key_row = copy_row(
        "connect-key",
        "key",
        "Key",
        short_key(&reveal.key),
        reveal.key.clone(),
        "key",
        reveal.copied == Some("key"),
        cx,
    );
    let footnote = |text: String| {
        div()
            .px(px(INSET + 12.0))
            .pt(px(8.0))
            .text_size(px(11.5))
            .line_height(px(15.0))
            .text_color(p.placeholder)
            .child(text)
    };

    let status: gpui::AnyElement = if connected {
        div()
            .flex()
            .items_center()
            .gap(px(9.0))
            .px(px(14.0))
            .py(px(12.0))
            .text_size(px(13.0))
            .text_color(p.title)
            .child(sf_symbols::glyph("checkmark", 14.0, theme.online))
            .child(format!(
                "Connected · {} · asp/1",
                match transport {
                    Transport::Acp => "ACP",
                    Transport::Grok => "Grok",
                    Transport::Custom => "custom",
                }
            ))
            .debug_selector(|| "connect-status-connected".into())
            .into_any_element()
    } else {
        PulseClock::request(cx);
        div()
            .flex()
            .items_center()
            .gap(px(9.0))
            .px(px(14.0))
            .py(px(12.0))
            .text_size(px(13.0))
            .text_color(p.placeholder)
            .child(crate::ui::cards::activity_spinner(14.0, &theme, cx))
            .child("Waiting for the first heartbeat…")
            .debug_selector(|| "connect-status-waiting".into())
            .into_any_element()
    };

    let body = div()
        .flex()
        .flex_col()
        .w_full()
        .pb(px(16.0))
        .child(section("Give this to your agent"))
        .child(card(vec![
            link_row.into_any_element(),
            kit::divider(&theme).into_any_element(),
            key_row.into_any_element(),
        ]))
        .child(footnote(if reveal.rotated {
            "The old key stopped working. This one is shown once.".to_owned()
        } else {
            "The key is shown once. If you lose it, rotate it from the agent's profile.".to_owned()
        }))
        .child(footnote(transport.how_to().to_owned()))
        .child(div().h(px(14.0)))
        .child(card(vec![status]));

    let done = div()
        .id("connect-reveal-done")
        .h(px(27.0))
        .w(px(67.0))
        .rounded_full()
        .bg(theme.primary)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(14.2))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.on_primary)
        .cursor_pointer()
        .hover(|s| s.opacity(0.88))
        .child("Done")
        .on_click(cx.listener(|this, _, _, cx| {
            this.state.update(cx, |s, cx| s.close_connect_reveal(cx));
        }));
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
        .id("connect-reveal-card")
        .debug_selector(|| "connect-reveal-card".into())
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
    div()
        .absolute()
        .inset_0()
        .occlude()
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        .flex()
        .items_center()
        .justify_center()
        .p(px(24.0))
        .bg(Theme::with_alpha(theme.bg, 0.55))
        .child(frosted(RADIUS, PANEL_BLUR, sheet))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.state.update(cx, |s, cx| s.close_connect_reveal(cx));
            }),
        )
}

/// The agent pane's Details for an outside agent: **Connection** (Status · Transport ·
/// Protocol) and **Key** (Rotate key… · Revoke access). Replaces the Settings card — model,
/// reasoning and voice are the outside agent's own business.
pub fn details_sections(
    project_id: &Id,
    info: &OutsideInfo,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Vec<Section> {
    let mut out = Vec::new();
    out.push(Section::Title("Connection".into()));
    let waiting = !info.connected && info.status != "revoked";
    let status_trailing: Option<gpui::AnyElement> = if waiting && info.status == "never" {
        PulseClock::request(cx);
        Some(crate::ui::cards::activity_spinner(13.0, theme, cx).into_any_element())
    } else {
        None
    };
    let rows: Vec<gpui::AnyElement> = vec![
        kit::row(
            "outside-status",
            Leading::None,
            "Status",
            None,
            Some(info.status_line(now_ms() as i64).into()),
            status_trailing,
            false,
            theme,
        )
        .debug_selector(|| "outside-status".into())
        .into_any_element(),
        kit::value_row(
            "outside-transport",
            "Transport",
            info.transport_label(),
            theme,
        )
        .into_any_element(),
        kit::value_row(
            "outside-protocol",
            "Protocol",
            if info.protocol.is_empty() {
                "asp/1".to_owned()
            } else {
                info.protocol.clone()
            },
            theme,
        )
        .into_any_element(),
    ];
    out.push(Section::Card(rows));
    out.push(Section::Footnote(
        "It reads and posts in the chats it is in, reacts, and shows typing and status. Secrets and members stay off."
            .into(),
    ));
    out.push(Section::Title("Key".into()));
    let pid = project_id.clone();
    let rotate = kit::row(
        "outside-rotate",
        Leading::Tile("key"),
        "Rotate key…",
        None,
        None,
        None,
        true,
        theme,
    )
    .cursor_pointer()
    .on_click(cx.listener(move |this, _, _, cx| {
        let pid = pid.clone();
        this.state.update(cx, |s, cx| s.rotate_outside_key(pid, cx));
    }))
    .into_any_element();
    let pid = project_id.clone();
    let revoke_label = if info.status == "revoked" {
        "Access revoked"
    } else {
        "Revoke access"
    };
    let revoke = kit::danger_row("outside-revoke", revoke_label, theme)
        .when(info.status != "revoked", |el| {
            el.cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    let pid = pid.clone();
                    this.state
                        .update(cx, |s, cx| s.revoke_outside_agent(pid, cx));
                }))
        })
        .into_any_element();
    out.push(Section::Card(vec![rotate, revoke]));
    out.push(Section::Footnote(
        "Revoke stops it at once. Its messages stay.".into(),
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_links_are_shortened_for_the_row() {
        assert_eq!(
            short_key("arbk_14788fc5760db12964ebf67724279435f6054bcd"),
            "arbk_14788fc5…54bcd"
        );
        assert_eq!(short_key("arbk_short"), "arbk_short");
        assert_eq!(
            short_link("https://relay.arbos.life/asp/connect/01M"),
            "relay.arbos.life/asp/connect/01M"
        );
    }

    #[test]
    fn status_lines_read_as_the_mockup() {
        let now = 1_000_000_000;
        let mut i = OutsideInfo {
            status: "never".into(),
            ..Default::default()
        };
        assert_eq!(i.status_line(now), "Waiting for the first heartbeat");
        i.status = "connected".into();
        i.connected = true;
        i.last_seen_at = Some(now - 12_000);
        assert_eq!(i.status_line(now), "Connected · 12 s ago");
        i.status = "away".into();
        i.last_seen_at = Some(now - 40 * 60_000);
        assert_eq!(i.status_line(now), "Away · 40 min ago");
        i.status = "revoked".into();
        assert_eq!(i.status_line(now), "Disconnected");
        assert_eq!(
            OutsideInfo {
                transport: "acp".into(),
                ..Default::default()
            }
            .transport_label(),
            "ACP · on your Mac"
        );
    }
}
