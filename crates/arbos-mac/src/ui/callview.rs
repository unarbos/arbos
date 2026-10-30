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
use crate::ui::glass;
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

/// The call strip (Jacob, 2026-10-23, "call + chat"): the call lives at the top of the OPEN
/// CHAT COLUMN only — the sidebar is never covered or shrunk. One rounded group with the
/// participant tiles (avatar + name, a muted-mic glyph on muted tiles, the active speaker
/// outlined, #1436 ringing tiles greyed "Ringing…" until they join) and Mute / Camera /
/// "Let <agent> see" (#1353) / Hang up to the right; under it a thin drag handle that resizes
/// the strip against the transcript (`AppState::call_strip_height`). The messages and the
/// composer stay visible below. Ending the call removes it; the column is a plain chat again.
pub mod strip_layout {
    /// Smallest strip: one row of tiles with their names.
    pub const MIN_HEIGHT: f32 = 112.0;
    /// Default strip height (four tiles with 56 pt avatars, like the mock).
    pub const DEFAULT_HEIGHT: f32 = 168.0;
    /// The transcript keeps at least this much of the column (header + a few rows + composer).
    pub const TRANSCRIPT_MIN: f32 = 260.0;
    /// The handle under the strip: its hit height and the visible pill.
    pub const HANDLE_HEIGHT: f32 = 12.0;

    /// Largest strip for a column this tall.
    pub fn max_height(column_height: f32) -> f32 {
        (column_height - TRANSCRIPT_MIN).max(MIN_HEIGHT)
    }

    /// Clamp a wanted height into the bounds for this column.
    pub fn clamp_height(wanted: f32, column_height: f32) -> f32 {
        wanted.round().clamp(MIN_HEIGHT, max_height(column_height))
    }

    /// The height after a drag of the handle by `dy` (down = taller strip).
    pub fn height_after_drag(current: f32, dy: f32, column_height: f32) -> f32 {
        clamp_height(current + dy, column_height)
    }

    /// Tile avatar size for a strip this tall (name row and paddings taken off).
    pub fn avatar_size(strip_height: f32) -> f32 {
        (strip_height - 100.0).clamp(28.0, 96.0)
    }

    /// Is the strip shown in the column of `space_id`? Only while MY call is live there and
    /// expanded (collapsed = the slim bar; a call in another chat = nothing here, that chat's
    /// column has it).
    pub fn shown(my_call: Option<&crate::app::ActiveCall>, space_id: &str) -> bool {
        my_call.is_some_and(|c| c.expanded && c.call.is_live() && c.call.space_id() == space_id)
    }
}

/// Drag payload for the strip's resize handle.
#[derive(Clone)]
struct StripResizeDrag;

struct StripResizeDragGhost;

impl gpui::Render for StripResizeDragGhost {
    fn render(&mut self, _: &mut gpui::Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// One tile of the strip: a person (or agent) in the room or still being rung.
fn strip_tile(
    id: ElementId,
    p: &CallParticipant,
    image: Option<Arc<RenderImage>>,
    avatar: f32,
    caption: Option<&str>,
    theme: &Theme,
) -> Stateful<Div> {
    let ringing = caption.is_some();
    let mut card = div()
        .id(id)
        .relative()
        .flex_1()
        .min_w(px(84.0))
        .max_w(px(220.0))
        .h_full()
        .rounded(px(14.0))
        .overflow_hidden()
        .bg(theme.surface_2)
        .border_2()
        .border_color(if p.speaking {
            theme.online
        } else {
            gpui::transparent_black()
        })
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .when(ringing, |el| el.opacity(0.55));
    match p.video.clone() {
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
            card = card.child(picture(&p.sender, image, avatar, theme));
        }
    }
    let name = if p.identity == "local" {
        "You".to_string()
    } else {
        p.sender.display_name()
    };
    // Name (truncated to the tile), the muted mic before it, "Ringing…" under it.
    card = card.child(
        div()
            .max_w_full()
            .px(px(8.0))
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .max_w_full()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded_full()
                    .when(p.video.is_some(), |el| {
                        el.bg(gpui::hsla(0.0, 0.0, 0.0, 0.45))
                    })
                    .text_size(px(TEXT_SMALL))
                    .text_color(theme.text)
                    .when(p.muted, |el| {
                        el.child(sf_symbols::glyph("mic.slash", 11.0, theme.text_muted))
                    })
                    .child(div().truncate().child(name)),
            )
            .children(caption.map(|c| {
                div()
                    .text_size(px(TEXT_TINY))
                    .text_color(theme.text_muted)
                    .child(c.to_string())
            })),
    );
    card
}

/// The strip plus its handle, for `pre` of the chat column.
pub fn strip(shell: &Shell, active: &ActiveCall, cx: &mut Context<Shell>) -> Div {
    let theme = Theme::of(cx).clone();
    PulseClock::request(cx);
    let state = shell.state.read(cx);
    let height = state.call_strip_height;
    let avatar = strip_layout::avatar_size(height);
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
    // Tiles: everyone in the room, then who is still being rung (#1436), then me.
    let in_room: Vec<String> = active
        .participants
        .iter()
        .map(|p| p.identity.clone())
        .collect();
    let ringing: Vec<Sender> = if active.status == CallStatus::Connected {
        let expected = state
            .project(active.call.space_id())
            .map(|p| model::call_people(p, &state.me_id(), &active.call.agents))
            .unwrap_or_else(|| active.call.agents.clone());
        model::still_ringing(&expected, &in_room)
    } else {
        Vec::new()
    };
    let humans_ringing: Vec<String> = ringing
        .iter()
        .filter(|s| s.kind != "agent")
        .map(|s| s.display_name())
        .collect();
    let mut tiles = div()
        .flex_1()
        .min_w_0()
        .h_full()
        .flex()
        .items_stretch()
        .gap(px(8.0));
    for (i, p) in active.participants.iter().enumerate() {
        let image = sender_image(state, &p.sender);
        tiles = tiles.child(strip_tile(
            ElementId::Name(SharedString::from(format!("strip-tile-{i}"))),
            p,
            image,
            avatar,
            None,
            &theme,
        ));
    }
    for (i, s) in ringing.iter().enumerate() {
        let ghost = CallParticipant {
            identity: s.identity(),
            sender: s.clone(),
            speaking: false,
            muted: false,
            video: None,
        };
        let caption = if s.kind == "agent" {
            "Joining…"
        } else {
            "Ringing…"
        };
        tiles = tiles.child(strip_tile(
            ElementId::Name(SharedString::from(format!("strip-ringing-{i}"))),
            &ghost,
            sender_image(state, s),
            avatar,
            Some(caption),
            &theme,
        ));
    }
    let seeing_agent = active
        .agent_seeing
        .then(|| agent_name(shell, active, cx))
        .flatten();
    let me_tile = CallParticipant {
        identity: "local".into(),
        sender: me.clone(),
        speaking: false,
        muted: active.muted,
        video: active.local_video.clone(),
    };
    let my_image = state.avatar(&me.avatar_url);
    let me_card = strip_tile(
        ElementId::Name("strip-tile-me".into()),
        &me_tile,
        my_image,
        avatar,
        None,
        &theme,
    );
    tiles = tiles.child(match seeing_agent.as_deref() {
        Some(agent) => me_card.child(
            div()
                .absolute()
                .top(px(6.0))
                .right(px(6.0))
                .child(seeing_badge(agent, &theme)),
        ),
        None => me_card,
    });

    let elapsed = now_ms().saturating_sub(active.joined_at);
    let status = if let Some(notice) = active.notice.clone() {
        notice
    } else if !humans_ringing.is_empty() && active.reachable.is_some() {
        // "Ringing Bob's iPhone" / "X's Arbos isn't open — they'll see a missed call"
        // (contract §8), kept while anyone is still being rung.
        active.reachable.clone().unwrap_or_default()
    } else if !humans_ringing.is_empty() {
        ringing_line(&humans_ringing)
    } else {
        match &active.status {
            CallStatus::Connecting => "Connecting…".into(),
            CallStatus::Failed(why) => format!("Could not connect · {why}"),
            CallStatus::Connected if active.participants.is_empty() => {
                "Waiting for others to join…".into()
            }
            CallStatus::Connected => {
                format!("{} · {}", active.call.kind.label_title(), mmss(elapsed))
            }
        }
    };

    // "Let <agent> see" (#1353): beside the camera while it is on in a call with an agent.
    let vision = active
        .camera
        .then(|| vision_agent(shell, active, cx))
        .flatten();
    let controls = div()
        .flex_shrink_0()
        .flex()
        .items_start()
        .gap(px(18.0))
        .pl(px(16.0))
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
                "Camera",
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

    let group = div()
        .relative()
        .w_full()
        .h(px(height))
        .rounded(px(PANEL_RADIUS))
        .bg(theme.surface_1)
        .border_1()
        .border_color(theme.border)
        .p(px(10.0))
        .flex()
        .items_stretch()
        .child(tiles)
        .child(controls)
        // Collapse to the slim bar (the agents' in-call bar) — the bar brings it back.
        .child(
            div()
                .id("call-collapse")
                .absolute()
                .top(px(6.0))
                .left(px(6.0))
                .size(px(22.0))
                .rounded(px(6.0))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .map(|el| highlight::button(el, &theme))
                .child(sf_symbols::glyph("chevron.up", 12.0, theme.text_faint))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.state.update(cx, |s, cx| s.toggle_call_expanded(cx));
                    }),
                ),
        );

    // The handle: a thin band with a pill; dragging it resizes the strip against the chat.
    let handle = div()
        .id("call-strip-handle")
        .w_full()
        .h(px(strip_layout::HANDLE_HEIGHT))
        .flex()
        .items_center()
        .justify_center()
        .cursor(gpui::CursorStyle::ResizeRow)
        // hover-fill: exception handle
        .hover(|s| s.bg(Theme::with_alpha(theme.primary, 0.12)))
        .child(div().w(px(40.0)).h(px(4.0)).rounded_full().bg(theme.border))
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_drag(StripResizeDrag, |_, _, _, cx| {
            cx.new(|_| StripResizeDragGhost)
        })
        .on_drag_move(cx.listener(
            |this, ev: &gpui::DragMoveEvent<StripResizeDrag>, window, cx| {
                // The handle follows the pointer: grow by how far the pointer is below its
                // middle, shrink by how far above.
                let middle = f32::from(ev.bounds.origin.y) + f32::from(ev.bounds.size.height) / 2.0;
                let dy = f32::from(ev.event.position.y) - middle;
                let column = f32::from(window.viewport_size().height);
                this.state.update(cx, |s, cx| {
                    let next = strip_layout::height_after_drag(s.call_strip_height, dy, column);
                    s.set_call_strip_height(next, cx);
                });
            },
        ));

    div()
        .w_full()
        .flex()
        .flex_col()
        .px(px(12.0))
        .pt(px(8.0))
        .child(group)
        .child(
            div()
                .w_full()
                .pt(px(4.0))
                .text_center()
                .text_size(px(TEXT_TINY))
                .text_color(theme.text_muted)
                .child(status),
        )
        .child(handle)
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

#[cfg(test)]
mod strip_tests {
    //! Jacob, 2026-10-23 ("call + chat"): the call is a strip at the top of the open chat
    //! column — present only while my call is live there, resizable within bounds by its
    //! handle, gone when the call ends; the sidebar is never part of it.

    use super::strip_layout::{
        avatar_size, clamp_height, height_after_drag, max_height, shown, DEFAULT_HEIGHT,
        MIN_HEIGHT, TRANSCRIPT_MIN,
    };
    use crate::app::{ActiveCall, CallStatus};
    use crate::model::{Call, CallKind};

    fn live_call(space: &str) -> ActiveCall {
        ActiveCall {
            call: Call {
                call_id: "c1".into(),
                chat_id: space.into(),
                project_id: "people".into(),
                kind: CallKind::Voice,
                ..Default::default()
            },
            status: CallStatus::Connected,
            muted: false,
            camera: false,
            participants: Vec::new(),
            local_video: None,
            joined_at: 0,
            expanded: true,
            notice: None,
            outgoing: true,
            reachable: None,
            agent_seeing: false,
        }
    }

    #[test]
    fn the_strip_is_only_in_the_column_of_my_live_call() {
        let call = live_call("chat_product_launch");
        assert!(shown(Some(&call), "chat_product_launch"));
        assert!(
            !shown(Some(&call), "chat_other"),
            "another chat's column stays plain chat"
        );
        assert!(!shown(None, "chat_product_launch"), "no call, no strip");
        let mut collapsed = live_call("chat_product_launch");
        collapsed.expanded = false;
        assert!(
            !shown(Some(&collapsed), "chat_product_launch"),
            "collapsed = the slim bar"
        );
        let mut ended = live_call("chat_product_launch");
        ended.call.ended_at = Some(1);
        assert!(
            !shown(Some(&ended), "chat_product_launch"),
            "hang-up collapses the strip"
        );
    }

    #[test]
    fn the_handle_resizes_within_bounds() {
        let column = 780.0;
        assert_eq!(clamp_height(DEFAULT_HEIGHT, column), DEFAULT_HEIGHT);
        assert_eq!(
            height_after_drag(DEFAULT_HEIGHT, 40.0, column),
            DEFAULT_HEIGHT + 40.0
        );
        assert_eq!(
            height_after_drag(DEFAULT_HEIGHT, -500.0, column),
            MIN_HEIGHT
        );
        assert_eq!(
            height_after_drag(DEFAULT_HEIGHT, 5000.0, column),
            column - TRANSCRIPT_MIN
        );
        assert!(
            max_height(column) + TRANSCRIPT_MIN <= column,
            "the transcript keeps its room"
        );
        // A tiny window: the strip never goes under its minimum, whatever the column.
        assert_eq!(max_height(200.0), MIN_HEIGHT);
        assert_eq!(clamp_height(999.0, 200.0), MIN_HEIGHT);
        // Avatars follow the strip height between sensible sizes.
        assert_eq!(avatar_size(MIN_HEIGHT), 28.0);
        assert_eq!(avatar_size(DEFAULT_HEIGHT), 68.0);
        assert_eq!(avatar_size(600.0), 96.0);
    }
}
