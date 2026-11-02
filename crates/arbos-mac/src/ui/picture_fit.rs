//! Pictures drawn at the pixels they are shown at.
//!
//! GPUI samples every image through a `min_filter::linear` sampler with **no mipmaps**: a texel
//! grid much larger than the quad it fills is read 2 × 2 texels per output pixel and the rest is
//! skipped — a 1024 px avatar in a 36 pt disc (72 px on a Retina display) is point-sampled
//! 14 ×, and a 4000 px file in the picker's 114 pt preview 17 × — which is exactly the
//! stair-stepped edge Jacob reported on 2026-10-26 (v0.6.268). [`fit`] hands the sampler a copy
//! already resampled to the display size (Lanczos3 on the CPU, memoised per picture and size), so
//! the GPU only ever draws at ≈ 1:1.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;

use gpui::{ImageId, RenderImage};

thread_local! {
    /// The window's backing scale (`Window::scale_factor`), noted by the shell on every frame so
    /// the element helpers — which have no `Window` — size their pixels right. Retina until told.
    /// Per thread like the memo: it is written and read while rendering, on the UI thread, and a
    /// test thread's note never leaks into another test.
    static SCALE: Cell<f32> = const { Cell::new(2.0) };
}

pub fn note_scale_factor(scale: f32) {
    if scale.is_finite() && scale > 0.0 {
        SCALE.with(|s| s.set(scale));
    }
}

pub fn scale_factor() -> f32 {
    SCALE.with(Cell::get)
}

/// Above this many entries the memo is dropped (a few hundred pictures × sizes; the sized
/// copies are cheap to make again).
const MEMO_CAP: usize = 512;

thread_local! {
    static MEMO: RefCell<HashMap<(ImageId, u32), Arc<RenderImage>>> = RefCell::new(HashMap::new());
}

/// The device pixels a `size` pt square covers on the current display.
pub fn device_px(size: f32) -> u32 {
    ((size * scale_factor()).ceil() as u32).max(1)
}

/// `image` as GPUI should draw it in a `size` pt square with `ObjectFit::Cover`: the picture
/// whose **shorter** side is the square's pixel size (so the cover crop is 1 : 1), made once per
/// (picture, size) on the UI thread and memoised. A picture within 1.5 × of that size — or an
/// animated one — is returned as is; the GPU's linear filter is fine that close and for
/// enlarging.
pub fn fit(image: &Arc<RenderImage>, size: f32) -> Arc<RenderImage> {
    let target = device_px(size);
    let dims = image.size(0);
    let (w, h) = (dims.width.0 as u32, dims.height.0 as u32);
    if image.frame_count() != 1 || w == 0 || h == 0 || w.min(h) * 2 <= target * 3 {
        return image.clone();
    }
    let key = (image.id, target);
    if let Some(hit) = MEMO.with(|m| m.borrow().get(&key).cloned()) {
        return hit;
    }
    let sized = Arc::new(resample(image, target).unwrap_or_else(|| (**image).clone_frames()));
    MEMO.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() >= MEMO_CAP {
            m.clear();
        }
        m.insert(key, sized.clone());
    });
    sized
}

/// Lanczos3 downscale of the frame's BGRA pixels so the shorter side is `target` px. Above 4 ×
/// an integer box pre-shrink keeps the kernel short (the `image` crate scales the Lanczos
/// support with the ratio, so nothing is skipped either way).
fn resample(image: &RenderImage, target: u32) -> Option<RenderImage> {
    let dims = image.size(0);
    let (w, h) = (dims.width.0 as u32, dims.height.0 as u32);
    let raw = image.as_bytes(0)?.to_vec();
    let buffer = image::RgbaImage::from_raw(w, h, raw)?;
    let shorter = w.min(h) as f32;
    let scale = target as f32 / shorter;
    let (nw, nh) = (
        ((w as f32 * scale).round() as u32).max(1),
        ((h as f32 * scale).round() as u32).max(1),
    );
    let src = image::DynamicImage::ImageRgba8(buffer);
    let ratio = shorter / target as f32;
    let staged;
    let from = if ratio >= 4.0 {
        staged = src.thumbnail_exact(nw * 2, nh * 2);
        &staged
    } else {
        &src
    };
    let small = from
        .resize_exact(nw, nh, image::imageops::FilterType::Lanczos3)
        .into_rgba8();
    debug_assert_eq!(small.dimensions(), (nw, nh));
    Some(RenderImage::new(vec![image::Frame::new(small)]))
}

/// A frame-for-frame copy with a new id (the fallback when the pixels cannot be read back).
trait CloneFrames {
    fn clone_frames(&self) -> RenderImage;
}

impl CloneFrames for RenderImage {
    fn clone_frames(&self) -> RenderImage {
        let dims = self.size(0);
        let (w, h) = (dims.width.0 as u32, dims.height.0 as u32);
        let buffer = self
            .as_bytes(0)
            .and_then(|b| image::RgbaImage::from_raw(w, h, b.to_vec()))
            .unwrap_or_else(|| image::RgbaImage::new(1, 1));
        RenderImage::new(vec![image::Frame::new(buffer)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(side: u32) -> Arc<RenderImage> {
        // A hard-edged black disc on white (BGRA, like the avatar decoder hands GPUI).
        let c = side as f32 / 2.0;
        let mut img = image::RgbaImage::from_pixel(side, side, image::Rgba([255, 255, 255, 255]));
        for (x, y, p) in img.enumerate_pixels_mut() {
            let (dx, dy) = (x as f32 + 0.5 - c, y as f32 + 0.5 - c);
            if (dx * dx + dy * dy).sqrt() <= side as f32 * 0.4 {
                *p = image::Rgba([0, 0, 0, 255]);
            }
        }
        Arc::new(RenderImage::new(vec![image::Frame::new(img)]))
    }

    fn edge_softness(image: &RenderImage) -> f32 {
        let dims = image.size(0);
        let (w, h) = (dims.width.0 as u32, dims.height.0 as u32);
        let g = image::RgbaImage::from_raw(w, h, image.as_bytes(0).unwrap().to_vec()).unwrap();
        let (mut crossings, mut shaded) = (0u32, 0u32);
        let mut judge = |a: u8, b: u8| {
            let d = (a as i32 - b as i32).unsigned_abs();
            if d > 48 {
                crossings += 1;
                if d < 224 {
                    shaded += 1;
                }
            }
        };
        for y in 0..h {
            for x in 0..w {
                if x + 1 < w {
                    judge(g.get_pixel(x, y)[0], g.get_pixel(x + 1, y)[0]);
                }
                if y + 1 < h {
                    judge(g.get_pixel(x, y)[0], g.get_pixel(x, y + 1)[0]);
                }
            }
        }
        if crossings == 0 {
            0.0
        } else {
            shaded as f32 / crossings as f32
        }
    }

    /// A 1024 px master in a 36 pt disc on a 2 × display → a 72 px copy with shaded edges, the
    /// same `Arc` on the next call (memoised), untouched when it is already small enough.
    #[test]
    fn fits_to_the_display_pixels_and_memoises() {
        note_scale_factor(2.0);
        let master = picture(1024);
        let small = fit(&master, 36.0);
        let dims = small.size(0);
        assert_eq!((dims.width.0, dims.height.0), (72, 72));
        let soft = edge_softness(&small);
        assert!(soft > 0.8, "edge softness {soft}");
        assert!(
            Arc::ptr_eq(&fit(&master, 36.0), &small),
            "memoised per size"
        );
        assert!(
            !Arc::ptr_eq(&fit(&master, 114.0), &small),
            "another size, another copy"
        );

        let hero = fit(&master, 114.0);
        assert_eq!(hero.size(0).width.0, 228);

        let tiny = picture(96);
        assert!(
            Arc::ptr_eq(&fit(&tiny, 36.0), &tiny),
            "within 1.5 × stays with the GPU"
        );

        note_scale_factor(1.0);
        assert_eq!(fit(&master, 36.0).size(0).width.0, 36);
        note_scale_factor(2.0);
    }

    /// A non-square picture is sized on its shorter side so the cover crop stays 1 : 1.
    #[test]
    fn non_square_pictures_fit_their_shorter_side() {
        note_scale_factor(2.0);
        let wide = image::RgbaImage::from_pixel(1600, 800, image::Rgba([10, 20, 30, 255]));
        let wide = Arc::new(RenderImage::new(vec![image::Frame::new(wide)]));
        let s = fit(&wide, 40.0);
        assert_eq!((s.size(0).width.0, s.size(0).height.0), (160, 80));
    }
}
