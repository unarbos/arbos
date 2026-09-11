//! Images the model sees as images.
//!
//! An image is a file on disk. Events and tool results carry its path; the
//! projection loads the bytes and sends a `data:` URL as an `image_url`
//! content part. Nothing base64 is ever written to the transcript.

use std::path::Path;

/// Providers reject bigger payloads (OpenAI: 20 MB).
pub const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

/// Flat token cost per image for the working-set estimate. A 1024×1024
/// image is ~765 tokens on OpenAI high detail and ~1600 on Anthropic;
/// 1200 sits between and errs high, which is the safe side for a budget.
pub const IMAGE_TOKENS: u64 = 1200;

/// How many of the most recent images stay in the projection. Older ones
/// become a one-line stub the model can act on (`read` it again).
pub const KEEP_IMAGES: usize = 8;

/// A loaded image ready for the wire.
#[derive(Debug, Clone)]
pub struct ImagePart {
    pub mime: &'static str,
    pub b64: String,
}

impl ImagePart {
    pub fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.mime, self.b64)
    }
}

/// Extensions we hand to the model as pixels. SVG is XML and stays text.
pub fn is_image_path(path: &Path) -> bool {
    matches!(ext(path).as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp")
}

fn ext(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// MIME from the first bytes only. All four formats have a fixed
/// signature, so a miss means the file is not an image whatever its name.
/// A wrong MIME on a data URL is a 400 from the provider.
pub fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

/// Why a path did not become an image part. Shown to the model as text so
/// it can do something about it.
#[derive(Debug)]
pub enum LoadError {
    Missing,
    TooLarge(u64),
    NotImage,
    Io(String),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Missing => f.write_str("file not found"),
            LoadError::TooLarge(n) => write!(
                f,
                "{} MB, over the {} MB limit",
                n / (1024 * 1024),
                MAX_IMAGE_BYTES / (1024 * 1024)
            ),
            LoadError::NotImage => f.write_str("not a png/jpeg/gif/webp"),
            LoadError::Io(e) => f.write_str(e),
        }
    }
}

/// Read and encode one image. Cheap enough to do on every projection: the
/// OS caches the file and base64 of a few MB is sub-millisecond.
pub fn load(path: &Path) -> Result<ImagePart, LoadError> {
    let meta = std::fs::metadata(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            LoadError::Missing
        } else {
            LoadError::Io(e.to_string())
        }
    })?;
    if meta.len() > MAX_IMAGE_BYTES {
        return Err(LoadError::TooLarge(meta.len()));
    }
    let bytes = std::fs::read(path).map_err(|e| LoadError::Io(e.to_string()))?;
    let mime = sniff_mime(&bytes).ok_or(LoadError::NotImage)?;
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes);
    Ok(ImagePart { mime, b64 })
}

/// Width×height when the header makes it cheap (PNG, GIF). For the tool
/// body line so the model knows the scale before it looks.
pub fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() >= 24 && bytes.starts_with(b"\x89PNG") {
        let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        return Some((w, h));
    }
    if bytes.len() >= 10 && bytes.starts_with(b"GIF8") {
        let w = u16::from_le_bytes([bytes[6], bytes[7]]) as u32;
        let h = u16::from_le_bytes([bytes[8], bytes[9]]) as u32;
        return Some((w, h));
    }
    None
}
