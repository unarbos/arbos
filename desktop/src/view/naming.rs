//! Naming and the chat menu: the one text field every rename shares, the
//! `⋯` menu a chat opens, and what Delete does to the highlighted chat.
//! These used to live with the sidebar; the tab bar and the panel both
//! reach them now.

use crate::view::{
    component::menu::{self, Menu},
    root::{Arbos, CommitName, DismissName},
};
use bezel::{
    gpui::{
        AnyElement, App, Context, Focusable as _, MouseButton, Pixels, SharedString, TextRun,
        Window, div, font, prelude::*, px,
    },
    theme::{TextStyle, Theme, Typeset},
    ui::{icons, input, menu::Item, popover},
};

/// What the name field is attached to. One field, because only one chat
/// can be being named at a time. Held by the session's id, never by a
/// position: that moves the moment a neighbour is made or dropped, and
/// the field would follow it onto whichever chat slid underneath.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Renaming {
    Session(u64),
}

impl Arbos {
    /// The chat's menu, anchored below whatever holds it.
    pub(crate) fn session_menu_element(
        &self,
        id: u64,
        archived: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.menu != Some(Menu::Session(id)) {
            return None;
        }
        let archive_label = if archived { "Unarchive" } else { "Archive" };
        let archive_icon = if archived {
            icons::files::ARCHIVE_UP_MINIMALISTIC
        } else {
            icons::files::ARCHIVE_MINIMALISTIC
        };
        let rows = vec![
            menu::row(
                Item::action("Copy").with_icon(icons::files::COPY),
                move |this, _, cx| {
                    this.workspace
                        .update(cx, |workspace, cx| workspace.copy_chat_link(id, cx));
                },
            ),
            menu::row(
                Item::action("Fork").with_icon(icons::editing::GIT_BRANCH),
                move |this, _, cx| {
                    this.workspace
                        .update(cx, |workspace, cx| workspace.fork_session(id, cx));
                },
            ),
            menu::row(
                Item::action(archive_label).with_icon(archive_icon),
                move |this, _, cx| {
                    this.workspace.update(cx, |workspace, cx| {
                        workspace.archive_session(id, !archived, cx);
                    });
                },
            ),
            menu::row(
                Item::action("Delete")
                    .with_icon(icons::files::TRASH_BIN_MINIMALISTIC)
                    .with_keystroke("⌫"),
                move |this, _, cx| this.delete_or_archive(id, cx),
            ),
        ];
        let key = SharedString::from(format!("session-menu-{id}"));
        Some(popover::anchored_menu_below(
            key.clone(),
            self.menu_card(key, rows, cx),
            None,
        ))
    }

    /// Open chat: put it in the archive. Archived chat: remove it.
    /// Then light the next row in that same list so Delete can fire again.
    pub(crate) fn delete_or_archive(&mut self, id: u64, cx: &mut Context<Self>) {
        let next = self.neighbor_after_remove(id, cx);
        let archived = self
            .workspace
            .read(cx)
            .session(id)
            .is_some_and(|chat| chat.closed);
        self.workspace.update(cx, |workspace, cx| {
            if archived {
                workspace.delete_session(id, cx);
            } else {
                workspace.archive_session(id, true, cx);
            }
        });
        if let Some(next) = next {
            self.select_session(next, cx);
        }
    }

    /// Who should be in front after `id` leaves its section. Next row
    /// in the same list, or the one above if it was last. Archiving the
    /// last open chat keeps that chat — it is now the archived row.
    /// Deleting the last archived chat falls back to the last open one.
    fn neighbor_after_remove(&self, id: u64, cx: &App) -> Option<u64> {
        let workspace = self.workspace.read(cx);
        let project = workspace
            .projects
            .iter()
            .find(|project| project.session(id).is_some())?;
        let chat = project.session(id)?;
        let closed = chat.closed;
        let nested = !project.is_root(chat);
        let parent = chat.parent;
        let mut ids: Vec<(i64, u64)> = project
            .sessions
            .iter()
            .filter(|sibling| {
                sibling.closed == closed
                    && if nested {
                        sibling.parent == parent
                    } else {
                        project.is_root(sibling)
                    }
            })
            .map(|sibling| (sibling.rank, sibling.id))
            .collect();
        ids.sort_by_key(|key| *key);
        let ids: Vec<u64> = ids.into_iter().map(|(_, sid)| sid).collect();
        let at = ids.iter().position(|&sid| sid == id)?;
        if let Some(&below) = ids.get(at + 1) {
            return Some(below);
        }
        if at > 0 {
            return Some(ids[at - 1]);
        }
        if nested {
            return parent.filter(|&parent| project.session(parent).is_some());
        }
        if !closed {
            return None;
        }
        let mut open: Vec<(i64, u64)> = project
            .roots()
            .filter(|chat| !chat.closed)
            .map(|chat| (chat.rank, chat.id))
            .collect();
        open.sort_by_key(|key| *key);
        open.last().map(|(_, sid)| *sid)
    }

    pub(crate) fn rename_empty_title(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_heading = true;
        self.start_rename(Renaming::Session(id), true, window, cx);
        // The heading is painted under the activating double-click. That
        // press reaches the new field and parks a caret in the word, so
        // the same-tick SelectAll in `start_rename` loses. Frame callbacks
        // run before the draw, so the first one still sees a tree without
        // the field and its SelectAll would miss too: let that frame paint
        // the field, then select on the next one and drop the flag.
        cx.on_next_frame(window, |_, window, cx| {
            cx.on_next_frame(window, |this, window, cx| {
                this.select_name_field(window, cx);
                this.select_heading = false;
            });
        });
    }

    /// Rename from the chat header: the same field, Body-sized, in the
    /// title's place on the header line. Reached by a double-click on the
    /// title, like the empty-chat heading.
    #[allow(dead_code)]
    pub(crate) fn rename_header_title(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_heading = true;
        self.start_rename(Renaming::Session(id), false, window, cx);
        self.rename_in_header = true;
        // Same two-frame wait as `rename_empty_title`: the activating press
        // lands on the new field first and would park a caret in the word.
        cx.on_next_frame(window, |_, window, cx| {
            cx.on_next_frame(window, |this, window, cx| {
                this.select_name_field(window, cx);
                this.select_heading = false;
            });
        });
    }

    /// Same field as the chat header's title: Body, hugging the text, so
    /// the header line does not shift when editing starts.
    #[allow(dead_code)]
    pub(crate) fn header_name_field(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let text = self.name_field.read(cx).content().to_string();
        let width = name_width(&text, TextStyle::Body, window, cx);
        self.name_field_frame(TextStyle::Body, px(18.), width, cx)
    }

    /// Same field, set as the empty-chat title: Title3, hugging the text,
    /// so the heading does not jump left or drop a size.
    pub(crate) fn heading_name_field(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let text = self.name_field.read(cx).content().to_string();
        let width = name_width(&text, TextStyle::Title3, window, cx);
        self.name_field_frame(
            TextStyle::Title3,
            px(TextStyle::Title3.line_height()),
            width,
            cx,
        )
    }

    /// The field, hugging its text: it carries its own press, because
    /// `TextField` does not focus itself, and a press that reached the
    /// title behind it would open what is being named out from under it.
    fn name_field_frame(
        &self,
        style: TextStyle,
        line: Pixels,
        width: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .w(width)
            .max_w_full()
            .flex()
            .items_center()
            .text_style(style)
            .line_height(line)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    window.focus(&this.name_field.read(cx).focus_handle(cx), cx);
                    if this.select_heading {
                        this.select_heading = false;
                        window.dispatch_action(Box::new(input::SelectAll), cx);
                    }
                }),
            )
            // Pressing anywhere else is finishing, not abandoning — the name
            // typed is the name meant. `escape` is what discards.
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                this.commit_name(&CommitName, window, cx);
            }))
            .child(self.name_field.clone())
            .into_any_element()
    }

    fn start_rename(
        &mut self,
        what: Renaming,
        heading: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let label = match &what {
            Renaming::Session(id) => self.workspace.read(cx).display_label(*id),
        };
        self.bind_name_field(heading, cx);
        self.rename_in_header = false;
        self.name_field
            .update(cx, |field, cx| field.set_content(label, cx));
        self.renaming = Some(what);
        window.focus(&self.name_field.read(cx).focus_handle(cx), cx);
        // `set_content` parks the caret at the end. Select after the
        // field is focused and in the tree, or the action misses it.
        cx.spawn_in(window, async move |this, cx| {
            let _ = this.update_in(cx, |this, window, cx| {
                this.select_name_field(window, cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn select_name_field(&self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.name_field.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        window.dispatch_action(Box::new(input::SelectAll), cx);
    }

    pub(crate) fn commit_name(&mut self, _: &CommitName, _: &mut Window, cx: &mut Context<Self>) {
        let Some(what) = self.renaming.take() else {
            return;
        };
        self.rename_heading = false;
        self.rename_in_header = false;
        self.select_heading = false;
        let name = self.name_field.read(cx).content().to_string();
        self.workspace.update(cx, |workspace, cx| match what {
            Renaming::Session(id) => workspace.rename_session(id, name, cx),
        });
        cx.notify();
    }

    pub(crate) fn dismiss_name(&mut self, _: &DismissName, _: &mut Window, cx: &mut Context<Self>) {
        self.renaming = None;
        self.rename_heading = false;
        self.rename_in_header = false;
        self.select_heading = false;
        cx.notify();
    }
}

/// Width of the empty-chat title, plus a sliver for the caret, so the
/// field stays where the idle heading sat.
fn name_width(text: &str, style: TextStyle, window: &mut Window, cx: &App) -> Pixels {
    let theme = Theme::of(cx);
    let shown = if text.is_empty() { " " } else { text };
    let size = px(style.painted());
    let run = TextRun {
        len: shown.len(),
        font: font(theme.font_sans.clone()),
        color: theme.text,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(shown.to_string().into(), size, &[run], None)
        .width()
        + px(2.)
}
