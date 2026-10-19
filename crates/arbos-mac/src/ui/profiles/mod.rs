//! Telegram-style profile pages in the Grok Bot look: a user, a group (people
//! chat or shared project) or an agent. One glass sheet, the same skeleton
//! for all three — hero (avatar, name, status line), a row of round action
//! buttons, an info card, a segmented tab bar and the tab's list.
//!
//! Everything shown is derived from state the app already holds: members from
//! the space, media / files / voice / links from the loaded chat docs,
//! presence from the room frames.

mod agent;
mod group;
mod user;

use std::sync::Arc;

use gpui::{
    div, prelude::*, px, AnyElement, Context, Div, ElementId, FontWeight, Hsla, RenderImage,
    SharedString, Stateful,
};

use crate::app::{AppState, ProfilePage};
use crate::model::{now_ms, Attachment, AttachmentKind, Chat, Message, Part};
use crate::ui::glass;
use crate::ui::icons::icon;
use crate::ui::shell::Shell;
use crate::ui::theme::{Theme, PANEL_RADIUS, TEXT_BODY, TEXT_SMALL, TEXT_TINY};

pub const SHEET_WIDTH: f32 = 480.0;
/// "online" text, shared with the iPhone pages.
pub(super) fn presence_blue() -> Hsla {
    gpui::rgb(0x1084fe).into()
}
/// The "owner" capsule.
pub(super) fn owner_purple() -> Hsla {
    gpui::rgb(0x8b7cf6).into()
}
pub const SHEET_MAX_HEIGHT: f32 = 780.0;
const HERO_AVATAR: f32 = 112.0;

/// The open page, if any.
pub fn sheet(page: &ProfilePage, shell: &Shell, cx: &mut Context<Shell>) -> Option<Div> {
    match page {
        ProfilePage::User { handle, space_id } => {
            user::page(handle, space_id.as_deref(), shell, cx)
        }
        ProfilePage::Group { space_id } => group::page(space_id, shell, cx),
        ProfilePage::Agent { project_id } => agent::page(project_id, shell, cx),
    }
}

// ----- scaffold ----------------------------------------------------------------

/// `[‹ back] (title) [right]` header + scrolling body in the centred glass sheet.
pub(super) fn frame(
    body: Div,
    right: Option<Stateful<Div>>,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Div {
    let back = glass::circle("profile-back", 34.0, theme)
        .child(icon("chevron-left", 15.0, theme.text))
        .on_click(cx.listener(|this, _, window, cx| {
            this.state.update(cx, |s, cx| {
                s.close_profile(cx);
            });
            crate::ui::composer::Composer::focus(&this.composer, window, cx);
        }));
    let header = glass::sheet_header("", Some(back), right, theme);
    glass::sheet(
        SHEET_WIDTH,
        theme,
        header,
        body.id("profile-body")
            .min_h_0()
            .overflow_y_scroll()
            .max_h(px(SHEET_MAX_HEIGHT)),
    )
}

/// Big avatar, name, status line.
pub(super) fn hero(
    avatar: AnyElement,
    name: &str,
    status: &str,
    status_color: Hsla,
    theme: &Theme,
) -> Div {
    div()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(6.0))
        .pt(px(4.0))
        .pb(px(18.0))
        .child(div().size(px(HERO_AVATAR)).child(avatar))
        .child(
            div()
                .pt(px(8.0))
                .text_size(px(TEXT_BODY + 8.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .max_w(px(SHEET_WIDTH - 80.0))
                .truncate()
                .child(name.to_string()),
        )
        .child(
            div()
                .text_size(px(TEXT_SMALL))
                .text_color(status_color)
                .child(status.to_string()),
        )
}

/// One round action: glyph above a caption, like Telegram's call / mute / search row.
pub(super) struct Action {
    pub id: &'static str,
    pub glyph: &'static str,
    pub label: String,
    pub enabled: bool,
    /// Caption under the label for disabled actions ("soon").
    pub note: Option<&'static str>,
}

impl Action {
    pub fn new(id: &'static str, glyph: &'static str, label: impl Into<String>) -> Self {
        Self {
            id,
            glyph,
            label: label.into(),
            enabled: true,
            note: None,
        }
    }

    /// A disabled action with a "soon" caption (none today: voice calls landed with D75).
    #[allow(dead_code)]
    pub fn soon(mut self) -> Self {
        self.enabled = false;
        self.note = Some("soon");
        self
    }
}

pub(super) fn action_button(a: &Action, theme: &Theme) -> Stateful<Div> {
    let fg = if a.enabled {
        theme.primary
    } else {
        theme.text_faint
    };
    div()
        .id(a.id)
        .flex_1()
        .min_w(px(64.0))
        .h(px(60.0))
        .rounded(px(PANEL_RADIUS))
        .bg(theme.surface_1)
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(4.0))
        .when(a.enabled, |el| {
            el.cursor_pointer().hover(|s| s.bg(theme.surface_2))
        })
        .child(icon(a.glyph, 19.0, fg))
        .child(
            div()
                .text_size(px(TEXT_TINY))
                .text_color(fg)
                .child(match a.note {
                    Some(n) => format!("{} · {n}", a.label),
                    None => a.label.clone(),
                }),
        )
}

pub(super) fn action_row(buttons: Vec<Stateful<Div>>) -> Div {
    div()
        .flex_shrink_0()
        .flex()
        .gap(px(8.0))
        .px(px(16.0))
        .pb(px(14.0))
        .children(buttons)
}

/// Info card row: small caption over a value (Telegram's "username / @x").
pub(super) fn info_row(caption: &str, value: impl Into<SharedString>, theme: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .px(px(16.0))
        .py(px(10.0))
        .border_b_1()
        .border_color(theme.border)
        .child(
            div()
                .text_size(px(TEXT_TINY))
                .text_color(theme.text_muted)
                .child(caption.to_string()),
        )
        .child(
            div()
                .text_size(px(TEXT_BODY))
                .text_color(theme.text)
                .child(value.into()),
        )
}

/// Info card action row ("Add to Contacts", "Block User"): coloured label, clickable.
pub(super) fn info_action(
    id: impl Into<ElementId>,
    label: &str,
    color: Hsla,
    glyph: Option<&str>,
    theme: &Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(10.0))
        .min_h(px(44.0))
        .px(px(16.0))
        .border_b_1()
        .border_color(theme.border)
        .text_size(px(TEXT_BODY))
        .text_color(color)
        .cursor_pointer()
        .hover(|s| s.bg(theme.surface_2))
        .when_some(glyph, |el, g| el.child(icon(g, 16.0, color)))
        .child(label.to_string())
}

pub(super) fn card(theme: &Theme) -> Div {
    glass::group(theme)
        .flex_shrink_0()
        .mx(px(16.0))
        .mb(px(14.0))
}

/// Segmented tab bar; `tabs[i]` is selected when `i == current`.
pub(super) fn tab_bar(
    tabs: &[&'static str],
    current: usize,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Div {
    let mut bar = div()
        .flex_shrink_0()
        .mx(px(16.0))
        .mb(px(10.0))
        .p(px(3.0))
        .rounded_full()
        .bg(theme.surface_1)
        .border_1()
        .border_color(theme.border)
        .flex()
        .gap(px(2.0));
    for (i, name) in tabs.iter().enumerate() {
        let on = i == current;
        bar = bar.child(
            div()
                .id(ElementId::Name(SharedString::from(format!(
                    "profile-tab-{name}"
                ))))
                .flex_1()
                .h(px(30.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(TEXT_SMALL))
                .font_weight(if on {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                })
                .text_color(if on { theme.text } else { theme.text_muted })
                .bg(if on {
                    theme.surface_2
                } else {
                    gpui::transparent_black()
                })
                .cursor_pointer()
                .hover(|s| s.bg(theme.surface_2))
                .child(name.to_string())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.update(cx, |s, cx| s.set_profile_tab(i, cx));
                })),
        );
    }
    bar
}

/// Centered grey line for an empty tab.
pub(super) fn empty(text: &str, theme: &Theme) -> Div {
    div()
        .flex_shrink_0()
        .px(px(16.0))
        .py(px(28.0))
        .flex()
        .justify_center()
        .text_size(px(TEXT_SMALL))
        .text_color(theme.text_muted)
        .child(text.to_string())
}

/// A list row: leading element, title, subtitle, optional trailing element.
pub(super) fn list_row(
    id: impl Into<ElementId>,
    leading: AnyElement,
    title: impl Into<SharedString>,
    subtitle: impl Into<SharedString>,
    subtitle_color: Hsla,
    trailing: Option<AnyElement>,
    theme: &Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(12.0))
        .min_h(px(56.0))
        .px(px(14.0))
        .py(px(6.0))
        .border_b_1()
        .border_color(theme.border)
        .child(leading)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(1.0))
                .child(
                    div()
                        .text_size(px(TEXT_BODY))
                        .text_color(theme.text)
                        .truncate()
                        .child(title.into()),
                )
                .child(
                    div()
                        .text_size(px(TEXT_SMALL))
                        .text_color(subtitle_color)
                        .truncate()
                        .child(subtitle.into()),
                ),
        )
        .children(trailing)
}

/// Owner / admin capsule.
pub(super) fn badge(text: &str, color: Hsla) -> Div {
    glass::chip(text.to_string(), color)
}

// ----- content from the chat docs ----------------------------------------------

/// One attachment found in a chat, with where it came from.
pub(super) struct Found {
    pub attachment: Attachment,
    pub chat: String,
    pub when: u64,
    pub thumbnail: Option<Arc<RenderImage>>,
}

/// A link found in a message.
pub(super) struct Link {
    pub url: String,
    pub by: String,
    pub chat: String,
    pub when: u64,
}

pub(super) struct Content {
    pub media: Vec<Found>,
    pub files: Vec<Found>,
    pub voice: Vec<Found>,
    pub links: Vec<Link>,
}

/// Everything shareable in `chats`, newest first. Only loaded docs count.
pub(super) fn collect(state: &AppState, chats: &[&Chat]) -> Content {
    let mut out = Content {
        media: Vec::new(),
        files: Vec::new(),
        voice: Vec::new(),
        links: Vec::new(),
    };
    for chat in chats {
        for m in state.messages_of(&chat.id) {
            let by = sender_label(state, m);
            for part in &m.parts {
                match part {
                    Part::Attachment { attachment, .. } => {
                        let thumbnail = attachment
                            .blob_ref
                            .as_ref()
                            .and_then(|k| state.thumbnails.get(k).cloned());
                        let found = Found {
                            attachment: attachment.clone(),
                            chat: chat.title.clone(),
                            when: m.created_at,
                            thumbnail,
                        };
                        match attachment.kind() {
                            AttachmentKind::Image | AttachmentKind::Video => out.media.push(found),
                            AttachmentKind::Audio => out.voice.push(found),
                            _ => out.files.push(found),
                        }
                    }
                    Part::Text { text, .. } => {
                        for url in urls_in(text) {
                            out.links.push(Link {
                                url,
                                by: by.clone(),
                                chat: chat.title.clone(),
                                when: m.created_at,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    out.media.sort_by_key(|a| std::cmp::Reverse(a.when));
    out.files.sort_by_key(|a| std::cmp::Reverse(a.when));
    out.voice.sort_by_key(|a| std::cmp::Reverse(a.when));
    out.links.sort_by_key(|a| std::cmp::Reverse(a.when));
    out.links.dedup_by(|a, b| a.url == b.url);
    out
}

fn sender_label(state: &AppState, m: &Message) -> String {
    if let Some(h) = m.sender_handle.as_deref().filter(|h| !h.is_empty()) {
        return format!("@{h}");
    }
    if let Some(n) = m.sender_name.as_deref().filter(|n| !n.is_empty()) {
        return n.to_string();
    }
    if state.is_mine(m) {
        return "You".into();
    }
    m.sender
        .strip_prefix("agent:")
        .or_else(|| m.sender.strip_prefix("project:"))
        .and_then(|pid| state.project(pid))
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "Agent".into())
}

/// `http(s)://…` tokens, trailing punctuation trimmed, in order of appearance.
pub(super) fn urls_in(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || c == '<' || c == '>' || c == '(' || c == ')')
        .filter(|t| t.starts_with("http://") || t.starts_with("https://"))
        .map(|t| {
            t.trim_end_matches(['.', ',', ';', ':', '!', '?', '"', '\''])
                .to_string()
        })
        .filter(|t| t.len() > 8)
        .collect()
}

/// "today", "yesterday", "3 days ago"…
pub(super) fn when_text(at: u64) -> String {
    let ago = now_ms().saturating_sub(at);
    if ago < 86_400_000 {
        "today".into()
    } else {
        crate::app::ago_text(ago)
    }
}

fn size_text(bytes: Option<u64>) -> Option<String> {
    let b = bytes?;
    Some(if b < 1024 {
        format!("{b} B")
    } else if b < 1024 * 1024 {
        format!("{} KB", b / 1024)
    } else {
        format!("{:.1} MB", b as f64 / (1024.0 * 1024.0))
    })
}

/// Media tab: a 3-wide grid of thumbnails (photo glyph while loading).
pub(super) fn media_grid(items: &[Found], theme: &Theme) -> Div {
    if items.is_empty() {
        return empty("No photos or videos yet", theme);
    }
    let cell = (SHEET_WIDTH - 32.0 - 2.0 * 4.0) / 3.0;
    let mut grid = div()
        .flex_shrink_0()
        .flex()
        .flex_wrap()
        .gap(px(4.0))
        .px(px(16.0))
        .pb(px(16.0));
    for (i, f) in items.iter().enumerate() {
        let tile = div()
            .id(ElementId::Name(SharedString::from(format!(
                "profile-media-{i}"
            ))))
            .size(px(cell))
            .rounded(px(8.0))
            .overflow_hidden()
            .bg(theme.surface_1)
            .flex()
            .items_center()
            .justify_center();
        grid = grid.child(match &f.thumbnail {
            Some(img) => tile.child(
                gpui::img(img.clone())
                    .size(px(cell))
                    .object_fit(gpui::ObjectFit::Cover),
            ),
            None => tile.child(icon(
                if f.attachment.kind() == AttachmentKind::Video {
                    "video"
                } else {
                    "photo"
                },
                22.0,
                theme.text_faint,
            )),
        });
    }
    grid
}

/// Files / Voice tabs: one row per attachment.
pub(super) fn file_list(items: &[Found], glyph: &'static str, none: &str, theme: &Theme) -> Div {
    if items.is_empty() {
        return empty(none, theme);
    }
    let mut list = card(theme);
    for (i, f) in items.iter().enumerate() {
        let title = f
            .attachment
            .title
            .clone()
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| f.attachment.name.clone());
        let mut sub = vec![when_text(f.when), f.chat.clone()];
        if let Some(size) = size_text(f.attachment.bytes) {
            sub.insert(0, size);
        }
        list = list.child(list_row(
            ElementId::Name(SharedString::from(format!("profile-file-{i}"))),
            div()
                .size(px(40.0))
                .rounded(px(10.0))
                .bg(theme.surface_2)
                .flex()
                .items_center()
                .justify_center()
                .child(icon(glyph, 18.0, theme.primary))
                .into_any_element(),
            title,
            sub.join(" · "),
            theme.text_muted,
            None,
            theme,
        ));
    }
    list
}

/// Links tab: favicon-less rows like Telegram's — a letter tile, the URL, who shared it.
pub(super) fn link_list(items: &[Link], theme: &Theme, cx: &mut Context<Shell>) -> Div {
    if items.is_empty() {
        return empty("No links shared yet", theme);
    }
    let mut list = card(theme);
    for (i, l) in items.iter().enumerate() {
        let host = l
            .url
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap_or("")
            .trim_start_matches("www.")
            .to_string();
        let letter = host
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "#".into());
        let url = l.url.clone();
        list = list.child(
            list_row(
                ElementId::Name(SharedString::from(format!("profile-link-{i}"))),
                div()
                    .size(px(40.0))
                    .rounded(px(10.0))
                    .bg(crate::ui::groups::sender_color(&host, theme))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(TEXT_BODY + 2.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child(letter)
                    .into_any_element(),
                host,
                format!("{} · {} · {}", l.by, l.chat, when_text(l.when)),
                theme.text_muted,
                Some(
                    div()
                        .max_w(px(150.0))
                        .text_size(px(TEXT_TINY))
                        .text_color(theme.primary)
                        .truncate()
                        .child(l.url.clone())
                        .into_any_element(),
                ),
                theme,
            )
            .cursor_pointer()
            .hover(|s| s.bg(theme.surface_2))
            .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&url))),
        );
    }
    list
}

/// The chats of a space, for content tabs (one per agent unless multi-chat is on).
pub(super) fn chats_of<'a>(state: &'a AppState, space_id: &str) -> Vec<&'a Chat> {
    state.visible_tabs(space_id)
}

/// A plain pill for the "more" menu.
pub(super) fn more_menu(rows: Vec<Stateful<Div>>, theme: &Theme) -> Div {
    div()
        .flex_shrink_0()
        .mx(px(16.0))
        .mb(px(14.0))
        .child(glass::menu_card(SHEET_WIDTH - 32.0, theme).children(rows))
}
