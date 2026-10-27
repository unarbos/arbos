//! The picture master a picked file becomes before it goes up: the Mac twin of the relay's
//! `avatar_image::master` (the relay stores what it gets when it is already within size, so
//! the quality decided here is the quality everyone sees).
//!
//! Jacob, 2026-10-26 (v0.6.268): a 4000 px black-and-white mark came out with jagged edges.
//! The upload half of that loss was `resize(512, Triangle)` + `to_rgb8()` + JPEG q85 — soft,
//! ringing strokes, alpha dropped. Now: EXIF upright, longest side ≤ [`MASTER_SIDE`] with
//! Lanczos3 (the `image` crate widens the kernel by the ratio, so one call is a true area
//! filter; ≥ 4× gets an integer box pre-shrink), alpha flattened over white, **PNG** for flat
//! graphics (a mark, a logo — few colours), **JPEG q92** for photographs.

use std::io::Cursor;

use anyhow::{anyhow, Context, Result};
use image::{DynamicImage, GenericImageView, ImageDecoder, ImageFormat, RgbImage};

/// Longest side of the master.
pub const MASTER_SIDE: u32 = 1024;
/// The relay's cap for `PUT /me/avatar`.
pub const MAX_BYTES: usize = 3 * 1024 * 1024;
const JPEG_QUALITY: u8 = 92;
const FLAT_MAX_COLORS: usize = 64;
const FLAT_COVERAGE: f64 = 0.90;

/// Encoded master: the bytes plus the type they are (the relay checks that header and bytes
/// agree, so the two travel together).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Master {
    pub bytes: Vec<u8>,
    pub png: bool,
}

impl Master {
    pub fn mime(&self) -> &'static str {
        if self.png {
            "image/png"
        } else {
            "image/jpeg"
        }
    }

    pub fn ext(&self) -> &'static str {
        if self.png {
            "png"
        } else {
            "jpg"
        }
    }

    /// The same picture as JPEG q92 — for a relay that still takes only JPEG on
    /// `PUT /me/avatar` (415 on a PNG master while the relay half has not deployed yet).
    pub fn to_jpeg(&self) -> Result<Master> {
        if !self.png {
            return Ok(self.clone());
        }
        let img = decode(&self.bytes)?;
        Ok(Master {
            bytes: encode_jpeg(&flatten(&img), JPEG_QUALITY)?,
            png: false,
        })
    }
}

fn decode(bytes: &[u8]) -> Result<DynamicImage> {
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .context("not an image")?;
    let mut decoder = reader.into_decoder().context("not an image")?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).context("not an image")?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// Shrink so the longest side is at most `side`; never enlarge.
pub fn fit(img: &DynamicImage, side: u32) -> DynamicImage {
    let (w, h) = img.dimensions();
    let longest = w.max(h);
    if longest <= side {
        return img.clone();
    }
    let ratio = longest as f32 / side as f32;
    let staged;
    let src = if ratio >= 4.0 {
        let scale = (side * 2) as f32 / longest as f32;
        let (pw, ph) = (
            ((w as f32 * scale).round() as u32).max(1),
            ((h as f32 * scale).round() as u32).max(1),
        );
        staged = img.thumbnail_exact(pw, ph);
        &staged
    } else {
        img
    };
    src.resize(side, side, image::imageops::FilterType::Lanczos3)
}

/// Alpha over white (a transparent logo flattened to black blobs through `to_rgb8` before).
fn flatten(img: &DynamicImage) -> RgbImage {
    if !img.color().has_alpha() {
        return img.to_rgb8();
    }
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut out = RgbImage::new(w, h);
    for (x, y, p) in rgba.enumerate_pixels() {
        let a = p[3] as u32;
        let blend = |c: u8| ((c as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
        out.put_pixel(x, y, image::Rgb([blend(p[0]), blend(p[1]), blend(p[2])]));
    }
    out
}

/// A handful of colours covering almost every pixel (the rest being anti-aliased edges).
pub fn is_flat_graphic(rgb: &RgbImage) -> bool {
    use std::collections::HashMap;
    let (w, h) = rgb.dimensions();
    let stride = ((w.max(h) / 128).max(1)) as usize;
    let mut counts: HashMap<[u8; 3], u32> = HashMap::new();
    let mut total = 0u32;
    for (x, y, p) in rgb.enumerate_pixels() {
        if x as usize % stride != 0 || y as usize % stride != 0 {
            continue;
        }
        total += 1;
        *counts
            .entry([p[0] & 0xF8, p[1] & 0xF8, p[2] & 0xF8])
            .or_default() += 1;
    }
    if total == 0 {
        return false;
    }
    let mut top: Vec<u32> = counts.values().copied().collect();
    top.sort_unstable_by(|a, b| b.cmp(a));
    let covered: u32 = top.iter().take(FLAT_MAX_COLORS).sum();
    covered as f64 / total as f64 >= FLAT_COVERAGE
}

fn encode_png(rgb: &RgbImage) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    DynamicImage::ImageRgb8(rgb.clone())
        .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        .context("encode")?;
    Ok(out)
}

fn encode_jpeg(rgb: &RgbImage, quality: u8) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
    enc.encode_image(rgb).context("encode")?;
    Ok(out)
}

/// Any picture the `image` crate decodes (PNG, JPEG, GIF, WebP) → the master.
pub fn master(bytes: &[u8]) -> Result<Master> {
    let lossless_source = bytes.starts_with(b"\x89PNG") || bytes.starts_with(b"GIF8");
    let img = decode(bytes)?;
    let img = fit(&img, MASTER_SIDE);
    let rgb = flatten(&img);
    let flat = is_flat_graphic(&rgb);
    if flat || lossless_source {
        let png = encode_png(&rgb)?;
        if (flat || png.len() <= MAX_BYTES / 3) && png.len() <= MAX_BYTES {
            return Ok(Master {
                bytes: png,
                png: true,
            });
        }
    }
    let mut jpeg = encode_jpeg(&rgb, JPEG_QUALITY)?;
    if jpeg.len() > MAX_BYTES {
        jpeg = encode_jpeg(&rgb, 85)?;
    }
    if jpeg.len() > MAX_BYTES {
        return Err(anyhow!("the picture is too large even at {MASTER_SIDE} px"));
    }
    Ok(Master {
        bytes: jpeg,
        png: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hard-edged ring + bar, like Jacob's mark: black on white, no anti-aliasing.
    fn mark(side: u32) -> image::RgbaImage {
        let c = side as f32 / 2.0;
        let mut img = image::RgbaImage::from_pixel(side, side, image::Rgba([255, 255, 255, 255]));
        for (x, y, p) in img.enumerate_pixels_mut() {
            let (dx, dy) = (x as f32 + 0.5 - c, y as f32 + 0.5 - c);
            let d = (dx * dx + dy * dy).sqrt();
            let ring = d <= side as f32 * 0.46 && d >= side as f32 * 0.40;
            let bar = (dx - dy).abs() < side as f32 * 0.04 && d < side as f32 * 0.38;
            if ring || bar {
                *p = image::Rgba([0, 0, 0, 255]);
            }
        }
        img
    }

    fn png_bytes(img: &image::RgbaImage) -> Vec<u8> {
        let mut out = Vec::new();
        DynamicImage::ImageRgba8(img.clone())
            .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    /// Share of edge crossings that pass through grey (anti-aliased) rather than jumping
    /// black ↔ white in one step; 0 for point sampling, ≈ 1 for a proper downscale.
    pub(crate) fn edge_softness(img: &DynamicImage) -> f32 {
        let g = img.to_luma8();
        let (w, h) = g.dimensions();
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

    /// Jacob's upload: a 4000 px two-colour PNG → a 1024 px PNG master with shaded edges.
    #[test]
    fn a_large_flat_mark_becomes_a_shaded_png_master() {
        let m = master(&png_bytes(&mark(4000))).unwrap();
        assert!(m.png, "a flat mark stays lossless");
        let img = image::load_from_memory(&m.bytes).unwrap();
        assert_eq!(img.dimensions(), (1024, 1024));
        let soft = edge_softness(&img);
        let nearest = DynamicImage::ImageRgba8(mark(4000)).resize_exact(
            1024,
            1024,
            image::imageops::FilterType::Nearest,
        );
        let n_soft = edge_softness(&nearest);
        assert!(soft > 0.85, "edge softness {soft} (nearest {n_soft})");
        assert!(
            n_soft < 0.2,
            "the metric tells point sampling apart: {n_soft}"
        );
    }

    /// A photograph (gradients + noise, JPEG in) is a JPEG master; nothing is ever enlarged;
    /// transparency lands on white.
    #[test]
    fn photos_are_jpeg_small_pictures_keep_their_size_alpha_lands_on_white() {
        let mut photo = image::RgbaImage::new(1600, 1200);
        let mut seed = 7u32;
        for (x, y, p) in photo.enumerate_pixels_mut() {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let n = (seed >> 24) as u8 / 8;
            *p = image::Rgba([
                (x * 255 / 1600) as u8 ^ n,
                (y * 255 / 1200) as u8,
                ((x + y) % 256) as u8,
                255,
            ]);
        }
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95)
            .encode_image(&DynamicImage::ImageRgba8(photo).to_rgb8())
            .unwrap();
        let m = master(&jpeg).unwrap();
        assert!(!m.png);
        assert_eq!(m.mime(), "image/jpeg");
        assert_eq!(
            image::load_from_memory(&m.bytes).unwrap().dimensions(),
            (1024, 768)
        );

        let m = master(&png_bytes(&mark(300))).unwrap();
        assert_eq!(
            image::load_from_memory(&m.bytes).unwrap().dimensions(),
            (300, 300)
        );

        let mut clear = image::RgbaImage::from_pixel(64, 64, image::Rgba([0, 0, 0, 0]));
        clear.put_pixel(10, 10, image::Rgba([0, 0, 0, 255]));
        let m = master(&png_bytes(&clear)).unwrap();
        let out = image::load_from_memory(&m.bytes).unwrap().to_rgb8();
        assert_eq!(out.get_pixel(0, 0).0, [255, 255, 255]);
        assert_eq!(out.get_pixel(10, 10).0, [0, 0, 0]);

        assert!(master(b"not a picture at all").is_err());

        let png = master(&png_bytes(&mark(300))).unwrap();
        let jpeg = png.to_jpeg().unwrap();
        assert!(!jpeg.png);
        assert_eq!(jpeg.mime(), "image/jpeg");
        assert_eq!(
            image::load_from_memory(&jpeg.bytes).unwrap().dimensions(),
            (300, 300)
        );
    }
}
