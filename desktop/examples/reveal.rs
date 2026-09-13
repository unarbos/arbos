//! Prose that rises into place one painted line at a time — the web client's
//! kugiri reveal, native. A stage for judging the motion before it reaches the
//! transcript.
//!
//! ```sh
//! cargo run --example reveal
//! ```
//!
//! Keys: `R` replay the settled answer · `S` stream it word by word ·
//! `1` `2` `3` pace · `F` fade veil on and off · `Q` quit.

use bezel::{
    gpui::{
        App, Bounds, Context, FocusHandle, KeyDownEvent, Render, TitlebarOptions, Window,
        WindowBounds, WindowOptions, div, point, prelude::*, px, size,
    },
    gpui_platform,
    theme::{
        Theme,
        appearance::{self, AppearanceMode},
    },
    ui,
};
use markdown::{Editing, Pace, Reveal};
use std::time::Duration;

const WIDTH: f32 = 880.0;
const HEIGHT: f32 = 640.0;
const COLUMN: f32 = 600.0;
/// One delta of the simulated stream.
const WORD_TICK: Duration = Duration::from_millis(70);

const SAMPLE: &str = "\
Here is what I found. The reveal reads line breaks back from the layout \
engine, so the motion follows the lines the text system painted and never \
lines it guessed at. Each line rises out of its own clip, one after another, \
and holds still once it lands.

- **Line by line:** `LineReveal` wraps one paragraph and clips each painted \
line, so what moves is exactly what the text system laid out.
- **Remembered:** `RevealClock` remembers when every line first appeared, \
and a line seen once never rises again.
- **Reduced motion:** the text paints where it stands.

A block that lands whole staggers from its first line. A block still \
streaming rises one new line at a time while the lines already read hold \
still, which is the case the web client skips.

Press **R** to replay, **S** to stream, **1 2 3** for the pace, **F** for the veil.";

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Settled,
    Streaming,
}

#[derive(Clone, Copy, PartialEq)]
enum Preset {
    Web,
    Tight,
    Loose,
}

impl Preset {
    fn pace(self) -> Pace {
        let (duration, stagger) = match self {
            Preset::Web => (700, 55),
            Preset::Tight => (450, 40),
            Preset::Loose => (900, 80),
        };
        Pace {
            duration: Duration::from_millis(duration),
            stagger: Duration::from_millis(stagger),
            ..Pace::default()
        }
    }

    fn label(self) -> &'static str {
        match self {
            Preset::Web => "1 · web (700 / 55)",
            Preset::Tight => "2 · tight (450 / 40)",
            Preset::Loose => "3 · loose (900 / 80)",
        }
    }
}

struct Stage {
    focus: FocusHandle,
    mode: Mode,
    preset: Preset,
    veil: bool,
    reveal: Reveal,
    /// Words of [`SAMPLE`] shown so far while streaming.
    shown: usize,
    /// Bumped to cancel a stream in flight.
    generation: u64,
}

fn words() -> Vec<&'static str> {
    SAMPLE.split_inclusive(' ').collect()
}

impl Stage {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            mode: Mode::Settled,
            preset: Preset::Web,
            veil: false,
            reveal: Reveal::new(Preset::Web.pace()),
            shown: 0,
            generation: 0,
        }
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let veil = self.veil.then(|| Theme::of(cx).bg);
        self.reveal = Reveal::new(self.preset.pace()).with_veil(veil);
        match self.mode {
            Mode::Settled => cx.notify(),
            Mode::Streaming => self.stream(cx),
        }
    }

    fn stream(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.shown = 0;
        let generation = self.generation;
        let total = words().len();
        cx.notify();
        cx.spawn(async move |this, cx| {
            // Chunks of one to six words, as a model's deltas come: not one
            // word at a time, and not evenly.
            let mut seed: u32 = 0x9e37_79b9;
            let mut shown = 0;
            while shown < total {
                cx.background_executor().timer(WORD_TICK).await;
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                shown = (shown + 1 + (seed >> 28) as usize % 6).min(total);
                let live = this.update(cx, |stage, cx| {
                    if stage.generation != generation {
                        return false;
                    }
                    stage.shown = shown;
                    cx.notify();
                    true
                });
                if !matches!(live, Ok(true)) {
                    return;
                }
            }
        })
        .detach();
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "r" => {
                self.mode = Mode::Settled;
                self.rebuild(cx);
            }
            "s" => {
                self.mode = Mode::Streaming;
                self.rebuild(cx);
            }
            "1" => {
                self.preset = Preset::Web;
                self.rebuild(cx);
            }
            "2" => {
                self.preset = Preset::Tight;
                self.rebuild(cx);
            }
            "3" => {
                self.preset = Preset::Loose;
                self.rebuild(cx);
            }
            "f" => {
                self.veil = !self.veil;
                self.rebuild(cx);
            }
            "q" => cx.quit(),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn text(&self) -> String {
        match self.mode {
            Mode::Settled => SAMPLE.to_string(),
            Mode::Streaming => words()[..self.shown].concat(),
        }
    }
}

impl Render for Stage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let text = self.text();
        let doc = markdown::parse(&text);
        let prose = markdown::render_revealed(&doc, Editing::default(), &self.reveal, window, cx);
        let mode = match self.mode {
            Mode::Settled => "R · settled",
            Mode::Streaming => "S · streaming",
        };
        let veil = if self.veil {
            "F · veil on"
        } else {
            "F · veil off"
        };
        let state = if self.reveal.is_settled() {
            "at rest"
        } else {
            "rising"
        };
        let status = format!("{mode}     {}     {veil}     {state}", self.preset.label());

        div()
            .size_full()
            .bg(theme.bg)
            .text_color(theme.text)
            .track_focus(&self.focus)
            .key_context("Reveal")
            .on_key_down(cx.listener(Self::key_down))
            .flex()
            .flex_col()
            .items_center()
            .pt(px(48.0))
            .child(
                div()
                    .w(px(COLUMN))
                    .text_size(px(12.0))
                    .text_color(theme.text_muted)
                    .font_family(theme.font_mono.clone())
                    .child(status),
            )
            .child(div().w(px(COLUMN)).mt(px(32.0)).child(prose))
    }
}

fn main() {
    let app = gpui_platform::application();
    app.run(|cx: &mut App| {
        if let Err(err) = ui::register_fonts(cx) {
            eprintln!("font registration failed: {err:?}");
        }
        appearance::init(AppearanceMode::System, cx);
        let bounds = Bounds::centered(None, size(px(WIDTH), px(HEIGHT)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Reveal".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(12.0), px(12.0))),
                }),
                ..Default::default()
            },
            |window, cx| {
                let stage = cx.new(Stage::new);
                let focus = stage.read(cx).focus.clone();
                window.focus(&focus, cx);
                stage
            },
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
