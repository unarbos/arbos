//! Agent-hosted sites (#2060): `publish_site`, `unpublish_site`, `list_sites`, and the box-local
//! preview server. The site lives in the project folder at `sites/<app>/` (`index.html` plus
//! assets); publishing packs it under the shared rules (`arbos_core::sites`), ships only the
//! blobs the relay lacks, commits, and then — before the tool says anything is live — reads
//! `index.html` back over the public route and compares hashes (the #1900/#2026 rule: no claim
//! without a read-back). The public URL is `https://<owner>.arbos.life/<app>/`.
//!
//! The preview server (`$ARBOS_SITES_PREVIEW_URL<app>/…` = `127.0.0.1:<sites_port>/sites/<app>/…`, loopback only) serves the same
//! folder with the same rules: no traversal, no dotfiles, no symlink escape, declared MIME types,
//! a 404 page — so the agent can check a page the way the edge will serve it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Path as AxPath, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use arbos_core::sites::{
    allowed_extensions, check_app, check_files, check_html, check_path, resolve_request_path,
    SiteFile, MAX_FILES, MAX_FILE_BYTES, MAX_SITE_BYTES,
};

use crate::agent::Agent;
use crate::mcp::{err_result, text_result};

/// How long the publish waits for the public route to serve the new `index.html`.
const READ_BACK_WAIT: Duration = Duration::from_secs(50);
const READ_BACK_EVERY: Duration = Duration::from_millis(1500);

pub fn tool_specs() -> Vec<Value> {
    vec![
        json!({
            "name": "publish_site",
            "description": "Publish a static website from this project's `sites/<app>/` folder to its public address `https://<owner>.arbos.life/<app>/`. The folder needs an `index.html` at its root; HTML, CSS, JS, images, fonts, JSON, PDF and similar static files are allowed (no server code). Limits: 25 MB per site, 500 files, 10 MB per file; dotfiles, symlinks and secret-looking files are refused. The call uploads only what changed, then reads the page back over the public internet and returns `live: true` plus the URL only when the published bytes came back. If it returns `live: false`, say the site is published but not yet reachable and give the reason — never say it is live. After success, answer the user with one short line that contains the link as `[<site title>](<public_url>)`.",
            "inputSchema": { "type": "object", "properties": {
                "app": { "type": "string", "description": "Site name: lower-case letters, digits and dashes (for example `portfolio`). Becomes the URL path and the folder `sites/<app>/`." },
                "dir": { "type": "string", "description": "Optional folder to publish instead of `sites/<app>/` (relative to the project folder). Must contain index.html." }
            }, "required": ["app"] }
        }),
        json!({
            "name": "unpublish_site",
            "description": "Take a published site offline (its public URL answers 404 within about 30 seconds). `all: true` takes every site of this owner down. The files in `sites/<app>/` stay on the box.",
            "inputSchema": { "type": "object", "properties": {
                "app": { "type": "string", "description": "Site name." },
                "all": { "type": "boolean", "description": "Take every published site of the owner down." }
            } }
        }),
        json!({
            "name": "list_sites",
            "description": "List the owner's published sites with their public URLs, sizes, versions and status, plus the owner's address prefix and the publishing limits.",
            "inputSchema": { "type": "object", "properties": {} }
        }),
    ]
}

/// `<project folder>/sites`.
pub fn sites_root(a: &Agent) -> PathBuf {
    a.cfg.work_dir.join("sites")
}

struct Packed {
    files: Vec<(SiteFile, PathBuf)>,
    skipped: Vec<String>,
    bytes: u64,
}

/// Walk `dir` (never following symlinks), apply the shared rules, hash every file.
fn pack(dir: &Path) -> Result<Packed, String> {
    let root = dir
        .canonicalize()
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    if !root.is_dir() {
        return Err(format!("{} is not a folder", dir.display()));
    }
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    let mut stack = vec![root.clone()];
    let mut bytes: u64 = 0;
    while let Some(d) = stack.pop() {
        let rd = std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))?;
        for entry in rd {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let rel = path
                .strip_prefix(&root)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            let meta = std::fs::symlink_metadata(&path).map_err(|e| format!("{rel}: {e}"))?;
            if meta.file_type().is_symlink() {
                skipped.push(format!("{rel} (symlink — never followed)"));
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                skipped.push(format!("{rel} (dotfile)"));
                continue;
            }
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            if !meta.is_file() {
                skipped.push(format!("{rel} (not a regular file)"));
                continue;
            }
            let ct = check_path(&rel).map_err(|e| e.0)?;
            if meta.len() > MAX_FILE_BYTES {
                return Err(format!(
                    "`{rel}` is {} MB; one file may be at most {} MB",
                    meta.len() / (1024 * 1024),
                    MAX_FILE_BYTES / (1024 * 1024)
                ));
            }
            let data = std::fs::read(&path).map_err(|e| format!("{rel}: {e}"))?;
            if ct.starts_with("text/")
                || ct.contains("json")
                || ct.contains("javascript")
                || ct.contains("xml")
            {
                let text = String::from_utf8_lossy(&data);
                if crate::store::redact(&text) != text {
                    return Err(format!(
                        "`{rel}` contains what looks like a secret or API key; remove it before publishing (sites are public)"
                    ));
                }
                if ct.starts_with("text/html") {
                    check_html(&rel, &text).map_err(|e| e.0)?;
                }
            }
            bytes = bytes.saturating_add(data.len() as u64);
            if bytes > MAX_SITE_BYTES {
                return Err(format!(
                    "the site is over {} MB in total; shrink images or drop files",
                    MAX_SITE_BYTES / (1024 * 1024)
                ));
            }
            if files.len() >= MAX_FILES {
                return Err(format!(
                    "more than {MAX_FILES} files; a site may have at most {MAX_FILES}"
                ));
            }
            files.push((
                SiteFile {
                    path: rel,
                    size: data.len() as u64,
                    sha256: hex::encode(Sha256::digest(&data)),
                },
                path,
            ));
        }
    }
    files.sort_by(|a, b| a.0.path.cmp(&b.0.path));
    let list: Vec<SiteFile> = files.iter().map(|(f, _)| f.clone()).collect();
    check_files(&list).map_err(|e| e.0)?;
    Ok(Packed {
        files,
        skipped,
        bytes,
    })
}

fn relay_error(what: &str, status: reqwest::StatusCode, body: &str) -> String {
    let msg = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| body.chars().take(300).collect());
    format!("{what}: HTTP {status}: {msg}")
}

fn site_dir(a: &Agent, app: &str, dir: Option<&str>) -> Result<PathBuf, String> {
    let root = a
        .cfg
        .work_dir
        .canonicalize()
        .unwrap_or_else(|_| a.cfg.work_dir.clone());
    let p = match dir.map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => {
            let raw = Path::new(d);
            if raw.is_absolute() {
                raw.to_path_buf()
            } else {
                a.cfg.work_dir.join(raw)
            }
        }
        None => sites_root(a).join(app),
    };
    let canon = p.canonicalize().map_err(|e| {
        format!(
            "{}: {e}. Put the site in `sites/{app}/` (with an index.html) and call publish_site again",
            p.display()
        )
    })?;
    if !canon.starts_with(&root) {
        return Err("the site folder must be inside the project folder".into());
    }
    Ok(canon)
}

/// `publish_site`: pack → begin → upload missing blobs → commit → read back over the public route.
pub async fn publish_site(a: &Arc<Agent>, args: &Value) -> Value {
    let app = args
        .get("app")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_ascii_lowercase())
        .unwrap_or_default();
    if let Err(e) = check_app(&app) {
        return err_result(e.0);
    }
    let dir = match site_dir(a, &app, args.get("dir").and_then(Value::as_str)) {
        Ok(d) => d,
        Err(e) => return err_result(e),
    };
    let packed = match pack(&dir) {
        Ok(p) => p,
        Err(e) => {
            return err_result(format!(
                "{e}. Allowed file types: {}.",
                allowed_extensions()
            ))
        }
    };
    let files: Vec<SiteFile> = packed.files.iter().map(|(f, _)| f.clone()).collect();
    let client = a.http_client();
    let base = a.cfg.relay_url.trim_end_matches('/');

    // 1. begin
    let resp = match client
        .post(format!("{base}/sites/{app}/begin"))
        .bearer_auth(&a.cfg.box_token)
        .header("x-arbos-project", &a.cfg.project_id)
        .json(&json!({ "files": files }))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return err_result(format!("relay unreachable: {e}")),
    };
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return err_result(relay_error("publish refused", status, &text));
    }
    let begin: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let missing: Vec<String> = begin
        .get("missing")
        .and_then(Value::as_array)
        .map(|v| {
            v.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let public_url = begin
        .get("public_url")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();

    // 2. blobs the relay does not have
    let mut uploaded = 0usize;
    for sha in &missing {
        let Some((f, path)) = packed.files.iter().find(|(f, _)| &f.sha256 == sha) else {
            continue;
        };
        let data = match tokio::fs::read(path).await {
            Ok(d) => d,
            Err(e) => return err_result(format!("{}: {e}", f.path)),
        };
        let r = client
            .put(format!("{base}/sites/{app}/blob/{sha}"))
            .bearer_auth(&a.cfg.box_token)
            .header("x-arbos-project", &a.cfg.project_id)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .body(data)
            .send()
            .await;
        match r {
            Ok(r) if r.status().is_success() => uploaded += 1,
            Ok(r) => {
                let st = r.status();
                let body = r.text().await.unwrap_or_default();
                return err_result(relay_error(
                    &format!("upload of `{}` refused", f.path),
                    st,
                    &body,
                ));
            }
            Err(e) => return err_result(format!("upload of `{}` failed: {e}", f.path)),
        }
    }

    // 3. commit
    let resp = match client
        .post(format!("{base}/sites/{app}/commit"))
        .bearer_auth(&a.cfg.box_token)
        .header("x-arbos-project", &a.cfg.project_id)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return err_result(format!("relay unreachable at commit: {e}")),
    };
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return err_result(relay_error("commit refused", status, &text));
    }
    let commit: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let public_url = commit
        .get("public_url")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or(public_url);
    let version = commit.get("version").and_then(Value::as_i64).unwrap_or(0);
    let edge = commit.get("edge").and_then(Value::as_bool).unwrap_or(false);
    let edge_error = commit
        .get("edge_error")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let index_sha = files
        .iter()
        .find(|f| f.path == "index.html")
        .map(|f| f.sha256.clone())
        .unwrap_or_default();

    // 4. read-back over the public route: the bytes we published must come back.
    let mut live = false;
    let mut read_back_ms: Option<u128> = None;
    let mut last_err = String::new();
    if edge && !public_url.is_empty() {
        let started = Instant::now();
        let rb = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .ok();
        while started.elapsed() < READ_BACK_WAIT {
            if let Some(rb) = &rb {
                match rb
                    .get(format!(
                        "{public_url}?arbos_rb={}",
                        arbos_core::ids::now_ms()
                    ))
                    .header(header::CACHE_CONTROL, "no-cache")
                    .send()
                    .await
                {
                    Ok(r) if r.status().is_success() => {
                        let body = r.bytes().await.unwrap_or_default();
                        if hex::encode(Sha256::digest(&body)) == index_sha {
                            live = true;
                            read_back_ms = Some(started.elapsed().as_millis());
                            break;
                        }
                        last_err = "the public route still serves the previous version".into();
                    }
                    Ok(r) => last_err = format!("public route answered HTTP {}", r.status()),
                    Err(e) => last_err = format!("public route not reachable: {e}"),
                }
            }
            tokio::time::sleep(READ_BACK_EVERY).await;
        }
    } else if !edge {
        last_err = edge_error
            .clone()
            .unwrap_or_else(|| "the public route is not configured on this relay".into());
    }
    tracing::info!(
        app,
        version,
        live,
        uploaded,
        files = files.len(),
        bytes = packed.bytes,
        "publish_site"
    );

    let title = html_title(&dir.join("index.html")).unwrap_or_else(|| app.clone());
    let mut out = json!({
        "published": true,
        "live": live,
        "public_url": public_url,
        "user_url": commit.get("user_url"),
        "app": app,
        "version": version,
        "files": files.len(),
        "bytes": packed.bytes,
        "uploaded": uploaded,
        "skipped": packed.skipped,
        "title": title,
    });
    if live {
        out["read_back_ms"] = json!(read_back_ms);
        out["next"] = json!(format!(
            "The site is live (read back over the public route in {} ms). Tell the user in one short line with the link: [{}]({}).",
            read_back_ms.unwrap_or(0), title, public_url
        ));
    } else {
        out["reason"] = json!(last_err);
        out["next"] = json!("Published on the relay but NOT confirmed live: the public route did not return the new page. Tell the user it is published but not reachable yet and why; do not call it live. Try publish_site again in a minute to re-check.");
    }
    text_result(out.to_string())
}

fn html_title(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let lower = text.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open_end = lower[start..].find('>')? + start + 1;
    let close = lower[open_end..].find("</title>")? + open_end;
    let t: String = text[open_end..close]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let t = t.replace(['[', ']', '(', ')'], "");
    if t.is_empty() || t.chars().count() > 80 {
        None
    } else {
        Some(t)
    }
}

/// `unpublish_site`.
pub async fn unpublish_site(a: &Arc<Agent>, args: &Value) -> Value {
    let base = a.cfg.relay_url.trim_end_matches('/');
    let all = args.get("all").and_then(Value::as_bool).unwrap_or(false);
    let url = if all {
        format!("{base}/sites?all=1")
    } else {
        let app = args
            .get("app")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_ascii_lowercase())
            .unwrap_or_default();
        if let Err(e) = check_app(&app) {
            return err_result(e.0);
        }
        format!("{base}/sites/{app}")
    };
    match a
        .http_client()
        .delete(url)
        .bearer_auth(&a.cfg.box_token)
        .header("x-arbos-project", &a.cfg.project_id)
        .send()
        .await
    {
        Ok(r) => {
            let st = r.status();
            let body = r.text().await.unwrap_or_default();
            if st.is_success() {
                text_result(format!(
                    "{body}\nThe public address stops answering within about 30 seconds. Tell the user in one line."
                ))
            } else {
                err_result(relay_error("unpublish refused", st, &body))
            }
        }
        Err(e) => err_result(format!("relay unreachable: {e}")),
    }
}

/// `list_sites`.
pub async fn list_sites(a: &Arc<Agent>) -> Value {
    let base = a.cfg.relay_url.trim_end_matches('/');
    match a
        .http_client()
        .get(format!("{base}/sites"))
        .bearer_auth(&a.cfg.box_token)
        .header("x-arbos-project", &a.cfg.project_id)
        .send()
        .await
    {
        Ok(r) => {
            let st = r.status();
            let body = r.text().await.unwrap_or_default();
            if st.is_success() {
                text_result(body)
            } else {
                err_result(relay_error("list_sites", st, &body))
            }
        }
        Err(e) => err_result(format!("relay unreachable: {e}")),
    }
}

// ---- preview server (loopback) ------------------------------------------------------------------

type S = Arc<Agent>;

fn not_found(what: &str) -> Response {
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>Not found</title><style>body{{font:16px system-ui;margin:3rem;color:#333}}</style><h1>404</h1><p>{} is not part of this site.</p>",
        what.replace('<', "&lt;").replace('>', "&gt;")
    );
    (
        StatusCode::NOT_FOUND,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
        .into_response()
}

async fn serve_file(a: &Agent, app: &str, rest: &str) -> Response {
    if check_app(app).is_err() {
        return not_found("that site name");
    }
    let Some(key) = resolve_request_path(rest) else {
        return not_found(rest);
    };
    let root = match sites_root(a).join(app).canonicalize() {
        Ok(r) => r,
        Err(_) => return not_found(app),
    };
    let path = root.join(&key);
    let canon = match path.canonicalize() {
        Ok(c) => c,
        Err(_) => return not_found(&key),
    };
    if !canon.starts_with(&root) {
        return not_found(&key);
    }
    let Ok(meta) = std::fs::symlink_metadata(&path) else {
        return not_found(&key);
    };
    if meta.file_type().is_symlink() || !canon.is_file() {
        return not_found(&key);
    }
    let ct = match check_path(&key) {
        Ok(ct) => ct,
        Err(_) => return not_found(&key),
    };
    match tokio::fs::read(&canon).await {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, ct.to_owned()),
                (header::CACHE_CONTROL, "no-store".to_owned()),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
            ],
            Body::from(bytes),
        )
            .into_response(),
        Err(_) => not_found(&key),
    }
}

async fn preview_root(State(a): State<S>, AxPath(app): AxPath<String>) -> Response {
    serve_file(&a, &app, "").await
}

async fn preview_path(
    State(a): State<S>,
    AxPath((app, rest)): AxPath<(String, String)>,
) -> Response {
    serve_file(&a, &app, &rest).await
}

pub fn preview_router(agent: Arc<Agent>) -> Router {
    Router::new()
        .route("/sites/{app}", get(preview_root))
        .route("/sites/{app}/", get(preview_root))
        .route("/sites/{app}/{*rest}", get(preview_path))
        .with_state(agent)
}
