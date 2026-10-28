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
        "pencil",
        "<path d=\"M4 20l4.2-1 10.3-10.3a2.3 2.3 0 0 0-3.2-3.2L5 15.8z\"/><path d=\"M13.5 7.3l3.2 3.2\"/>"
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
    icon!(
        "hourglass",
        "<path d=\"M7 3h10M7 21h10M8 3c0 5 4 6 4 9s-4 4-4 9M16 3c0 5-4 6-4 9s4 4 4 9\"/>"
    ),
    icon!("xmark", "<path d=\"M6 6l12 12M18 6L6 18\"/>"),
    // Grok Bot's activity spinner (the Coding row while the bot works): eight
    // spokes fading round the dial; `cards::activity_spinner` steps it.
    icon!(
        "spinner",
        "<path d=\"M12.00 5.50L12.00 1.50\" stroke-width=\"2.4\" stroke-opacity=\"1.0\"/><path d=\"M16.60 7.40L19.42 4.58\" stroke-width=\"2.4\" stroke-opacity=\"0.85\"/><path d=\"M18.50 12.00L22.50 12.00\" stroke-width=\"2.4\" stroke-opacity=\"0.7\"/><path d=\"M16.60 16.60L19.42 19.42\" stroke-width=\"2.4\" stroke-opacity=\"0.55\"/><path d=\"M12.00 18.50L12.00 22.50\" stroke-width=\"2.4\" stroke-opacity=\"0.42\"/><path d=\"M7.40 16.60L4.58 19.42\" stroke-width=\"2.4\" stroke-opacity=\"0.3\"/><path d=\"M5.50 12.00L1.50 12.00\" stroke-width=\"2.4\" stroke-opacity=\"0.2\"/><path d=\"M7.40 7.40L4.58 4.58\" stroke-width=\"2.4\" stroke-opacity=\"0.12\"/>"
    ),
    // Grok Bot's share disc glyph (SF `square.and.arrow.up`): an open-topped
    // box with the arrow rising out of it. Measured 14 × 14 pt in the 36 pt
    // disc on `grok-right-pane-details.png` (2 px/pt); the mark spans 16 of
    // the 24 grid, so `glass::PANE_SHARE_GLYPH` 21 paints 14 pt.
    icon!(
        "share",
        "<path d=\"M8 10H6a2 2 0 0 0-2 2v6a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-6a2 2 0 0 0-2-2h-2\" stroke-width=\"1.8\"/><path d=\"M12 15V4M8.5 7.5L12 4l3.5 3.5\" stroke-width=\"1.8\"/>"
    ),
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
    // Group pane's Description row (SF `text.alignleft`): three left-aligned lines.
    icon!("text-align-left", "<path d=\"M4 7h16M4 12h11M4 17h14\"/>"),
    // Settings → Secrets (D83): key tile, pause switch, vault.
    icon!("key", "<circle cx=\"8\" cy=\"15\" r=\"4\"/><path d=\"M11 12l9-9M16 6l2 2M13 9l2 2\"/>"),
    icon!("pause", "<rect x=\"6\" y=\"5\" width=\"4\" height=\"14\" rx=\"1\" fill=\"#000\"/><rect x=\"14\" y=\"5\" width=\"4\" height=\"14\" rx=\"1\" fill=\"#000\"/>"),
    // Channel row menu (D88): archive box, and the box with its lid lifting for Unarchive.
    icon!(
        "archive",
        "<rect x=\"3\" y=\"4\" width=\"18\" height=\"5\" rx=\"1\"/><path d=\"M5 9v10a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V9\"/><path d=\"M10 13h4\"/>"
    ),
    icon!(
        "tray-up",
        "<path d=\"M4 14v5a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-5\"/><path d=\"M4 14h4l1.5 2h5l1.5-2h4\"/><path d=\"M12 11V4M9 7l3-3 3 3\"/>"
    ),
    icon!("lock", "<rect x=\"5\" y=\"11\" width=\"14\" height=\"10\" rx=\"2\"/><path d=\"M8 11V8a4 4 0 0 1 8 0v3\"/>"),
    // The Blocked state ([feedback blocked-state-mac-2026-10-26]): a circle with a slash.
    icon!("nosign", "<circle cx=\"12\" cy=\"12\" r=\"8.5\"/><path d=\"M6 6l12 12\"/>"),
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
    // Reply: arrow turning up-left (iPhone `arrowshape.turn.up.left`).
    icon!(
        "reply",
        "<path d=\"M9 14l-5-5 5-5\"/><path d=\"M4 9h9a6 6 0 0 1 6 6v4\"/>"
    ),
    // Forward (iPhone `arrowshape.turn.up.right`): the media viewer's Forward pill.
    icon!(
        "forward",
        "<path d=\"M15 14l5-5-5-5\"/><path d=\"M20 9h-9a6 6 0 0 0-6 6v4\"/>"
    ),
    // Speech bubble (iPhone `bubble.left`): the reply quote header, the message
    // menu's Reply in DM and the person pane's Message pill. One ellipse (centre
    // 12,10.5 · radii 8.5 × 6.5: top → right → the tail's root at the lower
    // left) with the tail's tip at 4.5,20, closed by the arc back up to the top,
    // so the outline meets itself — the earlier hand-drawn curves ended 1 pt
    // short of their start and `z` drew a stub there
    // ([feedback reply-in-dm-icon-2026-10-27]; `tests::no_glyph_closes_with_a_stub`).
    icon!(
        "bubble",
        "<path d=\"M12 4a8.5 6.5 0 0 1 8.5 6.5 8.5 6.5 0 0 1-11.4 6.1L4.5 20l-.5-7.3A8.5 6.5 0 0 1 12 4z\"/>"
    ),
    icon!("crown", "<path d=\"M4 17h16l1-9-5 3.5L12 5l-4 6.5L3 8z\" fill=\"#000\"/>"),
    icon!("photo", "<rect x=\"3\" y=\"5\" width=\"18\" height=\"14\" rx=\"2\"/><circle cx=\"9\" cy=\"10\" r=\"1.5\" fill=\"#000\"/><path d=\"M21 16l-5-5-8 8\"/>"),
    icon!("arrow-down", "<path d=\"M12 5v14M5 12l7 7 7-7\" stroke-width=\"2.6\"/>"),
    // Chart cards: Save PNG (arrow into a tray) and Expand (arrows out of the corners).
    icon!(
        "download",
        "<path d=\"M12 4v11M7.5 10.5L12 15l4.5-4.5\"/><path d=\"M4 17v2a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-2\"/>"
    ),
    icon!("expand", "<path d=\"M14 4h6v6M10 20H4v-6M20 4l-7 7M4 20l7-7\"/>"),
    // Grok Bot's update disc glyph (SF `icloud.and.arrow.down`): a cloud open
    // at the bottom — 8 pt dome, 3 pt lobes, straight sides — with the arrow
    // coming out of it. Traced pixel for pixel from the 72 px disc in Jacob's
    // Oct-2026 ref (glyph box 26 × 28 px, 2.5 px stroke): 15.6 × 16.2 on the
    // 24 grid at a 1.5 stroke, so `size` 20 paints Grok's 13 × 14 pt mark.
    icon!(
        "cloud-download",
        "<path d=\"M4.95 13.8V11.4A3 3 0 0 1 7.95 8.4A4.05 4.05 0 0 1 16.05 8.4A3 3 0 0 1 19.05 11.4V13.8\" stroke-width=\"1.5\"/><path d=\"M12 11.4V19M8.4 15.6L12 19L15.6 15.6\" stroke-width=\"1.5\"/>"
    ),
    icon!("cpu", "<rect x=\"6\" y=\"6\" width=\"12\" height=\"12\" rx=\"2\"/><rect x=\"10\" y=\"10\" width=\"4\" height=\"4\" fill=\"#000\" stroke=\"none\"/><path d=\"M9 2v4M15 2v4M9 18v4M15 18v4M2 9h4M2 15h4M18 9h4M18 15h4\"/>"),
    icon!("window", "<rect x=\"3\" y=\"4\" width=\"18\" height=\"16\" rx=\"2\"/><path d=\"M3 9h18\"/>"),
    // Settings root rows (D80 / iPhone parity): Credits, Machine, Appearance, About.
    icon!(
        "creditcard",
        "<rect x=\"2\" y=\"6\" width=\"20\" height=\"13\" rx=\"2\"/><path d=\"M2 11h20M6 16h4\"/>"
    ),
    icon!(
        "desktop",
        "<rect x=\"3\" y=\"4\" width=\"18\" height=\"12\" rx=\"2\"/><path d=\"M8 20h8M12 16v4\"/>"
    ),
    // Settings → Devices tiles (Oct 23).
    icon!(
        "laptop",
        "<rect x=\"3\" y=\"5\" width=\"18\" height=\"12\" rx=\"2\"/><path d=\"M2 19h20\"/>"
    ),
    icon!(
        "smartphone",
        "<rect x=\"7\" y=\"2\" width=\"10\" height=\"20\" rx=\"2\"/><path d=\"M11 18h2\"/>"
    ),
    icon!(
        "bot",
        "<rect x=\"4\" y=\"8\" width=\"16\" height=\"12\" rx=\"3\"/><path d=\"M12 4v4M9 14h.01M15 14h.01\"/>"
    ),
    icon!(
        "circle-half",
        "<circle cx=\"12\" cy=\"12\" r=\"9\"/><path d=\"M12 3a9 9 0 0 0 0 18z\" fill=\"#000\"/>"
    ),
    icon!("info", "<circle cx=\"12\" cy=\"12\" r=\"9\"/><path d=\"M12 11v6M12 8h.01\"/>"),
    icon!(
        "exclamation",
        "<circle cx=\"12\" cy=\"12\" r=\"9\"/><path d=\"M12 7v6M12 16.5h.01\" stroke-width=\"2.4\"/>"
    ),
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
    icon!(
        "trash",
        "<path d=\"M4 7h16M9 7V5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2M8 7l1 12a2 2 0 0 0 2 2h2a2 2 0 0 0 2-2l1-12M10 11v6M14 11v6\"/>"
    ),
    icon!(
        "branch",
        "<circle cx=\"6\" cy=\"6\" r=\"2.5\"/><circle cx=\"18\" cy=\"6\" r=\"2.5\"/><circle cx=\"18\" cy=\"18\" r=\"2.5\"/><path d=\"M6 8.5v5.5a4 4 0 0 0 4 4h5.5M18 8.5V15.5\"/>"
    ),
];

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        // D92 leaf avatars: `leaves/<slug>.svg` from the shared art in arbos-core.
        if let Some(bytes) = crate::ui::avatars::leaf_asset_bytes(path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        // The link tiles' bundled brand marks (`brands/<slug>.svg`).
        if let Some(bytes) = crate::ui::link_tile::brand_asset_bytes(path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
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

/// The icon pack's stroke weight on the 24 grid (the `icon!` header).
#[cfg(test)]
pub const STROKE: f32 = 2.0;

/// The drawn body of the glyph `name` — what sits inside the `icon!` header
/// and `</svg>` — for the lints below.
#[cfg(test)]
pub fn body(name: &str) -> Option<&'static str> {
    let path = format!("icons/{name}.svg");
    ICONS
        .iter()
        .find(|(p, _)| *p == path)
        .map(|(_, svg)| inner(svg))
}

#[cfg(test)]
fn inner(svg: &str) -> &str {
    let start = svg.find('>').map_or(0, |i| i + 1);
    let end = svg.rfind("</svg>").unwrap_or(svg.len());
    &svg[start..end]
}

/// Icon-pack lint ([feedback reply-in-dm-icon-2026-10-27]): the shape checks
/// that keep a hand-drawn glyph honest, run over every icon by the tests.
#[cfg(test)]
pub mod lint {
    /// One drawn subpath, walked from its `d` attribute.
    #[derive(Debug, Default, Clone, PartialEq)]
    pub struct Walk {
        /// Every anchor (end point) the path visits, in order.
        pub anchors: Vec<(f32, f32)>,
        /// For every `z`: the distance between the pen and the subpath's start
        /// when it closed. `0` = the curves met; a long gap = a real straight
        /// closing edge (a triangle's base); a short one = a stub.
        pub closing_gaps: Vec<f32>,
    }

    /// Walk an SVG path `d` string (M L H V C S Q T A Z, absolute and
    /// relative, implicit repeats) and report its anchors and closing gaps.
    pub fn walk(d: &str) -> Walk {
        let mut tokens: Vec<String> = Vec::new();
        let mut number = String::new();
        let flush = |number: &mut String, tokens: &mut Vec<String>| {
            if !number.is_empty() {
                tokens.push(std::mem::take(number));
            }
        };
        for ch in d.chars() {
            match ch {
                'a'..='z' | 'A'..='Z' => {
                    flush(&mut number, &mut tokens);
                    tokens.push(ch.to_string());
                }
                '-' => {
                    flush(&mut number, &mut tokens);
                    number.push(ch);
                }
                '.' if number.contains('.') => {
                    flush(&mut number, &mut tokens);
                    number.push(ch);
                }
                '0'..='9' | '.' => number.push(ch),
                _ => flush(&mut number, &mut tokens),
            }
        }
        flush(&mut number, &mut tokens);

        let mut out = Walk::default();
        let (mut cur, mut start) = ((0.0f32, 0.0f32), (0.0f32, 0.0f32));
        let mut cmd = 'M';
        let mut i = 0;
        let num = |tokens: &[String], i: &mut usize| -> f32 {
            let v = tokens.get(*i).and_then(|t| t.parse::<f32>().ok());
            *i += 1;
            v.unwrap_or_else(|| panic!("bad path token {:?} in {d:?}", tokens.get(*i - 1)))
        };
        while i < tokens.len() {
            if tokens[i]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
            {
                cmd = tokens[i].chars().next().unwrap();
                i += 1;
            } else if cmd == 'M' {
                cmd = 'L';
            } else if cmd == 'm' {
                cmd = 'l';
            }
            let rel = cmd.is_ascii_lowercase();
            let at = |cur: (f32, f32), x: f32, y: f32| {
                if rel {
                    (cur.0 + x, cur.1 + y)
                } else {
                    (x, y)
                }
            };
            match cmd.to_ascii_uppercase() {
                'M' => {
                    let (x, y) = (num(&tokens, &mut i), num(&tokens, &mut i));
                    cur = at(cur, x, y);
                    start = cur;
                    out.anchors.push(cur);
                }
                'L' | 'T' => {
                    let (x, y) = (num(&tokens, &mut i), num(&tokens, &mut i));
                    cur = at(cur, x, y);
                    out.anchors.push(cur);
                }
                'H' => {
                    let x = num(&tokens, &mut i);
                    cur = (if rel { cur.0 + x } else { x }, cur.1);
                    out.anchors.push(cur);
                }
                'V' => {
                    let y = num(&tokens, &mut i);
                    cur = (cur.0, if rel { cur.1 + y } else { y });
                    out.anchors.push(cur);
                }
                'C' | 'S' | 'Q' | 'A' => {
                    let skip = match cmd.to_ascii_uppercase() {
                        'C' => 4,
                        'S' | 'Q' => 2,
                        _ => 5,
                    };
                    for _ in 0..skip {
                        num(&tokens, &mut i);
                    }
                    let (x, y) = (num(&tokens, &mut i), num(&tokens, &mut i));
                    cur = at(cur, x, y);
                    out.anchors.push(cur);
                }
                'Z' => {
                    out.closing_gaps
                        .push(((cur.0 - start.0).powi(2) + (cur.1 - start.1).powi(2)).sqrt());
                    cur = start;
                }
                other => panic!("unknown path command {other:?} in {d:?}"),
            }
        }
        out
    }

    /// Every `d="…"` attribute in an icon body.
    pub fn paths(body: &str) -> Vec<&str> {
        body.split(" d=\"")
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .collect()
    }

    /// A `z` that lands within `stroke / 2` of its start but not on it draws
    /// a stub shorter than the line is wide: the curves were meant to meet
    /// and missed. That is the bubble's old defect, and what this guards.
    pub fn stubs(body: &str, stroke: f32) -> Vec<f32> {
        paths(body)
            .into_iter()
            .flat_map(|d| walk(d).closing_gaps)
            .filter(|gap| *gap > 0.05 && *gap < stroke)
            .collect()
    }

    /// Bounding box of every anchor in the body's paths, rects and circles:
    /// `(min_x, min_y, max_x, max_y)`.
    pub fn anchor_box(body: &str) -> (f32, f32, f32, f32) {
        let mut b = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        let mut take = |x: f32, y: f32| {
            b = (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y));
        };
        for d in paths(body) {
            for (x, y) in walk(d).anchors {
                take(x, y);
            }
        }
        let attr = |tag: &str, name: &str| -> Option<f32> {
            tag.split(&format!(" {name}=\""))
                .nth(1)?
                .split('"')
                .next()?
                .parse()
                .ok()
        };
        for tag in body.split('<').skip(1) {
            if tag.starts_with("rect") {
                if let (Some(x), Some(y), Some(w), Some(h)) = (
                    attr(tag, "x"),
                    attr(tag, "y"),
                    attr(tag, "width"),
                    attr(tag, "height"),
                ) {
                    take(x, y);
                    take(x + w, y + h);
                }
            } else if tag.starts_with("circle") {
                if let (Some(cx), Some(cy), Some(r)) =
                    (attr(tag, "cx"), attr(tag, "cy"), attr(tag, "r"))
                {
                    take(cx - r, cy - r);
                    take(cx + r, cy + r);
                }
            }
        }
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyph(name: &str) -> &'static str {
        body(name).unwrap_or_else(|| panic!("no glyph {name:?} in the pack"))
    }

    /// A closing `z` that lands within a stroke of its start but not on it is
    /// a stub (the old `bubble` ended 1 pt under where it began). Walks the
    /// whole pack, so the next hand-drawn glyph with a missed join fails here.
    #[test]
    fn no_glyph_closes_with_a_stub() {
        let broken: Vec<_> = ICONS
            .iter()
            .map(|(p, svg)| (*p, lint::stubs(inner(svg), STROKE)))
            .filter(|(_, stubs)| !stubs.is_empty())
            .collect();
        assert!(broken.is_empty(), "stubs in {broken:?}");
    }

    /// The message menu's glyphs are one set: outline strokes of the pack's
    /// weight (no `fill`, no `stroke-width` of their own) and one optical size
    /// (anchors inside the 24 grid's 3…21 band, 13…18 pt across) — Copy,
    /// Reply, Forward, Reply in DM and Delete read as siblings.
    #[test]
    fn message_menu_glyphs_share_one_set_weight_and_size() {
        let names: Vec<&str> = crate::ui::message_menu::ITEMS
            .iter()
            .map(|i| i.glyph())
            .collect();
        assert_eq!(names, ["copy", "reply", "forward", "bubble", "trash"]);
        for name in names {
            let b = glyph(name);
            assert!(
                !b.contains("fill=\"#000\""),
                "{name}: filled shape in an outline set"
            );
            assert!(!b.contains("stroke-width="), "{name}: own stroke weight");
            assert!(
                lint::stubs(b, STROKE).is_empty(),
                "{name}: closing stub {:?}",
                lint::stubs(b, STROKE)
            );
            let (x0, y0, x1, y1) = lint::anchor_box(b);
            assert!(
                x0 >= 3.0 && y0 >= 3.0 && x1 <= 21.0 && y1 <= 21.0,
                "{name}: anchors {:?} leave the 3…21 band",
                (x0, y0, x1, y1)
            );
            let (w, h) = (x1 - x0, y1 - y0);
            assert!(
                (13.0..=18.0).contains(&w.max(h)) && w.min(h) >= 12.0,
                "{name}: {w} × {h} is not the set's optical size"
            );
        }
    }

    /// The bubble: one ellipse plus a tail, meeting at the top — the pen
    /// returns exactly to its start before `z`, the tail's tip is the lowest
    /// point and sits left of centre.
    #[test]
    fn the_bubble_outline_closes_on_itself_with_a_lower_left_tail() {
        let b = glyph("bubble");
        let d = lint::paths(b);
        assert_eq!(d.len(), 1, "one outline, not several strokes");
        let walk = lint::walk(d[0]);
        assert_eq!(walk.closing_gaps, [0.0]);
        let first = walk.anchors[0];
        assert_eq!(*walk.anchors.last().unwrap(), first);
        let tip = walk
            .anchors
            .iter()
            .copied()
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        assert!(tip.0 < 12.0 && tip.1 >= 19.0, "tail tip {tip:?}");
        assert_eq!(first, (12.0, 4.0), "the join is at the top centre");
        // Every anchor sits on the ellipse or is the tail's tip.
        for (x, y) in walk.anchors.iter().copied().filter(|a| *a != tip) {
            let on = ((x - 12.0) / 8.5).powi(2) + ((y - 10.5) / 6.5).powi(2);
            assert!(
                (on - 1.0).abs() < 0.02,
                "({x}, {y}) is off the ellipse: {on}"
            );
        }
    }

    fn near(a: &[(f32, f32)], b: &[(f32, f32)]) -> bool {
        a.len() == b.len()
            && a.iter()
                .zip(b)
                .all(|(p, q)| (p.0 - q.0).abs() < 1e-4 && (p.1 - q.1).abs() < 1e-4)
    }

    #[test]
    fn the_walker_reads_every_command_form() {
        let w = lint::walk("M1 1h2v2H1V1zm5 5l1 1-1 1Z");
        assert!(near(
            &w.anchors,
            &[
                (1.0, 1.0),
                (3.0, 1.0),
                (3.0, 3.0),
                (1.0, 3.0),
                (1.0, 1.0),
                (6.0, 6.0),
                (7.0, 7.0),
                (6.0, 8.0)
            ]
        ));
        // The square returns home; the open chevron closes with a 2-long edge.
        assert_eq!(w.closing_gaps, [0.0, 2.0]);
        let arcs = lint::walk("M12 4a8.5 6.5 0 1 1-2.9 12.6A8.5 6.5 0 0 1 12 4z");
        assert!(near(
            &arcs.anchors,
            &[(12.0, 4.0), (9.1, 16.6), (12.0, 4.0)]
        ));
        let curves = lint::walk("M0 0c1 1 2 2 3 3s1 1 2 2q1 1 2 2t1 1");
        assert!(near(
            &curves.anchors[1..],
            &[(3.0, 3.0), (5.0, 5.0), (7.0, 7.0), (8.0, 8.0)]
        ));
        // `.5.5` is two numbers, as in the pack's hand-written paths.
        assert!(near(
            &lint::walk("M.5.5l.5.5").anchors,
            &[(0.5, 0.5), (1.0, 1.0)]
        ));
        // Rects and circles count toward the box.
        assert_eq!(
            lint::anchor_box("<rect x=\"9\" y=\"9\" width=\"11\" height=\"11\" rx=\"2\"/><circle cx=\"5\" cy=\"5\" r=\"2\"/>"),
            (3.0, 3.0, 20.0, 20.0)
        );
    }
}
