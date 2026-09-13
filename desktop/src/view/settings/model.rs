//! The model section: which provider answers, where its key is, and the
//! model a chat uses when it says `inherit`.
//!
//! It edits the same `~/.config/arbos/config.toml` that `arbos-kernel setup`
//! writes, so the two never disagree. The key is typed into a masked field
//! (or pasted from the clipboard) and goes straight to the file: this window
//! never shows it back, only where it came from.

use crate::{
    kernel::{self, HostSummary},
    view::settings::{self, SettingsWindow},
};
use arbos_core::host::{KeySource, ProviderKind};
use bezel::{
    gpui::{AnyElement, ClipboardItem, Context, Entity, div, prelude::*, px},
    theme::{TextStyle, Theme, Typeset},
    ui::{
        input::{FieldEvent, TextField},
        widgets::{ButtonStyle, Buttons, Content, Scaffolding, Status},
    },
};

/// Model chips shown under the search field at most.
const MODEL_MATCHES: usize = 12;

/// What the section holds between draws. `summary` is re-read after every
/// save; `pending` is a key check on the background executor.
pub(super) struct HostPanel {
    pub summary: HostSummary,
    pub pending: bool,
    /// The last thing that happened, for the strip under the key row.
    pub note: Option<Note>,
    /// Typed key entry, masked. Cleared once saved.
    pub key_field: Entity<TextField>,
    /// The custom base URL, for the Custom provider.
    pub base_field: Entity<TextField>,
    /// Live filter over the provider's model catalog.
    pub model_search: Entity<TextField>,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) enum Note {
    Ok(String),
    Problem(String),
}

impl HostPanel {
    pub fn new(cx: &mut Context<SettingsWindow>) -> Self {
        let summary = kernel::host_summary();
        let key_field = cx.new(|cx| {
            TextField::new(cx)
                .with_masked(true)
                .with_placeholder("Type or paste the key")
        });
        let base_field = cx.new(|cx| {
            let mut f = TextField::new(cx).with_placeholder("https://host/v1");
            if summary.provider == ProviderKind::Custom && !summary.base.starts_with('(') {
                f.set_content(summary.base.clone(), cx);
            }
            f
        });
        let model_search = cx.new(|cx| TextField::new(cx).with_placeholder("Search models"));
        for field in [&key_field, &base_field, &model_search] {
            cx.subscribe(
                field,
                |_this: &mut SettingsWindow, _, event: &FieldEvent, cx| {
                    if *event == FieldEvent::Changed {
                        cx.notify();
                    }
                },
            )
            .detach();
        }
        Self {
            summary,
            pending: false,
            note: None,
            key_field,
            base_field,
            model_search,
        }
    }

    pub fn refresh(&mut self) {
        self.summary = kernel::host_summary();
    }
}

impl SettingsWindow {
    pub(super) fn model_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let panel = &self.host;
        div()
            .flex()
            .flex_col()
            .gap(px(settings::GROUP_GAP))
            .children(
                panel
                    .summary
                    .error
                    .as_ref()
                    .map(|e| theme.error_strip(format!("config.toml did not parse: {e}"))),
            )
            .child(
                theme
                    .group_box()
                    .child(self.provider_row(cx))
                    .children(
                        (self.host.summary.provider == ProviderKind::Custom)
                            .then(|| self.base_row(cx)),
                    )
                    .child(self.key_row(cx))
                    .child(self.model_row(cx)),
            )
            .children(panel.note.as_ref().map(|note| match note {
                Note::Ok(text) => theme.badge_active(text.clone()).into_any_element(),
                Note::Problem(text) => theme.error_strip(text.clone()).into_any_element(),
            }))
            .child(self.file_group(cx))
            .into_any_element()
    }

    fn provider_row(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = Theme::of(cx).clone();
        let current = self.host.summary.provider;
        let base = self.host.summary.base.clone();
        theme
            .card_row(true)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(theme.row_title("Provider"))
                    .child(
                        div()
                            .mt(px(4.))
                            .text_style(TextStyle::Subheadline)
                            .text_color(theme.text_muted)
                            .child(base),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .gap(px(2.))
                    .p(px(2.))
                    .rounded(px(Theme::button_radius()))
                    .border_1()
                    .border_color(theme.border)
                    .children(ProviderKind::ALL.into_iter().enumerate().map(|(ix, kind)| {
                        let selected = kind == current;
                        div()
                            .id(("provider", ix))
                            .px(px(10.))
                            .py(px(4.))
                            .rounded(px(Theme::control_radius()))
                            .text_style(TextStyle::Callout)
                            .cursor_pointer()
                            .when(selected, |el| {
                                el.bg(theme.element_active).text_color(theme.text)
                            })
                            .when(!selected, |el| {
                                el.text_color(theme.text_muted)
                                    .hover(|el| el.bg(theme.element_hover))
                            })
                            .child(kind.label())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_provider(kind, cx);
                            }))
                    })),
            )
    }

    fn key_row(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = Theme::of(cx).clone();
        let summary = &self.host.summary;
        let (status, missing) = match &summary.key {
            KeySource::Config => ("Saved in config.toml.".to_string(), false),
            KeySource::Env(env) => (format!("Read from ${env}."), false),
            KeySource::Missing(env) => (
                match summary.provider.keys_url() {
                    Some(url) => format!(
                        "None yet. Get one at {url}, copy it, then paste it here. Or set ${env}."
                    ),
                    None => format!("None yet. Copy it, then paste it here. Or set ${env}."),
                },
                true,
            ),
        };
        let pending = self.host.pending;
        let saved = summary.key == KeySource::Config;
        // The kernel behind the open chat may read another config (a remote
        // machine, a key set in memory): say what it has, not only what
        // this window's file says (ui-010).
        let kernel_line = self
            .workspace
            .read(cx)
            .active_session()
            .and_then(|chat| chat.kernel_provider.clone())
            .map(|k| {
                if k.key {
                    format!(
                        "The kernel of the open chat answers with {} ({}); key from {}.",
                        k.provider, k.model, k.source
                    )
                } else {
                    format!("The kernel of the open chat has no key for {}.", k.provider)
                }
            });
        theme
            .card_row(false)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(theme.row_title("API key"))
                    .child(
                        div()
                            .mt(px(4.))
                            .text_style(TextStyle::Subheadline)
                            .text_color(if missing {
                                theme.warning_muted
                            } else {
                                theme.text_muted
                            })
                            .child(status),
                    )
                    .children(kernel_line.map(|line| {
                        div()
                            .mt(px(2.))
                            .text_style(TextStyle::Subheadline)
                            .text_color(theme.text_muted)
                            .child(line)
                    })),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .id("key-field")
                            .w(px(220.))
                            .child(self.host.key_field.clone()),
                    )
                    .child(
                        theme
                            .button("Save key", ButtonStyle::Ghost, None)
                            .id("key-save")
                            .on_click(cx.listener(|this, _, _, cx| this.save_typed_key(cx))),
                    )
                    .when(saved, |row| {
                        row.child(
                            theme
                                .button("Forget", ButtonStyle::Ghost, None)
                                .id("key-forget")
                                .on_click(cx.listener(|this, _, _, cx| this.forget_key(cx))),
                        )
                    })
                    .child(
                        theme
                            .button(
                                if pending {
                                    "Checking…"
                                } else if saved {
                                    "Paste new key"
                                } else {
                                    "Paste key"
                                },
                                if missing {
                                    ButtonStyle::Prominent
                                } else {
                                    ButtonStyle::Ghost
                                },
                                None,
                            )
                            .id("key-paste")
                            .when(pending, |el| el.opacity(0.5))
                            .on_click(cx.listener(|this, _, _, cx| this.paste_key(cx))),
                    ),
            )
    }

    /// The Custom provider's base URL, typed.
    fn base_row(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = Theme::of(cx).clone();
        theme
            .card_row(false)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(theme.row_title("Base URL"))
                    .child(
                        div()
                            .mt(px(4.))
                            .text_style(TextStyle::Subheadline)
                            .text_color(theme.text_muted)
                            .child("An OpenAI-compatible endpoint, up to and including /v1."),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .id("base-field")
                            .w(px(280.))
                            .child(self.host.base_field.clone()),
                    )
                    .child(
                        theme
                            .button("Save", ButtonStyle::Ghost, None)
                            .id("base-save")
                            .on_click(cx.listener(|this, _, _, cx| this.save_base(cx))),
                    ),
            )
    }

    fn model_row(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = Theme::of(cx).clone();
        let summary = &self.host.summary;
        let current = summary.model.clone();
        let query = self
            .host
            .model_search
            .read(cx)
            .content()
            .trim()
            .to_ascii_lowercase();
        let catalog = self.workspace.read(cx).models.clone();
        let picks: Vec<String> = if query.is_empty() {
            summary
                .provider
                .suggested_models()
                .iter()
                .map(|s| s.to_string())
                .collect()
        } else {
            // The live catalog first (what the provider will accept), the
            // suggestions as a fallback when the catalog is empty.
            let mut pool: Vec<String> = catalog.models.iter().map(|m| m.id.clone()).collect();
            if pool.is_empty() {
                pool = summary
                    .provider
                    .suggested_models()
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
            }
            pool.retain(|id| {
                id.to_ascii_lowercase().contains(&query)
                    || kernel::model_display_name(id)
                        .to_ascii_lowercase()
                        .contains(&query)
            });
            pool.truncate(MODEL_MATCHES);
            pool
        };
        let no_match = !query.is_empty() && picks.is_empty();
        let typed_model = (!query.is_empty())
            .then(|| self.host.model_search.read(cx).content().trim().to_string());
        theme.card_row(false).child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(theme.row_title("Default model"))
                .child(
                    div()
                        .mt(px(4.))
                        .text_style(TextStyle::Subheadline)
                        .text_color(theme.text_muted)
                        .child(format!(
                            "{current} — what a chat uses until /model picks another."
                        )),
                )
                .child(
                    div()
                        .id("model-search")
                        .mt(px(10.))
                        .w(px(320.))
                        .child(self.host.model_search.clone()),
                )
                .when(no_match, |col| {
                    let typed = typed_model.clone().unwrap_or_default();
                    col.child(
                        div()
                            .mt(px(6.))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_style(TextStyle::Caption)
                                    .text_color(theme.text_faint)
                                    .child(format!("No catalog match for \"{typed}\".")),
                            )
                            .child(
                                theme
                                    .button("Use it anyway", ButtonStyle::Ghost, None)
                                    .id("model-use-typed")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_model(&typed, cx);
                                    })),
                            ),
                    )
                })
                .when(!picks.is_empty(), |col| {
                    col.child(
                        div()
                            .mt(px(10.))
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap(px(6.))
                            .children(picks.into_iter().enumerate().map(|(ix, id)| {
                                let selected = id == current;
                                let label = kernel::model_display_name(&id);
                                let pick = id.clone();
                                div()
                                    .id(("model-pick", ix))
                                    .px(px(8.))
                                    .py(px(2.))
                                    .rounded_full()
                                    .border_1()
                                    .border_color(if selected {
                                        theme.accent
                                    } else {
                                        theme.border
                                    })
                                    .text_style(TextStyle::Caption)
                                    .cursor_pointer()
                                    .when(selected, |el| {
                                        el.bg(theme.element_active).text_color(theme.text)
                                    })
                                    .when(!selected, |el| {
                                        el.text_color(theme.text_muted)
                                            .hover(|el| el.bg(theme.element_hover))
                                    })
                                    .child(label)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_model(&pick, cx);
                                    }))
                            })),
                    )
                }),
        )
    }

    fn file_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let path = self.host.summary.config_path.clone();
        let shown = path.display().to_string();
        div()
            .flex()
            .flex_col()
            .gap(px(settings::LABEL_GAP))
            .child(theme.field_label("File"))
            .child(
                theme.group_box().child(
                    theme
                        .card_row(true)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(theme.row_title("config.toml"))
                                .child(
                                    div()
                                        .mt(px(4.))
                                        .text_style(TextStyle::Subheadline)
                                        .text_color(theme.text_muted)
                                        .child(format!(
                                            "{shown} — the same file `arbos-kernel setup` writes."
                                        )),
                                ),
                        )
                        .child(
                            theme
                                .button("Reveal", ButtonStyle::Ghost, None)
                                .id("config-reveal")
                                .on_click(move |_, _, cx| cx.reveal_path(&path)),
                        ),
                ),
            )
            .into_any_element()
    }

    fn set_provider(&mut self, kind: ProviderKind, cx: &mut Context<Self>) {
        match kernel::save_host_provider(kind) {
            Ok(summary) => {
                self.host.summary = summary;
                self.host.note = None;
            }
            Err(e) => self.host.note = Some(Note::Problem(format!("{e:#}"))),
        }
        cx.notify();
    }

    fn set_model(&mut self, model: &str, cx: &mut Context<Self>) {
        match kernel::save_host_model(model) {
            Ok(summary) => self.host.summary = summary,
            Err(e) => self.host.note = Some(Note::Problem(format!("{e:#}"))),
        }
        cx.notify();
    }

    fn forget_key(&mut self, cx: &mut Context<Self>) {
        match kernel::save_host_key("") {
            Ok(summary) => {
                self.host.summary = summary;
                self.host.note = Some(Note::Ok("Key removed from config.toml.".into()));
            }
            Err(e) => self.host.note = Some(Note::Problem(format!("{e:#}"))),
        }
        cx.notify();
    }

    /// Take the key off the clipboard, check it with the provider, save it.
    /// The text never reaches an element: a bad paste is named by shape
    /// ("that was 3 lines"), not by content.
    /// The Custom provider's typed base URL, saved to config.toml.
    fn save_base(&mut self, cx: &mut Context<Self>) {
        let base = self.host.base_field.read(cx).content().trim().to_string();
        match kernel::save_host_base(&base) {
            Ok(summary) => {
                self.host.summary = summary;
                self.host.note = Some(Note::Ok(if base.is_empty() {
                    "Base URL cleared; the provider's default is used.".into()
                } else {
                    format!("Base URL saved: {base}.")
                }));
            }
            Err(e) => self.host.note = Some(Note::Problem(format!("Not saved: {e:#}."))),
        }
        cx.notify();
    }

    /// The key typed into the masked field: checked with the provider, then
    /// saved; the field is cleared either way so the key is not kept in the
    /// window.
    fn save_typed_key(&mut self, cx: &mut Context<Self>) {
        let key = self.host.key_field.read(cx).content().trim().to_string();
        if key.is_empty() {
            self.host.note = Some(Note::Problem(
                "Type or paste the key into the field first, then press Save key.".into(),
            ));
            cx.notify();
            return;
        }
        self.host.key_field.update(cx, |field, cx| field.clear(cx));
        self.take_key(key, cx);
    }

    fn paste_key(&mut self, cx: &mut Context<Self>) {
        let text = cx
            .read_from_clipboard()
            .as_ref()
            .and_then(ClipboardItem::text)
            .unwrap_or_default();
        let key = text.trim().to_string();
        if key.is_empty() {
            self.host.note = Some(Note::Problem(
                "The clipboard is empty. Copy the key first, then press Paste key.".into(),
            ));
            cx.notify();
            return;
        }
        self.take_key(key, cx);
    }

    /// Check `key` with the provider off the main thread, then save it.
    fn take_key(&mut self, key: String, cx: &mut Context<Self>) {
        if self.host.pending {
            return;
        }
        if key.lines().count() > 1 || key.contains(' ') {
            self.host.note = Some(Note::Problem(format!(
                "That is {} lines with spaces; a key is one word.",
                key.lines().count()
            )));
            cx.notify();
            return;
        }
        self.host.pending = true;
        self.host.note = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let checked = cx
                .background_executor()
                .spawn(async move {
                    kernel::check_host_key(&key)?;
                    kernel::save_host_key(&key)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.host.pending = false;
                match checked {
                    Ok(summary) => {
                        this.host.summary = summary;
                        this.host.note = Some(Note::Ok(
                            "Key accepted and saved. Chats use it from their next turn.".into(),
                        ));
                    }
                    Err(e) => {
                        this.host.refresh();
                        this.host.note = Some(Note::Problem(format!("Not saved: {e:#}.")));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}
