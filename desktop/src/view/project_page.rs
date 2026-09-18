//! The project page as a reading-size layout. The window no longer puts
//! this over the chat — the page lives in the right panel.
#![allow(dead_code)]

use crate::{
    model::{store_view::StoreFile, workspace::Workspace},
    view::{
        panel::{PageScale, age, file_glyph, page_heading},
        root::{self, Arbos, ShowChat},
    },
};
use bezel::{
    gpui::{AnyElement, FontWeight, SharedString, Window, div, prelude::*, px},
    theme::{TextStyle, Theme, Typeset},
    ui::{icons, tooltip::Tooltip, widgets::Buttons},
};
use std::cmp::Reverse;
use std::time::SystemTime;

/// The page's reading column. Cursor's Project home is a document, not a
/// card grid: a little wider than chat, with room to breathe.
const PAGE_MAX_WIDTH: f32 = 720.;

/// The page's own gutter, and the space between its parts.
const PAGE_GUTTER: f32 = 48.;
const PART_GAP: f32 = 40.;

/// How many recent chats the page lists.
const RECENTS_CAP: usize = 8;

impl Arbos {
    /// The page for the project in front.
    pub(crate) fn project_view(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let workspace = self.workspace.read(cx);
        let Some(project) = workspace.active_project() else {
            return div().flex_1().into_any_element();
        };
        let name = Workspace::tab_label(project);
        let (glyph, tint) = (project.identity.glyph(), project.identity.hsla());
        let where_ = project.place().encode();
        let remote = project.is_remote();
        let store = project.store_view.clone();
        let notes_path = store.page.as_ref().map(|page| page.path.clone());

        let mut recents: Vec<(u64, String, SystemTime)> = project
            .sessions
            .iter()
            .filter(|chat| !chat.closed)
            .map(|chat| (chat.id, workspace.display_label(chat.id), chat.updated))
            .collect();
        recents.sort_by_key(|row| Reverse(row.2));
        recents.truncate(RECENTS_CAP);

        let head = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_start()
            .gap(px(16.))
            .pt(px(20.))
            .child(
                div()
                    .flex_none()
                    .size(px(36.))
                    .rounded(px(9.))
                    .bg(theme.element_hover)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icons::icon(glyph).size(px(18.)).text_color(tint)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .truncate()
                            .text_style(TextStyle::Title2)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .child(SharedString::from(name)),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_style(TextStyle::Caption)
                            .text_color(theme.text_faint)
                            .child(SharedString::from(where_)),
                    ),
            )
            .children(notes_path.map(|path| {
                let title = "notes.md".to_string();
                theme
                    .ghost("page-open-notes")
                    .flex_none()
                    .size(px(26.))
                    .items_center()
                    .justify_center()
                    .tooltip(|window, cx| Tooltip::text("Open notes.md as a document", window, cx))
                    .child(
                        icons::icon(icons::files::DOCUMENT)
                            .size(px(14.))
                            .text_color(theme.text_muted),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_store_file(path.clone(), &title, cx);
                    }))
            }))
            // The way out, in words: Jacob could not find the lone chat
            // glyph. Escape and ⌘1 do the same; so does the tab.
            .child(
                theme
                    .ghost("page-back-to-chat")
                    .flex_none()
                    .h(px(26.))
                    .px(px(8.))
                    .gap(px(6.))
                    .items_center()
                    .justify_center()
                    .tooltip(|window, cx| {
                        Tooltip::with_keystroke("Back to the chat (Esc)", "⌘1", window, cx)
                    })
                    .child(
                        icons::icon(icons::system::CHAT_ROUND_LINE)
                            .size(px(14.))
                            .text_color(theme.text_muted),
                    )
                    .child(
                        div()
                            .text_style(TextStyle::Callout)
                            .text_color(theme.text_muted)
                            .child("Back to chat"),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.show_chat(&ShowChat, window, cx);
                    })),
            );

        let recents_block = (!recents.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .child(page_heading(2, "Recents", false, PageScale::Page, &theme))
                .children(
                    recents
                        .into_iter()
                        .enumerate()
                        .map(|(n, (id, title, at))| {
                            self.recent_row(n as u64, id, title, at, &theme, cx)
                        }),
                )
        });

        let page = self.project_page(store.page.as_ref(), remote, PageScale::Page, &theme, cx);

        let files = (!store.files.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .child(page_heading(2, "Files", true, PageScale::Page, &theme))
                .children(
                    store
                        .files
                        .iter()
                        .enumerate()
                        .map(|(n, file)| self.page_file_row(n as u64, file, &theme, cx)),
                )
        });

        // The context document, rendered; while it is still the template,
        // a line that says so — the file's own hints are not content.
        let context = store.context.is_some().then(|| {
            div()
                .flex()
                .flex_col()
                .child(page_heading(2, "Context", true, PageScale::Page, &theme))
                .child(match store.context_text.as_deref() {
                    Some(text) => div()
                        .pt(px(4.))
                        .text_style(TextStyle::Body)
                        .text_color(theme.text)
                        .child(markdown::markdown(text, window, cx))
                        .into_any_element(),
                    None => div()
                        .pt(px(4.))
                        .text_style(TextStyle::Body)
                        .text_color(theme.text_faint)
                        .child("Nothing in the context document yet.")
                        .into_any_element(),
                })
        });

        div()
            .id("project-page")
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_y_scroll()
            .flex()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(PAGE_MAX_WIDTH))
                    .px(px(PAGE_GUTTER))
                    .pt(px(root::HEADER_HEIGHT))
                    .pb(px(80.))
                    .flex()
                    .flex_col()
                    .gap(px(PART_GAP))
                    .child(head)
                    .children(recents_block)
                    .child(page)
                    .children(files)
                    .children(context),
            )
            .into_any_element()
    }

    /// One recent chat: its name, how long ago it last spoke. A click
    /// opens that chat in the column.
    fn recent_row(
        &self,
        n: u64,
        id: u64,
        title: String,
        at: SystemTime,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(("page-recent", n))
            .flex_none()
            .h(px(32.))
            .px(px(4.))
            .rounded(px(6.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .cursor_pointer()
            .hover(|el| el.bg(theme.element_hover))
            .child(
                icons::icon(icons::system::CHAT_ROUND_LINE)
                    .size(px(14.))
                    .flex_none()
                    .text_color(theme.text_muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_style(TextStyle::Body)
                    .text_color(theme.text)
                    .child(SharedString::from(title)),
            )
            .child(
                div()
                    .flex_none()
                    .text_style(TextStyle::Caption)
                    .text_color(theme.text_faint)
                    .child(SharedString::from(age(at))),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.select_session(id, cx);
            }))
            .into_any_element()
    }

    /// One file of the store as a list row: its glyph, its name, where it
    /// sits and how long ago it changed, dim. The context document leads.
    fn page_file_row(
        &self,
        n: u64,
        file: &StoreFile,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let path = file.path.clone();
        let title = file.name.clone();
        let sub = match (file.pinned, file.folder.is_empty(), file.modified) {
            (true, _, _) => "the project's context".to_string(),
            (false, false, Some(at)) => format!("{} · {}", file.folder, age(at)),
            (false, true, Some(at)) => age(at),
            (false, false, None) => file.folder.clone(),
            (false, true, None) => String::new(),
        };
        div()
            .id(("page-file", n))
            .flex_none()
            .h(px(32.))
            .px(px(4.))
            .rounded(px(6.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .cursor_pointer()
            .hover(|el| el.bg(theme.element_hover))
            .child(
                icons::icon(file_glyph(file.kind))
                    .size(px(14.))
                    .flex_none()
                    .text_color(if file.pinned {
                        theme.accent
                    } else {
                        theme.text_muted
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_style(TextStyle::Body)
                    .text_color(theme.text)
                    .child(SharedString::from(if file.pinned {
                        "Context".to_string()
                    } else {
                        file.name.clone()
                    })),
            )
            .when(!sub.is_empty(), |el| {
                el.child(
                    div()
                        .flex_none()
                        .truncate()
                        .text_style(TextStyle::Caption)
                        .text_color(theme.text_faint)
                        .child(SharedString::from(sub)),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_store_file(path.clone(), &title, cx);
            }))
            .into_any_element()
    }
}
