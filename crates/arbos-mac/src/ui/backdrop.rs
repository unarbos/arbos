//! The desktop behind the window, measured ([feedback mac-glass-adaptive-2026-10-26]).
//!
//! The Glass washes (`Theme::adaptive_wash`) are only as thick as the text on
//! them needs over *this* desktop. To know the desktop, the app images the
//! screen region under its own window (`CGWindowListCreateImage`, "on screen
//! below window", nominal resolution) and reduces it to three mean
//! luminances — under the sidebar, the chat column and the right pane — in
//! `Theme::backdrop`. Throttled: once every [`INTERVAL`] while the window is
//! on Glass, and at once when the window moved or resized; a change under
//! [`MIN_CHANGE`] is ignored so the washes do not breathe. A capture that
//! shows nothing (no Screen Recording grant on some systems leaves other
//! apps' windows blank) is not a sample: the washes then keep assuming the
//! worst desktop, which is what they did before this item.

use std::time::{Duration, Instant};

use gpui::{AnyWindowHandle, App, Bounds, Pixels};

use crate::ui::theme::{Backdrop, Theme};

/// How often the desktop is re-read while nothing moved.
pub const INTERVAL: Duration = Duration::from_millis(1500);
/// A luminance change (0–1) below this is not worth a repaint.
pub const MIN_CHANGE: f32 = 0.02;
/// A capture must carry at least this share of opaque pixels to count.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const MIN_COVERAGE: f32 = 0.5;

/// Pure: is it time to sample again?
pub fn due(last: Option<Instant>, now: Instant, moved: bool) -> bool {
    moved || last.is_none_or(|t| now.duration_since(t) >= INTERVAL)
}

/// Pure: does the new sample differ enough from the old one to repaint?
pub fn changed(old: Option<Backdrop>, new: Backdrop) -> bool {
    match old {
        None => true,
        Some(o) => {
            (o.left - new.left).abs() >= MIN_CHANGE
                || (o.center - new.center).abs() >= MIN_CHANGE
                || (o.right - new.right).abs() >= MIN_CHANGE
        }
    }
}

/// The three bands of a window of `width` columns: the left quarter (the
/// sidebar), the middle half (the chat column), the right quarter (the pane).
/// Returned as `[start, end)` column ranges.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn bands(width: usize) -> [(usize, usize); 3] {
    let q = width / 4;
    [
        (0, q.max(1)),
        (q, (width - q).max(q + 1)),
        (width - q, width),
    ]
}

/// Pure: reduce a row-major 8-bit RGBA(-ish) buffer to the three band
/// luminances; `pixel(x, y) -> Option<(r, g, b)>` (None = transparent).
/// Returns `None` when fewer than [`MIN_COVERAGE`] of the sampled pixels
/// were opaque.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn reduce(
    width: usize,
    height: usize,
    stride: usize,
    pixel: impl Fn(usize, usize) -> Option<(u8, u8, u8)>,
) -> Option<Backdrop> {
    if width == 0 || height == 0 {
        return None;
    }
    let step = stride.max(1);
    let mut sums = [0.0f64; 3];
    let mut counts = [0usize; 3];
    let mut seen = 0usize;
    let mut opaque = 0usize;
    let bands = bands(width);
    let mut y = 0;
    while y < height {
        let mut x = 0;
        while x < width {
            seen += 1;
            if let Some((r, g, b)) = pixel(x, y) {
                opaque += 1;
                let l = 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
                for (i, (s, e)) in bands.iter().enumerate() {
                    if x >= *s && x < *e {
                        sums[i] += l;
                        counts[i] += 1;
                    }
                }
            }
            x += step;
        }
        y += step;
    }
    if seen == 0 || (opaque as f32) / (seen as f32) < MIN_COVERAGE {
        return None;
    }
    let mean = |i: usize| -> f32 {
        if counts[i] == 0 {
            0.5
        } else {
            (sums[i] / counts[i] as f64) as f32
        }
    };
    Some(Backdrop {
        left: mean(0),
        center: mean(1),
        right: mean(2),
    })
}

/// sRGB byte → linear light.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn lin(v: u8) -> f64 {
    let c = v as f64 / 255.0;
    if c <= 0.039_28 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Start the sampler for a window: a loop on the foreground executor that
/// wakes every [`INTERVAL`], samples when due, and updates the theme (and
/// repaints) when the desktop changed. Ends when the window is gone.
pub fn start(window: AnyWindowHandle, cx: &mut App) {
    cx.spawn(async move |cx| {
        let mut last: Option<Instant> = None;
        let mut last_bounds: Option<Bounds<Pixels>> = None;
        loop {
            cx.background_executor().timer(INTERVAL / 3).await;
            let alive = cx.update(|cx| {
                window
                    .update(cx, |_, window, cx| {
                        let theme = Theme::of(cx);
                        if !theme.is_glass() {
                            return;
                        }
                        let bounds = window.bounds();
                        let moved = last_bounds != Some(bounds);
                        let now = Instant::now();
                        if !due(last, now, moved) {
                            return;
                        }
                        last = Some(now);
                        last_bounds = Some(bounds);
                        let Some(sample) = platform::sample(window) else {
                            return;
                        };
                        if changed(theme.backdrop, sample) {
                            let mut theme = theme.clone();
                            theme.backdrop = Some(sample);
                            cx.set_global(theme);
                            window.refresh();
                        }
                    })
                    .is_ok()
            });
            if !alive {
                break;
            }
        }
    })
    .detach();
}

#[cfg(target_os = "macos")]
mod platform {
    use core_graphics::display::CGDisplay;
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    use core_graphics::window::{
        create_image, kCGWindowImageNominalResolution, kCGWindowListOptionOnScreenBelowWindow,
    };
    use foreign_types::ForeignType as _;
    use objc2_app_kit::NSView;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    use super::reduce;
    use crate::ui::theme::Backdrop;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGImageGetBitmapInfo(image: *const std::ffi::c_void) -> u32;
    }

    /// The desktop (and whatever else lies below our window) under the
    /// window's frame, reduced to the three band luminances.
    pub(super) fn sample(window: &gpui::Window) -> Option<Backdrop> {
        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
            return None;
        };
        // SAFETY: GPUI's raw window handle is the live window's NSView*.
        let view = unsafe { appkit.ns_view.cast::<NSView>().as_ref() };
        let ns_window = view.window()?;
        let number = ns_window.windowNumber();
        if number <= 0 {
            return None;
        }
        // AppKit's frame is bottom-left based in screen points; CoreGraphics
        // wants top-left of the main display.
        let frame = ns_window.frame();
        let main_h = CGDisplay::main().bounds().size.height;
        let rect = CGRect::new(
            &CGPoint::new(
                frame.origin.x,
                main_h - (frame.origin.y + frame.size.height),
            ),
            &CGSize::new(frame.size.width, frame.size.height),
        );
        let img = create_image(
            rect,
            kCGWindowListOptionOnScreenBelowWindow,
            number as u32,
            kCGWindowImageNominalResolution,
        )?;
        let (w, h) = (img.width(), img.height());
        if w == 0 || h == 0 || img.bits_per_pixel() != 32 {
            return None;
        }
        let bpr = img.bytes_per_row();
        let data = img.data();
        let bytes = data.bytes();
        // SAFETY: `img` is a live CGImageRef owned by this scope.
        let info = unsafe { CGImageGetBitmapInfo(img.as_ptr() as *const std::ffi::c_void) };
        let little = info & 0x7000 == 0x2000;
        let alpha_first = matches!(info & 0x1F, 2 | 4 | 6);
        let no_alpha = matches!(info & 0x1F, 0 | 5 | 6);
        // ~64 columns × ~40 rows of samples whatever the window size.
        let stride = (w / 64).max(1);
        reduce(w, h, stride, |x, y| {
            let i = y * bpr + x * 4;
            let px = bytes.get(i..i + 4)?;
            let (r, g, b, a) = match (little, alpha_first) {
                (true, true) => (px[2], px[1], px[0], px[3]),
                (true, false) => (px[3], px[2], px[1], px[0]),
                (false, true) => (px[1], px[2], px[3], px[0]),
                (false, false) => (px[0], px[1], px[2], px[3]),
            };
            (no_alpha || a >= 128).then_some((r, g, b))
        })
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use crate::ui::theme::Backdrop;

    pub(super) fn sample(_window: &gpui::Window) -> Option<Backdrop> {
        None
    }
}

/// [feedback mac-glass-adaptive-2026-10-26]: the sampler's pure parts — the
/// throttle, the change gate, the band split, and the reduction with its
/// coverage rule (a blank capture is not a sample).
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_throttled_but_a_move_samples_at_once() {
        let t0 = Instant::now();
        assert!(due(None, t0, false), "first sample at once");
        assert!(!due(Some(t0), t0 + Duration::from_millis(200), false));
        assert!(
            due(Some(t0), t0 + Duration::from_millis(200), true),
            "moved"
        );
        assert!(due(Some(t0), t0 + INTERVAL, false));
    }

    #[test]
    fn small_changes_do_not_repaint() {
        let a = Backdrop::uniform(0.30);
        assert!(changed(None, a));
        assert!(!changed(Some(a), Backdrop::uniform(0.31)));
        assert!(changed(Some(a), Backdrop::uniform(0.35)));
        assert!(changed(
            Some(a),
            Backdrop {
                left: 0.30,
                center: 0.30,
                right: 0.60
            }
        ));
    }

    #[test]
    fn the_bands_split_the_window_into_quarter_half_quarter() {
        assert_eq!(bands(1200), [(0, 300), (300, 900), (900, 1200)]);
        assert_eq!(bands(4), [(0, 1), (1, 3), (3, 4)]);
    }

    #[test]
    fn a_half_white_half_black_desktop_reads_per_band() {
        // Left half white, right half black: the sidebar band is white, the
        // pane band black, the chat band half and half.
        let b = reduce(400, 100, 1, |x, _| {
            Some(if x < 200 { (255, 255, 255) } else { (0, 0, 0) })
        })
        .expect("opaque capture");
        assert!((b.left - 1.0).abs() < 0.01);
        assert!((b.right - 0.0).abs() < 0.01);
        assert!((b.center - 0.5).abs() < 0.02);
    }

    #[test]
    fn a_blank_capture_is_not_a_sample() {
        assert_eq!(reduce(100, 100, 2, |_, _| None), None);
        // Mostly transparent (other apps' windows without the grant): rejected.
        assert_eq!(
            reduce(100, 100, 1, |x, _| (x < 30).then_some((128, 128, 128))),
            None
        );
        // Mostly opaque: accepted.
        assert!(reduce(100, 100, 1, |x, _| (x < 70).then_some((128, 128, 128))).is_some());
        assert_eq!(reduce(0, 0, 1, |_, _| Some((0, 0, 0))), None);
    }
}
