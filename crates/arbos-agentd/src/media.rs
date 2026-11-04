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
use crate::sync::DocHost;

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
    /// The job this delivery belongs to, when it belongs to one: the worker's own job, or the
    /// job whose answer the main turn is relaying. Files are deduplicated within it.
    pub job_id: Option<String>,
}

pub(crate) async fn target(a: &Agent, session_id: Option<&str>) -> Option<Target> {
    let worker_target =
        |chat_id: &str, row_id: &str, (host_doc_ask, job_id): (Option<String>, Option<String>)| {
            Target {
                chat_id: chat_id.to_owned(),
                ask_id: host_doc_ask.or_else(|| Some(row_id.to_owned())),
                main_turn: false,
                job_id,
            }
        };
    if let Some(sid) = session_id.filter(|s| !s.is_empty()) {
        if let Some((chat_id, row_id)) = a.subagent_msgs.lock().await.get(sid).cloned() {
            let row = worker_row(a, &chat_id, &row_id).await;
            return Some(worker_target(&chat_id, &row_id, row));
        }
    }
    {
        let tl = a.turn.lock().await;
        if let Some(t) = tl.as_ref() {
            return Some(Target {
                chat_id: t.chat_id.clone(),
                ask_id: Some(t.user_msg_id.clone()),
                main_turn: true,
                job_id: t.job_id.clone(),
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
            let row = worker_row(a, &chat_id, &row_id).await;
            Some(worker_target(&chat_id, &row_id, row))
        }
        None => None,
    }
}

/// `inReplyTo` (the ask a worker row answers) and the id of the job the worker runs, read from
/// the worker's own progress row when the chat is open.
async fn worker_row(a: &Agent, chat_id: &str, row_id: &str) -> (Option<String>, Option<String>) {
    let Some(host) = a.sync.host(chat_id).await else {
        return (None, None);
    };
    let h = host.lock().await;
    let Some(row) = h.doc.message(row_id) else {
        return (None, None);
    };
    let ask = row
        .rest
        .get("inReplyTo")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let job_id = row
        .rest
        .get("job")
        .and_then(|j| j.get("id"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    (ask, job_id)
}

/// Row `msg_id` carries an attachment pointing at blob `key`: what makes a card real, both on
/// the read-back after a write and when an older card is offered in place of a second one.
fn row_holds_blob(h: &DocHost, msg_id: &str, key: &str) -> bool {
    h.doc.message(msg_id).is_some_and(|m| {
        m.parts.iter().any(|p| {
            p.get("kind").and_then(Value::as_str) == Some("attachment")
                && p.pointer("/attachment/ref").and_then(Value::as_str) == Some(key)
        })
    })
}

/// Content address of a blob: what the relay stores it under, and what tells "the same file
/// again" apart from "the same name with new content".
pub(crate) fn blob_key(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}

pub(crate) async fn upload(
    a: &Arc<Agent>,
    chat_id: &str,
    bytes: Vec<u8>,
    key: &str,
    mime: &str,
) -> anyhow::Result<()> {
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
    Ok(())
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
    /// These bytes were already delivered earlier in the same job, so `message_id` is that
    /// earlier row and nothing new was written.
    pub duplicate: bool,
}

/// Source of a delivery: a file on the box, or bytes fetched from a URL.
pub enum Source<'a> {
    Path(&'a str),
    Url(&'a str),
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
            let p = Path::new(path);
            let p = if p.is_absolute() {
                p.to_path_buf()
            } else {
                a.cfg.work_dir.join(p)
            };
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
    let key = blob_key(&bytes);
    let attachment = json!({
        "name": name, "mime": mime, "ref": key, "bytes": size, "kind": kind, "title": title,
    });
    // One job delivers through its worker, the fresh worker a failed attempt gets and the
    // result turn that relays the answer, and each of them posted these same bytes again. The
    // card is already in the chat: point at it instead of writing a second one.
    if let Some(job_id) = target.job_id.as_deref() {
        if let Some(message_id) = a.job_file_posted(job_id, &key).await {
            // Only a card that is still in the chat may stand in for this one; if it went, the
            // honest answer is to write a new one.
            let standing = match a.sync.host(&target.chat_id).await {
                Some(host) => row_holds_blob(&*host.lock().await, &message_id, &key),
                None => false,
            };
            if standing {
                // The sentence that says the file is posted needs a read-back row from this
                // turn (#2025); the card the model is pointed at is that row.
                a.note_posted(&target.chat_id, &message_id).await;
                tracing::info!(
                    chat_id = %target.chat_id, job = job_id, %name, %message_id,
                    "deliver_file: this job already put these bytes in the chat; not posted twice"
                );
                return Ok(Posted {
                    chat_id: target.chat_id,
                    message_id,
                    attachment,
                    duplicate: true,
                });
            }
        }
    }
    upload(a, &target.chat_id, bytes, &key, &mime).await?;
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
        if !row_holds_blob(&h, &msg_id, &key) {
            let _ = h.doc.delete_message(&msg_id);
            anyhow::bail!("the attachment row did not read back from the chat doc");
        }
    }
    a.note_posted(&target.chat_id, &msg_id).await;
    if let Some(job_id) = target.job_id.as_deref() {
        a.note_job_file(job_id, &key, &msg_id).await;
    }
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
        duplicate: false,
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
        "note": if p.duplicate {
            "This exact file is already in the chat from earlier in this job — the message id above is that card, and no second one was written. Refer to it; do not post it again. Change the file and it goes in as a new card."
        } else if p.attachment["kind"] == "image" {
            "The picture is in the chat as its own message, inline for everyone. Say in one line what it shows; do not repeat the path."
        } else {
            "The file is in the chat as a card with a download button, visible to everyone. Say in one line what it contains; do not repeat the path."
        },
    });
    if p.duplicate {
        if let Some(o) = v.as_object_mut() {
            o.insert("already_posted".to_owned(), json!(true));
        }
    }
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
    use crate::sync::{DocHost, SharedHost};

    fn attachment_parts(m: &arbos_core::doc::MessageView) -> Vec<Value> {
        m.parts
            .iter()
            .filter(|p| p.get("kind").and_then(Value::as_str) == Some("attachment"))
            .cloned()
            .collect()
    }

    fn attachment_rows(h: &DocHost) -> usize {
        h.doc
            .messages()
            .iter()
            .filter(|m| !attachment_parts(m).is_empty())
            .count()
    }

    /// A hidden worker progress row for `job_id`, registered under session `sid` the way
    /// `spawn_subagent_task` registers a running worker.
    async fn worker_session(
        agent: &Arc<Agent>,
        host: &SharedHost,
        chat: &str,
        ask: &str,
        job_id: &str,
        sid: &str,
    ) {
        let row = ulid();
        {
            let h = host.lock().await;
            h.doc
                .start_assistant_message(&row, ask, &agent.cfg.device_id);
            h.doc.set_message_fields(
                &row,
                &[
                    (
                        "job",
                        json!({ "id": job_id, "title": "Pricing the options", "kind": "research", "touches": [] }),
                    ),
                    ("worker", json!(true)),
                    ("hidden", json!(true)),
                ],
            );
        }
        agent
            .subagent_msgs
            .lock()
            .await
            .insert(sid.to_owned(), (chat.to_owned(), row));
    }

    /// Test5 round 2, bug J: one ask, the same file in the chat twice. A job delivers through
    /// its worker, through the fresh worker a failed attempt gets and again through the result
    /// turn that relays the answer, and each of them posted the same bytes as a new card.
    ///
    /// The key is the job and the file's content hash: the same bytes again in the same job
    /// answer with the card that is already there, a file the job wrote anew still goes in, and
    /// another job is untouched.
    #[tokio::test]
    async fn a_job_puts_the_same_file_in_the_chat_once() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATATTACH0000000005";
        let host = agent.sync.open_room(chat).await.unwrap();
        let ask = ulid();
        {
            let h = host.lock().await;
            h.doc
                .append_user_message(
                    &ask,
                    "compare the plans and send me the sheet",
                    "dev-mac",
                    &arbos_core::doc::UserMessageOpts::default(),
                )
                .unwrap();
        }
        let job = "01TESTJOB00000000000000001";
        let first = "01TESTWORKERSESSION0000010";
        worker_session(&agent, &host, chat, &ask, job, first).await;
        let dir = out_dir(&agent.cfg);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("plans.csv");
        std::fs::write(&path, b"plan,price\nbasic,10\n").unwrap();

        let posted = deliver_file(
            &agent,
            &json!({ "path": "out/plans.csv", "session_id": first }),
        )
        .await;
        let r: Value =
            serde_json::from_str(posted["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(r["posted"], json!(true));
        assert!(r.get("already_posted").is_none(), "the first card is new");
        let card = r["message_id"].as_str().unwrap().to_owned();
        assert_eq!(attachment_rows(&*host.lock().await), 1);

        // The same worker calls again, and the fresh worker the retry gives the job posts the
        // file it found on disk: one card, and both calls point the model at it.
        let retry = "01TESTWORKERSESSION0000011";
        worker_session(&agent, &host, chat, &ask, job, retry).await;
        for sid in [first, retry] {
            let again = deliver_file(
                &agent,
                &json!({ "path": "out/plans.csv", "session_id": sid }),
            )
            .await;
            let r: Value =
                serde_json::from_str(again["content"][0]["text"].as_str().unwrap()).unwrap();
            assert_eq!(
                attachment_rows(&*host.lock().await),
                1,
                "session {sid} wrote a second card for the same file"
            );
            assert_eq!(r["posted"], json!(true), "the file IS in the chat");
            assert_eq!(r["already_posted"], json!(true));
            assert_eq!(
                r["message_id"],
                json!(card),
                "the answer names the card that is already there"
            );
        }

        // The result turn of the same job relays the answer and posts the file once more. The
        // job ran for ten minutes, so the worker's card is older than the window a "posted"
        // sentence is checked against (#2025).
        for (_, at) in agent
            .posted_ledger
            .lock()
            .await
            .get_mut(chat)
            .unwrap()
            .iter_mut()
        {
            *at = now_ms() - 600_000;
        }
        let _busy = agent.test_hold_turn(chat).await;
        agent.turn.lock().await.as_mut().unwrap().job_id = Some(job.to_owned());
        let relayed = deliver_file(&agent, &json!({ "path": "out/plans.csv" })).await;
        let r: Value =
            serde_json::from_str(relayed["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(r["already_posted"], json!(true));
        assert_eq!(r["message_id"], json!(card));
        assert_eq!(
            attachment_rows(&*host.lock().await),
            1,
            "the result turn wrote a second card for the file its worker already posted"
        );
        assert!(
            agent
                .turn
                .lock()
                .await
                .as_ref()
                .unwrap()
                .attachments_posted
                .is_empty(),
            "a card that was not written is not this turn's posted result"
        );
        // #2025 holds the other way too: the file really is in the chat, so the sentence that
        // says so must still go out. Without this the fix would trade two cards for a refusal.
        assert!(
            agent
                .hook_send_message("The plans file is posted above as a card.", None, None)
                .await
                .is_ok(),
            "a duplicate that was suppressed still confirms the card in the chat"
        );

        // The job wrote the sheet again with the real numbers: that is a second, real delivery.
        std::fs::write(&path, b"plan,price\nbasic,10\npro,30\n").unwrap();
        let updated = deliver_file(
            &agent,
            &json!({ "path": "out/plans.csv", "session_id": first }),
        )
        .await;
        let r: Value =
            serde_json::from_str(updated["content"][0]["text"].as_str().unwrap()).unwrap();
        assert!(
            r.get("already_posted").is_none(),
            "a file whose content changed is a new delivery"
        );
        assert_ne!(r["message_id"], json!(card));
        assert_eq!(attachment_rows(&*host.lock().await), 2);

        // Another job answering another ask is not held back by what this one delivered.
        std::fs::write(&path, b"plan,price\nbasic,10\n").unwrap();
        let other = "01TESTWORKERSESSION0000012";
        worker_session(
            &agent,
            &host,
            chat,
            &ask,
            "01TESTJOB00000000000000002",
            other,
        )
        .await;
        let elsewhere = deliver_file(
            &agent,
            &json!({ "path": "out/plans.csv", "session_id": other }),
        )
        .await;
        let r: Value =
            serde_json::from_str(elsewhere["content"][0]["text"].as_str().unwrap()).unwrap();
        assert!(r.get("already_posted").is_none(), "another job delivers");
        assert_eq!(attachment_rows(&*host.lock().await), 3);
        let _ = std::fs::remove_dir_all(root);
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
