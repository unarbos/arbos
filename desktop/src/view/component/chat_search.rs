//! Search chats (⌘K): one palette over every open tab's chats — main chats,
//! sub-agents, your own side-threads — by title and first words. Cursor's
//! sidebar had Search over its time buckets; with projects as tabs the
//! search is the one place that reaches across them.

use bezel::{
    gpui::{
        AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Hsla, MouseButton,
        Render, SharedString, Window, div, prelude::*, px,
    },
    ui::palette::{CommandPalette, PaletteEvent},
};

/// One chat the palette can open: which tab, which session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub project: usize,
    pub session: u64,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatSearchEvent {
    /// Open this chat: switch to its tab and select it.
    Open { project: usize, session: u64 },
    Dismiss,
}

pub struct ChatSearch {
    palette: Option<Entity<CommandPalette>>,
    hits: Vec<Hit>,
    focus: FocusHandle,
}

impl EventEmitter<ChatSearchEvent> for ChatSearch {}

impl ChatSearch {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            palette: None,
            hits: Vec::new(),
            focus: cx.focus_handle(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.palette.is_some()
    }

    /// Put the palette up over `hits`, newest first as the caller ordered
    /// them, and give it the keyboard.
    pub fn show(&mut self, hits: Vec<Hit>, window: &mut Window, cx: &mut Context<Self>) {
        let items: Vec<SharedString> = hits
            .iter()
            .map(|hit| SharedString::from(hit.label.clone()))
            .collect();
        let palette = cx.new(|cx| {
            let mut palette = CommandPalette::new(items, cx);
            palette.set_placeholder("Search chats…", cx);
            palette
        });
        cx.subscribe(&palette, |this, _, event: &PaletteEvent, cx| match event {
            PaletteEvent::Selected(ix) => {
                if let Some(hit) = this.hits.get(*ix) {
                    let (project, session) = (hit.project, hit.session);
                    this.close(cx);
                    cx.emit(ChatSearchEvent::Open { project, session });
                }
            }
            PaletteEvent::Dismissed => {
                this.close(cx);
                cx.emit(ChatSearchEvent::Dismiss);
            }
        })
        .detach();
        palette.update(cx, |palette, cx| palette.focus(window, cx));
        self.hits = hits;
        self.palette = Some(palette);
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.palette = None;
        self.hits.clear();
        cx.notify();
    }
}

impl Focusable for ChatSearch {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ChatSearch {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(palette) = self.palette.clone() else {
            return div().into_any_element();
        };
        // A click outside the card closes it, as the tab sheet's does; the
        // card itself keeps the press.
        div()
            .id("chat-search")
            .absolute()
            .inset_0()
            .flex()
            .items_start()
            .justify_center()
            .pt(px(96.))
            .bg(Hsla {
                h: 0.,
                s: 0.,
                l: 0.,
                a: 0.25,
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close(cx);
                    cx.emit(ChatSearchEvent::Dismiss);
                }),
            )
            .child(
                div()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(palette),
            )
            .into_any_element()
    }
}
