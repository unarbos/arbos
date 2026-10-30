//! Calls v1 UI, Grok Bot call-screen style (the iPhone call screen scaled
//! up): start a call from the composer (mic + phone) or the "…" menu —
//! always audio-only until Camera is toggled here — the call screen over
//! the conversation (tiles with the camera or the avatar, name, Mute ·
//! Camera · Hang up), the slim call bar it collapses to, the "On a call ·
//! Join" bar for a call I am not in, and the ringing card.

use std::sync::Arc;

use gpui::{
    div, prelude::*, px, Context, Div, ElementId, FontWeight, RenderImage, SharedString, Stateful,
};

use crate::app::{ActiveCall, AppState, CallParticipant, CallStatus};
use crate::model::{self, now_ms, Call, CallKind, Project, Sender};
use crate::ui::avatars;
use crate::ui::glass::{self, WindowDrag};
use crate::ui::highlight;
use crate::ui::motion::PulseClock;
use crate::ui::people;
use crate::ui::sf_symbols;
use crate::ui::shell::Shell;
use crate::ui::theme::Appearance;
use crate::ui::theme::{Theme, HEADER_HEIGHT, PANEL_RADIUS, TEXT_BODY, TEXT_SMALL, TEXT_TINY};
use crate::ui::widgets::CardPalette;

/// Chats where a call can start: every people chat and every shared project.
/// Every space takes a call: people chats and shared projects ring their humans; a project
/// chat with nobody else in it is a voice call with the agent (Calls Phase 2, D75).
pub fn callable(space: &Project) -> bool {
    let _ = space;
    true
}

fn mmss(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// The decoded picture the loader has for a participant: a person's GitHub photo, or an
/// agent's uploaded / generated picture found through the shared identity rule (#1592) — the
/// same picture its bubbles and the presence row show.
fn sender_image(state: &AppState, sender: &Sender) -> Option<Arc<RenderImage>> {
    if sender.kind == "agent" {
        let a = avatars::agent_identity(
            state.selected(),
            &state.projects,
            &sender.id,
            Some(&sender.handle),
            Some(&sender.name),
        );
        return a.image_key.as_deref().and_then(|url| state.avatar(url));
    }
    state.avatar(&sender.avatar_url)
}

/// A participant's picture: GitHub photo, coloured tree, or the initial.
fn picture(sender: &Sender, image: Option<Arc<RenderImage>>, size: f32, theme: &Theme) -> Div {
    // A photo wins; otherwise the sender's initials disc (agents and people alike).
    if image.is_some() {
        return people::avatar(image, &sender.as_person(), size, theme);
    }
    avatars::sender(sender, size, theme)
}

/// One tile: the camera when it is on, else the picture on a dark card.
fn tile(
    id: ElementId,
    p: &CallParticipant,
    image: Option<Arc<RenderImage>>,
    video: Option<Arc<RenderImage>>,
    label: String,
    big: bool,
    theme: &Theme,
) -> Stateful<Div> {
    let avatar_size = if big { 112.0 } else { 72.0 };
    let mut card = div()
        .id(id)
        .relative()
        .flex_1()
        .min_w(px(180.0))
        .min_h(px(if big { 260.0 } else { 160.0 }))
        .rounded(px(24.0))
        .overflow_hidden()
        .bg(theme.surface_1)
        .border_2()
        .border_color(if p.speaking {
            theme.online
        } else {
            gpui::transparent_black()
        })
        .flex()
        .items_center()
        .justify_center();
    match video {
        Some(frame) => {
            card = card.child(
                gpui::img(frame)
                    .absolute()
                    .inset_0()
                    .size_full()
                    .object_fit(gpui::ObjectFit::Cover),
            );
        }
        None => {
            card = card.child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(14.0))
                    .child(picture(&p.sender, image, avatar_size, theme))
                    .child(sf_symbols::glyph(
                        "video.slash.fill",
                        22.0,
                        theme.text_faint,
                    )),
            );
        }
    }
    // Name (and the muted mic) pinned to the bottom-left.
    card.child(
        div()
            .absolute()
            .left(px(12.0))
            .bottom(px(10.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .px(px(10.0))
            .py(px(4.0))
            .rounded_full()
            .bg(gpui::hsla(0.0, 0.0, 0.0, 0.45))
            .text_size(px(TEXT_SMALL))
            .text_color(theme.text)
            .when(p.muted, |el| {
                el.child(sf_symbols::glyph("mic.slash", 12.0, theme.text_muted))
            })
            .child(label),
    )
}

fn round_button(
    id: &'static str,
    glyph: &str,
    label: &str,
    on: bool,
    danger: bool,
    theme: &Theme,
) -> Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(8.0))
        .child(
            div()
                .id(id)
                .size(px(60.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .bg(if danger {
                    theme.danger
                } else if on {
                    theme.text
                } else {
                    theme.surface_2
                })
                .hover(|s| s.opacity(0.88))
                .child(sf_symbols::glyph(
                    glyph,
                    24.0,
                    if danger {
                        theme.text
                    } else if on {
                        theme.bg
                    } else {
                        theme.text
                    },
                )),
        )
        .child(
            div()
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text_muted)
                .child(label.to_string()),
        )
}

/// The full call screen, over the conversation column.
pub fn screen(shell: &Shell, active: &ActiveCall, cx: &mut Context<Shell>) -> Div {
    let theme = Theme::of(cx).clone();
    PulseClock::request(cx);
    let state = shell.state.read(cx);
    let elapsed = now_ms().saturating_sub(active.joined_at);
    let title = match active.status {
        CallStatus::Connecting => "Connecting…".to_string(),
        CallStatus::Connected => format!("{} · {}", active.call.kind.label_title(), mmss(elapsed)),
        CallStatus::Failed(ref why) => format!("Could not connect · {why}"),
    };
    let me = Sender {
        kind: "user".into(),
        id: state.me_id(),
        name: state
            .account
            .as_ref()
            .map(|a| a.name.clone())
            .unwrap_or_default(),
        handle: state.me_handle(),
        avatar: None,
        avatar_color: None,
        avatar_color_index: None,
        avatar_leaf: None,
        avatar_leaf_color: None,
        avatar_url: state
            .account
            .as_ref()
            .map(|a| a.avatar_url.clone())
            .unwrap_or_default(),
    };
    // Tiles: everyone else, then me (small) — alone, my own tile fills the screen.
    let remote: Vec<(CallParticipant, Option<Arc<RenderImage>>)> = active
        .participants
        .iter()
        .map(|p| (p.clone(), sender_image(state, &p.sender)))
        .collect();
    // Who was rung and has not joined yet (#1430): a greyed tile each, "Ringing…", so a group
    // call shows the whole group while only the agent has picked up.
    let ringing: Vec<(Sender, Option<Arc<RenderImage>>)> =
        if active.call.is_live() && active.status == CallStatus::Connected {
            let expected = state
                .project(active.call.space_id())
                .map(|p| model::call_people(p, &state.me_id(), &active.call.agents))
                .unwrap_or_else(|| active.call.agents.clone());
            let in_room: Vec<String> = active
                .participants
                .iter()
                .map(|p| p.identity.clone())
                .collect();
            model::still_ringing(&expected, &in_room)
                .into_iter()
                .map(|s| {
                    let image = sender_image(state, &s);
                    (s, image)
                })
                .collect()
        } else {
            Vec::new()
        };
    let my_image = state.avatar(&me.avatar_url);
    let me_tile = CallParticipant {
        identity: "local".into(),
        sender: me,
        speaking: false,
        muted: active.muted,
        video: active.local_video.clone(),
    };
    let n = remote.len() + ringing.len();
    let big = n <= 1;
    let mut grid = div()
        .flex()
        .flex_wrap()
        .items_stretch()
        .justify_center()
        .gap(px(12.0))
        .w_full()
        .max_w(px(if n <= 1 { 720.0 } else { 1100.0 }))
        .flex_1()
        .min_h_0();
    for (i, (p, image)) in remote.iter().enumerate() {
        let video = p.video.clone();
        grid = grid.child(tile(
            ElementId::Name(SharedString::from(format!("tile-{i}"))),
            p,
            image.clone(),
            video,
            p.sender.display_name(),
            big,
            &theme,
        ));
    }
    for (i, (s, image)) in ringing.iter().enumerate() {
        let caption = if s.kind == "agent" {
            "Joining…"
        } else {
            "Ringing…"
        };
        let ghost = CallParticipant {
            identity: s.identity(),
            sender: s.clone(),
            speaking: false,
            muted: false,
            video: None,
        };
        grid = grid.child(
            tile(
                ElementId::Name(SharedString::from(format!("tile-ringing-{i}"))),
                &ghost,
                image.clone(),
                None,
                format!("{} · {caption}", s.display_name()),
                big,
                &theme,
            )
            .opacity(0.55),
        );
    }
    let seeing_agent = active
        .agent_seeing
        .then(|| agent_name(shell, active, cx))
        .flatten();
    if n == 0 {
        let me_big = tile(
            ElementId::Name("tile-me".into()),
            &me_tile,
            my_image.clone(),
            active.local_video.clone(),
            "You".into(),
            true,
            &theme,
        );
        grid = grid.child(match seeing_agent.as_deref() {
            Some(agent) => me_big.child(
                div()
                    .absolute()
                    .top(px(12.0))
                    .right(px(12.0))
                    .child(seeing_badge(agent, &theme)),
            ),
            None => me_big,
        });
    }
    let waiting = n == 0 && active.status == CallStatus::Connected;

    // "Let <agent> see" (#1353): beside the camera while it is on in a call
    // with an agent — the one switch for the agent's `vision_enabled`.
    let vision = active
        .camera
        .then(|| vision_agent(shell, active, cx))
        .flatten();

    let controls = div()
        .flex()
        .items_start()
        .justify_center()
        .gap(px(36.0))
        .pb(px(28.0))
        .child(
            round_button(
                "call-mute",
                if active.muted { "mic.slash" } else { "mic" },
                if active.muted { "Unmute" } else { "Mute" },
                active.muted,
                false,
                &theme,
            )
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.state.update(cx, |s, cx| s.toggle_call_mute(cx));
                }),
            ),
        )
        .child(
            round_button(
                "call-camera",
                if active.camera {
                    "video.fill"
                } else {
                    "video.slash.fill"
                },
                if active.camera {
                    "Camera on"
                } else {
                    "Camera off"
                },
                active.camera,
                false,
                &theme,
            )
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.state.update(cx, |s, cx| s.toggle_camera(cx));
                }),
            ),
        )
        .children(vision.map(|(pid, name, on)| vision_switch(pid, &name, on, &theme, cx)))
        .child(
            round_button(
                "call-hangup",
                "phone.down.fill",
                "Hang up",
                false,
                true,
                &theme,
            )
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.state.update(cx, |s, cx| s.leave_call(cx));
                }),
            ),
        );

    let mut column = div()
        .absolute()
        .inset_0()
        .flex()
        .flex_col()
        .items_center()
        .bg(theme.bg)
        .child(
            div()
                .w_full()
                .h(px(HEADER_HEIGHT))
                .flex_shrink_0()
                .flex()
                .items_center()
                .px(px(12.0))
                .window_drag()
                .child(
                    div()
                        .id("call-collapse")
                        .size(px(32.0))
                        .rounded(px(8.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .map(|el| highlight::button(el, &theme))
                        .child(sf_symbols::glyph("chevron.down", 16.0, theme.text_muted))
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.state.update(cx, |s, cx| s.toggle_call_expanded(cx));
                            }),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .text_center()
                        .text_size(px(TEXT_SMALL))
                        .text_color(theme.text_muted)
                        .child(title),
                )
                .child(div().size(px(32.0))),
        )
        .child(
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .px(px(40.0))
                .py(px(12.0))
                .child(grid),
        );
    // Name under the single big tile, like the iPhone.
    if n <= 1 {
        let name = remote
            .first()
            .map(|(p, _)| p.sender.display_name())
            .or_else(|| ringing.first().map(|(s, _)| s.display_name()))
            .unwrap_or_else(|| "You".into());
        column = column.child(
            div()
                .pt(px(6.0))
                .text_size(px(TEXT_BODY + 7.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(name),
        );
    }
    let humans_ringing: Vec<String> = ringing
        .iter()
        .filter(|(s, _)| s.kind != "agent")
        .map(|(s, _)| s.display_name())
        .collect();
    let sub = if let Some(notice) = active.notice.clone() {
        Some(notice)
    } else if !humans_ringing.is_empty() && active.reachable.is_some() {
        // "Robert's Arbos isn't open — they'll see a missed call" / "Ringing
        // Robert's iPhone" (contract §8, same words as the iPhone app) — kept while
        // anyone is still being rung, not only while the room is empty.
        active.reachable.clone()
    } else if !humans_ringing.is_empty() {
        Some(ringing_line(&humans_ringing))
    } else if waiting {
        Some("Waiting for others to join…".into())
    } else if let CallStatus::Failed(_) = active.status {
        Some("Hang up and try again.".into())
    } else {
        None
    };
    if let Some(sub) = sub {
        column = column.child(
            div()
                .pt(px(4.0))
                .px(px(40.0))
                .text_center()
                .text_size(px(TEXT_BODY))
                .text_color(theme.text_muted)
                .child(sub),
        );
    }
    // My small self-view in the corner when others are here and my camera is on.
    if n >= 1 && active.local_video.is_some() {
        column = column.child(
            div()
                .absolute()
                .right(px(24.0))
                .bottom(px(140.0))
                .w(px(180.0))
                .h(px(135.0))
                .child({
                    let me_small = tile(
                        ElementId::Name("tile-me-small".into()),
                        &me_tile,
                        my_image,
                        active.local_video.clone(),
                        "You".into(),
                        false,
                        &theme,
                    );
                    match seeing_agent.as_deref() {
                        Some(agent) => me_small.child(
                            div()
                                .absolute()
                                .top(px(8.0))
                                .right(px(8.0))
                                .child(seeing_badge(agent, &theme)),
                        ),
                        None => me_small,
                    }
                }),
        );
    }
    column.child(div().h(px(24.0))).child(controls)
}

/// The in-call bar under the header while the call screen is collapsed: the
/// chat stays fully usable below it. Grok question-card palette (white in
/// light, near-black in dark): picture, "Voice call · Alice", who is speaking,
/// timer, mute, hang up; a click anywhere else restores the full screen.
pub fn bar(shell: &Shell, active: &ActiveCall, cx: &mut Context<Shell>) -> Stateful<Div> {
    let theme = Theme::of(cx).clone();
    let palette = CardPalette::of(&theme);
    PulseClock::request(cx);
    let state = shell.state.read(cx);
    let elapsed = now_ms().saturating_sub(active.joined_at);
    let space_name = state
        .project(active.call.space_id())
        .map(|p| {
            if p.is_people() && !p.is_people_group() {
                p.person()
                    .map(|x| x.display_name())
                    .unwrap_or_else(|| p.name.clone())
            } else {
                p.name.clone()
            }
        })
        .unwrap_or_else(|| "call".into());
    let title = format!("{} · {}", active.call.kind.label_title(), space_name);
    let speaking: Vec<String> = active
        .participants
        .iter()
        .filter(|p| p.speaking)
        .map(|p| p.sender.display_name())
        .collect();
    let sub = match &active.status {
        CallStatus::Connecting => "Connecting…".to_string(),
        CallStatus::Failed(why) => format!("Could not connect · {why}"),
        CallStatus::Connected if !speaking.is_empty() => {
            format!("{} speaking", speaking.join(", "))
        }
        CallStatus::Connected if active.participants.is_empty() => {
            if let Some(r) = &active.reachable {
                r.clone()
            } else {
                "Waiting for others to join…".to_string()
            }
        }
        CallStatus::Connected => {
            let who: Vec<String> = active
                .participants
                .iter()
                .map(|p| p.sender.display_name())
                .collect();
            format!("With {}", who.join(", "))
        }
    };
    let first = active.participants.first().cloned();
    let picture_el = match &first {
        Some(p) => {
            let image = sender_image(state, &p.sender);
            picture(&p.sender, image, 30.0, &theme)
        }
        None => div()
            .size(px(30.0))
            .rounded_full()
            .bg(theme.online)
            .flex()
            .items_center()
            .justify_center()
            .child(sf_symbols::glyph("phone", 14.0, gpui::white())),
    };
    let bg = match theme.appearance {
        Appearance::Light => palette.inner,
        Appearance::Dark => palette.outer,
    };
    let round = |id: &'static str, glyph: &str, fill: gpui::Hsla, ink: gpui::Hsla| {
        div()
            .id(id)
            .size(px(30.0))
            .rounded_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(fill)
            .hover(|s| s.opacity(0.85))
            .child(sf_symbols::glyph(glyph, 14.0, ink))
    };
    div()
        .id("call-bar")
        .mx(px(16.0))
        .mt(px(6.0))
        .mb(px(8.0))
        .h(px(52.0))
        .pl(px(10.0))
        .pr(px(8.0))
        .flex()
        .items_center()
        .gap(px(10.0))
        .rounded(px(16.0))
        .bg(bg)
        .border_1()
        .border_color(palette.hairline)
        .shadow_md()
        .cursor_pointer()
        .on_click(cx.listener(|this, _, _, cx| {
            this.state.update(cx, |s, cx| s.toggle_call_expanded(cx));
        }))
        .child(picture_el)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .truncate()
                        .text_size(px(TEXT_SMALL))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child(title),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(TEXT_TINY))
                        .text_color(if speaking.is_empty() {
                            theme.text_muted
                        } else {
                            theme.online
                        })
                        .child(sub),
                ),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text_muted)
                .child(mmss(elapsed)),
        )
        .child(
            round(
                "bar-mute",
                if active.muted { "mic.slash" } else { "mic" },
                if active.muted {
                    theme.text
                } else {
                    Theme::with_alpha(theme.text, 0.1)
                },
                if active.muted { theme.bg } else { theme.text },
            )
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.state.update(cx, |s, cx| s.toggle_call_mute(cx));
                }),
            ),
        )
        .child(
            round("bar-hangup", "phone.down.fill", theme.danger, gpui::white()).on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.state.update(cx, |s, cx| s.leave_call(cx));
                }),
            ),
        )
        .child(highlight::button(
            round(
                "bar-expand",
                "chevron.up",
                gpui::transparent_black(),
                theme.text_muted,
            ),
            &theme,
        ))
}

/// Picture-in-picture while a video call is collapsed: the remote camera (else
/// mine, else the picture) in a small tile floating over the chat. Drag it by
/// its body; a plain click restores the full call screen.
pub fn pip(shell: &Shell, active: &ActiveCall, cx: &mut Context<Shell>) -> Stateful<Div> {
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let remote = active.participants.iter().find(|p| p.video.is_some());
    let video = remote
        .and_then(|p| p.video.clone())
        .or_else(|| active.local_video.clone());
    let label = remote
        .map(|p| p.sender.display_name())
        .unwrap_or_else(|| "You".into());
    let (right, bottom) = shell.pip_offset;
    let mut tile = div()
        .id("call-pip")
        .absolute()
        .right(px(right))
        .bottom(px(bottom))
        .w(px(PIP_W))
        .h(px(PIP_H))
        .rounded(px(14.0))
        .overflow_hidden()
        .bg(gpui::black())
        .border_1()
        .border_color(Theme::with_alpha(gpui::white(), 0.18))
        .shadow_lg()
        .cursor_pointer()
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(|this, ev: &gpui::MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.pip_drag = Some((ev.position, this.pip_offset, false));
                cx.notify();
            }),
        );
    if let Some(frame) = video {
        tile = tile.child(
            gpui::img(frame)
                .size_full()
                .object_fit(gpui::ObjectFit::Cover),
        );
    } else if let Some(p) = active.participants.first() {
        let image = sender_image(state, &p.sender);
        tile = tile.child(
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(picture(&p.sender, image, 56.0, &theme)),
        );
    } else {
        tile = tile.child(
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(sf_symbols::glyph(
                    "video.fill",
                    22.0,
                    Theme::with_alpha(gpui::white(), 0.6),
                )),
        );
    }
    let seeing = (remote.is_none() && active.agent_seeing)
        .then(|| agent_name(shell, active, cx))
        .flatten();
    tile.child(
        div()
            .absolute()
            .left(px(8.0))
            .bottom(px(6.0))
            .px(px(7.0))
            .py(px(2.0))
            .rounded_full()
            .bg(Theme::with_alpha(gpui::black(), 0.55))
            .text_size(px(TEXT_TINY))
            .text_color(gpui::white())
            .child(label),
    )
    .when_some(seeing, |el, agent| {
        el.child(
            div()
                .absolute()
                .top(px(8.0))
                .right(px(8.0))
                .child(seeing_badge(&agent, &theme)),
        )
    })
}

/// "<Agent> can see" — the voice worker is sampling my camera (voice API §11).
fn seeing_badge(agent: &str, theme: &Theme) -> Div {
    let _ = theme;
    div()
        .flex()
        .items_center()
        .gap(px(5.0))
        .px(px(8.0))
        .py(px(3.0))
        .rounded_full()
        .bg(Theme::with_alpha(gpui::black(), 0.55))
        // The iPhone's badge: semibold on a 0.5 pt white hairline.
        .border(px(0.5))
        .border_color(Theme::with_alpha(gpui::white(), 0.25))
        .text_size(px(TEXT_TINY))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(gpui::white())
        .child(sf_symbols::glyph("eye", 12.0, gpui::white()))
        .child(format!("{agent} can see"))
}

/// The agent's name for the vision badge: the space the call is in.
/// The agent this call is with and whether it may look through my camera —
/// `None` for calls with people (the switch is an agent setting).
fn vision_agent(
    shell: &Shell,
    active: &ActiveCall,
    cx: &Context<Shell>,
) -> Option<(crate::model::Id, String, bool)> {
    let state = shell.state.read(cx);
    state
        .project(active.call.space_id())
        .filter(|p| !p.is_people() && !p.is_guest())
        .map(|p| (p.id.clone(), p.name.clone(), p.vision_enabled))
}

/// The switch beside the camera button: "Let <agent> see" over a macOS
/// switch, laid out like the round buttons so the row stays level.
fn vision_switch(
    project_id: crate::model::Id,
    agent: &str,
    on: bool,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(8.0))
        .child(
            div()
                .id("call-vision")
                .h(px(60.0))
                .flex()
                .items_center()
                .cursor_pointer()
                .child(glass::switch(on, theme))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        let id = project_id.clone();
                        this.state.update(cx, |s, cx| s.set_vision(id, !on, cx));
                    }),
                ),
        )
        .child(
            div()
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text_muted)
                .child(format!("Let {agent} see")),
        )
}

fn agent_name(shell: &Shell, active: &ActiveCall, cx: &Context<Shell>) -> Option<String> {
    let state = shell.state.read(cx);
    state
        .project(active.call.space_id())
        .filter(|p| !p.is_people())
        .map(|p| p.name.clone())
}

/// PiP tile size.
pub const PIP_W: f32 = 200.0;
pub const PIP_H: f32 = 150.0;

/// "On a call · Join" for a live call in this chat that I am not in.
pub fn join_bar(call: &Call, cx: &mut Context<Shell>) -> Div {
    let theme = Theme::of(cx).clone();
    let who: Vec<String> = call.participants.iter().map(|p| p.display_name()).collect();
    let text = if who.is_empty() {
        format!(
            "{} started a {}",
            call.started_by.display_name(),
            call.kind.label()
        )
    } else {
        format!("On a call · {}", who.join(", "))
    };
    let call_id = call.call_id.clone();
    div()
        .mx(px(16.0))
        .mb(px(8.0))
        .h(px(40.0))
        .pl(px(12.0))
        .pr(px(6.0))
        .flex()
        .items_center()
        .gap(px(10.0))
        .rounded_full()
        .bg(theme.surface_2)
        .child(sf_symbols::glyph(
            if call.kind == CallKind::Video {
                "video.fill"
            } else {
                "phone"
            },
            14.0,
            theme.online,
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text)
                .child(text),
        )
        .child(
            glass::primary_button("join-call", "Join", &theme)
                .h(px(28.0))
                .px(px(14.0))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let id = call_id.clone();
                    this.state.update(cx, |s, cx| s.join_call(id, cx));
                })),
        )
}

/// "Ringing Bob and Carol…" / "Ringing Bob, Carol and 2 more…".
pub fn ringing_line(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [a] => format!("Ringing {a}…"),
        [a, b] => format!("Ringing {a} and {b}…"),
        [a, b, rest @ ..] => format!("Ringing {a}, {b} and {} more…", rest.len()),
    }
}

/// Ringing card (top-right): caller, "Incoming video call", Decline / Accept.
pub fn ring_card(shell: &Shell, call: &Call, sender: &Sender, cx: &mut Context<Shell>) -> Div {
    let theme = Theme::of(cx).clone();
    let image = sender_image(shell.state.read(cx), sender);
    PulseClock::request(cx);
    let phase = PulseClock::phase(cx, 1200);
    let card = div()
        .w(px(340.0))
        .flex()
        .flex_col()
        .gap(px(14.0))
        .p(px(16.0))
        .rounded(px(PANEL_RADIUS + 6.0))
        .bg(theme.card())
        .border_1()
        .border_color(Theme::with_alpha(theme.online, 0.3 + 0.4 * phase))
        .shadow_lg()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(picture(sender, image, 48.0, &theme))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(px(TEXT_BODY))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text)
                                .child(sender.display_name()),
                        )
                        .child(
                            div()
                                .text_size(px(TEXT_SMALL))
                                .text_color(theme.text_muted)
                                .child(format!("Incoming {}", call.kind.label())),
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .gap(px(10.0))
                .child(
                    div()
                        .id("ring-decline")
                        .flex_1()
                        .h(px(40.0))
                        .rounded_full()
                        .bg(theme.danger)
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(6.0))
                        .text_size(px(TEXT_SMALL))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .cursor_pointer()
                        .hover(|s| s.opacity(0.88))
                        .child(sf_symbols::glyph("phone.down.fill", 14.0, theme.text))
                        .child("Decline")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.update(cx, |s, cx| s.decline_incoming(cx));
                        })),
                )
                .child(
                    div()
                        .id("ring-accept")
                        .flex_1()
                        .h(px(40.0))
                        .rounded_full()
                        .bg(theme.online)
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(6.0))
                        .text_size(px(TEXT_SMALL))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.bg)
                        .cursor_pointer()
                        .hover(|s| s.opacity(0.88))
                        .child(sf_symbols::glyph("phone", 14.0, theme.bg))
                        .child("Accept")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.update(cx, |s, cx| s.accept_incoming(cx));
                        })),
                ),
        );
    div()
        .absolute()
        .right(px(16.0))
        .top(px(HEADER_HEIGHT + 8.0))
        .child(glass::popover(crate::ui::frost::frosted(
            PANEL_RADIUS + 6.0,
            crate::ui::frost::MENU_BLUR,
            card,
        )))
}
