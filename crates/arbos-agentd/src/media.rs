//! Media and document delivery (F10, D38): `deliver_file` uploads a file the agent produced to the
//! relay blob store and appends an `attachment` part to the current assistant message;
//! `generate_image` asks the relay's image proxy (the OpenAI key stays on the relay, D2), saves
//! the PNG under the project's `out/` folder and delivers it the same way.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine;
use serde_json::{json, Value};

use arbos_core::ids::{now_ms, ulid};

use crate::agent::Agent;
use crate::config::Config;
use crate::mcp::{err_result, text_result};

/// The relay accepts 64 MiB bodies; stay under it.
const MAX_BYTES: u64 = 60 * 1024 * 1024;
const DEFAULT_IMAGE_MODEL: &str = "gpt-image-1";

/// One line in the workspace's `.arbos/audit.jsonl` (the file the pi extension and the approval gate write).
pub(crate) fn audit(agent: &Agent, kind: &str, fields: Value) {
    let mut entry = json!({ "kind": kind, "at": now_ms() });
    if let (Some(e), Some(f)) = (entry.as_object_mut(), fields.as_object()) {
        for (k, v) in f {
            e.insert(k.clone(), v.clone());
        }
    }
    let path = agent.cfg.audit_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{entry}");
    }
}

pub fn tool_specs() -> Vec<Value> {
    vec![
        json!({ "name": "deliver_file", "description": "Put a file or picture into THIS chat as a real message — the same in a 1:1, a group, a server channel or a DM, shown to every participant on Mac and iPhone: an image appears inline, any other file as a card with a download button. This is the only way to show the user a file or a picture; never paste a path, a markdown image or a link instead. Give `path` (a file you made, under your project's out/ folder) or `url` (a public image or file to fetch and post). Returns `posted: true` with the message id of the row in the chat; an error means the file was NOT posted — then say so plainly instead of claiming it was.",
                "inputSchema": { "type": "object", "properties": { "path": { "type": "string", "description": "Absolute path, or relative to your project folder" }, "url": { "type": "string", "description": "https URL of an image or file to fetch and post (instead of path)" }, "title": { "type": "string", "description": "Short caption shown on the card (defaults to the file name)" }, "caption": { "type": "string", "description": "One sentence shown with the file in the same message (optional)" }, "kind": { "type": "string", "enum": ["image", "video", "slides", "doc", "sheet", "pdf", "audio", "file"] }, "reply_to": { "type": "string", "description": "Message id this answers (default: the message that started this turn)" }, "session_id": { "type": "string", "description": "Subagents: your ARBOS_SESSION_ID" } } } }),
        json!({ "name": "generate_image", "description": "Generate an image from a text prompt (through the relay; no key needed here). Saves out/<id>.png in your project folder and posts it inline in this chat (any chat type) the way deliver_file does. One call per image.",
                "inputSchema": { "type": "object", "properties": { "prompt": { "type": "string" }, "size": { "type": "string", "enum": ["1024x1024", "1024x1536", "1536x1024"], "description": "default 1024x1024" }, "quality": { "type": "string", "enum": ["low", "medium", "high"], "description": "default medium" }, "title": { "type": "string" }, "session_id": { "type": "string" } }, "required": ["prompt"] } }),
    ]
}

/// `<project folder>/out`: where the agent writes files meant for the user.
pub fn out_dir(cfg: &Config) -> PathBuf {
    cfg.work_dir.join("out")
}

/// Python interpreter that has python-pptx/docx/openpyxl/Pillow/reportlab: the venv named by
/// `ARBOS_MEDIA_VENV` (the image sets `/opt/prime-venv`; `scripts/box-media-deps.sh` for a
/// process box), else plain `python3`.
pub fn media_python() -> String {
    match std::env::var("ARBOS_MEDIA_VENV") {
        Ok(v) if !v.trim().is_empty() => format!("{}/bin/python", v.trim_end_matches('/')),
        _ => "python3".to_owned(),
    }
}

/// "## Producing files" for the main-agent prompt and the subagent pack.
pub fn prompt_section(cfg: &Config) -> String {
    format!(
        "## Producing files and pictures\n\nWhen the user wants a document, slides, a spreadsheet, a PDF, an image or a video, make the file and put it in the chat with `deliver_file(path, title?, caption?)`: it lands as a real message in THIS chat — a 1:1, a group, a server channel or a DM alike — an image inline, any other file as a card with a download button, visible to every participant. A picture or file that already exists on the web goes in with `deliver_file(url=…)`. This is the only way to show a file or picture; a path, a markdown image or a link in a message is not. Write files under `{out}/` with a clear name (`report.pptx`, `budget.xlsx`). Python at `{py}` has python-pptx, python-docx, openpyxl, Pillow and reportlab; `ffmpeg` is on PATH for video and audio. For a picture from a description use `generate_image(prompt, size?)`; it saves and posts the PNG itself. A file is in the chat only when the tool returned `posted: true` with a message id; if it returned an error, say the file could not be posted and why — never say you posted or attached something otherwise. After posting, say in one line what the file is; never paste its contents or its path unless asked.\n\n",
        out = out_dir(cfg).display(),
        py = media_python()
    )
}

/// Attachment kind from the extension (or the MIME type when the extension says nothing).
pub fn kind_for(name: &str, mime: &str) -> &'static str {
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "heic" | "tiff" => "image",
        // Neither client previews SVG: an "image" that never renders was Jacob's blank card
        // (2026-10-26); as a file card it opens and downloads.
        "svg" => "file",
        "mp4" | "mov" | "webm" | "mkv" | "avi" => "video",
        "pptx" | "ppt" | "key" | "odp" => "slides",
        "docx" | "doc" | "odt" | "rtf" | "md" | "txt" | "html" => "doc",
        "xlsx" | "xls" | "csv" | "ods" | "tsv" => "sheet",
        "pdf" => "pdf",
        "mp3" | "wav" | "m4a" | "ogg" | "flac" | "aac" => "audio",
        _ => {
            if mime.starts_with("image/") {
                "image"
            } else if mime.starts_with("video/") {
                "video"
            } else if mime.starts_with("audio/") {
                "audio"
            } else if mime == "application/pdf" {
                "pdf"
            } else {
                "file"
            }
        }
    }
}

pub fn mime_for(name: &str) -> &'static str {
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "tiff" => "image/tiff",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "ppt" => "application/vnd.ms-powerpoint",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "doc" => "application/msword",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xls" => "application/vnd.ms-excel",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "pdf" => "application/pdf",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "md" => "text/markdown",
        "txt" => "text/plain",
        "html" => "text/html",
        "json" => "application/json",
        "zip" => "application/zip",
        "gz" | "tgz" => "application/gzip",
        _ => "application/octet-stream",
    }
}

const KINDS: [&str; 8] = [
    "image", "video", "slides", "doc", "sheet", "pdf", "audio", "file",
];

/// Where the file lands: the chat and the ask it answers. A worker (`session_id`) posts into the
/// chat its job or hand-off belongs to, under that ask; the main turn posts into its own chat.
/// Never the worker's own progress row: that row is `hidden`/`worker` and both clients skip it,
/// which is how three logo deliveries in Jacob's group "never appeared" (2026-10-26).
#[derive(Debug, Clone)]
pub struct Target {
    pub chat_id: String,
    /// The user message the file answers (`inReplyTo`), when known.
    pub ask_id: Option<String>,
    /// The main turn is the poster (counts as its result).
    pub main_turn: bool,
}

pub(crate) async fn target(a: &Agent, session_id: Option<&str>) -> Option<Target> {
    let worker_target = |chat_id: &str, row_id: &str, host_doc_ask: Option<String>| Target {
        chat_id: chat_id.to_owned(),
        ask_id: host_doc_ask.or_else(|| Some(row_id.to_owned())),
        main_turn: false,
    };
    if let Some(sid) = session_id.filter(|s| !s.is_empty()) {
        if let Some((chat_id, row_id)) = a.subagent_msgs.lock().await.get(sid).cloned() {
            let ask = row_in_reply_to(a, &chat_id, &row_id).await;
            return Some(worker_target(&chat_id, &row_id, ask));
        }
    }
    {
        let tl = a.turn.lock().await;
        if let Some(t) = tl.as_ref() {
            return Some(Target {
                chat_id: t.chat_id.clone(),
                ask_id: Some(t.user_msg_id.clone()),
                main_turn: true,
            });
        }
    }
    let newest = a
        .subagent_msgs
        .lock()
        .await
        .values()
        .max_by_key(|(_, msg)| msg.clone())
        .cloned();
    match newest {
        Some((chat_id, row_id)) => {
            let ask = row_in_reply_to(a, &chat_id, &row_id).await;
            Some(worker_target(&chat_id, &row_id, ask))
        }
        None => None,
    }
}

/// `inReplyTo` of a row in `chat_id` (the ask a worker row answers), when the chat is open.
async fn row_in_reply_to(a: &Agent, chat_id: &str, row_id: &str) -> Option<String> {
    let host = a.sync.host(chat_id).await?;
    let h = host.lock().await;
    h.doc
        .message(row_id)?
        .rest
        .get("inReplyTo")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

pub(crate) async fn upload(
    a: &Arc<Agent>,
    chat_id: &str,
    bytes: Vec<u8>,
    mime: &str,
) -> anyhow::Result<String> {
    use sha2::Digest;
    let key = hex::encode(sha2::Sha256::digest(&bytes));
    // Same slot clients read: people chats live under `people:{chat}`, not the project id.
    let slot = a.blob_slot(chat_id);
    let r = a
        .http_client()
        .put(format!("{}/blob/{}/{}", a.cfg.relay_url, slot, key))
        .bearer_auth(&a.cfg.box_token)
        .header("content-type", mime)
        .body(bytes)
        .send()
        .await?;
    if !r.status().is_success() {
        anyhow::bail!("relay refused the upload: HTTP {}", r.status());
    }
    Ok(key)
}

/// Fetch `url` (https, ≤ `MAX_BYTES`) for `deliver_file(url=…)`: returns the bytes, the MIME type
/// the server named and a file name (the URL's last path segment, with an extension that fits
/// the MIME type when the URL has none).
async fn fetch_url(a: &Arc<Agent>, url: &str) -> anyhow::Result<(Vec<u8>, String, String)> {
    let parsed = reqwest::Url::parse(url).map_err(|e| anyhow::anyhow!("{url}: {e}"))?;
    if parsed.scheme() != "https" && parsed.scheme() != "http" {
        anyhow::bail!("{url}: only http(s) URLs can be fetched");
    }
    let r = a
        .http_client()
        .get(parsed.clone())
        .header("user-agent", "Mozilla/5.0 (compatible; Arbos agent)")
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("{url}: {e}"))?;
    if !r.status().is_success() {
        anyhow::bail!("{url}: HTTP {}", r.status());
    }
    let mime = r
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "application/octet-stream".to_owned());
    if r.content_length().is_some_and(|n| n > MAX_BYTES) {
        anyhow::bail!("{url}: larger than {} MiB", MAX_BYTES / (1024 * 1024));
    }
    let bytes = r.bytes().await?.to_vec();
    if bytes.len() as u64 > MAX_BYTES {
        anyhow::bail!("{url}: larger than {} MiB", MAX_BYTES / (1024 * 1024));
    }
    let mut name = parsed
        .path_segments()
        .and_then(|mut s| s.next_back())
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.chars()
                .filter(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_'))
                .collect::<String>()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "download".to_owned());
    if Path::new(&name).extension().is_none() {
        if let Some(ext) = ext_for_mime(&mime) {
            name = format!("{name}.{ext}");
        }
    }
    Ok((bytes, mime, name))
}

fn ext_for_mime(mime: &str) -> Option<&'static str> {
    Some(match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "application/pdf" => "pdf",
        "video/mp4" => "mp4",
        "audio/mpeg" => "mp3",
        "text/plain" => "txt",
        "text/markdown" => "md",
        "application/json" => "json",
        _ => return None,
    })
}

/// What a successful delivery left in the chat.
#[derive(Debug, Clone)]
pub struct Posted {
    pub chat_id: String,
    pub message_id: String,
    pub attachment: Value,
}

/// Source of a delivery: a file on the box, or bytes fetched from a URL.
pub enum Source<'a> {
    Path(&'a str),
    Url(&'a str),
}

/// Resolve a delivery path to a working file of this project, or refuse it.
///
/// agentd runs as root and `deliver_file` is not gated, so the path is the only thing standing
/// between a turn and any file on the box: an absolute path, a `..` climb or a symlink would
/// otherwise post another workspace's `.arbos/secrets.env` (D70 keeps it from this workspace's
/// user, not from root) or this workspace's own vault file, store and harness configs into a
/// chat, where they land in relay message storage for every participant.
fn resolve_in_project(cfg: &Config, path: &str) -> anyhow::Result<PathBuf> {
    let p = Path::new(path);
    let asked = if p.is_absolute() {
        p.to_path_buf()
    } else {
        cfg.work_dir.join(p)
    };
    let real = asked
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("{}: {e}", asked.display()))?;
    let root = cfg
        .work_dir
        .canonicalize()
        .unwrap_or_else(|_| cfg.work_dir.clone());
    if !real.starts_with(&root) {
        anyhow::bail!(
            "{} is outside this project's folder; only files under {} can be posted",
            asked.display(),
            root.display()
        );
    }
    let state = cfg
        .state_dir
        .canonicalize()
        .unwrap_or_else(|_| cfg.state_dir.clone());
    if real.starts_with(&state) {
        anyhow::bail!(
            "{} is in the project's private .arbos folder (vault, store, harness credentials); nothing in it is ever posted into a chat",
            asked.display()
        );
    }
    Ok(real)
}

/// Resolve, read or fetch, upload, and write the attachment as its own finished message in the
/// target chat — then read it back. Returns what landed, with the message id.
pub async fn deliver(
    a: &Arc<Agent>,
    source: Source<'_>,
    title: Option<&str>,
    caption: Option<&str>,
    kind: Option<&str>,
    reply_to: Option<&str>,
    session_id: Option<&str>,
) -> anyhow::Result<Posted> {
    let Some(target) = target(a, session_id).await else {
        anyhow::bail!("no running turn or job to post the file from");
    };
    let (bytes, mime, name, origin) = match source {
        Source::Path(path) => {
            let p = resolve_in_project(&a.cfg, path)?;
            let meta = tokio::fs::metadata(&p)
                .await
                .map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))?;
            if !meta.is_file() {
                anyhow::bail!("{} is not a file", p.display());
            }
            if meta.len() > MAX_BYTES {
                anyhow::bail!(
                    "{} is {} MiB; the limit is {} MiB (compress or split it)",
                    p.display(),
                    meta.len() / (1024 * 1024),
                    MAX_BYTES / (1024 * 1024)
                );
            }
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_owned();
            let mime = mime_for(&name).to_owned();
            let bytes = tokio::fs::read(&p).await?;
            (bytes, mime, name, p.display().to_string())
        }
        Source::Url(url) => {
            let (bytes, mime, name) = fetch_url(a, url).await?;
            // Keep a copy under out/ so the Library rule holds (what was exchanged exists on
            // the box) and the user can ask for it again.
            let dir = out_dir(&a.cfg);
            let _ = tokio::fs::create_dir_all(&dir).await;
            let saved = dir.join(format!("{}-{}", ulid().to_lowercase(), name));
            let _ = tokio::fs::write(&saved, &bytes).await;
            let mime = if mime == "application/octet-stream" {
                mime_for(&name).to_owned()
            } else {
                mime
            };
            (bytes, mime, name, url.to_owned())
        }
    };
    let kind = match kind.filter(|k| KINDS.contains(k)) {
        Some("image") if mime == "image/svg+xml" => "file".to_owned(),
        Some(k) => k.to_owned(),
        None => kind_for(&name, &mime).to_owned(),
    };
    let title = title
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or(&name)
        .to_owned();
    let size = bytes.len() as u64;
    let key = upload(a, &target.chat_id, bytes, &mime).await?;
    let attachment = json!({
        "name": name, "mime": mime, "ref": key, "bytes": size, "kind": kind, "title": title,
    });
    let Some(host) = a.sync.host(&target.chat_id).await else {
        anyhow::bail!("chat {} is not open on this box", target.chat_id);
    };
    let msg_id = ulid();
    let caption = caption.map(str::trim).filter(|c| !c.is_empty());
    {
        let h = host.lock().await;
        let ask = reply_to
            .map(str::trim)
            .filter(|r| !r.is_empty() && h.doc.message(r).is_some())
            .map(str::to_owned)
            .or_else(|| target.ask_id.clone())
            .unwrap_or_default();
        let text_part = h
            .doc
            .start_assistant_message(&msg_id, &ask, &a.cfg.device_id);
        h.doc
            .set_message_fields(&msg_id, &[("turnRole", json!("attachment"))]);
        if let Some(attr) = a.people_reply_fields(&target.chat_id) {
            h.doc.set_message_fields(&msg_id, &attr.fields);
        }
        if !ask.is_empty() && a.reply_to_wanted_for(&h.doc, &target.chat_id, &ask) {
            h.doc
                .set_message_fields(&msg_id, &[("replyTo", json!(ask))]);
        }
        if let Some(c) = caption {
            // D19 backstop: the caption bypasses write_text, so redact here (#1974 review).
            h.doc
                .append_text(&msg_id, &text_part, &crate::store::redact(c));
        }
        if h.doc.add_attachment_part(&msg_id, &attachment).is_none() {
            anyhow::bail!("message {msg_id} not found in chat {}", target.chat_id);
        }
        h.doc
            .finish_message(&msg_id, arbos_core::doc::MessageStatus::Done);
        // Read it back: the row must hold the attachment, or this was not a delivery.
        let ok = h.doc.message(&msg_id).is_some_and(|m| {
            m.parts.iter().any(|p| {
                p.get("kind").and_then(Value::as_str) == Some("attachment")
                    && p.pointer("/attachment/ref").and_then(Value::as_str) == Some(key.as_str())
            })
        });
        if !ok {
            let _ = h.doc.delete_message(&msg_id);
            anyhow::bail!("the attachment row did not read back from the chat doc");
        }
    }
    a.note_posted(&target.chat_id, &msg_id).await;
    if target.main_turn {
        if let Some(t) = a.turn.lock().await.as_mut() {
            t.attachments_posted.push(msg_id.clone());
        }
    }
    let _ = a.store.t3_append(
        &target.chat_id,
        None,
        session_id,
        "attachment",
        Some("assistant"),
        &format!("posted {name} ({size} bytes, {kind}) as blob {key}"),
        Some(&json!({ "message_id": msg_id, "source": origin, "attachment": attachment, "caption": caption })),
    );
    audit(
        a,
        "file_delivered",
        json!({ "chatId": target.chat_id, "messageId": msg_id, "name": name, "bytes": size, "kind": kind, "ref": key, "source": origin, "worker": !target.main_turn }),
    );
    Ok(Posted {
        chat_id: target.chat_id,
        message_id: msg_id,
        attachment,
    })
}

fn posted_result(p: &Posted, extra: Value) -> Value {
    let mut v = json!({
        "posted": true,
        "message_id": p.message_id,
        "chat_id": p.chat_id,
        "name": p.attachment["name"],
        "kind": p.attachment["kind"],
        "bytes": p.attachment["bytes"],
        "ref": p.attachment["ref"],
        "note": if p.attachment["kind"] == "image" {
            "The picture is in the chat as its own message, inline for everyone. Say in one line what it shows; do not repeat the path."
        } else {
            "The file is in the chat as a card with a download button, visible to everyone. Say in one line what it contains; do not repeat the path."
        },
    });
    if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
        for (k, val) in e {
            o.insert(k.clone(), val.clone());
        }
    }
    v
}

pub async fn deliver_file(a: &Arc<Agent>, args: &Value) -> Value {
    let s = |k: &str| args.get(k).and_then(Value::as_str);
    let path = s("path").map(str::trim).filter(|p| !p.is_empty());
    let url = s("url").map(str::trim).filter(|u| !u.is_empty());
    let source = match (path, url) {
        (Some(p), _) => Source::Path(p),
        (None, Some(u)) => Source::Url(u),
        (None, None) => return err_result("path or url is required; nothing was posted"),
    };
    match deliver(a, source, s("title"), s("caption"), s("kind"), s("reply_to"), s("session_id")).await {
        Ok(p) => {
            let svg = p.attachment["mime"] == "image/svg+xml";
            text_result(
                posted_result(
                    &p,
                    if svg {
                        json!({ "hint": "SVG shows as a file card, not inline. For an inline picture render it to PNG first (for example with Pillow via cairosvg, or `rsvg-convert`) and post the PNG." })
                    } else {
                        json!({})
                    },
                )
                .to_string(),
            )
        }
        Err(e) => err_result(format!(
            "NOT posted — the file is not in the chat: {e}. Tell the user plainly that it could not be posted; do not say it was."
        )),
    }
}

pub async fn generate_image(a: &Arc<Agent>, args: &Value) -> Value {
    let s = |k: &str| args.get(k).and_then(Value::as_str);
    let Some(prompt) = s("prompt").map(str::trim).filter(|p| !p.is_empty()) else {
        return err_result("prompt is required");
    };
    let size = s("size").unwrap_or("1024x1024");
    let quality = s("quality").unwrap_or("medium");
    let model = std::env::var("ARBOS_IMAGE_MODEL")
        .ok()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| DEFAULT_IMAGE_MODEL.to_owned());
    let body =
        json!({ "model": model, "prompt": prompt, "size": size, "quality": quality, "n": 1 });
    let t0 = std::time::Instant::now();
    let resp = match a
        .http_client()
        .post(format!("{}/proxy/v1/images/generations", a.cfg.relay_url))
        .bearer_auth(&a.cfg.box_token)
        .header("x-arbos-project", &a.cfg.project_id)
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return err_result(format!("relay: {e}")),
    };
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let msg = v
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("image generation failed");
        return err_result(format!("HTTP {status}: {msg}"));
    }
    let mock = v
        .get("arbos_mock")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let Some(b64) = v.pointer("/data/0/b64_json").and_then(Value::as_str) else {
        return err_result("relay returned no image data");
    };
    let png = match base64::engine::general_purpose::STANDARD.decode(b64) {
        Ok(b) => b,
        Err(e) => return err_result(format!("bad image payload: {e}")),
    };
    let dir = out_dir(&a.cfg);
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        return err_result(format!("{}: {e}", dir.display()));
    }
    let path = dir.join(format!("{}.png", ulid().to_lowercase()));
    if let Err(e) = tokio::fs::write(&path, &png).await {
        return err_result(format!("{}: {e}", path.display()));
    }
    let title = s("title")
        .map(str::to_owned)
        .unwrap_or_else(|| prompt.chars().take(80).collect());
    audit(
        a,
        "image_generated",
        json!({ "prompt": prompt, "size": size, "quality": quality, "model": model, "mock": mock, "path": path.display().to_string(), "ms": t0.elapsed().as_millis() as i64, "at": now_ms() }),
    );
    match deliver(
        a,
        Source::Path(&path.display().to_string()),
        Some(&title),
        None,
        Some("image"),
        None,
        s("session_id"),
    )
    .await
    {
        Ok(p) => text_result(
            posted_result(
                &p,
                json!({ "path": path.display().to_string(), "mock": mock, "mock_note": if mock { "ARBOS_IMAGE_MOCK=1: the relay drew a labelled placeholder instead of calling the provider." } else { "" } }),
            )
            .to_string(),
        ),
        Err(e) => err_result(format!(
            "image saved to {} but NOT posted to the chat: {e}. Tell the user it could not be posted; do not say it was.",
            path.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::tests::test_agent;

    fn attachment_parts(m: &arbos_core::doc::MessageView) -> Vec<Value> {
        m.parts
            .iter()
            .filter(|p| p.get("kind").and_then(Value::as_str) == Some("attachment"))
            .cloned()
            .collect()
    }

    /// Jacob, 2026-10-26 (Affine, group chat): three logo deliveries landed on the job worker's
    /// `hidden` row, which both clients skip, while the agent said "I've posted the logo". A
    /// worker's file now lands as its own visible message under the ask, and the tool answers
    /// with that message's id — read back from the doc.
    #[tokio::test]
    async fn a_worker_delivery_is_its_own_visible_row_under_the_ask() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATATTACH0000000001";
        let host = agent.sync.open_room(chat).await.unwrap();
        let ask = ulid();
        let hidden_row = ulid();
        {
            let h = host.lock().await;
            h.doc
                .append_user_message(
                    &ask,
                    "post the logo here",
                    "dev-mac",
                    &arbos_core::doc::UserMessageOpts::default(),
                )
                .unwrap();
            h.doc
                .start_assistant_message(&hidden_row, &ask, &agent.cfg.device_id);
            h.doc.set_message_fields(
                &hidden_row,
                &[
                    ("job", json!({ "id": "j1", "title": "Reading affine.io", "kind": "code", "touches": [] })),
                    ("worker", json!(true)),
                    ("hidden", json!(true)),
                ],
            );
        }
        let sid = "01TESTWORKERSESSION0000001";
        agent
            .subagent_msgs
            .lock()
            .await
            .insert(sid.to_owned(), (chat.to_owned(), hidden_row.clone()));
        let dir = out_dir(&agent.cfg);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("affine-logo.png"), b"\x89PNG\r\n\x1a\nfake").unwrap();

        let posted = deliver(
            &agent,
            Source::Path("out/affine-logo.png"),
            Some("affine.io logo"),
            Some("The affine.io mark."),
            None,
            None,
            Some(sid),
        )
        .await
        .unwrap();
        assert_eq!(posted.chat_id, chat);
        assert_ne!(
            posted.message_id, hidden_row,
            "never the worker's hidden row"
        );
        let h = host.lock().await;
        let row = h.doc.message(&posted.message_id).expect("the row exists");
        assert_eq!(row.role, "assistant");
        assert_eq!(row.status, "done");
        assert!(row.rest.get("hidden").is_none() && row.rest.get("job").is_none());
        assert_eq!(row.rest["inReplyTo"], json!(ask));
        assert_eq!(row.rest["turnRole"], json!("attachment"));
        assert_eq!(row.text(), "The affine.io mark.");
        let parts = attachment_parts(&row);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["attachment"]["kind"], json!("image"));
        assert_eq!(parts[0]["attachment"]["mime"], json!("image/png"));
        assert_eq!(parts[0]["attachment"]["title"], json!("affine.io logo"));
        let worker = h.doc.message(&hidden_row).unwrap();
        assert!(
            attachment_parts(&worker).is_empty(),
            "nothing on the hidden row"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// From the main turn the file is a posted result (no fallback line, no "didn't answer"
    /// row after it), and the tool's answer carries `posted: true` + the message id.
    #[tokio::test]
    async fn a_main_turn_delivery_counts_as_a_posted_result() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATATTACH0000000002";
        let host = agent.sync.open_room(chat).await.unwrap();
        {
            let h = host.lock().await;
            h.doc
                .append_user_message(
                    "m-busy",
                    "post the report",
                    "dev-mac",
                    &arbos_core::doc::UserMessageOpts::default(),
                )
                .unwrap();
        }
        let _busy = agent.test_hold_turn(chat).await;
        let dir = out_dir(&agent.cfg);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("report.pdf"), b"%PDF-1.4 fake").unwrap();
        let v = deliver_file(&agent, &json!({ "path": "out/report.pdf" })).await;
        assert!(v.get("isError").is_none(), "{v}");
        let text = v["content"][0]["text"].as_str().unwrap();
        let r: Value = serde_json::from_str(text).unwrap();
        assert_eq!(r["posted"], json!(true));
        assert_eq!(r["kind"], json!("pdf"));
        let id = r["message_id"].as_str().unwrap().to_owned();
        {
            let tl = agent.turn.lock().await;
            let t = tl.as_ref().unwrap();
            assert_eq!(t.attachments_posted, vec![id.clone()]);
        }
        let h = host.lock().await;
        let row = h.doc.message(&id).unwrap();
        assert_eq!(row.rest["inReplyTo"], json!("m-busy"));
        assert_eq!(attachment_parts(&row).len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    /// #2025: with nothing read back in the chat, a bubble that claims a post is refused with the
    /// correction; after a real delivery the same bubble goes through.
    #[tokio::test]
    async fn a_posted_claim_needs_a_read_back_row() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATATTACH0000000004";
        let host = agent.sync.open_room(chat).await.unwrap();
        {
            let h = host.lock().await;
            h.doc
                .append_user_message(
                    "m-busy",
                    "post the chart",
                    "dev-mac",
                    &arbos_core::doc::UserMessageOpts::default(),
                )
                .unwrap();
        }
        let _busy = agent.test_hold_turn(chat).await;
        let r = agent
            .hook_send_message(
                "Correction: the chart is posted. The chat service then confirmed it.",
                None,
                None,
            )
            .await;
        assert!(r.is_err(), "claim with no read-back must be refused");
        assert!(r.unwrap_err().contains("Not sent"));
        assert!(agent
            .hook_send_message(
                "The chart did not confirm as posted; the card above needs your approval.",
                None,
                None
            )
            .await
            .is_ok());
        let dir = out_dir(&agent.cfg);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("chart.png"), b"\x89PNG\r\n\x1a\nfake").unwrap();
        let v = deliver_file(&agent, &json!({ "path": "out/chart.png" })).await;
        assert!(v.get("isError").is_none(), "{v}");
        assert!(agent
            .hook_send_message("The chart is posted above as a picture.", None, None)
            .await
            .is_ok());
        let _ = std::fs::remove_dir_all(root);
    }

    /// A delivery reads only the project's own working files: a path outside the project folder
    /// (absolute or climbing out with `..`) and a path inside the project's own `.arbos/` state
    /// folder are both refused, so an ungated `deliver_file` cannot turn agentd's root privilege
    /// into a read of another workspace's secrets or of this workspace's own credential files.
    #[tokio::test]
    async fn a_delivery_cannot_leave_the_project_folder() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATATTACH0000000005";
        let host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;

        // A neighbour workspace on the same box, with the secrets file isolation keeps from this
        // workspace's user but not from agentd (which runs as root).
        let neighbour = root.join("01OTHERPROJECTONTHISBOX00").join(".arbos");
        std::fs::create_dir_all(&neighbour).unwrap();
        let theirs = neighbour.join("secrets.env");
        std::fs::write(&theirs, b"export STRIPE_KEY='sk_live_not_yours'\n").unwrap();
        // This workspace's own state folder: the vault file, the store, the harness configs that
        // carry the box token.
        let mine = agent.cfg.state_dir.join("secrets.env");
        std::fs::write(&mine, b"export OPENAI_API_KEY='sk-mine'\n").unwrap();

        let before = host.lock().await.doc.messages().len();
        for path in [
            theirs.display().to_string(),
            "../01OTHERPROJECTONTHISBOX00/.arbos/secrets.env".to_owned(),
            ".arbos/secrets.env".to_owned(),
            "/etc/hostname".to_owned(),
        ] {
            let v = deliver_file(&agent, &json!({ "path": path })).await;
            assert_eq!(v["isError"], json!(true), "{path} must not be posted: {v}");
            let text = v["content"][0]["text"].as_str().unwrap();
            assert!(text.contains("NOT posted"), "{path}: {text}");
        }
        assert_eq!(
            host.lock().await.doc.messages().len(),
            before,
            "no row for a refused delivery"
        );

        // A real file the agent made is still delivered.
        let dir = out_dir(&agent.cfg);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("report.pdf"), b"%PDF-1.4 fake").unwrap();
        let v = deliver_file(&agent, &json!({ "path": "out/report.pdf" })).await;
        assert!(v.get("isError").is_none(), "{v}");
        let _ = std::fs::remove_dir_all(root);
    }

    /// Honesty: a delivery that fails returns an error that says NOT posted, writes no row and
    /// never reports a message id; an SVG is a file card, not an "image" that never renders.
    #[tokio::test]
    async fn a_failed_delivery_says_not_posted_and_leaves_no_row() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATATTACH0000000003";
        let host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        let before = host.lock().await.doc.messages().len();
        let v = deliver_file(&agent, &json!({ "path": "out/does-not-exist.png" })).await;
        assert_eq!(v["isError"], json!(true));
        let text = v["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("NOT posted"), "{text}");
        assert!(!text.contains("message_id"));
        assert_eq!(host.lock().await.doc.messages().len(), before);
        assert!(agent
            .turn
            .lock()
            .await
            .as_ref()
            .unwrap()
            .attachments_posted
            .is_empty());
        let v = deliver_file(&agent, &json!({})).await;
        assert_eq!(v["isError"], json!(true));
        // SVG → file card + hint
        let dir = out_dir(&agent.cfg);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("mark.svg"),
            b"<svg xmlns='http://www.w3.org/2000/svg'/>",
        )
        .unwrap();
        let v = deliver_file(&agent, &json!({ "path": "out/mark.svg", "kind": "image" })).await;
        let r: Value = serde_json::from_str(v["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(r["posted"], json!(true));
        assert_eq!(r["kind"], json!("file"));
        assert!(r["hint"].as_str().unwrap().contains("PNG"));
        assert_eq!(kind_for("logo.svg", "image/svg+xml"), "file");
        assert_eq!(kind_for("logo.png", "image/png"), "image");
        let _ = std::fs::remove_dir_all(root);
    }
}
