//! The chat pane as a file-drop target ([feedback chat-drop-highlight-2026-10-23]).
//!
//! Finder drags reach GPUI as `FileDropEvent`s that the window turns into an
//! `ExternalPaths` drag plus synthetic mouse moves. The pane used to rely on
//! `group_drag_over` for its highlight, which only ever styled the composer's
//! input field and, like every GPUI hover style, is gated on
//! `HitboxId::is_hovered` — false whenever the last input was a key press
//! (typing a message, then dragging a picture in). So the pane accepted the
//! drop while nothing lit up.
//!
//! Here the view owns the state: `on_drag_move` fires for every move of an
//! external drag whatever the hover state, and tells us whether the pointer
//! is inside the pane's own hitbox bounds. The drop, a drag that leaves the
//! window (`FileDropEvent::Exited`, also what Escape produces) and the drag's
//! end all clear it. While set, [`overlay`] paints over the whole pane.

use gpui::{
    canvas, div, prelude::*, px, AnyElement, Context, Div, DragMoveEvent, ExternalPaths,
    FileDropEvent, FontWeight, MouseUpEvent, Stateful, WeakEntity, Window,
};

use crate::ui::sf_symbols;
use crate::ui::theme::{Theme, TEXT_BODY, TEXT_SMALL};

/// The outline's inset from the pane edges, and its corner radius.
pub const OUTLINE_INSET: f32 = 10.0;
pub const OUTLINE_RADIUS: f32 = 18.0;
/// How much the pane recedes under the outline: a dark theme dims towards
/// black, a light one washes towards white (a black dim turns a white chat
/// grey and muddy).
const DIM_ALPHA_DARK: f32 = 0.42;
const WASH_ALPHA_LIGHT: f32 = 0.6;

pub const HINT_TITLE: &str = "Drop to attach";
pub const HINT_BODY: &str = "Images and files go into this chat";

/// Wire `pane` — the chat column root, a stateful div with its own hitbox —
/// as the drop target. `set_over(view, inside)` is called on every move of an
/// external drag (the caller notifies only on change); `on_drop` receives the
/// paths. The overlay is appended when `over`, so it sits above everything in
/// the pane, header included.
pub fn target<V: 'static>(
    pane: Stateful<Div>,
    over: bool,
    theme: &Theme,
    cx: &mut Context<V>,
    set_over: impl Fn(&mut V, bool, &mut Context<V>) + Clone + 'static,
    on_drop: impl Fn(&mut V, &ExternalPaths, &mut Window, &mut Context<V>) + 'static,
) -> Stateful<Div> {
    let set_on_move = set_over.clone();
    let set_on_drop = set_over.clone();
    pane.on_drag_move::<ExternalPaths>(cx.listener(
        move |this, ev: &DragMoveEvent<ExternalPaths>, _, cx| {
            set_on_move(this, ev.bounds.contains(&ev.event.position), cx);
        },
    ))
    .can_drop(|dragged, _, _| dragged.is::<ExternalPaths>())
    .on_drop(cx.listener(move |this, paths: &ExternalPaths, window, cx| {
        set_on_drop(this, false, cx);
        on_drop(this, paths, window, cx);
    }))
    .child(drag_end_watch(cx.weak_entity(), set_over))
    .when(over, |el| el.child(overlay(theme)))
}

/// A drag that leaves the window, or ends anywhere, clears the state. These
/// are window-level listeners: `FileDropEvent::Exited` is not routed to any
/// element, and a mouse-up outside the pane never reaches its `on_drop`.
fn drag_end_watch<V: 'static>(
    view: WeakEntity<V>,
    set_over: impl Fn(&mut V, bool, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |_, _, window, _| {
            let on_exit = view.clone();
            let set_on_exit = set_over.clone();
            window.on_mouse_event(move |ev: &FileDropEvent, _, _, cx| {
                if matches!(ev, FileDropEvent::Exited) {
                    on_exit
                        .update(cx, |this, cx| set_on_exit(this, false, cx))
                        .ok();
                }
            });
            let on_up = view.clone();
            let set_on_up = set_over.clone();
            window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| {
                on_up.update(cx, |this, cx| set_on_up(this, false, cx)).ok();
            });
        },
    )
    .absolute()
    .size(px(0.0))
    .into_any_element()
}

/// The highlight: the pane dims, an accent outline runs just inside its edges,
/// and a centred card says what the drop does. No ids → no hitboxes, so the
/// drop still lands on the pane beneath.
pub fn overlay(theme: &Theme) -> Div {
    let recede = if theme.is_light() {
        Theme::with_alpha(gpui::white(), WASH_ALPHA_LIGHT)
    } else {
        Theme::with_alpha(gpui::black(), DIM_ALPHA_DARK)
    };
    div()
        .debug_selector(|| "chat-drop-overlay".into())
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(recede)
        .child(
            div()
                .absolute()
                .inset(px(OUTLINE_INSET))
                .rounded(px(OUTLINE_RADIUS))
                .border_2()
                .border_dashed()
                .border_color(theme.accent)
                .bg(Theme::with_alpha(theme.accent, 0.05)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(10.0))
                .px(px(26.0))
                .py(px(22.0))
                .rounded(px(16.0))
                .bg(theme.pane_card)
                .border_1()
                .border_color(theme.border_strong)
                .shadow_lg()
                .child(
                    div()
                        .size(px(44.0))
                        .rounded_full()
                        .bg(theme.accent)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(sf_symbols::glyph("arrow.down", 22.0, gpui::white())),
                )
                .child(
                    div()
                        .text_size(px(TEXT_BODY))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child(HINT_TITLE),
                )
                .child(
                    div()
                        .text_size(px(TEXT_SMALL))
                        .text_color(theme.text_muted)
                        .child(HINT_BODY),
                ),
        )
}

#[cfg(test)]
mod drop_tests {
    //! The pane lights up on drag-enter, follows the pointer across its whole
    //! bounds (and only those), and clears on drop, exit and release — with a
    //! keyboard-last input too, where GPUI's own hover styles stay dark.

    use std::path::PathBuf;

    use gpui::{point, px, Bounds, FileDropEvent, Pixels, Render, TestAppContext};

    use super::*;
    use crate::ui::theme::{AppearancePref, Surface};

    const SIDEBAR: f32 = 260.0;
    const PANE_W: f32 = 884.0;
    const H: f32 = 600.0;

    struct Chat {
        over: bool,
        dropped: Vec<PathBuf>,
    }

    impl Chat {
        fn set_over(&mut self, over: bool, cx: &mut Context<Self>) {
            if self.over != over {
                self.over = over;
                cx.notify();
            }
        }
    }

    impl Render for Chat {
        fn render(&mut self, _: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::of(cx).clone();
            let pane = div()
                .id("chat-pane")
                .debug_selector(|| "test-pane".into())
                .relative()
                .w(px(PANE_W))
                .h(px(H))
                .child(div().id("transcript").size_full());
            div()
                .flex()
                .w(px(SIDEBAR + PANE_W))
                .h(px(H))
                .child(div().id("sidebar").w(px(SIDEBAR)).h(px(H)))
                .child(target(
                    pane,
                    self.over,
                    &theme,
                    cx,
                    Chat::set_over,
                    |this, paths, _, cx| {
                        this.dropped.extend(paths.paths().iter().cloned());
                        cx.notify();
                    },
                ))
        }
    }

    fn paths() -> ExternalPaths {
        ExternalPaths(std::iter::once(PathBuf::from("/tmp/shot.png")).collect())
    }

    fn setup(cx: &mut TestAppContext) -> (gpui::Entity<Chat>, &mut gpui::VisualTestContext) {
        cx.update(|cx| {
            if !cx.has_global::<Theme>() {
                let theme = Theme::for_pref(AppearancePref::Dark, Surface::Matte, cx);
                cx.set_global(theme);
            }
        });
        let (view, vcx) = cx.add_window_view(|_, _| Chat {
            over: false,
            dropped: Vec::new(),
        });
        vcx.run_until_parked();
        (view, vcx)
    }

    fn over(view: &gpui::Entity<Chat>, vcx: &gpui::VisualTestContext) -> bool {
        view.read_with(vcx, |w, _| w.over)
    }

    fn in_pane() -> gpui::Point<Pixels> {
        point(px(SIDEBAR + 300.0), px(H / 2.0))
    }

    #[gpui::test]
    fn enter_lights_the_whole_pane_and_drop_clears_it(cx: &mut TestAppContext) {
        let (view, vcx) = setup(cx);
        assert!(
            vcx.debug_bounds("chat-drop-overlay").is_none(),
            "dark at rest"
        );

        vcx.simulate_event(FileDropEvent::Entered {
            position: in_pane(),
            paths: paths(),
        });
        vcx.run_until_parked();
        assert!(over(&view, vcx), "drag-enter sets the state");
        let pane: Bounds<Pixels> = vcx.debug_bounds("test-pane").expect("pane drawn");
        let overlay = vcx
            .debug_bounds("chat-drop-overlay")
            .expect("overlay drawn while over");
        assert_eq!(overlay, pane, "the highlight covers the full chat bounds");

        vcx.simulate_event(FileDropEvent::Submit {
            position: point(px(SIDEBAR + 40.0), px(H - 30.0)),
        });
        vcx.run_until_parked();
        let dropped = view.read_with(vcx, |w, _| w.dropped.clone());
        assert_eq!(dropped, vec![PathBuf::from("/tmp/shot.png")]);
        assert!(!over(&view, vcx), "the drop clears the state");
        assert!(vcx.debug_bounds("chat-drop-overlay").is_none());
    }

    #[gpui::test]
    fn the_sidebar_is_not_part_of_the_target(cx: &mut TestAppContext) {
        let (view, vcx) = setup(cx);
        vcx.simulate_event(FileDropEvent::Entered {
            position: point(px(SIDEBAR / 2.0), px(H / 2.0)),
            paths: paths(),
        });
        vcx.run_until_parked();
        assert!(!over(&view, vcx), "over the sidebar: dark");

        vcx.simulate_event(FileDropEvent::Pending {
            position: point(px(SIDEBAR + 1.0), px(1.0)),
        });
        vcx.run_until_parked();
        assert!(over(&view, vcx), "the pane's top-left corner counts");

        vcx.simulate_event(FileDropEvent::Pending {
            position: point(px(SIDEBAR - 1.0), px(H / 2.0)),
        });
        vcx.run_until_parked();
        assert!(!over(&view, vcx), "back over the sidebar: dark again");
    }

    #[gpui::test]
    fn leaving_the_window_clears_it(cx: &mut TestAppContext) {
        let (view, vcx) = setup(cx);
        vcx.simulate_event(FileDropEvent::Entered {
            position: in_pane(),
            paths: paths(),
        });
        vcx.run_until_parked();
        assert!(over(&view, vcx));

        vcx.simulate_event(FileDropEvent::Exited);
        vcx.run_until_parked();
        assert!(!over(&view, vcx), "exit (or Escape) clears the state");
        assert!(vcx.debug_bounds("chat-drop-overlay").is_none());
        assert!(
            view.read_with(vcx, |w, _| w.dropped.is_empty()),
            "nothing attached"
        );
    }

    #[gpui::test]
    fn lights_up_after_typing_too(cx: &mut TestAppContext) {
        let (view, vcx) = setup(cx);
        // Jacob's path: type a message, reach for a picture in Finder, drag it
        // in. GPUI's hover styles stay off while the last input was a key.
        vcx.simulate_keystrokes("a");
        vcx.simulate_event(FileDropEvent::Entered {
            position: in_pane(),
            paths: paths(),
        });
        vcx.run_until_parked();
        assert!(over(&view, vcx), "the highlight does not depend on hover");
        assert!(vcx.debug_bounds("chat-drop-overlay").is_some());
    }
}
