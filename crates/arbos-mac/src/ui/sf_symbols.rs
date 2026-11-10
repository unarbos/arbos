//! Apple's SF Symbols, by name, as GPUI elements — the same symbols the iPhone
//! draws with `Image(systemName:)` ([feedback menu-sf-symbols-2026-10-27];
//! Jacob: "use standard versions instead of making them").
//!
//! **How a symbol is painted.** macOS keeps SF Symbols as vector artwork
//! behind `NSImage(systemSymbolName:)`; the system font does not carry them
//! under their names (`CTFontGetGlyphWithName` finds `plus`, `minus` and
//! `ellipsis` and nothing else — probed on macOS 26.6), so there is no text or
//! path route. The symbol is therefore rasterised the way AppKit's own menus
//! do it: `NSImage(systemSymbolName:)` + `NSImageSymbolConfiguration(pointSize,
//! weight, scale: .medium)`, drawn as the template it is into an
//! `NSBitmapImageRep`, then tinted with the text colour through a `sourceIn`
//! fill, and handed to GPUI as a BGRA [`RenderImage`]. One
//! raster per (name, size, weight, scale, colour), cached for the app's
//! lifetime — a handful of KB. The element is laid out inside a fixed square
//! slot, so rows align on the slot while each symbol keeps Apple's
//! proportions.
//!
//! **Straight alpha, not premultiplied.** `RenderImage` holds BGRA with
//! **un-premultiplied** alpha — GPUI's own SVG renderer takes tiny-skia's
//! premultiplied pixmap and runs every pixel through `swap_rgba_pa_to_bgra`
//! before it builds one. [`render_pixel`] does the same here, because an
//! `NSBitmapImageRep` with an empty `NSBitmapFormat` is premultiplied too.
//! Handing that buffer over unconverted multiplies the colour by its alpha a
//! second time, and the damage is worth spelling out: a stroke tinted
//! `label_muted` (white at 72 %) over the group pane's tile should paint 207,
//! and painted 156; an edge pixel at a quarter coverage should sit at 115 and
//! sank to 78, **below the 85 of the tile behind it**. Every symbol in the app
//! therefore wore a dark rim it had no business having and never reached its
//! own colour — read off the build as thin, faint and grainy, which is one
//! fault wearing three names rather than three values anyone chose.
//!
//! **The pixel grid.** The raster is a bitmap, so it also has to cover whole
//! device pixels and land on them. Three sizes have to agree, and
//! [`whole_points`] / [`snap_box`] / [`raster_pixels`] are the three steps
//! that make them:
//!
//! 1. the **point size** the symbol is configured at is a whole number, so
//!    AppKit lays Apple's artwork out on the pixel grid rather than between it;
//! 2. the **box** the element occupies is a whole number of points, so it
//!    covers an even number of device pixels at 2× — the same parity as the
//!    whole-point tile around it, which is what makes the glyph land exactly
//!    on the tile's centre (`centring` pins this with measured bounds);
//! 3. the **raster** is exactly that box times the backing scale, so GPUI
//!    blits it 1:1 instead of rescaling it.
//!
//! Before that rule the box was the symbol's raw natural size and the raster
//! `ceil(natural × scale)`, both off the grid for any configuration whose
//! natural size is fractional — which is most of them, because [`glyph`] asked
//! for `box × 0.85` points (an 18 pt box gave 15.3). A symbol then sat a
//! device pixel off the centre of its tile, and neighbouring strokes picked up
//! different coverage, so some read crisper than others.
//!
//! **The slot does not have to be whole, and that is not an oversight.** Three
//! callers pass a fractional one (`11.5` in the people search field, `24 ×
//! 0.58` in the palette, `72 × 0.45` on a profile hero). It looks like the
//! same fault as a fractional box and it is not: GPUI rounds only the *final
//! absolute* corners of a box, so a half device pixel inside the invisible
//! slot is cancelled by the other half when the glyph's own corners are
//! rounded against the tile. Measured through a real window, every tile the
//! app draws centres its glyph exactly with the slots as they stand, while a
//! fractional **box** is a device pixel out in every one of them. Rounding the
//! slots would buy none of that back, and would cost: the search field's
//! placeholder shifts half a point off the width measured from Grok Mac, and
//! the hero's glyph drops from 28 pt to 27 because [`glyph`] takes its point
//! size from the slot. So the slots are left as their callers wrote them.
//!
//! **Fallback.** On a macOS that lacks a name (`NSImage` answers nil), and on
//! every other OS (the Linux dev build and the headless tests), the pack glyph
//! named in [`FALLBACKS`] is drawn through `icons::icon` at the slot size.
//! Nothing else may draw from the pack — `icons::tests` pin it.
//!
//! **Every surface** ([feedback sf-symbols-everywhere-mac-2026-10-28]): [`glyph`]
//! is the drop-in for the old `icons::icon(name, size, color)` — same box, same
//! layout — with the symbol at `size × 0.85` points, rounded to the nearest
//! whole point by [`whole_points`] (the menu precedent: 13 in a 15 box),
//! **medium** weight (the pack's 2 pt stroke on the 24 grid ≈ SF
//! Medium) and semibold for the few marks the pack drew heavier (checkmark,
//! the arrows, the waveform, number, code, the exclamation).

use gpui::{div, prelude::*, px, Div, Hsla};

use crate::ui::icons::icon;

/// Symbol weight, matched to the label beside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Weight {
    Regular,
    Medium,
    Semibold,
}

/// The symbol's point size for a glyph box of `size` (the old `icon` box).
pub const GLYPH_POINT_RATIO: f32 = 0.85;

/// Step 1 of the pixel grid: the whole point size a symbol is configured at.
/// Apple's canvas for a symbol is a whole number of points at a whole point
/// size. `18 × 0.85` is 15.3, and a 15.3 pt canvas lays strokes down between
/// device pixels instead of on them, so neighbours end up with different
/// coverage and read as unevenly drawn.
pub fn whole_points(size: f32) -> f32 {
    size.round().max(1.0)
}

/// Step 2: the box an element gives a symbol of `natural` points — whole
/// points, so the slot centres it by a whole device pixel at 2×. AppKit
/// scales the *vector* artwork into this box when it rasterises, so the
/// rounding costs sharpness nothing; leaving the box fractional cost it
/// everything.
///
/// Only the mac raster calls this — a build without system symbols draws the
/// pack through `icons::icon`, which GPUI paints as a vector at any size — but
/// the rule it states is pinned by this module's tests on every platform.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn snap_box(natural: (f32, f32)) -> (f32, f32) {
    (natural.0.round().max(1.0), natural.1.round().max(1.0))
}

/// Step 3: the raster for a `box_pt` point box on a `scale`× display —
/// exactly the device pixels the box covers, so GPUI blits it 1:1. The old
/// `ceil(natural × scale)` made a bitmap a fraction of a pixel wider than its
/// box and left GPUI to resample the difference away. Mac-only for the same
/// reason as [`snap_box`].
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn raster_pixels(box_pt: (f32, f32), scale: f32) -> (u32, u32) {
    let px = |v: f32| ((v * scale).round() as u32).max(1);
    (px(box_pt.0), px(box_pt.1))
}

/// One raster pixel the way [`RenderImage`](gpui::RenderImage) reads it: the
/// channels swapped to BGRA **and the alpha divided back out**, through the
/// same `swap_rgba_pa_to_bgra` GPUI's SVG renderer uses. An
/// `NSBitmapImageRep` hands back premultiplied RGBA; passing that straight on
/// applies the alpha twice, which is the dark rim and the washed-out stroke
/// described in the module doc. Mac-only for the same reason as [`snap_box`].
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn render_pixel(premultiplied_rgba: [u8; 4]) -> [u8; 4] {
    let mut pixel = premultiplied_rgba;
    gpui::swap_rgba_pa_to_bgra(&mut pixel);
    pixel
}

/// Marks the pack drew with a 2.2–2.6 stroke: semibold, so they keep their weight.
const HEAVIER: &[&str] = &[
    "checkmark",
    "arrow.up",
    "arrow.down",
    "waveform",
    "number",
    "chevron.left.forwardslash.chevron.right",
    "exclamationmark.circle",
];

/// Drop-in for `icons::icon(name, size, color)`: the system symbol `name` in a
/// `size` square (the old glyph box), medium weight — semibold for [`HEAVIER`].
pub fn glyph(name: &str, size: f32, color: Hsla) -> Div {
    let weight = if HEAVIER.contains(&name) {
        Weight::Semibold
    } else {
        Weight::Medium
    };
    symbol(name, size * GLYPH_POINT_RATIO, weight, size, color)
}

/// System symbol name → the pack glyph drawn where the symbol is unavailable
/// (and on non-mac builds). Every name any surface draws is here; the lint
/// reads this table. The first block is the menus' set (#2124), the rest the
/// surfaces converted by [feedback sf-symbols-everywhere-mac-2026-10-28].
pub const FALLBACKS: &[(&str, &str)] = &[
    ("doc.on.doc", "copy"),
    ("arrowshape.turn.up.left", "reply"),
    ("arrowshape.turn.up.right", "forward"),
    ("bubble.left", "bubble"),
    ("trash", "trash"),
    ("archivebox", "archive"),
    ("tray.and.arrow.up", "tray-up"),
    ("bell", "bell"),
    ("bell.slash", "bell-slash"),
    ("arrow.triangle.branch", "branch"),
    ("person.crop.circle", "person"),
    ("pencil", "pencil"),
    ("plus", "plus"),
    ("photo", "photo"),
    ("arrow.down.to.line", "download"),
    ("globe", "globe"),
    ("link", "link"),
    ("xmark", "xmark"),
    ("doc.on.clipboard", "doc"),
    ("nosign", "nosign"),
    ("magnifyingglass", "search"),
    ("person.fill", "person"),
    ("ellipsis", "ellipsis"),
    ("sidebar.left", "sidebar-left"),
    ("chevron.left", "chevron-left"),
    ("chevron.right", "chevron-right"),
    ("chevron.down", "chevron-down"),
    ("chevron.up", "chevron-up"),
    ("eye", "eye"),
    ("mic", "mic"),
    ("waveform", "waveform"),
    ("arrow.up", "arrow-up"),
    ("hourglass", "hourglass"),
    ("square.and.arrow.up", "share"),
    ("square.and.pencil", "square-pencil"),
    ("person.badge.plus", "person-plus"),
    ("person.badge.key", "key"),
    ("person.badge.minus", "person"),
    ("person.2.fill", "people"),
    ("stop.fill", "stop"),
    ("play.fill", "play"),
    ("paperclip", "paperclip"),
    ("gearshape", "gear"),
    ("sparkles", "sparkles"),
    ("phone", "phone"),
    ("phone.fill", "phone"),
    ("checkmark", "check"),
    ("doc", "doc"),
    ("key", "key"),
    ("pause.fill", "pause"),
    ("lock", "lock"),
    ("terminal", "terminal"),
    ("video", "video"),
    ("rectangle.portrait.and.arrow.right", "leave"),
    ("crown.fill", "crown"),
    ("arrow.down", "arrow-down"),
    ("arrow.up.left.and.arrow.down.right", "expand"),
    ("icloud.and.arrow.down", "cloud-download"),
    ("cpu", "cpu"),
    ("macwindow", "window"),
    ("creditcard", "creditcard"),
    ("desktopcomputer", "desktop"),
    ("laptopcomputer", "laptop"),
    ("iphone", "smartphone"),
    ("server.rack", "bot"),
    ("circle.lefthalf.filled", "circle-half"),
    ("info.circle", "info"),
    ("exclamationmark.circle", "exclamation"),
    ("exclamationmark.triangle", "exclamation"),
    ("minus", "minus"),
    ("envelope", "envelope"),
    ("number", "hash"),
    ("chevron.left.forwardslash.chevron.right", "code"),
    ("antenna.radiowaves.left.and.right", "antenna"),
    ("puzzlepiece", "puzzle"),
    ("face.smiling", "face-smile"),
    ("video.fill", "camera"),
    ("video.slash.fill", "camera-off"),
    ("mic.slash", "mic-off"),
    ("phone.down.fill", "phone-down"),
    ("text.alignleft", "text-align-left"),
    // Sidebar pinning ([feedback sidebar-pinning-mac-2026-10-28]): the row menu's Pin /
    // Unpin and the pinned row's mark — the iPhone's `pin` / `pin.slash` / `pin.fill`.
    ("pin", "pin"),
    ("pin.slash", "pin-slash"),
    ("pin.fill", "pin-fill"),
];

/// The pack glyph that stands in for `name`, if `name` is a registered symbol.
pub fn fallback(name: &str) -> Option<&'static str> {
    FALLBACKS
        .iter()
        .find(|(sf, _)| *sf == name)
        .map(|(_, glyph)| *glyph)
}

/// Is `name` a symbol this app may draw (registered with a fallback)?
pub fn is_registered(name: &str) -> bool {
    fallback(name).is_some()
}

/// The system symbol `name` at `size` points (its font point size — the
/// label's size for a menu row), `weight`, tinted `color`, inside a `slot`
/// square so neighbours align. Falls back to the pack glyph (see the module
/// doc). Panics in debug builds on a name that is not registered: register
/// it in [`FALLBACKS`] first.
pub fn symbol(name: &str, size: f32, weight: Weight, slot: f32, color: Hsla) -> Div {
    debug_assert!(
        is_registered(name),
        "{name:?} is not a registered SF Symbol — add it to sf_symbols::FALLBACKS"
    );
    let size = whole_points(size);
    let frame = div()
        .size(px(slot))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center();
    #[cfg(target_os = "macos")]
    if let Some(natural) = mac::natural_size(name, size, weight) {
        return frame.child(mac::element(name, size, weight, color, snap_box(natural)));
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (size, weight);
    let glyph = fallback(name).unwrap_or("xmark");
    frame.child(icon(glyph, slot, color))
}

#[cfg(target_os = "macos")]
mod mac {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};

    use gpui::{img, px, Hsla, ImageSource, Img, RenderImage, Styled};
    use objc2::AnyThread;
    use objc2_app_kit::{
        NSBitmapFormat, NSBitmapImageRep, NSCalibratedRGBColorSpace, NSColor,
        NSCompositingOperation, NSFontWeightMedium, NSFontWeightRegular, NSFontWeightSemibold,
        NSGraphicsContext, NSImage, NSImageSymbolConfiguration, NSImageSymbolScale,
        NSRectFillUsingOperation,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

    use super::Weight;

    type SizeKey = (String, u32, Weight);
    type RasterKey = (String, u32, Weight, u32, [u8; 4]);

    type Sizes = Mutex<HashMap<SizeKey, Option<(f32, f32)>>>;

    fn sizes() -> &'static Sizes {
        static SIZES: OnceLock<Sizes> = OnceLock::new();
        SIZES.get_or_init(Default::default)
    }

    fn rasters() -> &'static Mutex<HashMap<RasterKey, Option<Arc<RenderImage>>>> {
        static RASTERS: OnceLock<Mutex<HashMap<RasterKey, Option<Arc<RenderImage>>>>> =
            OnceLock::new();
        RASTERS.get_or_init(Default::default)
    }

    fn font_weight(weight: Weight) -> f64 {
        // SAFETY: AppKit constants, read-only.
        unsafe {
            match weight {
                Weight::Regular => NSFontWeightRegular,
                Weight::Medium => NSFontWeightMedium,
                Weight::Semibold => NSFontWeightSemibold,
            }
        }
    }

    /// The configured symbol image, or `None` when this macOS has no symbol of
    /// that name.
    fn configured(name: &str, size: f32, weight: Weight) -> Option<objc2::rc::Retained<NSImage>> {
        let ns_name = NSString::from_str(name);
        let base = NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns_name, None)?;
        let config = NSImageSymbolConfiguration::configurationWithPointSize_weight_scale(
            size as f64,
            font_weight(weight),
            NSImageSymbolScale::Medium,
        );
        base.imageWithSymbolConfiguration(&config)
    }

    /// The symbol's natural size in points at this configuration (Apple's
    /// canvas for it — a `trash` at 13 pt is 15 × 17), or `None` if unknown.
    pub fn natural_size(name: &str, size: f32, weight: Weight) -> Option<(f32, f32)> {
        let key = (name.to_owned(), (size * 100.0) as u32, weight);
        if let Some(hit) = sizes().lock().ok()?.get(&key) {
            return *hit;
        }
        let found = configured(name, size, weight).map(|image| {
            let s = image.size();
            (s.width as f32, s.height as f32)
        });
        sizes().lock().ok()?.insert(key, found);
        found
    }

    /// The image element in a `box_pt` box: the raster is produced at paint
    /// time, when the window's scale factor is known, and cached.
    ///
    /// The box is applied here rather than left to the caller because the
    /// raster is measured in **device** pixels and there is no way to say so:
    /// `RenderImage::scale_factor` is crate-private to GPUI, which its own SVG
    /// renderer sets and nothing outside it can. An `img` left to size itself
    /// therefore takes its pixel dimensions as points, and every symbol would
    /// come out twice too large on a 2× display.
    pub fn element(name: &str, size: f32, weight: Weight, color: Hsla, box_pt: (f32, f32)) -> Img {
        let name: Arc<str> = name.into();
        img(ImageSource::Custom(Arc::new(move |window, _cx| {
            raster(&name, size, weight, window.scale_factor(), color).map(Ok)
        })))
        .w(px(box_pt.0))
        .h(px(box_pt.1))
    }

    fn raster(
        name: &str,
        size: f32,
        weight: Weight,
        scale: f32,
        color: Hsla,
    ) -> Option<Arc<RenderImage>> {
        let rgba = color.to_rgb();
        let tint = [
            (rgba.r * 255.0).round() as u8,
            (rgba.g * 255.0).round() as u8,
            (rgba.b * 255.0).round() as u8,
            (rgba.a * 255.0).round() as u8,
        ];
        let key = (
            name.to_owned(),
            (size * 100.0) as u32,
            weight,
            (scale * 100.0) as u32,
            tint,
        );
        if let Some(hit) = rasters().lock().ok()?.get(&key) {
            return hit.clone();
        }
        let made = draw(name, size, weight, scale, rgba);
        rasters().lock().ok()?.insert(key, made.clone());
        made
    }

    /// Draw the template symbol into a bitmap of `natural × scale` pixels and
    /// tint it: premultiplied BGRA, the format [`RenderImage`] expects.
    fn draw(
        name: &str,
        size: f32,
        weight: Weight,
        scale: f32,
        rgba: gpui::Rgba,
    ) -> Option<Arc<RenderImage>> {
        let image = configured(name, size, weight)?;
        let natural = image.size();
        // The same box `symbol` lays the element out in, so the bitmap is the
        // element's device pixels exactly and GPUI never resamples it.
        let box_pt = super::snap_box((natural.width as f32, natural.height as f32));
        let (w_px, h_px) = super::raster_pixels(box_pt, scale);
        let (width, height) = (w_px as isize, h_px as isize);
        // SAFETY: plain AppKit drawing on the main thread (GPUI paints there);
        // the rep owns its buffer and outlives every pointer read below.
        let pixels = unsafe {
            let rep = NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
                NSBitmapImageRep::alloc(),
                std::ptr::null_mut(),
                width,
                height,
                8,
                4,
                true,
                false,
                NSCalibratedRGBColorSpace,
                NSBitmapFormat::empty(),
                0,
                32,
            )?;
            let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
            let rect = NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(width as f64, height as f64),
            );
            NSGraphicsContext::saveGraphicsState_class();
            NSGraphicsContext::setCurrentContext(Some(&context));
            image.drawInRect_fromRect_operation_fraction(
                rect,
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)),
                NSCompositingOperation::SourceOver,
                1.0,
            );
            NSColor::colorWithSRGBRed_green_blue_alpha(
                rgba.r as f64,
                rgba.g as f64,
                rgba.b as f64,
                rgba.a as f64,
            )
            .set();
            NSRectFillUsingOperation(rect, NSCompositingOperation::SourceIn);
            NSGraphicsContext::restoreGraphicsState_class();
            let data = rep.bitmapData();
            if data.is_null() {
                return None;
            }
            let stride = rep.bytesPerRow() as usize;
            let (w, h) = (width as usize, height as usize);
            let mut out = Vec::with_capacity(w * h * 4);
            for y in 0..h {
                let row = std::slice::from_raw_parts(data.add(y * stride), w * 4);
                for px in row.chunks_exact(4) {
                    // Premultiplied RGBA → the straight BGRA `RenderImage` reads.
                    out.extend_from_slice(&super::render_pixel([px[0], px[1], px[2], px[3]]));
                }
            }
            out
        };
        let buffer = image::RgbaImage::from_raw(width as u32, height as u32, pixels)?;
        Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
    }
}

#[cfg(test)]
mod tests {
    //! The pixel grid (see the module doc), pinned without a window: a symbol
    //! is configured at a whole point size, occupies a whole-point box, and
    //! rasters to exactly the device pixels that box covers. Break any one of
    //! the three and GPUI resamples the bitmap — the thin, faint, grainy
    //! tiles on the group pane.
    use super::{raster_pixels, render_pixel, snap_box, whole_points, GLYPH_POINT_RATIO};

    /// The backing scales a Mac actually paints at.
    const SCALES: [f32; 3] = [1.0, 2.0, 3.0];

    /// Natural sizes AppKit hands back: the fractional ones are what a
    /// fractional point size produced, and they are the whole point of this.
    const NATURALS: [(f32, f32); 6] = [
        (15.0, 17.0),
        (17.595, 16.42),
        (19.5, 15.0),
        (13.333, 13.333),
        (23.86, 20.14),
        (0.4, 0.6),
    ];

    /// `label_muted` on a dark Glass pane: white at 72 %, which AppKit stores
    /// premultiplied. GPUI wants the white back.
    #[test]
    fn a_raster_pixel_hands_gpui_its_colour_not_its_colour_times_alpha() {
        let alpha = (0.72 * 255.0_f32).round() as u8;
        let covered = [alpha, alpha, alpha, alpha]; // premultiplied white
        assert_eq!(
            render_pixel(covered),
            [255, 255, 255, alpha],
            "a fully covered stroke is white at the tint's alpha"
        );

        // A quarter-covered edge pixel: still white, just thinner.
        let quarter = (0.25 * alpha as f32).round() as u8;
        let [b, g, r, a] = render_pixel([quarter, quarter, quarter, quarter]);
        assert_eq!((b, g, r), (255, 255, 255), "edges keep the colour too");
        assert_eq!(a, quarter);

        // Fully clear stays clear, and does not divide by zero.
        assert_eq!(render_pixel([0, 0, 0, 0]), [0, 0, 0, 0]);
    }

    /// The flat tile the group pane draws these on reads 85; a stroke tinted
    /// `label_muted` should reach 207 over it, and an edge pixel should never
    /// fall *below* 85. Premultiplied data reached 156 and sank to 78 — the
    /// dark rim Scout read as grain.
    #[test]
    fn a_composited_stroke_reaches_its_colour_and_never_darkens_the_tile() {
        const TILE: f32 = 85.0;
        let over = |pixel: [u8; 4]| {
            let [b, _, _, a] = pixel;
            let a = a as f32 / 255.0;
            b as f32 * a + TILE * (1.0 - a)
        };
        let alpha = (0.72 * 255.0_f32).round() as u8;
        let covered = over(render_pixel([alpha, alpha, alpha, alpha]));
        assert!(
            (covered - 207.0).abs() <= 1.0,
            "a covered stroke paints {covered}, not label_muted's 207"
        );
        for step in 0..=20 {
            let c = step as f32 / 20.0;
            let v = (c * alpha as f32).round() as u8;
            let painted = over(render_pixel([v, v, v, v]));
            assert!(
                painted >= TILE - 0.5,
                "coverage {c} painted {painted}, darker than the {TILE} tile"
            );
        }
    }

    #[test]
    fn a_glyph_box_asks_for_a_whole_point_size() {
        // Every box `glyph` is called with across the app; `× 0.85` makes all
        // but a few fractional, which is why the rounding is not optional.
        for box_pt in [
            8.0, 11.0, 11.5, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0, 20.0, 36.0,
        ] {
            let size = whole_points(box_pt * GLYPH_POINT_RATIO);
            assert_eq!(size, size.trunc(), "{box_pt} pt box → {size} pt symbol");
            assert!(size >= 1.0, "{box_pt} pt box → {size} pt symbol");
        }
    }

    #[test]
    fn a_snapped_box_is_whole_points_and_never_vanishes() {
        for natural in NATURALS {
            let (w, h) = snap_box(natural);
            assert_eq!((w.trunc(), h.trunc()), (w, h), "{natural:?} → {w}×{h}");
            assert!(w >= 1.0 && h >= 1.0, "{natural:?} → {w}×{h}");
        }
    }

    /// The invariant that keeps the bitmap crisp: its pixels *are* the box's
    /// device pixels, so GPUI blits 1:1. The old `ceil(natural × scale)` could
    /// not promise this — the raster was a fraction of a pixel wider than the
    /// box it was painted into, and the difference came out as blur.
    #[test]
    fn a_snapped_box_rasters_onto_exactly_its_device_pixels() {
        for natural in NATURALS {
            let (w, h) = snap_box(natural);
            for scale in SCALES {
                let (w_px, h_px) = raster_pixels((w, h), scale);
                assert_eq!(
                    (w_px as f32, h_px as f32),
                    (w * scale, h * scale),
                    "{natural:?} at {scale}×: raster {w_px}×{h_px} vs box {w}×{h} pt"
                );
            }
        }
    }

    /// What the old path did, for the record: a fractional natural size at 2×
    /// is a bitmap wider than its box, and that gap is the resample.
    #[test]
    fn the_old_ceil_did_not_land_on_the_grid() {
        let natural = (17.595_f32, 16.42_f32);
        let scale = 2.0_f32;
        let ceiled = ((natural.0 * scale).ceil(), (natural.1 * scale).ceil());
        assert_ne!(ceiled.0, natural.0 * scale, "the width had slack");
        assert_ne!(ceiled.1, natural.1 * scale, "the height had slack");
    }

    /// A whole-point box covers an even number of device pixels at 2×, which
    /// is the parity it shares with the whole-point tile around it — the
    /// arithmetic behind `centring`'s measurements. A raw natural size does
    /// not, which is why [`snap_box`] exists.
    #[test]
    fn a_whole_point_box_covers_an_even_number_of_device_pixels() {
        for natural in NATURALS {
            let (w, h) = snap_box(natural);
            for side in [w, h] {
                assert_eq!((side * 2.0) as u32 % 2, 0, "box {side} pt is odd at 2×");
            }
        }
        for natural in [17.595_f32, 27.54, 11.73] {
            assert_ne!(
                (natural * 2.0).round() as u32 % 2,
                0,
                "{natural} pt would have shared the tile's parity by luck"
            );
        }
    }
}

/// Where a glyph lands inside its tile, **measured** through a real GPUI
/// window at the 2× a Mac paints at rather than modelled: the window reports
/// the bounds it computed, including its own rounding to whole device pixels.
///
/// The rule these pin is narrower than it looks. What has to be a whole number
/// of points is the glyph's **box**; the slot between the box and the tile does
/// not, because GPUI rounds only a box's final absolute corners, so a half
/// device pixel inside the invisible slot cancels rather than accumulates.
#[cfg(test)]
mod centring {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{
        canvas, div, point, prelude::*, px, size, AvailableSpace, Bounds, Context, Pixels, Render,
        TestAppContext, VisualTestContext, Window,
    };

    use super::{whole_points, GLYPH_POINT_RATIO};

    struct Pane;
    impl Render for Pane {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    /// Every tile the app centres a glyph in, as `(tile, slot)`. The three
    /// fractional slots are deliberate: a measured reproduction of Grok Mac's
    /// search field, and two picture-relative ratios.
    const TILES: [(&str, f32, f32); 6] = [
        ("profile hero", 72.0, 72.0 * 0.45),
        ("kit glyph tile", 36.0, 20.0),
        ("settings tile", 30.0, 15.0),
        ("palette settings tile", 24.0, 12.0),
        ("palette picture tile", 24.0, 24.0 * 0.58),
        ("people search field", 32.0, 11.5),
    ];

    /// The device pixels left and right of a `box_pt` box, centred in `slot`,
    /// centred in `tile` — the nesting every glyph tile draws.
    fn gaps(vcx: &mut VisualTestContext, tile: f32, slot: f32, box_pt: f32) -> (i32, i32) {
        let scale = vcx.update(|window, _| window.scale_factor());
        let device = |p: Pixels| (p.to_f64() as f32 * scale).round() as i32;
        let measure = |vcx: &mut VisualTestContext, inner: bool| -> Bounds<Pixels> {
            let seen = Rc::new(Cell::new(Bounds::default()));
            let out = seen.clone();
            vcx.draw(
                point(px(0.0), px(0.0)),
                size(AvailableSpace::MinContent, AvailableSpace::MinContent),
                move |_, _| {
                    let leaf = canvas(move |b, _, _| out.set(b), |_, _, _, _| {});
                    if inner {
                        div()
                            .size(px(tile))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .size(px(slot))
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(leaf.w(px(box_pt)).h(px(box_pt))),
                            )
                    } else {
                        div().size(px(tile)).child(leaf.w(px(tile)).h(px(tile)))
                    }
                },
            );
            seen.get()
        };
        let outer = measure(vcx, false);
        let inner = measure(vcx, true);
        let left = device(inner.origin.x) - device(outer.origin.x);
        let right = (device(outer.origin.x) + device(outer.size.width))
            - (device(inner.origin.x) + device(inner.size.width));
        (left, right)
    }

    /// Every tile the app draws, with the slots as they stand — fractional
    /// ones included.
    #[gpui::test]
    fn a_whole_point_box_lands_dead_centre_of_its_tile(cx: &mut TestAppContext) {
        let (_, vcx) = cx.add_window_view(|_, _| Pane);
        for (name, tile, slot) in TILES {
            let box_pt = whole_points(slot * GLYPH_POINT_RATIO);
            let (left, right) = gaps(vcx, tile, slot, box_pt);
            assert_eq!(
                left, right,
                "{name}: a {box_pt} pt glyph in a {tile} pt tile sits {left} px from one \
                 edge and {right} px from the other"
            );
        }
    }

    /// The teeth: the raw natural sizes the code handed over before
    /// [`super::snap_box`] are a device pixel out in every tile. Lose the
    /// whole-point box and the glyph goes off centre again — the slot is not
    /// what holds this up.
    #[gpui::test]
    fn a_fractional_box_is_a_device_pixel_out(cx: &mut TestAppContext) {
        let (_, vcx) = cx.add_window_view(|_, _| Pane);
        for (tile, natural) in [(72.0_f32, 27.54_f32), (36.0, 17.595), (24.0, 11.73)] {
            let (left, right) = gaps(vcx, tile, tile, natural);
            assert_eq!(
                right - left,
                1,
                "a {natural} pt box in a {tile} pt tile: {left} px and {right} px"
            );
        }
    }
}
