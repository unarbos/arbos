//! Chart bubbles ([feedback chart-bubble-mac-2026-10-26]): the card an agent's `chart` part
//! draws inside its bubble — the code card's frame (`code_bg`, hairline, radius 10, the 24 pt
//! icon squares top-right: Copy data · Save PNG · Expand), a title and subtitle, the legend, and
//! a canvas that paints `arbos_core::chart`'s layout with GPUI primitives. Hover shows a
//! crosshair with every series' value at that x (touch-and-hold on the iPhone). Nothing is
//! computed here but pixels: scales, ticks, formats and the hit test come from the core, so the
//! Mac, the iPhone and the fallback picture agree on every number.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    canvas, div, fill, point, prelude::*, px, quad, size, AnyElement, App, BorderStyle, Bounds,
    Corners, Edges, FontWeight, Hsla, PathBuilder, Pixels, Point, SharedString, TextAlign, TextRun,
    Window,
};

use arbos_core::chart::{
    AnnotationShape, Chart, ChartModel, Kind, LayoutStyle, LineStyle, PlotLayout, Pt, Readout,
    SeriesShape, PALETTE_DARK, PALETTE_LIGHT,
};

use super::icons::icon;
use super::theme::{Appearance, Theme};
use super::transcript::CodeCopy;

/// The card's geometry (the code card's, #1970).
pub const CARD_RADIUS: f32 = 10.0;
pub const CARD_PAD: f32 = 12.0;
pub const ICON_SQUARE: f32 = 24.0;
pub const ICON_GLYPH: f32 = 13.0;
/// Plot heights inside a bubble card.
pub const PLOT_HEIGHT: f32 = 210.0;
pub const BAR_PLOT_HEIGHT: f32 = 200.0;
pub const PIE_PLOT_HEIGHT: f32 = 220.0;
/// Room above the plot for the horizontal axis title (the rotated label of the picture).
const Y_TITLE_H: f32 = 16.0;

/// A parsed, resolved chart; cached per spec so hover repaints cost pixels only.
pub struct Prepared {
    pub chart: Chart,
    pub model: ChartModel,
}

thread_local! {
    static CACHE: RefCell<HashMap<u64, Rc<Prepared>>> = RefCell::new(HashMap::new());
}

/// Parse + resolve `spec` (the `chart` part's JSON). `None` when it is not a valid chart
/// (agentd validated it, so this is a newer spec than this app knows).
pub fn prepare(spec: &str) -> Option<Rc<Prepared>> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    spec.hash(&mut h);
    let key = h.finish();
    if let Some(p) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return Some(p);
    }
    let value: serde_json::Value = serde_json::from_str(spec).ok()?;
    let chart = Chart::parse(&value).ok()?;
    let model = chart.model();
    let prepared = Rc::new(Prepared { chart, model });
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= 48 {
            c.clear();
        }
        c.insert(key, prepared.clone());
    });
    Some(prepared)
}

/// "Chart: <title>" for quote headers and one-line previews.
pub fn one_line_title(spec: &str) -> String {
    prepare(spec)
        .map(|p| p.chart.preview_text())
        .unwrap_or_else(|| "Chart".into())
}

/// A card action with no arguments (hover repaint).
pub type RepaintFn = Rc<dyn Fn(&mut Window, &mut App)>;
/// A card action with two strings: `(spec, title)` for Save PNG, `(part id, spec)` for Expand.
pub type CardFn = Rc<dyn Fn(String, String, &mut Window, &mut App)>;

/// The transcript's wiring for the cards' actions (passed with the code cards' `CodeCopy`).
#[derive(Clone)]
pub struct ChartUi {
    /// Pointer moved over or left a plot: repaint (the crosshair follows the mouse).
    pub on_hover: RepaintFn,
    /// Save PNG: `(spec, title)`.
    pub on_save: CardFn,
    /// Expand: `(part id, spec)`.
    pub on_expand: CardFn,
}

/// The series colours for a theme.
pub fn palette(theme: &Theme) -> &'static [&'static str] {
    match theme.appearance {
        Appearance::Dark => &PALETTE_DARK,
        Appearance::Light => &PALETTE_LIGHT,
    }
}

/// `#RRGGBB` → colour; the fallback for anything else.
pub fn hex_color(s: &str, fallback: Hsla) -> Hsla {
    let hex = s.trim_start_matches('#');
    if hex.len() != 6 {
        return fallback;
    }
    match u32::from_str_radix(hex, 16) {
        Ok(v) => gpui::rgb(v).into(),
        Err(_) => fallback,
    }
}

fn series_color(model: &ChartModel, i: usize, theme: &Theme) -> Hsla {
    hex_color(&model.series_color(i, palette(theme)), theme.accent)
}

fn slice_color(model: &ChartModel, i: usize, theme: &Theme) -> Hsla {
    match model.series.first().and_then(|s| s.color.as_deref()) {
        Some(c) => hex_color(c, theme.accent),
        None => {
            let p = palette(theme);
            hex_color(p[i % p.len()], theme.accent)
        }
    }
}

/// The plot height for a chart kind inside a bubble card.
pub fn plot_height(kind: Kind) -> f32 {
    match kind {
        Kind::Bar => BAR_PLOT_HEIGHT,
        Kind::Pie => PIE_PLOT_HEIGHT,
        Kind::Line | Kind::Area | Kind::Scatter => PLOT_HEIGHT,
    }
}

/// The 24 pt icon square of the code card (hairline, 6 pt radius, muted glyph).
pub fn icon_square(
    id: SharedString,
    glyph: &'static str,
    tint: Option<Hsla>,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let color = tint.unwrap_or(theme.text_muted);
    let hover_bg = theme.surface_2;
    let selector = id.to_string();
    div()
        .id(id)
        .debug_selector(move || selector.clone())
        .size(px(ICON_SQUARE))
        .rounded(px(6.0))
        .border_1()
        .border_color(theme.border)
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
        .child(icon(glyph, ICON_GLYPH, color))
}

/// The card: title · subtitle, the three icon squares, the legend, the plot.
pub fn card(row_id: &str, part_id: &str, spec: &str, theme: &Theme, copy: &CodeCopy) -> AnyElement {
    let Some(prepared) = prepare(spec) else {
        return div()
            .w_full()
            .rounded(px(CARD_RADIUS))
            .bg(theme.code_bg)
            .border_1()
            .border_color(theme.border)
            .p(px(CARD_PAD))
            .text_size(px(12.5))
            .text_color(theme.text_muted)
            .debug_selector(|| "chart-unreadable".into())
            .child("This chart needs a newer version of Arbos.")
            .into_any_element();
    };
    let model = &prepared.model;
    let copy_id = SharedString::from(format!("{row_id}#chart-copy"));
    let copied = copy.copied.as_ref() == Some(&copy_id);
    let tsv = prepared.chart.table_tsv();
    let on_copy = copy.on_copy.clone();
    let ui = copy.chart.clone();
    let title = prepared
        .chart
        .title
        .clone()
        .filter(|t| !t.trim().is_empty());
    let subtitle = prepared.chart.subtitle.clone();
    let save_title = title.clone().unwrap_or_else(|| "chart".into());
    let spec_save = spec.to_owned();
    let spec_expand = spec.to_owned();
    let part_for_expand = part_id.to_owned();

    let mut icons = div()
        .absolute()
        .top(px(8.0))
        .right(px(8.0))
        .flex()
        .gap(px(6.0));
    {
        let copy_id2 = copy_id.clone();
        icons = icons.child(
            icon_square(
                copy_id.clone(),
                if copied { "check" } else { "copy" },
                copied.then_some(theme.online),
                theme,
            )
            .on_click(move |_, window, cx| {
                if let Some(on_copy) = &on_copy {
                    on_copy(copy_id2.clone(), tsv.clone(), window, cx);
                }
            }),
        );
    }
    {
        let ui = ui.clone();
        icons = icons.child(
            icon_square(
                SharedString::from(format!("{row_id}#chart-save")),
                "download",
                None,
                theme,
            )
            .on_click(move |_, window, cx| {
                if let Some(ui) = &ui {
                    (ui.on_save)(spec_save.clone(), save_title.clone(), window, cx);
                }
            }),
        );
    }
    {
        let ui = ui.clone();
        icons = icons.child(
            icon_square(
                SharedString::from(format!("{row_id}#chart-expand")),
                "expand",
                None,
                theme,
            )
            .on_click(move |_, window, cx| {
                if let Some(ui) = &ui {
                    (ui.on_expand)(part_for_expand.clone(), spec_expand.clone(), window, cx);
                }
            }),
        );
    }

    let mut card = div()
        .relative()
        .flex()
        .flex_col()
        .w_full()
        .rounded(px(CARD_RADIUS))
        .bg(theme.code_bg)
        .border_1()
        .border_color(theme.border)
        .px(px(CARD_PAD))
        .pt(px(CARD_PAD))
        .pb(px(10.0))
        .debug_selector(|| "chart-card".into());
    let mut head = div().flex().flex_col().pr(px(100.0));
    if let Some(t) = &title {
        head = head.child(
            div()
                .text_size(px(13.0))
                .line_height(px(17.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .debug_selector(|| "chart-title".into())
                .child(t.clone()),
        );
    }
    if let Some(s) = &subtitle {
        head = head.child(
            div()
                .text_size(px(11.5))
                .line_height(px(15.0))
                .text_color(theme.text_muted)
                .child(s.clone()),
        );
    }
    card = card.child(head).child(icons);
    if model.show_legend {
        card = card.child(legend(model, theme, 11.5, 8.0));
    }
    let height = plot_height(model.kind);
    card = card.child(plot(
        SharedString::from(format!("{row_id}#chart-plot")),
        prepared.clone(),
        theme.clone(),
        Some(height),
        LayoutStyle::MAC,
        ui,
    ));
    card.into_any_element()
}

/// The legend row: a dot per series (or per slice for a pie) and its name.
pub fn legend(model: &ChartModel, theme: &Theme, font: f32, dot: f32) -> gpui::Div {
    let mut row = div()
        .flex()
        .flex_wrap()
        .gap_x(px(14.0))
        .gap_y(px(4.0))
        .mt(px(10.0))
        .mb(px(2.0))
        .text_size(px(font))
        .text_color(theme.text_muted)
        .debug_selector(|| "chart-legend".into());
    let items: Vec<(String, Hsla)> = if model.kind == Kind::Pie {
        model
            .categories
            .iter()
            .enumerate()
            .map(|(i, c)| (c.clone(), slice_color(model, i, theme)))
            .collect()
    } else {
        model
            .series
            .iter()
            .enumerate()
            .map(|(i, s)| (s.name.clone(), series_color(model, i, theme)))
            .collect()
    };
    for (name, color) in items {
        row = row.child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(div().size(px(dot)).rounded_full().bg(color))
                .child(name),
        );
    }
    row
}

/// The plot: a canvas that lays the chart out for its bounds every frame and paints it, with
/// the hover readout for the pointer's position. `id` makes the element interactive (mouse
/// moves repaint through `ui.on_hover`).
pub fn plot(
    id: SharedString,
    prepared: Rc<Prepared>,
    theme: Theme,
    height: Option<f32>,
    style: LayoutStyle,
    ui: Option<Rc<ChartUi>>,
) -> AnyElement {
    let hover_move = ui.clone();
    let hover_leave = ui;
    let selector = id.to_string();
    div()
        .id(id)
        .debug_selector(move || selector.clone())
        .w_full()
        .when_some(height, |el, h| el.h(px(h)))
        .when(height.is_none(), |el| el.flex_1().min_h_0().h_full())
        .on_mouse_move(move |_, window, cx| {
            if let Some(ui) = &hover_move {
                (ui.on_hover)(window, cx);
            }
        })
        .on_hover(move |_, window, cx| {
            if let Some(ui) = &hover_leave {
                (ui.on_hover)(window, cx);
            }
        })
        .child(
            canvas(
                move |bounds, window, _| {
                    let y_title =
                        prepared.model.y.label.is_some() && prepared.model.kind != Kind::Pie;
                    let top = if y_title { Y_TITLE_H } else { 0.0 };
                    let w = f32::from(bounds.size.width);
                    let h = (f32::from(bounds.size.height) - top).max(10.0);
                    // The axis title is painted above the plot, not rotated beside it.
                    let mut model = prepared.model.clone();
                    let y_title_text = model.y.label.take().map(|l| {
                        if model.y.unit.is_empty() {
                            l
                        } else {
                            format!("{l} ({})", model.y.unit)
                        }
                    });
                    let layout = model.layout(w, h, &style);
                    let mouse = window.mouse_position();
                    let local = Pt {
                        x: f32::from(mouse.x - bounds.origin.x),
                        y: f32::from(mouse.y - bounds.origin.y) - top,
                    };
                    let readout = if window.is_window_hovered()
                        && local.x >= 0.0
                        && local.y >= 0.0
                        && local.x <= w
                        && local.y <= h
                    {
                        model.hit(&layout, local)
                    } else {
                        None
                    };
                    (model, layout, readout, top, y_title_text)
                },
                move |bounds, (model, layout, readout, top, y_title), window, cx| {
                    let origin = point(bounds.origin.x, bounds.origin.y + px(top));
                    if let Some(t) = y_title {
                        let (line, _) =
                            shape(window, &t, layout.label_font, theme.text_faint, false);
                        let x = origin.x + px(layout.plot.x) - px(2.0);
                        let _ = line.paint(
                            point(x.max(bounds.origin.x), bounds.origin.y),
                            px(Y_TITLE_H),
                            TextAlign::Left,
                            None,
                            window,
                            cx,
                        );
                    }
                    paint_plot(
                        origin,
                        &model,
                        &layout,
                        readout.as_ref(),
                        &theme,
                        window,
                        cx,
                    );
                },
            )
            .size_full(),
        )
        .into_any_element()
}

fn shape(
    window: &mut Window,
    text: &str,
    size: f32,
    color: Hsla,
    bold: bool,
) -> (gpui::ShapedLine, Pixels) {
    let mut font = window.text_style().font();
    font.weight = if bold {
        FontWeight::SEMIBOLD
    } else {
        FontWeight::NORMAL
    };
    let run = TextRun {
        len: text.len(),
        font,
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window.text_system().shape_line(
        SharedString::from(text.to_owned()),
        px(size),
        &[run],
        None,
    );
    let w = line.width;
    (line, w)
}

fn hline(window: &mut Window, x0: f32, x1: f32, y: f32, width: f32, color: Hsla, o: Point<Pixels>) {
    let b = Bounds::new(
        point(o.x + px(x0.min(x1)), o.y + px(y - width / 2.0)),
        size(px((x1 - x0).abs()), px(width)),
    );
    window.paint_quad(fill(b, color));
}

fn vline(window: &mut Window, x: f32, y0: f32, y1: f32, width: f32, color: Hsla, o: Point<Pixels>) {
    let b = Bounds::new(
        point(o.x + px(x - width / 2.0), o.y + px(y0.min(y1))),
        size(px(width), px((y1 - y0).abs())),
    );
    window.paint_quad(fill(b, color));
}

fn disc(window: &mut Window, c: Pt, r: f32, color: Hsla, o: Point<Pixels>) {
    let b = Bounds::new(
        point(o.x + px(c.x - r), o.y + px(c.y - r)),
        size(px(r * 2.0), px(r * 2.0)),
    );
    window.paint_quad(fill(b, color).corner_radii(px(r)));
}

fn dashed(window: &mut Window, from: Pt, to: Pt, width: f32, color: Hsla, o: Point<Pixels>) {
    let mut pb = PathBuilder::stroke(px(width)).dash_array(&[px(3.0), px(3.0)]);
    pb.move_to(point(o.x + px(from.x), o.y + px(from.y)));
    pb.line_to(point(o.x + px(to.x), o.y + px(to.y)));
    if let Ok(path) = pb.build() {
        window.paint_path(path, color);
    }
}

/// Paint a laid-out chart at `o` (the plot area's top-left in window coordinates).
#[allow(clippy::too_many_arguments)]
pub fn paint_plot(
    o: Point<Pixels>,
    model: &ChartModel,
    layout: &PlotLayout,
    readout: Option<&Readout>,
    theme: &Theme,
    window: &mut Window,
    cx: &mut App,
) {
    let fs = layout.font;
    let line_h = px(fs * 1.3);
    if let Some(pie) = &layout.pie {
        let c = pie.centre;
        for s in &pie.slices {
            if s.end - s.start <= 0.0 {
                continue;
            }
            let color = slice_color(model, s.index, theme);
            let dim = readout.is_some_and(|r| r.column != s.index);
            let color = if dim {
                Theme::with_alpha(color, 0.45)
            } else {
                color
            };
            let mut pb = PathBuilder::fill();
            pb.move_to(point(o.x + px(c.x), o.y + px(c.y)));
            let steps = ((s.end - s.start) / 0.06).ceil().max(2.0) as usize;
            for i in 0..=steps {
                let a = s.start + (s.end - s.start) * i as f32 / steps as f32;
                pb.line_to(point(
                    o.x + px(c.x + pie.radius * a.sin()),
                    o.y + px(c.y - pie.radius * a.cos()),
                ));
            }
            pb.close();
            if let Ok(path) = pb.build() {
                window.paint_path(path, color);
            }
            // The slice seam in the card colour.
            let mut seam = PathBuilder::stroke(px(1.5));
            seam.move_to(point(o.x + px(c.x), o.y + px(c.y)));
            seam.line_to(point(
                o.x + px(c.x + pie.radius * s.start.sin()),
                o.y + px(c.y - pie.radius * s.start.cos()),
            ));
            if let Ok(path) = seam.build() {
                window.paint_path(path, theme.bubble_assistant);
            }
            if s.share >= 0.06 {
                let mid = (s.start + s.end) / 2.0;
                let label = format!("{}%", (s.share * 100.0).round() as i64);
                let (line, w) = shape(window, &label, fs, theme.bubble_assistant, true);
                let x = o.x + px(c.x + pie.radius * 0.62 * mid.sin()) - w / 2.0;
                let y = o.y + px(c.y - pie.radius * 0.62 * mid.cos()) - line_h / 2.0;
                let _ = line.paint(point(x, y), line_h, TextAlign::Left, None, window, cx);
            }
        }
        if let Some(r) = readout {
            paint_readout(o, layout, r, model, theme, window, cx);
        }
        return;
    }
    let plot = layout.plot;
    // Grid + y ticks.
    for (g, t) in layout.gridlines.iter().zip(&layout.y_ticks) {
        hline(window, plot.x, plot.right(), *g, 1.0, theme.border, o);
        let (line, w) = shape(window, &t.label, fs, theme.text_faint, false);
        let _ = line.paint(
            point(
                o.x + px(plot.x - fs * 0.55) - w,
                o.y + px(t.pos) - line_h / 2.0,
            ),
            line_h,
            TextAlign::Left,
            None,
            window,
            cx,
        );
    }
    for t in &layout.y2_ticks {
        let (line, _) = shape(window, &t.label, fs, theme.text_faint, false);
        let _ = line.paint(
            point(
                o.x + px(plot.right() + fs * 0.55),
                o.y + px(t.pos) - line_h / 2.0,
            ),
            line_h,
            TextAlign::Left,
            None,
            window,
            cx,
        );
    }
    hline(
        window,
        plot.x,
        plot.right(),
        plot.bottom(),
        1.0,
        theme.border_strong,
        o,
    );
    for t in &layout.x_ticks {
        let (line, w) = shape(window, &t.label, fs, theme.text_faint, false);
        let _ = line.paint(
            point(
                o.x + px(t.pos) - w / 2.0,
                o.y + px(plot.bottom() + fs * 0.5),
            ),
            line_h,
            TextAlign::Left,
            None,
            window,
            cx,
        );
    }
    if let Some(l) = &layout.x_label {
        let (line, w) = shape(window, l, layout.label_font, theme.text_faint, false);
        let _ = line.paint(
            point(
                o.x + px(plot.x + plot.w / 2.0) - w / 2.0,
                o.y + px(layout.h - layout.label_font * 1.45),
            ),
            line_h,
            TextAlign::Left,
            None,
            window,
            cx,
        );
    }
    // Annotations under the series.
    for a in &layout.annotations {
        match a {
            AnnotationShape::Band { rect, label } => {
                let b = Bounds::new(
                    point(o.x + px(rect.x), o.y + px(rect.y)),
                    size(px(rect.w), px(rect.h)),
                );
                window.paint_quad(fill(b, Theme::with_alpha(theme.text, 0.05)));
                if let Some(l) = label {
                    let (line, _) = shape(window, l, fs - 0.5, theme.text_faint, false);
                    let _ = line.paint(
                        point(o.x + px(rect.x + 4.0), o.y + px(rect.y + 2.0)),
                        line_h,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }
            }
            AnnotationShape::Vline { x, label } => {
                dashed(
                    window,
                    Pt { x: *x, y: plot.y },
                    Pt {
                        x: *x,
                        y: plot.bottom(),
                    },
                    1.0,
                    theme.text_faint,
                    o,
                );
                if let Some(l) = label {
                    let (line, _) = shape(window, l, fs - 0.5, theme.text_faint, false);
                    let _ = line.paint(
                        point(o.x + px(x + 5.0), o.y + px(plot.y + 1.0)),
                        line_h,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }
            }
            AnnotationShape::Hline { y, label } => {
                dashed(
                    window,
                    Pt { x: plot.x, y: *y },
                    Pt {
                        x: plot.right(),
                        y: *y,
                    },
                    1.0,
                    theme.text_faint,
                    o,
                );
                if let Some(l) = label {
                    let (line, w) = shape(window, l, fs - 0.5, theme.text_faint, false);
                    let _ = line.paint(
                        point(o.x + px(plot.right() - 2.0) - w, o.y + px(y - fs * 1.35)),
                        line_h,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }
            }
            AnnotationShape::Point { at, label } => {
                disc(window, *at, layout.line_width * 2.5, theme.text, o);
                disc(
                    window,
                    *at,
                    layout.line_width * 1.5,
                    theme.bubble_assistant,
                    o,
                );
                if let Some(l) = label {
                    let (line, _) = shape(window, l, fs - 0.5, theme.text, false);
                    let _ = line.paint(
                        point(o.x + px(at.x + fs * 0.6), o.y + px(at.y - fs * 1.3)),
                        line_h,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }
            }
        }
    }
    let lw = layout.line_width;
    for s in &layout.series {
        let color = series_color(model, s.index, theme);
        let dim = model.kind == Kind::Bar && readout.is_some();
        match &s.shape {
            SeriesShape::Line {
                pts,
                lower,
                area,
                style,
            } => {
                if *area {
                    let mut i = 0;
                    while i < pts.len() {
                        if pts[i].is_none() {
                            i += 1;
                            continue;
                        }
                        let mut j = i;
                        while j < pts.len() && pts[j].is_some() {
                            j += 1;
                        }
                        let mut pb = PathBuilder::fill();
                        for (k, p) in pts[i..j].iter().enumerate() {
                            let p = p.unwrap();
                            let at = point(o.x + px(p.x), o.y + px(p.y));
                            if k == 0 {
                                pb.move_to(at);
                            } else {
                                pb.line_to(at);
                            }
                        }
                        for p in lower[i..j].iter().rev().flatten() {
                            pb.line_to(point(o.x + px(p.x), o.y + px(p.y)));
                        }
                        pb.close();
                        if let Ok(path) = pb.build() {
                            window.paint_path(path, Theme::with_alpha(color, 0.18));
                        }
                        i = j;
                    }
                }
                let mut pb = PathBuilder::stroke(px(lw));
                pb = match style {
                    LineStyle::Solid => pb,
                    LineStyle::Dashed => pb.dash_array(&[px(lw * 3.0), px(lw * 2.0)]),
                    LineStyle::Dotted => pb.dash_array(&[px(lw * 0.5), px(lw * 2.0)]),
                };
                let mut pen = false;
                let mut any = false;
                for p in pts {
                    match p {
                        Some(p) => {
                            let at = point(o.x + px(p.x), o.y + px(p.y));
                            if pen {
                                pb.line_to(at);
                            } else {
                                pb.move_to(at);
                            }
                            pen = true;
                            any = true;
                        }
                        None => pen = false,
                    }
                }
                if any {
                    if let Ok(path) = pb.build() {
                        window.paint_path(path, color);
                    }
                }
                for (i, p) in pts.iter().enumerate() {
                    let alone = p.is_some()
                        && (i == 0 || pts[i - 1].is_none())
                        && (i + 1 == pts.len() || pts[i + 1].is_none());
                    if alone {
                        disc(window, p.unwrap(), lw * 1.2, color, o);
                    }
                }
            }
            SeriesShape::Scatter { pts } => {
                for p in pts {
                    disc(window, *p, lw * 1.8, Theme::with_alpha(color, 0.9), o);
                }
            }
            SeriesShape::Bars { rects } => {
                for (ci, r) in rects.iter().enumerate() {
                    let Some(r) = r else { continue };
                    let faded = dim && readout.is_some_and(|ro| ro.column != ci);
                    let c = if faded {
                        Theme::with_alpha(color, 0.45)
                    } else {
                        color
                    };
                    let rad = (r.w * 0.18).min(3.0).min(r.h / 2.0);
                    let b =
                        Bounds::new(point(o.x + px(r.x), o.y + px(r.y)), size(px(r.w), px(r.h)));
                    window.paint_quad(fill(b, c).corner_radii(Corners {
                        top_left: px(rad),
                        top_right: px(rad),
                        bottom_left: px(0.0),
                        bottom_right: px(0.0),
                    }));
                }
            }
        }
    }
    if let Some(r) = readout {
        if model.kind != Kind::Bar {
            vline(
                window,
                r.x,
                plot.y,
                plot.bottom(),
                1.0,
                Theme::with_alpha(theme.text, 0.28),
                o,
            );
            for e in &r.entries {
                let color = series_color(model, e.series, theme);
                disc(
                    window,
                    Pt { x: r.x, y: e.y },
                    lw * 2.2,
                    theme.bubble_assistant,
                    o,
                );
                disc(window, Pt { x: r.x, y: e.y }, lw * 1.5, color, o);
            }
        }
        paint_readout(o, layout, r, model, theme, window, cx);
    }
}

/// The hover chip: the x label, then one row per series with its dot, name and value.
fn paint_readout(
    o: Point<Pixels>,
    layout: &PlotLayout,
    r: &Readout,
    model: &ChartModel,
    theme: &Theme,
    window: &mut Window,
    cx: &mut App,
) {
    let fs = layout.font + 0.5;
    let row_h = fs + 7.0;
    let pad_x = 10.0;
    let (title, title_w) = shape(window, &r.label, fs, theme.text, true);
    let mut rows = Vec::new();
    let mut widest = f32::from(title_w);
    for e in &r.entries {
        let color = if model.kind == Kind::Pie {
            slice_color(model, e.color_index, theme)
        } else {
            series_color(model, e.series, theme)
        };
        let (name, name_w) = shape(window, &e.name, fs, theme.text_muted, false);
        let (value, value_w) = shape(window, &e.text, fs, theme.text, false);
        widest = widest.max(7.0 + 7.0 + f32::from(name_w) + 18.0 + f32::from(value_w));
        rows.push((color, name, value, value_w));
    }
    let w = widest + pad_x * 2.0;
    let h = 6.0 + row_h + rows.len() as f32 * row_h + 4.0;
    // Beside the crosshair at the plot's top; flipped left near the right edge; clamped.
    let plot = layout.plot;
    let mut x = r.x + 12.0;
    if x + w > plot.right() + 2.0 {
        x = r.x - 12.0 - w;
    }
    x = x.clamp(0.0, (layout.w - w).max(0.0));
    let mut y = plot.y + 2.0;
    if model.kind == Kind::Pie {
        y = (r.entries.first().map(|e| e.y).unwrap_or(plot.y) - h / 2.0).max(0.0);
    }
    let b = Bounds::new(point(o.x + px(x), o.y + px(y)), size(px(w), px(h)));
    let bg = match theme.appearance {
        Appearance::Dark => theme.surface_2,
        Appearance::Light => theme.surface_1,
    };
    window.paint_quad(quad(
        Bounds::new(point(b.origin.x, b.origin.y + px(2.0)), b.size),
        px(8.0),
        Theme::with_alpha(gpui::black(), 0.18),
        Edges::all(px(0.0)),
        gpui::transparent_black(),
        BorderStyle::default(),
    ));
    window.paint_quad(quad(
        b,
        px(8.0),
        bg,
        Edges::all(px(1.0)),
        theme.border_strong,
        BorderStyle::default(),
    ));
    let lh = px(row_h);
    let mut ty = b.origin.y + px(6.0);
    let _ = title.paint(
        point(b.origin.x + px(pad_x), ty),
        lh,
        TextAlign::Left,
        None,
        window,
        cx,
    );
    ty += lh;
    for (color, name, value, value_w) in rows {
        let cy = ty + lh / 2.0;
        window.paint_quad(
            fill(
                Bounds::new(
                    point(b.origin.x + px(pad_x), cy - px(3.5)),
                    size(px(7.0), px(7.0)),
                ),
                color,
            )
            .corner_radii(px(3.5)),
        );
        let _ = name.paint(
            point(b.origin.x + px(pad_x + 14.0), ty),
            lh,
            TextAlign::Left,
            None,
            window,
            cx,
        );
        let _ = value.paint(
            point(b.origin.x + px(w - pad_x) - value_w, ty),
            lh,
            TextAlign::Left,
            None,
            window,
            cx,
        );
        ty += lh;
    }
}

/// Render the chart as a PNG in `theme` at `scale` px/pt (the "Save PNG" action): the same
/// core layout the card paints, through the picture renderer.
pub fn png_bytes(
    spec: &str,
    theme: &Theme,
    scale: f32,
    credit_left: &str,
) -> anyhow::Result<Vec<u8>> {
    use arbos_core::chart_render::{
        picture_png_with, PictureCredit, PictureTheme, PNG_HEIGHT, PNG_WIDTH,
    };
    let value: serde_json::Value = serde_json::from_str(spec)?;
    let chart = Chart::parse(&value).map_err(|e| anyhow::anyhow!("{e}"))?;
    let picture_theme = match theme.appearance {
        Appearance::Dark => PictureTheme::DARK,
        Appearance::Light => PictureTheme::LIGHT,
    };
    picture_png_with(
        &chart,
        PNG_WIDTH,
        PNG_HEIGHT,
        scale,
        &picture_theme,
        &PictureCredit {
            left: credit_left.to_owned(),
        },
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

/// "Save PNG": render the chart in the current theme at 2x, write it to Downloads, say so (the
/// toast + Finder reveal every saved attachment gets), or show the shared error toast.
pub fn save_png(spec: &str, title: &str, state: &gpui::Entity<crate::app::AppState>, cx: &mut App) {
    let theme = Theme::of(cx).clone();
    let credit = format!("Arbos · {}", chrono::Local::now().format("%Y-%m-%d"));
    let result = png_bytes(spec, &theme, 2.0, &credit).and_then(|bytes| {
        let path = crate::sync::relay::unique_download_path(&png_file_name(title));
        std::fs::write(&path, bytes)?;
        Ok(path)
    });
    state.update(cx, |s, cx| {
        match result {
            Ok(path) => {
                s.push_notice(format!("Saved to {}", path.display()));
                super::attachments::reveal(&path);
            }
            Err(err) => s.push_failure(
                crate::sync::user_error::UserError::classify("save the chart", &err),
                None,
            ),
        }
        cx.notify();
    });
}

/// A file name for a saved chart: `eval-score-over-runs.png`.
pub fn png_file_name(title: &str) -> String {
    let stem: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .take(48)
        .collect();
    if stem.is_empty() {
        "chart.png".into()
    } else {
        format!("{stem}.png")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: &str = r#"{"kind":"line","title":"Eval score over runs","series":[{"name":"a","points":[[1,70],[2,80]]}],"y":{"unit":"%"}}"#;

    #[test]
    fn prepare_caches_and_reads_the_title() {
        let p = prepare(SPEC).expect("valid");
        assert_eq!(p.model.columns.len(), 2);
        assert!(Rc::ptr_eq(&p, &prepare(SPEC).unwrap()), "cached by spec");
        assert_eq!(one_line_title(SPEC), "Chart: Eval score over runs");
        assert!(prepare("{\"kind\":\"pie\"}").is_none());
        assert_eq!(one_line_title("nope"), "Chart");
    }

    #[test]
    fn colours_and_names() {
        let dark = Theme::dark(crate::ui::theme::Surface::Matte);
        let light = Theme::light(crate::ui::theme::Surface::Matte);
        let p = prepare(SPEC).unwrap();
        assert_ne!(
            series_color(&p.model, 0, &dark),
            series_color(&p.model, 0, &light),
            "a palette per theme"
        );
        assert_eq!(
            hex_color("#1084FE", dark.text),
            Hsla::from(gpui::rgb(0x1084fe))
        );
        assert_eq!(hex_color("red", dark.text), dark.text);
        assert_eq!(
            png_file_name("Eval score over runs"),
            "eval-score-over-runs.png"
        );
        assert_eq!(png_file_name("***"), "chart.png");
        assert_eq!(plot_height(Kind::Bar), BAR_PLOT_HEIGHT);
    }

    #[test]
    fn the_saved_png_is_the_theme_at_scale() {
        let dark = Theme::dark(crate::ui::theme::Surface::Matte);
        let png = png_bytes(SPEC, &dark, 2.0, "Affine · Arbos").unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        assert_eq!(&png[16..24], &[0, 0, 9, 96, 0, 0, 5, 70], "2400 × 1350 px");
        assert!(png_bytes("{}", &dark, 1.0, "").is_err());
    }
}
