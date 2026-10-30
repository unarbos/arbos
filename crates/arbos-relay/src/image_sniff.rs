//! Avatar uploads are images, judged on the bytes — never on the file name. A picture for a
//! person, a group or a server passes only when its `Content-Type` names one of the allowed
//! image types AND the first bytes carry that type's signature. One rule for every client.

use std::fmt;

/// The image types an avatar may be (D79 photos): the same list every picker offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageKind {
    Png,
    Jpeg,
    Gif,
    Webp,
    Heic,
}

impl ImageKind {
    /// The kind a `Content-Type` header names; parameters (`; charset=…`) and case are
    /// ignored, the usual aliases (`image/jpg`, `image/heif`, `image/x-png`) are accepted.
    pub fn from_mime(content_type: &str) -> Option<Self> {
        let base = content_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        match base.as_str() {
            "image/png" | "image/x-png" => Some(ImageKind::Png),
            "image/jpeg" | "image/jpg" | "image/pjpeg" => Some(ImageKind::Jpeg),
            "image/gif" => Some(ImageKind::Gif),
            "image/webp" => Some(ImageKind::Webp),
            "image/heic" | "image/heif" | "image/heic-sequence" | "image/heif-sequence" => {
                Some(ImageKind::Heic)
            }
            _ => None,
        }
    }
}

impl fmt::Display for ImageKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ImageKind::Png => "PNG",
            ImageKind::Jpeg => "JPEG",
            ImageKind::Gif => "GIF",
            ImageKind::Webp => "WebP",
            ImageKind::Heic => "HEIC",
        })
    }
}

/// Human list of the allowed types, for error messages.
pub const ALLOWED_LIST: &str = "PNG, JPEG, HEIC, GIF or WebP";

/// How many leading bytes `sniff` needs to decide; callers reading from disk read this much.
pub const SNIFF_LEN: usize = 16;

/// The image type the bytes themselves announce, or `None` when they are not one of ours.
pub fn sniff(bytes: &[u8]) -> Option<ImageKind> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(ImageKind::Png);
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(ImageKind::Jpeg);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some(ImageKind::Gif);
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some(ImageKind::Webp);
    }
    // ISO BMFF: a size, then `ftyp`, then the major brand. HEIF files from iPhones and Macs
    // announce `heic`/`heix` (still), `hevc`/`hevx` (sequence) or the structural brands
    // `mif1`/`msf1`.
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        let brand = &bytes[8..12];
        let heif = [
            b"heic", b"heix", b"hevc", b"hevx", b"heim", b"heis", b"hevm", b"hevs", b"mif1",
            b"msf1", b"heif",
        ];
        if heif.iter().any(|b| *b == brand) {
            return Some(ImageKind::Heic);
        }
    }
    None
}

/// Why an upload is not an acceptable avatar; `Display` is the plain message a client shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotAnImage {
    /// No `Content-Type` at all.
    MissingMime,
    /// A `Content-Type` outside the allowed list.
    BadMime(String),
    /// The bytes carry no known image signature.
    BadBytes,
    /// Header and bytes disagree (`image/png` header, JPEG bytes, …).
    Mismatch { header: ImageKind, bytes: ImageKind },
}

impl fmt::Display for NotAnImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NotAnImage::MissingMime => {
                write!(
                    f,
                    "Pictures need a Content-Type; send a {ALLOWED_LIST} image"
                )
            }
            NotAnImage::BadMime(m) => {
                write!(f, "{m} is not an image; pictures must be {ALLOWED_LIST}")
            }
            NotAnImage::BadBytes => {
                write!(
                    f,
                    "That file is not an image; pictures must be {ALLOWED_LIST}"
                )
            }
            NotAnImage::Mismatch { header, bytes } => write!(
                f,
                "The file is {bytes} but was sent as {header}; pictures must be {ALLOWED_LIST}"
            ),
        }
    }
}

/// The gate every avatar endpoint runs: the declared type must be an allowed image, the
/// bytes must carry that same type's signature. Returns the agreed kind.
pub fn check_avatar(content_type: Option<&str>, bytes: &[u8]) -> Result<ImageKind, NotAnImage> {
    let header = match content_type.map(str::trim).filter(|s| !s.is_empty()) {
        None => return Err(NotAnImage::MissingMime),
        Some(ct) => ImageKind::from_mime(ct).ok_or_else(|| {
            NotAnImage::BadMime(ct.split(';').next().unwrap_or(ct).trim().to_owned())
        })?,
    };
    let kind = sniff(bytes).ok_or(NotAnImage::BadBytes)?;
    if kind != header {
        return Err(NotAnImage::Mismatch {
            header,
            bytes: kind,
        });
    }
    Ok(kind)
}

/// The largest picture the relay will decode, per side.
///
/// A picture's header says how big it is and its pixels come out of a compressed body, so a
/// small file can ask for an enormous frame buffer: measured on 2026-10-30, an 854 KB grey PNG
/// declaring 30000 x 30000 took 1.8 GB resident to decode, and a 3.5 MB one declaring
/// 60000 x 60000 asked the allocator for 3.6 GB in a single call — which, when the allocation
/// fails, aborts the process rather than the request. `image`'s own `max_alloc` does not cover
/// it (`decoder_to_vec` reserves the whole output buffer itself, outside the decoders'
/// accounting), but `max_image_width` / `max_image_height` are strict and are checked from the
/// header before any of it happens.
///
/// Everything the relay decodes is a profile picture, a link preview or a site icon, and all of
/// them are resampled down to 1024 px or less, so 8192 px a side — a 67 Mpx photo, larger than
/// any phone takes — is well past anything real.
pub const MAX_DECODE_SIDE: u32 = 8192;

/// What [`decode`] and [`decoder`] say about a picture too large to hold in memory.
pub const TOO_LARGE: &str = "picture is too large to decode";

fn reader(bytes: &[u8]) -> Result<image::ImageReader<std::io::Cursor<&[u8]>>, String> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("not an image: {e}"))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DECODE_SIDE);
    limits.max_image_height = Some(MAX_DECODE_SIDE);
    reader.limits(limits);
    Ok(reader)
}

fn why(e: image::ImageError) -> String {
    match e {
        image::ImageError::Limits(_) => TOO_LARGE.to_owned(),
        other => format!("decode: {other}"),
    }
}

/// Decode a picture, refusing one over [`MAX_DECODE_SIDE`] before anything is allocated for it.
/// Every decode of bytes the relay did not make itself goes through here or [`decoder`].
pub fn decode(bytes: &[u8]) -> Result<image::DynamicImage, String> {
    reader(bytes)?.decode().map_err(why)
}

/// Same, for a caller that needs the decoder itself (EXIF orientation).
pub fn decoder(bytes: &[u8]) -> Result<impl image::ImageDecoder + '_, String> {
    reader(bytes)?.into_decoder().map_err(why)
}

#[cfg(test)]
pub(crate) mod samples {
    //! Minimal byte heads for tests: real signatures, no pixel data needed to be judged.

    pub const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR\x00\x00\x00\x01";
    pub const JPEG: &[u8] = &[
        0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0x00,
    ];
    pub const GIF: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00";
    pub const WEBP: &[u8] = b"RIFF\x24\x00\x00\x00WEBPVP8 \x18\x00\x00\x00";
    pub const HEIC: &[u8] = b"\x00\x00\x00\x18ftypheic\x00\x00\x00\x00mif1heic";
    /// The head of an ONNX model (a protobuf), as in the report that opened this work.
    pub const ONNX: &[u8] = b"\x08\x07\x12\x07pytorch\x1a\x052.1.0:\xe4\x03";
    pub const TEXT: &[u8] = b"hello, this is not a picture at all\n";

    /// A whole, well-formed PNG of `w` x `h` whose pixel data is a stub: the IHDR states the
    /// size, and a decoder reserves the frame buffer for it before reading any pixels, so this
    /// is as expensive to open as a real picture that big (30000 x 30000 is 900 MB) and 68 bytes
    /// on the wire.
    pub fn png_claiming(w: u32, h: u32) -> Vec<u8> {
        fn crc32(bytes: &[u8]) -> u32 {
            let mut c = 0xFFFF_FFFFu32;
            for b in bytes {
                c ^= u32::from(*b);
                for _ in 0..8 {
                    c = if c & 1 == 1 {
                        0xEDB8_8320 ^ (c >> 1)
                    } else {
                        c >> 1
                    };
                }
            }
            c ^ 0xFFFF_FFFF
        }
        fn chunk(typ: &[u8], data: &[u8]) -> Vec<u8> {
            let mut body = typ.to_vec();
            body.extend_from_slice(data);
            let mut out = (data.len() as u32).to_be_bytes().to_vec();
            out.extend_from_slice(&body);
            out.extend_from_slice(&crc32(&body).to_be_bytes());
            out
        }
        let mut ihdr = w.to_be_bytes().to_vec();
        ihdr.extend(h.to_be_bytes());
        ihdr.extend([8, 0, 0, 0, 0]); // 8-bit greyscale, no interlace
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        out.extend(chunk(b"IHDR", &ihdr));
        out.extend(chunk(
            b"IDAT",
            b"\x78\x9c\x01\x00\x00\xff\xff\x00\x00\x00\x01",
        ));
        out.extend(chunk(b"IEND", b""));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::samples::*;
    use super::*;

    #[test]
    fn sniff_recognises_each_allowed_type_and_nothing_else() {
        assert_eq!(sniff(PNG), Some(ImageKind::Png));
        assert_eq!(sniff(JPEG), Some(ImageKind::Jpeg));
        assert_eq!(sniff(GIF), Some(ImageKind::Gif));
        assert_eq!(sniff(WEBP), Some(ImageKind::Webp));
        assert_eq!(sniff(HEIC), Some(ImageKind::Heic));
        assert_eq!(
            sniff(b"\x00\x00\x00\x1cftypmif1\x00\x00\x00\x00mif1heic"),
            Some(ImageKind::Heic),
            "structural HEIF brand"
        );
        assert_eq!(sniff(ONNX), None);
        assert_eq!(sniff(TEXT), None);
        assert_eq!(sniff(b""), None);
        assert_eq!(sniff(b"\x89PN"), None, "a truncated signature is not a PNG");
        assert_eq!(
            sniff(b"RIFF\x24\x00\x00\x00WAVEfmt "),
            None,
            "RIFF alone is not WebP"
        );
        assert_eq!(
            sniff(b"\x00\x00\x00\x18ftypisom\x00\x00\x02\x00isom"),
            None,
            "an MP4 is ISO BMFF but not HEIF"
        );
        assert_eq!(
            sniff(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3"),
            None,
            "a PDF is not an image"
        );
    }

    #[test]
    fn mime_parsing_accepts_aliases_and_parameters() {
        assert_eq!(ImageKind::from_mime("image/jpeg"), Some(ImageKind::Jpeg));
        assert_eq!(ImageKind::from_mime("image/jpg"), Some(ImageKind::Jpeg));
        assert_eq!(
            ImageKind::from_mime("IMAGE/PNG; charset=binary"),
            Some(ImageKind::Png)
        );
        assert_eq!(ImageKind::from_mime("image/heif"), Some(ImageKind::Heic));
        assert_eq!(ImageKind::from_mime("application/octet-stream"), None);
        assert_eq!(ImageKind::from_mime("image/svg+xml"), None, "SVG is script");
        assert_eq!(ImageKind::from_mime("image/avif"), None);
        assert_eq!(ImageKind::from_mime(""), None);
    }

    #[test]
    fn check_needs_header_and_bytes_to_agree() {
        assert_eq!(check_avatar(Some("image/png"), PNG), Ok(ImageKind::Png));
        assert_eq!(
            check_avatar(Some("image/jpeg; q=1"), JPEG),
            Ok(ImageKind::Jpeg)
        );
        assert_eq!(check_avatar(Some("image/heif"), HEIC), Ok(ImageKind::Heic));

        // The report: an .onnx file. Whatever the header says, the bytes are not a picture.
        assert_eq!(
            check_avatar(Some("application/octet-stream"), ONNX),
            Err(NotAnImage::BadMime("application/octet-stream".into()))
        );
        assert_eq!(
            check_avatar(Some("image/png"), ONNX),
            Err(NotAnImage::BadBytes),
            "a renamed .onnx with an image header is still refused"
        );
        assert_eq!(check_avatar(None, PNG), Err(NotAnImage::MissingMime));
        assert_eq!(check_avatar(Some("  "), PNG), Err(NotAnImage::MissingMime));
        assert_eq!(
            check_avatar(Some("image/png"), JPEG),
            Err(NotAnImage::Mismatch {
                header: ImageKind::Png,
                bytes: ImageKind::Jpeg
            })
        );
        assert_eq!(
            check_avatar(Some("text/html"), b"<svg onload=alert(1)>"),
            Err(NotAnImage::BadMime("text/html".into()))
        );
    }

    #[test]
    fn messages_are_plain() {
        let msg = NotAnImage::BadBytes.to_string();
        assert!(msg.contains("not an image"), "{msg}");
        assert!(msg.contains("PNG, JPEG, HEIC, GIF or WebP"), "{msg}");
        let msg = NotAnImage::BadMime("application/octet-stream".into()).to_string();
        assert!(
            msg.starts_with("application/octet-stream is not an image"),
            "{msg}"
        );
        let msg = NotAnImage::Mismatch {
            header: ImageKind::Png,
            bytes: ImageKind::Jpeg,
        }
        .to_string();
        assert_eq!(
            msg,
            "The file is JPEG but was sent as PNG; pictures must be PNG, JPEG, HEIC, GIF or WebP"
        );
    }

    /// A PNG's IHDR states the size and the decoder reserves the whole frame buffer from it
    /// before reading a single pixel, so a few hundred bytes can ask for gigabytes. The relay
    /// refuses anything over `MAX_DECODE_SIDE` from the header, and still decodes real pictures.
    #[test]
    fn a_picture_that_claims_to_be_huge_is_refused_before_it_is_allocated() {
        let bomb = png_claiming(30_000, 30_000);
        assert!(bomb.len() < 200, "{} bytes on the wire", bomb.len());
        assert_eq!(sniff(&bomb), Some(ImageKind::Png));
        assert_eq!(decode(&bomb).unwrap_err(), TOO_LARGE);
        assert_eq!(
            decoder(&bomb).err().as_deref(),
            Some(TOO_LARGE),
            "the EXIF path is bounded too"
        );
        assert_eq!(decode(&png_claiming(1, 60_000)).unwrap_err(), TOO_LARGE);

        // A real picture of a sane size still decodes.
        let mut real = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(64, 48)
            .write_to(&mut real, image::ImageFormat::Png)
            .unwrap();
        let real = real.into_inner();
        assert_eq!(decode(&real).unwrap().width(), 64);
        assert!(decoder(&real).is_ok());
    }
}
