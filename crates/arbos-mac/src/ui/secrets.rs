//! Settings → Secrets (D83, `docs/spec-secrets.md` §8.7): the kill switch, connected vaults,
//! the secrets list, a Secret page with per-agent switches, the New secret form with agent
//! chips, requests waiting for the four D76 answers, and the agent's Secrets page. D80 grouped
//! cards inside the Apps & tools panel. No value is ever displayed: the relay never sends one.

use std::rc::Rc;

use gpui::{
    div, prelude::*, px, App, Context, Div, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, Render, SharedString, Stateful, Window,
};

use crate::app::AppState;
use crate::model::{
    ago, person_usage, secret_env_name, secret_usage, ConnectedVault, Id, NewSecret,
    ProjectSecrets, Secret, SecretActivity, SecretRequest, SecretsView,
};
use crate::ui::agent_picker::{self, AgentPicker};
use crate::ui::avatars;
use crate::ui::composer::ComposerInput;
use crate::ui::error_note;
use crate::ui::frost::{frosted, MENU_BLUR};
use crate::ui::glass;
use crate::ui::highlight::{self, Shape};
use crate::ui::profiles::kit;
use crate::ui::sf_symbols;
use crate::ui::theme::{Theme, PANEL_RADIUS, RAIL_WIDTH, TEXT_BODY, TEXT_SMALL, TEXT_TINY};

pub enum SecretsEvent {
    /// "‹ Settings": return to the settings sheet.
    Back,
}

/// The failures the Secrets pane draws inline while one of its forms is up (the verbs
/// `sync::relay` reports them under).
const SECRETS_VERBS: &[&str] = &[
    "save the secret",
    "replace the value",
    "delete the secret",
    "change who may use it",
    "revoke the access",
    "change that person's access",
    "remove that person",
    "add @",
    "change the pause",
    "answer the request",
    "connect 1Password",
    "replace the token",
    "change the vault access",
    "save the item name",
    "disconnect the vault",
    "load your secrets",
    "load the agent's secrets",
];

#[derive(Clone, Debug, PartialEq, Eq)]
enum Page {
    List,
    Secret(Id),
    Add,
    Agent(Id),
    Connect,
    Vault(Id),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AddKind {
    Key,
    Login,
    Text,
}

impl AddKind {
    fn as_str(self) -> &'static str {
        match self {
            AddKind::Key => "key",
            AddKind::Login => "login",
            AddKind::Text => "text",
        }
    }
}

/// The note field: what agents may do with the key (the iPhone's `SecretsText` twin).
// One line in the Mac field (the iPhone's field is multiline and carries the longer example).
const NOTE_PLACEHOLDER: &str = "What may agents do with it? e.g. read-only";
const NOTE_HINT: &str = "Agents follow this as the rule for the key — for egress and for programs that read the real value. Say what it is for, what is allowed, and any spend limit; without one they confirm before anything that charges.";
const ACCESS_NOTE_HINT: &str = "Shown whenever the agent sees this secret, on top of the secret's own note. Allowed actions and spend limits for this agent, e.g. reports only · may spend up to $5 a day.";
const VAULT_NOTE_HINT: &str = "The rule for every item in this vault, for the agents you pick: what it is for, what is allowed, any spend limit. No spending guidance means they confirm before anything that charges.";

pub struct SecretsPanel {
    state: Entity<AppState>,
    open: bool,
    page: Page,
    add_kind: AddKind,
    /// Add form: "Real value in the agent's environment" from the start.
    add_inject: bool,
    /// The agents picked in the Add / Connect / Give-access forms and the "Add agent"
    /// dropdown — one component for the four pickers (`ui::agent_picker`). The agent
    /// page's "Give access to a secret…" list reuses `picked` for secret ids.
    pub(crate) agent_picker: AgentPicker,
    /// Inline pickers open on the Secret / Agent pages.
    giving: bool,
    replacing: bool,
    /// Two-click confirmations: the row id armed for deletion / pause.
    confirm: Option<String>,
    name_input: Entity<ComposerInput>,
    url_input: Entity<ComposerInput>,
    user_input: Entity<ComposerInput>,
    secret_input: Entity<ComposerInput>,
    note_input: Entity<ComposerInput>,
    hosts_input: Entity<ComposerInput>,
    token_input: Entity<ComposerInput>,
    error: Option<String>,
    local_details_open: bool,
    /// Rig: a secret env name to open as soon as the list arrives.
    pending_env: Option<String>,
    /// Rig: with `pending_env`, open that secret's "Give a person access" sheet too.
    pending_people: bool,
    /// Rig: open the first vault as soon as the list arrives.
    pending_vault: bool,
    /// Rig: `ARBOS_RIG_AGENT_PICKED=<name>,<name>` — these agents start as chips of the
    /// open form (applied once the registry lists them).
    rig_picked: Option<String>,
    /// Rig: `ARBOS_RIG_AGENT_PICKER=<query>` — the "Add agent" dropdown starts open with
    /// that query typed (empty = open, nothing typed).
    rig_query: Option<String>,
    /// Shown as a page inside the Settings sheet (#1086) rather than as the
    /// bottom-left popover; the sheet's header carries the back button.
    pub(crate) embedded: bool,
    focus_handle: FocusHandle,
}

impl EventEmitter<SecretsEvent> for SecretsPanel {}

impl SecretsPanel {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let mk = |cx: &mut Context<Self>, placeholder: &str, masked: bool| {
            let input = cx.new(ComposerInput::new);
            input.update(cx, |i, _| {
                i.set_placeholder(placeholder);
                if masked {
                    i.set_masked(true);
                }
            });
            cx.observe(&input, |_, _, cx| cx.notify()).detach();
            input
        };
        let name_input = mk(cx, "STRIPE_KEY", false);
        let url_input = mk(cx, "https://github.com/login", false);
        let user_input = mk(cx, "name@example.com", false);
        let secret_input = mk(cx, "sk_live_…", true);
        let note_input = mk(cx, NOTE_PLACEHOLDER, false);
        let hosts_input = mk(cx, "api.stripe.com", false);
        let token_input = mk(cx, "ops_••••••••••••••••••••", true);
        {
            let weak = cx.weak_entity();
            hosts_input.update(cx, |input, _| {
                input.on_submit = Some(Rc::new(move |_window, cx| {
                    let _ = weak.update(cx, |this, cx| this.submit_add(cx));
                }));
            });
        }
        Self {
            state,
            open: false,
            page: Page::List,
            add_kind: AddKind::Key,
            add_inject: false,
            agent_picker: AgentPicker::new(cx),
            giving: false,
            replacing: false,
            confirm: None,
            name_input,
            url_input,
            user_input,
            secret_input,
            note_input,
            hosts_input,
            token_input,
            error: None,
            local_details_open: false,
            pending_env: None,
            pending_people: false,
            pending_vault: false,
            rig_picked: std::env::var("ARBOS_RIG_AGENT_PICKED").ok(),
            rig_query: std::env::var("ARBOS_RIG_AGENT_PICKER").ok(),
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
        self.page = Page::List;
        self.state.update(cx, |s, cx| s.secrets_refresh(cx));
        cx.notify();
    }

    /// Rig: open on one secret by env name once the relay (or fixture) has listed it.
    pub fn show_secret_env(&mut self, env: String, cx: &mut Context<Self>) {
        self.open = true;
        self.pending_env = Some(env);
        self.page = Page::List;
        self.state.update(cx, |s, cx| s.secrets_refresh(cx));
        cx.notify();
    }

    /// Rig: one secret's page by env name with its "Give a person access" sheet
    /// open once the relay (or fixture) has listed it.
    pub fn show_secret_people(&mut self, env: String, cx: &mut Context<Self>) {
        self.pending_people = true;
        self.show_secret_env(env, cx);
    }

    /// Rig: the New secret form.
    pub fn show_add(&mut self, cx: &mut Context<Self>) {
        self.open = true;
        self.page = Page::Add;
        self.state.update(cx, |s, cx| s.secrets_refresh(cx));
        cx.notify();
    }

    /// Rig: the Connect 1Password form.
    pub fn show_connect(&mut self, cx: &mut Context<Self>) {
        self.open = true;
        self.page = Page::Connect;
        self.state.update(cx, |s, cx| s.secrets_refresh(cx));
        cx.notify();
    }

    /// Rig: the first connected vault's page once the list arrives.
    pub fn show_first_vault(&mut self, cx: &mut Context<Self>) {
        self.open = true;
        self.pending_vault = true;
        self.page = Page::List;
        self.state.update(cx, |s, cx| s.secrets_refresh(cx));
        cx.notify();
    }

    /// Agent profile → secrets: open straight on that agent's page.
    pub fn show_agent(&mut self, project_id: Id, cx: &mut Context<Self>) {
        let project_id = self
            .state
            .read(cx)
            .projects
            .iter()
            .find(|p| p.id == project_id || p.name.eq_ignore_ascii_case(&project_id))
            .map(|p| p.id.clone())
            .unwrap_or(project_id);
        self.open = true;
        self.page = Page::Agent(project_id.clone());
        self.state.update(cx, |s, cx| {
            s.secrets_refresh(cx);
            s.project_secrets_refresh(project_id, cx);
        });
        cx.notify();
    }

    pub fn hide(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        self.embedded = false;
        self.reset_form(cx);
        self.page = Page::List;
        cx.notify();
    }

    /// True while one of the form fields has keyboard focus (the shell must not steal it).
    pub fn input_focused(&self, window: &Window, cx: &App) -> bool {
        self.inputs()
            .iter()
            .any(|i| i.read(cx).focus_handle(cx).is_focused(window))
    }

    fn inputs(&self) -> [&Entity<ComposerInput>; 8] {
        [
            &self.agent_picker.search,
            &self.name_input,
            &self.url_input,
            &self.user_input,
            &self.secret_input,
            &self.note_input,
            &self.hosts_input,
            &self.token_input,
        ]
    }

    fn reset_form(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        self.agent_picker.reset(cx);
        self.giving = false;
        self.replacing = false;
        self.confirm = None;
        for input in self.inputs() {
            input.update(cx, |i, cx| i.clear(cx));
        }
    }

    /// The agents the open page's picker must not offer again: the ones with access on
    /// a secret's or a vault's page (the forms that create something exclude nothing).
    fn agent_picker_exclude(&self, cx: &App) -> Vec<Id> {
        let state = self.state.read(cx);
        match &self.page {
            Page::Secret(id) => state
                .secrets
                .secret(id)
                .map(|s| {
                    s.agents
                        .iter()
                        .filter(|a| a.enabled)
                        .map(|a| a.project_id.clone())
                        .collect()
                })
                .unwrap_or_default(),
            Page::Vault(id) => state
                .secrets
                .vaults
                .iter()
                .find(|v| v.id == *id)
                .map(|v| {
                    v.agents
                        .iter()
                        .filter(|a| a.enabled)
                        .map(|a| a.project_id.clone())
                        .collect()
                })
                .unwrap_or_default(),
            Page::List | Page::Add | Page::Agent(_) | Page::Connect => Vec::new(),
        }
    }

    /// ↩ in the picker's search field: the highlighted candidate becomes a chip.
    pub(crate) fn agent_picker_accept(&mut self, cx: &mut Context<Self>) {
        if !self.agent_picker.open {
            return;
        }
        let exclude = self.agent_picker_exclude(cx);
        let query = self.agent_picker.query(cx);
        let pick = {
            let state = self.state.read(cx);
            agent_picker::candidates(&state.projects, &self.agent_picker.picked, &exclude, &query)
                .get(self.agent_picker.highlight)
                .map(|p| p.id.clone())
        };
        if let Some(id) = pick {
            self.agent_picker.add(id, cx);
        }
        cx.notify();
    }

    /// ↑ / ↓ in the picker's search field: move the highlight, clamped to the list.
    pub(crate) fn agent_picker_move(&mut self, delta: i32, cx: &mut Context<Self>) {
        if !self.agent_picker.open {
            return;
        }
        let exclude = self.agent_picker_exclude(cx);
        let query = self.agent_picker.query(cx);
        let n = {
            let state = self.state.read(cx);
            agent_picker::candidates(&state.projects, &self.agent_picker.picked, &exclude, &query)
                .len()
        };
        if n == 0 {
            self.agent_picker.highlight = 0;
        } else {
            let cur = self.agent_picker.highlight.min(n - 1) as i32;
            self.agent_picker.highlight = (cur + delta).clamp(0, n as i32 - 1) as usize;
        }
        cx.notify();
    }

    /// Rig hooks for the agent picker shots, applied once the registry is known and a
    /// form that carries the picker is open.
    fn apply_picker_rig(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let on_form = matches!(self.page, Page::Add | Page::Connect)
            || (matches!(self.page, Page::Secret(_) | Page::Vault(_)) && self.giving);
        if !on_form || self.state.read(cx).projects.is_empty() {
            return;
        }
        if let Some(names) = self.rig_picked.take() {
            let ids: Vec<Id> = {
                let state = self.state.read(cx);
                names
                    .split(',')
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .filter_map(|n| {
                        state
                            .projects
                            .iter()
                            .find(|p| p.name.eq_ignore_ascii_case(n))
                            .map(|p| p.id.clone())
                    })
                    .collect()
            };
            for id in ids {
                if !self.agent_picker.picked.contains(&id) {
                    self.agent_picker.picked.push(id);
                }
            }
        }
        if let Some(q) = self.rig_query.take() {
            self.agent_picker.open(window, cx);
            if !q.is_empty() {
                self.agent_picker
                    .search
                    .update(cx, |i, cx| i.set_text(q, cx));
            }
        }
    }

    /// Esc / a click elsewhere: fold the "Add agent" dropdown. `true` when one was open.
    pub fn close_agent_picker(&mut self, cx: &mut Context<Self>) -> bool {
        let was = self.agent_picker.close(cx);
        if was {
            cx.notify();
        }
        was
    }

    fn go(&mut self, page: Page, cx: &mut Context<Self>) {
        self.reset_form(cx);
        if let Page::Agent(pid) = &page {
            let pid = pid.clone();
            self.state
                .update(cx, |s, cx| s.project_secrets_refresh(pid, cx));
        }
        self.page = page;
        cx.notify();
    }

    fn hosts_from(&self, cx: &App) -> Vec<String> {
        self.hosts_input
            .read(cx)
            .text()
            .split([',', ' ', '\n'])
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .map(str::to_string)
            .collect()
    }

    fn submit_add(&mut self, cx: &mut Context<Self>) {
        if self.page != Page::Add {
            return;
        }
        let kind = self.add_kind;
        let name = self.name_input.read(cx).text().trim().to_string();
        let secret = self.secret_input.read(cx).text().to_string();
        if name.is_empty()
            || name.len() > 64
            || secret_env_name(&name).is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' '))
        {
            self.error = Some("Letters, numbers and _ (1–64 characters).".into());
            cx.notify();
            return;
        }
        if secret.is_empty() {
            self.error = Some(if kind == AddKind::Login {
                "Enter the password.".into()
            } else {
                "Enter the value.".into()
            });
            cx.notify();
            return;
        }
        let mut fields = Vec::new();
        match kind {
            AddKind::Login => {
                fields.push((
                    "url".to_string(),
                    self.url_input.read(cx).text().trim().to_string(),
                ));
                fields.push((
                    "username".to_string(),
                    self.user_input.read(cx).text().trim().to_string(),
                ));
                fields.push(("password".to_string(), secret));
            }
            AddKind::Key | AddKind::Text => fields.push(("value".to_string(), secret)),
        }
        let item = NewSecret {
            name,
            kind: kind.as_str().into(),
            fields,
            note: self.note_input.read(cx).text().trim().to_string(),
            hosts: self.hosts_from(cx),
            agents: self.agent_picker.picked.clone(),
            inject_env: self.add_inject,
        };
        self.add_inject = false;
        self.state.update(cx, |s, cx| s.secret_add(item, cx));
        self.go(Page::List, cx);
    }

    fn submit_replace(&mut self, id: Id, kind: &str, cx: &mut Context<Self>) {
        let value = self.secret_input.read(cx).text().to_string();
        if value.is_empty() {
            self.error = Some("Enter the new value.".into());
            cx.notify();
            return;
        }
        let body = if kind == "login" {
            serde_json::json!({ "fields": { "url": self.url_input.read(cx).text().trim(), "username": self.user_input.read(cx).text().trim(), "password": value } })
        } else {
            serde_json::json!({ "value": value })
        };
        self.state
            .update(cx, |s, cx| s.secret_replace_value(id, body, cx));
        self.replacing = false;
        self.error = None;
        self.secret_input.update(cx, |i, cx| i.clear(cx));
        cx.notify();
    }

    fn submit_connect(&mut self, cx: &mut Context<Self>) {
        let token = self.token_input.read(cx).text().trim().to_string();
        if token.len() < 20 {
            self.error = Some("Paste the whole service-account token.".into());
            cx.notify();
            return;
        }
        let note = self.note_input.read(cx).text().trim().to_string();
        let agents = self.agent_picker.picked.clone();
        self.state
            .update(cx, |s, cx| s.vault_connect(token, agents, note, cx));
        self.go(Page::List, cx);
    }

    // ---- building blocks -----------------------------------------------------------------------

    fn group(theme: &Theme) -> Div {
        glass::group(theme).bg(theme.pane_card)
    }

    fn section(label: &str, group: Div, note: Option<String>, theme: &Theme) -> Div {
        div()
            .flex()
            .flex_col()
            .child(glass::section_label(label, theme))
            .child(group)
            .when_some(note, |el, n| el.child(Self::footnote(n, theme)))
    }

    fn footnote(text: impl Into<SharedString>, theme: &Theme) -> Div {
        div()
            .px(px(16.0))
            .pt(px(6.0))
            .text_size(px(TEXT_TINY))
            .text_color(theme.text_muted)
            .child(text.into())
    }

    /// Tile · title over subtitle · trailing.
    fn row(
        id: impl Into<gpui::ElementId>,
        glyph: &str,
        title: impl Into<SharedString>,
        subtitle: Option<String>,
        dim: bool,
        theme: &Theme,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .relative()
            .flex()
            .items_center()
            .gap(px(10.0))
            .min_h(px(52.0))
            .px(px(16.0))
            .py(px(8.0))
            // Lights for rows that join `kit::HOT` (vault / secret rows, the
            // switch rows); a plain detail row never does.
            .child(highlight::layer(false, Shape::Nested(PANEL_RADIUS), theme))
            .child(kit::hairline_below(theme))
            .child(glass::icon_tile(glyph, theme).when(dim, |el| el.opacity(0.55)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        // Two lines before the ellipsis (the iPhone's SecretRowLabel):
                        // "1Password · Arbos — Research" no longer ends in "Resea…"
                        // at the right pane's width.
                        div()
                            .text_size(px(TEXT_BODY))
                            .text_color(if dim { theme.text_muted } else { theme.text })
                            .overflow_hidden()
                            .line_clamp(2)
                            .child(title.into()),
                    )
                    .when_some(subtitle.filter(|s| !s.is_empty()), |el, s| {
                        el.child(
                            div()
                                .text_size(px(TEXT_TINY))
                                .text_color(theme.text_muted)
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(s),
                        )
                    }),
            )
    }

    fn add_row(id: &'static str, label: &'static str, theme: &Theme) -> Stateful<Div> {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(10.0))
            .min_h(px(44.0))
            .px(px(16.0))
            .text_size(px(TEXT_BODY))
            .text_color(theme.text)
            .cursor_pointer()
            .map(|el| highlight::row(el, false, Shape::Nested(PANEL_RADIUS), theme))
            .child(
                div()
                    .size(px(30.0))
                    .flex_shrink_0()
                    .rounded(px(7.5))
                    .bg(theme.field)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(sf_symbols::glyph("plus", 15.0, theme.text)),
            )
            .child(label)
    }

    fn value_row(label: &str, value: impl Into<SharedString>, theme: &Theme) -> Div {
        glass::row(label.to_string(), theme).child(
            div()
                .flex_shrink_0()
                .max_w(px(220.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(px(TEXT_BODY))
                .text_color(theme.text_muted)
                .child(value.into()),
        )
    }

    fn action_row(
        id: impl Into<gpui::ElementId>,
        label: impl Into<SharedString>,
        danger: bool,
        theme: &Theme,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .flex()
            .items_center()
            .min_h(px(44.0))
            .px(px(16.0))
            .text_size(px(TEXT_BODY))
            .text_color(if danger { theme.danger } else { theme.text })
            .cursor_pointer()
            .map(|el| highlight::row(el, false, Shape::Nested(PANEL_RADIUS), theme))
            .child(label.into())
    }

    fn field(label: &str, input: Entity<ComposerInput>, hint: Option<&str>, theme: &Theme) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(5.0))
            .child(
                div()
                    .text_size(px(TEXT_TINY))
                    .text_color(theme.text_muted)
                    .child(label.to_string()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .min_h(px(34.0))
                    .px(px(12.0))
                    .py(px(4.0))
                    .rounded(px(10.0))
                    .bg(theme.field)
                    .border_1()
                    .border_color(theme.field_border)
                    .child(input),
            )
            .when_some(hint, |el, h| {
                el.child(
                    div()
                        .text_size(px(TEXT_TINY))
                        .text_color(theme.text_muted)
                        .child(h.to_string()),
                )
            })
    }

    fn small_pill(
        id: impl Into<gpui::ElementId>,
        label: impl Into<SharedString>,
        danger: bool,
        theme: &Theme,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .h(px(26.0))
            .px(px(10.0))
            .rounded_full()
            .flex()
            .items_center()
            .text_size(px(TEXT_TINY))
            .font_weight(FontWeight::MEDIUM)
            .text_color(if danger { theme.danger } else { theme.text })
            .cursor_pointer()
            .map(|el| highlight::washed(el, theme.field, theme))
            .child(label.into())
    }

    fn switch_row(
        id: impl Into<gpui::ElementId>,
        row: Stateful<Div>,
        on: bool,
        theme: &Theme,
    ) -> Stateful<Div> {
        let _ = id;
        row.cursor_pointer()
            .group(kit::HOT)
            .child(glass::switch(on, theme))
    }

    fn activity_rows(items: &[SecretActivity], theme: &Theme) -> Vec<Div> {
        items
            .iter()
            .map(|a| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .px(px(16.0))
                    .py(px(8.0))
                    .relative()
                    .child(kit::hairline_below(theme))
                    .child(
                        div()
                            .text_size(px(TEXT_SMALL))
                            .text_color(if a.action == "refused" {
                                theme.danger
                            } else {
                                theme.text
                            })
                            .child(a.line()),
                    )
                    .child(
                        div()
                            .text_size(px(TEXT_TINY))
                            .text_color(theme.text_muted)
                            .child(a.when_line()),
                    )
            })
            .collect()
    }

    fn request_row(&self, r: &SecretRequest, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let agent = r
            .project_name
            .clone()
            .unwrap_or_else(|| "this agent".to_string());
        let for_whom = r.for_whom();
        let title = match (&r.host, &for_whom) {
            (Some(h), Some(w)) => format!("{} for {w} at {h}", r.name),
            (Some(h), None) => format!("{} at {h}", r.name),
            (None, Some(w)) => format!("{} for {w}", r.name),
            (None, None) => r.name.clone(),
        };
        let mut sub: Vec<String> = vec![agent.clone()];
        if let Some(n) = r.asked_by_name.as_ref().filter(|n| !n.is_empty()) {
            sub.push(format!("{n} asked"));
        }
        if !r.purpose.is_empty() {
            sub.push(r.purpose.clone());
        }
        sub.push(ago(r.created_at));
        let always = match &for_whom {
            Some(w) => format!("Always allow {w}"),
            None => format!("Always allow {agent}"),
        };
        let mut answers = div().flex().flex_wrap().gap(px(6.0)).pt(px(4.0));
        for (choice, label, danger) in [
            ("once", "Allow once".to_string(), false),
            ("chat", "Allow for this chat".to_string(), false),
            ("always", always, false),
            ("deny", "Deny".to_string(), true),
        ] {
            let rid = r.id.clone();
            answers = answers.child(
                Self::small_pill(format!("req-{}-{choice}", r.id), label, danger, theme).on_click(
                    cx.listener(move |this, _, _, cx| {
                        let rid = rid.clone();
                        this.state
                            .update(cx, |s, cx| s.secret_decide(rid, choice.to_string(), cx));
                    }),
                ),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(px(3.0))
            .px(px(16.0))
            .py(px(10.0))
            .relative()
            .child(kit::hairline_below(theme))
            .child(
                div()
                    .text_size(px(TEXT_BODY))
                    .text_color(theme.text)
                    .child(title),
            )
            .child(
                div()
                    .text_size(px(TEXT_TINY))
                    .text_color(theme.text_muted)
                    .child(sub.join(" · ")),
            )
            .child(answers)
    }

    // ---- pages -----------------------------------------------------------------------------------

    fn render_list(&self, view: &SecretsView, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let mut body = div().flex().flex_col().gap(px(18.0));

        // Kill switch.
        // The switch flips at once (optimistic; the relay's answer or the next
        // refresh puts it back on error) — no inline confirm (Jacob, Oct 21).
        let paused = view.paused;
        let pause_row = Self::row(
            "secrets-pause",
            "pause.fill",
            "Pause all secrets",
            None,
            false,
            theme,
        )
        .border_b_0();
        let pause_row = Self::switch_row("secrets-pause-switch", pause_row, paused, theme)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.confirm = None;
                this.state.update(cx, |s, cx| s.secrets_pause(!paused, cx));
                cx.notify();
            }));
        body = body.child(
            div()
                .flex()
                .flex_col()
                .child(Self::group(theme).child(pause_row))
                .child(Self::footnote(
                    if paused {
                        "Secrets are paused. Agents can't use any secret until you turn this off."
                    } else {
                        "Agents can't use any secret while paused."
                    },
                    theme,
                )),
        );

        let pending: Vec<&SecretRequest> = view.pending().collect();
        if !pending.is_empty() {
            let mut g = Self::group(theme);
            for r in pending {
                g = g.child(self.request_row(r, theme, cx));
            }
            body = body.child(Self::section("Waiting for you", g, None, theme));
        }

        // The connect flow shows only when the relay's remote flag `features.vault_connect`
        // is on and it advertises a provider (default off since 2026-10-21; D83 Phase 2 hidden).
        let can_connect = view.vault_connect && !view.providers.is_empty();
        if !view.vaults.is_empty() || can_connect {
            let mut g = Self::group(theme);
            for v in &view.vaults {
                let vid = v.id.clone();
                let agents: Vec<&str> = v
                    .agents
                    .iter()
                    .filter(|a| a.enabled)
                    .map(|a| a.name.as_str())
                    .collect();
                let mut sub = vec![format!(
                    "{} item{}",
                    v.items_count,
                    if v.items_count == 1 { "" } else { "s" }
                )];
                if !agents.is_empty() {
                    sub.push(agents.join(", "));
                }
                if let Some(t) = v.token_expires_at {
                    sub.push(format!("token expires {}", crate::model::short_date(t)));
                }
                g = g.child(
                    Self::row(
                        format!("vault-{}", v.id),
                        "lock",
                        format!("{} · {}", v.provider_label(), v.name),
                        Some(sub.join(" · ")),
                        false,
                        theme,
                    )
                    .cursor_pointer()
                    .group(kit::HOT)
                    .child(sf_symbols::glyph("chevron.right", 13.0, theme.text_faint))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.go(Page::Vault(vid.clone()), cx)),
                    ),
                );
            }
            if can_connect {
                g = g.child(
                    Self::add_row("secrets-connect", "Connect a vault…", theme).on_click(
                        cx.listener(|this, _, window, cx| {
                            this.go(Page::Connect, cx);
                            let h = this.token_input.read(cx).focus_handle(cx);
                            window.focus(&h, cx);
                        }),
                    ),
                );
            }
            body = body.child(Self::section("Vaults", g, None, theme));
        }

        let mut g = Self::group(theme);
        for s in &view.secrets {
            let sid = s.id.clone();
            let agents: Vec<&str> = s
                .agents
                .iter()
                .filter(|a| a.enabled)
                .map(|a| a.name.as_str())
                .collect();
            let who = if agents.is_empty() {
                "no agents".to_string()
            } else {
                agents.join(", ")
            };
            g = g.child(
                Self::row(
                    format!("secret-{}", s.id),
                    s.glyph(),
                    s.env.clone(),
                    Some(match s.source_label() {
                        // Secret hand-off: "Arbos · given in chat · never used".
                        Some(given) => format!(
                            "{who} · {given} · {}",
                            secret_usage(s.last_used_at, None, true)
                        ),
                        None => format!("{who} · {}", secret_usage(s.last_used_at, None, true)),
                    }),
                    paused,
                    theme,
                )
                .cursor_pointer()
                .group(kit::HOT)
                .child(sf_symbols::glyph("chevron.right", 13.0, theme.text_faint))
                .on_click(
                    cx.listener(move |this, _, _, cx| this.go(Page::Secret(sid.clone()), cx)),
                ),
            );
        }
        g = g.child(
            Self::add_row("secrets-add", "Add a secret…", theme).on_click(cx.listener(
                |this, _, window, cx| {
                    this.go(Page::Add, cx);
                    let h = this.name_input.read(cx).focus_handle(cx);
                    window.focus(&h, cx);
                },
            )),
        );
        let note = if view.secrets.is_empty() && view.vaults.is_empty() {
            if view.loaded {
                "Add an API key or password here, pick which agents may use it, and they'll use it without ever seeing it."
            } else {
                "Loading from the relay…"
            }
        } else {
            "Agents use secrets without seeing them. Values are never shown again after you save them."
        };
        body = body.child(Self::section("Secrets", g, Some(note.to_string()), theme));

        if !view.activity.is_empty() {
            let mut g = Self::group(theme);
            for row in Self::activity_rows(&view.activity[..view.activity.len().min(3)], theme) {
                g = g.child(row);
            }
            body = body.child(Self::section("Recent activity", g, None, theme));
        }
        body
    }

    fn render_secret(&self, s: &Secret, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let mut body = div().flex().flex_col().gap(px(18.0));
        let images = avatars::project_images(self.state.read(cx));
        let (paused, projects, acts) = {
            let st = self.state.read(cx);
            let acts: Vec<SecretActivity> = st
                .secrets
                .activity
                .iter()
                .filter(|a| a.name == s.env)
                .take(5)
                .cloned()
                .collect();
            (st.secrets.paused, st.projects.clone(), acts)
        };
        if paused {
            body = body.child(Self::footnote(
                "Secrets are paused. Agents can't use any secret until you turn this off.",
                theme,
            ));
        }
        if !s.note.is_empty() {
            body = body.child(
                Self::group(theme).child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .px(px(16.0))
                        .py(px(10.0))
                        .child(
                            div()
                                .text_size(px(TEXT_TINY))
                                .text_color(theme.text_muted)
                                .child("Note"),
                        )
                        .child(
                            div()
                                .text_size(px(TEXT_BODY))
                                .text_color(theme.text)
                                .child(s.note.clone()),
                        ),
                ),
            );
        }

        // Facts.
        let mut g = Self::group(theme).child(Self::value_row("Kind", s.kind_label(), theme));
        if s.name != s.env {
            g = g.child(Self::value_row("Name", s.name.clone(), theme));
        }
        // Secret hand-off: where it came from ("In chat with Arbos").
        if let Some(added) = s.added_label() {
            g = g.child(Self::value_row("Added", added, theme));
        }
        if let Some(u) = s.username.as_deref().filter(|u| !u.is_empty()) {
            g = g.child(Self::value_row("Username", u.to_string(), theme));
        }
        g = g.child(Self::value_row(
            "Used at",
            if s.hosts.is_empty() {
                "Decided at first use".to_string()
            } else {
                s.hosts.join(", ")
            },
            theme,
        ));
        let sid = s.id.clone();
        let kind = s.kind.clone();
        if self.replacing {
            let mut form = div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .px(px(16.0))
                .py(px(10.0))
                .relative()
                .child(kit::hairline_below(theme));
            if s.kind == "login" {
                form = form
                    .child(Self::field(
                        "Sign-in URL",
                        self.url_input.clone(),
                        None,
                        theme,
                    ))
                    .child(Self::field(
                        "Username",
                        self.user_input.clone(),
                        None,
                        theme,
                    ))
                    .child(Self::field(
                        "Password",
                        self.secret_input.clone(),
                        Some("Sent once and encrypted. You won't see it again."),
                        theme,
                    ));
            } else {
                form = form.child(Self::field(
                    "Value",
                    self.secret_input.clone(),
                    Some("Sent once and encrypted. The old value keeps working for 24 hours."),
                    theme,
                ));
            }
            let sid2 = sid.clone();
            form = form.child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .child(
                        glass::primary_button("secret-replace-save", "Replace", theme)
                            .h(px(30.0))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.submit_replace(sid2.clone(), &kind, cx)
                            })),
                    )
                    .child(
                        glass::quiet_button("secret-replace-cancel", "Cancel", theme)
                            .h(px(30.0))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.replacing = false;
                                this.error = None;
                                cx.notify();
                            })),
                    ),
            );
            g = g.child(form);
        } else {
            g = g.child(
                glass::row("Value", theme)
                    .child(
                        div()
                            .text_size(px(TEXT_BODY))
                            .text_color(theme.text_muted)
                            .child("••••••"),
                    )
                    .child(
                        div()
                            .id("secret-replace")
                            .text_size(px(TEXT_BODY))
                            .text_color(theme.accent)
                            .cursor_pointer()
                            .hover(|s| s.opacity(0.8))
                            .child("Replace…")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.replacing = true;
                                this.error = None;
                                let h = this.secret_input.read(cx).focus_handle(cx);
                                window.focus(&h, cx);
                                cx.notify();
                            })),
                    ),
            );
        }
        g = g.child(Self::value_row(
            "Expires",
            s.expires_at
                .map(crate::model::short_date)
                .unwrap_or_else(|| "Never".to_string()),
            theme,
        ));
        let ask = s.ask_every_time;
        let sid_ask = sid.clone();
        g = g.child(
            glass::hot_row("secret-ask", "Ask every time", theme)
                .border_b_0()
                .child(glass::switch(ask, theme))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let id = sid_ask.clone();
                    this.state.update(cx, |st, cx| {
                        st.secret_patch(id, serde_json::json!({ "ask_every_time": !ask }), cx)
                    });
                })),
        );
        body = body.child(div().flex().flex_col().child(g).child(Self::footnote(
            "Ask every time shows you a card before each use, even for agents that have access.",
            theme,
        )));

        // Real value in the agent's environment (D83 decision 2 as decided).
        let inject = s.inject_env;
        let sid_inject = sid.clone();
        let row = Self::row(
            "secret-inject",
            "terminal",
            "Real value in the environment",
            Some(if inject {
                "On · agents with access see the value".to_string()
            } else {
                format!("Agents with access get {} itself", s.real_var_list())
            }),
            false,
            theme,
        )
        .border_b_0();
        let g = Self::group(theme).child(Self::switch_row("x", row, inject, theme).on_click(
            cx.listener(move |this, _, _, cx| {
                let id = sid_inject.clone();
                this.state.update(cx, |st, cx| {
                    st.secret_patch(id, serde_json::json!({ "inject_env": !inject }), cx)
                });
            }),
        ));
        let note = if inject {
            format!(
                "On: {} is a normal variable in the box of every agent with access. Any program the agent runs can use the key directly — including to spend money or change things on your accounts — without asking you. No card, no host check. Turning access or Pause all off removes it within seconds. Arbos still redacts the value from chat and logs as best it can.",
                s.real_var_list()
            )
        } else {
            format!(
                "Off: agents get a placeholder and the relay fills the value in only through egress, for the allowed hosts. Turn this on only for keys you would hand to a CLI or SDK that must read ${} itself — once on, programs can act with it, including spending, without asking you.",
                s.env
            )
        };
        body = body.child(
            div()
                .flex()
                .flex_col()
                .child(g)
                .child(Self::footnote(note, theme)),
        );

        // Agents.
        let mut g = Self::group(theme);
        for a in &s.agents {
            let mut sub: Vec<String> = Vec::new();
            if !a.note.is_empty() {
                sub.push(a.note.clone());
            }
            if a.scope.kind == "chat" {
                sub.push("this chat only".into());
            }
            sub.push(secret_usage(a.last_used_at, a.disabled_at, a.enabled));
            let avatar = match projects.iter().find(|p| p.id == a.project_id) {
                Some(p) => avatars::project_with(images.get(&p.id).cloned(), p, 30.0, theme),
                None => avatars::agent_picture(
                    &arbos_core::avatar::identity::resolve(
                        &avatars::AgentQuery {
                            sender: &a.project_id,
                            handle: None,
                            name: Some(&a.name),
                        },
                        &[],
                    ),
                    None,
                    30.0,
                    theme,
                ),
            };
            let (sid_a, pid_a, on) = (sid.clone(), a.project_id.clone(), a.enabled);
            g = g.child(
                div()
                    .id(format!("access-{}", a.access_id))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .min_h(px(52.0))
                    .px(px(16.0))
                    .py(px(8.0))
                    .relative()
                    .child(kit::hairline_below(theme))
                    .cursor_pointer()
                    .map(|el| highlight::row(el, false, Shape::Nested(PANEL_RADIUS), theme))
                    .child(avatar)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(px(TEXT_BODY))
                                    .text_color(theme.text)
                                    .child(a.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(TEXT_TINY))
                                    .text_color(theme.text_muted)
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(sub.join(" · ")),
                            ),
                    )
                    .child(glass::switch(on, theme))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let (id, pid) = (sid_a.clone(), pid_a.clone());
                        this.state
                            .update(cx, |st, cx| st.secret_access(id, pid, !on, None, cx));
                    })),
            );
        }
        if s.agents.is_empty() && !self.giving {
            g = g.child(Self::footnote("No agent has access yet.", theme).pb(px(8.0)));
        }
        if self.giving {
            let have: Vec<Id> = s
                .agents
                .iter()
                .filter(|a| a.enabled)
                .map(|a| a.project_id.clone())
                .collect();
            let sid_g = sid.clone();
            g = g.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .px(px(16.0))
                    .py(px(10.0))
                    .child(agent_picker::render(
                        self,
                        &self.state.clone(),
                        "give",
                        &have,
                        theme,
                        cx,
                    ))
                    .child(Self::field(
                        "Note for the agent",
                        self.note_input.clone(),
                        Some(ACCESS_NOTE_HINT),
                        theme,
                    ))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(
                                glass::primary_button("give-save", "Save", theme)
                                    .h(px(30.0))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let note =
                                            this.note_input.read(cx).text().trim().to_string();
                                        let picked = this.agent_picker.picked.clone();
                                        for pid in picked {
                                            let id = sid_g.clone();
                                            let n = (!note.is_empty()).then(|| note.clone());
                                            this.state.update(cx, |st, cx| {
                                                st.secret_access(id, pid, true, n, cx)
                                            });
                                        }
                                        this.giving = false;
                                        this.agent_picker.reset(cx);
                                        this.note_input.update(cx, |i, cx| i.clear(cx));
                                        cx.notify();
                                    })),
                            )
                            .child(
                                glass::quiet_button("give-cancel", "Cancel", theme)
                                    .h(px(30.0))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.giving = false;
                                        this.agent_picker.reset(cx);
                                        cx.notify();
                                    })),
                            ),
                    ),
            );
        } else {
            g = g.child(
                Self::add_row("secret-give", "Give another agent access…", theme).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.giving = true;
                        this.agent_picker.reset(cx);
                        cx.notify();
                    }),
                ),
            );
        }
        body = body.child(Self::section("Agents", g, None, theme));

        // People (spec §7.7): who may ask an agent with access to use it — a mirror of Agents.
        let mut g = Self::group(theme);
        for d in &s.directors {
            let mut sub: Vec<String> = Vec::new();
            if !d.handle.is_empty() {
                sub.push(format!("@{}", d.handle));
            }
            if !d.note.is_empty() {
                sub.push(d.note.clone());
            }
            if d.scope.kind == "chat" {
                sub.push("this chat only".into());
            }
            sub.push(person_usage(d.last_asked_at, d.disabled_at, d.enabled));
            // Shift-click / right-click arms removal; the next click takes them off the list.
            let armed = self.confirm.as_deref() == Some(&format!("person:{}", d.user_id));
            if armed {
                sub = vec!["Click again to remove from the list".into()];
            }
            let (sid_p, uid_p, on) = (sid.clone(), d.user_id.clone(), d.enabled);
            let uid_r = d.user_id.clone();
            g = g.child(
                div()
                    .id(format!("person-{}", d.user_id))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .min_h(px(52.0))
                    .px(px(16.0))
                    .py(px(8.0))
                    .relative()
                    .child(kit::hairline_below(theme))
                    .cursor_pointer()
                    .map(|el| highlight::row(el, false, Shape::Nested(PANEL_RADIUS), theme))
                    .child(avatars::element(
                        &d.user_id,
                        &d.display_name(),
                        None,
                        30.0,
                        theme,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(px(TEXT_BODY))
                                    .text_color(theme.text)
                                    .child(d.display_name()),
                            )
                            .child(
                                div()
                                    .text_size(px(TEXT_TINY))
                                    .text_color(theme.text_muted)
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(sub.join(" · ")),
                            ),
                    )
                    .child(glass::switch(on, theme))
                    .on_mouse_down(gpui::MouseButton::Right, {
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.confirm = Some(format!("person:{uid_r}"));
                            cx.notify();
                        })
                    })
                    .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                        let (id, uid) = (sid_p.clone(), uid_p.clone());
                        let key = format!("person:{uid}");
                        if this.confirm.as_deref() == Some(&key) {
                            this.confirm = None;
                            this.state
                                .update(cx, |st, cx| st.secret_person_remove(id, uid, cx));
                            return;
                        }
                        if event.modifiers().shift || event.modifiers().control {
                            this.confirm = Some(key);
                            cx.notify();
                            return;
                        }
                        this.state
                            .update(cx, |st, cx| st.secret_person(id, uid, !on, None, cx));
                    })),
            );
        }
        if s.directors.is_empty() {
            g = g.child(Self::footnote("Only you right now.", theme).pb(px(8.0)));
        }
        let sid_people = sid.clone();
        g = g.child(
            Self::add_row("secret-give-person", "Give a person access…", theme).on_click(
                cx.listener(move |this, _, _, cx| {
                    let id = sid_people.clone();
                    this.state
                        .update(cx, |st, cx| st.open_secret_people_picker(id, cx));
                }),
            ),
        );
        body = body.child(Self::section(
            "People",
            g,
            Some(format!(
                "People here can ask any agent that has access to use {}, with no card to you. They never see the value. What they spend is yours.",
                s.env
            )),
            theme,
        ));
        // Others who ask: Refuse (default) | Ask me.
        let card_mode = s.others_ask == "card";
        let mut seg = div()
            .flex()
            .items_center()
            .gap(px(2.0))
            .p(px(2.0))
            .rounded_full()
            .bg(theme.field);
        for (value, label, on) in [
            ("refuse", "Refuse", !card_mode),
            ("card", "Ask me", card_mode),
        ] {
            let sid_o = sid.clone();
            seg = seg.child(
                glass::segment(format!("others-ask-{value}"), label, on, theme)
                    .h(px(24.0))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let id = sid_o.clone();
                        this.state.update(cx, |st, cx| {
                            st.secret_patch(id, serde_json::json!({ "others_ask": value }), cx)
                        });
                    })),
            );
        }
        let others = Self::group(theme).child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .min_h(px(52.0))
                .px(px(16.0))
                .py(px(8.0))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(TEXT_BODY))
                        .text_color(theme.text)
                        .child("Others who ask"),
                )
                .child(seg),
        );
        body = body.child(div().flex().flex_col().child(others).child(Self::footnote(
            if card_mode {
                "Ask me: anyone else who asks gets you a card first."
            } else {
                "Refuse: anyone else who asks is told no."
            },
            theme,
        )));

        // Activity for this secret (from the overview; the relay keeps 30 days).
        let mut g = Self::group(theme);
        if acts.is_empty() {
            g = g.child(Self::footnote("No activity in the last 30 days.", theme).pb(px(8.0)));
        }
        for row in Self::activity_rows(&acts, theme) {
            g = g.child(row);
        }
        body = body.child(Self::section("Activity", g, None, theme));

        // Remove from all agents · Delete.
        let any_on = s.agents.iter().any(|a| a.enabled);
        let sid_r = sid.clone();
        let revoke_armed = self.confirm.as_deref() == Some("revoke");
        body = body.child(
            Self::group(theme).child(
                Self::action_row(
                    "secret-revoke",
                    if revoke_armed {
                        "Remove from all agents? Click again to confirm."
                    } else {
                        "Remove from all agents"
                    },
                    false,
                    theme,
                )
                .when(!any_on, |el| el.opacity(0.5))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !any_on {
                        return;
                    }
                    if this.confirm.as_deref() == Some("revoke") {
                        this.confirm = None;
                        let id = sid_r.clone();
                        this.state.update(cx, |st, cx| st.secret_revoke_all(id, cx));
                    } else {
                        this.confirm = Some("revoke".into());
                    }
                    cx.notify();
                })),
            ),
        );
        let delete_armed = self.confirm.as_deref() == Some("delete");
        let sid_d = sid.clone();
        let env = s.env.clone();
        body = body.child(
            Self::group(theme).child(
                Self::action_row(
                    "secret-delete",
                    if delete_armed {
                        format!("Delete {env}? Agents lose it immediately. Activity is kept. Click again.")
                    } else {
                        "Delete secret".to_string()
                    },
                    true,
                    theme,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.confirm.as_deref() == Some("delete") {
                        this.confirm = None;
                        let id = sid_d.clone();
                        this.state.update(cx, |st, cx| st.secret_delete(id, cx));
                        this.go(Page::List, cx);
                    } else {
                        this.confirm = Some("delete".into());
                        cx.notify();
                    }
                })),
            ),
        );
        body
    }

    /// The claimed relay failure first, else the form's validation (a refusal with the words).
    fn note(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<Div> {
        let claimed = self.state.read(cx).claimed_failure.clone();
        let (slot, claimed_one) = match (claimed, &self.error) {
            (Some(slot), _) => (slot, true),
            (None, Some(words)) => (crate::app::refused(self.verb(), words), false),
            (None, None) => return None,
        };
        let mut handlers = error_note::Handlers::new();
        if claimed_one {
            handlers = handlers
                .toggle_details(cx.listener(|this, _, _, cx| {
                    this.state.update(cx, |s, cx| {
                        s.toggle_claimed_failure_details();
                        cx.notify();
                    });
                }))
                .try_again(cx.listener(|this, _, _, cx| this.retry_last(cx)));
        } else {
            handlers = handlers.toggle_details(cx.listener(|this, _, _, cx| {
                this.local_details_open = !this.local_details_open;
                cx.notify();
            }));
        }
        let open = if claimed_one {
            slot.details_open
        } else {
            self.local_details_open
        };
        Some(div().child(error_note::render(
            "secrets-error",
            &slot.error,
            open,
            error_note::Style::Inline,
            theme,
            handlers,
        )))
    }

    /// What the open form does, for the validation note's sentence.
    fn verb(&self) -> &'static str {
        match self.page {
            Page::Add => "save the secret",
            Page::Connect => "connect 1Password",
            Page::Vault(_) => "replace the token",
            Page::Secret(_) => "save the secret",
            Page::Agent(_) | Page::List => "save the change",
        }
    }

    /// Try again on a claimed failure: the form's own action, once more.
    fn retry_last(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.clear_claimed_failure();
            cx.notify();
        });
        match self.page {
            Page::Add => self.submit_add(cx),
            Page::Connect => self.submit_connect(cx),
            Page::Vault(_) | Page::Secret(_) | Page::Agent(_) | Page::List => {}
        }
    }

    fn render_add(&self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let kind = self.add_kind;
        let segs = div()
            .flex()
            .gap(px(2.0))
            .p(px(3.0))
            .rounded_full()
            .bg(theme.field)
            .children(
                [AddKind::Key, AddKind::Login, AddKind::Text]
                    .into_iter()
                    .map(|k| {
                        let label = match k {
                            AddKind::Key => "Key",
                            AddKind::Login => "Login",
                            AddKind::Text => "Text",
                        };
                        glass::segment(format!("add-kind-{label}"), label, k == kind, theme)
                            .flex_1()
                            .justify_center()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.add_kind = k;
                                this.error = None;
                                // The value field's example follows the kind (the iPhone's).
                                this.secret_input.update(cx, |i, _| {
                                    i.set_placeholder(match k {
                                        AddKind::Key => "sk_live_…",
                                        AddKind::Login => "Password",
                                        AddKind::Text => "Paste the text",
                                    })
                                });
                                cx.notify();
                            }))
                    }),
            );
        let env = secret_env_name(self.name_input.read(cx).text());
        let name_hint = format!(
            "Letters, numbers and _. Agents see it as ${}.",
            if env.is_empty() {
                "NAME".to_string()
            } else {
                env
            }
        );
        let mut form = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(segs)
            .child(Self::field(
                "Name",
                self.name_input.clone(),
                Some(&name_hint),
                theme,
            ));
        match kind {
            AddKind::Login => {
                form = form
                    .child(Self::field(
                        "Sign-in URL",
                        self.url_input.clone(),
                        None,
                        theme,
                    ))
                    .child(Self::field(
                        "Username",
                        self.user_input.clone(),
                        None,
                        theme,
                    ))
                    .child(Self::field(
                        "Password",
                        self.secret_input.clone(),
                        Some("Sent once and encrypted. You won't see it again."),
                        theme,
                    ));
            }
            AddKind::Key | AddKind::Text => {
                form = form.child(Self::field(
                    "Value",
                    self.secret_input.clone(),
                    Some(if kind == AddKind::Text {
                        "Paste a certificate, JSON key, or any text. Sent once and encrypted."
                    } else {
                        "Sent once and encrypted. You won't see it again."
                    }),
                    theme,
                ));
            }
        }
        form = form
            .child(Self::field(
                "Note",
                self.note_input.clone(),
                Some(NOTE_HINT),
                theme,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(5.0))
                    .child(
                        div()
                            .text_size(px(TEXT_TINY))
                            .text_color(theme.text_muted)
                            .child("Agents"),
                    )
                    .child(agent_picker::render(
                        self,
                        &self.state.clone(),
                        "add",
                        &[],
                        theme,
                        cx,
                    )),
            )
            .child(Self::field(
                "Where it may be used (optional)",
                self.hosts_input.clone(),
                Some("Leave empty to decide the first time an agent uses it."),
                theme,
            ));
        let inject = self.add_inject;
        let env_preview = {
            let n = secret_env_name(self.name_input.read(cx).text().trim());
            if n.is_empty() {
                "NAME".to_string()
            } else {
                n
            }
        };
        form = form.child(
            div()
                .flex()
                .flex_col()
                .gap(px(5.0))
                .child(
                    div()
                        .text_size(px(TEXT_TINY))
                        .text_color(theme.text_muted)
                        .child("In the agent's environment"),
                )
                .child(
                    div()
                        .id("add-inject")
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(10.0))
                        .cursor_pointer()
                        .child(
                            div()
                                .text_size(px(TEXT_BODY))
                                .text_color(theme.text)
                                .child("Real value in the agent's environment"),
                        )
                        .child(glass::switch(inject, theme))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.add_inject = !inject;
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .text_size(px(TEXT_TINY))
                        .text_color(theme.text_muted)
                        .child(if inject {
                            format!("On: agents with access get ${env_preview} itself. Any program they run can use it directly — including to spend money — without asking you. Only for keys you would hand to a CLI.")
                        } else {
                            "Off: agents get a placeholder; the relay fills the value in only through egress, for the allowed hosts. Turn on only for keys a CLI or SDK must read itself.".to_string()
                        }),
                ),
        );
        form.child(
            div()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(8.0))
                .pt(px(4.0))
                .child(
                    glass::quiet_button("add-cancel", "Cancel", theme)
                        .h(px(32.0))
                        .on_click(cx.listener(|this, _, _, cx| this.go(Page::List, cx))),
                )
                .child(
                    glass::primary_button("add-save", "Save", theme)
                        .h(px(32.0))
                        .on_click(cx.listener(|this, _, _, cx| this.submit_add(cx))),
                ),
        )
    }

    fn render_agent(&self, project_id: &str, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let state = self.state.read(cx);
        let name = state
            .project(project_id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Agent".into());
        let data: ProjectSecrets = state
            .project_secrets
            .get(project_id)
            .cloned()
            .unwrap_or_default();
        let mut body = div().flex().flex_col().gap(px(18.0));
        if data.paused {
            body = body.child(Self::footnote(
                "Secrets are paused. Agents can't use any secret until you turn this off.",
                theme,
            ));
        }
        let mut g = Self::group(theme);
        for item in &data.items {
            let (glyph, title, sub) = if let Some(v) = &item.vault {
                let mut sub = format!(
                    "everything in this vault · {} item{}",
                    v.items_count,
                    if v.items_count == 1 { "" } else { "s" }
                );
                if v.env_items > 0 {
                    sub.push_str(&format!(
                        " · {} as real value{}",
                        v.env_items,
                        if v.env_items == 1 { "" } else { "s" }
                    ));
                }
                ("lock", v.name.clone(), sub)
            } else {
                let mut sub: Vec<String> = Vec::new();
                if item.secret.as_ref().is_some_and(|s| s.inject_env) {
                    sub.push("Real value in its environment".into());
                }
                if !item.note.is_empty() {
                    sub.push(item.note.clone());
                }
                sub.push(secret_usage(
                    item.last_used_at,
                    item.disabled_at,
                    item.enabled,
                ));
                (
                    item.secret.as_ref().map(|s| s.glyph()).unwrap_or("key"),
                    item.title(),
                    sub.join(" · "),
                )
            };
            let on = item.enabled;
            let pid = project_id.to_string();
            let target = item
                .secret
                .as_ref()
                .map(|s| ("secret", s.id.clone()))
                .or_else(|| item.vault.as_ref().map(|v| ("vault", v.id.clone())));
            let row = Self::row(
                format!("agent-access-{}", item.access_id),
                glyph,
                title,
                Some(sub),
                data.paused,
                theme,
            );
            g = g.child(Self::switch_row("x", row, on, theme).on_click(cx.listener(
                move |this, _, _, cx| {
                    let pid = pid.clone();
                    match target.clone() {
                        Some(("secret", id)) => this
                            .state
                            .update(cx, |st, cx| st.secret_access(id, pid, !on, None, cx)),
                        Some((_, id)) => this
                            .state
                            .update(cx, |st, cx| st.vault_access(id, pid, !on, cx)),
                        None => {}
                    }
                },
            )));
        }
        if self.giving {
            let have: Vec<Id> = data
                .items
                .iter()
                .filter(|i| i.enabled)
                .filter_map(|i| i.secret.as_ref().map(|s| s.id.clone()))
                .collect();
            let candidates: Vec<Secret> = state
                .secrets
                .secrets
                .iter()
                .filter(|s| !have.contains(&s.id))
                .cloned()
                .collect();
            let mut chips = div().flex().flex_wrap().gap(px(6.0));
            if candidates.is_empty() {
                chips = chips.child(
                    div()
                        .text_size(px(TEXT_SMALL))
                        .text_color(theme.text_muted)
                        .child("Every secret is already on. Add one in Settings → Secrets."),
                );
            }
            for s in candidates {
                let on = self.agent_picker.picked.contains(&s.id);
                let sid = s.id.clone();
                chips = chips.child(
                    div()
                        .id(format!("pick-secret-{}", s.id))
                        .h(px(30.0))
                        .px(px(11.0))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .bg(if on { theme.primary } else { theme.field })
                        .border_1()
                        .border_color(if on {
                            theme.primary
                        } else {
                            theme.field_border
                        })
                        .text_size(px(TEXT_SMALL))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if on { theme.on_primary } else { theme.text })
                        .cursor_pointer()
                        .child(s.env.clone())
                        .when(on, |el| {
                            el.child(sf_symbols::glyph("checkmark", 11.0, theme.on_primary))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let picked = &mut this.agent_picker.picked;
                            if let Some(i) = picked.iter().position(|x| *x == sid) {
                                picked.remove(i);
                            } else {
                                picked.push(sid.clone());
                            }
                            cx.notify();
                        })),
                );
            }
            let pid = project_id.to_string();
            g = g.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .px(px(16.0))
                    .py(px(10.0))
                    .child(chips)
                    .child(Self::field(
                        "Note for the agent",
                        self.note_input.clone(),
                        Some(ACCESS_NOTE_HINT),
                        theme,
                    ))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(
                                glass::primary_button("agent-give-save", "Save", theme)
                                    .h(px(30.0))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let note =
                                            this.note_input.read(cx).text().trim().to_string();
                                        for sid in this.agent_picker.picked.clone() {
                                            let pid = pid.clone();
                                            let n = (!note.is_empty()).then(|| note.clone());
                                            this.state.update(cx, |st, cx| {
                                                st.secret_access(sid, pid, true, n, cx)
                                            });
                                        }
                                        this.giving = false;
                                        this.agent_picker.reset(cx);
                                        this.note_input.update(cx, |i, cx| i.clear(cx));
                                        cx.notify();
                                    })),
                            )
                            .child(
                                glass::quiet_button("agent-give-cancel", "Cancel", theme)
                                    .h(px(30.0))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.giving = false;
                                        this.agent_picker.reset(cx);
                                        cx.notify();
                                    })),
                            ),
                    ),
            );
        } else if data.owner {
            g = g.child(
                Self::add_row("agent-give", "Give access to a secret…", theme).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.giving = true;
                        this.agent_picker.reset(cx);
                        cx.notify();
                    }),
                ),
            );
        }
        let note = if data.items.is_empty() {
            format!("{name} has no secrets yet.")
        } else {
            let real: Vec<&str> = data
                .items
                .iter()
                .filter_map(|i| i.secret.as_ref())
                .filter(|s| s.inject_env)
                .map(|s| s.env.as_str())
                .collect();
            let except = if real.is_empty() {
                String::new()
            } else {
                format!(
                    ", except {} — a real value in its environment",
                    real.join(", ")
                )
            };
            format!(
                "{name} uses these without seeing them{except}. Turn one off and it stops within seconds."
            )
        };
        body = body.child(Self::section("Secrets", g, Some(note), theme));

        let pending: Vec<&SecretRequest> =
            data.requests.iter().filter(|r| r.is_pending()).collect();
        if !pending.is_empty() {
            let mut g = Self::group(theme);
            for r in pending {
                g = g.child(self.request_row(r, theme, cx));
            }
            body = body.child(Self::section("Waiting for you", g, None, theme));
        }

        let mut g = Self::group(theme);
        if data.activity.is_empty() {
            g = g.child(Self::footnote("No activity in the last 30 days.", theme).pb(px(8.0)));
        }
        for row in Self::activity_rows(&data.activity[..data.activity.len().min(6)], theme) {
            g = g.child(row);
        }
        body.child(Self::section("Activity", g, None, theme))
    }

    fn render_connect(&self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let step = |n: &str, text: &str, theme: &Theme| {
            div()
                .flex()
                .items_start()
                .min_w_0()
                .gap(px(10.0))
                .child(
                    div()
                        .size(px(22.0))
                        .flex_shrink_0()
                        .rounded_full()
                        .bg(theme.primary)
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(TEXT_TINY))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.on_primary)
                        .child(n.to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(TEXT_BODY))
                        .line_height(px(TEXT_BODY + 6.0))
                        .text_color(theme.text)
                        .child(text.to_string()),
                )
        };
        let form = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(step(
                "1",
                "In 1Password, create a service account with read access to one vault, e.g. \"Arbos — Research\".",
                theme,
            ))
            .child(
                div()
                    .id("connect-open-1p")
                    .pl(px(32.0))
                    .text_size(px(TEXT_SMALL))
                    .text_color(theme.accent)
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.8))
                    .child("Open 1password.com ›")
                    .on_click(|_, _, cx| {
                        cx.open_url("https://my.1password.com/developer-tools/infrastructure-secrets/serviceaccount/");
                    }),
            )
            .child(step("2", "Paste its token", theme))
            .child(
                div().pl(px(32.0)).child(Self::field(
                    "Token",
                    self.token_input.clone(),
                    Some("Stored encrypted. Arbos never shows it again."),
                    theme,
                )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(5.0))
                    .child(div().text_size(px(TEXT_TINY)).text_color(theme.text_muted).child("Agents that may use this vault"))
                    .child(agent_picker::render(self, &self.state.clone(), "connect", &[], theme, cx)),
            )
            .child(Self::field("Note", self.note_input.clone(), Some(VAULT_NOTE_HINT), theme));
        form.child(
            div()
                .flex()
                .justify_end()
                .gap(px(8.0))
                .child(
                    glass::quiet_button("connect-cancel", "Cancel", theme)
                        .h(px(32.0))
                        .on_click(cx.listener(|this, _, _, cx| this.go(Page::List, cx))),
                )
                .child(
                    glass::primary_button("connect-save", "Connect", theme)
                        .h(px(32.0))
                        .on_click(cx.listener(|this, _, _, cx| this.submit_connect(cx))),
                ),
        )
    }

    fn render_vault(&self, v: &ConnectedVault, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let mut body = div().flex().flex_col().gap(px(18.0));
        let mut g = Self::group(theme)
            .child(Self::value_row("Items", v.items_count.to_string(), theme))
            .child(Self::value_row(
                "Token expires",
                v.token_expires_at
                    .map(crate::model::short_date)
                    .unwrap_or_else(|| "Never".to_string()),
                theme,
            ));
        if let (Some(used), Some(cap)) = (
            v.limits["reads_used_hour"].as_i64(),
            v.limits["reads_per_hour"].as_i64(),
        ) {
            g = g.child(Self::value_row(
                "Reads this hour",
                format!("{used} / {cap}"),
                theme,
            ));
        }
        if v.status != "ok" && !v.status.is_empty() {
            g = g.child(Self::value_row("Status", v.status.replace('_', " "), theme));
        }
        body = body.child(g);
        let mut g = Self::group(theme);
        let projects = self.state.read(cx).projects.clone();
        let images = avatars::project_images(self.state.read(cx));
        for a in &v.agents {
            let avatar = match projects.iter().find(|p| p.id == a.project_id) {
                Some(p) => avatars::project_with(images.get(&p.id).cloned(), p, 30.0, theme),
                None => avatars::agent_picture(
                    &arbos_core::avatar::identity::resolve(
                        &avatars::AgentQuery {
                            sender: &a.project_id,
                            handle: None,
                            name: Some(&a.name),
                        },
                        &[],
                    ),
                    None,
                    30.0,
                    theme,
                ),
            };
            let (vid, pid, on) = (v.id.clone(), a.project_id.clone(), a.enabled);
            g = g.child(
                div()
                    .id(format!("vault-access-{}", a.access_id))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .min_h(px(52.0))
                    .px(px(16.0))
                    .py(px(8.0))
                    .relative()
                    .child(kit::hairline_below(theme))
                    .cursor_pointer()
                    .map(|el| highlight::row(el, false, Shape::Nested(PANEL_RADIUS), theme))
                    .child(avatar)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(px(TEXT_BODY))
                                    .text_color(theme.text)
                                    .child(a.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(TEXT_TINY))
                                    .text_color(theme.text_muted)
                                    .child(if a.note.is_empty() {
                                        secret_usage(a.last_used_at, a.disabled_at, a.enabled)
                                    } else {
                                        a.note.clone()
                                    }),
                            ),
                    )
                    .child(glass::switch(on, theme))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let (id, pid) = (vid.clone(), pid.clone());
                        this.state
                            .update(cx, |st, cx| st.vault_access(id, pid, !on, cx));
                    })),
            );
        }
        if self.giving {
            let have: Vec<Id> = v
                .agents
                .iter()
                .filter(|a| a.enabled)
                .map(|a| a.project_id.clone())
                .collect();
            let vid = v.id.clone();
            g = g.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .px(px(16.0))
                    .py(px(10.0))
                    .child(agent_picker::render(
                        self,
                        &self.state.clone(),
                        "vault-give",
                        &have,
                        theme,
                        cx,
                    ))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(
                                glass::primary_button("vault-give-save", "Save", theme)
                                    .h(px(30.0))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        for pid in this.agent_picker.picked.clone() {
                                            let id = vid.clone();
                                            this.state.update(cx, |st, cx| {
                                                st.vault_access(id, pid, true, cx)
                                            });
                                        }
                                        this.giving = false;
                                        this.agent_picker.reset(cx);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                glass::quiet_button("vault-give-cancel", "Cancel", theme)
                                    .h(px(30.0))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.giving = false;
                                        this.agent_picker.reset(cx);
                                        cx.notify();
                                    })),
                            ),
                    ),
            );
        } else {
            g = g.child(
                Self::add_row("vault-give", "Give another agent access…", theme).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.giving = true;
                        this.agent_picker.reset(cx);
                        cx.notify();
                    }),
                ),
            );
        }
        body = body.child(Self::section(
            "Agents",
            g,
            Some("An agent with access may use any item in this vault by reference; the first use at a new host asks you.".into()),
            theme,
        ));
        // Items (names only, from the relay's cache).
        let mut g = Self::group(theme);
        if v.items.is_empty() {
            g = g.child(Self::footnote("No items listed yet.", theme).pb(px(8.0)));
        }
        for (i, it) in v.items.iter().take(50).enumerate() {
            let fields: Vec<&str> = it["fields"]
                .as_array()
                .map(|a| a.iter().filter_map(|f| f.as_str()).collect())
                .unwrap_or_default();
            let on = it["inject_env"].as_bool().unwrap_or(false);
            let env = it["env"].as_str().unwrap_or("").to_string();
            let reference = it["ref"].as_str().unwrap_or("").to_string();
            let vid = v.id.clone();
            g = g.child(
                div()
                    .id(format!("vault-item-{i}"))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .px(px(16.0))
                    .py(px(8.0))
                    .relative()
                    .child(kit::hairline_below(theme))
                    .cursor_pointer()
                    .map(|el| highlight::row(el, false, Shape::Nested(PANEL_RADIUS), theme))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(px(TEXT_BODY))
                                    .text_color(theme.text)
                                    .child(it["title"].as_str().unwrap_or("Item").to_string()),
                            )
                            .child(
                                div()
                                    .text_size(px(TEXT_TINY))
                                    .text_color(theme.text_muted)
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(if on {
                                        format!("Real value as ${env} in the agents' environment")
                                    } else {
                                        fields.join(" · ")
                                    }),
                            ),
                    )
                    .child(glass::switch(on, theme))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let (vid, reference) = (vid.clone(), reference.clone());
                        this.state
                            .update(cx, |st, cx| st.vault_item_env(vid, reference, !on, cx));
                    })),
            );
        }
        body = body.child(Self::section(
            "Items",
            g,
            Some(format!(
                "Agents reference an item as op://{}/<item>/<field>. Switch an item on to put its real value in the environment of every agent with access, as $NAME.",
                v.name
            )),
            theme,
        ));

        // Activity for this vault (from the overview).
        let acts: Vec<SecretActivity> = self
            .state
            .read(cx)
            .secrets
            .activity
            .iter()
            .filter(|a| a.name == v.name)
            .take(5)
            .cloned()
            .collect();
        let mut g = Self::group(theme);
        if acts.is_empty() {
            g = g.child(Self::footnote("No activity in the last 30 days.", theme).pb(px(8.0)));
        }
        for row in Self::activity_rows(&acts, theme) {
            g = g.child(row);
        }
        body = body.child(Self::section("Activity", g, None, theme));

        // Replace token…
        let vid_t = v.id.clone();
        if self.replacing {
            let mut form = div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .px(px(16.0))
                .py(px(10.0));
            form = form.child(Self::field(
                "Token",
                self.token_input.clone(),
                Some("Stored encrypted. Arbos never shows it again."),
                theme,
            ));
            form = form.child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .child(
                        glass::primary_button("vault-token-save", "Replace token", theme)
                            .h(px(30.0))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let token = this.token_input.read(cx).text().trim().to_string();
                                if token.len() < 20 {
                                    this.error =
                                        Some("Paste the whole service-account token.".into());
                                    cx.notify();
                                    return;
                                }
                                let id = vid_t.clone();
                                this.state
                                    .update(cx, |st, cx| st.vault_replace_token(id, token, cx));
                                this.replacing = false;
                                this.error = None;
                                this.token_input.update(cx, |i, cx| i.clear(cx));
                                cx.notify();
                            })),
                    )
                    .child(
                        glass::quiet_button("vault-token-cancel", "Cancel", theme)
                            .h(px(30.0))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.replacing = false;
                                this.error = None;
                                cx.notify();
                            })),
                    ),
            );
            body = body.child(Self::group(theme).child(form));
        } else {
            body = body.child(Self::group(theme).child(
                Self::action_row("vault-token", "Replace token…", false, theme).on_click(
                    cx.listener(|this, _, window, cx| {
                        this.replacing = true;
                        this.error = None;
                        let h = this.token_input.read(cx).focus_handle(cx);
                        window.focus(&h, cx);
                        cx.notify();
                    }),
                ),
            ));
        }

        let armed = self.confirm.as_deref() == Some("disconnect");
        let agents: Vec<&str> = v
            .agents
            .iter()
            .filter(|a| a.enabled)
            .map(|a| a.name.as_str())
            .collect();
        let who = if agents.is_empty() {
            "Agents".to_string()
        } else {
            agents.join(" and ")
        };
        let vid = v.id.clone();
        body.child(
            div()
                .flex()
                .flex_col()
                .child(Self::group(theme).child(
                    Self::action_row(
                        "vault-disconnect",
                        if armed {
                            format!("Disconnect this vault? {who} lose every item in it. Click again.")
                        } else {
                            "Disconnect".to_string()
                        },
                        true,
                        theme,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.confirm.as_deref() == Some("disconnect") {
                            this.confirm = None;
                            let id = vid.clone();
                            this.state.update(cx, |st, cx| st.vault_disconnect(id, cx));
                            this.go(Page::List, cx);
                        } else {
                            this.confirm = Some("disconnect".into());
                            cx.notify();
                        }
                    })),
                ))
                .child(Self::footnote("Also revoke the service account in 1Password.", theme)),
        )
    }

    fn title(&self, view: &SecretsView, cx: &App) -> String {
        match &self.page {
            Page::List => "Secrets".into(),
            Page::Secret(id) => view
                .secret(id)
                .map(|s| s.env.clone())
                .unwrap_or_else(|| "Secret".into()),
            Page::Add => "New secret".into(),
            Page::Agent(pid) => format!(
                "{} · Secrets",
                self.state
                    .read(cx)
                    .project(pid)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "Agent".into())
            ),
            Page::Connect => "Connect 1Password".into(),
            Page::Vault(id) => view
                .vaults
                .iter()
                .find(|v| v.id == *id)
                .map(|v| format!("{} · {}", v.provider_label(), v.name))
                .unwrap_or_else(|| "Vault".into()),
        }
    }
}

impl Focusable for SecretsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SecretsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let view = self.state.read(cx).secrets.clone();
        self.apply_picker_rig(window, cx);
        if let Some(env) = self.pending_env.clone() {
            if let Some(s) = view.secrets.iter().find(|s| s.env == env) {
                self.pending_env = None;
                self.page = Page::Secret(s.id.clone());
                if std::mem::take(&mut self.pending_people) {
                    let id = s.id.clone();
                    self.state
                        .update(cx, |st, cx| st.open_secret_people_picker(id, cx));
                }
            }
        }
        if self.pending_vault {
            if let Some(v) = view.vaults.first() {
                self.pending_vault = false;
                self.page = Page::Vault(v.id.clone());
            }
        }
        // The rig may name an agent before the project list arrived; swap in its id once known.
        if let Page::Agent(pid) = self.page.clone() {
            let resolved = self
                .state
                .read(cx)
                .projects
                .iter()
                .find(|p| p.id != pid && p.name.eq_ignore_ascii_case(&pid))
                .map(|p| p.id.clone());
            if let Some(id) = resolved {
                self.page = Page::Agent(id.clone());
                self.state
                    .update(cx, |s, cx| s.project_secrets_refresh(id, cx));
            }
        }
        let title = self.title(&view, cx);
        let embedded = self.embedded;
        let back_label = if self.page == Page::List {
            "‹ Settings"
        } else {
            "‹ Secrets"
        };

        // Inside the Settings sheet the list page has the sheet's own back
        // button; sub-pages keep "‹ Secrets" to return to the list.
        let header = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(4.0))
            .when(!(embedded && self.page == Page::List), |el| {
                el.child(
                    div()
                        .id("secrets-back")
                        .text_size(px(TEXT_SMALL))
                        .text_color(theme.text_muted)
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme.text))
                        .child(back_label)
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.page == Page::List {
                                this.hide(cx);
                                cx.emit(SecretsEvent::Back);
                            } else {
                                this.go(Page::List, cx);
                            }
                        })),
                )
            })
            .child(
                div()
                    .flex_1()
                    .text_center()
                    .text_size(px(TEXT_BODY))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .when(!(embedded && self.page == Page::List), |el| el.child(title)),
            )
            .child(
                div()
                    .id("secrets-refresh")
                    .text_size(px(TEXT_SMALL))
                    .text_color(theme.text_muted)
                    .cursor_pointer()
                    .hover(|s| s.text_color(theme.text))
                    .child("Refresh")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let page = this.page.clone();
                        this.state.update(cx, |s, cx| {
                            s.secrets_refresh(cx);
                            if let Page::Agent(pid) = page {
                                s.project_secrets_refresh(pid, cx);
                            }
                        });
                    })),
            );

        let body = match self.page.clone() {
            Page::List => self.render_list(&view, &theme, cx),
            Page::Secret(id) => match view.secret(&id) {
                Some(s) => self.render_secret(s, &theme, cx),
                None => div().child(Self::footnote("This secret is gone.", &theme)),
            },
            Page::Add => self.render_add(&theme, cx),
            Page::Agent(pid) => self.render_agent(&pid, &theme, cx),
            Page::Connect => self.render_connect(&theme, cx),
            Page::Vault(id) => match view.vaults.iter().find(|v| v.id == id) {
                Some(v) => self.render_vault(v, &theme, cx),
                None => div().child(Self::footnote("This vault is gone.", &theme)),
            },
        };

        // The page's note — under the header, before the first field (Jacob 2026-10-26): the
        // relay's refusal of a save / connect / delete (claimed from the toast stack while a
        // form is up) or the form's own validation, through the one shared element.
        let form_open = matches!(
            self.page,
            Page::Add | Page::Connect | Page::Secret(_) | Page::Vault(_) | Page::Agent(_)
        );
        let claim = form_open.then_some(crate::app::FailureClaim {
            surface: "secrets",
            verbs: SECRETS_VERBS,
        });
        if self.state.read(cx).failure_claim != claim {
            self.state.update(cx, |s, _| s.set_failure_claim(claim));
        }
        let note = self.note(&theme, cx);
        let card = div()
            .id("secrets-popover")
            .track_focus(&self.focus_handle)
            .flex()
            .flex_col()
            .gap(px(10.0))
            .p(px(12.0))
            .child(header)
            .children(note)
            .child(
                div()
                    .id("secrets-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .pr(px(2.0))
                    .child(body),
            );
        if embedded {
            // The sheet body scrolls; no height cap of our own.
            return card.w_full().into_any_element();
        }
        let card = card
            .w(px(460.0))
            .max_h(px(680.0))
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

#[cfg(test)]
mod picker_tests {
    //! The real `Shell` headless with the Secrets page open: the four agent pickers (New
    //! secret, Connect 1Password, a secret's "Give another agent access…", a vault's) draw
    //! the one component — chips for the granted agents, one "Add agent" button, a dropdown
    //! that filters through the matcher and hides the agents with access — and ↩ / × add and
    //! remove chips ([feedback secrets-agent-picker-mac-2026-10-26]).
    use futures::future::BoxFuture;
    use gpui::{
        point, px, size, AppContext, Bounds, Entity, Modifiers, Pixels, TestAppContext,
        VisualTestContext,
    };
    use tokio::sync::mpsc;

    use super::Page;
    use crate::app::AppState;
    use crate::model::{AccountView, ConnectedVault, Project, Secret, SecretsView};
    use crate::sync::{ClientCommand, SyncEvent, SyncSource};
    use crate::ui::agent_picker::{ADD_SELECTOR, DROPDOWN_SELECTOR, EMPTY_SELECTOR};
    use crate::ui::motion::PulseClock;
    use crate::ui::shell::{SettingsPage, Shell};
    use crate::ui::theme::Theme;

    struct Idle;

    impl SyncSource for Idle {
        fn name(&self) -> &'static str {
            "idle"
        }

        fn run(
            self: Box<Self>,
            _events: mpsc::UnboundedSender<SyncEvent>,
            _commands: mpsc::UnboundedReceiver<ClientCommand>,
        ) -> BoxFuture<'static, anyhow::Result<()>> {
            Box::pin(std::future::pending())
        }
    }

    const AGENTS: [(&str, &str); 6] = [
        ("a1", "Affine"),
        ("a2", "Arbos"),
        ("a3", "ArbosTest"),
        ("a4", "Doug agent"),
        ("a5", "Hezbola"),
        ("a6", "Arbonites"),
    ];

    fn secret_with_access(id: &str, env: &str, agents: &[(&str, &str)]) -> Secret {
        let agents: Vec<serde_json::Value> = agents
            .iter()
            .map(|(pid, name)| {
                serde_json::json!({ "project_id": pid, "name": name, "access_id": format!("acc-{pid}"), "enabled": true })
            })
            .collect();
        serde_json::from_value(serde_json::json!({
            "id": id, "name": env, "env": env, "kind": "key", "agents": agents
        }))
        .expect("secret fixture")
    }

    fn vault_with_access(id: &str, agents: &[(&str, &str)]) -> ConnectedVault {
        let agents: Vec<serde_json::Value> = agents
            .iter()
            .map(|(pid, name)| {
                serde_json::json!({ "project_id": pid, "name": name, "access_id": format!("vacc-{pid}"), "enabled": true })
            })
            .collect();
        serde_json::from_value(serde_json::json!({
            "id": id, "provider": "onepassword", "name": "Research", "items_count": 3, "status": "ok", "agents": agents
        }))
        .expect("vault fixture")
    }

    fn mount(
        cx: &mut TestAppContext,
    ) -> (
        tokio::runtime::Runtime,
        Entity<AppState>,
        Entity<Shell>,
        &mut VisualTestContext,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let handle = rt.handle().clone();
        cx.update(|cx| {
            gpui_tokio::init_from_handle(cx, handle);
            cx.set_reduce_motion(true);
            cx.set_global(Theme::dark(crate::ui::theme::Surface::Matte));
            if !cx.has_global::<PulseClock>() {
                PulseClock::init(cx);
            }
        });
        let state = cx.new(|cx| AppState::new(Box::new(Idle), cx));
        state.update(cx, |s, cx| {
            s.account = Some(AccountView {
                id: "gh_1".into(),
                login: "unconst".into(),
                name: "Jacob".into(),
                avatar_url: String::new(),
                device_name: "test".into(),
                balance_usd: 0.0,
                spent_today_usd: 0.0,
                daily_cap_usd: None,
            });
            for (id, name) in AGENTS {
                s.projects.push(Project {
                    id: id.into(),
                    name: name.into(),
                    ..Project::default()
                });
            }
            s.selected_project = Some("a1".into());
            s.secrets = SecretsView {
                loaded: true,
                secrets: vec![secret_with_access("s1", "STRIPE_KEY", &[("a2", "Arbos")])],
                vaults: vec![vault_with_access(
                    "v1",
                    &[("a1", "Affine"), ("a2", "Arbos")],
                )],
                ..SecretsView::default()
            };
            cx.notify();
        });
        let (shell, vcx) = cx.add_window_view(|window, cx| Shell::new(state.clone(), window, cx));
        // Tall enough that every form's Agents block is on screen without scrolling the pane.
        vcx.simulate_resize(size(px(1180.0), px(1400.0)));
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            shell.update(cx, |sh, cx| {
                sh.open_settings_page(SettingsPage::Secrets, window, cx)
            })
        });
        vcx.run_until_parked();
        (rt, state, shell, vcx)
    }

    fn settle(shell: &Entity<Shell>, vcx: &mut VisualTestContext) {
        for _ in 0..3 {
            shell.update(vcx, |_, cx| cx.notify());
            vcx.run_until_parked();
        }
    }

    fn centre(b: Bounds<Pixels>) -> gpui::Point<Pixels> {
        point(b.left() + b.size.width / 2.0, b.top() + b.size.height / 2.0)
    }

    fn secrets_page(shell: &Entity<Shell>, vcx: &mut VisualTestContext, page: Page, giving: bool) {
        shell.update(vcx, |sh, cx| {
            sh.secrets.update(cx, |p, cx| {
                p.page = page;
                p.giving = giving;
                cx.notify();
            });
        });
        settle(shell, vcx);
    }

    /// `debug_bounds` wants a `'static` selector; the row ids are few and fixed.
    fn row_selector(i: usize) -> &'static str {
        Box::leak(format!("agent-picker-row-{i}").into_boxed_str())
    }

    fn rows(vcx: &mut VisualTestContext) -> usize {
        (0..20)
            .take_while(|i| vcx.debug_bounds(row_selector(*i)).is_some())
            .count()
    }

    fn picked(shell: &Entity<Shell>, vcx: &mut VisualTestContext) -> Vec<String> {
        shell.read_with(&*vcx, |sh, cx| {
            sh.secrets.read(cx).agent_picker.picked.clone()
        })
    }

    #[gpui::test]
    fn new_secret_filters_through_the_matcher_and_enter_adds_a_chip(cx: &mut TestAppContext) {
        let (_rt, _state, shell, vcx) = mount(cx);
        secrets_page(&shell, vcx, Page::Add, false);
        assert!(
            vcx.debug_bounds(EMPTY_SELECTOR).is_some(),
            "empty state hint"
        );
        assert!(
            vcx.debug_bounds(DROPDOWN_SELECTOR).is_none(),
            "closed at rest"
        );
        let add = vcx
            .debug_bounds(ADD_SELECTOR)
            .expect("one Add agent button");
        assert_eq!(f32::from(add.size.height), 30.0, "the chip family's height");

        vcx.simulate_click(centre(add), Modifiers::default());
        settle(&shell, vcx);
        let dd = vcx
            .debug_bounds(DROPDOWN_SELECTOR)
            .expect("the dropdown is up");
        assert!(
            dd.top() >= add.bottom() || dd.bottom() <= add.top(),
            "under the chips row (or over it when the window has no room below): {dd:?} vs {add:?}"
        );
        assert!(
            (f32::from(dd.left()) - f32::from(add.left())).abs() < 1.0,
            "anchored at the row's left edge"
        );
        assert_eq!(f32::from(dd.size.width), 308.0, "the content width");
        assert_eq!(rows(vcx), AGENTS.len(), "no query: every agent of mine");

        vcx.simulate_input("ar");
        settle(&shell, vcx);
        assert_eq!(rows(vcx), 3, "“ar” → Arbos, ArbosTest, Arbonites");

        vcx.dispatch_action(crate::ui::composer::Down);
        vcx.run_until_parked();
        vcx.dispatch_action(crate::ui::composer::Send);
        settle(&shell, vcx);
        assert_eq!(
            picked(&shell, vcx),
            vec!["a3".to_string()],
            "↓ then ↩ added ArbosTest"
        );
        assert!(
            vcx.debug_bounds(DROPDOWN_SELECTOR).is_none(),
            "one agent per visit: the list folds"
        );
        assert!(
            vcx.debug_bounds("agent-picker-chip-a3").is_some(),
            "its chip is drawn"
        );
        assert!(vcx.debug_bounds(EMPTY_SELECTOR).is_none());

        // The chip's × removes it again.
        let x = vcx
            .debug_bounds("agent-picker-chip-x-a3")
            .expect("× on the chip");
        vcx.simulate_click(centre(x), Modifiers::default());
        settle(&shell, vcx);
        assert!(picked(&shell, vcx).is_empty(), "× removed the grant");
        assert!(vcx.debug_bounds("agent-picker-chip-a3").is_none());
    }

    #[gpui::test]
    fn escape_folds_the_dropdown_and_a_picked_agent_leaves_the_list(cx: &mut TestAppContext) {
        let (_rt, _state, shell, vcx) = mount(cx);
        secrets_page(&shell, vcx, Page::Add, false);
        let add = vcx.debug_bounds(ADD_SELECTOR).expect("Add agent");
        vcx.simulate_click(centre(add), Modifiers::default());
        settle(&shell, vcx);
        let row = vcx.debug_bounds("agent-picker-row-0").expect("first row");
        vcx.simulate_click(centre(row), Modifiers::default());
        settle(&shell, vcx);
        assert_eq!(
            picked(&shell, vcx),
            vec!["a1".to_string()],
            "a click adds Affine"
        );

        let add = vcx.debug_bounds(ADD_SELECTOR).expect("Add agent");
        vcx.simulate_click(centre(add), Modifiers::default());
        settle(&shell, vcx);
        assert_eq!(
            rows(vcx),
            AGENTS.len() - 1,
            "Affine is a chip now, not a row"
        );
        vcx.dispatch_action(crate::ui::shell::Escape);
        settle(&shell, vcx);
        assert!(
            vcx.debug_bounds(DROPDOWN_SELECTOR).is_none(),
            "esc folds the list"
        );
        assert_eq!(
            picked(&shell, vcx),
            vec!["a1".to_string()],
            "the chip stays"
        );
    }

    #[gpui::test]
    fn every_secrets_picker_is_the_component_and_hides_the_agents_with_access(
        cx: &mut TestAppContext,
    ) {
        let (_rt, _state, shell, vcx) = mount(cx);
        // Connect 1Password: nothing has access yet, every agent is offered.
        secrets_page(&shell, vcx, Page::Connect, false);
        let add = vcx
            .debug_bounds(ADD_SELECTOR)
            .expect("Connect form: Add agent");
        vcx.simulate_click(centre(add), Modifiers::default());
        settle(&shell, vcx);
        assert_eq!(rows(vcx), AGENTS.len());
        vcx.dispatch_action(crate::ui::shell::Escape);
        settle(&shell, vcx);

        // A secret's page, "Give another agent access…": Arbos has access → not offered.
        secrets_page(&shell, vcx, Page::Secret("s1".into()), true);
        let add = vcx
            .debug_bounds(ADD_SELECTOR)
            .expect("secret page: Add agent");
        vcx.simulate_click(centre(add), Modifiers::default());
        settle(&shell, vcx);
        assert_eq!(rows(vcx), AGENTS.len() - 1, "Arbos already has access");
        vcx.simulate_input("arbos");
        settle(&shell, vcx);
        assert_eq!(rows(vcx), 1, "“arbos” → ArbosTest only (Arbos is hidden)");
        vcx.dispatch_action(crate::ui::shell::Escape);
        settle(&shell, vcx);

        // A vault's page: Affine and Arbos have access → four left.
        secrets_page(&shell, vcx, Page::Vault("v1".into()), true);
        let add = vcx
            .debug_bounds(ADD_SELECTOR)
            .expect("vault page: Add agent");
        vcx.simulate_click(centre(add), Modifiers::default());
        settle(&shell, vcx);
        assert_eq!(
            rows(vcx),
            AGENTS.len() - 2,
            "Affine and Arbos already have access"
        );
        vcx.dispatch_action(crate::ui::shell::Escape);
        settle(&shell, vcx);
        assert!(vcx.debug_bounds(DROPDOWN_SELECTOR).is_none());
    }
}
