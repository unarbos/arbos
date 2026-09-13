use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use bezel::gpui::{Image, ImageFormat as PreviewFormat};
use image::{ImageFormat, ImageReader, Limits};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs::File,
    io::{Cursor, Read},
    path::PathBuf,
    sync::{Arc, OnceLock},
};

const MAX_BYTES: usize = 24 * 1024 * 1024;
const MAX_ATTACHMENTS: usize = 16;
const MAX_TOTAL_BYTES: usize = 20 * 1024 * 1024;

#[derive(Clone)]
pub struct Attachment {
    pub path: PathBuf,
    pub preview: Option<Arc<Image>>,
    pub history_image: Option<MessageImage>,
    image: Option<Arc<Value>>,
    bytes: usize,
}

impl Attachment {
    pub fn load(path: PathBuf) -> Result<Self> {
        let file = File::open(&path).with_context(|| format!("Cannot read {}", path.display()))?;
        if !file.metadata()?.is_file() {
            bail!("Not a file: {}", path.display());
        }
        let mut header = [0; 32];
        let mut file = file;
        let count = file.read(&mut header)?;
        let format = image::guess_format(&header[..count]).ok();
        if format.is_none() {
            let extension = path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            if matches!(
                extension.as_str(),
                "png"
                    | "jpg"
                    | "jpeg"
                    | "gif"
                    | "webp"
                    | "heic"
                    | "heif"
                    | "svg"
                    | "tif"
                    | "tiff"
                    | "bmp"
                    | "avif"
            ) {
                bail!(
                    "Invalid or unsupported image: {}. Use PNG, JPEG, GIF or WebP.",
                    path.display()
                );
            }
            return Ok(Self {
                path,
                preview: None,
                history_image: None,
                image: None,
                bytes: 0,
            });
        }
        let format = format.unwrap();
        let mime = match format {
            ImageFormat::Png => "image/png",
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Gif => "image/gif",
            ImageFormat::WebP => "image/webp",
            _ => bail!(
                "Unsupported image: {}. Use PNG, JPEG, GIF or WebP.",
                path.display()
            ),
        };
        let mut bytes = header[..count].to_vec();
        file.take((MAX_BYTES + 1 - count) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_BYTES {
            bail!("Image exceeds 24 MiB: {}", path.display());
        }
        let mut reader = ImageReader::with_format(Cursor::new(&bytes), format);
        let mut limits = Limits::default();
        limits.max_image_width = Some(16384);
        limits.max_image_height = Some(16384);
        limits.max_alloc = Some(128 * 1024 * 1024);
        reader.limits(limits);
        let decoded = reader
            .decode()
            .with_context(|| format!("Invalid image: {}", path.display()))?;
        let mut thumbnail = Cursor::new(Vec::new());
        decoded
            .thumbnail(96, 96)
            .write_to(&mut thumbnail, ImageFormat::Png)?;
        Ok(Self {
            path,
            preview: Some(Arc::new(Image::from_bytes(
                PreviewFormat::Png,
                thumbnail.into_inner(),
            ))),
            history_image: Some(MessageImage::from_decoded(&decoded)?),
            image: Some(Arc::new(json!({"type":"image", "image":{
                "data": STANDARD.encode(&bytes), "mimeType": mime,
            }}))),
            bytes: bytes.len(),
        })
    }

    pub fn is_image(&self) -> bool {
        self.image.is_some()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct MessageImage {
    data: String,
    width: u32,
    height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip)]
    preview: OnceLock<Option<Arc<Image>>>,
}

impl MessageImage {
    fn from_decoded(decoded: &image::DynamicImage) -> Result<Self> {
        let thumbnail = decoded.thumbnail(decoded.width().min(880), decoded.height().min(640));
        let mut bytes = Cursor::new(Vec::new());
        thumbnail.write_to(&mut bytes, ImageFormat::Png)?;
        Ok(Self {
            data: STANDARD.encode(bytes.into_inner()),
            width: thumbnail.width(),
            height: thumbnail.height(),
            name: None,
            preview: OnceLock::new(),
        })
    }

    pub fn from_part(part: &Value) -> Result<Self> {
        let data = part["image"]["data"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Image data unavailable"))?;
        if data.len() > MAX_BYTES.div_ceil(3) * 4 {
            bail!("Image exceeds 24 MiB");
        }
        let bytes = STANDARD.decode(data)?;
        let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
        if !matches!(
            reader.format(),
            Some(ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP)
        ) {
            bail!("Unsupported image format");
        }
        let mut limits = Limits::default();
        limits.max_image_width = Some(16384);
        limits.max_image_height = Some(16384);
        limits.max_alloc = Some(128 * 1024 * 1024);
        reader.limits(limits);
        Self::from_decoded(&reader.decode()?)
    }

    pub fn preview(&self) -> Option<Arc<Image>> {
        self.preview
            .get_or_init(|| {
                STANDARD
                    .decode(&self.data)
                    .ok()
                    .map(|bytes| Arc::new(Image::from_bytes(PreviewFormat::Png, bytes)))
            })
            .clone()
    }

    pub fn display_size(&self) -> (f32, f32) {
        let scale = (412. / self.width.max(1) as f32)
            .min(320. / self.height.max(1) as f32)
            .min(1.);
        (self.width as f32 * scale, self.height as f32 * scale)
    }

    pub fn label(&self) -> &str {
        self.name
            .as_deref()
            .filter(|name| !name.is_empty())
            .unwrap_or("image")
    }
}

/// A non-image file that sat on the composer card. Path is enough to
/// redraw the chip after send; the kernel already got the path in the
/// prompt text.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageFile {
    pub name: String,
    pub path: String,
}

impl MessageFile {
    pub fn from_path(path: impl AsRef<std::path::Path>) -> Self {
        let path = path.as_ref();
        Self {
            name: file_name(path),
            path: path.display().to_string(),
        }
    }
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file")
        .to_string()
}

/// Pull `Attached file "name": \`path\`` paragraphs out of a user
/// message so history can draw chips instead of the markdown dump.
fn take_attached_files(text: &str) -> (String, Vec<MessageFile>) {
    let mut files = Vec::new();
    let mut kept = Vec::new();
    for piece in text.split("\n\n") {
        if let Some(file) = attached_file_line(piece) {
            if !files
                .iter()
                .any(|held: &MessageFile| held.path == file.path)
            {
                files.push(file);
            }
        } else if !piece.is_empty() {
            kept.push(piece);
        }
    }
    (kept.join("\n\n"), files)
}

fn attached_file_line(s: &str) -> Option<MessageFile> {
    let s = s.trim();
    let rest = s.strip_prefix("Attached file \"")?;
    let (name, rest) = rest.split_once("\": `")?;
    let path = rest.strip_suffix('`')?;
    if name.is_empty() || path.is_empty() {
        return None;
    }
    Some(MessageFile {
        name: name.to_string(),
        path: path.to_string(),
    })
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(from = "StoredMessage")]
pub struct UserMessage {
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<MessageImage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<MessageFile>,
    /// Unix millis when the prompt was sent (or, on replay, the transcript
    /// line's time). The relative time under the answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<i64>,
    /// The user's thumbs on this turn's answer: 1 up, -1 down.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<i8>,
}

impl UserMessage {
    fn lift_files(&mut self) {
        let (text, files) = take_attached_files(&self.text);
        if files.is_empty() {
            return;
        }
        self.text = text;
        for file in files {
            if !self.files.iter().any(|held| held.path == file.path) {
                self.files.push(file);
            }
        }
    }

    pub fn add_file_path(&mut self, path: &str) {
        let file = MessageFile::from_path(std::path::Path::new(path));
        if !self.files.iter().any(|held| held.path == file.path) {
            self.files.push(file);
        }
    }

    pub fn has_attachments(&self) -> bool {
        !self.images.is_empty() || !self.files.is_empty()
    }

    /// Rebuild a sendable prompt from a transcript card: the words, plus
    /// any files whose paths are still on disk.
    pub fn to_prompt(&self) -> Prompt {
        let attachments = self
            .files
            .iter()
            .filter_map(|file| Attachment::load(PathBuf::from(&file.path)).ok())
            .collect();
        Prompt::compose(&self.text, attachments)
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StoredMessage {
    Text(String),
    Images {
        text: String,
        #[serde(default)]
        images: Vec<MessageImage>,
        #[serde(default)]
        files: Vec<MessageFile>,
    },
}

impl From<StoredMessage> for UserMessage {
    fn from(message: StoredMessage) -> Self {
        let mut out = match message {
            StoredMessage::Text(text) => Self {
                text,
                images: Vec::new(),
                files: Vec::new(),
                sent_at: None,
                feedback: None,
            },
            StoredMessage::Images {
                text,
                images,
                files,
            } => Self {
                text,
                images,
                files,
                sent_at: None,
                feedback: None,
            },
        };
        out.lift_files();
        out
    }
}

impl From<String> for UserMessage {
    fn from(text: String) -> Self {
        let mut message = Self {
            text,
            images: Vec::new(),
            files: Vec::new(),
            sent_at: Some(arbos_core::now_ms()),
            feedback: None,
        };
        message.lift_files();
        message
    }
}

#[derive(Clone, Default)]
pub struct AttachmentTray {
    pub items: Vec<Attachment>,
    pub loading: usize,
    pub error: Option<String>,
}

impl AttachmentTray {
    pub fn insert(&mut self, attachment: Attachment) -> Result<()> {
        if self.items.iter().any(|held| held.path == attachment.path) {
            return Ok(());
        }
        if self.items.len() >= MAX_ATTACHMENTS {
            bail!("Attach at most 16 files per message");
        }
        if self.items.iter().map(|a| a.bytes).sum::<usize>() + attachment.bytes > MAX_TOTAL_BYTES {
            bail!("Images in one message must total at most 20 MiB");
        }
        self.items.push(attachment);
        Ok(())
    }
}

#[derive(Default)]
pub struct AttachmentDrafts {
    trays: HashMap<u64, AttachmentTray>,
}

impl AttachmentDrafts {
    pub fn get(&self, session: Option<u64>) -> Option<&AttachmentTray> {
        session.and_then(|id| self.trays.get(&id))
    }

    pub fn get_mut(&mut self, session: u64) -> &mut AttachmentTray {
        self.trays.entry(session).or_default()
    }
}

#[derive(Clone, Default)]
pub struct Prompt {
    pub text: String,
    pub attachments: Vec<Attachment>,
}

impl From<String> for Prompt {
    fn from(text: String) -> Self {
        Self {
            text,
            attachments: Vec::new(),
        }
    }
}

impl Prompt {
    pub fn compose(text: &str, attachments: Vec<Attachment>) -> Self {
        let lines = attachments.iter().filter(|a| a.image.is_none()).map(|a| {
            format!(
                "Attached file \"{}\": `{}`",
                file_name(&a.path),
                a.path.display()
            )
        });
        let text = text.trim();
        let pieces: Vec<String> = if text.starts_with('/') {
            std::iter::once(text.to_owned()).chain(lines).collect()
        } else {
            lines.chain(std::iter::once(text.to_owned())).collect()
        };
        Self {
            text: pieces
                .into_iter()
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n"),
            attachments,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.attachments.is_empty()
    }

    /// One prompt from several: texts stacked, attachments kept.
    pub fn join(parts: impl IntoIterator<Item = Self>) -> Self {
        let mut text = Vec::new();
        let mut attachments = Vec::new();
        for part in parts {
            if !part.text.trim().is_empty() {
                text.push(part.text);
            }
            attachments.extend(part.attachments);
        }
        Self {
            text: text.join("\n\n"),
            attachments,
        }
    }

    pub fn wire(&self) -> Value {
        let parts: Vec<&Value> = self
            .attachments
            .iter()
            .filter_map(|a| a.image.as_deref())
            .collect();
        if parts.is_empty() {
            json!({"text":self.text})
        } else {
            json!({"text":self.text,"parts":parts})
        }
    }

    pub fn message(&self) -> UserMessage {
        let mut message = UserMessage::from(self.text.clone());
        message.images = self
            .attachments
            .iter()
            .filter_map(|a| {
                a.history_image.clone().map(|mut image| {
                    image.name = Some(file_name(&a.path));
                    image
                })
            })
            .collect();
        let files: Vec<MessageFile> = self
            .attachments
            .iter()
            .filter(|a| a.image.is_none())
            .map(|a| MessageFile::from_path(&a.path))
            .collect();
        if !files.is_empty() {
            message.files = files;
        }
        message
    }

    pub fn display(&self) -> String {
        let mut pieces = vec![self.text.clone()];
        pieces.extend(
            self.attachments
                .iter()
                .filter(|a| a.image.is_some())
                .map(|a| {
                    format!(
                        "[Image: {}]",
                        a.path.file_name().unwrap_or_default().to_string_lossy()
                    )
                }),
        );
        pieces
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);
    impl Fixture {
        fn new(bytes: &[u8], extension: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "arbos-image-{}-{}.{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
                extension
            ));
            std::fs::write(&path, bytes).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    fn png() -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 2)
            .write_to(&mut out, ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn image_bytes_are_snapshot_not_local_path() {
        let bytes = png();
        let file = Fixture::new(&bytes, "jpg");
        let attachment = Attachment::load(file.0.clone()).unwrap();
        assert!(attachment.preview.is_some());
        drop(file);
        let prompt = Prompt::compose("describe this", vec![attachment]);
        let wire = prompt.wire();
        assert_eq!(wire["text"], "describe this");
        assert_eq!(wire["parts"][0]["type"], "image");
        assert_eq!(wire["parts"][0]["image"]["mimeType"], "image/png");
        assert_eq!(
            STANDARD
                .decode(wire["parts"][0]["image"]["data"].as_str().unwrap())
                .unwrap(),
            bytes
        );
    }

    #[test]
    fn inline_message_survives_archive_and_source_deletion() {
        use crate::model::session::ChatItem;
        let file = Fixture::new(&png(), "png");
        let prompt = Prompt::compose(
            "describe this",
            vec![Attachment::load(file.0.clone()).unwrap()],
        );
        drop(file);
        let item = ChatItem::User(prompt.message());
        let json = serde_json::to_string(&item).unwrap();
        assert!(!json.contains("[Image:"));
        let ChatItem::User(message) = serde_json::from_str::<ChatItem>(&json).unwrap() else {
            panic!("expected user")
        };
        assert_eq!(message.text, "describe this");
        assert_eq!(message.images.len(), 1);
        assert!(message.images[0].preview().is_some());
        let bytes = STANDARD.decode(&message.images[0].data).unwrap();
        assert!(image::load_from_memory(&bytes).is_ok());
        assert!(Arc::ptr_eq(
            &message.images[0].preview().unwrap(),
            &message.images[0].preview().unwrap()
        ));
    }

    #[test]
    fn legacy_text_messages_still_restore() {
        use crate::model::session::ChatItem;
        let ChatItem::User(message) =
            serde_json::from_str::<ChatItem>(r#"{"User":"old message"}"#).unwrap()
        else {
            panic!("expected user")
        };
        assert_eq!(message.text, "old message");
        assert!(message.images.is_empty());
        let item =
            serde_json::from_str::<ChatItem>(r#"{"From":{"who":"Sam","text":"hello"}}"#).unwrap();
        assert!(matches!(item, ChatItem::From { images, .. } if images.is_empty()));
    }

    #[test]
    fn preview_fits_chat_without_distorting_image() {
        for (width, height) in [(1600, 900), (900, 1600), (20, 20)] {
            let image =
                MessageImage::from_decoded(&image::DynamicImage::new_rgb8(width, height)).unwrap();
            let (w, h) = image.display_size();
            assert!(w <= 412. && h <= 320.);
            assert!((w / h - width as f32 / height as f32).abs() < 0.01);
        }
        assert!(
            MessageImage::from_part(&json!({"type":"image","image":{"data":"broken"}})).is_err()
        );
    }

    #[test]
    fn image_only_prompt_survives_queue_clone() {
        let file = Fixture::new(&png(), "png");
        let prompt = Prompt::compose("", vec![Attachment::load(file.0.clone()).unwrap()]);
        assert!(!prompt.is_empty());
        let mut queue = std::collections::VecDeque::from([prompt.clone()]);
        assert_eq!(queue.pop_front().unwrap().wire(), prompt.wire());
        assert!(prompt.display().contains("[Image:"));
    }

    #[test]
    fn ordinary_files_keep_slash_commands_first() {
        let file = Fixture::new(b"hello", "txt");
        let prompt = Prompt::compose(
            "/review please",
            vec![Attachment::load(file.0.clone()).unwrap()],
        );
        assert!(prompt.text.starts_with("/review please\n\nAttached file"));
        assert!(prompt.wire().get("parts").is_none());
        assert!(Prompt::default().is_empty());
    }

    #[test]
    fn invalid_and_oversized_images_are_rejected() {
        let bad = Fixture::new(b"not an image", "png");
        assert!(Attachment::load(bad.0.clone()).is_err());
        let mut bytes = png();
        bytes.resize(MAX_BYTES + 1, 0);
        let large = Fixture::new(&bytes, "png");
        assert!(
            Attachment::load(large.0.clone())
                .err()
                .unwrap()
                .to_string()
                .contains("24 MiB")
        );
    }

    #[test]
    fn drafts_are_isolated_and_duplicate_files_are_not_added() {
        let file = Fixture::new(&png(), "png");
        let attachment = Attachment::load(file.0.clone()).unwrap();
        let mut drafts = AttachmentDrafts::default();
        drafts.get_mut(1).insert(attachment.clone()).unwrap();
        drafts.get_mut(1).insert(attachment).unwrap();
        assert_eq!(drafts.get(Some(1)).unwrap().items.len(), 1);
        assert!(drafts.get(Some(2)).is_none());
        drafts.get_mut(2).loading = 1;
        assert_eq!(drafts.get(Some(1)).unwrap().loading, 0);
        drafts.get_mut(1).items.remove(0);
        assert!(drafts.get(Some(1)).unwrap().items.is_empty());
    }

    #[test]
    fn all_supported_formats_use_their_real_mime_type() {
        for (format, mime) in [
            (ImageFormat::Png, "image/png"),
            (ImageFormat::Jpeg, "image/jpeg"),
            (ImageFormat::Gif, "image/gif"),
            (ImageFormat::WebP, "image/webp"),
        ] {
            let mut bytes = Cursor::new(Vec::new());
            image::DynamicImage::new_rgb8(2, 2)
                .write_to(&mut bytes, format)
                .unwrap();
            let file = Fixture::new(bytes.get_ref(), "bin");
            let prompt = Prompt::compose("", vec![Attachment::load(file.0.clone()).unwrap()]);
            assert_eq!(prompt.wire()["parts"][0]["image"]["mimeType"], mime);
        }
    }

    #[test]
    fn aggregate_bytes_and_count_are_bounded() {
        let file = Fixture::new(&png(), "png");
        let mut attachment = Attachment::load(file.0.clone()).unwrap();
        attachment.bytes = MAX_TOTAL_BYTES;
        let mut tray = AttachmentTray::default();
        tray.insert(attachment.clone()).unwrap();
        attachment.path = "second.png".into();
        assert!(tray.insert(attachment.clone()).is_err());
        assert_eq!(tray.items.len(), 1);
        tray.items.clear();
        attachment.bytes = 0;
        for i in 0..MAX_ATTACHMENTS {
            attachment.path = format!("{i}.png").into();
            tray.insert(attachment.clone()).unwrap();
        }
        attachment.path = "extra.png".into();
        assert!(tray.insert(attachment).is_err());
    }
}
