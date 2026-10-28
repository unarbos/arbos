//! D82: the first-login screen (state in `crate::onboarding`). A terminal
//! panel types muted token bars behind a prompt while the relay builds the
//! user's computer and first agent; the hero text underneath says what is
//! happening. On finish the panel morphs into the agent's greeting bubble
//! where the transcript draws it, and the shell fades in underneath.
//! GPUI primitives only (quads and one stroked path), 30 fps via PulseClock.

use std::time::Instant;

use gpui::{
    canvas, div, fill, prelude::*, px, AnyElement, Bounds, Context, Entity, FontWeight, Hsla,
    PathBuilder, Pixels, Point, Window,
};

use crate::app::AppState;
use crate::onboarding::{Onboarding, Stage};
use crate::ui::motion::PulseClock;
use crate::ui::shell::Shell;
use crate::ui::theme::{Theme, BUBBLE_RADIUS, COLUMN_MARGIN, HEADER_HEIGHT, TEXT_BODY, TEXT_SMALL};
use crate::ui::transcript::{assistant_cap, COLUMN_MAX_WIDTH};

const CARD_W: f32 = 256.0;
const CARD_H: f32 = 172.0;
const CARD_R: f32 = 22.0;
const LINES: usize = 6;
const LOOP_S: f32 = crate::onboarding::LOOP_MS as f32 / 1000.0;
const PER_LINE: f32 = LOOP_S / LINES as f32;
const SLIDE: f32 = 14.0 / 60.0;
const TYPE_START: f32 = 14.0 / 60.0;
const TYPE_END: f32 = 38.0 / 60.0;
const FADE_START: f32 = 200.0 / 60.0;
const FADE_END: f32 = 232.0 / 60.0;
const PITCH: f32 = 24.0;
const BAR_H: f32 = 8.0;

/// (offset, width, alpha) per bar; six "token" lines of different shapes.
const PATTERNS: [&[(f32, f32, f32)]; LINES] = [
    &[(0.0, 44.0, 0.42), (52.0, 78.0, 0.20)],
    &[(0.0, 30.0, 0.42), (38.0, 110.0, 0.20)],
    &[(0.0, 56.0, 0.42), (64.0, 40.0, 0.20), (112.0, 52.0, 0.20)],
    &[(0.0, 38.0, 0.42), (46.0, 92.0, 0.20)],
    &[(0.0, 64.0, 0.42), (72.0, 66.0, 0.20)],
    &[(0.0, 26.0, 0.42), (34.0, 124.0, 0.20)],
];

fn slot_y(k: usize) -> f32 {
    52.0 - PITCH * k as f32
}

fn smooth(p: f32) -> f32 {
    let p = p.clamp(0.0, 1.0);
    p * p * (3.0 - 2.0 * p)
}

/// 0 before `a`, 1 after `b`, eased in between.
fn ramp(p: f32, a: f32, b: f32) -> f32 {
    smooth((p - a) / (b - a))
}

/// Material standard curve, cubic-bezier(0.4, 0, 0.2, 1).
fn ease(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    let (x1, y1, x2, y2) = (0.4f32, 0.0f32, 0.2f32, 1.0f32);
    let mut t = x;
    for _ in 0..10 {
        let cx = 3.0 * x1 * (1.0 - t) * (1.0 - t) * t + 3.0 * x2 * (1.0 - t) * t * t + t * t * t;
        let dx = 3.0 * (1.0 - t) * (1.0 - t) * x1
            + 6.0 * (1.0 - t) * t * (x2 - x1)
            + 3.0 * t * t * (1.0 - x2);
        if dx.abs() < 1e-6 {
            break;
        }
        t = (t - (cx - x) / dx).clamp(0.0, 1.0);
    }
    3.0 * y1 * (1.0 - t) * (1.0 - t) * t + 3.0 * y2 * (1.0 - t) * t * t + t * t * t
}

fn lerp_bounds(a: Bounds<Pixels>, b: Bounds<Pixels>, t: f32) -> Bounds<Pixels> {
    let l = |x: Pixels, y: Pixels| x + (y - x) * t;
    Bounds {
        origin: Point::new(l(a.origin.x, b.origin.x), l(a.origin.y, b.origin.y)),
        size: gpui::Size {
            width: l(a.size.width, b.size.width),
            height: l(a.size.height, b.size.height),
        },
    }
}

fn alpha(c: Hsla, a: f32) -> Hsla {
    Theme::with_alpha(c, a.clamp(0.0, 1.0))
}

/// The greeting bubble as the transcript lays it out: the transcript is
/// bottom-anchored, so the only message sits just above the composer, left
/// in the chat column (the composer's own: 16 pt margins, 960 cap — on a
/// 1180×780 window left 312; bottom 74.5 above the window edge, 42.5 tall
/// for one line).
fn bubble_target(viewport: gpui::Size<Pixels>, sidebar_width: f32, text_px: f32) -> Bounds<Pixels> {
    let left = sidebar_width;
    let center_w = f32::from(viewport.width) - left;
    let column_w = (center_w - 2.0 * COLUMN_MARGIN).min(COLUMN_MAX_WIDTH);
    let column_x = left + (center_w - column_w) / 2.0;
    let bubble_cap = assistant_cap(column_w);
    let width = (text_px + 28.0).min(bubble_cap);
    let lines = (text_px / (bubble_cap - 28.0)).ceil().max(1.0);
    let height = 22.0 + 21.0 * lines;
    let bottom = f32::from(viewport.height) - 74.5;
    Bounds {
        origin: Point::new(px(column_x), px((bottom - height).max(HEADER_HEIGHT + 8.0))),
        size: gpui::Size {
            width: px(width),
            height: px(height),
        },
    }
}

pub fn render(
    shell: &Shell,
    state: &Entity<AppState>,
    o: &Onboarding,
    window: &Window,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let theme = Theme::of(cx).clone();
    PulseClock::request(cx);
    let now = Instant::now();
    let viewport = window.viewport_size();
    let vw = f32::from(viewport.width);
    let vh = f32::from(viewport.height);
    let card_center = (vw / 2.0, vh * 0.42);
    let loop_rect = Bounds {
        origin: Point::new(
            px(card_center.0 - CARD_W / 2.0),
            px(card_center.1 - CARD_H / 2.0),
        ),
        size: gpui::Size {
            width: px(CARD_W),
            height: px(CARD_H),
        },
    };
    let greeting = format!(
        "Hi, I’m {}. Your computer is ready — what should we build first?",
        o.agent_name
    );
    let text_px = greeting.chars().count() as f32 * 6.75;
    let sidebar_width = {
        let s = state.read(cx);
        if s.rail_collapsed || s.window_is_narrow(vw) {
            0.0
        } else {
            s.sidebar_width
        }
    };
    let target = bubble_target(viewport, sidebar_width, text_px);

    let p = o.finish_progress(now).map(ease);
    let reduce = o.reduce_motion;
    let morph = if reduce {
        0.0
    } else {
        p.map(|p| ramp(p, 0.15, 0.5)).unwrap_or(0.0)
    };
    let content_alpha = if reduce {
        1.0
    } else {
        p.map(|p| 1.0 - ramp(p, 0.0, 0.25)).unwrap_or(1.0)
    };
    let greeting_alpha = if reduce {
        0.0
    } else {
        p.map(|p| ramp(p, 0.5, 0.7)).unwrap_or(0.0)
    };
    let page_alpha = p.map(|p| 1.0 - ramp(p, 0.6, 1.0)).unwrap_or(1.0);
    let whole_alpha = if reduce {
        p.map(|p| 1.0 - p).unwrap_or(1.0)
    } else {
        1.0
    };
    let t = if reduce {
        2.0
    } else {
        now.duration_since(o.started_at).as_secs_f32()
    };
    let rect = lerp_bounds(loop_rect, target, morph);
    let radius = CARD_R + (BUBBLE_RADIUS - CARD_R) * morph;

    let ink = theme.text;
    let dark = theme.appearance == crate::ui::theme::Appearance::Dark;
    let surface = theme.bubble_assistant;
    let shadow_alpha = if dark { 0.30 } else { 0.05 } * (1.0 - morph) * whole_alpha;
    let frozen = reduce;

    let panel = canvas(
        move |_, _, _| (),
        move |_, _, window, _| {
            // Soft shadow, then the panel itself.
            if shadow_alpha > 0.0 {
                let mut sb = rect;
                sb.origin.x += px(8.0);
                sb.origin.y += px(12.0);
                sb.size.width -= px(16.0);
                sb.size.height -= px(8.0);
                window.paint_quad(
                    fill(sb, alpha(gpui::black(), shadow_alpha)).corner_radii(px(radius)),
                );
            }
            window.paint_quad(fill(rect, alpha(surface, whole_alpha)).corner_radii(px(radius)));
            if content_alpha > 0.0 {
                let mut clip = loop_rect;
                clip.origin.x += px(4.0);
                clip.origin.y += px(4.0);
                clip.size.width -= px(8.0);
                clip.size.height -= px(8.0);
                window.with_content_mask(Some(gpui::ContentMask { bounds: clip }), |window| {
                    paint_terminal(
                        window,
                        loop_rect,
                        t,
                        ink,
                        dark,
                        frozen,
                        content_alpha * whole_alpha,
                    );
                });
            }
        },
    )
    .absolute()
    .inset_0();

    let mut root = div()
        .absolute()
        .inset_0()
        .bg(alpha(theme.bg, page_alpha))
        .child(panel);

    if greeting_alpha > 0.0 {
        root = root.child(
            div()
                .absolute()
                .left(target.origin.x)
                .top(target.origin.y)
                .w(target.size.width)
                .h(target.size.height)
                .px(px(14.0))
                .py(px(11.0))
                .text_size(px(TEXT_BODY))
                .line_height(px(21.0))
                .text_color(alpha(theme.text, greeting_alpha))
                .child(greeting.clone()),
        );
    }

    if o.stage != Stage::Finishing {
        let since = now.duration_since(o.stage_since).as_secs_f32();
        let title_alpha = if reduce {
            1.0
        } else {
            (since / 0.35).clamp(0.0, 1.0)
        };
        let title_dy = if reduce {
            0.0
        } else {
            6.0 * (1.0 - smooth(title_alpha))
        };
        let block_top = card_center.1 + CARD_H / 2.0 + 44.0;
        let mut block = div()
            .absolute()
            .left_0()
            .right_0()
            .top(px(block_top))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.0));
        if o.stage == Stage::Failed {
            let st = state.clone();
            block = block
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_size(px(TEXT_BODY))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child(crate::ui::icons::icon("window", 14.0, theme.text))
                        .child(o.stage.title()),
                )
                .when_some(o.error.clone(), |el, err| {
                    el.child(
                        div()
                            .max_w(px(420.0))
                            .text_size(px(TEXT_SMALL))
                            .text_color(theme.text_muted)
                            .text_center()
                            .child(err),
                    )
                })
                .child(
                    div()
                        .id("onboarding-retry")
                        .mt(px(8.0))
                        .px(px(18.0))
                        .py(px(7.0))
                        .rounded_full()
                        .bg(theme.field)
                        .border_1()
                        .border_color(theme.field_border)
                        .text_size(px(TEXT_SMALL))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .cursor_pointer()
                        .hover(|el| el.opacity(0.88))
                        .child("Retry")
                        .on_click(move |_, _, cx| {
                            st.update(cx, |s, cx| s.retry_onboarding(cx));
                        }),
                );
        } else {
            block = block
                .child(
                    div()
                        .mt(px(title_dy))
                        .text_size(px(22.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(alpha(theme.text, title_alpha))
                        .child(o.stage.title()),
                )
                .child(
                    div()
                        .max_w(px(460.0))
                        .text_size(px(TEXT_SMALL))
                        .text_color(theme.text_muted)
                        .text_center()
                        .child(o.detail.clone().unwrap_or_default()),
                );
        }
        root = root.child(block);
    }
    let _ = shell;
    root.into_any_element()
}

/// The terminal's bars, prompt and cursor at loop time `t` (same numbers as the
/// approved Lottie concept `terminal-boot`: 4 s loop, six lines, a new line
/// every 0.667 s, 24 px pitch, linear typing, fade at the top).
fn paint_terminal(
    window: &mut Window,
    card: Bounds<Pixels>,
    t: f32,
    ink: Hsla,
    dark: bool,
    frozen: bool,
    a: f32,
) {
    let cx = f32::from(card.origin.x) + CARD_W / 2.0;
    let cy = f32::from(card.origin.y) + CARD_H / 2.0;
    let x0 = -CARD_W / 2.0 + 22.0;
    let bar_x = x0 + 18.0;
    let alpha_scale = if dark { 1.15 } else { 1.0 };

    for (i, segs) in PATTERNS.iter().enumerate() {
        let tau = (t + i as f32 * PER_LINE) % LOOP_S;
        let k = ((tau / PER_LINE) as usize).min(LINES - 1);
        let mut y = slot_y(k);
        if k >= 1 {
            let s = (tau - k as f32 * PER_LINE) / SLIDE;
            if s < 1.0 {
                y = slot_y(k - 1) + (slot_y(k) - slot_y(k - 1)) * ease(s.max(0.0));
            }
        }
        let mut line_alpha = 1.0;
        if tau > FADE_START {
            line_alpha = 1.0 - smooth((tau - FADE_START) / (FADE_END - FADE_START));
        }
        if line_alpha <= 0.0 {
            continue;
        }
        let total = segs.last().map(|s| s.0 + s.1).unwrap_or(1.0);
        let typed = if frozen || k > 0 {
            total
        } else if tau <= TYPE_START {
            0.0
        } else {
            total * ((tau - TYPE_START) / (TYPE_END - TYPE_START)).min(1.0)
        };
        for (off, w, seg_a) in segs.iter() {
            let visible = (typed - off).clamp(0.0, *w);
            if visible <= 0.0 {
                continue;
            }
            let b = Bounds {
                origin: Point::new(px(cx + bar_x + off), px(cy + y - BAR_H / 2.0)),
                size: gpui::Size {
                    width: px(visible),
                    height: px(BAR_H),
                },
            };
            window.paint_quad(
                fill(b, alpha(ink, seg_a * alpha_scale * line_alpha * a)).corner_radii(px(4.0)),
            );
        }
    }

    // Prompt chevron on the active slot.
    let mut pb = PathBuilder::stroke(px(2.0));
    pb.move_to(Point::new(px(cx + x0 + 2.0), px(cy + slot_y(0) - 5.0)));
    pb.line_to(Point::new(px(cx + x0 + 7.0), px(cy + slot_y(0))));
    pb.line_to(Point::new(px(cx + x0 + 2.0), px(cy + slot_y(0) + 5.0)));
    if let Ok(path) = pb.build() {
        window.paint_path(path, alpha(ink, if dark { 0.55 } else { 0.45 } * a));
    }

    // Block cursor: follows the active line's typing, blinks twice a second.
    let active = (LINES - ((t / PER_LINE) as usize % LINES)) % LINES;
    let tau_a = t % PER_LINE;
    let total_a = PATTERNS[active].last().map(|s| s.0 + s.1).unwrap_or(0.0);
    let progress = if frozen {
        1.0
    } else if tau_a <= TYPE_START {
        0.0
    } else {
        ((tau_a - TYPE_START) / (TYPE_END - TYPE_START)).min(1.0)
    };
    let on = frozen || ((t * 2.0) as i32) % 2 == 0;
    if on {
        let b = Bounds {
            origin: Point::new(
                px(cx + bar_x + total_a * progress + 6.0 * progress + 4.0),
                px(cy + slot_y(0) - 7.0),
            ),
            size: gpui::Size {
                width: px(8.0),
                height: px(14.0),
            },
        };
        window.paint_quad(
            fill(b, alpha(ink, if dark { 0.85 } else { 0.7 } * a)).corner_radii(px(2.0)),
        );
    }
}
