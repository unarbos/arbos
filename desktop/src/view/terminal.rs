use crate::{agent::acp, kernel, model::place::Place};
use arbos_core::wire::Frame;
use bezel::{
    gpui::{
        self, App, ClipboardItem, Context, FocusHandle, Focusable, KeyBinding, KeyDownEvent,
        MouseButton, Pixels, Point, Task, Window, div, prelude::*, px,
    },
    theme::{TextStyle, Theme, Typeset},
    ui::widgets::{ButtonStyle, Buttons},
};
use std::time::Duration;
use terminal::{
    emulator::{Emulator, SelectionType},
    view::{
        GridGeometry, GridSnapshot, RESIZE_DEBOUNCE_MS, TERM_LINE_HEIGHT, TerminalElement, cell_at,
        keystroke_bytes, paste_bytes, terminal_panel_bg,
    },
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    sync::mpsc,
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

enum Event {
    Connected,
    Output(Vec<u8>),
    Closed(String),
}

fn encode_bytes(bytes: &[u8]) -> String {
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
}

async fn connect(
    place: Place,
    id: String,
    mut input: mpsc::UnboundedReceiver<Vec<u8>>,
    output: mpsc::UnboundedSender<Event>,
) -> anyhow::Result<()> {
    let info = tokio::task::spawn_blocking({
        let place = place.clone();
        move || kernel::attach_or_spawn_place(&place)
    })
    .await??;
    let addr = kernel::tcp_addr(&info.url).ok_or_else(|| anyhow::anyhow!("bad kernel url"))?;
    let stream = tokio::time::timeout(Duration::from_secs(15), TcpStream::connect(addr)).await??;
    let (reader, mut writer) = stream.into_split();
    let _ = output.send(Event::Connected);
    let _ = writer
        .write_all(
            serde_json::to_string(&Frame::PtyIn {
                agent: "root".into(),
                page: id.clone(),
                data: encode_bytes(&[]),
            })?
            .as_bytes(),
        )
        .await;
    let _ = writer.write_all(b"\n").await;
    let mut lines = BufReader::new(reader).lines();
    loop {
        tokio::select! {
            line = lines.next_line() => match line {
                Ok(Some(text)) => {
                    if let Ok(Frame::Pty { page, data, .. }) = serde_json::from_str(&text) {
                        if page == id {
                            if let Ok(bytes) = base64::Engine::decode(
                                &base64::engine::general_purpose::STANDARD,
                                data,
                            ) {
                                if output.send(Event::Output(bytes)).is_err() { break; }
                            }
                        }
                    }
                }
                Ok(None) => break,
                Err(err) => return Err(err.into()),
            },
            message = input.recv() => match message {
                Some(bytes) => {
                    let frame = Frame::PtyIn {
                        agent: "root".into(),
                        page: id.clone(),
                        data: encode_bytes(&bytes),
                    };
                    let line = serde_json::to_string(&frame)?;
                    writer.write_all(line.as_bytes()).await?;
                    writer.write_all(b"\n").await?;
                }
                None => return Ok(()),
            }
        }
    }
    let _ = output.send(Event::Closed("Disconnected".into()));
    Ok(())
}

pub struct TerminalPane {
    place: Place,
    id: String,
    emulator: Emulator,
    focus: FocusHandle,
    geometry: Option<GridGeometry>,
    selecting: bool,
    scroll_remainder: f32,
    input: Option<mpsc::UnboundedSender<Vec<u8>>>,
    connection: Option<tokio::task::JoinHandle<()>>,
    reader: Option<Task<()>>,
    resize: Option<Task<()>>,
    status: String,
    connected: bool,
    closed: bool,
}

impl Drop for TerminalPane {
    fn drop(&mut self) {
        if let Some(task) = self.connection.take() {
            task.abort();
        }
    }
}

impl Focusable for TerminalPane {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TerminalPane {
    pub fn new(place: Place, id: String, cx: &mut Context<Self>) -> Self {
        let mut pane = Self::disconnected(place, id, cx);
        pane.attach(cx);
        pane
    }

    fn disconnected(place: Place, id: String, cx: &mut Context<Self>) -> Self {
        Self {
            place,
            id,
            emulator: Emulator::new(80, 24),
            focus: cx.focus_handle(),
            geometry: None,
            selecting: false,
            scroll_remainder: 0.,
            input: None,
            connection: None,
            reader: None,
            resize: None,
            status: "Connecting…".into(),
            connected: false,
            closed: false,
        }
    }

    fn attach(&mut self, cx: &mut Context<Self>) {
        if let Some(task) = self.connection.take() {
            task.abort();
        }
        self.resize = None;
        let cols = self.emulator.cols() as u16;
        let rows = self.emulator.rows() as u16;
        self.emulator = Emulator::new(cols, rows);
        self.connected = false;
        self.closed = false;
        self.status = "Connecting…".into();
        let (input, receiver) = mpsc::unbounded_channel();
        let (output, mut events) = mpsc::unbounded_channel();
        let place = self.place.clone();
        let id = self.id.clone();
        self.input = Some(input);
        self.connection = Some(acp::runtime().spawn(async move {
            if let Err(error) = connect(place, id, receiver, output.clone()).await {
                let _ = output.send(Event::Closed(format!("Connection failed: {error}")));
            }
        }));
        self.reader = Some(cx.spawn(async move |this, cx| {
            while let Some(event) = events.recv().await {
                if this
                    .update(cx, |this, cx| {
                        match event {
                            Event::Connected => {
                                this.connected = true;
                                this.status = "Connected".into();
                            }
                            Event::Output(bytes) => {
                                let response = this.emulator.feed(&bytes);
                                if !response.is_empty() {
                                    this.send(response);
                                }
                            }
                            Event::Closed(reason) => {
                                this.connected = false;
                                this.closed = true;
                                this.input = None;
                                this.status = reason;
                            }
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn send(&self, message: Vec<u8>) {
        if let Some(input) = &self.input {
            let _ = input.send(message);
        }
    }

    fn type_bytes(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        if !self.connected {
            return;
        }
        self.emulator.clear_selection();
        self.emulator.scroll_to_bottom();
        self.send(bytes);
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
                            .child(self.status.clone()),
                    )
                    .when(self.closed, |row| {
                        row.child(
                            theme
                                .button("Reconnect", ButtonStyle::Ghost, None)
                                .id("terminal-reconnect")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.attach(cx);
                                    window.focus(&this.focus, cx);
                                })),
                        )
                    })
                    .child("⌘C copy · ⌘V paste"),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bezel::gpui::Modifiers;
    use terminal::emulator::CellColor;

    #[gpui::test]
    fn rendered_terminal_handles_keys_selection_and_paste(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            bezel::theme::appearance::init(bezel::theme::appearance::AppearanceMode::default(), cx);
            init(cx);
        });
        let (input, mut received) = mpsc::unbounded_channel();
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut pane = TerminalPane::disconnected(Place::default(), "test".into(), cx);
            pane.input = Some(input);
            pane.connected = true;
            pane.emulator.feed(b"hello world\r\n");
            window.focus(&pane.focus, cx);
            pane
        });
        cx.run_until_parked();
        assert!(view.read_with(cx, |pane, _| pane.geometry.is_some()));
        while received.try_recv().is_ok() {}
        cx.simulate_keystrokes("a space b enter ctrl-c up");
        let mut bytes = Vec::new();
        while let Ok(data) = received.try_recv() {
            bytes.extend_from_slice(&data);
        }
        assert_eq!(bytes, b"a b\r\x03\x1b[A");
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
        view.update(cx, |pane, _| {
            pane.emulator.feed(b"\x1b[?2004h");
        });
        cx.simulate_keystrokes("cmd-v");
        let mut pasted = Vec::new();
        while let Ok(data) = received.try_recv() {
            pasted.extend_from_slice(&data);
        }
        assert_eq!(pasted, b"\x1b[200~hello\x1b[201~");
    }

    #[test]
    fn attach_url_is_tcp() {
        let info = kernel::WebInfo {
            url: "tcp://127.0.0.1:9".into(),
            pid: 0,
            started: 0,
        };
        assert!(kernel::tcp_addr(&info.url).is_some());
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
