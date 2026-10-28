//! One backdrop for every modal sheet — the "Forward to" picker (from a bubble
//! or the media viewer), "Delete message?", the search palette: a true modal
//! (Jacob, feedback forward-sheet-focus-2026-10-24). The backdrop dims the
//! window, swallows scroll and clicks so nothing reaches the chat behind it,
//! and a click outside the card closes the sheet. Keys: the sheet names the
//! focus handle that owns the keyboard while it is up (`Shell::modal_focus`),
//! and the shell moves focus there every frame — the composer behind a sheet
//! never types.
//!
//! Use: `modal_overlay("my-sheet", dismiss, card)` appended straight to the
//! shell's root (a positioned child inside a flex item lands off-screen).

use gpui::{div, prelude::*, App, Div, ElementId, FocusHandle, MouseButton, Stateful, Window};

use crate::ui::theme::Theme;

/// How much the window dims behind a sheet.
pub const BACKDROP_ALPHA: f32 = 0.35;

/// The full-window backdrop with `card` centred on it. `dismiss` runs on a
/// click outside the card (Esc is the shell's: see `Shell::escape`).
pub fn modal_overlay(
    id: impl Into<ElementId>,
    dismiss: impl Fn(&mut Window, &mut App) + 'static,
    card: impl IntoElement,
) -> Stateful<Div> {
    overlay_at(id, dismiss, card, None)
}

fn overlay_at(
    id: impl Into<ElementId>,
    dismiss: impl Fn(&mut Window, &mut App) + 'static,
    card: impl IntoElement,
    top: Option<f32>,
) -> Stateful<Div> {
    backdrop(id, top, BACKDROP_ALPHA)
        .occlude()
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            cx.stop_propagation();
            dismiss(window, cx);
        })
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(modal_card(card))
}

/// The palette's backdrop ([feedback search-toggle-click-outside-mac-2026-10-27],
/// Jacob 2026-10-27): a click outside the panel dismisses it **and still lands
/// on what is under it** — a message, a sidebar row, a button answers in the
/// same click. So this backdrop does not occlude and takes no mouse-down of
/// its own; the dismissal is the shell's capture-phase click-outside
/// (`Shell::click_outside`). It still swallows the wheel so the transcript
/// stays put (#1632). `card` keeps the modal card's own mouse-down stop, so a
/// click inside the panel never reaches the chat.
///
/// It does **not** dim ([feedback search-popup-single-layer-2026-10-28], Jacob
/// 2026-10-28 "double transparency"): the panel is one frosted plate
/// (`glass::popup_plate`), and a dim under a translucent plate was the second
/// layer — the two stacked to a muddy wash with the chat still reading through.
/// Spotlight does not dim the desktop either.
pub fn modal_overlay_top_passthrough(
    id: impl Into<ElementId>,
    top: f32,
    card: impl IntoElement,
) -> Stateful<Div> {
    backdrop(id, Some(top), PASSTHROUGH_BACKDROP_ALPHA).child(modal_card(card))
}

/// How much the see-through backdrop dims: nothing. One layer, the plate.
pub const PASSTHROUGH_BACKDROP_ALPHA: f32 = 0.0;

fn backdrop(id: impl Into<ElementId>, top: Option<f32>, dim: f32) -> Stateful<Div> {
    div()
        .id(id)
        .absolute()
        .inset_0()
        .flex()
        .flex_col()
        .items_center()
        .map(|el| match top {
            Some(top) => el.justify_start().pt(gpui::px(top)),
            None => el.justify_center(),
        })
        .when(dim > 0.0, |el| el.bg(Theme::with_alpha(gpui::black(), dim)))
        // Wheel / trackpad scrolling over the sheet — list or backdrop — ends
        // here; the transcript underneath stays still.
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
}

/// Keep the keyboard inside the open sheet: when neither `owner` nor anything
/// inside it has focus (the composer took it, a click moved it, a stale
/// handle), hand it back. Call once per frame while the sheet is up.
pub fn keep_focus(owner: &FocusHandle, window: &mut Window, cx: &mut App) {
    if !owner.is_focused(window) && !owner.contains_focused(window, cx) {
        window.focus(owner, cx);
    }
}

/// The card's hit box: clicks inside stay inside (they must not dismiss or
/// reach the window behind).
fn modal_card(card: impl IntoElement) -> Div {
    div()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .child(card)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{
        point, px, Focusable, Pixels, ScrollDelta, ScrollHandle, ScrollWheelEvent, TestAppContext,
        TouchPhase,
    };

    use super::*;
    use crate::ui::composer::ComposerInput;
    use crate::ui::theme::{AppearancePref, Surface};

    const W: f32 = 900.0;
    const H: f32 = 600.0;
    const ROWS: usize = 40;
    const ROW_H: f32 = 40.0;

    /// A chat column that scrolls with a composer under it, and — when `sheet`
    /// is on — the modal with a search field and a scrolling list of its own
    /// over it (the Forward picker's shape). `keep_focus` runs each frame while
    /// the sheet is up, as the shell does.
    struct Scene {
        transcript: ScrollHandle,
        sheet_list: ScrollHandle,
        sheet: bool,
        dismissed: Rc<Cell<bool>>,
        composer: gpui::Entity<ComposerInput>,
        field: gpui::Entity<ComposerInput>,
    }

    fn rows(id: &'static str, n: usize, h: f32) -> Div {
        let mut list = div().flex().flex_col();
        for i in 0..n {
            list = list.child(div().h(px(h)).flex_shrink_0().child(format!("{id} {i}")));
        }
        list
    }

    impl gpui::Render for Scene {
        fn render(
            &mut self,
            window: &mut Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            if self.sheet {
                keep_focus(&self.field.read(cx).focus_handle(cx), window, cx);
            }
            let transcript = div()
                .id("transcript")
                .absolute()
                .inset_0()
                .overflow_y_scroll()
                .track_scroll(&self.transcript)
                .debug_selector(|| "transcript".into())
                .child(rows("message", ROWS, ROW_H));
            let composer = div()
                .absolute()
                .bottom_0()
                .left_0()
                .right_0()
                .h(px(44.0))
                .child(self.composer.clone());
            let mut root = div()
                .relative()
                .w(px(W))
                .h(px(H))
                .child(transcript)
                .child(composer);
            if self.sheet {
                let dismissed = self.dismissed.clone();
                let list = div()
                    .id("sheet-list")
                    .h(px(200.0))
                    .overflow_y_scroll()
                    .track_scroll(&self.sheet_list)
                    .debug_selector(|| "sheet-list".into())
                    .child(rows("target", 30, 52.0));
                let card = div()
                    .w(px(360.0))
                    .h(px(300.0))
                    .flex()
                    .flex_col()
                    .debug_selector(|| "sheet-card".into())
                    .child(div().h(px(28.0)).child(self.field.clone()))
                    .child(list);
                root = root.child(modal_overlay(
                    "sheet",
                    move |_, _| dismissed.set(true),
                    card,
                ));
            }
            root
        }
    }

    fn setup(
        cx: &mut TestAppContext,
        sheet: bool,
    ) -> (gpui::Entity<Scene>, &mut gpui::VisualTestContext) {
        cx.update(|cx| {
            if !cx.has_global::<Theme>() {
                let theme = Theme::for_pref(AppearancePref::Dark, Surface::Matte, cx);
                cx.set_global(theme);
            }
        });
        let (view, vcx) = cx.add_window_view(|window, cx| {
            let composer = cx.new(ComposerInput::new);
            let field = cx.new(ComposerInput::new);
            // The chat's composer owns the keyboard before the sheet opens.
            let handle = composer.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
            Scene {
                transcript: ScrollHandle::new(),
                sheet_list: ScrollHandle::new(),
                sheet,
                dismissed: Rc::new(Cell::new(false)),
                composer,
                field,
            }
        });
        vcx.run_until_parked();
        (view, vcx)
    }

    fn texts(view: &gpui::Entity<Scene>, vcx: &gpui::VisualTestContext) -> (String, String) {
        view.read_with(vcx, |s, cx| {
            (
                s.composer.read(cx).text().to_owned(),
                s.field.read(cx).text().to_owned(),
            )
        })
    }

    fn wheel(vcx: &mut gpui::VisualTestContext, at: gpui::Point<Pixels>, dy: f32) {
        vcx.simulate_event(ScrollWheelEvent {
            position: at,
            delta: ScrollDelta::Pixels(point(px(0.0), px(dy))),
            modifiers: Default::default(),
            touch_phase: TouchPhase::Moved,
        });
        vcx.run_until_parked();
    }

    fn offset_y(view: &gpui::Entity<Scene>, vcx: &gpui::VisualTestContext) -> f32 {
        view.read_with(vcx, |s, _| s.transcript.offset().y.into())
    }

    /// Control: without a sheet the same wheel event scrolls the chat.
    #[gpui::test]
    fn the_chat_scrolls_when_no_sheet_is_up(cx: &mut TestAppContext) {
        let (view, vcx) = setup(cx, false);
        wheel(vcx, point(px(W / 2.0), px(H / 2.0)), -120.0);
        assert!(
            offset_y(&view, vcx) < -1.0,
            "the transcript did not scroll ({})",
            offset_y(&view, vcx)
        );
    }

    /// Jacob (Oct 24): scrolling inside "Forward to" scrolled the chat behind
    /// it. Over the card's list, over the card and over the backdrop, the
    /// transcript stays where it was; the sheet's own list still scrolls.
    #[gpui::test]
    fn a_sheet_keeps_the_chat_still(cx: &mut TestAppContext) {
        let (view, vcx) = setup(cx, true);
        let list = vcx.debug_bounds("sheet-list").expect("sheet list");
        let card = vcx.debug_bounds("sheet-card").expect("card");
        let before = offset_y(&view, vcx);
        wheel(vcx, list.center(), -120.0);
        wheel(
            vcx,
            point(card.center().x, card.bottom() - px(10.0)),
            -120.0,
        );
        wheel(vcx, point(px(20.0), px(20.0)), -120.0);
        assert_eq!(
            offset_y(&view, vcx),
            before,
            "the transcript moved under the sheet"
        );
        let own: f32 = view.read_with(vcx, |s, _| s.sheet_list.offset().y.into());
        assert!(own < -1.0, "the sheet's own list did not scroll ({own})");
    }

    /// A click on the backdrop closes the sheet; a click inside the card does not.
    #[gpui::test]
    fn a_click_outside_the_card_dismisses(cx: &mut TestAppContext) {
        let (view, vcx) = setup(cx, true);
        let card = vcx.debug_bounds("sheet-card").expect("card");
        vcx.simulate_click(card.center(), Default::default());
        vcx.run_until_parked();
        assert!(!view.read_with(vcx, |s, _| s.dismissed.get()));
        vcx.simulate_click(point(px(20.0), px(20.0)), Default::default());
        vcx.run_until_parked();
        assert!(view.read_with(vcx, |s, _| s.dismissed.get()));
    }

    /// Jacob (Oct 24): keystrokes went to the chat composer behind the sheet.
    /// The sheet's field takes the keyboard on open, keeps it, and typing lands
    /// there — the composer stays empty.
    #[gpui::test]
    fn the_sheet_field_owns_the_keyboard(cx: &mut TestAppContext) {
        let (view, vcx) = setup(cx, true);
        let focused = view.read_with(vcx, |s, cx| {
            let field = s.field.read(cx).focus_handle(cx);
            let composer = s.composer.read(cx).focus_handle(cx);
            (field, composer)
        });
        vcx.update(|window, _| {
            assert!(
                focused.0.is_focused(window),
                "focus moved to the sheet's field on open"
            );
            assert!(!focused.1.is_focused(window), "the composer lost focus");
        });
        vcx.simulate_input("sam");
        vcx.run_until_parked();
        assert_eq!(texts(&view, vcx), (String::new(), "sam".into()));
    }

    /// Control: with no sheet the same keystrokes type into the composer.
    #[gpui::test]
    fn without_a_sheet_the_composer_types(cx: &mut TestAppContext) {
        let (view, vcx) = setup(cx, false);
        vcx.simulate_input("sam");
        vcx.run_until_parked();
        assert_eq!(texts(&view, vcx), ("sam".into(), String::new()));
    }
}
