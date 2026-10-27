//! Settings → Channels (F7): reach a Project from Slack, Telegram, WhatsApp, or a Beeper chat.
//! Platform rows show whether the relay holds the platform's credentials (or runs in mock
//! mode); "New pairing code" mints a 6-digit code with a 10-minute countdown that the user
//! sends as `/pair <code>` from the channel; bound chats can be unbound.

use gpui::{
    div, prelude::*, px, App, Context, Div, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, Render, Window,
};

use crate::app::AppState;
use crate::model::{ChannelBinding, ChannelPlatform, ChannelPlatformState, ChannelsView};
use crate::ui::frost::{frosted, MENU_BLUR};
use crate::ui::highlight;
use crate::ui::motion::PulseClock;
use crate::ui::theme::{Theme, PANEL_RADIUS, RAIL_WIDTH};

pub enum ChannelsEvent {
    /// "‹ Settings": return to the settings popover.
    Back,
}

pub struct ChannelsPanel {
    state: Entity<AppState>,
    open: bool,
    /// Shown as a page inside the Settings sheet (#1086) rather than as the
    /// bottom-left popover; the sheet's header carries the back button.
    pub(crate) embedded: bool,
    focus_handle: FocusHandle,
}

impl EventEmitter<ChannelsEvent> for ChannelsPanel {}

impl ChannelsPanel {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            open: false,
            embedded: false,
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Open as the bottom-left popover (not inside the Settings sheet).
    pub fn is_floating(&self) -> bool {
        self.open && !self.embedded
    }

    pub fn show(&mut self, cx: &mut Context<Self>) {
        self.open = true;
        self.state.update(cx, |s, cx| s.refresh_channels(cx));
        cx.notify();
    }

    pub fn hide(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        self.embedded = false;
        cx.notify();
    }

    fn chip(
        theme: &Theme,
        text: impl Into<gpui::SharedString>,
        fg: gpui::Hsla,
        bg: gpui::Hsla,
    ) -> Div {
        div()
            .px(px(7.0))
            .py(px(2.0))
            .rounded(px(5.0))
            .text_size(px(10.5))
            .font_weight(FontWeight::SEMIBOLD)
            .bg(bg)
            .text_color(fg)
            .child(text.into())
            .border_1()
            .border_color(theme.border)
    }

    fn button(
        id: impl Into<gpui::ElementId>,
        label: impl Into<gpui::SharedString>,
        primary: bool,
        theme: &Theme,
    ) -> gpui::Stateful<Div> {
        div()
            .id(id)
            .px(px(9.0))
            .py(px(4.0))
            .rounded(px(6.0))
            .text_size(px(11.5))
            .cursor_pointer()
            .when(primary, |el| {
                el.bg(theme.text)
                    .text_color(theme.bg)
                    .font_weight(FontWeight::SEMIBOLD)
            })
            .when(!primary, |el| {
                highlight::button(
                    el.text_color(theme.text_muted)
                        .border_1()
                        .border_color(theme.border),
                    theme,
                )
            })
            .child(label.into())
    }

    fn platform_glyph(platform: &str) -> &'static str {
        match platform {
            "slack" => "#",
            "telegram" => "✈",
            "whatsapp" => "☎",
            "beeper" => "◎",
            _ => "•",
        }
    }

    fn render_platform(&self, p: &ChannelPlatform, theme: &Theme) -> Div {
        let (fg, bg) = match p.state {
            ChannelPlatformState::Configured => (theme.accent, theme.accent_soft),
            ChannelPlatformState::Missing => (theme.warn, theme.surface_2),
            ChannelPlatformState::Mock => (theme.warn, theme.surface_2),
            ChannelPlatformState::Other => (theme.text_muted, theme.surface_2),
        };
        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .py(px(6.0))
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .w(px(16.0))
                            .text_size(px(12.0))
                            .text_color(theme.text_muted)
                            .child(Self::platform_glyph(&p.platform)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .child(p.label.clone()),
                    )
                    .child(Self::chip(theme, p.state.label(), fg, bg)),
            )
            .child(
                div()
                    .pl(px(24.0))
                    .text_size(px(10.5))
                    .text_color(theme.text_faint)
                    .child(p.detail.clone()),
            )
    }

    fn render_pairing(&self, view: &ChannelsView, theme: &Theme, cx: &mut Context<Self>) -> Div {
        if view.pairing.is_some() {
            // Live countdown: ask the shared clock for ticks while the code is showing.
            PulseClock::request(cx);
        }
        let state = self.state.read(cx);
        let project_name = state
            .selected_project
            .as_deref()
            .and_then(|id| state.project(id))
            .map(|p| p.name.clone());
        let mut section = div().flex().flex_col().gap(px(6.0)).py(px(8.0));
        match &view.pairing {
            Some(code) => {
                let left = code.seconds_left();
                let target = state
                    .project(&code.project_id)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| code.project_id.clone());
                let chat_title = state
                    .chats
                    .get(&code.project_id)
                    .and_then(|cs| cs.iter().find(|c| c.id == code.chat_id))
                    .map(|c| c.title.clone())
                    .unwrap_or_else(|| "pinned chat".into());
                let spaced: String = code
                    .code
                    .chars()
                    .enumerate()
                    .flat_map(|(i, c)| if i == 3 { vec![' ', c] } else { vec![c] })
                    .collect();
                section = section
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .child(
                                div()
                                    .px(px(12.0))
                                    .py(px(6.0))
                                    .rounded(px(8.0))
                                    .bg(theme.surface_2)
                                    .border_1()
                                    .border_color(theme.border_strong)
                                    .font_family(theme.font_mono)
                                    .text_size(px(22.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme.text)
                                    .child(spaced),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_size(px(11.5))
                                            .text_color(theme.text)
                                            .child(format!("→ {target} · {chat_title}")),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(if left < 60 {
                                                theme.warn
                                            } else {
                                                theme.text_muted
                                            })
                                            .child(format!(
                                                "expires in {}:{:02}",
                                                left / 60,
                                                left % 60
                                            )),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme.text_muted)
                            .child(format!(
                                "Send  /pair {}  from Slack, Telegram, WhatsApp, or a Beeper chat you own. One use.",
                                code.code
                            )),
                    );
            }
            None => {
                section = section.child(
                    div()
                        .text_size(px(11.0))
                        .text_color(theme.text_muted)
                        .child(match &project_name {
                            Some(n) => format!(
                                "A code binds one external chat to {n} (its pinned chat) for 10 minutes."
                            ),
                            None => "Select a project, then mint a code.".to_owned(),
                        }),
                );
            }
        }
        section.child(
            div().flex().child(
                Self::button(
                    "channels-pair-new",
                    if view.pairing.is_some() {
                        "New code"
                    } else {
                        "New pairing code"
                    },
                    view.pairing.is_none(),
                    theme,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state.update(cx, |s, cx| s.new_channel_pairing(cx));
                })),
            ),
        )
    }

    fn render_binding(&self, b: &ChannelBinding, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let state = self.state.read(cx);
        let project = state
            .project(&b.project_id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| b.project_id.clone());
        let chat = state
            .chats
            .get(&b.project_id)
            .and_then(|cs| cs.iter().find(|c| c.id == b.chat_id))
            .map(|c| c.title.clone())
            .unwrap_or_else(|| "chat".into());
        let label = if b.label.is_empty() {
            format!("{} · {}", b.platform, b.external_chat_id)
        } else {
            b.label.clone()
        };
        let id = b.id.clone();
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .py(px(6.0))
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .w(px(16.0))
                    .text_size(px(12.0))
                    .text_color(theme.text_muted)
                    .child(Self::platform_glyph(&b.platform)),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme.text)
                            .child(label),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(theme.text_faint)
                            .child(format!("→ {project} · {chat}")),
                    ),
            )
            .child(
                Self::button(format!("unbind-{}", b.id), "Unbind", false, theme).on_click(
                    cx.listener(move |this, _, _, cx| {
                        let id = id.clone();
                        this.state.update(cx, |s, cx| s.unbind_channel(id, cx));
                    }),
                ),
            )
    }

    fn section_title(theme: &Theme, text: &'static str) -> Div {
        div()
            .pt(px(8.0))
            .text_size(px(10.5))
            .text_color(theme.text_faint)
            .child(text)
    }
}

impl Focusable for ChannelsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ChannelsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let view = self.state.read(cx).channels.clone();

        let embedded = self.embedded;
        let mut header = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .when(!embedded, |el| {
                el.child(
                    div()
                        .id("channels-back")
                        .text_size(px(11.5))
                        .text_color(theme.text_muted)
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme.text))
                        .child("‹ Settings")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.hide(cx);
                            cx.emit(ChannelsEvent::Back);
                        })),
                )
            })
            .child(
                div()
                    .flex_1()
                    .text_size(px(11.0))
                    .text_color(theme.text_faint)
                    .when(!embedded, |el| el.child("CHANNELS")),
            );
        if view.mock {
            header = header.child(Self::chip(&theme, "Mock", theme.warn, theme.surface_2));
        }
        header = header.child(
            div()
                .id("channels-refresh")
                .text_size(px(11.5))
                .text_color(theme.text_muted)
                .cursor_pointer()
                .hover(|s| s.text_color(theme.text))
                .child("Refresh")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state.update(cx, |s, cx| s.refresh_channels(cx));
                })),
        );

        let mut platforms = div().flex().flex_col();
        if view.platforms.is_empty() {
            platforms = platforms.child(
                div()
                    .py(px(10.0))
                    .text_size(px(11.5))
                    .text_color(theme.text_muted)
                    .child(if view.loaded {
                        "The relay offers no channels."
                    } else {
                        "Loading from the relay…"
                    }),
            );
        }
        for p in &view.platforms {
            platforms = platforms.child(self.render_platform(p, &theme));
        }

        let mut bindings = div().flex().flex_col();
        if view.bindings.is_empty() {
            bindings = bindings.child(
                div()
                    .py(px(6.0))
                    .text_size(px(11.0))
                    .text_color(theme.text_muted)
                    .child(
                        "No chats paired yet. Unpaired senders get one hint and never reach a box.",
                    ),
            );
        }
        for b in &view.bindings {
            bindings = bindings.child(self.render_binding(b, &theme, cx));
        }

        let card = div()
            .id("channels-popover")
            .track_focus(&self.focus_handle)
            .flex()
            .flex_col()
            .gap(px(4.0))
            .p(px(10.0))
            .child(header)
            .child(Self::section_title(&theme, "PAIR A CHAT"))
            .child(self.render_pairing(&view, &theme, cx))
            .child(Self::section_title(&theme, "PLATFORMS"))
            .child(platforms)
            .child(Self::section_title(&theme, "BOUND CHATS"))
            .child(bindings)
            .child(
                div()
                    .pt(px(6.0))
                    .text_size(px(10.5))
                    .text_color(theme.text_faint)
                    .child(if view.mock {
                        "Mock mode: replies land in the relay's channel_outbox; inject messages with arbosctl channels inbound."
                    } else {
                        "Messages from a paired chat run as your turn in the bound chat; the reply goes back the same way."
                    }),
            );
        if embedded {
            return card.w_full().into_any_element();
        }
        let card = card
            .w(px(380.0))
            .rounded(px(PANEL_RADIUS))
            .bg(theme.card())
            .border_1()
            .border_color(theme.border_strong)
            .shadow_lg();
        div()
            .absolute()
            .left(px(RAIL_WIDTH + 8.0))
            .bottom(px(14.0))
            .child(frosted(PANEL_RADIUS, MENU_BLUR, card))
            .into_any_element()
    }
}
