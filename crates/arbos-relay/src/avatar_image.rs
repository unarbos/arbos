//! Avatar pixels: the master a picture is stored as, and the sizes it is served at.
//!
//! Jacob, 2026-10-26 (Mac v0.6.268): a 4000 px black-and-white mark came out with jagged edges.
//! Two losses along the way — the apps shrank every picture to a 512 px JPEG q85 (soft edges,
//! ringing round the black strokes) and then drew that texture, or the raw 4000 px file, through
//! a GPU sampler with no mipmaps (stair-steps). This module fixes the stored half for every
//! client, old or new:
//!
//! - **Master**: the longest side fits [`MASTER_SIDE`] (1024 px; never enlarged), resampled with
//!   Lanczos3 — the `image` crate widens the kernel by the shrink ratio, so one call is a true
//!   area filter — alpha flattened over white, EXIF orientation applied. Flat graphics (a logo, a
//!   mark, a few colours) stay **PNG**; photographs become **JPEG q92**.
//! - **Sizes**: `?w=<px>` on the picture routes serves a Lanczos3 downscale of the master at one of
//!   [`BUCKETS`] (rounded up), cached on disk by the master's content hash, in the master's format.
//!   A client asks for the pixels it draws (points × scale) instead of shrinking on the GPU.

use std::io::Cursor;
use std::path::PathBuf;

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use image::{DynamicImage, GenericImageView, ImageDecoder, ImageFormat, RgbImage};
use sha2::Digest;

use crate::image_sniff::{sniff, ImageKind};
use crate::state::AppState;

/// Longest side of a stored master.
pub const MASTER_SIDE: u32 = 1024;
/// Largest master the relay keeps (a 1024 px PNG photo would be bigger — it becomes JPEG first).
pub const MAX_MASTER_BYTES: usize = 3 * 1024 * 1024;
/// JPEG quality for photographic masters.
pub const JPEG_QUALITY: u8 = 92;
/// JPEG quality for served sizes (smaller pictures, same visual quality).
pub const SIZED_JPEG_QUALITY: u8 = 90;
/// The widths `?w=` is rounded up to (fewer files on disk, one URL per surface size).
pub const BUCKETS: &[u32] = &[32, 48, 64, 96, 128, 192, 256, 384, 512, 768, 1024];
/// How many distinct colours a flat graphic may have before it is treated as a photograph.
const FLAT_MAX_COLORS: usize = 64;
/// Share of the pixels the most common colours must cover for a flat graphic (anti-aliased
/// edges are the rest).
const FLAT_COVERAGE: f64 = 0.90;

/// A stored picture: its bytes and the type they are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Master {
    pub kind: ImageKind,
    pub bytes: Vec<u8>,
}

impl Master {
    pub fn mime(&self) -> &'static str {
        mime_of(self.kind)
    }

    /// The file extension a blob key for this picture carries.
    pub fn ext(&self) -> &'static str {
        match self.kind {
            ImageKind::Png => "png",
            ImageKind::Jpeg => "jpg",
            ImageKind::Gif => "gif",
            ImageKind::Webp => "webp",
            ImageKind::Heic => "heic",
        }
    }
}

pub fn mime_of(kind: ImageKind) -> &'static str {
    match kind {
        ImageKind::Png => "image/png",
        ImageKind::Jpeg => "image/jpeg",
        ImageKind::Gif => "image/gif",
        ImageKind::Webp => "image/webp",
        ImageKind::Heic => "image/heic",
    }
}

/// Decode with the EXIF orientation applied (phone photos are stored sideways with a flag).
fn decode(bytes: &[u8]) -> Result<DynamicImage, String> {
    let mut decoder = crate::image_sniff::decoder(bytes)?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).map_err(|e| format!("decode: {e}"))?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// Shrink so the longest side is at most `side`; never enlarge. Lanczos3 whose support the
/// `image` crate scales with the ratio, so a 4000 → 1024 step is one clean area resample; above
/// 4× an integer box pre-shrink keeps the kernel small and the time short.
pub fn fit(img: &DynamicImage, side: u32) -> DynamicImage {
    let (w, h) = img.dimensions();
    let longest = w.max(h);
    if longest <= side {
        return img.clone();
    }
    let ratio = longest as f32 / side as f32;
    let staged;
    let src = if ratio >= 4.0 {
        // `thumbnail` is the crate's integer area average: every source pixel lands in exactly
        // one target pixel, so nothing is skipped; Lanczos3 then does the last ≤ 2× step.
        let pre = side * 2;
        let scale = pre as f32 / longest as f32;
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

/// Alpha over white: the picture routes serve JPEG (no alpha) and every surface draws the disc
/// on its own background, so one flattening rule keeps the two formats looking the same.
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

/// A flat graphic — a logo, a mark, a diagram — has a handful of colours that cover almost
/// every pixel; the rest are its anti-aliased edges. Sampled on a stride, so a 1024 px picture
/// costs a few thousand pixels.
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
        // Quantise lightly so JPEG noise around one colour still counts as that colour.
        let q = [p[0] & 0xF8, p[1] & 0xF8, p[2] & 0xF8];
        *counts.entry(q).or_default() += 1;
    }
    if total == 0 {
        return false;
    }
    let mut top: Vec<u32> = counts.values().copied().collect();
    top.sort_unstable_by(|a, b| b.cmp(a));
    let covered: u32 = top.iter().take(FLAT_MAX_COLORS).sum();
    covered as f64 / total as f64 >= FLAT_COVERAGE
}

fn encode_png(rgb: &RgbImage) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    DynamicImage::ImageRgb8(rgb.clone())
        .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        .map_err(|e| format!("encode png: {e}"))?;
    Ok(out)
}

fn encode_jpeg(rgb: &RgbImage, quality: u8) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
    enc.encode_image(rgb)
        .map_err(|e| format!("encode jpeg: {e}"))?;
    Ok(out)
}

/// Any picture the `image` crate decodes → the master the relay stores: ≤ `MASTER_SIDE`,
/// upright, opaque; PNG when it is a flat graphic (or the source was PNG/GIF and stays small),
/// else JPEG q92. `source_kind` is what the upload said it was — a PNG source leans PNG.
pub fn master(bytes: &[u8]) -> Result<Master, String> {
    let source_kind = sniff(bytes);
    let img = decode(bytes)?;
    let img = fit(&img, MASTER_SIDE);
    let rgb = flatten(&img);
    let flat = is_flat_graphic(&rgb);
    let lossless_source = matches!(source_kind, Some(ImageKind::Png | ImageKind::Gif));
    if flat || lossless_source {
        let png = encode_png(&rgb)?;
        // A PNG photograph (a screenshot, a PNG export of a photo) is large; JPEG wins unless
        // the picture is genuinely flat.
        if (flat || png.len() <= MAX_MASTER_BYTES / 3) && png.len() <= MAX_MASTER_BYTES {
            return Ok(Master {
                kind: ImageKind::Png,
                bytes: png,
            });
        }
    }
    let mut jpeg = encode_jpeg(&rgb, JPEG_QUALITY)?;
    if jpeg.len() > MAX_MASTER_BYTES {
        jpeg = encode_jpeg(&rgb, 85)?;
    }
    Ok(Master {
        kind: ImageKind::Jpeg,
        bytes: jpeg,
    })
}

/// What an upload becomes before it is stored: anything larger than the master size is brought
/// down to it (an older or foreign client's 1536 px JPEG, a 4000 px PNG sent raw); a picture
/// already within the master size is kept byte for byte (no second lossy generation).
/// Bytes that do not decode are returned unchanged with their sniffed kind — the caller has
/// already run the type gate.
///
/// `Err` only for a picture too large to decode ([`crate::image_sniff::MAX_DECODE_SIDE`]).
/// Those are refused rather than kept: storing one would hand every viewer's client the same
/// picture the relay would not open itself.
pub fn normalize_upload(kind: ImageKind, bytes: &[u8]) -> Result<Master, String> {
    match decode(bytes) {
        Ok(img) => {
            let (w, h) = img.dimensions();
            if w.max(h) > MASTER_SIDE {
                if let Ok(m) = master(bytes) {
                    return Ok(m);
                }
            }
        }
        Err(why) if why == crate::image_sniff::TOO_LARGE => return Err(why),
        Err(_) => {}
    }
    Ok(Master {
        kind,
        bytes: bytes.to_vec(),
    })
}

/// `?w=` → the bucket served (rounded up; above the largest bucket the master itself).
pub fn bucket(w: u32) -> Option<u32> {
    BUCKETS.iter().copied().find(|b| *b >= w)
}

/// The master at `w` px on its longest side, in the master's own format (PNG stays PNG; JPEG at
/// q90). `None` when the master is already that small or does not decode — serve it as is.
pub fn sized(master: &Master, w: u32) -> Option<Master> {
    let img = decode(&master.bytes).ok()?;
    let (iw, ih) = img.dimensions();
    if iw.max(ih) <= w {
        return None;
    }
    let small = fit(&img, w);
    let rgb = flatten(&small);
    let bytes = match master.kind {
        ImageKind::Png | ImageKind::Gif | ImageKind::Webp | ImageKind::Heic => {
            encode_png(&rgb).ok()?
        }
        ImageKind::Jpeg => encode_jpeg(&rgb, SIZED_JPEG_QUALITY).ok()?,
    };
    Some(Master {
        kind: if master.kind == ImageKind::Jpeg {
            ImageKind::Jpeg
        } else {
            ImageKind::Png
        },
        bytes,
    })
}

/// The query every picture route takes: `?w=<px>` (plus whatever else the client appends, e.g.
/// `v=` for cache busting, which is ignored here).
#[derive(Debug, Default, serde::Deserialize)]
pub struct SizeQuery {
    pub w: Option<u32>,
}

fn cache_path(state: &AppState, master: &[u8], w: u32, ext: &str) -> PathBuf {
    let digest = sha2::Sha256::digest(master);
    let tag = hex::encode(&digest[..12]);
    state
        .cfg
        .data_dir
        .join("avatar-cache")
        .join(format!("{tag}-{w}.{ext}"))
}

/// The picture at the requested size (or the master when no usable `w` was given), with the
/// cache header for the public routes. Sized variants are produced off the async threads and
/// kept on disk under the master's content hash, so a changed picture never serves stale pixels.
pub async fn respond(
    state: &AppState,
    master: Master,
    w: Option<u32>,
    cache_control: Option<&'static str>,
) -> Response {
    let served = match w.and_then(bucket) {
        Some(bucket) => sized_cached(state, &master, bucket).await,
        None => master,
    };
    let mime = served.mime();
    let mut response = (StatusCode::OK, served.bytes).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    if let Some(cc) = cache_control {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(cc));
    }
    response
}

async fn sized_cached(state: &AppState, master: &Master, w: u32) -> Master {
    let ext = if master.kind == ImageKind::Jpeg {
        "jpg"
    } else {
        "png"
    };
    let path = cache_path(state, &master.bytes, w, ext);
    if let Ok(bytes) = tokio::fs::read(&path).await {
        return Master {
            kind: if ext == "jpg" {
                ImageKind::Jpeg
            } else {
                ImageKind::Png
            },
            bytes,
        };
    }
    let work = master.clone();
    let produced = tokio::task::spawn_blocking(move || sized(&work, w))
        .await
        .ok()
        .flatten();
    match produced {
        Some(small) => {
            if let Some(parent) = path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let _ = tokio::fs::write(&path, &small.bytes).await;
            small
        }
        None => master.clone(),
    }
}

#[cfg(test)]
pub(crate) mod synth {
    //! A synthetic black-and-white mark like Jacob's: a thick ring, a triangle and a disc, drawn
    //! at any size with hard edges (what a vector export at 4000 px looks like), plus the
    //! edge metrics the quality tests judge a downscale by.

    use image::{Rgba, RgbaImage};

    /// Hard-edged mark: black strokes on white, `side` px square, no anti-aliasing at all.
    pub fn mark(side: u32) -> RgbaImage {
        let c = side as f32 / 2.0;
        let r_outer = side as f32 * 0.46;
        let r_inner = side as f32 * 0.40;
        let r_disc = side as f32 * 0.10;
        let mut img = RgbaImage::from_pixel(side, side, Rgba([255, 255, 255, 255]));
        for (x, y, p) in img.enumerate_pixels_mut() {
            let (dx, dy) = (x as f32 + 0.5 - c, y as f32 + 0.5 - c);
            let d = (dx * dx + dy * dy).sqrt();
            let ring = d <= r_outer && d >= r_inner;
            // Triangle outline: three half-planes, stroke = the band between two offsets.
            let tri = |off: f32| -> bool {
                let top = c - side as f32 * 0.30 - off;
                let base = c + side as f32 * 0.22 + off;
                let half = (side as f32 * 0.30 + off) * (y as f32 + 0.5 - top) / (base - top);
                (y as f32 + 0.5) >= top && (y as f32 + 0.5) <= base && dx.abs() <= half
            };
            let tri_stroke = tri(side as f32 * 0.03) && !tri(-(side as f32) * 0.03);
            let disc = d <= r_disc;
            if ring || tri_stroke || disc {
                *p = Rgba([0, 0, 0, 255]);
            }
        }
        img
    }

    /// Reference downscale by exact area averaging (integer factor): what a perfect filter
    /// gives. `from.side` must be a multiple of `side`.
    pub fn area_reference(from: &RgbaImage, side: u32) -> Vec<f32> {
        let (w, _) = from.dimensions();
        let f = w / side;
        let mut out = vec![0f32; (side * side) as usize];
        for y in 0..side {
            for x in 0..side {
                let mut sum = 0u64;
                for yy in 0..f {
                    for xx in 0..f {
                        sum += from.get_pixel(x * f + xx, y * f + yy)[0] as u64;
                    }
                }
                out[(y * side + x) as usize] = sum as f32 / (f * f) as f32;
            }
        }
        out
    }

    /// Mean absolute error of the grey channel against the area reference (0 = perfect).
    pub fn mae(gray: &[f32], reference: &[f32]) -> f32 {
        gray.iter()
            .zip(reference)
            .map(|(a, b)| (a - b).abs())
            .sum::<f32>()
            / gray.len() as f32
    }

    /// Edge-contrast metric: over every pair of neighbouring pixels that crosses an edge (they
    /// differ by more than 48 grey levels), the share of crossings that are *shaded* — the two
    /// values do not jump straight from black to white (difference below 224). A proper
    /// downscale shades almost every crossing (an anti-aliased edge passes through grey); a
    /// point-sampled or stair-stepped one jumps 0 ↔ 255 everywhere (→ 0). Axis-aligned edges
    /// that land exactly on a pixel boundary are legitimately hard, so even a perfect filter
    /// does not reach 1.0 on a synthetic mark.
    pub fn edge_softness(gray: &[f32], side: u32) -> f32 {
        let s = side as usize;
        let at = |x: usize, y: usize| gray[y * s + x];
        let mut crossings = 0u32;
        let mut shaded = 0u32;
        let mut judge = |a: f32, b: f32| {
            let d = (a - b).abs();
            if d > 48.0 {
                crossings += 1;
                if d < 224.0 {
                    shaded += 1;
                }
            }
        };
        for y in 0..s {
            for x in 0..s {
                if x + 1 < s {
                    judge(at(x, y), at(x + 1, y));
                }
                if y + 1 < s {
                    judge(at(x, y), at(x, y + 1));
                }
            }
        }
        if crossings == 0 {
            0.0
        } else {
            shaded as f32 / crossings as f32
        }
    }

    pub fn gray_of(img: &image::DynamicImage) -> Vec<f32> {
        img.to_luma8().as_raw().iter().map(|v| *v as f32).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::synth::*;
    use super::*;

    fn png_bytes(img: &image::RgbaImage) -> Vec<u8> {
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(img.clone())
            .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    /// Jacob's case: a 4000 px hard-edged mark → a 1024 px master that stays PNG, keeps its
    /// edges shaded (no stair-steps) and sits close to the perfect area average.
    #[test]
    fn a_large_flat_mark_becomes_a_shaded_png_master() {
        let src = mark(4096);
        let m = master(&png_bytes(&src)).unwrap();
        assert_eq!(m.kind, ImageKind::Png, "a two-colour mark stays lossless");
        let img = image::load_from_memory(&m.bytes).unwrap();
        assert_eq!(img.dimensions(), (1024, 1024));
        let gray = gray_of(&img);
        let reference = area_reference(&src, 1024);
        let err = mae(&gray, &reference);
        let soft = edge_softness(&gray, 1024);
        // Point sampling (what the GPU did) for comparison: hard edges, large error.
        let nearest = image::DynamicImage::ImageRgba8(src.clone()).resize_exact(
            1024,
            1024,
            image::imageops::FilterType::Nearest,
        );
        let n_gray = gray_of(&nearest);
        let n_soft = edge_softness(&n_gray, 1024);
        let n_err = mae(&n_gray, &reference);
        eprintln!(
            "master 4096→1024: edge softness {soft:.3} (nearest {n_soft:.3}), mae {err:.2} (nearest {n_err:.2}), {} bytes {:?}",
            m.bytes.len(),
            m.kind
        );
        assert!(soft > 0.85, "edge softness {soft} (nearest {n_soft})");
        assert!(
            n_soft < 0.2,
            "the metric tells point sampling apart: {n_soft}"
        );
        assert!(err < 6.0, "mae {err} vs nearest {n_err}");
        assert!(
            n_err > err * 2.0,
            "nearest {n_err} should be far worse than {err}"
        );
    }

    /// A photograph-like picture (smooth gradients + noise, JPEG source) becomes JPEG q92, not
    /// a multi-megabyte PNG; a smaller picture is never enlarged.
    #[test]
    fn a_photo_becomes_a_jpeg_and_nothing_is_enlarged() {
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
            .encode_image(&image::DynamicImage::ImageRgba8(photo).to_rgb8())
            .unwrap();
        let m = master(&jpeg).unwrap();
        assert_eq!(m.kind, ImageKind::Jpeg);
        let img = image::load_from_memory(&m.bytes).unwrap();
        assert_eq!(img.dimensions(), (1024, 768));
        assert!(m.bytes.len() < MAX_MASTER_BYTES);

        let small = mark(200);
        let m = master(&png_bytes(&small)).unwrap();
        let img = image::load_from_memory(&m.bytes).unwrap();
        assert_eq!(
            img.dimensions(),
            (200, 200),
            "a small picture is kept at its size"
        );
    }

    /// Transparent pixels land on white, not on black (a transparent PNG logo on a dark theme
    /// used to flatten to black blobs through `to_rgb8`).
    #[test]
    fn transparency_is_flattened_over_white() {
        let mut img = image::RgbaImage::from_pixel(64, 64, image::Rgba([0, 0, 0, 0]));
        img.put_pixel(32, 32, image::Rgba([0, 0, 0, 255]));
        let m = master(&png_bytes(&img)).unwrap();
        let out = image::load_from_memory(&m.bytes).unwrap().to_rgb8();
        assert_eq!(out.get_pixel(0, 0).0, [255, 255, 255]);
        assert_eq!(out.get_pixel(32, 32).0, [0, 0, 0]);
    }

    /// `sized` shrinks in the master's format and rounds `?w=` up to a bucket; the master is
    /// served as is above its own size.
    #[test]
    fn sizes_follow_the_buckets_and_the_masters_format() {
        let m = master(&png_bytes(&mark(2048))).unwrap();
        assert_eq!(bucket(70), Some(96));
        assert_eq!(bucket(96), Some(96));
        assert_eq!(bucket(1025), None, "above the largest bucket: the master");
        let s = sized(&m, 96).unwrap();
        assert_eq!(s.kind, ImageKind::Png);
        let img = image::load_from_memory(&s.bytes).unwrap();
        assert_eq!(img.dimensions(), (96, 96));
        assert!(
            edge_softness(&gray_of(&img), 96) > 0.8,
            "the served size is shaded, not stair-stepped"
        );
        assert!(sized(&m, 1024).is_none(), "already that small");

        let j = Master {
            kind: ImageKind::Jpeg,
            bytes: encode_jpeg(&flatten(&image::DynamicImage::ImageRgba8(mark(512))), 92).unwrap(),
        };
        let s = sized(&j, 64).unwrap();
        assert_eq!(s.kind, ImageKind::Jpeg);
        assert_eq!(sniff(&s.bytes), Some(ImageKind::Jpeg));
    }

    /// The blob route: `?w=` on an image blob (a group / server picture, a generated preview)
    /// serves the bucketed size in the blob's format; without `w`, or on a blob that is not a
    /// picture, the stored bytes stream as before.
    #[tokio::test]
    async fn blob_route_serves_image_blobs_by_size() {
        use crate::people::tests_support::{person, project_of};
        use crate::state::test_support::test_state;
        use axum::extract::{Path, Query, State};
        use axum::http::{header, HeaderMap, HeaderValue, StatusCode};

        let state = test_state(crate::state::AuthMode::Github);
        let (alice, alice_dev) = person(&state, 61);
        let project = project_of(&state, &alice, "Sizer");
        let png = png_bytes(&mark(1024));
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
        let r = crate::blobs::put(
            State(state.clone()),
            alice_dev.clone(),
            Path((project.clone(), "avatar-x.png".to_owned())),
            headers,
            axum::body::Bytes::from(png.clone()),
        )
        .await;
        assert_eq!(r.status(), StatusCode::NO_CONTENT);
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        let r = crate::blobs::put(
            State(state.clone()),
            alice_dev.clone(),
            Path((project.clone(), "notes.txt".to_owned())),
            headers,
            axum::body::Bytes::from_static(b"not a picture"),
        )
        .await;
        assert_eq!(r.status(), StatusCode::NO_CONTENT);

        async fn get(
            state: &AppState,
            who: &crate::auth::Principal,
            slot: &str,
            key: &str,
            w: Option<u32>,
        ) -> (StatusCode, String, Vec<u8>) {
            let r = crate::blobs::get(
                State(state.clone()),
                who.clone(),
                Path((slot.to_owned(), key.to_owned())),
                Query(SizeQuery { w }),
            )
            .await;
            let status = r.status();
            let mime = r
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_owned();
            let body = axum::body::to_bytes(r.into_body(), 1 << 24).await.unwrap();
            (status, mime, body.to_vec())
        }
        let (status, mime, body) =
            get(&state, &alice_dev, &project, "avatar-x.png", Some(40)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(mime, "image/png");
        let img = image::load_from_memory(&body).unwrap();
        assert_eq!(img.dimensions(), (48, 48), "40 → the 48 bucket");
        let (_, _, full) = get(&state, &alice_dev, &project, "avatar-x.png", None).await;
        assert_eq!(full, png, "no w: the stored bytes");
        let (status, mime, text) = get(&state, &alice_dev, &project, "notes.txt", Some(40)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(mime, "text/plain");
        assert_eq!(text, b"not a picture", "w is ignored for a non-image blob");
    }

    /// Uploads above the master size are brought down; smaller ones and undecodable bytes pass
    /// through untouched.
    #[test]
    fn uploads_are_normalized_only_when_too_large() {
        let big = png_bytes(&mark(1536));
        let m = normalize_upload(ImageKind::Png, &big).unwrap();
        assert_eq!(
            image::load_from_memory(&m.bytes).unwrap().dimensions(),
            (1024, 1024)
        );
        let fine = png_bytes(&mark(800));
        let m = normalize_upload(ImageKind::Png, &fine).unwrap();
        assert_eq!(m.bytes, fine, "kept byte for byte");
        let head = crate::image_sniff::samples::JPEG;
        let m = normalize_upload(ImageKind::Jpeg, head).unwrap();
        assert_eq!(m.bytes, head);
        assert_eq!(m.kind, ImageKind::Jpeg);
    }
}
