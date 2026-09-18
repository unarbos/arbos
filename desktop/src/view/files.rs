//! Browse files: the project's local directory as a tree.

use crate::{
    model::file_tree::{self, FileRow},
    view::root::Arbos,
};
use bezel::{
    gpui::{AnyElement, ClickEvent, Context, SharedString, Window, div, prelude::*, px},
    theme::{TextStyle, Theme, Typeset},
    ui::{icons, tree},
};
use std::path::Path;

impl Arbos {
    /// The project's folder as a VS Code-style tree: folders, expand and
    /// collapse, the disk. A click on a file opens it as an editor.
    pub(crate) fn file_tree_body(
        &self,
        root: &Path,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let expanded = self
            .workspace
            .read(cx)
            .active_project()
            .map(|project| project.file_expanded.clone())
            .unwrap_or_default();
        let rows = file_tree::visible(root, &expanded);
        let folder = root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(".")
            .to_owned();
        div()
            .id("file-tree")
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .px(px(12.))
                    .py(px(8.))
                    .text_style(TextStyle::Caption)
                    .text_color(theme.text_faint)
                    .child(SharedString::from(folder)),
            )
            .child(
                div()
                    .id("file-tree-rows")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(6.))
                    .pb(px(16.))
                    .child(if rows.is_empty() {
                        div()
                            .px(px(8.))
                            .py(px(12.))
                            .text_style(TextStyle::Callout)
                            .text_color(theme.text_faint)
                            .child("Empty folder.")
                            .into_any_element()
                    } else {
                        tree::tree()
                            .key_context(tree::KEY_CONTEXT)
                            .children(
                                rows.into_iter()
                                    .enumerate()
                                    .map(|(index, row)| self.file_tree_row(index, row, &theme, cx)),
                            )
                            .into_any_element()
                    }),
            )
            .into_any_element()
    }

    fn file_tree_row(
        &self,
        index: usize,
        row: FileRow,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let path = row.path.clone();
        let dir = row.dir;
        let name = row.name.clone();
        let glyph = if dir {
            icons::files::FOLDER
        } else {
            icons::files::DOCUMENT
        };
        tree::tree_row(
            theme,
            &tree::Row {
                depth: row.depth,
                expanded: row.expanded,
            },
            false,
            false,
        )
        .id(("file-tree-row", index))
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            if dir {
                this.workspace.update(cx, |workspace, cx| {
                    if let Some(project) = workspace.active_project_mut() {
                        file_tree::toggle(&mut project.file_expanded, &path);
                        cx.notify();
                    }
                });
            } else {
                let title = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                this.open_store_file(path.clone(), &title, cx);
            }
        }))
        .child(
            icons::icon(glyph)
                .size(px(13.))
                .flex_none()
                .text_color(theme.text_muted),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .pl(px(6.))
                .child(SharedString::from(name)),
        )
        .into_any_element()
    }
}
