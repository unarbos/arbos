//! The crop-and-draw document for a staged composer picture, and the one place
//! it is turned back into a file.
//!
//! Nothing here touches GPUI or the send path, so every rule the editor claims
//! is a unit test: the crop and the strokes are independent until Done, a
//! stroke outside the crop is kept and merely not drawn, and `Original` clears
//! both.
//!
//! **Coordinates.** A [`CropRect`] and every [`Stroke`] point are in *source-image
//! pixels* — the picture's own pixels, not the screen. That is the whole reason
//! widening a crop brings earlier strokes back: the strokes never moved, the
//! window onto them did. [`Placement`] is the only conversion between source
//! pixels and the canvas's points, and the editor owns no other one.

use std::path::{Path, PathBuf};

use gpui::{rgb, Hsla};

/// A crop in source-image pixels: the window of the picture that survives.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CropRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// A crop narrower or shorter than this (source pixels) is not a crop, it is a
/// slip of the hand: the drag stops rather than letting the rectangle collapse.
pub const MIN_CROP: f32 = 24.0;

impl CropRect {
    pub fn whole(w: f32, h: f32) -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            w,
            h,
        }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    /// Whether this crop still shows the whole picture `w` × `h`. A crop that
    /// does is no crop at all, which is what lets `Original` and "has an edit"
    /// agree without a second flag.
    pub fn is_whole(&self, w: f32, h: f32) -> bool {
        self.x <= 0.5 && self.y <= 0.5 && self.right() >= w - 0.5 && self.bottom() >= h - 0.5
    }

    fn clamped(mut self, w: f32, h: f32) -> Self {
        self.x = self.x.clamp(0.0, (w - MIN_CROP).max(0.0));
        self.y = self.y.clamp(0.0, (h - MIN_CROP).max(0.0));
        self.w = self.w.clamp(MIN_CROP.min(w), w - self.x);
        self.h = self.h.clamp(MIN_CROP.min(h), h - self.y);
        self
    }
}

/// The five inks. Fixed sRGB, not theme tokens: ink is paint on a photograph,
/// so it must not change when the user switches appearance or surface — the
/// annotation would stop meaning what it meant when it was drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ink {
    White,
    Red,
    Amber,
    Green,
    Blue,
}

impl Ink {
    pub const ALL: [Ink; 5] = [Ink::Red, Ink::White, Ink::Amber, Ink::Green, Ink::Blue];

    fn srgb(self) -> u32 {
        match self {
            Ink::White => 0xffffff,
            Ink::Red => 0xff453a,
            Ink::Amber => 0xffb000,
            Ink::Green => 0x34c759,
            Ink::Blue => 0x1084fe,
        }
    }

    pub fn hsla(self) -> Hsla {
        rgb(self.srgb()).into()
    }

    pub fn rgb8(self) -> [u8; 3] {
        let v = self.srgb();
        [(v >> 16) as u8, (v >> 8) as u8, v as u8]
    }

    /// For the swatch's accessible name and the tooltip.
    pub fn label(self) -> &'static str {
        match self {
            Ink::White => "White",
            Ink::Red => "Red",
            Ink::Amber => "Amber",
            Ink::Green => "Green",
            Ink::Blue => "Blue",
        }
    }
}

/// The three tools. Width belongs to the tool, not to a separate slider: a
/// width control is a second thing to learn for a feature whose whole request
/// was "scribble on it".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Pen,
    Marker,
    Eraser,
}

impl Tool {
    pub const ALL: [Tool; 3] = [Tool::Pen, Tool::Marker, Tool::Eraser];

    /// On-screen stroke width in points. Divided by the canvas scale when a
    /// stroke is recorded, so a line drawn on a zoomed-out photo is as thick
    /// in the flattened file as it looked while being drawn.
    pub fn screen_width(self) -> f32 {
        match self {
            Tool::Pen => 4.0,
            Tool::Marker => 18.0,
            // The eraser takes whole strokes, so its width is only the reach
            // of the pointer, never a painted width.
            Tool::Eraser => 20.0,
        }
    }

    /// How opaque the ink lands. A marker is a highlighter: it has to leave the
    /// photograph readable underneath or it is just a thick pen.
    pub fn alpha(self) -> f32 {
        match self {
            Tool::Pen => 1.0,
            Tool::Marker => 0.42,
            Tool::Eraser => 0.0,
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Tool::Pen => "pencil.tip",
            Tool::Marker => "highlighter",
            Tool::Eraser => "eraser",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tool::Pen => "Pen",
            Tool::Marker => "Marker",
            Tool::Eraser => "Eraser",
        }
    }
}

/// One freehand line, in source-image pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    pub ink: Ink,
    /// Width in source-image pixels.
    pub width: f32,
    pub alpha: f32,
    pub points: Vec<(f32, f32)>,
}

impl Stroke {
    /// Whether the pointer at `at` (source pixels) is within `reach` of this
    /// line — the eraser's hit test. The eraser takes whole strokes, like
    /// PencilKit's object eraser, because the strokes are vector data: erasing
    /// pixels would mean flattening them, and a flattened stroke cannot come
    /// back when the crop widens.
    pub fn touched_by(&self, at: (f32, f32), reach: f32) -> bool {
        let r = reach + self.width / 2.0;
        match self.points.as_slice() {
            [] => false,
            [only] => distance(at, *only) <= r,
            points => points
                .windows(2)
                .any(|seg| distance_to_segment(at, seg[0], seg[1]) <= r),
        }
    }
}

/// The edit: a crop and a list of strokes, each independent of the other, both
/// held until Done.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PhotoEdit {
    /// `None` is the whole picture. Never `Some(the whole picture)`, so that
    /// "is there an edit" is one question and not two.
    pub crop: Option<CropRect>,
    pub strokes: Vec<Stroke>,
}

impl PhotoEdit {
    pub fn is_empty(&self) -> bool {
        self.crop.is_none() && self.strokes.is_empty()
    }

    /// `Original`: one control clears both, because the state the user is
    /// asking for is the picture they picked, not "the crop I had before".
    pub fn clear(&mut self) {
        self.crop = None;
        self.strokes.clear();
    }

    /// Drop the last stroke. Undo is stroke-level only: a crop is visible while
    /// it is being made and reversible by dragging back, so giving ⌘Z two
    /// meanings would make it unpredictable for the one it is needed for.
    pub fn undo(&mut self) -> bool {
        self.strokes.pop().is_some()
    }

    /// Record a crop, folding "the whole picture" back to `None`.
    pub fn set_crop(&mut self, crop: CropRect, source: (f32, f32)) {
        self.crop = if crop.is_whole(source.0, source.1) {
            None
        } else {
            Some(crop)
        };
    }

    /// What is visible: the crop, or the whole picture when there is none.
    pub fn visible(&self, source: (f32, f32)) -> CropRect {
        self.crop
            .unwrap_or_else(|| CropRect::whole(source.0, source.1))
    }

    /// Take out every stroke the eraser's `at` touches. Returns whether any went.
    pub fn erase_at(&mut self, at: (f32, f32), reach: f32) -> bool {
        let before = self.strokes.len();
        self.strokes.retain(|s| !s.touched_by(at, reach));
        self.strokes.len() != before
    }
}

// ---------------------------------------------------------------------------
// Source pixels ↔ canvas points
// ---------------------------------------------------------------------------

/// Where the picture sits on the canvas: `visible` fitted and centred inside
/// the canvas, with `origin` the canvas point of source pixel (0, 0).
///
/// Crop mode fits the WHOLE picture so the rectangle can be dragged back out to
/// the edges; Draw mode fits the crop, so the canvas shows what will be sent.
/// Both are this one type with a different `visible`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub scale: f32,
    pub origin: (f32, f32),
}

impl Placement {
    pub fn fit(visible: CropRect, view: (f32, f32)) -> Self {
        let scale = if visible.w <= 0.0 || visible.h <= 0.0 {
            1.0
        } else {
            (view.0 / visible.w).min(view.1 / visible.h)
        };
        Self {
            scale,
            origin: (
                (view.0 - visible.w * scale) / 2.0 - visible.x * scale,
                (view.1 - visible.h * scale) / 2.0 - visible.y * scale,
            ),
        }
    }

    /// Move the whole placement across the canvas. [`Self::fit`] centres in a
    /// box that starts at the origin; a caller that fits into an inset box adds
    /// the inset back with this.
    pub fn shifted(mut self, by: (f32, f32)) -> Self {
        self.origin.0 += by.0;
        self.origin.1 += by.1;
        self
    }

    pub fn to_view(self, p: (f32, f32)) -> (f32, f32) {
        (
            self.origin.0 + p.0 * self.scale,
            self.origin.1 + p.1 * self.scale,
        )
    }

    pub fn to_source(self, p: (f32, f32)) -> (f32, f32) {
        if self.scale <= 0.0 {
            return (0.0, 0.0);
        }
        (
            (p.0 - self.origin.0) / self.scale,
            (p.1 - self.origin.1) / self.scale,
        )
    }

    /// The crop's rectangle in canvas points: what the overlay draws and what
    /// the handle hit test measures against.
    pub fn rect_in_view(&self, r: CropRect) -> (f32, f32, f32, f32) {
        let (x, y) = self.to_view((r.x, r.y));
        (x, y, r.w * self.scale, r.h * self.scale)
    }
}

// ---------------------------------------------------------------------------
// Dragging the crop
// ---------------------------------------------------------------------------

/// What a press on the crop rectangle grabbed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Top,
    Bottom,
    Left,
    Right,
    /// Inside the rectangle: the whole crop moves.
    Body,
}

/// How close to an edge or corner (canvas points) a press still counts as
/// grabbing it. Generous, because a 1 pt rim is not a target.
pub const HANDLE_SLOP: f32 = 14.0;

/// Which handle a press at `at` (canvas points) takes, for a crop drawn at
/// `rect` (canvas points, from [`Placement::rect_in_view`]). Corners win over
/// edges, edges over the body, so the corner brackets always do what they look
/// like they do.
pub fn handle_at(rect: (f32, f32, f32, f32), at: (f32, f32)) -> Option<Handle> {
    let (x, y, w, h) = rect;
    let near_l = (at.0 - x).abs() <= HANDLE_SLOP;
    let near_r = (at.0 - (x + w)).abs() <= HANDLE_SLOP;
    let near_t = (at.1 - y).abs() <= HANDLE_SLOP;
    let near_b = (at.1 - (y + h)).abs() <= HANDLE_SLOP;
    let within_x = at.0 >= x - HANDLE_SLOP && at.0 <= x + w + HANDLE_SLOP;
    let within_y = at.1 >= y - HANDLE_SLOP && at.1 <= y + h + HANDLE_SLOP;
    match (near_l, near_r, near_t, near_b) {
        (true, _, true, _) => Some(Handle::TopLeft),
        (_, true, true, _) => Some(Handle::TopRight),
        (true, _, _, true) => Some(Handle::BottomLeft),
        (_, true, _, true) => Some(Handle::BottomRight),
        (true, _, _, _) if within_y => Some(Handle::Left),
        (_, true, _, _) if within_y => Some(Handle::Right),
        (_, _, true, _) if within_x => Some(Handle::Top),
        (_, _, _, true) if within_x => Some(Handle::Bottom),
        _ if at.0 > x && at.0 < x + w && at.1 > y && at.1 < y + h => Some(Handle::Body),
        _ => None,
    }
}

/// Move `start` by `(dx, dy)` source pixels on `handle`, kept inside the
/// picture and never smaller than [`MIN_CROP`].
pub fn drag_crop(
    start: CropRect,
    handle: Handle,
    delta: (f32, f32),
    source: (f32, f32),
) -> CropRect {
    let (dx, dy) = delta;
    let (sw, sh) = source;
    let mut r = start;
    let mut edge = |left: bool, right: bool, top: bool, bottom: bool| {
        if left {
            let x = (start.x + dx).clamp(0.0, start.right() - MIN_CROP);
            r.w = start.right() - x;
            r.x = x;
        }
        if right {
            r.w = (start.w + dx).clamp(MIN_CROP, sw - start.x);
        }
        if top {
            let y = (start.y + dy).clamp(0.0, start.bottom() - MIN_CROP);
            r.h = start.bottom() - y;
            r.y = y;
        }
        if bottom {
            r.h = (start.h + dy).clamp(MIN_CROP, sh - start.y);
        }
    };
    match handle {
        Handle::TopLeft => edge(true, false, true, false),
        Handle::TopRight => edge(false, true, true, false),
        Handle::BottomLeft => edge(true, false, false, true),
        Handle::BottomRight => edge(false, true, false, true),
        Handle::Left => edge(true, false, false, false),
        Handle::Right => edge(false, true, false, false),
        Handle::Top => edge(false, false, true, false),
        Handle::Bottom => edge(false, false, false, true),
        Handle::Body => {
            r.x = (start.x + dx).clamp(0.0, (sw - start.w).max(0.0));
            r.y = (start.y + dy).clamp(0.0, (sh - start.h).max(0.0));
        }
    }
    r.clamped(sw, sh)
}

// ---------------------------------------------------------------------------
// Which attachments the editor takes
// ---------------------------------------------------------------------------

/// Why a staged file does not open the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotEditable {
    /// Not a picture at all: a video, a PDF, a document.
    NotAPicture,
    /// A GIF or WebP with more than one frame. Editing it would flatten the
    /// animation to a single frame, and doing that silently is worse than not
    /// offering the editor: the user would send something they did not make.
    Animated,
}

/// Whether `bytes` is a still picture the editor can take, and its pixel size
/// — the coordinate system the whole edit is expressed in.
///
/// The test is **frame count, not mime type**: an animated GIF and a still GIF
/// are both `image/gif`, and the file extension cannot tell them apart. A
/// one-frame GIF is a photograph and edits normally.
///
/// The size comes from the header rather than a decode, because this runs for
/// every file the composer stages.
pub fn editable(bytes: &[u8]) -> Result<(u32, u32), NotEditable> {
    use image::AnimationDecoder;
    use std::io::Cursor;

    let gif = bytes.starts_with(b"GIF8");
    let webp = bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP";
    if gif || webp {
        // Two frames is all the answer needs, so a long animation is never
        // decoded in full just to be refused.
        let frames: Box<dyn Iterator<Item = image::ImageResult<image::Frame>>> = if gif {
            match image::codecs::gif::GifDecoder::new(Cursor::new(bytes)) {
                Ok(d) => Box::new(d.into_frames()),
                Err(_) => return Err(NotEditable::NotAPicture),
            }
        } else {
            match image::codecs::webp::WebPDecoder::new(Cursor::new(bytes)) {
                Ok(d) => Box::new(d.into_frames()),
                // A still WebP is often rejected by the animation decoder; the
                // header read below still has to get its chance.
                Err(_) => Box::new(std::iter::empty()),
            }
        };
        if frames.take(2).filter(|f| f.is_ok()).count() > 1 {
            return Err(NotEditable::Animated);
        }
    }
    let size = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| NotEditable::NotAPicture)?
        .into_dimensions()
        .map_err(|_| NotEditable::NotAPicture)?;
    if size.0 == 0 || size.1 == 0 {
        return Err(NotEditable::NotAPicture);
    }
    Ok(size)
}

// ---------------------------------------------------------------------------
// Done: the edit becomes a file
// ---------------------------------------------------------------------------

/// Where the flattened copies live. A temp directory, like the pasted-image
/// stash beside it: the file only has to outlive the send, and the system
/// clears it.
pub fn edited_dir() -> PathBuf {
    std::env::temp_dir().join("arbos-edited")
}

/// The original decoded for the editor's canvas, with the picture's own pixel
/// size — the coordinate system the whole edit is expressed in.
///
/// Deliberately **not** `app::decode_thumbnail`, which trims transparent
/// margins through `opaque_rect` before it resizes. That is right for a 64 pt
/// tile and wrong here: the trim would put the canvas and [`flatten`] in two
/// different coordinate systems, so a stroke would land somewhere else in the
/// file than where it was drawn on screen.
pub fn decode_for_canvas(bytes: &[u8], max_side: u32) -> Option<(gpui::RenderImage, (u32, u32))> {
    let img = image::load_from_memory(bytes).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let long = w.max(h);
    let shown = if long <= max_side {
        img
    } else {
        let scale = max_side as f32 / long as f32;
        image::imageops::resize(
            &img,
            ((w as f32) * scale).round().max(1.0) as u32,
            ((h as f32) * scale).round().max(1.0) as u32,
            image::imageops::FilterType::Triangle,
        )
    };
    crate::app::bgra_frame(shown).map(|frame| (frame, (w, h)))
}

/// Draw `edit` into `original` and write the result as a PNG in `dir`,
/// returning its path.
///
/// PNG whatever went in: a crop and a stroke are a deliberate change, and
/// re-encoding them as JPEG would add compression noise to the one part of the
/// picture the user drew attention to. The send path reads the extension for
/// the mime type (`sync::relay::mime_for`), so a `.png` needs nothing else.
pub fn flatten(original: &Path, edit: &PhotoEdit, dir: &Path) -> std::io::Result<PathBuf> {
    let bytes = std::fs::read(original)?;
    let mut img = image::load_from_memory(&bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?
        .into_rgba8();
    for stroke in &edit.strokes {
        paint_stroke(&mut img, stroke);
    }
    let (w, h) = (img.width() as f32, img.height() as f32);
    let out = match edit.crop {
        Some(c) if !c.is_whole(w, h) => {
            let x = c.x.max(0.0).round() as u32;
            let y = c.y.max(0.0).round() as u32;
            let cw = (c.w.round() as u32)
                .min(img.width().saturating_sub(x))
                .max(1);
            let ch = (c.h.round() as u32)
                .min(img.height().saturating_sub(y))
                .max(1);
            image::imageops::crop_imm(&img, x, y, cw, ch).to_image()
        }
        _ => img,
    };
    std::fs::create_dir_all(dir)?;
    let path = unique_png(dir, original);
    out.save(&path).map_err(std::io::Error::other)?;
    Ok(path)
}

/// `Screenshot (edited).png`, then `(edited 2)`, so two edits of one file never
/// collide and the name still says what the attachment is.
fn unique_png(dir: &Path, original: &Path) -> PathBuf {
    let stem = original
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Picture");
    let mut path = dir.join(format!("{stem} (edited).png"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem} (edited {n}).png"));
        n += 1;
    }
    path
}

/// One stroke, anti-aliased, round-capped.
///
/// Coverage is accumulated for the whole stroke before anything is composited.
/// Blending each segment on its own would double-darken every join, which a
/// marker at 42 % shows as a row of beads along the line.
fn paint_stroke(img: &mut image::RgbaImage, stroke: &Stroke) {
    let half = (stroke.width / 2.0).max(0.5);
    let alpha = stroke.alpha.clamp(0.0, 1.0);
    if stroke.points.is_empty() || alpha <= 0.0 {
        return;
    }
    let (iw, ih) = (img.width() as i64, img.height() as i64);
    let pad = half + 1.0;
    let min_x = stroke.points.iter().fold(f32::MAX, |a, p| a.min(p.0)) - pad;
    let max_x = stroke.points.iter().fold(f32::MIN, |a, p| a.max(p.0)) + pad;
    let min_y = stroke.points.iter().fold(f32::MAX, |a, p| a.min(p.1)) - pad;
    let max_y = stroke.points.iter().fold(f32::MIN, |a, p| a.max(p.1)) + pad;
    let x0 = (min_x.floor() as i64).clamp(0, iw);
    let x1 = (max_x.ceil() as i64).clamp(0, iw);
    let y0 = (min_y.floor() as i64).clamp(0, ih);
    let y1 = (max_y.ceil() as i64).clamp(0, ih);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let bw = (x1 - x0) as usize;
    let mut coverage = vec![0f32; bw * (y1 - y0) as usize];
    for y in y0..y1 {
        for x in x0..x1 {
            let at = (x as f32 + 0.5, y as f32 + 0.5);
            let mut d = f32::MAX;
            match stroke.points.as_slice() {
                [only] => d = distance(at, *only),
                points => {
                    for seg in points.windows(2) {
                        d = d.min(distance_to_segment(at, seg[0], seg[1]));
                    }
                }
            }
            // One pixel of feather: full ink inside, nothing a pixel outside.
            let c = (half + 0.5 - d).clamp(0.0, 1.0);
            if c > 0.0 {
                coverage[(y - y0) as usize * bw + (x - x0) as usize] = c;
            }
        }
    }
    let [ir, ig, ib] = stroke.ink.rgb8();
    for y in y0..y1 {
        for x in x0..x1 {
            let c = coverage[(y - y0) as usize * bw + (x - x0) as usize] * alpha;
            if c <= 0.0 {
                continue;
            }
            let p = img.get_pixel_mut(x as u32, y as u32);
            for (channel, ink) in p.0.iter_mut().zip([ir, ig, ib]) {
                *channel = (*channel as f32 * (1.0 - c) + ink as f32 * c).round() as u8;
            }
            p.0[3] = (p.0[3] as f32 * (1.0 - c) + 255.0 * c).round() as u8;
        }
    }
}

fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

fn distance_to_segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (vx, vy) = (b.0 - a.0, b.1 - a.1);
    let len2 = vx * vx + vy * vy;
    if len2 <= f32::EPSILON {
        return distance(p, a);
    }
    let t = (((p.0 - a.0) * vx + (p.1 - a.1) * vy) / len2).clamp(0.0, 1.0);
    distance(p, (a.0 + vx * t, a.1 + vy * t))
}

#[cfg(test)]
mod tests {
    //! The document's rules, which are the approved design's rules: the crop
    //! and the strokes are independent until Done, a stroke outside the crop is
    //! kept and merely not drawn, `Original` clears both, and the editor is
    //! never offered for an animated picture.
    use super::*;

    const SOURCE: (f32, f32) = (1200.0, 800.0);

    fn stroke_at(points: &[(f32, f32)]) -> Stroke {
        Stroke {
            ink: Ink::Red,
            width: 6.0,
            alpha: 1.0,
            points: points.to_vec(),
        }
    }

    #[test]
    fn a_stroke_outside_the_crop_is_kept_and_the_crop_can_bring_it_back() {
        let mut edit = PhotoEdit::default();
        edit.strokes
            .push(stroke_at(&[(100.0, 100.0), (140.0, 130.0)]));
        edit.set_crop(
            CropRect {
                x: 600.0,
                y: 0.0,
                w: 600.0,
                h: 800.0,
            },
            SOURCE,
        );
        // The stroke is outside the crop; the crop does not touch it.
        assert_eq!(edit.strokes.len(), 1, "cropping away deleted a stroke");
        let visible = edit.visible(SOURCE);
        assert!(
            edit.strokes[0].points[0].0 < visible.x,
            "the stroke should be outside the visible window"
        );
        // Widening the crop back to the whole picture shows it again, unmoved.
        edit.set_crop(CropRect::whole(SOURCE.0, SOURCE.1), SOURCE);
        assert_eq!(edit.crop, None, "the whole picture is not a crop");
        assert_eq!(edit.strokes[0].points[0], (100.0, 100.0));
    }

    #[test]
    fn crop_and_strokes_do_not_disturb_each_other() {
        let mut edit = PhotoEdit::default();
        let crop = CropRect {
            x: 10.0,
            y: 20.0,
            w: 300.0,
            h: 400.0,
        };
        edit.set_crop(crop, SOURCE);
        edit.strokes.push(stroke_at(&[(0.0, 0.0), (10.0, 10.0)]));
        assert_eq!(edit.crop, Some(crop), "drawing moved the crop");
        edit.set_crop(
            CropRect {
                x: 0.0,
                y: 0.0,
                w: 500.0,
                h: 500.0,
            },
            SOURCE,
        );
        assert_eq!(edit.strokes.len(), 1, "cropping dropped the drawing");
    }

    #[test]
    fn original_clears_both_and_undo_only_takes_strokes() {
        let mut edit = PhotoEdit::default();
        edit.set_crop(
            CropRect {
                x: 5.0,
                y: 5.0,
                w: 100.0,
                h: 100.0,
            },
            SOURCE,
        );
        edit.strokes.push(stroke_at(&[(1.0, 1.0)]));
        edit.strokes.push(stroke_at(&[(2.0, 2.0)]));
        assert!(edit.undo(), "undo takes the last stroke");
        assert_eq!(edit.strokes.len(), 1);
        assert!(edit.crop.is_some(), "undo must not touch the crop");
        edit.clear();
        assert!(edit.is_empty(), "Original left something behind");
    }

    #[test]
    fn the_eraser_takes_the_stroke_it_touches_and_leaves_the_others() {
        let mut edit = PhotoEdit::default();
        edit.strokes.push(stroke_at(&[(0.0, 0.0), (100.0, 0.0)]));
        edit.strokes
            .push(stroke_at(&[(0.0, 500.0), (100.0, 500.0)]));
        assert!(edit.erase_at((50.0, 2.0), 8.0));
        assert_eq!(edit.strokes.len(), 1);
        assert_eq!(edit.strokes[0].points[0], (0.0, 500.0));
        assert!(!edit.erase_at((600.0, 600.0), 8.0), "erased empty space");
    }

    #[test]
    fn a_point_maps_to_source_and_back_unchanged() {
        // Crop mode: the whole picture fitted in a short, wide canvas.
        let p = Placement::fit(CropRect::whole(SOURCE.0, SOURCE.1), (600.0, 300.0));
        let there = p.to_view((300.0, 200.0));
        let back = p.to_source(there);
        assert!((back.0 - 300.0).abs() < 0.01 && (back.1 - 200.0).abs() < 0.01);
        // Fit means contained: the picture's long side fills the canvas.
        assert!((p.scale - 300.0 / 800.0).abs() < 1e-4, "scale {}", p.scale);
    }

    #[test]
    fn a_corner_drag_never_collapses_or_leaves_the_picture() {
        let start = CropRect {
            x: 100.0,
            y: 100.0,
            w: 200.0,
            h: 200.0,
        };
        // Dragged far past the opposite corner: stops at the minimum.
        let tiny = drag_crop(start, Handle::TopLeft, (9000.0, 9000.0), SOURCE);
        assert!(tiny.w >= MIN_CROP && tiny.h >= MIN_CROP, "{tiny:?}");
        // Dragged far outside: stops at the picture's edge.
        let big = drag_crop(start, Handle::BottomRight, (9000.0, 9000.0), SOURCE);
        assert!(
            big.right() <= SOURCE.0 + 0.01 && big.bottom() <= SOURCE.1 + 0.01,
            "{big:?}"
        );
        // The body moves without resizing.
        let moved = drag_crop(start, Handle::Body, (-9000.0, 50.0), SOURCE);
        assert_eq!((moved.w, moved.h), (start.w, start.h));
        assert_eq!(moved.x, 0.0);
    }

    #[test]
    fn corners_win_over_edges_so_the_brackets_do_what_they_look_like() {
        let rect = (100.0, 100.0, 200.0, 200.0);
        assert_eq!(handle_at(rect, (100.0, 100.0)), Some(Handle::TopLeft));
        assert_eq!(handle_at(rect, (300.0, 300.0)), Some(Handle::BottomRight));
        assert_eq!(handle_at(rect, (200.0, 100.0)), Some(Handle::Top));
        assert_eq!(handle_at(rect, (100.0, 200.0)), Some(Handle::Left));
        assert_eq!(handle_at(rect, (200.0, 200.0)), Some(Handle::Body));
        assert_eq!(handle_at(rect, (20.0, 20.0)), None);
    }

    /// A one-frame GIF is a photograph and edits; a two-frame GIF is an
    /// animation and the editor is not offered for it. The test is the frames,
    /// not the `image/gif` mime both of them carry.
    #[test]
    fn the_animated_test_is_frames_and_not_the_mime_type() {
        fn gif(frames: usize) -> Vec<u8> {
            let mut out = Vec::new();
            {
                let mut enc = image::codecs::gif::GifEncoder::new(&mut out);
                for i in 0..frames {
                    let shade = 10 * i as u8;
                    let buf =
                        image::RgbaImage::from_pixel(4, 4, image::Rgba([shade, shade, shade, 255]));
                    enc.encode_frame(image::Frame::new(buf)).expect("frame");
                }
            }
            out
        }
        assert_eq!(editable(&gif(1)), Ok((4, 4)), "a still GIF edits");
        assert_eq!(
            editable(&gif(3)),
            Err(NotEditable::Animated),
            "an animated GIF must not open the editor"
        );
        assert_eq!(editable(b"not a picture"), Err(NotEditable::NotAPicture));
    }

    /// Done is the only thing that writes pixels. The written file carries the
    /// crop's size and the stroke's ink, and the original file is untouched.
    #[test]
    fn done_writes_a_cropped_png_with_the_ink_in_it_and_leaves_the_original() {
        let dir = std::env::temp_dir().join(format!("arbos-edit-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let original = dir.join("Screenshot.png");
        image::RgbaImage::from_pixel(200, 120, image::Rgba([0, 0, 0, 255]))
            .save(&original)
            .expect("write original");
        let before = std::fs::read(&original).expect("read original");

        let edit = PhotoEdit {
            crop: Some(CropRect {
                x: 50.0,
                y: 20.0,
                w: 100.0,
                h: 60.0,
            }),
            strokes: vec![Stroke {
                ink: Ink::Red,
                width: 8.0,
                alpha: 1.0,
                points: vec![(60.0, 50.0), (140.0, 50.0)],
            }],
        };
        let out = flatten(&original, &edit, &dir).expect("flatten");
        assert_eq!(out.extension().and_then(|e| e.to_str()), Some("png"));
        assert!(out
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .contains("edited"));
        let flat = image::open(&out).expect("open result").into_rgba8();
        assert_eq!(flat.dimensions(), (100, 60), "the crop set the size");
        // The stroke ran through the crop's middle row, in red.
        let mid = flat.get_pixel(50, 30);
        assert!(
            mid.0[0] > 200 && mid.0[1] < 120,
            "expected red ink at the stroke, got {:?}",
            mid.0
        );
        assert_eq!(
            std::fs::read(&original).expect("reread"),
            before,
            "flatten wrote over the original"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A marker is translucent, so a join that composited twice shows as a
    /// bead. Coverage for the whole stroke is accumulated before anything is
    /// blended, so the corner is no darker than the straight run beside it.
    #[test]
    fn a_marker_join_is_not_darker_than_the_line_it_turns_on() {
        let mut img = image::RgbaImage::from_pixel(80, 80, image::Rgba([255, 255, 255, 255]));
        paint_stroke(
            &mut img,
            &Stroke {
                ink: Ink::Red,
                width: 10.0,
                alpha: Tool::Marker.alpha(),
                // Many short segments meeting at one point: the worst case.
                points: vec![(20.0, 40.0), (39.0, 40.0), (40.0, 40.0), (60.0, 40.0)],
            },
        );
        let straight = img.get_pixel(25, 40).0[1] as i32;
        let join = img.get_pixel(40, 40).0[1] as i32;
        assert!(
            (join - straight).abs() <= 2,
            "the join at {join} is darker than the line at {straight}"
        );
    }
}
