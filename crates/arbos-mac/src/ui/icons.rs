//! SF-Symbol-like glyphs, embedded as tiny SVGs and painted through GPUI's
//! `svg()` element in the current text colour. Hand-drawn strokes on a 24 pt
//! grid (round caps, 2 pt weight) to match the iPhone app's header controls.

use std::borrow::Cow;

use gpui::{prelude::*, px, svg, AssetSource, Hsla, Result, SharedString, Svg};

pub struct Assets;

macro_rules! icon {
    ($name:literal, $body:literal) => {
        (
            concat!("icons/", $name, ".svg"),
            concat!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\" fill=\"none\" ",
                "stroke=\"#000\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\">",
                $body,
                "</svg>"
            ),
        )
    };
}

const ICONS: &[(&str, &str)] = &[
    icon!("plus", "<path d=\"M12 5v14M5 12h14\"/>"),
    icon!("search", "<circle cx=\"11\" cy=\"11\" r=\"6.5\"/><path d=\"M16 16l4.5 4.5\"/>"),
    icon!(
        "person",
        "<circle cx=\"12\" cy=\"8\" r=\"4\" fill=\"#000\"/><path d=\"M4 21c0-4 3.6-7 8-7s8 3 8 7z\" fill=\"#000\"/>"
    ),
    icon!(
        "ellipsis",
        "<circle cx=\"5\" cy=\"12\" r=\"1.8\" fill=\"#000\" stroke=\"none\"/><circle cx=\"12\" cy=\"12\" r=\"1.8\" fill=\"#000\" stroke=\"none\"/><circle cx=\"19\" cy=\"12\" r=\"1.8\" fill=\"#000\" stroke=\"none\"/>"
    ),
    icon!(
        "sidebar-left",
        "<rect x=\"3\" y=\"5\" width=\"18\" height=\"14\" rx=\"2.5\"/><path d=\"M9.5 5v14\"/>"
    ),
    icon!("chevron-left", "<path d=\"M15 5l-7 7 7 7\"/>"),
    icon!("chevron-right", "<path d=\"M9 5l7 7-7 7\"/>"),
    icon!("chevron-down", "<path d=\"M5 9l7 7 7-7\"/>"),
    icon!("chevron-up", "<path d=\"M5 15l7-7 7 7\"/>"),
    icon!(
        "eye",
        "<path d=\"M2 12s3.5-6.5 10-6.5S22 12 22 12s-3.5 6.5-10 6.5S2 12 2 12z\"/><circle cx=\"12\" cy=\"12\" r=\"3\"/>"
    ),
    icon!(
        "mic",
        "<rect x=\"9\" y=\"3\" width=\"6\" height=\"11\" rx=\"3\"/><path d=\"M5 11a7 7 0 0 0 14 0M12 18v3\"/>"
    ),
    icon!(
        "waveform",
        "<path d=\"M4 10v4M8 7v10M12 4v16M16 8v8M20 10v4\" stroke-width=\"2.4\"/>"
    ),
    icon!("arrow-up", "<path d=\"M12 19V5M5 12l7-7 7 7\" stroke-width=\"2.6\"/>"),
    icon!("xmark", "<path d=\"M6 6l12 12M18 6L6 18\"/>"),
    icon!(
        "archivebox",
        "<rect x=\"3\" y=\"4\" width=\"18\" height=\"4\" rx=\"1\"/><path d=\"M5 8v10a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8M10 12h4\"/>"
    ),
    icon!(
        "square-pencil",
        "<path d=\"M12 4H6a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-6\"/><path d=\"M18 3l3 3-9 9H9v-3z\"/>"
    ),
    icon!(
        "person-plus",
        "<circle cx=\"10\" cy=\"8\" r=\"4\" fill=\"#000\"/><path d=\"M2 21c0-4 3.6-7 8-7 1.3 0 2.5.3 3.6.7\" fill=\"#000\"/><path d=\"M19 13v6M16 16h6\"/>"
    ),
    icon!(
        "people",
        "<circle cx=\"9\" cy=\"8\" r=\"3.5\" fill=\"#000\"/><path d=\"M2 20c0-3.5 3.1-6 7-6s7 2.5 7 6z\" fill=\"#000\"/><circle cx=\"17\" cy=\"8.5\" r=\"3\" fill=\"#000\"/><path d=\"M17 14c2.9 0 5 2.2 5 5h-5.5\" fill=\"#000\"/>"
    ),
    icon!("stop", "<rect x=\"6\" y=\"6\" width=\"12\" height=\"12\" rx=\"2\" fill=\"#000\"/>"),
    icon!("play", "<path d=\"M8 5.5v13l10-6.5z\" fill=\"#000\" stroke-linejoin=\"round\"/>"),
    icon!(
        "paperclip",
        "<path d=\"M20 12.5l-7.8 7.8a5 5 0 0 1-7-7L13.5 5a3.3 3.3 0 0 1 4.7 4.7l-8.3 8.3a1.6 1.6 0 0 1-2.3-2.3L15 8.4\"/>"
    ),
    // Cog with teeth (SF Symbol gearshape); the old path was a sun with rays.
    icon!(
        "gear",
"<circle cx=\"12\" cy=\"12\" r=\"3\"/><path d=\"M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z\"/>"
    ),
    icon!(
        "sparkles",
        "<path d=\"M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8z\" fill=\"#000\"/><path d=\"M19 16l.8 2.2L22 19l-2.2.8L19 22l-.8-2.2L16 19l2.2-.8z\" fill=\"#000\"/>"
    ),
    icon!("bell", "<path d=\"M6 17V11a6 6 0 0 1 12 0v6l1.5 2h-15z\"/><path d=\"M10 21a2 2 0 0 0 4 0\"/>"),
    icon!("phone", "<path d=\"M5 4h4l2 5-2.5 1.5a11 11 0 0 0 5 5L15 13l5 2v4a2 2 0 0 1-2 2A16 16 0 0 1 3 6a2 2 0 0 1 2-2z\"/>"),
    icon!("check", "<path d=\"M5 12.5l4.5 4.5L19 7\" stroke-width=\"2.6\"/>"),
    icon!("globe", "<circle cx=\"12\" cy=\"12\" r=\"9\"/><path d=\"M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18\"/>"),
    icon!("doc", "<path d=\"M7 3h7l5 5v13H7z\"/><path d=\"M14 3v5h5\"/>"),
    // Settings → Secrets (D83): key tile, pause switch, vault.
    icon!("key", "<circle cx=\"8\" cy=\"15\" r=\"4\"/><path d=\"M11 12l9-9M16 6l2 2M13 9l2 2\"/>"),
    icon!("pause", "<rect x=\"6\" y=\"5\" width=\"4\" height=\"14\" rx=\"1\" fill=\"#000\"/><rect x=\"14\" y=\"5\" width=\"4\" height=\"14\" rx=\"1\" fill=\"#000\"/>"),
    icon!("lock", "<rect x=\"5\" y=\"11\" width=\"14\" height=\"10\" rx=\"2\"/><path d=\"M8 11V8a4 4 0 0 1 8 0v3\"/>"),
    // A secret whose real value is in the agent's environment (D83 decision 2).
    icon!("terminal", "<rect x=\"3\" y=\"5\" width=\"18\" height=\"14\" rx=\"2\"/><path d=\"M7 9l3 3-3 3M12 15h5\"/>"),
    // Profile pages (Telegram-style action rows).
    icon!("video", "<rect x=\"3\" y=\"7\" width=\"12\" height=\"10\" rx=\"2\"/><path d=\"M15 11l6-3v8l-6-3z\"/>"),
    icon!(
        "bell-slash",
        "<path d=\"M8 8.5A6 6 0 0 1 18 11v6l1.5 2H8\"/><path d=\"M6 11v6l-1.5 2H5\"/><path d=\"M10 21a2 2 0 0 0 4 0\"/><path d=\"M4 4l16 16\"/>"
    ),
    icon!("link", "<path d=\"M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1.5 1.5\"/><path d=\"M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1.5-1.5\"/>"),
    icon!("leave", "<path d=\"M10 4H6a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h4\"/><path d=\"M14 8l4 4-4 4M18 12H9\"/>"),
    icon!("block", "<circle cx=\"12\" cy=\"12\" r=\"8.5\"/><path d=\"M6 6l12 12\"/>"),
    icon!("copy", "<rect x=\"9\" y=\"9\" width=\"11\" height=\"11\" rx=\"2\"/><path d=\"M5 15V6a2 2 0 0 1 2-2h9\"/>"),
    icon!("crown", "<path d=\"M4 17h16l1-9-5 3.5L12 5l-4 6.5L3 8z\" fill=\"#000\"/>"),
    icon!("photo", "<rect x=\"3\" y=\"5\" width=\"18\" height=\"14\" rx=\"2\"/><circle cx=\"9\" cy=\"10\" r=\"1.5\" fill=\"#000\"/><path d=\"M21 16l-5-5-8 8\"/>"),
    icon!("arrow-down", "<path d=\"M12 5v14M5 12l7 7 7-7\" stroke-width=\"2.6\"/>"),
    icon!("cpu", "<rect x=\"6\" y=\"6\" width=\"12\" height=\"12\" rx=\"2\"/><rect x=\"10\" y=\"10\" width=\"4\" height=\"4\" fill=\"#000\" stroke=\"none\"/><path d=\"M9 2v4M15 2v4M9 18v4M15 18v4M2 9h4M2 15h4M18 9h4M18 15h4\"/>"),
    icon!("window", "<rect x=\"3\" y=\"4\" width=\"18\" height=\"16\" rx=\"2\"/><path d=\"M3 9h18\"/>"),
    icon!("minus", "<path d=\"M5 12h14\"/>"),
    icon!(
        "envelope",
        "<rect x=\"3\" y=\"5\" width=\"18\" height=\"14\" rx=\"2.5\"/><path d=\"M4 8l8 5.5L20 8\"/>"
    ),
    icon!("hash", "<path d=\"M9 4L7 20M17 4l-2 16M4 9h17M3 15h17\" stroke-width=\"2.2\"/>"),
    icon!("code", "<path d=\"M8 7l-5 5 5 5M16 7l5 5-5 5M13.5 4l-3 16\" stroke-width=\"2.2\"/>"),
    icon!(
        "antenna",
        "<circle cx=\"12\" cy=\"12\" r=\"2\" fill=\"#000\" stroke=\"none\"/><path d=\"M12 14v7M8.5 8.5a5 5 0 0 0 0 7M15.5 8.5a5 5 0 0 1 0 7M5.5 5.5a9 9 0 0 0 0 13M18.5 5.5a9 9 0 0 1 0 13\"/>"
    ),
    icon!("puzzle", "<path d=\"M10 3a2 2 0 0 1 2 2v1h3a2 2 0 0 1 2 2v3h1a2 2 0 1 1 0 4h-1v3a2 2 0 0 1-2 2h-3v-1a2 2 0 1 0-4 0v1H5a2 2 0 0 1-2-2v-3h1a2 2 0 1 0 0-4H3V8a2 2 0 0 1 2-2h3V5a2 2 0 0 1 2-2z\"/>"),
    icon!(
        "face-smile",
        "<circle cx=\"12\" cy=\"12\" r=\"9\"/><path d=\"M8.3 14.3c.9 1.3 2.2 2 3.7 2s2.8-.7 3.7-2\"/><circle cx=\"9.2\" cy=\"9.8\" r=\"1.1\" fill=\"#000\" stroke=\"none\"/><circle cx=\"14.8\" cy=\"9.8\" r=\"1.1\" fill=\"#000\" stroke=\"none\"/>"
    ),
    icon!("camera", "<path d=\"M3 7h11a2 2 0 0 1 2 2v6a2 2 0 0 1-2 2H3z\" fill=\"#000\"/><path d=\"M16 10l5-3v10l-5-3z\" fill=\"#000\"/>"),
    icon!("camera-off", "<path d=\"M3 7h9l6 6v2a2 2 0 0 1-2 2H3z\" fill=\"#000\"/><path d=\"M16 10l5-3v10\" fill=\"#000\"/><path d=\"M3 3l18 18\" stroke-width=\"2.4\"/>"),
    icon!("mic-off", "<rect x=\"9\" y=\"3\" width=\"6\" height=\"11\" rx=\"3\"/><path d=\"M5 11a7 7 0 0 0 11.5 5.3M19 11a7 7 0 0 1-.9 3.4M12 18v3\"/><path d=\"M3 3l18 18\" stroke-width=\"2.4\"/>"),
    icon!("phone-down", "<path d=\"M3 14c5-5 13-5 18 0l-2.5 2.5-3-1.5v-2.5a9 9 0 0 0-7 0V15l-3 1.5z\" fill=\"#000\"/>"),
];

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, body)| Cow::Borrowed(body.as_bytes())))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .filter(|(p, _)| p.starts_with(path))
            .map(|(p, _)| SharedString::from(*p))
            .collect())
    }
}

/// `icon("plus", 16.0, color)` — a square glyph of `size` points.
pub fn icon(name: &str, size: f32, color: Hsla) -> Svg {
    svg()
        .path(SharedString::from(format!("icons/{name}.svg")))
        .size(px(size))
        .flex_shrink_0()
        .text_color(color)
}
