//! Settings → Connectors (F1): the user's external accounts as the relay knows them.
//! Connect opens the provider's OAuth page in the browser (the relay finishes the loopback
//! callback); token providers take a pasted key; Disconnect deletes the relay-side tokens.

use std::rc::Rc;

use gpui::{
    div, prelude::*, px, App, Context, Div, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, Render, Window,
};

use crate::app::AppState;
use crate::model::{ConnectorProvider, ConnectorStatus, ConnectorsView};
use crate::sync::user_error::UserError;
use crate::ui::composer::ComposerInput;
use crate::ui::error_note;
use crate::ui::frost::{frosted, MENU_BLUR};
use crate::ui::glass;
use crate::ui::highlight;
use crate::ui::profiles::kit;
use crate::ui::sf_symbols;
use crate::ui::theme::{Theme, PANEL_RADIUS, RAIL_WIDTH};

pub enum ConnectorsEvent {
    /// "‹ Settings": return to the settings popover.
    Back,
}

pub struct ConnectorsPanel {
    state: Entity<AppState>,
    token_input: Entity<ComposerInput>,
    /// Provider the token field is for (shown only while `Some`).
    token_for: Option<String>,
    /// Provider whose Connect menu (sign in / paste a token) is open.
    menu_for: Option<String>,
    /// Connected pill clicked: (connector id, provider) awaiting the Disconnect confirmation.
    disconnecting: Option<(String, String)>,
    open: bool,
    /// Shown as a page inside the Settings sheet (#1086) rather than as the
    /// bottom-left popover; the sheet's header carries the back button.
    pub(crate) embedded: bool,
    focus_handle: FocusHandle,
}

impl EventEmitter<ConnectorsEvent> for ConnectorsPanel {}

impl ConnectorsPanel {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let token_input = cx.new(ComposerInput::new);
        {
            let weak = cx.weak_entity();
            token_input.update(cx, |input, _| {
                input.set_placeholder("Paste a token, then Enter");
                input.on_submit = Some(Rc::new(move |_window, cx| {
                    let _ = weak.update(cx, |this, cx| this.submit_token(cx));
                }));
            });
        }
        cx.observe(&token_input, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            token_input,
            token_for: None,
            menu_for: None,
            disconnecting: None,
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
        self.state.update(cx, |s, cx| s.refresh_connectors(cx));
        cx.notify();
    }

    pub fn hide(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        self.embedded = false;
        self.token_for = None;
        self.menu_for = None;
        self.disconnecting = None;
        cx.notify();
    }

    /// True while the token field has keyboard focus (the shell must not steal it).
    pub fn input_focused(&self, window: &Window, cx: &App) -> bool {
        self.token_for.is_some()
            && self
                .token_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
    }

    fn submit_token(&mut self, cx: &mut Context<Self>) {
        let Some(provider) = self.token_for.clone() else {
            return;
        };
        let token = self.token_input.read(cx).text().trim().to_string();
        if token.is_empty() {
            return;
        }
        self.token_input.update(cx, |input, cx| input.clear(cx));
        self.token_for = None;
        self.state
            .update(cx, |s, cx| s.connect_connector(provider, Some(token), cx));
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
        label: &'static str,
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
            .child(label)
    }

    /// Grok's Plugins row (web-03), the iPhone's it.19 treatment: icon tile, title
    /// over a one-line status, one pill on the right — "Connect" (grey) or
    /// "✓ Connected" (green tint). OAuth + token providers open a two-item menu
    /// under the row, token-only ones open the paste field, and a connected
    /// pill asks before disconnecting. The token flow itself is unchanged.
    fn render_row(
        &self,
        view: &ConnectorsView,
        p: &ConnectorProvider,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        let cn = view.connector_for(&p.provider);
        let status = cn.map(|c| c.status);
        let connected = matches!(status, Some(ConnectorStatus::Connected));
        let approvals = p
            .actions
            .iter()
            .filter(|a| a.kind == "write" || a.kind == "send")
            .count();
        let sub = match (cn, status) {
            (Some(c), Some(ConnectorStatus::Connected)) if !c.label.is_empty() => c.label.clone(),
            (_, Some(s @ ConnectorStatus::Connected)) => s.label().to_string(),
            (_, Some(s @ (ConnectorStatus::NeedsAuth | ConnectorStatus::Error))) => {
                s.label().to_string()
            }
            _ if !p.has_oauth() => "Connects with an API key".to_owned(),
            _ if !p.oauth_configured && !view.mock => {
                if p.accepts_token() {
                    "Sign-in not set up — paste a token".to_owned()
                } else {
                    "Sign-in not set up on the relay".to_owned()
                }
            }
            _ => format!("{} actions · {approvals} need approval", p.actions.len()),
        };
        let sub_color = match status {
            Some(ConnectorStatus::NeedsAuth) => theme.warn,
            Some(ConnectorStatus::Error) => theme.danger,
            _ => theme.text_muted,
        };
        let provider = p.provider.clone();
        let token_open = self.token_for.as_deref() == Some(p.provider.as_str());
        let menu_open = self.menu_for.as_deref() == Some(p.provider.as_str());
        let confirm_open = self
            .disconnecting
            .as_ref()
            .is_some_and(|(_, prov)| prov == &p.provider);

        let pill: gpui::Stateful<Div> = if token_open || menu_open || confirm_open {
            Self::pill(
                format!("cancel-{}", p.provider),
                "Cancel",
                false,
                false,
                theme,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.menu_for = None;
                this.token_for = None;
                this.disconnecting = None;
                cx.notify();
            }))
        } else if connected {
            let id = cn.map(|c| c.id.clone()).unwrap_or_default();
            let prov = provider.clone();
            Self::pill(
                format!("connected-{}", p.provider),
                "Connected",
                true,
                true,
                theme,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.menu_for = None;
                this.token_for = None;
                this.disconnecting = Some((id.clone(), prov.clone()));
                cx.notify();
            }))
        } else if p.has_oauth() && p.accepts_token() {
            let prov = provider.clone();
            Self::pill(
                format!("connect-{}", p.provider),
                "Connect",
                false,
                false,
                theme,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.menu_for = Some(prov.clone());
                cx.notify();
            }))
        } else if p.has_oauth() {
            let prov = provider.clone();
            Self::pill(
                format!("connect-{}", p.provider),
                "Connect",
                false,
                false,
                theme,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.start_oauth(prov.clone(), cx);
            }))
        } else {
            let prov = provider.clone();
            Self::pill(
                format!("connect-{}", p.provider),
                "Connect",
                false,
                false,
                theme,
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_token(prov.clone(), window, cx);
            }))
        };

        let mut row = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .py(px(9.0))
            .relative()
            .child(kit::hairline_below(theme))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(glass::icon_tile(Self::glyph(&p.provider), theme))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.text)
                                    .child(p.label.clone()),
                            )
                            .child(div().text_size(px(11.0)).text_color(sub_color).child(sub)),
                    )
                    .child(pill),
            );
        if let Some(err) = cn.and_then(|c| c.last_error.clone()) {
            if !connected {
                // The connector's last failure, in the one shared element (no Try again here:
                // Connect in the row is the retry).
                let error = UserError::from_text(format!("connect {}", p.label), err);
                row = row.child(div().pl(px(42.0)).pt(px(6.0)).child(error_note::render(
                    &format!("connector-error-{}", p.provider),
                    &error,
                    false,
                    error_note::Style::Inline,
                    theme,
                    error_note::Handlers::new(),
                )));
            }
        }
        if menu_open {
            let (prov_a, prov_b) = (provider.clone(), provider.clone());
            row = row.child(
                div()
                    .ml(px(42.0))
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .p(px(4.0))
                    .rounded(px(10.0))
                    .bg(theme.surface_2)
                    .border_1()
                    .border_color(theme.border)
                    .child(
                        glass::menu_row(
                            format!("oauth-{}", p.provider),
                            "globe",
                            format!("Sign in with {}", Self::short_name(p)),
                            false,
                            theme,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.start_oauth(prov_a.clone(), cx);
                        })),
                    )
                    .child(
                        glass::menu_row(
                            format!("paste-{}", p.provider),
                            "doc.on.clipboard",
                            "Paste a token…",
                            false,
                            theme,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.open_token(prov_b.clone(), window, cx);
                            },
                        )),
                    ),
            );
        }
        if confirm_open {
            let keep = Self::button(format!("keep-{}", p.provider), "Keep", false, theme).on_click(
                cx.listener(|this, _, _, cx| {
                    this.disconnecting = None;
                    cx.notify();
                }),
            );
            let go = Self::button(
                format!("disconnect-{}", p.provider),
                "Disconnect",
                true,
                theme,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some((id, _)) = this.disconnecting.take() {
                    this.state
                        .update(cx, |s, cx| s.disconnect_connector(id, cx));
                }
                cx.notify();
            }));
            row = row.child(
                div()
                    .ml(px(42.0))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme.text_muted)
                            .child(format!(
                            "Disconnect {}? Your agents stop using it until you connect it again.",
                            Self::short_name(p)
                        )),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(6.0))
                            .child(keep)
                            .child(go),
                    ),
            );
        }
        if token_open {
            row = row.child(
                div()
                    .ml(px(42.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex_1()
                            .h(px(30.0))
                            .px(px(10.0))
                            .rounded(px(7.0))
                            .bg(theme.surface_2)
                            .border_1()
                            .border_color(theme.border_strong)
                            .child(self.token_input.clone()),
                    )
                    .child(
                        Self::button(format!("save-token-{}", p.provider), "Save", true, theme)
                            .on_click(cx.listener(|this, _, _, cx| this.submit_token(cx))),
                    ),
            );
        }
        row
    }

    /// The Grok "Add" / "✓ Added" pill: 26 pt tall, 11.5 pt semibold.
    fn pill(
        id: impl Into<gpui::ElementId>,
        label: &'static str,
        check: bool,
        on: bool,
        theme: &Theme,
    ) -> gpui::Stateful<Div> {
        let fg = if on { theme.online } else { theme.text };
        div()
            .id(id)
            .flex_shrink_0()
            .h(px(26.0))
            .px(px(11.0))
            .flex()
            .items_center()
            .gap(px(5.0))
            .rounded_full()
            .cursor_pointer()
            .text_size(px(11.5))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(fg)
            .border_1()
            .border_color(if on {
                Theme::with_alpha(theme.online, 0.35)
            } else {
                theme.field_border
            })
            .map(|el| {
                let base = if on {
                    Theme::with_alpha(theme.online, 0.14)
                } else {
                    theme.field
                };
                highlight::washed(el, base, theme)
            })
            .when(check, |el| {
                el.child(sf_symbols::glyph("checkmark", 11.0, fg))
            })
            .child(label)
    }

    fn glyph(provider: &str) -> &'static str {
        match provider {
            "google" => "envelope",
            "slack" => "number",
            "github" => "chevron.left.forwardslash.chevron.right",
            "harness_comms" => "antenna.radiowaves.left.and.right",
            _ => "puzzlepiece",
        }
    }

    fn short_name(p: &ConnectorProvider) -> String {
        match p.provider.as_str() {
            "google" => "Google".into(),
            "slack" => "Slack".into(),
            "github" => "GitHub".into(),
            _ => p.label.clone(),
        }
    }

    fn start_oauth(&mut self, provider: String, cx: &mut Context<Self>) {
        self.menu_for = None;
        self.token_for = None;
        self.disconnecting = None;
        self.state
            .update(cx, |s, cx| s.connect_connector(provider, None, cx));
        cx.notify();
    }

    fn open_token(&mut self, provider: String, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_for = None;
        self.disconnecting = None;
        self.token_for = Some(provider);
        self.token_input.update(cx, |input, cx| input.clear(cx));
        let handle = self.token_input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }
}

impl Focusable for ConnectorsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ConnectorsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let view = self.state.read(cx).connectors.clone();

        let embedded = self.embedded;
        let mut header = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .when(!embedded, |el| {
                el.child(
                    div()
                        .id("connectors-back")
                        .text_size(px(11.5))
                        .text_color(theme.text_muted)
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme.text))
                        .child("‹ Settings")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.hide(cx);
                            cx.emit(ConnectorsEvent::Back);
                        })),
                )
            })
            .child(
                div()
                    .flex_1()
                    .text_size(px(11.0))
                    .text_color(theme.text_faint)
                    .when(!embedded, |el| el.child("CONNECTORS")),
            );
        if view.mock {
            header = header.child(Self::chip(&theme, "Mock", theme.warn, theme.surface_2));
        }
        header = header.child(
            div()
                .id("connectors-refresh")
                .text_size(px(11.5))
                .text_color(theme.text_muted)
                .cursor_pointer()
                .hover(|s| s.text_color(theme.text))
                .child("Refresh")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state.update(cx, |s, cx| s.refresh_connectors(cx));
                })),
        );

        let mut list = div().flex().flex_col();
        if view.providers.is_empty() {
            list = list.child(
                div()
                    .py(px(10.0))
                    .text_size(px(11.5))
                    .text_color(theme.text_muted)
                    .child(if view.loaded {
                        "The relay offers no connectors."
                    } else {
                        "Loading from the relay…"
                    }),
            );
        }
        for p in &view.providers {
            list = list.child(self.render_row(&view, p, &theme, cx));
        }

        let card = div()
            .id("connectors-popover")
            .track_focus(&self.focus_handle)
            .flex()
            .flex_col()
            .gap(px(6.0))
            .p(px(10.0))
            .child(header)
            .child(list)
            .child(
                div()
                    .pt(px(4.0))
                    .text_size(px(10.5))
                    .text_color(theme.text_faint)
                    .child(if view.mock {
                        "Mock mode: canned replies; sends land in the relay's connector_outbox. Tokens never leave the relay."
                    } else {
                        "Tokens stay on the relay; the agent acts through POST /connectors/call and asks before it sends."
                    }),
            );
        if embedded {
            return card.w_full().into_any_element();
        }
        let card = card
            .w(px(360.0))
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
