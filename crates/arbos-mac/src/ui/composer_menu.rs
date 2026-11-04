//! The chat field's secondary-click menu — **Undo, Redo · Cut, Copy, Paste ·
//! Select All** ([feedback composer-context-menu-mac-2026-11-03]: Jacob,
//! "option click in Arbos chat does not pull up options at all for
//! pasting/undo/cut/copy etc").
//!
//! The composer is a hand-rolled GPUI input (`composer::ComposerInput`), not
//! an `NSTextView`, so macOS has no menu of its own to put there. This is
//! that menu, built from the same items, order and separators as the app's
//! own Edit menu in the menu bar (`main.rs`), with the same keyboard
//! equivalents beside them.
//!
//! Every row runs the field's own editing method — the very method the
//! keyboard shortcut runs — so the menu and ⌘X / ⌘C / ⌘V / ⌘Z / ⌘A can never
//! drift apart. Paste in particular calls `paste_clipboard`, which is also
//! what ⌘V calls, so a copied picture is staged as an attachment from either
//! path.
//!
//! Placement, frame and rows are the bubble menu's (`message_menu::place`,
//! `glass::frosted_menu`); only the row shape differs, because a system text
//! menu draws no icons and does show its shortcuts.

use gpui::{div, prelude::*, px, Div, Entity, Pixels, Point, Size, Window};

use crate::ui::composer::{ComposerInput, EditState};
use crate::ui::glass::{
    self, MENU_CARD_BORDER, MENU_CARD_PADDING, MENU_DIVIDER_HEIGHT, MENU_GAP, MENU_ROW_HEIGHT,
};
use crate::ui::message_menu::place;
use crate::ui::shell::Shell;
use crate::ui::theme::Theme;

/// Wide enough for "Select All" and its ⌘A together, at the menu's type size.
pub const COMPOSER_MENU_WIDTH: f32 = 196.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposerMenuItem {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

impl ComposerMenuItem {
    pub fn label(self) -> &'static str {
        match self {
            ComposerMenuItem::Undo => "Undo",
            ComposerMenuItem::Redo => "Redo",
            ComposerMenuItem::Cut => "Cut",
            ComposerMenuItem::Copy => "Copy",
            ComposerMenuItem::Paste => "Paste",
            ComposerMenuItem::SelectAll => "Select All",
        }
    }

    /// The keyboard equivalent as AppKit writes it: modifiers in ⌃⌥⇧⌘ order,
    /// then the key. These are the bindings `main.rs` registers for the
    /// `ComposerInput` key context.
    pub fn shortcut(self) -> &'static str {
        match self {
            ComposerMenuItem::Undo => "⌘Z",
            ComposerMenuItem::Redo => "⇧⌘Z",
            ComposerMenuItem::Cut => "⌘X",
            ComposerMenuItem::Copy => "⌘C",
            ComposerMenuItem::Paste => "⌘V",
            ComposerMenuItem::SelectAll => "⌘A",
        }
    }

    /// A separator is drawn above this item, as in the Edit menu: one under
    /// Redo, one under Paste.
    pub fn starts_group(self) -> bool {
        matches!(self, ComposerMenuItem::Cut | ComposerMenuItem::SelectAll)
    }

    /// Live, or greyed like a validated `NSMenuItem` that cannot act.
    pub fn enabled(self, state: EditState) -> bool {
        match self {
            ComposerMenuItem::Undo => state.can_undo,
            ComposerMenuItem::Redo => state.can_redo,
            // Nothing picked, nothing to take; a masked field gives up
            // neither (`has_selection` is already false there).
            ComposerMenuItem::Cut | ComposerMenuItem::Copy => state.has_selection,
            ComposerMenuItem::Paste => state.pasteboard_has_content,
            ComposerMenuItem::SelectAll => state.has_text && !state.all_selected,
        }
    }
}

/// The menu's items, in order. The app's Edit menu, the same list.
pub const ITEMS: [ComposerMenuItem; 6] = [
    ComposerMenuItem::Undo,
    ComposerMenuItem::Redo,
    ComposerMenuItem::Cut,
    ComposerMenuItem::Copy,
    ComposerMenuItem::Paste,
    ComposerMenuItem::SelectAll,
];

/// An open menu: which field it belongs to, where it was asked for, and what
/// that field could do at that moment.
#[derive(Clone)]
pub struct ComposerMenu {
    pub input: Entity<ComposerInput>,
    pub at: Point<Pixels>,
    pub state: EditState,
}

/// The menu's size, from the constants `glass` lays its card out with — the
/// same arithmetic as `message_menu::menu_size_for`, with this menu's row
/// count and its two separators.
pub fn menu_size() -> Size<Pixels> {
    let rows = ITEMS.len() as f32;
    let dividers = ITEMS.iter().filter(|i| i.starts_group()).count() as f32;
    let children = rows + dividers;
    let height = 2.0 * (MENU_CARD_PADDING + MENU_CARD_BORDER)
        + rows * MENU_ROW_HEIGHT
        + dividers * MENU_DIVIDER_HEIGHT
        + (children - 1.0) * MENU_GAP;
    gpui::size(px(COMPOSER_MENU_WIDTH), px(height))
}

/// `ARBOS_RIG_COMPOSER_MENU=<x>,<y>`: open the chat field's menu at that
/// window position at launch (screenshot and recording rigs under Xvfb, and
/// the EC2 Mac's GUI session, where posted pointer events never reach the
/// app). Mirrors `message_menu::rig_anchor`.
pub fn rig_anchor() -> Option<Point<Pixels>> {
    let spec = std::env::var("ARBOS_RIG_COMPOSER_MENU").ok()?;
    let (x, y) = spec.split_once(',')?;
    Some(gpui::point(
        px(x.trim().parse::<f32>().ok()?),
        px(y.trim().parse::<f32>().ok()?),
    ))
}

/// The row's element id, and the name tests and the rigs aim the pointer at.
pub fn row_id(item: ComposerMenuItem) -> &'static str {
    match item {
        ComposerMenuItem::Undo => "composer-menu-undo",
        ComposerMenuItem::Redo => "composer-menu-redo",
        ComposerMenuItem::Cut => "composer-menu-cut",
        ComposerMenuItem::Copy => "composer-menu-copy",
        ComposerMenuItem::Paste => "composer-menu-paste",
        ComposerMenuItem::SelectAll => "composer-menu-select-all",
    }
}

/// The open menu, appended straight to the shell's root (taffy places an
/// absolute child relative to its parent; a static wrapper would push it off).
pub fn render(shell: &Shell, window: &Window, cx: &mut Context<Shell>) -> Option<Div> {
    let menu = shell.composer_menu.clone()?;
    let theme = Theme::of(cx).clone();
    let origin = place(menu.at, menu_size(), window.viewport_size());
    let mut body = div().flex().flex_col().gap(px(MENU_GAP));
    for item in ITEMS {
        if item.starts_group() {
            body = body.child(glass::menu_divider(&theme));
        }
        let enabled = item.enabled(menu.state);
        let id = row_id(item);
        let row = glass::menu_text_row(id, item.label(), item.shortcut(), enabled, &theme)
            .debug_selector(move || id.to_string());
        let input = menu.input.clone();
        body = body.child(if enabled {
            row.on_click(cx.listener(move |this, _, window, cx| {
                this.composer_menu = None;
                let input = input.clone();
                input.update(cx, |input, cx| run(input, item, window, cx));
                cx.notify();
            }))
        } else {
            // A greyed row swallows its click, as a disabled menu item does:
            // the menu stays open and nothing happens.
            row.on_click(|_, _, cx| cx.stop_propagation())
        });
    }
    Some(
        div()
            .absolute()
            .left(origin.x)
            .top(origin.y)
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(glass::frosted_menu(COMPOSER_MENU_WIDTH, &theme, body)),
    )
}

/// One row's action, on the field the menu was opened in. Each arm calls the
/// method its keyboard shortcut calls — no second implementation exists.
fn run(
    input: &mut ComposerInput,
    item: ComposerMenuItem,
    window: &mut Window,
    cx: &mut Context<ComposerInput>,
) {
    match item {
        ComposerMenuItem::Undo => input.undo_edit(cx),
        ComposerMenuItem::Redo => input.redo_edit(cx),
        ComposerMenuItem::Cut => input.cut_selection(window, cx),
        ComposerMenuItem::Copy => input.copy_selection(cx),
        // The ⌘V path itself: words at the caret, pictures and files to the
        // owner, which stages them as attachments.
        ComposerMenuItem::Paste => input.paste_clipboard(window, cx),
        ComposerMenuItem::SelectAll => input.select_all_text(cx),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels() -> Vec<&'static str> {
        ITEMS.iter().map(|i| i.label()).collect()
    }

    #[test]
    fn items_match_the_edit_menu() {
        assert_eq!(
            labels(),
            ["Undo", "Redo", "Cut", "Copy", "Paste", "Select All"]
        );
        // Two separators, in the Edit menu's places: above Cut and above
        // Select All, never above the first row.
        let groups: Vec<&'static str> = ITEMS
            .iter()
            .filter(|i| i.starts_group())
            .map(|i| i.label())
            .collect();
        assert_eq!(groups, ["Cut", "Select All"]);
        assert!(!ITEMS[0].starts_group());
    }

    #[test]
    fn shortcuts_are_the_registered_bindings() {
        let pairs: Vec<(&str, &str)> = ITEMS.iter().map(|i| (i.label(), i.shortcut())).collect();
        assert_eq!(
            pairs,
            [
                ("Undo", "⌘Z"),
                ("Redo", "⇧⌘Z"),
                ("Cut", "⌘X"),
                ("Copy", "⌘C"),
                ("Paste", "⌘V"),
                ("Select All", "⌘A"),
            ]
        );
    }

    #[test]
    fn an_empty_field_with_an_empty_pasteboard_offers_nothing() {
        let state = EditState::default();
        assert!(ITEMS.iter().all(|i| !i.enabled(state)));
    }

    #[test]
    fn cut_and_copy_need_a_selection() {
        let typed = EditState {
            has_text: true,
            ..Default::default()
        };
        assert!(!ComposerMenuItem::Cut.enabled(typed));
        assert!(!ComposerMenuItem::Copy.enabled(typed));
        assert!(
            ComposerMenuItem::SelectAll.enabled(typed),
            "there is text to select"
        );
        let picked = EditState {
            has_selection: true,
            ..typed
        };
        assert!(ComposerMenuItem::Cut.enabled(picked));
        assert!(ComposerMenuItem::Copy.enabled(picked));
    }

    #[test]
    fn select_all_greys_out_once_everything_is_selected() {
        let all = EditState {
            has_text: true,
            has_selection: true,
            all_selected: true,
            ..Default::default()
        };
        assert!(!ComposerMenuItem::SelectAll.enabled(all));
        assert!(ComposerMenuItem::Copy.enabled(all));
    }

    #[test]
    fn paste_follows_the_pasteboard_alone() {
        // A picture on the pasteboard and an empty field: Paste is live even
        // though there are no words to insert — it stages the picture.
        let state = EditState {
            pasteboard_has_content: true,
            ..Default::default()
        };
        assert!(ComposerMenuItem::Paste.enabled(state));
        assert!(!ComposerMenuItem::Cut.enabled(state));
    }

    #[test]
    fn undo_and_redo_follow_their_stacks() {
        let undoable = EditState {
            can_undo: true,
            ..Default::default()
        };
        assert!(ComposerMenuItem::Undo.enabled(undoable));
        assert!(!ComposerMenuItem::Redo.enabled(undoable));
        let redoable = EditState {
            can_redo: true,
            ..Default::default()
        };
        assert!(ComposerMenuItem::Redo.enabled(redoable));
        assert!(!ComposerMenuItem::Undo.enabled(redoable));
    }

    #[test]
    fn the_menu_is_placed_inside_the_window() {
        let viewport = gpui::size(px(1180.0), px(780.0));
        let size = menu_size();
        assert!(size.height > px(0.0) && size.width == px(COMPOSER_MENU_WIDTH));
        // The composer sits at the bottom of the window, so the common case
        // is a click near the bottom edge: the menu flips above it.
        let origin = place(gpui::point(px(400.0), px(740.0)), size, viewport);
        assert_eq!(origin.y + size.height, px(740.0));
        assert!(origin.y >= px(0.0), "{origin:?}");
        assert_eq!(origin.x, px(400.0));
    }

    #[test]
    fn every_row_has_a_distinct_id() {
        let ids: Vec<&'static str> = ITEMS.iter().map(|i| row_id(*i)).collect();
        assert_eq!(ids[0], "composer-menu-undo");
        assert_eq!(ids[5], "composer-menu-select-all");
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len());
    }

    #[test]
    fn rig_anchor_parses_a_pair() {
        // The variable is read from the environment; parse the same way here.
        let parse = |spec: &str| -> Option<(f32, f32)> {
            let (x, y) = spec.split_once(',')?;
            Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
        };
        assert_eq!(parse("660, 653"), Some((660.0, 653.0)));
        assert_eq!(parse("nope"), None);
    }
}

#[cfg(test)]
mod behaviour_tests {
    //! The menu end to end in a mounted shell: a secondary click opens it,
    //! every row does its work on the field, and a greyed row does nothing.
    //! These are the proof asked for in [feedback
    //! composer-context-menu-mac-2026-11-03] — "verify the actions, do not
    //! just show the menu".
    use futures::future::BoxFuture;
    use gpui::{
        point, Bounds, ClipboardItem, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext,
    };
    use tokio::sync::mpsc;

    use super::*;
    use crate::app::AppState;
    use crate::model::{Chat, Project, SpaceKind};
    use crate::sync::{ClientCommand, SyncEvent, SyncSource};
    use crate::ui::composer::Composer;
    use crate::ui::motion::PulseClock;

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

    fn mount(
        cx: &mut TestAppContext,
    ) -> (
        tokio::runtime::Runtime,
        Entity<Shell>,
        Entity<Composer>,
        &mut VisualTestContext,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let handle = rt.handle().clone();
        cx.update(|cx| {
            gpui_tokio::init_from_handle(cx, handle);
            cx.set_global(Theme::dark(crate::ui::theme::Surface::Matte));
            if !cx.has_global::<PulseClock>() {
                PulseClock::init(cx);
            }
            crate::bind_keys(cx);
        });
        let state = cx.new(|cx| AppState::new(Box::new(Idle), cx));
        let now = crate::model::now_ms();
        state.update(cx, |s, cx| {
            s.projects.push(Project {
                id: "p1".into(),
                name: "Arbonites".into(),
                kind: SpaceKind::Group,
                ..Project::default()
            });
            let chat: Chat = serde_json::from_value(serde_json::json!({
                "id": "c1", "project_id": "p1", "title": "Arbonites", "is_pinned": true,
                "is_system": false, "unread": false, "last_message_at": now,
            }))
            .expect("chat");
            s.chats.insert("p1".into(), vec![chat]);
            s.selected_project = Some("p1".into());
            s.active_tab.insert("p1".into(), "c1".into());
            cx.notify();
        });
        let (shell, vcx) = cx.add_window_view(|window, cx| Shell::new(state.clone(), window, cx));
        vcx.run_until_parked();
        let composer = shell.read_with(&*vcx, |sh, _| sh.composer.clone());
        vcx.update(|window, cx| Composer::focus(&composer, window, cx));
        settle(&shell, vcx);
        (rt, shell, composer, vcx)
    }

    fn settle(shell: &Entity<Shell>, vcx: &mut VisualTestContext) {
        for _ in 0..3 {
            shell.update(vcx, |_, cx| cx.notify());
            vcx.run_until_parked();
        }
    }

    fn centre(b: Bounds<Pixels>) -> Point<Pixels> {
        point(b.left() + b.size.width / 2.0, b.top() + b.size.height / 2.0)
    }

    fn input(composer: &Entity<Composer>, vcx: &VisualTestContext) -> Entity<ComposerInput> {
        composer.read_with(vcx, |c, _| c.input.clone())
    }

    fn text(composer: &Entity<Composer>, vcx: &VisualTestContext) -> String {
        composer.read_with(vcx, |c, cx| c.input.read(cx).text().to_string())
    }

    fn type_draft(draft: &str, shell: &Entity<Shell>, vcx: &mut VisualTestContext) {
        vcx.simulate_input(draft);
        settle(shell, vcx);
    }

    fn select(
        composer: &Entity<Composer>,
        range: std::ops::Range<usize>,
        vcx: &mut VisualTestContext,
    ) {
        let input = input(composer, vcx);
        vcx.update(|_, cx| input.update(cx, |i, cx| i.select_range(range, cx)));
        vcx.run_until_parked();
    }

    /// A point `into` points along the drawn text, measured from the first
    /// character. The text area pads its left edge (`FIELD_TEXT_INSET`, where
    /// the chip pills draw) and that padding belongs to the parent, not to
    /// the input — a click there reaches nothing.
    fn field_point(into: f32, vcx: &mut VisualTestContext) -> Point<Pixels> {
        let text = vcx
            .debug_bounds("composer-field-text")
            .expect("the composer's text area is drawn");
        point(text.left() + px(10.0 + into), centre(text).y)
    }

    fn open_menu(modifiers: Modifiers, shell: &Entity<Shell>, vcx: &mut VisualTestContext) {
        open_menu_at(2.0, modifiers, shell, vcx);
    }

    /// A secondary click `into` points along the text. Mirrors a real
    /// pointer: a move first, then the button.
    fn open_menu_at(
        into: f32,
        modifiers: Modifiers,
        shell: &Entity<Shell>,
        vcx: &mut VisualTestContext,
    ) {
        let at = field_point(into, vcx);
        vcx.simulate_event(gpui::MouseMoveEvent {
            position: at,
            modifiers: Modifiers::default(),
            pressed_button: None,
        });
        if modifiers.control {
            vcx.simulate_event(gpui::MouseDownEvent {
                button: gpui::MouseButton::Left,
                position: at,
                modifiers,
                click_count: 1,
                first_mouse: false,
            });
        } else {
            vcx.simulate_event(gpui::MouseDownEvent {
                button: gpui::MouseButton::Right,
                position: at,
                modifiers,
                click_count: 1,
                first_mouse: false,
            });
        }
        settle(shell, vcx);
    }

    fn menu_is_open(shell: &Entity<Shell>, vcx: &VisualTestContext) -> bool {
        shell.read_with(vcx, |sh, _| sh.composer_menu.is_some())
    }

    fn click_row(item: ComposerMenuItem, shell: &Entity<Shell>, vcx: &mut VisualTestContext) {
        let id = row_id(item);
        let row = vcx
            .debug_bounds(id)
            .unwrap_or_else(|| panic!("{id} is drawn"));
        vcx.simulate_click(centre(row), Modifiers::default());
        settle(shell, vcx);
    }

    fn row_state(item: ComposerMenuItem, shell: &Entity<Shell>, vcx: &VisualTestContext) -> bool {
        shell
            .read_with(vcx, |sh, _| sh.composer_menu.clone())
            .map(|m| item.enabled(m.state))
            .expect("the menu is open")
    }

    #[gpui::test]
    fn a_right_click_in_the_field_opens_the_menu(cx: &mut TestAppContext) {
        let (_rt, shell, composer, vcx) = mount(cx);
        type_draft("hello there", &shell, vcx);
        assert!(!menu_is_open(&shell, vcx), "nothing is open to begin with");

        open_menu(Modifiers::default(), &shell, vcx);
        assert!(menu_is_open(&shell, vcx), "right-click opened the menu");
        // Every row is on screen, in the Edit menu's order.
        for item in ITEMS {
            assert!(
                vcx.debug_bounds(row_id(item)).is_some(),
                "{} is drawn",
                item.label()
            );
        }
        assert_eq!(
            text(&composer, vcx),
            "hello there",
            "the draft is untouched"
        );
    }

    #[gpui::test]
    fn a_control_click_opens_the_same_menu(cx: &mut TestAppContext) {
        let (_rt, shell, _composer, vcx) = mount(cx);
        type_draft("hello there", &shell, vcx);
        open_menu(Modifiers::control(), &shell, vcx);
        assert!(
            menu_is_open(&shell, vcx),
            "control-click is the other secondary click"
        );
        assert!(vcx.debug_bounds(row_id(ComposerMenuItem::Paste)).is_some());
    }

    /// AppKit's rule, both halves: a secondary click **inside** the selection
    /// leaves it alone, so Cut and Copy still act on what the user picked; a
    /// click **outside** collapses it, as a plain click does.
    #[gpui::test]
    fn a_secondary_click_keeps_a_selection_it_lands_in(cx: &mut TestAppContext) {
        let (_rt, shell, composer, vcx) = mount(cx);
        type_draft("hello there", &shell, vcx);
        select(&composer, 0..11, vcx);
        open_menu(Modifiers::default(), &shell, vcx);
        assert!(
            row_state(ComposerMenuItem::Copy, &shell, vcx),
            "the click landed inside the selection, so Copy is live"
        );
        let picked = input(&composer, vcx).read_with(vcx, |i, _| i.selected_text().to_string());
        assert_eq!(picked, "hello there", "the selection survived the click");
    }

    #[gpui::test]
    fn a_secondary_click_outside_the_selection_collapses_it(cx: &mut TestAppContext) {
        let (_rt, shell, composer, vcx) = mount(cx);
        type_draft("hello there", &shell, vcx);
        // Pick the last word, then click at the very start of the text.
        select(&composer, 6..11, vcx);
        open_menu_at(0.0, Modifiers::default(), &shell, vcx);
        assert!(
            !row_state(ComposerMenuItem::Copy, &shell, vcx),
            "the click fell outside the selection, so it collapsed and Copy is greyed"
        );
        let picked = input(&composer, vcx).read_with(vcx, |i, _| i.selected_text().to_string());
        assert!(picked.is_empty(), "{picked:?}");
    }

    #[gpui::test]
    fn copy_takes_the_selection_and_cut_also_removes_it(cx: &mut TestAppContext) {
        let (_rt, shell, composer, vcx) = mount(cx);
        type_draft("alpha beta", &shell, vcx);
        select(&composer, 0..5, vcx);
        open_menu(Modifiers::default(), &shell, vcx);
        click_row(ComposerMenuItem::Copy, &shell, vcx);
        assert!(!menu_is_open(&shell, vcx), "the menu closed after the row");
        assert_eq!(
            vcx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
            Some("alpha"),
            "Copy took the selected words, not the whole draft"
        );
        assert_eq!(text(&composer, vcx), "alpha beta", "Copy removed nothing");

        select(&composer, 0..5, vcx);
        open_menu(Modifiers::default(), &shell, vcx);
        click_row(ComposerMenuItem::Cut, &shell, vcx);
        assert_eq!(
            vcx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
            Some("alpha")
        );
        assert_eq!(
            text(&composer, vcx),
            " beta",
            "Cut removed exactly the selection"
        );
    }

    /// Paste lands at the caret, and the caret is where the secondary click
    /// put it — the click here falls at the start of the text, outside any
    /// selection, so the words go in front of the draft, not at its end.
    #[gpui::test]
    fn paste_puts_the_words_where_the_click_left_the_caret(cx: &mut TestAppContext) {
        let (_rt, shell, composer, vcx) = mount(cx);
        type_draft("see ", &shell, vcx);
        vcx.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string("this".into())));
        open_menu_at(0.0, Modifiers::default(), &shell, vcx);
        assert!(row_state(ComposerMenuItem::Paste, &shell, vcx));
        let caret = input(&composer, vcx).read_with(vcx, |i, _| i.cursor_offset());
        assert_eq!(caret, 0, "the secondary click moved the caret to the click");
        click_row(ComposerMenuItem::Paste, &shell, vcx);
        assert_eq!(text(&composer, vcx), "thissee ");
    }

    /// The menu's Paste is the ⌘V path itself: a picture on the pasteboard is
    /// staged as an attachment, exactly as the keystroke stages it. The other
    /// half of [feedback copy-image-messages-mac-2026-10-27] feeds this.
    #[gpui::test]
    fn paste_stages_a_picture_the_same_way_cmd_v_does(cx: &mut TestAppContext) {
        let png = {
            let mut out = std::io::Cursor::new(Vec::new());
            image::RgbaImage::from_pixel(4, 3, image::Rgba([10, 120, 220, 255]))
                .write_to(&mut out, image::ImageFormat::Png)
                .expect("png");
            out.into_inner()
        };
        let item = || ClipboardItem {
            entries: vec![gpui::ClipboardEntry::Image(gpui::Image {
                format: gpui::ImageFormat::Png,
                id: gpui::hash(&png),
                bytes: png.clone(),
            })],
        };

        // Through the menu.
        let (_rt, shell, composer, vcx) = mount(cx);
        vcx.update(|_, cx| cx.write_to_clipboard(item()));
        open_menu(Modifiers::default(), &shell, vcx);
        assert!(
            row_state(ComposerMenuItem::Paste, &shell, vcx),
            "a picture alone still makes Paste live"
        );
        click_row(ComposerMenuItem::Paste, &shell, vcx);
        let by_menu = composer.read_with(vcx, |c, _| c.staged_paths());
        assert_eq!(by_menu.len(), 1, "the menu staged the picture");

        // Through ⌘V, on a fresh field: the same outcome.
        let (_rt2, shell2, composer2, vcx2) = mount(cx);
        vcx2.update(|_, cx| cx.write_to_clipboard(item()));
        vcx2.simulate_keystrokes("cmd-v");
        settle(&shell2, vcx2);
        let by_key = composer2.read_with(vcx2, |c, _| c.staged_paths());
        assert_eq!(
            by_key.len(),
            by_menu.len(),
            "menu Paste and ⌘V stage the same picture — one code path"
        );
    }

    #[gpui::test]
    fn select_all_takes_every_line_of_a_multi_line_draft(cx: &mut TestAppContext) {
        let (_rt, shell, composer, vcx) = mount(cx);
        vcx.simulate_input("first line");
        vcx.simulate_keystrokes("shift-enter");
        vcx.simulate_input("second line");
        settle(&shell, vcx);
        let whole = text(&composer, vcx);
        assert!(whole.contains('\n'), "the draft really has two lines");

        // The caret sits at the end of the second line; Select All must not
        // stop at that line's start.
        open_menu(Modifiers::default(), &shell, vcx);
        click_row(ComposerMenuItem::SelectAll, &shell, vcx);
        let picked = input(&composer, vcx).read_with(vcx, |i, _| i.selected_text().to_string());
        assert_eq!(picked, whole, "both lines, the newline between them too");
    }

    #[gpui::test]
    fn undo_then_redo_walk_the_fields_own_history(cx: &mut TestAppContext) {
        let (_rt, shell, composer, vcx) = mount(cx);
        type_draft("keep this", &shell, vcx);
        select(&composer, 0..9, vcx);
        open_menu(Modifiers::default(), &shell, vcx);
        click_row(ComposerMenuItem::Cut, &shell, vcx);
        assert_eq!(text(&composer, vcx), "");

        open_menu(Modifiers::default(), &shell, vcx);
        assert!(row_state(ComposerMenuItem::Undo, &shell, vcx));
        assert!(
            !row_state(ComposerMenuItem::Redo, &shell, vcx),
            "nothing undone yet"
        );
        click_row(ComposerMenuItem::Undo, &shell, vcx);
        assert_eq!(text(&composer, vcx), "keep this", "Undo put the words back");

        open_menu(Modifiers::default(), &shell, vcx);
        assert!(row_state(ComposerMenuItem::Redo, &shell, vcx));
        click_row(ComposerMenuItem::Redo, &shell, vcx);
        assert_eq!(text(&composer, vcx), "", "Redo took them away again");
    }

    #[gpui::test]
    fn a_greyed_row_does_nothing_and_leaves_the_menu_up(cx: &mut TestAppContext) {
        let (_rt, shell, composer, vcx) = mount(cx);
        type_draft("nothing is selected", &shell, vcx);
        vcx.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string(String::new())));
        open_menu(Modifiers::default(), &shell, vcx);
        assert!(
            !row_state(ComposerMenuItem::Copy, &shell, vcx),
            "no selection, so Copy is greyed"
        );
        click_row(ComposerMenuItem::Copy, &shell, vcx);
        assert!(
            menu_is_open(&shell, vcx),
            "a greyed row swallows its click; the menu stays up"
        );
        assert_eq!(text(&composer, vcx), "nothing is selected");
    }

    #[gpui::test]
    fn escape_closes_the_menu_without_touching_the_draft(cx: &mut TestAppContext) {
        let (_rt, shell, composer, vcx) = mount(cx);
        type_draft("still here", &shell, vcx);
        open_menu(Modifiers::default(), &shell, vcx);
        assert!(menu_is_open(&shell, vcx));
        vcx.dispatch_action(crate::ui::shell::Escape);
        settle(&shell, vcx);
        assert!(!menu_is_open(&shell, vcx));
        assert_eq!(text(&composer, vcx), "still here");
    }
}
