//! Copy of a message, the pasteboard side ([feedback copy-image-messages-mac-2026-10-27]).
//!
//! Before this, the bubble menu's Copy put `plain_text()` on the clipboard and nothing
//! else: a picture message copied nothing at all, a picture with a caption copied the
//! caption. Now every Copy — bubble menu, control-click, ⌘C on a whole bubble, the media
//! viewer, the Library — goes through [`plan`] + [`write`]:
//!
//! - every picture of the message is one **pasteboard item** carrying its **original
//!   bytes under its own type** (PNG stays `public.png`, JPEG `public.jpeg`, GIF
//!   `com.compuserve.gif`, WebP `org.webmproject.webp`, HEIC `public.heic`), plus a
//!   `public.png` re-encode when the native type is not PNG (what Chromium / Electron
//!   apps — Slack, Discord — read) and a `public.tiff` representation (what older AppKit
//!   readers ask for), all on the same item;
//! - the message's text rides on the **first** item as `public.utf8-plain-text`, so a
//!   text field receives the words and an image well receives the picture — one rich
//!   item, as Apple's pasteboard guide asks;
//! - several pictures become several items in message order;
//! - a file that is not a picture is written to a temp file and travels as a
//!   `public.file-url` item (Finder pastes the file, the composer stages it).
//!
//! gpui's own clipboard writes one type per image and drops images from a mixed item,
//! so on macOS this module talks to `NSPasteboard` itself; elsewhere it falls back to
//! gpui's `ClipboardItem` (best effort: text and the first image).

use std::path::{Path, PathBuf};

use gpui::{App, ClipboardEntry, ClipboardItem, ClipboardString, ImageFormat};

/// The bytes of one attachment as fetched from the relay (`GET /blob/{slot}/{key}`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetched {
    pub name: String,
    /// The relay's `content-type`; the attachment's own `mime` when the relay's is generic.
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// A picture on its way to the pasteboard: the original bytes under their own type and
/// the PNG twin for readers that only ask for `public.png`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageData {
    pub format: ImageFormat,
    pub bytes: Vec<u8>,
    /// `None` when the original already is a PNG (or could not be decoded).
    pub png: Option<Vec<u8>>,
}

impl ImageData {
    /// The UTI the original bytes are declared under.
    #[cfg_attr(any(not(target_os = "macos"), test), allow(dead_code))]
    pub fn uti(&self) -> &'static str {
        uti_of(self.format)
    }
}

/// One `NSPasteboardItem`: at most one picture or one file, plus the text that rides on it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PasteItem {
    pub text: Option<String>,
    pub image: Option<ImageData>,
    pub file: Option<PathBuf>,
}

impl PasteItem {
    /// The pasteboard types this item declares, most descriptive first — the order a
    /// reader that walks `types` sees (the native picture type before its fallbacks).
    #[cfg_attr(any(not(target_os = "macos"), test), allow(dead_code))]
    pub fn types(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if let Some(image) = &self.image {
            out.push(image.uti());
            if image.png.is_some() {
                out.push(UTI_PNG);
            }
            out.push(UTI_TIFF);
        }
        if self.file.is_some() {
            out.push(UTI_FILE_URL);
        }
        if self.text.is_some() {
            out.push(UTI_UTF8_TEXT);
        }
        out
    }
}

#[cfg_attr(any(not(target_os = "macos"), test), allow(dead_code))]
pub const UTI_PNG: &str = "public.png";
#[cfg_attr(any(not(target_os = "macos"), test), allow(dead_code))]
pub const UTI_JPEG: &str = "public.jpeg";
#[cfg_attr(any(not(target_os = "macos"), test), allow(dead_code))]
pub const UTI_GIF: &str = "com.compuserve.gif";
#[cfg_attr(any(not(target_os = "macos"), test), allow(dead_code))]
pub const UTI_WEBP: &str = "org.webmproject.webp";
#[cfg_attr(any(not(target_os = "macos"), test), allow(dead_code))]
pub const UTI_TIFF: &str = "public.tiff";
#[cfg_attr(any(not(target_os = "macos"), test), allow(dead_code))]
pub const UTI_BMP: &str = "com.microsoft.bmp";
#[cfg_attr(any(not(target_os = "macos"), test), allow(dead_code))]
pub const UTI_SVG: &str = "public.svg-image";
#[cfg_attr(any(not(target_os = "macos"), test), allow(dead_code))]
pub const UTI_ICO: &str = "com.microsoft.ico";
#[cfg_attr(any(not(target_os = "macos"), test), allow(dead_code))]
pub const UTI_PNM: &str = "public.pbm";
#[cfg_attr(any(not(target_os = "macos"), test), allow(dead_code))]
pub const UTI_UTF8_TEXT: &str = "public.utf8-plain-text";
#[cfg_attr(any(not(target_os = "macos"), test), allow(dead_code))]
pub const UTI_FILE_URL: &str = "public.file-url";

/// The UTI gpui's own reader expects for `format` (its `UTType` table).
#[cfg_attr(any(not(target_os = "macos"), test), allow(dead_code))]
pub fn uti_of(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => UTI_PNG,
        ImageFormat::Jpeg => UTI_JPEG,
        ImageFormat::Gif => UTI_GIF,
        ImageFormat::Webp => UTI_WEBP,
        ImageFormat::Tiff => UTI_TIFF,
        ImageFormat::Bmp => UTI_BMP,
        ImageFormat::Svg => UTI_SVG,
        ImageFormat::Ico => UTI_ICO,
        ImageFormat::Pnm => UTI_PNM,
    }
}

/// The image format for a declared pasteboard type, `None` for anything else.
#[cfg_attr(any(not(target_os = "macos"), test), allow(dead_code))]
pub fn format_of_uti(uti: &str) -> Option<ImageFormat> {
    Some(match uti {
        UTI_PNG => ImageFormat::Png,
        UTI_JPEG | "public.jpg" => ImageFormat::Jpeg,
        UTI_GIF => ImageFormat::Gif,
        UTI_WEBP => ImageFormat::Webp,
        UTI_TIFF => ImageFormat::Tiff,
        UTI_BMP => ImageFormat::Bmp,
        UTI_SVG => ImageFormat::Svg,
        UTI_ICO => ImageFormat::Ico,
        UTI_PNM => ImageFormat::Pnm,
        _ => return None,
    })
}

/// The format of a fetched picture: the bytes decide (the relay's `content-type` and the
/// attachment's `mime` are hints that have been wrong before — a `.png` named file that
/// is a JPEG), then the MIME, then the file name. `None` for anything that is not a
/// still picture we can declare.
pub fn image_format(fetched: &Fetched) -> Option<ImageFormat> {
    if let Some(format) = sniff(&fetched.bytes) {
        return Some(format);
    }
    let mime = fetched.mime.to_ascii_lowercase();
    let by_mime = match mime.split(';').next().unwrap_or("").trim() {
        "image/png" => Some(ImageFormat::Png),
        "image/jpeg" | "image/jpg" => Some(ImageFormat::Jpeg),
        "image/gif" => Some(ImageFormat::Gif),
        "image/webp" => Some(ImageFormat::Webp),
        "image/tiff" => Some(ImageFormat::Tiff),
        "image/bmp" | "image/x-ms-bmp" => Some(ImageFormat::Bmp),
        "image/svg+xml" => Some(ImageFormat::Svg),
        "image/x-icon" | "image/vnd.microsoft.icon" => Some(ImageFormat::Ico),
        _ => None,
    };
    by_mime.or_else(|| {
        let ext = Path::new(&fetched.name)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())?;
        match ext.as_str() {
            "png" => Some(ImageFormat::Png),
            "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
            "gif" => Some(ImageFormat::Gif),
            "webp" => Some(ImageFormat::Webp),
            "tif" | "tiff" => Some(ImageFormat::Tiff),
            "bmp" => Some(ImageFormat::Bmp),
            "svg" => Some(ImageFormat::Svg),
            "ico" => Some(ImageFormat::Ico),
            _ => None,
        }
    })
}

/// Magic bytes of the formats we declare (HEIC is left to the MIME: `ftypheic` boxes
/// have no gpui format and go out as a file).
fn sniff(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ImageFormat::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(ImageFormat::Gif)
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(ImageFormat::Webp)
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        Some(ImageFormat::Tiff)
    } else if bytes.starts_with(b"BM") && bytes.len() > 14 {
        Some(ImageFormat::Bmp)
    } else {
        None
    }
}

/// The `public.png` twin of a picture that is not a PNG: the first frame, re-encoded.
/// `None` for a PNG (the original is the PNG) and for bytes the decoder refuses (the
/// item then carries the native type and the TIFF only).
pub fn png_fallback(format: ImageFormat, bytes: &[u8]) -> Option<Vec<u8>> {
    if format == ImageFormat::Png {
        return None;
    }
    let image_format = match format {
        ImageFormat::Jpeg => image::ImageFormat::Jpeg,
        ImageFormat::Gif => image::ImageFormat::Gif,
        ImageFormat::Webp => image::ImageFormat::WebP,
        ImageFormat::Ico => image::ImageFormat::Ico,
        ImageFormat::Bmp => image::ImageFormat::Bmp,
        ImageFormat::Tiff => image::ImageFormat::Tiff,
        ImageFormat::Png | ImageFormat::Svg | ImageFormat::Pnm => return None,
    };
    let decoded = image::load_from_memory_with_format(bytes, image_format).ok()?;
    let mut out = std::io::Cursor::new(Vec::new());
    decoded.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// The pasteboard items for a message: its `text` and its attachments' bytes in message
/// order (`fetched` is what came back; a blob that did not is simply not there).
///
/// Pictures become image items, anything else a file item at `<files_dir>/<name>`; the
/// text rides on the first item. Text alone is one text item; nothing at all is no item.
pub fn plan(text: &str, fetched: &[Fetched], files_dir: &Path) -> Vec<PasteItem> {
    let mut items: Vec<PasteItem> = Vec::new();
    for f in fetched {
        if f.bytes.is_empty() {
            continue;
        }
        match image_format(f) {
            Some(format) => items.push(PasteItem {
                image: Some(ImageData {
                    format,
                    png: png_fallback(format, &f.bytes),
                    bytes: f.bytes.clone(),
                }),
                ..Default::default()
            }),
            None => {
                if let Ok(path) = stash_file(f, files_dir) {
                    items.push(PasteItem {
                        file: Some(path),
                        ..Default::default()
                    });
                }
            }
        }
    }
    let text = text.trim();
    if !text.is_empty() {
        match items.first_mut() {
            Some(first) => first.text = Some(text.to_owned()),
            None => items.push(PasteItem {
                text: Some(text.to_owned()),
                ..Default::default()
            }),
        }
    }
    items
}

/// `<dir>/<name>` for a copied file (the system clears its temp dir on its own); a
/// clashing name gets ` (n)`.
fn stash_file(f: &Fetched, dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let safe = Path::new(&f.name)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("file");
    let mut path = dir.join(safe);
    let stem = Path::new(safe)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file")
        .to_owned();
    let ext = Path::new(safe)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    std::fs::write(&path, &f.bytes)?;
    Ok(path)
}

/// Where copied files are staged.
pub fn copied_files_dir() -> PathBuf {
    std::env::temp_dir().join("arbos-copied")
}

/// Put `items` on the general pasteboard, replacing what was there. `true` when
/// something was written.
pub fn write(items: &[PasteItem], cx: &mut App) -> bool {
    if items.is_empty() {
        return false;
    }
    // Under `cargo test` the app runs on gpui's test platform, whose clipboard is a
    // field of the test context: the real NSPasteboard is neither what the tests write
    // to nor something a test may clobber (the release lane's `enter_tests` run on the
    // Mac runner and failed on this once, 2026-10-28 05:50–07:50).
    #[cfg(all(target_os = "macos", not(test)))]
    {
        let _ = cx;
        macos::write(items)
    }
    #[cfg(not(all(target_os = "macos", not(test))))]
    {
        cx.write_to_clipboard(fallback_item(items));
        true
    }
}

/// What the general pasteboard holds, every item of it: file URLs as `ExternalPaths`,
/// each picture under the first declared type we know (the native one — a GIF comes
/// back as a GIF, not its PNG twin), text as a `String` entry. gpui's own reader stops
/// at the first item's text, which is why a copied picture-with-caption pasted into the
/// composer as words only.
pub fn read(cx: &App) -> Option<ClipboardItem> {
    #[cfg(all(target_os = "macos", not(test)))]
    {
        let _ = cx;
        macos::read()
    }
    #[cfg(not(all(target_os = "macos", not(test))))]
    {
        cx.read_from_clipboard()
    }
}

/// The gpui `ClipboardItem` for `items` where the platform writer is not available
/// (Linux dev builds, tests): every text and every picture as entries; gpui's writer
/// keeps what it can of them.
pub fn fallback_item(items: &[PasteItem]) -> ClipboardItem {
    let mut entries = Vec::new();
    let mut paths = Vec::new();
    for item in items {
        if let Some(path) = &item.file {
            paths.push(path.clone());
        }
    }
    if !paths.is_empty() {
        entries.push(ClipboardEntry::ExternalPaths(gpui::ExternalPaths(
            paths.into(),
        )));
    }
    for item in items {
        if let Some(image) = &item.image {
            entries.push(ClipboardEntry::Image(gpui::Image {
                format: image.format,
                id: gpui::hash(&image.bytes),
                bytes: image.bytes.clone(),
            }));
        }
    }
    for item in items {
        if let Some(text) = &item.text {
            entries.push(ClipboardEntry::String(ClipboardString::new(text.clone())));
        }
    }
    ClipboardItem { entries }
}

#[cfg(all(target_os = "macos", not(test)))]
mod macos {
    use std::path::PathBuf;

    use gpui::{ClipboardEntry, ClipboardItem, ClipboardString};
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2::AnyThread;
    use objc2_app_kit::{NSImage, NSPasteboard, NSPasteboardItem, NSPasteboardWriting};
    use objc2_foundation::{NSArray, NSData, NSString, NSURL};

    use super::{format_of_uti, PasteItem, UTI_FILE_URL, UTI_PNG, UTI_TIFF, UTI_UTF8_TEXT};

    fn ns_type(uti: &str) -> Retained<NSString> {
        NSString::from_str(uti)
    }

    /// One `NSPasteboardItem` per [`PasteItem`], every type declared on it, then
    /// `writeObjects:` in one go so the items keep their order.
    pub(super) fn write(items: &[PasteItem]) -> bool {
        let pasteboard = NSPasteboard::generalPasteboard();
        let mut objects: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = Vec::new();
        for item in items {
            let pb_item = NSPasteboardItem::new();
            let mut any = false;
            if let Some(image) = &item.image {
                let native = NSData::from_vec(image.bytes.clone());
                any |= pb_item.setData_forType(&native, &ns_type(image.uti()));
                if let Some(png) = &image.png {
                    any |=
                        pb_item.setData_forType(&NSData::from_vec(png.clone()), &ns_type(UTI_PNG));
                }
                // AppKit's own TIFF of the picture: what readers that predate UTIs ask for.
                if image.uti() != UTI_TIFF {
                    if let Some(tiff) = NSImage::initWithData(NSImage::alloc(), &native)
                        .and_then(|img| img.TIFFRepresentation())
                    {
                        any |= pb_item.setData_forType(&tiff, &ns_type(UTI_TIFF));
                    }
                }
            }
            if let Some(path) = &item.file {
                let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
                if let Some(abs) = url.absoluteString() {
                    any |= pb_item.setString_forType(&abs, &ns_type(UTI_FILE_URL));
                }
            }
            if let Some(text) = &item.text {
                any |=
                    pb_item.setString_forType(&NSString::from_str(text), &ns_type(UTI_UTF8_TEXT));
            }
            if any {
                objects.push(ProtocolObject::from_retained(pb_item));
            }
        }
        if objects.is_empty() {
            return false;
        }
        pasteboard.clearContents();
        pasteboard.writeObjects(&NSArray::from_retained_slice(&objects))
    }

    /// Every pasteboard item → gpui entries (see [`super::read`]).
    pub(super) fn read() -> Option<ClipboardItem> {
        let pasteboard = NSPasteboard::generalPasteboard();
        let items = pasteboard.pasteboardItems()?;
        let mut paths: Vec<PathBuf> = Vec::new();
        let mut images = Vec::new();
        let mut text = String::new();
        for item in items.iter() {
            let types = item.types();
            let declared: Vec<String> = types.iter().map(|t| t.to_string()).collect();
            if declared.iter().any(|t| t == UTI_FILE_URL) {
                if let Some(s) = item.stringForType(&ns_type(UTI_FILE_URL)) {
                    if let Some(path) = NSURL::URLWithString(&s).and_then(|u| u.to_file_path()) {
                        paths.push(path);
                        continue;
                    }
                }
            }
            if let Some(format) = declared.iter().find_map(|t| format_of_uti(t)) {
                if let Some(data) = item.dataForType(&ns_type(super::uti_of(format))) {
                    let bytes = data.to_vec();
                    if !bytes.is_empty() {
                        images.push(ClipboardEntry::Image(gpui::Image {
                            format,
                            id: gpui::hash(&bytes),
                            bytes,
                        }));
                    }
                }
            }
            if declared.iter().any(|t| t == UTI_UTF8_TEXT) {
                if let Some(s) = item.stringForType(&ns_type(UTI_UTF8_TEXT)) {
                    text.push_str(&s.to_string());
                }
            }
        }
        let mut entries = Vec::new();
        if !paths.is_empty() {
            entries.push(ClipboardEntry::ExternalPaths(gpui::ExternalPaths(
                paths.into(),
            )));
        }
        entries.extend(images);
        if !text.is_empty() {
            entries.push(ClipboardEntry::String(ClipboardString::new(text)));
        }
        (!entries.is_empty()).then_some(ClipboardItem { entries })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_1x1() -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(1, 1, image::Rgba([10, 20, 30, 255]))
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    fn jpeg_2x2() -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(2, 2, image::Rgb([200, 100, 50]))
            .write_to(&mut out, image::ImageFormat::Jpeg)
            .unwrap();
        out.into_inner()
    }

    fn gif_3x2() -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut out);
            let frame = image::Frame::new(image::RgbaImage::from_pixel(
                3,
                2,
                image::Rgba([0, 255, 0, 255]),
            ));
            encoder.encode_frames(std::iter::once(frame)).unwrap();
        }
        out
    }

    fn fetched(name: &str, mime: &str, bytes: Vec<u8>) -> Fetched {
        Fetched {
            name: name.into(),
            mime: mime.into(),
            bytes,
        }
    }

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "arbos-pasteboard-test-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn text_only_is_one_text_item() {
        let items = plan("  hello there  ", &[], &dir("text"));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text.as_deref(), Some("hello there"));
        assert!(items[0].image.is_none() && items[0].file.is_none());
        assert_eq!(items[0].types(), [UTI_UTF8_TEXT]);
    }

    #[test]
    fn nothing_gives_no_item() {
        assert!(plan("   ", &[], &dir("empty")).is_empty());
        // Bytes that never came back are not an item either.
        assert!(plan("", &[fetched("a.png", "image/png", vec![])], &dir("empty2")).is_empty());
    }

    #[test]
    fn image_only_keeps_the_png_under_its_own_type_with_a_tiff_twin() {
        let png = png_1x1();
        let items = plan(
            "",
            &[fetched("shot.png", "image/png", png.clone())],
            &dir("png"),
        );
        assert_eq!(items.len(), 1);
        let image = items[0].image.as_ref().expect("an image item");
        assert_eq!(image.format, ImageFormat::Png);
        assert_eq!(image.bytes, png, "the original bytes, not a re-encode");
        assert!(image.png.is_none(), "a PNG needs no PNG twin");
        assert!(
            items[0].text.is_none(),
            "no caption → no text type on the item"
        );
        assert_eq!(items[0].types(), [UTI_PNG, UTI_TIFF]);
    }

    #[test]
    fn jpeg_stays_jpeg_and_gets_a_png_fallback() {
        let jpeg = jpeg_2x2();
        let items = plan(
            "",
            &[fetched("photo.jpg", "image/jpeg", jpeg.clone())],
            &dir("jpeg"),
        );
        let image = items[0].image.as_ref().unwrap();
        assert_eq!(image.format, ImageFormat::Jpeg);
        assert_eq!(image.uti(), "public.jpeg");
        assert_eq!(image.bytes, jpeg);
        let png = image
            .png
            .as_ref()
            .expect("a public.png twin for PNG-only readers");
        let decoded = image::load_from_memory_with_format(png, image::ImageFormat::Png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (2, 2));
        assert_eq!(items[0].types(), [UTI_JPEG, UTI_PNG, UTI_TIFF]);
    }

    #[test]
    fn gif_type_is_preserved() {
        let gif = gif_3x2();
        // A wrong MIME from the relay does not change the declared type: the bytes decide.
        let items = plan(
            "",
            &[fetched("loop.gif", "application/octet-stream", gif.clone())],
            &dir("gif"),
        );
        let image = items[0].image.as_ref().unwrap();
        assert_eq!(image.format, ImageFormat::Gif);
        assert_eq!(image.uti(), "com.compuserve.gif");
        assert_eq!(image.bytes, gif, "the GIF bytes themselves, frames and all");
        assert!(image.png.is_some(), "plus a PNG of the first frame");
        assert_eq!(
            items[0].types()[0],
            UTI_GIF,
            "the GIF is the first declared type"
        );
    }

    #[test]
    fn mixed_message_is_one_rich_item_with_text_and_picture() {
        let items = plan(
            "Look at this",
            &[fetched("shot.png", "image/png", png_1x1())],
            &dir("mixed"),
        );
        assert_eq!(items.len(), 1, "one item, several types — not two items");
        assert_eq!(items[0].text.as_deref(), Some("Look at this"));
        assert!(items[0].image.is_some());
        assert_eq!(items[0].types(), [UTI_PNG, UTI_TIFF, UTI_UTF8_TEXT]);
    }

    #[test]
    fn multiple_images_are_multiple_items_in_message_order_text_on_the_first() {
        let items = plan(
            "three shots",
            &[
                fetched("1.png", "image/png", png_1x1()),
                fetched("2.jpg", "image/jpeg", jpeg_2x2()),
                fetched("3.gif", "image/gif", gif_3x2()),
            ],
            &dir("multi"),
        );
        assert_eq!(items.len(), 3);
        let formats: Vec<ImageFormat> = items
            .iter()
            .map(|i| i.image.as_ref().unwrap().format)
            .collect();
        assert_eq!(
            formats,
            [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::Gif]
        );
        assert_eq!(items[0].text.as_deref(), Some("three shots"));
        assert!(items[1].text.is_none() && items[2].text.is_none());
    }

    #[test]
    fn a_file_travels_as_a_file_url_item() {
        let d = dir("file");
        let items = plan(
            "the deck",
            &[fetched(
                "deck.pdf",
                "application/pdf",
                b"%PDF-1.4 fake".to_vec(),
            )],
            &d,
        );
        assert_eq!(items.len(), 1);
        let path = items[0].file.as_ref().expect("a staged file");
        assert_eq!(path.file_name().unwrap(), "deck.pdf");
        assert_eq!(std::fs::read(path).unwrap(), b"%PDF-1.4 fake");
        assert_eq!(items[0].types(), [UTI_FILE_URL, UTI_UTF8_TEXT]);
        // A second copy of the same name does not overwrite the first.
        let again = plan(
            "",
            &[fetched("deck.pdf", "application/pdf", b"v2".to_vec())],
            &d,
        );
        assert_eq!(
            again[0].file.as_ref().unwrap().file_name().unwrap(),
            "deck (2).pdf"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn format_comes_from_the_bytes_then_the_mime_then_the_name() {
        assert_eq!(
            image_format(&fetched("x.bin", "application/octet-stream", jpeg_2x2())),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(
            image_format(&fetched("x.bin", "image/webp", vec![1, 2, 3])),
            Some(ImageFormat::Webp)
        );
        assert_eq!(
            image_format(&fetched("x.gif", "application/octet-stream", vec![1, 2, 3])),
            Some(ImageFormat::Gif)
        );
        assert_eq!(
            image_format(&fetched("notes.pdf", "application/pdf", vec![1, 2, 3])),
            None
        );
    }

    #[test]
    fn utis_round_trip() {
        for format in [
            ImageFormat::Png,
            ImageFormat::Jpeg,
            ImageFormat::Gif,
            ImageFormat::Webp,
            ImageFormat::Tiff,
            ImageFormat::Bmp,
        ] {
            assert_eq!(format_of_uti(uti_of(format)), Some(format));
        }
        assert_eq!(format_of_uti("public.utf8-plain-text"), None);
    }

    /// The test-platform path end to end: what `write` puts on gpui's clipboard,
    /// `read` hands the composer back — text, both pictures (formats kept). Under
    /// `cargo test` this is the path on the Mac too (the real pasteboard is never touched).
    #[gpui::test]
    fn write_then_read_round_trips_text_and_pictures(cx: &mut gpui::TestAppContext) {
        let items = plan(
            "caption",
            &[
                fetched("1.png", "image/png", png_1x1()),
                fetched("2.gif", "image/gif", gif_3x2()),
            ],
            &dir("roundtrip"),
        );
        assert!(cx.update(|cx| write(&items, cx)));
        let back = cx
            .update(|cx| read(cx))
            .expect("something on the clipboard");
        let formats: Vec<ImageFormat> = back
            .entries()
            .iter()
            .filter_map(|e| match e {
                ClipboardEntry::Image(i) => Some(i.format),
                _ => None,
            })
            .collect();
        assert_eq!(formats, [ImageFormat::Png, ImageFormat::Gif]);
        assert_eq!(back.text().as_deref(), Some("caption"));
        let pasted = crate::ui::composer::Pasted::from_clipboard(&back);
        assert_eq!(pasted.images.len(), 2, "the composer stages both pictures");
        assert_eq!(pasted.text.as_deref(), Some("caption"));
    }

    #[test]
    fn the_fallback_item_carries_every_entry() {
        let items = plan(
            "caption",
            &[
                fetched("1.png", "image/png", png_1x1()),
                fetched("2.jpg", "image/jpeg", jpeg_2x2()),
            ],
            &dir("fallback"),
        );
        let item = fallback_item(&items);
        let images = item
            .entries()
            .iter()
            .filter(|e| matches!(e, ClipboardEntry::Image(_)))
            .count();
        assert_eq!(images, 2);
        assert_eq!(item.text().as_deref(), Some("caption"));
    }
}
