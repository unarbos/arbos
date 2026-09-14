//! The project page, full width in the column: Cursor's Project tab. The
//! project's face and name on top; the status page at reading size, every
//! label a link; the store's files as a grid, each a click from view; the
//! context document rendered under it. Everything on it is read off the
//! model, which the watch and the kernel's `changed` frames keep current.

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

/// The page's reading column. Cursor's Project page runs a little wider
/// than a chat: the grid wants the room.
const PAGE_MAX_WIDTH: f32 = 760.;

/// The page's own gutter, and the space between its parts.
const PAGE_GUTTER: f32 = 36.;
const PART_GAP: f32 = 28.;

/// A file card's size in the grid.
const CARD_WIDTH: f32 = 216.;
const CARD_HEIGHT: f32 = 64.;

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

        let head = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.))
            .child(
                div()
                    .flex_none()
                    .size(px(28.))
                    .rounded(px(7.))
                    .bg(theme.element_hover)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icons::icon(glyph).size(px(16.)).text_color(tint)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
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
            .child(
                theme
                    .ghost("page-back-to-chat")
                    .flex_none()
                    .size(px(26.))
                    .items_center()
                    .justify_center()
                    .tooltip(|window, cx| {
                        Tooltip::with_keystroke("Back to the chat", "⌘1", window, cx)
                    })
                    .child(
                        icons::icon(icons::system::CHAT_ROUND_LINE)
                            .size(px(14.))
                            .text_color(theme.text_muted),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.show_chat(&ShowChat, window, cx);
                    })),
            );

        let page = self.project_page(store.page.as_ref(), remote, PageScale::Page, &theme, cx);

        let files = (!store.files.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .child(page_heading(2, "Files", false, PageScale::Page, &theme))
                .child(
                    div().flex().flex_row().flex_wrap().gap(px(10.)).children(
                        store
                            .files
                            .iter()
                            .enumerate()
                            .map(|(n, file)| self.file_card(n as u64, file, &theme, cx)),
                    ),
                )
        });

        // The context document, rendered; while it is still the template,
        // a line that says so — the file's own hints are not content.
        let context = store.context.is_some().then(|| {
            div()
                .flex()
                .flex_col()
                .child(page_heading(2, "Context", false, PageScale::Page, &theme))
                .child(match store.context_text.as_deref() {
                    Some(text) => div()
                        .text_style(TextStyle::Body)
                        .text_color(theme.text)
                        .child(markdown::markdown(text, window, cx))
                        .into_any_element(),
                    None => div()
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
                    .pb(px(64.))
                    .flex()
                    .flex_col()
                    .gap(px(PART_GAP))
                    .child(head)
                    .child(page)
                    .children(files)
                    .children(context),
            )
            .into_any_element()
    }

    /// One file of the store as a card: its glyph, its name, where it sits
    /// and how long ago it changed, dim. The context document leads.
    fn file_card(
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
            .w(px(CARD_WIDTH))
            .h(px(CARD_HEIGHT))
            .px(px(12.))
            .rounded(px(8.))
            .border_1()
            .border_color(theme.border)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .cursor_pointer()
            .hover(|el| el.bg(theme.element_hover).border_color(theme.border_strong))
            .child(
                icons::icon(file_glyph(file.kind))
                    .size(px(16.))
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
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .text_style(TextStyle::Callout)
                            .font_weight(FontWeight::MEDIUM)
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
                                .truncate()
                                .text_style(TextStyle::Caption)
                                .text_color(theme.text_faint)
                                .child(SharedString::from(sub)),
                        )
                    }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_store_file(path.clone(), &title, cx);
            }))
            .into_any_element()
    }
}
