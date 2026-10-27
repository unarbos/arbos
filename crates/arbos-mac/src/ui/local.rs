//! Settings → This Mac (F9): the `arbos-local` helpers paired to the relay, their online state,
//! the install command, and the per-device "Allow reads without asking" toggle. Commands the
//! agent wants to run on a computer always show an approval card; this toggle only lets
//! `read_file` / `list_dir` skip the card (they stay in the audit log).

use gpui::{
    div, prelude::*, px, App, ClipboardItem, Context, Div, Entity, EventEmitter, FocusHandle,
    Focusable, FontWeight, Render, Window,
};

use crate::app::AppState;
use crate::model::{now_ms, LocalDevice, LocalView};
use crate::ui::frost::{frosted, MENU_BLUR};
use crate::ui::profiles::kit;
use crate::ui::theme::{Theme, PANEL_RADIUS, RAIL_WIDTH};

pub enum LocalEvent {
    /// "‹ Settings": return to the settings popover.
    Back,
}

pub struct LocalPanel {
    state: Entity<AppState>,
    open: bool,
    /// Install instructions expanded.
    show_install: bool,
    /// Set for a moment after Copy so the button says so.
    copied_at: Option<u64>,
    /// Shown as a page inside the Settings sheet (#1086) rather than as the
    /// bottom-left popover; the sheet's header carries the back button.
    pub(crate) embedded: bool,
    focus_handle: FocusHandle,
}

impl EventEmitter<LocalEvent> for LocalPanel {}

/// What the user pastes into a terminal on the computer to reach (one line per step).
pub fn install_command(relay_url: &str) -> String {
    format!(
        "cargo install --git https://github.com/unarbos/arbos arbos-local\narbos-local pair <code> --relay {relay_url}\narbos-local run"
    )
}

fn ago(ms: Option<i64>) -> String {
    let Some(ms) = ms else {
        return "never seen".into();
    };
    let d = (now_ms() as i64 - ms).max(0) / 1000;
    if d < 5 {
        "just now".into()
    } else if d < 60 {
        format!("{d} s ago")
    } else if d < 3600 {
        format!("{} min ago", d / 60)
    } else if d < 86_400 {
        format!("{} h ago", d / 3600)
    } else {
        format!("{} d ago", d / 86_400)
    }
}

impl LocalPanel {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            open: false,
            show_install: false,
            copied_at: None,
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
        self.copied_at = None;
        self.state.update(cx, |s, cx| s.local_refresh(cx));
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
            .border_1()
            .border_color(theme.border)
            .child(text.into())
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
                el.text_color(theme.text_muted)
                    .border_1()
                    .border_color(theme.border)
                    .hover(|s| s.bg(theme.surface_2))
            })
            .child(label.into())
    }

    /// A small pill switch; the knob sits right when `on`.
    fn switch(id: impl Into<gpui::ElementId>, on: bool, theme: &Theme) -> gpui::Stateful<Div> {
        let knob = div()
            .absolute()
            .top(px(2.0))
            .size(px(12.0))
            .rounded_full()
            .bg(if on { theme.bg } else { theme.text_muted })
            .when(on, |k| k.right(px(2.0)))
            .when(!on, |k| k.left(px(2.0)));
        div()
            .id(id)
            .relative()
            .w(px(28.0))
            .h(px(16.0))
            .rounded_full()
            .cursor_pointer()
            .bg(if on { theme.accent } else { theme.surface_2 })
            .border_1()
            .border_color(if on {
                theme.accent
            } else {
                theme.border_strong
            })
            .child(knob)
    }

    fn render_device(&self, d: &LocalDevice, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let (status, fg, bg) = if d.connected {
            ("Online", theme.accent, theme.accent_soft)
        } else {
            ("Offline", theme.text_faint, theme.surface_2)
        };
        let mut sub = vec![d.os_label().to_owned()];
        if !d.hostname.is_empty() && d.hostname != d.name {
            sub.push(d.hostname.clone());
        }
        if !d.home.is_empty() {
            sub.push(d.home.clone());
        }
        let seen = if d.connected {
            "connected".to_owned()
        } else {
            format!("last seen {}", ago(d.last_seen))
        };
        let id_toggle = d.device_id.clone();
        let id_forget = d.device_id.clone();
        let allow = d.allow_reads;
        div()
            .flex()
            .flex_col()
            .gap(px(5.0))
            .py(px(8.0))
            .relative()
            .child(kit::hairline_below(theme))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .child(d.name.clone()),
                    )
                    .child(Self::chip(theme, status, fg, bg)),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.text_muted)
                    .child(format!("{} · {seen}", sub.join(" · "))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .pt(px(2.0))
                    .child(
                        Self::switch(format!("local-reads-{}", d.device_id), allow, theme).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.state.update(cx, |s, cx| {
                                    s.local_set_allow_reads(id_toggle.clone(), !allow, cx)
                                });
                            }),
                        ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.5))
                            .text_color(theme.text)
                            .child("Allow reads without asking"),
                    )
                    .child(
                        Self::button(
                            format!("local-forget-{}", d.device_id),
                            "Forget",
                            false,
                            theme,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state
                                .update(cx, |s, cx| s.local_forget(id_forget.clone(), cx));
                        })),
                    ),
            )
            .child(
                div()
                    .text_size(px(10.5))
                    .text_color(theme.text_faint)
                    .child(if allow {
                        "Files and folders are read without a card (still audited). Commands always ask."
                    } else {
                        "Every command, file read and folder listing shows an approval card first."
                    }),
            )
    }

    fn render_install(&self, view: &LocalView, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let relay = if view.relay_url.is_empty() {
            "http://127.0.0.1:7700".to_owned()
        } else {
            view.relay_url.clone()
        };
        let command = install_command(&relay);
        let copied = self
            .copied_at
            .is_some_and(|t| now_ms().saturating_sub(t) < 2500);
        let mut section = div().flex().flex_col().gap(px(6.0)).pt(px(8.0)).child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(11.0))
                        .text_color(theme.text_faint)
                        .child("INSTALL HELPER"),
                )
                .child(
                    div()
                        .id("local-install-toggle")
                        .text_size(px(11.5))
                        .text_color(theme.text_muted)
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme.text))
                        .child(if self.show_install { "Hide" } else { "Show" })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_install = !this.show_install;
                            cx.notify();
                        })),
                ),
        );
        if !self.show_install {
            return section;
        }
        let command_for_copy = command.clone();
        section = section
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(theme.text_muted)
                    .child("On the computer the agent should reach, run (get a one-time code with `arbos-relay pair-code` on the relay host):"),
            )
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .p(px(8.0))
                            .rounded(px(7.0))
                            .bg(theme.surface_2)
                            .border_1()
                            .border_color(theme.border)
                            .font_family(theme.font_mono)
                            .text_size(px(10.5))
                            .text_color(theme.text)
                            .children(command.lines().map(|l| div().child(l.to_owned()))),
                    )
                    .child(
                        Self::button(
                            "local-install-copy",
                            if copied { "Copied" } else { "Copy" },
                            true,
                            theme,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                command_for_copy.clone(),
                            ));
                            this.copied_at = Some(now_ms());
                            cx.notify();
                        })),
                    ),
            )
            .child(
                div()
                    .text_size(px(10.5))
                    .text_color(theme.text_faint)
                    .child("The helper keeps one WebSocket to the relay and reconnects by itself. It only acts on requests you approved in a chat."),
            );
        section
    }
}

impl Focusable for LocalPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for LocalPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let view = self.state.read(cx).local.clone();

        let embedded = self.embedded;
        let header = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .when(!embedded, |el| {
                el.child(
                    div()
                        .id("local-back")
                        .text_size(px(11.5))
                        .text_color(theme.text_muted)
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme.text))
                        .child("‹ Settings")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.hide(cx);
                            cx.emit(LocalEvent::Back);
                        })),
                )
            })
            .child(
                div()
                    .flex_1()
                    .text_size(px(11.0))
                    .text_color(theme.text_faint)
                    .when(!embedded, |el| el.child("THIS MAC")),
            )
            .child(
                div()
                    .id("local-refresh")
                    .text_size(px(11.5))
                    .text_color(theme.text_muted)
                    .cursor_pointer()
                    .hover(|s| s.text_color(theme.text))
                    .child("Refresh")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.update(cx, |s, cx| s.local_refresh(cx));
                    })),
            );

        let mut list = div().flex().flex_col();
        if view.devices.is_empty() {
            list = list.child(
                div()
                    .py(px(10.0))
                    .text_size(px(11.5))
                    .text_color(theme.text_muted)
                    .child(if view.loaded {
                        "No helper has connected yet. Install one below."
                    } else {
                        "Loading from the relay…"
                    }),
            );
        }
        for d in &view.devices {
            list = list.child(self.render_device(d, &theme, cx));
        }

        let card = div()
            .id("local-popover")
            .track_focus(&self.focus_handle)
            .flex()
            .flex_col()
            .gap(px(6.0))
            .p(px(10.0))
            .child(header)
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.text_muted)
                    .child("Computers your agents can reach through the arbos-local helper. The exact command or path is shown in an approval card before anything runs."),
            )
            .child(list)
            .child(self.render_install(&view, &theme, cx));
        if embedded {
            return card.w_full().into_any_element();
        }
        let card = card
            .w(px(400.0))
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
