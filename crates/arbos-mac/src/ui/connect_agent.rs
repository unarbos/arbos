//! Connect an agent (#2141, design `docs/outside-agents-spec.md` §11; mockups
//! `media/feedback/outside-agents/mock-mac-connect-{reveal,profile}.png`): the key reveal
//! sheet after the Create screen, and the Connection / Key cards the agent pane shows for an
//! outside agent. The key is shown once; Copy puts it on the clipboard; the sheet polls the
//! relay and flips to "Connected" when the agent first checks in. Waiting uses the app's one
//! working indicator (`cards::activity_spinner`), never a coloured status dot.

use gpui::{
    div, prelude::*, px, AnyElement, ClipboardItem, Context, Div, FontWeight, MouseButton,
    SharedString, Stateful,
};

use crate::app::{ConnectReveal, ProfilePage};
use crate::model::{now_ms, Id, OutsideInfo, OutsideLogEntry, OUTSIDE_SCOPES};
use crate::ui::create_entity::{self, hex, palette, Transport};
use crate::ui::frost::{frosted, PANEL_BLUR};
use crate::ui::highlight;
use crate::ui::motion::PulseClock;
use crate::ui::profiles::kit::{self, Leading};
use crate::ui::profiles::pane::{self, Hero, Picture, ProfilePane, Section};
use crate::ui::sf_symbols;
use crate::ui::shell::Shell;
use crate::ui::theme::Theme;
use crate::ui::theme::{TEXT_BODY, TEXT_SMALL};

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
/// Protocol), **Wake URL** (URL · Authorization value · Save · Send test wake) and **Key**
/// (Rotate key… · Revoke access). Replaces the Settings card — model, reasoning and voice are
/// the outside agent's own business.
pub fn details_sections(
    project_id: &Id,
    info: &OutsideInfo,
    shell: &Shell,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Vec<Section> {
    let mut out = Vec::new();
    out.push(Section::Title("Connection".into()));
    prefill_wake_form(project_id, info, shell, cx);
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
    // The Session log card (spec §13.3, approved 2026-10-28): under Connection, above the
    // rest; only on a relay that keeps the log (#2224).
    if info.log.is_some() {
        out.extend(session_log_sections(project_id, info, shell, theme, cx));
    }
    // The Can card (spec §4.3, the approved mockup): one switch per scope, flipped on click.
    out.push(Section::Title("Can".into()));
    let switches: Vec<gpui::AnyElement> = OUTSIDE_SCOPES
        .iter()
        .map(|(name, title, subtitle)| {
            let on = info.scope(name);
            let pid = project_id.clone();
            let id: gpui::SharedString = format!("outside-scope-{name}").into();
            let sel = id.clone();
            kit::row(
                gpui::ElementId::Name(id),
                Leading::None,
                *title,
                subtitle.map(|s| s.into()),
                None,
                Some(crate::ui::glass::switch(on, theme).into_any_element()),
                false,
                theme,
            )
            .debug_selector(move || sel.to_string())
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                let pid = pid.clone();
                this.state
                    .update(cx, |s, cx| s.set_outside_scope(pid, name, !on, cx));
            }))
            .into_any_element()
        })
        .collect();
    out.push(Section::Card(switches));
    out.push(Section::Footnote(
        "Changes apply at once. The agent sees only what is on.".into(),
    ));
    // The card needs the relay half (#2185): an older relay reports no `wake` at all.
    if info.status != "revoked" && info.wake.is_some() {
        out.extend(wake_sections(project_id, info, shell, theme, cx));
    }
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

/// Fill the two Wake URL fields the first time the pane shows this agent (the stored URL;
/// the header value is never shown back, so that field starts empty). Done after this
/// render, so the fields' own observers do not re-enter it.
fn prefill_wake_form(project_id: &Id, info: &OutsideInfo, shell: &Shell, cx: &mut Context<Shell>) {
    if shell.wake_form_project.as_ref() == Some(project_id) {
        return;
    }
    let pid = project_id.clone();
    let url = info
        .wake
        .as_ref()
        .and_then(|w| w.url.clone())
        .unwrap_or_default();
    let weak = cx.weak_entity();
    cx.defer(move |cx| {
        let _ = weak.update(cx, |shell, cx| {
            shell.wake_form_project = Some(pid);
            shell
                .wake_url_input
                .update(cx, |input, cx| input.set_text(url, cx));
            shell
                .wake_auth_input
                .update(cx, |input, cx| input.clear(cx));
            cx.notify();
        });
    });
}

/// What the line under the Wake URL buttons says: the in-flight word, the last Save / test
/// outcome, else the relay's last delivery. Pure, so the words are testable.
pub(crate) fn wake_note(
    saving: bool,
    sending: bool,
    note: Option<&(bool, String)>,
    last: Option<&crate::model::WakeLast>,
    now: i64,
) -> Option<(bool, String)> {
    if saving {
        return Some((true, "Saving…".to_owned()));
    }
    if sending {
        return Some((true, "Sending…".to_owned()));
    }
    if let Some((ok, text)) = note {
        return Some((*ok, text.clone()));
    }
    last.map(|l| (l.error.is_none(), format!("Last wake · {}", l.line(now))))
}

/// **Wake URL** card: the URL field, the masked Authorization value field, Save and Send
/// test wake, the outcome line (braille spinner while something is in flight).
fn wake_sections(
    project_id: &Id,
    info: &OutsideInfo,
    shell: &Shell,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Vec<Section> {
    let form = shell
        .state
        .read(cx)
        .wake_form
        .clone()
        .filter(|f| &f.project_id == project_id);
    let saving = form.as_ref().is_some_and(|f| f.saving);
    let sending = form.as_ref().is_some_and(|f| f.sending);
    let wake = info.wake.clone().unwrap_or_default();
    let busy = saving || sending;

    let field = |label: &'static str,
                 hint: Option<String>,
                 input: gpui::Entity<crate::ui::composer::ComposerInput>,
                 selector: &'static str| {
        div()
            .px(px(kit::INSET))
            .pt(px(10.0))
            .pb(px(10.0))
            .flex()
            .flex_col()
            .gap(px(5.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(crate::ui::theme::TEXT_TINY))
                            .text_color(theme.text_muted)
                            .child(label),
                    )
                    .when_some(hint, |el, h| {
                        el.child(
                            div()
                                .text_size(px(crate::ui::theme::TEXT_TINY))
                                .text_color(theme.text_muted)
                                .child(h),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .min_h(px(32.0))
                    .px(px(10.0))
                    .rounded(px(9.0))
                    .bg(theme.field)
                    .border_1()
                    .border_color(theme.field_border)
                    .text_size(px(crate::ui::theme::TEXT_SMALL))
                    .debug_selector(move || selector.into())
                    .child(input),
            )
    };
    let url_field = field("URL", None, shell.wake_url_input.clone(), "wake-url-field");
    let auth_field = field(
        "Authorization header value",
        wake.auth_set
            .then(|| "saved · paste again to replace".to_owned()),
        shell.wake_auth_input.clone(),
        "wake-auth-field",
    );

    let pid = project_id.clone();
    let save = kit::pill_button("wake-save", "checkmark", "Save", theme)
        .when(busy, |el| el.opacity(0.5))
        .on_click(cx.listener(move |this, _, _, cx| {
            let url = this.wake_url_input.read(cx).text().to_owned();
            let auth = this.wake_auth_input.read(cx).text().to_owned();
            let pid = pid.clone();
            this.state.update(cx, |s, cx| {
                if s.wake_form.as_ref().is_some_and(|f| f.saving || f.sending) {
                    return;
                }
                s.save_wake_url(pid, url, auth, cx);
            });
            this.wake_auth_input.update(cx, |input, cx| input.clear(cx));
        }));
    let can_test = wake.url_set && wake.auth_set && !busy;
    let pid = project_id.clone();
    let test = kit::pill_button(
        "wake-test",
        "antenna.radiowaves.left.and.right",
        "Send test wake",
        theme,
    )
    .when(!can_test, |el| el.opacity(0.5))
    .when(can_test, |el| {
        el.on_click(cx.listener(move |this, _, _, cx| {
            let pid = pid.clone();
            this.state.update(cx, |s, cx| s.send_test_wake(pid, cx));
        }))
    });
    let note = wake_note(
        saving,
        sending,
        form.as_ref().and_then(|f| f.note.as_ref()),
        wake.last.as_ref(),
        now_ms() as i64,
    );
    // A delivery readout ("Last wake · Failed · HTTP 401 · 3 min ago"), muted like the status
    // rows — not an error note.
    let note_el: Option<gpui::AnyElement> = note.map(|(_, text)| {
        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .text_size(px(crate::ui::theme::TEXT_TINY))
            .text_color(theme.text_muted)
            .debug_selector(|| "wake-note".into())
            .when(busy, |el| el.child(crate::ui::working::glyph(theme, cx)))
            .child(text)
            .into_any_element()
    });
    let buttons = div()
        .px(px(kit::INSET))
        .pt(px(6.0))
        .pb(px(12.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(save)
                .child(test),
        )
        .children(note_el);

    vec![
        Section::Title("Wake URL".into()),
        Section::Custom(
            kit::card(theme)
                .flex()
                .flex_col()
                .child(url_field)
                .child(kit::divider(theme))
                .child(auth_field)
                .child(kit::divider(theme))
                .child(buttons)
                .into_any_element(),
        ),
        Section::Footnote(
            "When a message needs an answer, Arbos posts a small signed nudge here at once — no message text. The value is sent as the Authorization header, verbatim, and is never shown again. Keep a slow poll as the fallback."
                .into(),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_note_words() {
        let last = crate::model::WakeLast {
            at: 1_000_000 - 12_000,
            status: Some(200),
            error: None,
        };
        assert_eq!(
            wake_note(false, false, None, Some(&last), 1_000_000),
            Some((true, "Last wake · Delivered · HTTP 200 · 12 s ago".into()))
        );
        assert_eq!(
            wake_note(true, false, None, Some(&last), 1_000_000),
            Some((true, "Saving…".into()))
        );
        assert_eq!(
            wake_note(false, true, None, None, 1_000_000),
            Some((true, "Sending…".into()))
        );
        let note = (false, "Failed · HTTP 500: boom".to_owned());
        assert_eq!(
            wake_note(false, false, Some(&note), Some(&last), 1_000_000),
            Some(note.clone())
        );
        assert_eq!(wake_note(false, false, None, None, 1_000_000), None);
        let failed = crate::model::WakeLast {
            at: 1_000_000 - 180_000,
            status: Some(401),
            error: Some("HTTP 401".into()),
        };
        assert_eq!(failed.line(1_000_000), "Failed · HTTP 401 · 3 min ago");
    }

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

// ----- Session log (spec §13.3, [feedback sidecar-sessions-mac-2026-10-28]) ----------------

/// Rows the card shows before "View all".
const LOG_CARD_ROWS: usize = 4;

/// The View all page's filters — the pane's tab pill, same labels as the iPhone.
pub const LOG_TABS: [&str; 4] = ["All", "Replies", "Errors", "Sessions"];

fn secs(ms: i64) -> String {
    format!("{:.1} s", ms as f64 / 1000.0)
}

/// "Last reply 9.8 s · 12 wakes today".
fn log_header_line(info: &OutsideInfo, entries: &[OutsideLogEntry]) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(ms) = info.log.as_ref().and_then(|l| l.last_reply_ms) {
        parts.push(format!("Last reply {}", secs(ms)));
    }
    let today = entries
        .iter()
        .filter(|e| e.kind == "wake" && e.day() == "Today")
        .count();
    if today > 0 {
        parts.push(format!(
            "{today} wake{} today",
            if today == 1 { "" } else { "s" }
        ));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// One log line: the time on the left, then two lines — the chat (or the session event) over
/// the muted latency / detail. Only an error's sentence is red (no dots, no badges).
fn log_row(
    id: impl Into<gpui::ElementId>,
    entry: &OutsideLogEntry,
    chat_title: String,
    theme: &Theme,
) -> Stateful<Div> {
    let (title, detail): (SharedString, SharedString) = if entry.is_session() {
        (entry.session_title().into(), entry.detail().into())
    } else {
        (chat_title.into(), entry.detail().into())
    };
    let detail_color = if entry.is_error() {
        theme.danger
    } else {
        theme.text_muted
    };
    div()
        .id(id)
        .relative()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(kit::GAP))
        .min_h(px(kit::ROW_MIN_H))
        .px(px(kit::INSET))
        .py(px(6.0))
        .text_size(px(TEXT_BODY))
        .text_color(theme.text)
        .child(
            div()
                .flex_shrink_0()
                .w(px(62.0))
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text_muted)
                .child(entry.clock()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(1.0))
                .child(
                    div()
                        .truncate()
                        .when(entry.is_session(), |el| {
                            el.italic().text_color(theme.text_muted)
                        })
                        .child(title),
                )
                .when(!detail.is_empty(), |el| {
                    el.child(
                        div()
                            .text_size(px(TEXT_SMALL))
                            .text_color(detail_color)
                            .when(entry.is_session(), |el| el.italic())
                            .truncate()
                            .child(detail),
                    )
                }),
        )
        .child(kit::hairline_below(theme))
}

/// The card: a title with the header numbers, the newest four lines, View all, a footnote.
fn session_log_sections(
    project_id: &Id,
    info: &OutsideInfo,
    shell: &Shell,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Vec<Section> {
    let state = shell.state.read(cx);
    let entries: Vec<OutsideLogEntry> = state
        .outside_logs
        .get(project_id)
        .cloned()
        .unwrap_or_default();
    let header = log_header_line(info, &entries);
    let title: AnyElement = div()
        .flex()
        .items_center()
        .justify_between()
        .child(kit::section_title("Session log", theme))
        .when_some(header, |el, h| {
            el.child(
                div()
                    .pr(px(kit::INSET * 2.0))
                    .text_size(px(TEXT_SMALL))
                    .text_color(theme.text_muted)
                    .child(h),
            )
        })
        .into_any_element();
    let shown: Vec<&OutsideLogEntry> = entries
        .iter()
        .rev()
        .filter(|e| e.shown())
        .take(LOG_CARD_ROWS)
        .collect();
    let mut rows: Vec<AnyElement> = shown
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let chat = e
                .chat
                .as_deref()
                .map(|c| state.outside_log_chat_title(c, project_id))
                .unwrap_or_default();
            log_row(
                gpui::ElementId::Name(format!("outside-log-{i}").into()),
                e,
                chat,
                theme,
            )
            .debug_selector(move || format!("outside-log-{i}"))
            .into_any_element()
        })
        .collect();
    if shown.is_empty() {
        rows.push(
            kit::row(
                "outside-log-empty",
                Leading::None,
                "No wakes yet",
                Some("Lines appear here when someone writes to the agent.".into()),
                None,
                None,
                false,
                theme,
            )
            .into_any_element(),
        );
    }
    let pid = project_id.clone();
    rows.push(
        kit::row(
            "outside-log-all",
            Leading::None,
            "View all",
            None,
            None,
            None,
            true,
            theme,
        )
        .debug_selector(|| "outside-log-all".into())
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, _, cx| {
            let pid = pid.clone();
            this.state.update(cx, |s, cx| s.open_outside_log(pid, cx));
        }))
        .into_any_element(),
    );
    vec![
        Section::Custom(title),
        Section::Card(rows),
        Section::Footnote(
            "Written by the sidecar on the agent's machine. Never message text or secrets.".into(),
        ),
    ]
}

/// The View all page: every line of the log, newest first, grouped by day, with the pane's
/// tab pill as the filter (All · Replies · Errors · Sessions).
pub fn log_page(project_id: &Id, shell: &Shell, cx: &mut Context<Shell>) -> Option<Stateful<Div>> {
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let project = state.project(project_id)?.clone();
    let tab = state.profile_tab.min(LOG_TABS.len() - 1);
    let entries: Vec<OutsideLogEntry> = state
        .outside_logs
        .get(project_id)
        .cloned()
        .unwrap_or_default();
    let keep = |e: &OutsideLogEntry| match tab {
        1 => matches!(e.kind.as_str(), "reply" | "post"),
        2 => e.is_error(),
        3 => e.is_session(),
        _ => e.shown(),
    };
    let mut body: Vec<Section> = Vec::new();
    let mut day = String::new();
    let mut rows: Vec<AnyElement> = Vec::new();
    for (i, e) in entries.iter().rev().filter(|e| keep(e)).enumerate() {
        let d = e.day();
        if d != day {
            if !rows.is_empty() {
                body.push(Section::Card(std::mem::take(&mut rows)));
            }
            body.push(Section::Title(d.clone().into()));
            day = d;
        }
        let chat = e
            .chat
            .as_deref()
            .map(|c| state.outside_log_chat_title(c, project_id))
            .unwrap_or_default();
        rows.push(
            log_row(
                gpui::ElementId::Name(format!("outside-log-all-{i}").into()),
                e,
                chat,
                &theme,
            )
            .into_any_element(),
        );
    }
    if !rows.is_empty() {
        body.push(Section::Card(rows));
    }
    if body.is_empty() {
        body.push(Section::Empty(
            match tab {
                1 => "No replies yet",
                2 => "No errors",
                3 => "No session events yet",
                _ => "No wakes yet",
            }
            .into(),
        ));
    }
    body.push(Section::Footnote(
        "Written by the sidecar on the agent's machine. Never message text or secrets.".into(),
    ));
    let hero = Hero {
        picture: Picture::Glyph("list.bullet.rectangle"),
        name: pane::name("Session log", &theme),
        badge: None,
        subtitle: Some(project.name.clone().into()),
        subtitle_color: theme.text_muted,
        subtitle_click: None,
        subtitle_element: None,
        editor: None,
        action: None,
    };
    Some(pane::render(
        ProfilePane {
            page: ProfilePage::AgentLog {
                project_id: project_id.clone(),
            },
            hero,
            tabs: &LOG_TABS,
            selected: tab,
            body,
            footer: Vec::new(),
            done: None,
            share: None,
        },
        shell,
        cx,
    ))
}
