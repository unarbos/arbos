//! Blob mirror (crdt-sync §6.4): `PUT/GET /blob/{projectId}/{key}`. Files on disk,
//! keys are sha256 hex or ULIDs; people-chat slots use `people:{chat_id}`. Path traversal is refused.

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use tokio::io::AsyncReadExt;

use crate::auth::Principal;
use crate::image_sniff::{check_avatar, ImageKind, NotAnImage, SNIFF_LEN};
use crate::signin::api_error;
use crate::state::AppState;

/// What a `PATCH … {"image_key"}` finds under the key before it may become a picture.
#[derive(Debug)]
pub enum StoredImage {
    Image(ImageKind),
    /// Nothing was uploaded under that key (or the key is not a blob name).
    Missing,
    NotAnImage(NotAnImage),
}

/// Judge the blob a client wants as a group / server picture: its recorded `Content-Type`
/// and its first bytes must both say "image" (`image_sniff::check_avatar`). The upload
/// endpoint stays generic (chats carry files too); the picture setter is the gate.
pub async fn stored_image(state: &AppState, slot: &str, key: &str) -> StoredImage {
    if !safe_key(key) || !safe_key(slot) {
        return StoredImage::Missing;
    }
    let p = blob_path(state, slot, key);
    let Ok(mut f) = tokio::fs::File::open(&p).await else {
        return StoredImage::Missing;
    };
    let mut head = [0u8; SNIFF_LEN];
    let mut n = 0;
    while n < head.len() {
        match f.read(&mut head[n..]).await {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(_) => return StoredImage::Missing,
        }
    }
    let mime = tokio::fs::read_to_string(p.with_extension("mime"))
        .await
        .ok();
    match check_avatar(mime.as_deref(), &head[..n]) {
        Ok(kind) => StoredImage::Image(kind),
        Err(why) => StoredImage::NotAnImage(why),
    }
}

/// The plain refusal every avatar endpoint answers with.
pub fn not_an_image(why: &NotAnImage) -> Response {
    api_error(
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "not_an_image",
        why.to_string(),
    )
}

/// `image_key` names nothing that was uploaded.
pub fn no_such_image() -> Response {
    api_error(
        StatusCode::BAD_REQUEST,
        "no_such_image",
        "Upload the picture first, then set image_key to its name",
    )
}

fn safe_key(k: &str) -> bool {
    // `:` is required for people-chat blob slots (`people:{chat_id}`); still no `/` or `..` path bits.
    !k.is_empty()
        && k.len() <= 128
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == ':')
}

fn blob_path(state: &AppState, project_id: &str, key: &str) -> std::path::PathBuf {
    state.cfg.data_dir.join("blobs").join(project_id).join(key)
}

/// Write a blob the relay itself produced (the generated avatar) exactly as `put` would store an
/// upload: bytes under the slot, the MIME sidecar, the `blobs` row.
pub async fn store(
    state: &AppState,
    slot: &str,
    key: &str,
    mime: &str,
    bytes: &[u8],
) -> Result<(), String> {
    if !safe_key(key) || !safe_key(slot) {
        return Err("bad key".to_owned());
    }
    let p = blob_path(state, slot, key);
    if let Some(parent) = p.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    tokio::fs::write(&p, bytes)
        .await
        .map_err(|e| e.to_string())?;
    let _ = state
        .db
        .record_blob(slot, key, bytes.len() as i64, Some(mime));
    tokio::fs::write(p.with_extension("mime"), mime)
        .await
        .map_err(|e| e.to_string())
}

pub async fn put(
    State(state): State<AppState>,
    principal: Principal,
    Path((project_id, key)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !state.allows(&principal, &project_id) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !safe_key(&key) || !safe_key(&project_id) {
        return (StatusCode::BAD_REQUEST, "bad key").into_response();
    }
    let p = blob_path(&state, &project_id, &key);
    if let Some(parent) = p.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    if let Err(e) = tokio::fs::write(&p, &body).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    let mime = headers.get("content-type").and_then(|v| v.to_str().ok());
    let _ = state
        .db
        .record_blob(&project_id, &key, body.len() as i64, mime);
    if let Some(m) = mime {
        let _ = tokio::fs::write(p.with_extension("mime"), m).await;
    }
    StatusCode::NO_CONTENT.into_response()
}

/// `GET /blob/{slot}/{key}[?w=<px>]`. `w=` applies to image blobs only — the group / server /
/// preview pictures the apps draw — and serves the picture at that bucket (`avatar_image`);
/// any other blob, or a `w` the picture is already within, streams the stored bytes.
pub async fn get(
    State(state): State<AppState>,
    principal: Principal,
    Path((project_id, key)): Path<(String, String)>,
    Query(size): Query<crate::avatar_image::SizeQuery>,
) -> Response {
    if !state.allows(&principal, &project_id) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Some(w) = size.w {
        if let StoredImage::Image(kind) = stored_image(&state, &project_id, &key).await {
            if let Some(bytes) = read(&state, &project_id, &key).await {
                return crate::avatar_image::respond(
                    &state,
                    crate::avatar_image::Master { kind, bytes },
                    Some(w),
                    None,
                )
                .await;
            }
        }
    }
    serve(&state, &project_id, &key).await
}

/// The whole stored blob in memory (access already checked by the caller); `None` when there is
/// no such blob. For small things only — the avatar setter copying a picture master.
pub async fn read(state: &AppState, slot: &str, key: &str) -> Option<Vec<u8>> {
    if !safe_key(key) || !safe_key(slot) {
        return None;
    }
    tokio::fs::read(blob_path(state, slot, key)).await.ok()
}

/// Stream one stored blob (access already checked by the caller).
pub async fn serve(state: &AppState, slot: &str, key: &str) -> Response {
    if !safe_key(key) || !safe_key(slot) {
        return (StatusCode::BAD_REQUEST, "bad key").into_response();
    }
    let p = blob_path(state, slot, key);
    let Ok(f) = tokio::fs::File::open(&p).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mime = tokio::fs::read_to_string(p.with_extension("mime"))
        .await
        .unwrap_or_else(|_| "application/octet-stream".to_owned());
    let stream = tokio_util::io::ReaderStream::new(f);
    ([("content-type", mime)], Body::from_stream(stream)).into_response()
}

/// `GET /bootstrap/{file}` — dev-image bootstrap artifacts (box tokens only; see image/Dockerfile.dev-inline).
pub async fn bootstrap(
    State(state): State<AppState>,
    principal: Principal,
    Path(file): Path<String>,
) -> Response {
    if !principal.is_box() && !matches!(principal, Principal::Admin) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(dir) = &state.cfg.bootstrap_dir else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !safe_key(&file) {
        return (StatusCode::BAD_REQUEST, "bad name").into_response();
    }
    let Ok(f) = tokio::fs::File::open(dir.join(&file)).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let stream = tokio_util::io::ReaderStream::new(f);
    (
        [("content-type", "application/octet-stream")],
        Body::from_stream(stream),
    )
        .into_response()
}
