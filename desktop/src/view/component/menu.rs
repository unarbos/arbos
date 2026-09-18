//! The `+` menu a project heading opens, and the field that says which menu
//! is showing.

use crate::view::root::Arbos;
use bezel::{
    gpui::{AnyElement, Context, Div, SharedString, Stateful, Window, prelude::*},
    theme::Theme,
    ui::menu::{self, Hit, Item},
};

/// What a row does when it is picked.
type Act = Box<dyn Fn(&mut Arbos, &mut Window, &mut Context<Arbos>)>;

/// Which menu is open. One field rather than a flag each, so opening one
/// closes the rest by construction.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Menu {
    /// A chat row: copy, fork, archive, delete.
    Session(u64),
    /// A project tab: edit its face, close it.
    Tab(usize),
    /// The drawer's `+`: files, a terminal, a browser, the project tab,
    /// or an empty tab. A click used to open a tab with no ask; the menu
    /// is the ask.
    PanelNew,
}

/// One row of a menu, and what picking it does.
pub(crate) fn row(
    item: Item,
    act: impl Fn(&mut Arbos, &mut Window, &mut Context<Arbos>) + 'static,
) -> (Item, Act) {
    (item, Box::new(act))
}

impl Arbos {
    /// Open a menu, or shut the one already open.
    ///
    /// The press that reaches a trigger is the same press the open card
    /// dismisses on, so by click time the menu already reads as shut and a
    /// plain toggle would open it straight back. What the press found is noted
    /// by [`Arbos::menu_press`] instead, in the capture phase — ahead of
    /// that handler, whichever element owns it.
    pub(crate) fn toggle_menu(&mut self, menu: Menu, cx: &mut Context<Self>) {
        let closed_by_this_press = std::mem::take(&mut self.menu_pressed);
        self.menu = (!closed_by_this_press && self.menu != Some(menu)).then_some(menu);
        self.menu_cursor.clear();
        self.menu_at_header = false;
        cx.notify();
    }

    /// The same menu, opened from the chat header: it anchors under the
    /// header's `⋯` instead of beside the panel row.
    pub(crate) fn toggle_menu_at_header(&mut self, menu: Menu, cx: &mut Context<Self>) {
        self.toggle_menu(menu, cx);
        self.menu_at_header = self.menu.is_some();
    }

    /// Shut whichever menu is open, and forget the row it was on.
    pub(crate) fn shut_menu(&mut self) {
        self.menu = None;
        self.menu_cursor.clear();
        self.menu_at_header = false;
    }

    /// Note, on the way down, whether the press landed on the trigger of the
    /// menu that is open. Every trigger claims this, so the note is written
    /// afresh on each press and can never be read stale.
    pub(crate) fn menu_press(
        &self,
        el: Stateful<Div>,
        menu: Menu,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        el.capture_any_mouse_down(cx.listener(move |this, _, _, _| {
            this.menu_pressed = this.menu == Some(menu);
        }))
    }

    /// The card every menu hangs in, dismissed by a press outside it —
    /// which the card reports itself, since with a panel open only the tree
    /// knows which presses landed on none of it.
    pub(crate) fn menu_card(
        &self,
        id: impl Into<SharedString>,
        rows: Vec<(Item, Act)>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let (items, acts): (Vec<Item>, Vec<Act>) = rows.into_iter().unzip();
        // A hit names a row by its path, and reading one back means holding
        // the list it was built from.
        let paths = items.clone();
        menu::card(
            &theme,
            id,
            &items,
            &self.menu_cursor,
            cx,
            move |this, hit, window, cx| match hit {
                Hit::Point(path) => {
                    if this.menu_cursor.point_at(&paths, &path) {
                        cx.notify();
                    }
                }
                Hit::Choose(path) => {
                    let [row] = path[..] else { return };
                    this.shut_menu();
                    if let Some(act) = acts.get(row) {
                        act(this, window, cx);
                    }
                    cx.notify();
                }
                Hit::Dismiss => {
                    this.shut_menu();
                    cx.notify();
                }
            },
        )
        .into_any_element()
    }
}
