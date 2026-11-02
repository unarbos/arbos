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
//! `NSBitmapImageRep` of `natural size × the window's backing scale` (2× on
//! every Retina display — crisp, no resampling), then tinted with the text
//! colour through a `sourceIn` fill, and handed to GPUI as a premultiplied
//! BGRA [`RenderImage`]. One raster per (name, size, weight, scale, colour),
//! cached for the app's lifetime — a handful of KB. The element is laid out
//! at the symbol's natural point size inside a fixed square slot, so rows
//! align on the slot while each symbol keeps Apple's proportions.
//!
//! **Fallback.** On a macOS that lacks a name (`NSImage` answers nil), and on
//! every other OS (the Linux dev build and the headless tests), the pack glyph
//! named in [`FALLBACKS`] is drawn through `icons::icon` at the slot size.
//! Nothing else may draw from the pack — `icons::tests` pin it.
//!
//! **Every surface** ([feedback sf-symbols-everywhere-mac-2026-10-28]): [`glyph`]
//! is the drop-in for the old `icons::icon(name, size, color)` — same box, same
//! layout — with the symbol at `size × 0.85` points (the menu precedent: 13 in
//! a 15 box), **medium** weight (the pack's 2 pt stroke on the 24 grid ≈ SF
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
    let frame = div()
        .size(px(slot))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center();
    #[cfg(target_os = "macos")]
    if let Some((w, h)) = mac::natural_size(name, size, weight) {
        return frame.child(mac::element(name, size, weight, color).w(px(w)).h(px(h)));
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

    use gpui::{img, Hsla, ImageSource, Img, RenderImage};
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

    fn sizes() -> &'static Mutex<HashMap<SizeKey, Option<(f32, f32)>>> {
        #[allow(clippy::type_complexity)]
        static SIZES: OnceLock<Mutex<HashMap<SizeKey, Option<(f32, f32)>>>> = OnceLock::new();
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

    /// The image element: the raster is produced at paint time, when the
    /// window's scale factor is known, and cached.
    pub fn element(name: &str, size: f32, weight: Weight, color: Hsla) -> Img {
        // No `Styled` calls here (the trait is not in this module's scope and the
        // frame in `symbol` already fixes the size); the caller sizes the element.
        let name: Arc<str> = name.into();
        img(ImageSource::Custom(Arc::new(move |window, _cx| {
            raster(&name, size, weight, window.scale_factor(), color).map(Ok)
        })))
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
        let width = (natural.width * scale as f64).ceil().max(1.0) as isize;
        let height = (natural.height * scale as f64).ceil().max(1.0) as isize;
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
                    // RGBA premultiplied → BGRA premultiplied.
                    out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
                }
            }
            out
        };
        let buffer = image::RgbaImage::from_raw(width as u32, height as u32, pixels)?;
        Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
    }
}
