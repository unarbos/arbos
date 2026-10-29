//! Attachments on a user message reach the agent. A client uploads the bytes to the relay blob
//! store (`PUT /blob/{project}/{ref}`) and writes an `attachment` part `{ name, mime, ref }` on
//! the message. People chats use the slot `people:{chat_id}` (same as iOS/Mac); project chats use
//! the project id. For each one the box:
//!   - downloads the blob into `<workspace>/in/<ref>-<name>` so tools can open it;
//!   - images: also hands the bytes to the model as an image input (vision) on the pi prompt;
//!   - audio: transcribes through the relay (`/proxy/v1/audio/transcriptions`) and uses the
//!     text as the message when the user typed nothing (voice note), else appends it;
//!   - anything else: names the file and its path in the prompt.
//!
//! Clients should transcribe voice notes before sending; this is the fallback.

use std::path::Path;

use anyhow::{anyhow, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::{json, Value};

use arbos_core::doc::MessageView;

use crate::agent::Agent;

/// Images above this are not sent inline (the file stays on disk for the `read` tool).
const MAX_INLINE_IMAGE_BYTES: usize = 5 * 1024 * 1024;
const MAX_AUDIO_BYTES: usize = 25 * 1024 * 1024;

/// What one message's attachments add to a turn.
#[derive(Debug, Default)]
pub struct TurnAttachments {
    /// Lines appended to the prompt (file paths, transcripts).
    pub notes: Vec<String>,
    /// Image inputs for the pi prompt.
    pub images: Vec<Value>,
    /// A voice-note transcript to use as the message text when the user typed nothing.
    pub transcript: Option<String>,
}

fn attachment_parts(msg: &MessageView) -> Vec<Value> {
    msg.parts
        .iter()
        .filter(|p| p.get("kind").and_then(Value::as_str) == Some("attachment"))
        .filter_map(|p| p.get("attachment").cloned())
        .collect()
}

fn safe_name(name: &str) -> String {
    let n: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if n.is_empty() {
        "file".into()
    } else {
        n
    }
}

impl Agent {
    /// Download and prepare every attachment of the message. Never fails the turn: a file that
    /// cannot be fetched becomes a note saying so. `chat_id` picks the blob slot (people vs project).
    pub async fn turn_attachments(&self, chat_id: &str, msg: &MessageView) -> TurnAttachments {
        let mut out = TurnAttachments::default();
        let parts = attachment_parts(msg);
        if parts.is_empty() {
            return out;
        }
        let in_dir = self.cfg.work_dir.join("in");
        let _ = std::fs::create_dir_all(&in_dir);
        let blob_slot = self.blob_slot(chat_id);
        for a in parts {
            let name = a
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("attachment")
                .to_owned();
            let mime = a
                .get("mime")
                .and_then(Value::as_str)
                .unwrap_or("application/octet-stream")
                .to_owned();
            let Some(blob_ref) = a.get("ref").and_then(Value::as_str) else {
                // A picked GIF (ARB-544) lives with its provider: the row carries a `url` and
                // attribution, never a relay copy. Tell the model what it is — it is not a
                // failed upload, and nobody should be asked to send it again.
                if a.get("url").and_then(Value::as_str).is_some() {
                    let provider = a
                        .pointer("/source/provider")
                        .and_then(Value::as_str)
                        .unwrap_or("a GIF provider");
                    out.notes.push(format!(
                        "[The user sent a GIF (\"{name}\", via {provider}).]"
                    ));
                    continue;
                }
                out.notes.push(format!(
                    "[Attachment \"{name}\" ({mime}) was not uploaded by the client; ask the user to send it again.]"
                ));
                continue;
            };
            let bytes = match self.fetch_blob(&blob_slot, blob_ref).await {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(error = %e, blob_ref, blob_slot, "attachment download");
                    out.notes.push(format!(
                        "[Attachment \"{name}\" ({mime}) could not be fetched from the relay: {e}.]"
                    ));
                    continue;
                }
            };
            let path = in_dir.join(format!(
                "{}-{}",
                &blob_ref[..blob_ref.len().min(12)],
                safe_name(&name)
            ));
            if let Err(e) = std::fs::write(&path, &bytes) {
                tracing::warn!(error = %e, path = %path.display(), "attachment write");
            }
            let shown = self.display_path(&path);
            if mime.starts_with("image/") {
                if bytes.len() <= MAX_INLINE_IMAGE_BYTES {
                    out.images.push(
                        json!({ "type": "image", "data": B64.encode(&bytes), "mimeType": mime }),
                    );
                    out.notes.push(format!(
                        "[The user attached the image \"{name}\" ({mime}, {} KB); it is included with this message and saved at {shown}.]",
                        bytes.len() / 1024
                    ));
                } else {
                    out.notes.push(format!(
                        "[The user attached the image \"{name}\" ({mime}, {} MB), saved at {shown}; open it with your read tool to look at it.]",
                        bytes.len() / (1024 * 1024)
                    ));
                }
            } else if mime.starts_with("audio/")
                || name.ends_with(".m4a")
                || name.ends_with(".mp3")
                || name.ends_with(".wav")
            {
                if bytes.len() > MAX_AUDIO_BYTES {
                    out.notes.push(format!(
                        "[The user attached the audio file \"{name}\" ({} MB), saved at {shown}; too large to transcribe here.]",
                        bytes.len() / (1024 * 1024)
                    ));
                    continue;
                }
                match self.transcribe(&name, &mime, bytes).await {
                    Ok(text) if !text.trim().is_empty() => {
                        out.notes.push(format!(
                            "[Voice note \"{name}\" transcribed; the audio is saved at {shown}.]"
                        ));
                        out.transcript = Some(text.trim().to_owned());
                    }
                    Ok(_) => out.notes.push(format!(
                        "[Voice note \"{name}\" had no speech the transcriber could hear; the audio is saved at {shown}.]"
                    )),
                    Err(e) => {
                        tracing::warn!(error = %e, name, "voice note transcription");
                        out.notes.push(format!(
                            "[Voice note \"{name}\" could not be transcribed ({e}); the audio is saved at {shown}.]"
                        ));
                    }
                }
            } else {
                out.notes.push(format!(
                    "[The user attached \"{name}\" ({mime}, {} KB), saved at {shown}; read it from there.]",
                    bytes.len() / 1024
                ));
            }
        }
        out
    }

    async fn fetch_blob(&self, blob_slot: &str, blob_ref: &str) -> Result<Vec<u8>> {
        let r = self
            .http_client()
            .get(format!(
                "{}/blob/{}/{}",
                self.cfg.relay_url, blob_slot, blob_ref
            ))
            .bearer_auth(&self.cfg.box_token)
            .send()
            .await?;
        if !r.status().is_success() {
            return Err(anyhow!("HTTP {}", r.status()));
        }
        Ok(r.bytes().await?.to_vec())
    }

    /// `POST /proxy/v1/audio/transcriptions` through the relay (the key stays on the server).
    async fn transcribe(&self, name: &str, mime: &str, bytes: Vec<u8>) -> Result<String> {
        let file = reqwest::multipart::Part::bytes(bytes)
            .file_name(name.to_owned())
            .mime_str(mime)
            .unwrap_or_else(|_| reqwest::multipart::Part::bytes(Vec::new()));
        let form = reqwest::multipart::Form::new()
            .text("model", "whisper-1")
            .text("response_format", "json")
            .part("file", file);
        let r = self
            .http_client()
            .post(format!(
                "{}/proxy/v1/audio/transcriptions",
                self.cfg.relay_url
            ))
            .bearer_auth(&self.cfg.box_token)
            .header("x-arbos-project", &self.cfg.project_id)
            .multipart(form)
            .timeout(std::time::Duration::from_secs(120))
            .send()
            .await?;
        if !r.status().is_success() {
            return Err(anyhow!("HTTP {}", r.status()));
        }
        let v: Value = r.json().await?;
        Ok(v.get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned())
    }

    /// The path as the agent sees it (relative to its workspace when inside it).
    fn display_path(&self, p: &Path) -> String {
        p.strip_prefix(&self.cfg.work_dir)
            .map(|r| r.display().to_string())
            .unwrap_or_else(|_| p.display().to_string())
    }
}
