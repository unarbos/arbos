//! The static picture of a chart bubble: an SVG built from [`PlotLayout`] (always available, the
//! tests read it) and, behind the `chart-png` feature, the PNG agentd attaches as the fallback
//! for clients that cannot draw the `chart` part (old apps, push previews, Library). Light
//! theme on white, 16:9, bundled Inter subsets so the box needs no system fonts.

use std::fmt::Write as _;

use crate::chart::{
    AnnotationShape, Chart, ChartModel, LayoutStyle, PlotLayout, SeriesShape, PALETTE_LIGHT,
};

/// Default fallback size (16:9; reads on a phone and in a notification thumbnail).
pub const PNG_WIDTH: u32 = 1200;
pub const PNG_HEIGHT: u32 = 675;

/// Colours of the picture (the light theme's tokens).
#[derive(Clone, Debug, PartialEq)]
pub struct PictureTheme {
    pub bg: &'static str,
    pub text: &'static str,
    pub muted: &'static str,
    pub faint: &'static str,
    pub grid: &'static str,
    pub hair: &'static str,
    pub palette: &'static [&'static str],
}

impl PictureTheme {
    pub const LIGHT: PictureTheme = PictureTheme {
        bg: "#ffffff",
        text: "#111111",
        muted: "#6b6b70",
        faint: "#757579",
        grid: "rgba(0,0,0,0.08)",
        hair: "rgba(0,0,0,0.10)",
        palette: &PALETTE_LIGHT,
    };
    /// The dark app's tokens, for a picture saved from a dark Mac / iPhone.
    pub const DARK: PictureTheme = PictureTheme {
        bg: "#141414",
        text: "#ffffff",
        muted: "#95959b",
        faint: "#8a8a8e",
        grid: "rgba(255,255,255,0.08)",
        hair: "rgba(255,255,255,0.10)",
        palette: &crate::chart::PALETTE_DARK,
    };
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn f(v: f32) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" {
        "0".into()
    } else {
        s.to_owned()
    }
}

/// The plot area alone (axes, grid, series, annotations) as SVG elements, offset by `(ox, oy)`.
pub fn plot_svg(
    model: &ChartModel,
    layout: &PlotLayout,
    theme: &PictureTheme,
    ox: f32,
    oy: f32,
) -> String {
    let mut o = String::new();
    let fs = layout.font;
    let font = "font-family=\"Inter, system-ui, sans-serif\"";
    let tx = |x: f32| f(x + ox);
    let ty = |y: f32| f(y + oy);
    if let Some(pie) = &layout.pie {
        for s in &pie.slices {
            if s.end - s.start <= 0.0 {
                continue;
            }
            let (cx, cy, r) = (pie.centre.x + ox, pie.centre.y + oy, pie.radius);
            let (x0, y0) = (cx + r * s.start.sin(), cy - r * s.start.cos());
            let (x1, y1) = (cx + r * s.end.sin(), cy - r * s.end.cos());
            let large = if s.end - s.start > std::f32::consts::PI {
                1
            } else {
                0
            };
            let color = model.series_color(0, theme.palette);
            let color = if model.series[0].color.is_some() {
                color
            } else {
                theme.palette[s.index % theme.palette.len()].to_owned()
            };
            if (s.end - s.start - std::f32::consts::TAU).abs() < 1e-4 {
                let _ = write!(
                    o,
                    "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{color}\"/>",
                    f(cx),
                    f(cy),
                    f(r)
                );
            } else {
                let _ = write!(
                    o,
                    "<path d=\"M{} {} L{} {} A{} {} 0 {large} 1 {} {} Z\" fill=\"{color}\" stroke=\"{}\" stroke-width=\"2\"/>",
                    f(cx), f(cy), f(x0), f(y0), f(r), f(r), f(x1), f(y1), theme.bg
                );
            }
            if s.share >= 0.06 {
                let mid = (s.start + s.end) / 2.0;
                let (lx, ly) = (cx + r * 0.62 * mid.sin(), cy - r * 0.62 * mid.cos());
                let _ = write!(
                    o,
                    "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" font-size=\"{}\" fill=\"{}\" {font} font-weight=\"600\">{}%</text>",
                    f(lx), f(ly + fs * 0.36), f(fs), theme.bg, (s.share * 100.0).round() as i64
                );
            }
        }
        return o;
    }
    let plot = layout.plot;
    for (g, t) in layout.gridlines.iter().zip(&layout.y_ticks) {
        let _ = write!(
            o,
            "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{}\" stroke-width=\"1\"/>",
            tx(plot.x),
            ty(*g),
            tx(plot.right()),
            ty(*g),
            theme.grid
        );
        let _ = write!(
            o,
            "<text x=\"{}\" y=\"{}\" text-anchor=\"end\" font-size=\"{}\" fill=\"{}\" {font}>{}</text>",
            tx(plot.x - fs * 0.55), ty(t.pos + fs * 0.36), f(fs), theme.faint, esc(&t.label)
        );
    }
    for t in &layout.y2_ticks {
        let _ = write!(
            o,
            "<text x=\"{}\" y=\"{}\" font-size=\"{}\" fill=\"{}\" {font}>{}</text>",
            tx(plot.right() + fs * 0.55),
            ty(t.pos + fs * 0.36),
            f(fs),
            theme.faint,
            esc(&t.label)
        );
    }
    for t in &layout.x_ticks {
        let _ = write!(
            o,
            "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" font-size=\"{}\" fill=\"{}\" {font}>{}</text>",
            tx(t.pos), ty(plot.bottom() + fs * 1.45), f(fs), theme.faint, esc(&t.label)
        );
    }
    let _ = write!(
        o,
        "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{}\" stroke-width=\"1\"/>",
        tx(plot.x),
        ty(plot.bottom()),
        tx(plot.right()),
        ty(plot.bottom()),
        theme.hair
    );
    if let Some(l) = &layout.x_label {
        let _ = write!(
            o,
            "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" font-size=\"{}\" fill=\"{}\" {font}>{}</text>",
            tx(plot.x + plot.w / 2.0), ty(layout.h - fs * 0.3), f(layout.label_font), theme.faint, esc(l)
        );
    }
    if let Some(l) = &layout.y_label {
        let (x, y) = (ox + layout.label_font * 0.95, oy + plot.y + plot.h / 2.0);
        let _ = write!(
            o,
            "<text transform=\"translate({} {}) rotate(-90)\" text-anchor=\"middle\" font-size=\"{}\" fill=\"{}\" {font}>{}</text>",
            f(x), f(y), f(layout.label_font), theme.faint, esc(l)
        );
    }
    for a in &layout.annotations {
        match a {
            AnnotationShape::Band { rect, label } => {
                let _ = write!(
                    o,
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\" opacity=\"0.5\"/>",
                    tx(rect.x), ty(rect.y), f(rect.w), f(rect.h), theme.grid
                );
                if let Some(l) = label {
                    let _ = write!(
                        o,
                        "<text x=\"{}\" y=\"{}\" font-size=\"{}\" fill=\"{}\" {font}>{}</text>",
                        tx(rect.x + 4.0),
                        ty(rect.y + fs),
                        f(fs - 0.5),
                        theme.faint,
                        esc(l)
                    );
                }
            }
            AnnotationShape::Vline { x, label } => {
                let _ = write!(
                    o,
                    "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{}\" stroke-width=\"1\" stroke-dasharray=\"3 3\"/>",
                    tx(*x), ty(plot.y), tx(*x), ty(plot.bottom()), theme.faint
                );
                if let Some(l) = label {
                    let _ = write!(
                        o,
                        "<text x=\"{}\" y=\"{}\" font-size=\"{}\" fill=\"{}\" {font}>{}</text>",
                        tx(*x + 5.0),
                        ty(plot.y + fs),
                        f(fs - 0.5),
                        theme.faint,
                        esc(l)
                    );
                }
            }
            AnnotationShape::Hline { y, label } => {
                let _ = write!(
                    o,
                    "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{}\" stroke-width=\"1\" stroke-dasharray=\"3 3\"/>",
                    tx(plot.x), ty(*y), tx(plot.right()), ty(*y), theme.faint
                );
                if let Some(l) = label {
                    let _ = write!(
                        o,
                        "<text x=\"{}\" y=\"{}\" text-anchor=\"end\" font-size=\"{}\" fill=\"{}\" {font}>{}</text>",
                        tx(plot.right() - 2.0), ty(*y - 4.0), f(fs - 0.5), theme.faint, esc(l)
                    );
                }
            }
            AnnotationShape::Point { at, label } => {
                let _ = write!(
                    o,
                    "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"1.5\"/>",
                    tx(at.x), ty(at.y), f(layout.line_width * 2.5), theme.text
                );
                if let Some(l) = label {
                    let _ = write!(
                        o,
                        "<text x=\"{}\" y=\"{}\" font-size=\"{}\" fill=\"{}\" {font}>{}</text>",
                        tx(at.x + fs * 0.6),
                        ty(at.y - fs * 0.5),
                        f(fs - 0.5),
                        theme.text,
                        esc(l)
                    );
                }
            }
        }
    }
    let lw = layout.line_width;
    for s in &layout.series {
        let color = model.series_color(s.index, theme.palette);
        match &s.shape {
            SeriesShape::Line {
                pts,
                lower,
                area,
                style,
            } => {
                // One sub-path per run of present points (a gap breaks the line).
                let mut d = String::new();
                let mut pen = false;
                for p in pts {
                    match p {
                        Some(p) => {
                            let _ = write!(
                                d,
                                "{}{} {} ",
                                if pen { "L" } else { "M" },
                                tx(p.x),
                                ty(p.y)
                            );
                            pen = true;
                        }
                        None => pen = false,
                    }
                }
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
                        let mut poly = String::new();
                        for (k, p) in pts[i..j].iter().enumerate() {
                            let p = p.unwrap();
                            let _ = write!(
                                poly,
                                "{}{} {} ",
                                if k == 0 { "M" } else { "L" },
                                tx(p.x),
                                ty(p.y)
                            );
                        }
                        for p in lower[i..j].iter().rev().flatten() {
                            let _ = write!(poly, "L{} {} ", tx(p.x), ty(p.y));
                        }
                        let _ =
                            write!(o, "<path d=\"{poly}Z\" fill=\"{color}\" opacity=\"0.18\"/>");
                        i = j;
                    }
                }
                let dash = match style {
                    crate::chart::LineStyle::Solid => String::new(),
                    crate::chart::LineStyle::Dashed => {
                        format!(" stroke-dasharray=\"{} {}\"", f(lw * 3.0), f(lw * 2.0))
                    }
                    crate::chart::LineStyle::Dotted => {
                        format!(" stroke-dasharray=\"{} {}\"", f(lw * 0.1), f(lw * 2.0))
                    }
                };
                let _ = write!(
                    o,
                    "<path d=\"{}\" fill=\"none\" stroke=\"{color}\" stroke-width=\"{}\" stroke-linejoin=\"round\" stroke-linecap=\"round\"{dash}/>",
                    d.trim_end(), f(lw)
                );
                // A lone point (gaps on both sides) still shows.
                for (i, p) in pts.iter().enumerate() {
                    let alone = p.is_some()
                        && (i == 0 || pts[i - 1].is_none())
                        && (i + 1 == pts.len() || pts[i + 1].is_none());
                    if alone {
                        let p = p.unwrap();
                        let _ = write!(
                            o,
                            "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{color}\"/>",
                            tx(p.x),
                            ty(p.y),
                            f(lw * 1.2)
                        );
                    }
                }
            }
            SeriesShape::Scatter { pts } => {
                for p in pts {
                    let _ = write!(
                        o,
                        "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{color}\" opacity=\"0.9\"/>",
                        tx(p.x),
                        ty(p.y),
                        f(lw * 1.8)
                    );
                }
            }
            SeriesShape::Bars { rects } => {
                for r in rects.iter().flatten() {
                    let rad = (r.w * 0.18).min(3.0 * lw / 2.0).min(r.h / 2.0);
                    let _ = write!(
                        o,
                        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{}\" fill=\"{color}\"/>",
                        tx(r.x), ty(r.y), f(r.w), f(r.h), f(rad)
                    );
                }
            }
        }
    }
    o
}

/// Where the picture came from, for the footer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PictureCredit {
    /// "Affine · Arbos · 2026-10-26"
    pub left: String,
}

/// The whole picture as an SVG document: title, subtitle, legend, plot, footer.
pub fn picture_svg(
    chart: &Chart,
    w: u32,
    h: u32,
    theme: &PictureTheme,
    credit: &PictureCredit,
) -> String {
    let model = chart.model();
    let (wf, hf) = (w as f32, h as f32);
    let pad_x = wf * 0.034;
    let pad_t = hf * 0.054;
    let title_fs = (hf * 0.044).round();
    let sub_fs = (hf * 0.028).round();
    let legend_fs = (hf * 0.028).round();
    let foot_fs = (hf * 0.022).round();
    let font = "font-family=\"Inter, system-ui, sans-serif\"";
    let mut o = String::new();
    let _ = write!(
        o,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\"><rect width=\"{w}\" height=\"{h}\" fill=\"{}\"/>",
        theme.bg
    );
    let mut y = pad_t + title_fs;
    if let Some(t) = &chart.title {
        let _ = write!(
            o,
            "<text x=\"{}\" y=\"{}\" font-size=\"{}\" font-weight=\"600\" fill=\"{}\" {font}>{}</text>",
            f(pad_x), f(y), f(title_fs), theme.text, esc(t)
        );
        y += sub_fs * 1.35;
    }
    if let Some(s) = &chart.subtitle {
        let _ = write!(
            o,
            "<text x=\"{}\" y=\"{}\" font-size=\"{}\" fill=\"{}\" {font}>{}</text>",
            f(pad_x),
            f(y),
            f(sub_fs),
            theme.muted,
            esc(s)
        );
        y += sub_fs * 1.2;
    }
    if model.show_legend {
        y += legend_fs * 0.9;
        let mut x = pad_x;
        let dot = legend_fs * 0.68;
        let items: Vec<(String, String)> = if model.kind == crate::chart::Kind::Pie {
            model
                .categories
                .iter()
                .enumerate()
                .map(|(i, c)| (c.clone(), theme.palette[i % theme.palette.len()].to_owned()))
                .collect()
        } else {
            model
                .series
                .iter()
                .enumerate()
                .map(|(i, s)| (s.name.clone(), model.series_color(i, theme.palette)))
                .collect()
        };
        for (name, color) in items {
            let _ = write!(
                o,
                "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{color}\"/>",
                f(x + dot / 2.0),
                f(y - legend_fs * 0.34),
                f(dot / 2.0)
            );
            let _ = write!(
                o,
                "<text x=\"{}\" y=\"{}\" font-size=\"{}\" fill=\"{}\" {font}>{}</text>",
                f(x + dot + legend_fs * 0.5),
                f(y),
                f(legend_fs),
                theme.muted,
                esc(&name)
            );
            x += dot
                + legend_fs * 0.5
                + name.chars().count() as f32 * legend_fs * 0.56
                + legend_fs * 1.4;
        }
        y += legend_fs * 0.6;
    }
    let plot_top = y + hf * 0.02;
    let foot_y = hf - pad_t * 0.55;
    let plot_h = (foot_y - foot_fs * 1.6 - plot_top).max(40.0);
    let plot_w = wf - 2.0 * pad_x;
    let style = LayoutStyle {
        font: (hf * 0.025).round(),
        label_font: (hf * 0.0235).round(),
        line_width: (hf * 0.0047).max(2.0),
        glyph_em: 0.56,
    };
    let layout = model.layout(plot_w, plot_h, &style);
    o.push_str(&plot_svg(&model, &layout, theme, pad_x, plot_top));
    if !credit.left.is_empty() {
        let _ = write!(
            o,
            "<text x=\"{}\" y=\"{}\" font-size=\"{}\" fill=\"{}\" {font}>{}</text>",
            f(pad_x),
            f(foot_y),
            f(foot_fs),
            theme.faint,
            esc(&credit.left)
        );
    }
    let _ = write!(
        o,
        "<text x=\"{}\" y=\"{}\" text-anchor=\"end\" font-size=\"{}\" fill=\"{}\" {font}>Static picture — open in Arbos for the live chart and the data</text>",
        f(wf - pad_x), f(foot_y), f(foot_fs), theme.faint
    );
    o.push_str("</svg>");
    o
}

/// Render the fallback PNG (`PNG_WIDTH × PNG_HEIGHT`, light theme) — what agentd attaches.
#[cfg(feature = "chart-png")]
pub fn picture_png(chart: &Chart, credit: &PictureCredit) -> Result<Vec<u8>, String> {
    picture_png_with(
        chart,
        PNG_WIDTH,
        PNG_HEIGHT,
        1.0,
        &PictureTheme::LIGHT,
        credit,
    )
}

/// Render a `w × h` point picture at `scale` px/pt in `theme` (the apps' "Save PNG": the
/// current theme at the device scale).
#[cfg(feature = "chart-png")]
pub fn picture_png_with(
    chart: &Chart,
    w: u32,
    h: u32,
    scale: f32,
    theme: &PictureTheme,
    credit: &PictureCredit,
) -> Result<Vec<u8>, String> {
    use resvg::usvg;
    static INTER_REGULAR: &[u8] = include_bytes!("../assets/Inter-Regular-subset.ttf");
    static INTER_SEMIBOLD: &[u8] = include_bytes!("../assets/Inter-SemiBold-subset.ttf");
    let svg = picture_svg(chart, w, h, theme, credit);
    let mut fontdb = usvg::fontdb::Database::new();
    fontdb.load_font_data(INTER_REGULAR.to_vec());
    fontdb.load_font_data(INTER_SEMIBOLD.to_vec());
    let opt = usvg::Options {
        fontdb: std::sync::Arc::new(fontdb),
        font_family: "Inter".into(),
        ..Default::default()
    };
    let tree =
        usvg::Tree::from_data(svg.as_bytes(), &opt).map_err(|e| format!("chart svg: {e}"))?;
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let (pw, ph) = (
        (w as f32 * scale).round() as u32,
        (h as f32 * scale).round() as u32,
    );
    let mut pixmap = resvg::tiny_skia::Pixmap::new(pw.max(1), ph.max(1)).ok_or("pixmap")?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap.encode_png().map_err(|e| format!("png: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn chart() -> Chart {
        Chart::parse(&json!({
            "kind": "line", "title": "Eval score over runs", "subtitle": "memory-pack eval",
            "series": [
                { "name": "Affine 2", "points": [[1, 66.2], [2, 70.4], [3, 79.6], [4, 83.4]] },
                { "name": "Baseline", "points": [[1, 70.1], [2, 70.6], [3, 71.1], [4, 71.2]] }
            ],
            "x": { "label": "Run" }, "y": { "label": "Pass rate", "unit": "%", "min": 60, "max": 90 },
            "annotations": [{ "type": "vline", "x": 3, "label": "prompt v3" }, { "type": "hline", "y": 80, "label": "target" }]
        }))
        .unwrap()
    }

    #[test]
    fn the_svg_carries_title_legend_axes_series_and_footer() {
        let svg = picture_svg(
            &chart(),
            1200,
            675,
            &PictureTheme::LIGHT,
            &PictureCredit {
                left: "Affine · Arbos · 2026-10-26".into(),
            },
        );
        assert!(svg.starts_with(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1200\" height=\"675\""
        ));
        assert!(svg.contains(">Eval score over runs</text>"));
        assert!(svg.contains(">memory-pack eval</text>"));
        assert!(svg.contains(">Affine 2</text>") && svg.contains(">Baseline</text>"));
        assert!(svg.contains(">60%</text>") && svg.contains(">90%</text>"));
        assert!(svg.contains(">prompt v3</text>") && svg.contains(">target</text>"));
        assert!(
            svg.contains("stroke=\"#1F7F7A\"") && svg.contains("stroke=\"#B8641A\""),
            "light palette"
        );
        assert!(svg.contains("Affine · Arbos · 2026-10-26") && svg.contains("Static picture"));
        assert_eq!(svg.matches("<path d=\"M").count(), 2, "one path per series");
        assert!(svg.ends_with("</svg>"));
    }

    #[test]
    fn bars_pies_and_gaps_draw() {
        let bar = Chart::parse(&json!({ "kind": "bar", "categories": ["a", "b"], "series": [{ "name": "s", "values": [3, null] }] })).unwrap();
        let svg = picture_svg(
            &bar,
            600,
            400,
            &PictureTheme::LIGHT,
            &PictureCredit::default(),
        );
        assert_eq!(
            svg.matches("<rect x=").count(),
            1,
            "a null value draws no bar"
        );
        let pie = Chart::parse(&json!({ "kind": "pie", "categories": ["x", "y", "z"], "series": [{ "name": "s", "values": [1, 1, 2] }] })).unwrap();
        let svg = picture_svg(
            &pie,
            600,
            400,
            &PictureTheme::LIGHT,
            &PictureCredit::default(),
        );
        assert_eq!(svg.matches("<path d=\"M").count(), 3, "three arcs");
        assert!(svg.contains(">50%</text>"));
        let gap = Chart::parse(&json!({ "kind": "area", "series": [{ "name": "s", "points": [[1, 1], [2, null], [3, 2], [4, 3]] }] })).unwrap();
        let svg = picture_svg(
            &gap,
            600,
            400,
            &PictureTheme::LIGHT,
            &PictureCredit::default(),
        );
        assert!(svg.contains("opacity=\"0.18\""), "area fill");
        assert!(svg.contains("<circle"), "the lone first point still shows");
        assert!(svg.contains("&lt;") || !svg.contains('<'.to_string().repeat(2).as_str()));
    }

    #[cfg(feature = "chart-png")]
    #[test]
    fn the_png_renders_with_the_bundled_fonts() {
        let png = picture_png(
            &chart(),
            &PictureCredit {
                left: "Affine · Arbos".into(),
            },
        )
        .unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        // 1200 × 675 in the IHDR.
        assert_eq!(&png[16..24], &[0, 0, 4, 176, 0, 0, 2, 163]);
        assert!(
            png.len() > 20_000 && png.len() < 400_000,
            "{} bytes",
            png.len()
        );
    }
}
