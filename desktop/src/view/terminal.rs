//! The terminal pane: a grid, a keyboard, and nothing else.
//!
//! The bytes come from the window's own kernel connection, by way of
//! [`crate::model::pty::PtyStreams`], and the keys go back the same way. The
//! pane opens no socket of its own and waits on nothing.
//!
//! It used to. A pane attached to the place's kernel itself — which meant a
//! canonicalize, a bootstrap of `.arbos/`, a liveness probe, an HTTP call to
//! `/healthz` and, once, running the kernel binary to read its commit — and
//! then a second attach that replayed the greeting, the snapshot and the
//! focused chat's transcript before anything of the shell could arrive.
//! After all of it the prompt was not there to be had: it had gone out over
//! the connection the window already held, moments before this one existed.
//! The drawer showed "Connecting…", then an empty screen, until the person
//! pressed a key.

use bezel::{
    gpui::{
        self, App, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable, KeyBinding,
        KeyDownEvent, MouseButton, Pixels, Point, SharedString, Task, Window, div, prelude::*, px,
    },
    theme::{TextStyle, Theme, Typeset},
};
use std::time::Duration;
use terminal::{
    emulator::{Emulator, SelectionType},
    view::{
        GridGeometry, GridSnapshot, RESIZE_DEBOUNCE_MS, TERM_LINE_HEIGHT, TerminalElement, cell_at,
        keystroke_bytes, paste_bytes, terminal_panel_bg,
    },
};

gpui::actions!(terminal, [Copy, Paste]);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-c", Copy, Some("Terminal")),
        KeyBinding::new("cmd-v", Paste, Some("Terminal")),
        KeyBinding::new("ctrl-shift-c", Copy, Some("Terminal")),
        KeyBinding::new("ctrl-shift-v", Paste, Some("Terminal")),
    ]);
}

/// What the pane has for the shell: keys, a paste, or the emulator's own
/// answer to a query the shell sent it. The window puts it on the wire.
pub enum TerminalEvent {
    Input(Vec<u8>),
}

pub struct TerminalPane {
    /// The kernel's id for this shell (`t1`), which is what its output is
    /// carried under.
    id: String,
    emulator: Emulator,
    focus: FocusHandle,
    geometry: Option<GridGeometry>,
    selecting: bool,
    scroll_remainder: f32,
    /// How much of the shell's output has been fed to the emulator, counted
    /// from its first byte. What [`crate::model::pty::PtyStream::since`]
    /// takes to answer with the rest.
    cursor: u64,
    /// Whether keys go anywhere: false with the place's link down or the
    /// shell ended.
    live: bool,
    /// The kernel's words when the shell is not live. Nothing while it is.
    note: Option<SharedString>,
    resize: Option<Task<()>>,
}

impl EventEmitter<TerminalEvent> for TerminalPane {}

impl Focusable for TerminalPane {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TerminalPane {
    pub fn new(id: String, cx: &mut Context<Self>) -> Self {
        Self {
            id,
            emulator: Emulator::new(80, 24),
            focus: cx.focus_handle(),
            geometry: None,
            selecting: false,
            scroll_remainder: 0.,
            cursor: 0,
            live: true,
            note: None,
            resize: None,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// Everything the shell has written since this pane last looked. Called
    /// with the window's held bytes each time they change — and once when
    /// the pane is built, which is what puts the prompt on screen.
    pub fn feed(&mut self, bytes: &[u8], cursor: u64, cx: &mut Context<Self>) {
        if bytes.is_empty() {
            return;
        }
        self.cursor = cursor;
        let answer = self.emulator.feed(bytes);
        if !answer.is_empty() {
            cx.emit(TerminalEvent::Input(answer));
        }
        cx.notify();
    }

    /// How much of the shell's output the pane already holds.
    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    /// Whether the shell can still be typed into, and what to say when it
    /// cannot.
    pub fn set_state(&mut self, live: bool, note: Option<String>, cx: &mut Context<Self>) {
        let note = note.map(SharedString::from);
        if self.live == live && self.note == note {
            return;
        }
        self.live = live;
        self.note = note;
        cx.notify();
    }

    fn send(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        cx.emit(TerminalEvent::Input(bytes));
    }

    fn type_bytes(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        if !self.live {
            return;
        }
        self.emulator.clear_selection();
        self.emulator.scroll_to_bottom();
        self.send(bytes, cx);
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        if let Some(bytes) = keystroke_bytes(
            &key.key,
            key.key_char.as_deref(),
            &key.modifiers,
            self.emulator.app_cursor_mode(),
        ) {
            self.type_bytes(bytes, cx);
            cx.stop_propagation();
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.emulator.selection_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.type_bytes(paste_bytes(&text, self.emulator.bracketed_paste_mode()), cx);
        }
    }

    fn select(&mut self, position: Point<Pixels>, start: Option<SelectionType>) {
        let Some(grid) = self.geometry else {
            return;
        };
        let hit = cell_at(
            f32::from(position.x - grid.origin.x),
            f32::from(position.y - grid.origin.y),
            grid.cell_w,
            grid.line_h,
            grid.cols as usize,
            grid.rows as usize,
        );
        let point = self.emulator.grid_point(hit.row, hit.col);
        if let Some(kind) = start {
            self.emulator.start_selection(kind, point, hit.side);
        } else {
            self.emulator.update_selection(point, hit.side);
        }
    }

    fn snapshot(&mut self, geometry: GridGeometry, cx: &mut Context<Self>) -> GridSnapshot {
        self.geometry = Some(geometry);
        if self.emulator.cols() != geometry.cols as usize
            || self.emulator.rows() != geometry.rows as usize
        {
            self.emulator.resize(geometry.cols, geometry.rows);
            self.resize = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(RESIZE_DEBOUNCE_MS))
                    .await;
                let _ = this.update(cx, |this, _| {
                    let _ = this.emulator.cols();
                });
            }));
        }
        GridSnapshot {
            lines: self.emulator.lines(),
            cursor: self.emulator.cursor(),
        }
    }
}

impl Render for TerminalPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let weak = cx.entity().downgrade();
        let focused = self.focus.is_focused(window);
        div()
            .size_full()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(terminal_panel_bg(&theme))
            .child(
                div()
                    .id("terminal-grid")
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .track_focus(&self.focus)
                    .key_context("Terminal")
                    .cursor_text()
                    .on_action(cx.listener(Self::copy))
                    .on_action(cx.listener(Self::paste))
                    .on_key_down(cx.listener(Self::key_down))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                            window.focus(&this.focus, cx);
                            this.selecting = true;
                            let kind = match event.click_count {
                                2 => SelectionType::Semantic,
                                3.. => SelectionType::Lines,
                                _ => SelectionType::Simple,
                            };
                            this.select(event.position, Some(kind));
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                        if this.selecting && event.pressed_button == Some(MouseButton::Left) {
                            this.select(event.position, None);
                            cx.notify();
                        } else {
                            this.selecting = false;
                        }
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.selecting = false),
                    )
                    .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                        this.scroll_remainder +=
                            f32::from(event.delta.pixel_delta(px(TERM_LINE_HEIGHT)).y)
                                / TERM_LINE_HEIGHT;
                        let lines = this.scroll_remainder.trunc() as i32;
                        this.scroll_remainder -= lines as f32;
                        this.emulator.scroll(lines);
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .child(TerminalElement::new(
                        move |geometry, cx| {
                            weak.update(cx, |this, cx| this.snapshot(geometry, cx)).ok()
                        },
                        focused,
                    )),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(12.))
                    .py(px(6.))
                    .border_t_1()
                    .border_color(theme.border)
                    .text_style(TextStyle::Caption)
                    .text_color(theme.text_muted)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .children(self.note.clone()),
                    )
                    .child("⌘C copy · ⌘V paste"),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bezel::gpui::Modifiers;
    use std::sync::{Arc, Mutex};
    use terminal::emulator::CellColor;

    #[gpui::test]
    fn rendered_terminal_handles_keys_selection_and_paste(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            bezel::theme::appearance::init(bezel::theme::appearance::AppearanceMode::default(), cx);
            init(cx);
        });
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut pane = TerminalPane::new("t1".into(), cx);
            window.focus(&pane.focus, cx);
            pane
        });
        let typed: Arc<Mutex<Vec<u8>>> = Arc::default();
        let held = Arc::clone(&typed);
        let watch = cx.update(|_, cx| {
            cx.subscribe(&view, move |_, event: &TerminalEvent, _| {
                let TerminalEvent::Input(bytes) = event;
                held.lock().unwrap().extend_from_slice(bytes);
            })
        });
        // What the window hands a pane the moment it is built: the shell's
        // scrollback, prompt and all, taken off its own connection.
        view.update(cx, |pane, cx| pane.feed(b"hello world\r\n", 13, cx));
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |pane, _| pane.cursor()), 13);
        assert!(view.read_with(cx, |pane, _| pane.geometry.is_some()));
        typed.lock().unwrap().clear();
        cx.simulate_keystrokes("a space b enter ctrl-c up");
        cx.run_until_parked();
        assert_eq!(typed.lock().unwrap().as_slice(), b"a b\r\x03\x1b[A");
        let grid = view.read_with(cx, |pane, _| pane.geometry.unwrap());
        let position = grid.origin + gpui::point(px(grid.cell_w * 2.5), px(grid.line_h * 0.5));
        cx.simulate_mouse_move(position, None, Modifiers::default());
        cx.simulate_event(gpui::MouseDownEvent {
            position,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            click_count: 2,
            first_mouse: false,
        });
        cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());
        cx.simulate_keystrokes("cmd-c");
        cx.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some("hello".into())
            )
        });
        view.update(cx, |pane, cx| {
            pane.feed(b"\x1b[?2004h", 21, cx);
        });
        typed.lock().unwrap().clear();
        cx.simulate_keystrokes("cmd-v");
        cx.run_until_parked();
        assert_eq!(typed.lock().unwrap().as_slice(), b"\x1b[200~hello\x1b[201~");
        // A shell that has ended takes no more keys.
        view.update(cx, |pane, cx| {
            pane.set_state(false, Some("shell gone".into()), cx)
        });
        typed.lock().unwrap().clear();
        cx.simulate_keystrokes("x");
        cx.run_until_parked();
        assert!(typed.lock().unwrap().is_empty());
        drop(watch);
    }

    #[test]
    fn ansi_output_cursor_and_query_responses() {
        let mut emulator = Emulator::new(80, 24);
        emulator.feed(b"\x1b[32mready\x1b[0m\r\n");
        assert!(emulator.row_text(0).starts_with("ready"));
        assert_eq!(emulator.line(0)[0].fg, CellColor::Indexed(2));
        assert_eq!(emulator.feed(b"\x1b[6n"), b"\x1b[2;1R");
        emulator.resize(120, 40);
        assert_eq!((emulator.cols(), emulator.rows()), (120, 40));
    }

    #[test]
    fn shell_keys_and_bracketed_paste() {
        assert_eq!(
            keystroke_bytes(
                "c",
                None,
                &Modifiers {
                    control: true,
                    ..Default::default()
                },
                false
            ),
            Some(vec![3])
        );
        assert_eq!(
            keystroke_bytes("up", None, &Modifiers::default(), true),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            paste_bytes("echo ready", true),
            b"\x1b[200~echo ready\x1b[201~"
        );
    }
}
