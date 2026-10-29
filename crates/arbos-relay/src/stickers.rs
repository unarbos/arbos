//! Sticker packs for the apps' picker (ARB-544, Jacob 2026-10-28): the relay hosts two open
//! sets — **Fluent Emoji 3D** (Microsoft, MIT) and **Noto Animated Emoji** (Google, CC BY 4.0,
//! credit shown in the picker). Both licences allow copies, so the relay keeps each sticker
//! on disk after the first fetch (`<data>/stickers/<pack>/<id>`) and the apps store the ones
//! people send in the chat slot like any picture.
//!
//! - `GET /stickers/packs` — the manifest (`assets/stickers/packs.json`, built by
//!   `scripts/fun/build-emoji-and-packs.py`) with each item's relay path instead of its
//!   upstream URL.
//! - `GET /stickers/{pack}/{id}` — the sticker bytes: from disk, else fetched once from the
//!   publisher's CDN (only the two hosts the manifest names), cached, long `max-age`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::Principal;
use crate::signin::api_error;
use crate::state::AppState;

/// The committed manifest.
pub const PACKS_JSON: &str = include_str!("../assets/stickers/packs.json");
/// A sticker larger than this is refused (the Noto 512 px WebPs run to ~600 KB).
pub const STICKER_MAX_BYTES: usize = 3 * 1024 * 1024;
/// Publisher CDNs the relay will fetch from.
const UPSTREAM_HOSTS: &[&str] = &["raw.githubusercontent.com", "fonts.gstatic.com"];

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Item {
    pub id: String,
    pub name: String,
    pub emoji: String,
    pub mime: String,
    pub upstream: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pack {
    pub id: String,
    pub name: String,
    pub publisher: String,
    pub license: String,
    pub license_url: String,
    pub attribution: String,
    pub animated: bool,
    pub page_url: String,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Manifest {
    pub version: u32,
    pub packs: Vec<Pack>,
}

fn manifest() -> &'static Manifest {
    static M: OnceLock<Manifest> = OnceLock::new();
    M.get_or_init(|| serde_json::from_str(PACKS_JSON).expect("packs.json is valid"))
}

fn index() -> &'static HashMap<(String, String), &'static Item> {
    static I: OnceLock<HashMap<(String, String), &'static Item>> = OnceLock::new();
    I.get_or_init(|| {
        let mut map = HashMap::new();
        for pack in &manifest().packs {
            for item in &pack.items {
                map.insert((pack.id.clone(), item.id.clone()), item);
            }
        }
        map
    })
}

/// The manifest as the apps see it: items carry `path` (relative to the relay) and no
/// upstream URL, and each pack its licence line.
pub fn public_manifest() -> Value {
    let packs: Vec<Value> = manifest()
        .packs
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "name": p.name,
                "publisher": p.publisher,
                "license": p.license,
                "licenseUrl": p.license_url,
                "attribution": p.attribution,
                "animated": p.animated,
                "pageUrl": p.page_url,
                "mime": p.items.first().map(|i| i.mime.as_str()).unwrap_or("image/png"),
                "items": p.items.iter().map(|i| json!({
                    "id": i.id,
                    "name": i.name,
                    "emoji": i.emoji,
                    "mime": i.mime,
                    "path": format!("/stickers/{}/{}", p.id, i.id),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({ "version": manifest().version, "packs": packs })
}

/// `GET /stickers/packs` for any signed-in principal.
pub async fn packs(_principal: Principal) -> Response {
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "private, max-age=3600")],
        Json(public_manifest()),
    )
        .into_response()
}

fn upstream_allowed(url: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(url) else {
        return false;
    };
    u.scheme() == "https"
        && u.host_str()
            .is_some_and(|h| UPSTREAM_HOSTS.contains(&h.to_ascii_lowercase().as_str()))
}

fn safe_segment(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 80
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn disk_path(data_dir: &std::path::Path, pack: &str, id: &str) -> PathBuf {
    data_dir.join("stickers").join(pack).join(id)
}

/// The bytes of one sticker: disk first, then the publisher's CDN (stored for next time).
pub async fn fetch_sticker(state: &AppState, pack: &str, id: &str) -> Option<(String, Bytes)> {
    let item = *index().get(&(pack.to_owned(), id.to_owned()))?;
    let path = disk_path(&state.cfg.data_dir, pack, id);
    if let Ok(bytes) = tokio::fs::read(&path).await {
        if !bytes.is_empty() {
            return Some((item.mime.clone(), Bytes::from(bytes)));
        }
    }
    if !upstream_allowed(&item.upstream) {
        return None;
    }
    let resp = state
        .http
        .get(&item.upstream)
        .header(header::USER_AGENT, crate::gifs::USER_AGENT)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        tracing::warn!("sticker {pack}/{id}: upstream http {}", resp.status());
        return None;
    }
    if resp
        .content_length()
        .is_some_and(|l| l as usize > STICKER_MAX_BYTES)
    {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    if bytes.is_empty() || bytes.len() > STICKER_MAX_BYTES {
        return None;
    }
    if let Some(dir) = path.parent() {
        let _ = tokio::fs::create_dir_all(dir).await;
    }
    let tmp = path.with_extension("part");
    if tokio::fs::write(&tmp, &bytes).await.is_ok() {
        let _ = tokio::fs::rename(&tmp, &path).await;
    }
    Some((item.mime.clone(), bytes))
}

/// `GET /stickers/{pack}/{id}` for any signed-in principal.
pub async fn get(
    State(state): State<AppState>,
    _principal: Principal,
    Path((pack, id)): Path<(String, String)>,
) -> Response {
    if !safe_segment(&pack) || !safe_segment(&id) {
        return api_error(StatusCode::BAD_REQUEST, "bad_sticker", "unknown sticker");
    }
    if !index().contains_key(&(pack.clone(), id.clone())) {
        return api_error(StatusCode::NOT_FOUND, "no_sticker", "unknown sticker");
    }
    match fetch_sticker(&state, &pack, &id).await {
        Some((mime, bytes)) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, mime),
                (
                    header::CACHE_CONTROL,
                    "public, max-age=2592000, immutable".to_owned(),
                ),
            ],
            bytes,
        )
            .into_response(),
        None => api_error(
            StatusCode::BAD_GATEWAY,
            "sticker_unavailable",
            "This sticker could not be loaded. Try again.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_parses_and_names_both_packs() {
        let m = manifest();
        let ids: Vec<&str> = m.packs.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["fluent-3d", "noto-animated"]);
        for p in &m.packs {
            assert!(p.items.len() > 500, "{} has {} items", p.id, p.items.len());
            assert!(!p.attribution.is_empty());
            for i in &p.items {
                assert!(safe_segment(&i.id), "{}", i.id);
                assert!(upstream_allowed(&i.upstream), "{}", i.upstream);
            }
        }
        let fluent = &m.packs[0];
        assert_eq!(fluent.license, "MIT");
        assert!(!fluent.animated);
        let noto = &m.packs[1];
        assert_eq!(noto.license, "CC BY 4.0");
        assert!(noto.animated);
        assert!(noto.items.iter().all(|i| i.mime == "image/webp"));
    }

    #[test]
    fn public_manifest_hides_upstreams_and_gives_relay_paths() {
        let v = public_manifest();
        let item = &v["packs"][0]["items"][0];
        assert!(item.get("upstream").is_none());
        assert_eq!(
            item["path"],
            format!("/stickers/fluent-3d/{}", item["id"].as_str().unwrap())
        );
        assert_eq!(
            v["packs"][1]["attribution"],
            "Noto Animated Emoji · Google · CC BY 4.0"
        );
    }

    #[test]
    fn segments_and_upstreams_are_checked() {
        assert!(safe_segment("grinning-face"));
        assert!(!safe_segment("../etc"));
        assert!(!safe_segment(""));
        assert!(upstream_allowed(
            "https://raw.githubusercontent.com/microsoft/fluentui-emoji/main/x.png"
        ));
        assert!(!upstream_allowed("https://evil.example/x.png"));
        assert!(!upstream_allowed("http://fonts.gstatic.com/x.webp"));
    }
}
