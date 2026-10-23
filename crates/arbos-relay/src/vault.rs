//! `POST /egress` and the F8 `/vault/*` compatibility shims. The data model moved to
//! `secrets.rs` (D83): a box never receives a value; the agent gets a placeholder
//! `arbos-secret://<ENV>/<field>` and sends requests through `/egress`, where the relay swaps the
//! placeholder for the real value only when the project's Access is on, the destination host is
//! allowed and the user has not paused secrets; the reply comes back with every value redacted.
//!
//! The `/vault/*` routes answer the current agentd and Mac builds in their old shapes on top of
//! the new tables; they go away one release after `/box/secrets` and `/secrets` ship everywhere.
//!
//! `ARBOS_VAULT_MOCK=1` seeds two secrets for the pair-code user and serves an httpbin-style echo
//! target at `/vault/mock/echo`, so the whole flow runs on one machine.

use std::collections::BTreeMap;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::Principal;
use crate::secrets::{self, err, ApiErr, Refusal};
use crate::state::AppState;

type ApiResult = Result<Response, ApiErr>;

pub const PLACEHOLDER_SCHEME: &str = "arbos-secret://";
pub const REDACTED: &str = "[REDACTED:vault]";
const EGRESS_TIMEOUT: Duration = Duration::from_secs(30);
const EGRESS_MAX_BODY: usize = 1024 * 1024;
/// Mock secrets (`ARBOS_VAULT_MOCK=1`): demo values, host-restricted to this machine.
const MOCK_ITEMS: [(&str, &str); 2] = [("demo-login", "login"), ("demo-api-key", "key")];

fn internal(e: impl std::fmt::Display) -> ApiErr {
    err(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string())
}

fn bad(e: impl std::fmt::Display) -> ApiErr {
    err(StatusCode::BAD_REQUEST, "bad_request", e.to_string())
}

// ---- host allow-list and placeholder substitution (pure; the invariant tests cover these) -----

/// `host` matches an allow-list entry exactly or through a leading `*.` wildcard; ports are ignored.
pub fn host_allowed(host: &str, allowed: &[String]) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    allowed.iter().any(|a| {
        let a = a.trim().to_ascii_lowercase();
        let a = a.split(':').next().unwrap_or("").to_owned();
        if a.is_empty() {
            return false;
        }
        if let Some(suffix) = a.strip_prefix("*.") {
            host == suffix || host.ends_with(&format!(".{suffix}"))
        } else {
            host == a
        }
    })
}

/// A secret with its plaintext, as the substitution step needs it.
pub struct Resolved {
    pub name: String,
    pub allowed_hosts: Vec<String>,
    pub fields: BTreeMap<String, String>,
    /// The project may use it for this request (the rule in `secrets::decide_use` said yes).
    pub approved: bool,
}

/// Every `arbos-secret://item/field` occurrence in `s`, in order of appearance.
pub fn find_placeholders(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find(PLACEHOLDER_SCHEME) {
        let after = &rest[i + PLACEHOLDER_SCHEME.len()..];
        let end = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/')))
            .unwrap_or(after.len());
        let token = &after[..end];
        if let Some((item, field)) = token.split_once('/') {
            if !item.is_empty() && !field.is_empty() {
                out.push((item.to_owned(), field.to_owned()));
            }
        }
        rest = &after[end..];
    }
    out
}

#[derive(Default)]
pub struct Substitution {
    /// Placeholders swapped for the real value (`item/field`).
    pub substituted: Vec<String>,
    /// Placeholders left as they were, with the reason.
    pub refused: Vec<(String, String)>,
    /// Plaintext values that went out and must never come back.
    pub secrets: Vec<String>,
}

/// Replace placeholders in `text` for `host`. A placeholder is swapped only when the item exists,
/// the project holds an approval for it, and `host` is on the item's allow-list; everything else
/// stays verbatim so a non-allowed destination only ever sees the placeholder.
pub fn substitute(text: &str, host: &str, items: &[Resolved], sub: &mut Substitution) -> String {
    let mut out = text.to_owned();
    for (item, field) in find_placeholders(text) {
        let key = format!("{item}/{field}");
        let ph = secrets::placeholder(&item, &field);
        let Some(res) = items.iter().find(|r| r.name == item) else {
            if !sub.refused.iter().any(|(k, _)| *k == key) {
                sub.refused.push((key, "no such item".into()));
            }
            continue;
        };
        let Some(value) = res.fields.get(&field) else {
            sub.refused.push((key, "no such field".into()));
            continue;
        };
        if !res.approved {
            sub.refused.push((key, "no access for this project".into()));
            continue;
        }
        if !host_allowed(host, &res.allowed_hosts) {
            sub.refused.push((
                key,
                format!(
                    "host {host} not in allowed_hosts {}",
                    res.allowed_hosts.join(", ")
                ),
            ));
            continue;
        }
        out = out.replace(&ph, value);
        if !sub.substituted.contains(&key) {
            sub.substituted.push(key);
        }
        if !sub.secrets.contains(value) {
            sub.secrets.push(value.clone());
        }
    }
    out
}

/// Replace every secret with `[REDACTED:vault]` in all strings of `v` (keys included).
pub fn redact_value(v: &mut Value, secrets: &[String]) {
    if secrets.iter().all(String::is_empty) {
        return;
    }
    match v {
        Value::String(s) => {
            for sec in secrets.iter().filter(|s| !s.is_empty()) {
                if s.contains(sec.as_str()) {
                    *s = s.replace(sec.as_str(), REDACTED);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|x| redact_value(x, secrets)),
        Value::Object(map) => {
            let mut renamed = Vec::new();
            for (k, val) in map.iter_mut() {
                redact_value(val, secrets);
                if secrets
                    .iter()
                    .any(|s| !s.is_empty() && k.contains(s.as_str()))
                {
                    renamed.push(k.clone());
                }
            }
            for k in renamed {
                if let Some(val) = map.remove(&k) {
                    map.insert(REDACTED.to_owned(), val);
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

// ---- egress -------------------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct EgressBody {
    pub url: String,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// String body, or JSON (serialised) when an object/array.
    #[serde(default)]
    pub body: Option<Value>,
    /// Optional: the placeholders the caller intends to use (reported back, not required).
    #[serde(default)]
    pub placeholders: Vec<String>,
    /// The chat the tool call runs in (chat-scoped Access).
    #[serde(default, alias = "chatId")]
    pub chat_id: Option<String>,
    /// Human directing this turn (`user_id`). Absent = owner / unknown.
    #[serde(default, alias = "askedBy")]
    pub asked_by: Option<String>,
}

/// `POST /egress` (box): perform an HTTP request on the box's behalf with placeholders swapped for
/// allowed hosts, and return the reply with every substituted value redacted. 403 `paused` while
/// the kill switch is on; refused placeholders stay verbatim and are listed with the reason.
pub async fn egress(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
    Json(b): Json<EgressBody>,
) -> ApiResult {
    let (user_id, project_id) = secrets::box_project(&state, &principal, &headers)?;
    let url = reqwest::Url::parse(b.url.trim()).map_err(|e| bad(format!("url: {e}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(bad("url must be http(s)"));
    }
    let host = url
        .host_str()
        .ok_or_else(|| bad("url has no host"))?
        .to_owned();
    let method = Method::from_bytes(
        b.method
            .as_deref()
            .unwrap_or("GET")
            .trim()
            .to_ascii_uppercase()
            .as_bytes(),
    )
    .map_err(|_| bad("method"))?;
    if !find_placeholders(url.as_str()).is_empty() {
        return Err(bad(
            "placeholders are substituted in headers and body only, never in the URL",
        ));
    }
    if state
        .db
        .secrets_settings(&user_id)
        .map_err(internal)?
        .paused
    {
        return Err(err(
            StatusCode::FORBIDDEN,
            "paused",
            Refusal::Paused.message("", ""),
        ));
    }
    let body_text = match &b.body {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(other) => Some(other.to_string()),
    };
    let texts: Vec<&str> = b
        .headers
        .values()
        .map(String::as_str)
        .chain(body_text.as_deref())
        .collect();
    let chat_id = b.chat_id.as_deref().filter(|c| !c.is_empty());
    let asked_by = b
        .asked_by
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty());
    let plan = secrets::plan_egress(
        &state,
        &user_id,
        &project_id,
        &host,
        chat_id,
        asked_by,
        &texts,
    )?;
    // Connected vaults: `op://…` references resolve on the relay too (Phase 2).
    let op_plan =
        crate::vaults::plan_egress(&state, &user_id, &project_id, &host, chat_id, &texts).await;
    let mut sub = Substitution {
        refused: plan.refused.clone(),
        ..Default::default()
    };
    sub.refused.extend(op_plan.refused.iter().cloned());
    let swap_refs = |text: &str, sub: &mut Substitution| -> String {
        let mut out = text.to_owned();
        for (r, value) in &op_plan.values {
            if out.contains(r.as_str()) {
                out = out.replace(r.as_str(), value);
                if !sub.substituted.contains(r) {
                    sub.substituted.push(r.clone());
                }
            }
        }
        out
    };
    let mut headers = BTreeMap::new();
    for (k, v) in &b.headers {
        let v = substitute(v, &host, &plan.items, &mut sub);
        headers.insert(k.clone(), swap_refs(&v, &mut sub));
    }
    let body_out = body_text.as_deref().map(|t| {
        let t = substitute(t, &host, &plan.items, &mut sub);
        swap_refs(&t, &mut sub)
    });
    for (s, allowed) in &plan.used {
        if sub
            .substituted
            .iter()
            .any(|k| k.starts_with(&format!("{}/", s.env)))
        {
            secrets::after_use(
                &state,
                s,
                &project_id,
                &host,
                "header",
                chat_id,
                asked_by,
                allowed,
            );
        }
    }
    for (r, allowed) in &op_plan.used {
        if sub.substituted.contains(r) {
            crate::vaults::after_op_use(&state, r, &project_id, &host, "header", chat_id, allowed);
        }
    }
    let mut redact = plan.redact;
    for v in &sub.secrets {
        if !redact.contains(v) {
            redact.push(v.clone());
        }
    }
    for (_, v) in &op_plan.values {
        if !redact.contains(v) {
            redact.push(v.clone());
        }
    }
    let refused: Vec<Value> = sub
        .refused
        .iter()
        .map(|(p, r)| json!({ "placeholder": p, "reason": r }))
        .collect();

    let mut req = state.http.request(method.clone(), url.clone());
    req = req.timeout(EGRESS_TIMEOUT);
    for (k, v) in &headers {
        req = req.header(k.as_str(), v.as_str());
    }
    if let Some(body) = body_out {
        req = req.body(body);
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            let mut v = json!({ "ok": false, "error": format!("{host}: {e}"), "substituted": sub.substituted, "refused": refused });
            redact_value(&mut v, &redact);
            return Ok((StatusCode::BAD_GATEWAY, Json(v)).into_response());
        }
    };
    let status = resp.status();
    let mut resp_headers = BTreeMap::new();
    for (k, v) in resp.headers() {
        if let Ok(s) = v.to_str() {
            resp_headers.insert(k.as_str().to_owned(), s.to_owned());
        }
    }
    let bytes = resp.bytes().await.unwrap_or_default();
    let truncated = bytes.len() > EGRESS_MAX_BODY;
    let slice = &bytes[..bytes.len().min(EGRESS_MAX_BODY)];
    let body_json: Value = match std::str::from_utf8(slice) {
        Ok(text) => Value::String(text.to_owned()),
        Err(_) => json!({ "base64": B64.encode(slice) }),
    };
    let mut v = json!({
        "ok": status.is_success(), "status": status.as_u16(), "headers": resp_headers, "body": body_json,
        "truncated": truncated, "substituted": sub.substituted, "refused": refused,
        "requested_placeholders": b.placeholders,
    });
    redact_value(&mut v, &redact);
    Ok(Json(v).into_response())
}

// ---- F8 compatibility shims --------------------------------------------------------------------

/// The F8 item shape, built from a secret: names, kind, hosts, field names, placeholders, env.
fn legacy_item_json(item: &Value) -> Value {
    let kind = match item["kind"].as_str() {
        Some("login") => "login",
        Some("text") => "secret",
        _ => "api_key",
    };
    json!({
        "id": item["id"], "name": item["env"], "kind": kind, "allowed_hosts": item["hosts"],
        "fields": item["fields"], "placeholders": item["placeholders"], "env": item["env"],
        "created_at": item["created_at"], "last_used_at": item["last_used_at"],
        "daytona": false, "mock": false, "enabled": item["enabled"], "note": item["note"],
    })
}

fn legacy_status(status: &str) -> &'static str {
    match status {
        "pending" => "pending",
        "allowed_once" | "allowed_chat" | "allowed_always" => "approved",
        "denied" => "denied",
        _ => "expired",
    }
}

fn legacy_request_json(r: &Value) -> Value {
    json!({
        "id": r["id"], "project_id": r["project_id"], "chat_id": r["chat_id"], "item_id": r["target"]["id"],
        "item_name": r["name"], "purpose": r["purpose"], "use": r["use"],
        "status": legacy_status(r["status"].as_str().unwrap_or("expired")),
        "created_at": r["created_at"], "decided_at": r["decided_at"], "expires_at": r["expires_at"],
    })
}

/// `GET /vault/items` (device and box): the caller's secrets in the F8 shape.
pub async fn list_items(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
) -> ApiResult {
    let (user_id, project_id) = match &principal {
        Principal::Box { .. } => secrets::box_project(&state, &principal, &headers)?,
        _ => (secrets::device_user(&principal)?, String::new()),
    };
    let items: Vec<Value> = secrets::box_items(&state, &user_id, &project_id)
        .iter()
        .filter(|i| i["vault_name"] == secrets::ARBOS_VAULT_NAME)
        .map(legacy_item_json)
        .collect();
    Ok(Json(json!({ "mock": state.cfg.vault_mock, "items": items })).into_response())
}

#[derive(Deserialize, Default)]
pub struct ItemBody {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    /// `{ "url", "username", "password" }` or `{ "value" }`; absent on update keeps the stored one.
    #[serde(default)]
    pub fields: Option<Value>,
    #[serde(default, alias = "allowedHosts")]
    pub allowed_hosts: Option<Vec<String>>,
}

/// `POST /vault/items` (device): create a secret, or replace the value and hosts of one with
/// the same name (the F8 "create or replace by name").
pub async fn create_item(
    State(state): State<AppState>,
    principal: Principal,
    Json(b): Json<ItemBody>,
) -> ApiResult {
    let user_id = secrets::device_user(&principal)?;
    let name = b.name.clone().unwrap_or_default();
    let kind = match b.kind.as_deref() {
        Some("login") => "login",
        _ => "key",
    };
    let fields = b.fields.clone().unwrap_or(json!({}));
    let existing = state
        .db
        .secret_by_name(&user_id, name.trim())
        .map_err(internal)?;
    let body = if kind == "login" {
        json!({ "name": name, "kind": kind, "fields": fields, "hosts": b.allowed_hosts })
    } else {
        json!({ "name": name, "kind": kind, "value": fields.get("value"), "hosts": b.allowed_hosts })
    };
    let s = match existing {
        None => {
            let created: secrets::CreateBody = serde_json::from_value(body).map_err(bad)?;
            let resp =
                secrets::create(State(state.clone()), principal.clone(), Json(created)).await?;
            let v = response_json(resp).await?;
            v["secret"].clone()
        }
        Some(s) => {
            if let Some(hosts) = b.allowed_hosts.clone() {
                let patch = secrets::PatchBody {
                    hosts: Some(hosts),
                    ..Default::default()
                };
                secrets::patch(
                    State(state.clone()),
                    principal.clone(),
                    Path(s.id.clone()),
                    Json(patch),
                )
                .await?;
            }
            let resp = secrets::replace_value(
                State(state.clone()),
                principal.clone(),
                Path(s.id.clone()),
                Json(body),
            )
            .await?;
            let v = response_json(resp).await?;
            v["secret"].clone()
        }
    };
    let items = secrets::box_items(&state, &user_id, "");
    let item = items
        .iter()
        .find(|i| i["id"] == s["id"])
        .map(legacy_item_json)
        .unwrap_or(Value::Null);
    Ok((StatusCode::CREATED, Json(json!({ "item": item }))).into_response())
}

async fn response_json(resp: Response) -> Result<Value, ApiErr> {
    let bytes = axum::body::to_bytes(resp.into_body(), 4 * 1024 * 1024)
        .await
        .map_err(internal)?;
    serde_json::from_slice(&bytes).map_err(internal)
}

/// `PUT /vault/items/{id}` (device): hosts and/or a new value.
pub async fn update_item(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<ItemBody>,
) -> ApiResult {
    let user_id = secrets::device_user(&principal)?;
    if let Some(hosts) = b.allowed_hosts {
        let patch = secrets::PatchBody {
            hosts: Some(hosts),
            ..Default::default()
        };
        secrets::patch(
            State(state.clone()),
            principal.clone(),
            Path(id.clone()),
            Json(patch),
        )
        .await?;
    }
    if let Some(fields) = b.fields {
        let s = state
            .db
            .secret(&user_id, &id)
            .map_err(internal)?
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found", "no such item"))?;
        let body = if s.kind == "login" {
            json!({ "fields": fields })
        } else {
            json!({ "value": fields.get("value") })
        };
        secrets::replace_value(
            State(state.clone()),
            principal.clone(),
            Path(id.clone()),
            Json(body),
        )
        .await?;
    }
    let item = secrets::box_items(&state, &user_id, "")
        .iter()
        .find(|i| i["id"].as_str() == Some(id.as_str()))
        .map(legacy_item_json)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found", "no such item"))?;
    Ok(Json(json!({ "item": item })).into_response())
}

pub async fn delete_item(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> ApiResult {
    secrets::delete(State(state), principal, Path(id)).await
}

/// `GET /vault/requests` (device): the caller's requests in the F8 shape.
pub async fn list_requests(State(state): State<AppState>, principal: Principal) -> ApiResult {
    let resp = secrets::list_requests(State(state.clone()), principal).await?;
    let v = response_json(resp).await?;
    let items: Vec<Value> = v["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(legacy_request_json)
        .collect();
    Ok(Json(json!({ "mock": state.cfg.vault_mock, "items": items })).into_response())
}

#[derive(Deserialize)]
pub struct DecideBody {
    pub approve: bool,
}

/// `POST /vault/requests/{id}/decide { approve }` (device): approve = `always`, else `deny`.
pub async fn decide_request(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<DecideBody>,
) -> ApiResult {
    let user_id = secrets::device_user(&principal)?;
    let decision = if b.approve { "always" } else { "deny" };
    let (rq, _) = secrets::decide(&state, &user_id, &id, decision, &format!("card:{id}")).await?;
    let v = secrets::request_json(&state, &rq);
    Ok(Json(json!({ "request": legacy_request_json(&v), "daytona": null })).into_response())
}

#[derive(Deserialize)]
pub struct RequestBody {
    pub name: String,
    #[serde(default)]
    pub purpose: String,
    #[serde(rename = "use")]
    pub use_kind: String,
    #[serde(default, rename = "chatId", alias = "chat_id")]
    pub chat_id: String,
}

/// `POST /vault/request` (box): the F8 ask. Returns the request in the F8 shape (201).
pub async fn create_request(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
    Json(b): Json<RequestBody>,
) -> ApiResult {
    let body = secrets::BoxRequestBody {
        reference: None,
        name: Some(b.name),
        purpose: b.purpose,
        use_kind: Some(b.use_kind),
        host: None,
        chat_id: b.chat_id,
        asked_by: None,
    };
    let resp = secrets::box_request(State(state), principal, headers, Json(body)).await?;
    let v = response_json(resp).await?;
    Ok((
        StatusCode::CREATED,
        Json(legacy_request_json(&v["request"])),
    )
        .into_response())
}

/// `GET /vault/requests/{id}` (box: own project; device: own user): status, never values.
pub async fn request_status(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    let rq = state
        .db
        .secret_request(&id)
        .map_err(internal)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found", "no such request"))?;
    match &principal {
        Principal::Box { .. } => {
            let (_u, project_id) = secrets::box_project(&state, &principal, &headers)?;
            if rq.project_id != project_id {
                return Err(err(StatusCode::FORBIDDEN, "not_owner", "not your project"));
            }
        }
        Principal::Device { user_id, .. } => {
            if rq.user_id != *user_id {
                return Err(err(StatusCode::FORBIDDEN, "not_owner", "not your request"));
            }
        }
        Principal::Admin => {}
    }
    Ok(Json(legacy_request_json(&secrets::request_json(&state, &rq))).into_response())
}

/// `POST /vault/requests/{id}/values` (box): login fields for an approved `browser_fill`.
pub async fn request_values(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    secrets::box_request_values(State(state), principal, headers, Path(id)).await
}

// ---- mock ----------------------------------------------------------------------------------------

/// `ARBOS_VAULT_MOCK=1`: two demo secrets whose hosts are this machine, for the pair-code user,
/// so `/egress` against the relay's own echo target shows the substitution end to end.
pub fn seed_mock(state: &AppState) -> anyhow::Result<()> {
    if !state.cfg.vault_mock {
        return Ok(());
    }
    const MOCK_USER: &str = "u_default";
    let echo = format!("{}/vault/mock/echo", state.cfg.public_url);
    let hosts = vec!["127.0.0.1".to_owned(), "localhost".to_owned()];
    for (name, kind) in MOCK_ITEMS {
        if state.db.secret_by_name(MOCK_USER, name)?.is_some() {
            continue;
        }
        let body = match kind {
            "login" => json!({ "name": name, "kind": kind, "hosts": hosts,
                "fields": { "url": echo, "username": "demo@arbos.local", "password": format!("mock-pw-{}", arbos_core::ids::ulid().to_lowercase()) } }),
            _ => {
                json!({ "name": name, "kind": kind, "hosts": hosts, "value": format!("mock-key-{}", arbos_core::ids::ulid().to_lowercase()) })
            }
        };
        let created: secrets::CreateBody = serde_json::from_value(body)?;
        let principal = Principal::Device {
            id: "mock".into(),
            token: String::new(),
            user_id: MOCK_USER.into(),
        };
        let state = state.clone();
        tokio::spawn(async move {
            let _ = secrets::create(State(state), principal, Json(created)).await;
        });
    }
    tracing::info!(target = %echo, "ARBOS_VAULT_MOCK=1: seeded demo-login and demo-api-key (hosts 127.0.0.1, localhost)");
    Ok(())
}

/// `ANY /vault/mock/echo`: an httpbin-style target that reflects method, headers and body. No auth;
/// enabled only in mock mode. Shows what a real destination would have received.
pub async fn mock_echo(
    State(state): State<AppState>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult {
    if !state.cfg.vault_mock {
        return Err(err(StatusCode::NOT_FOUND, "not_found", "not in mock mode"));
    }
    let hdrs: BTreeMap<String, String> = headers
        .iter()
        .filter_map(|(k, v)| {
            v.to_str()
                .ok()
                .map(|s| (k.as_str().to_owned(), s.to_owned()))
        })
        .collect();
    let body_text = String::from_utf8_lossy(&body).into_owned();
    let authorized = hdrs
        .get("authorization")
        .is_some_and(|a| a.starts_with("Bearer mock-key-") || a.starts_with("Bearer "));
    Ok(Json(json!({
        "echo": true, "method": method.as_str(), "headers": hdrs, "body": body_text,
        "authorized": authorized, "received_placeholder": body_text.contains(PLACEHOLDER_SCHEME)
            || hdrs.values().any(|v| v.contains(PLACEHOLDER_SCHEME)),
    }))
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<Resolved> {
        vec![
            Resolved {
                name: "acme".into(),
                allowed_hosts: vec!["api.acme.test".into(), "*.acme.internal".into()],
                fields: BTreeMap::from([("value".to_owned(), "s3cr3t-VALUE-9f8e7d".to_owned())]),
                approved: true,
            },
            Resolved {
                name: "unapproved".into(),
                allowed_hosts: vec!["api.acme.test".into()],
                fields: BTreeMap::from([("value".to_owned(), "never-sent-1234".to_owned())]),
                approved: false,
            },
        ]
    }

    /// Invariant (F8): a placeholder is never substituted for a host outside `allowed_hosts`
    /// (nor for an item the project has no approval for); the destination only ever sees the
    /// placeholder text.
    #[test]
    fn placeholder_not_substituted_for_non_allowed_host() {
        let items = items();
        let header = "Bearer arbos-secret://acme/value";
        for host in [
            "evil.example",
            "acme.test",
            "api.acme.test.evil.example",
            "notacme.internal",
        ] {
            let mut sub = Substitution::default();
            let out = substitute(header, host, &items, &mut sub);
            assert_eq!(out, header, "{host} must keep the placeholder");
            assert!(
                sub.substituted.is_empty() && sub.secrets.is_empty(),
                "{host}"
            );
            assert_eq!(sub.refused.len(), 1);
        }
        // Allowed hosts (exact and wildcard) do substitute.
        for host in ["api.acme.test", "API.ACME.TEST", "db.acme.internal"] {
            let mut sub = Substitution::default();
            let out = substitute(header, host, &items, &mut sub);
            assert_eq!(out, "Bearer s3cr3t-VALUE-9f8e7d", "{host}");
            assert_eq!(sub.substituted, vec!["acme/value".to_owned()]);
        }
        // No approval: allowed host, still refused.
        let mut sub = Substitution::default();
        let out = substitute(
            "x arbos-secret://unapproved/value",
            "api.acme.test",
            &items,
            &mut sub,
        );
        assert_eq!(out, "x arbos-secret://unapproved/value");
        assert!(sub.refused[0].1.contains("no access"));
        // Placeholders in a JSON body, several at once.
        let mut sub = Substitution::default();
        let body = r#"{"k":"arbos-secret://acme/value","again":"arbos-secret://acme/value","missing":"arbos-secret://acme/nope"}"#;
        let out = substitute(body, "api.acme.test", &items, &mut sub);
        assert!(
            !out.contains("arbos-secret://acme/value") && out.contains("arbos-secret://acme/nope")
        );
        assert_eq!(sub.secrets, vec!["s3cr3t-VALUE-9f8e7d".to_owned()]);
    }

    /// Invariant (F8): the raw value never appears in an egress reply or in any item/request
    /// listing: every string (and key) carrying it is redacted, and the item JSON only names fields.
    #[test]
    fn raw_value_never_in_egress_response_or_listing() {
        let secret = "s3cr3t-VALUE-9f8e7d".to_owned();
        // An upstream that echoes everything it got, including the value in a header name.
        let mut reply = json!({
            "status": 200,
            "headers": { "x-echo": format!("Bearer {secret}"), secret.clone(): "as a key" },
            "body": format!("{{\"got\":\"{secret}\",\"nested\":[\"{secret}-suffix\"]}}"),
            "fine": "untouched",
        });
        redact_value(&mut reply, std::slice::from_ref(&secret));
        let text = reply.to_string();
        assert!(!text.contains(&secret), "{text}");
        assert!(text.contains(REDACTED));
        assert_eq!(reply["fine"], "untouched");
        assert_eq!(reply["headers"][REDACTED], "as a key");

        // The legacy item shape carries names, kind, hosts, field names, placeholders; no value.
        let item = json!({ "id": "i1", "env": "ACME", "kind": "key", "hosts": ["api.acme.test"], "fields": ["value"],
            "placeholders": { "value": "arbos-secret://ACME/value" }, "created_at": 0, "last_used_at": null, "enabled": true, "note": "" });
        let listing = legacy_item_json(&item).to_string();
        assert!(!listing.contains(&secret), "{listing}");
        assert!(listing.contains("arbos-secret://ACME/value"));
        assert_eq!(legacy_status("allowed_chat"), "approved");
        assert_eq!(legacy_status("denied"), "denied");
        assert_eq!(legacy_status("expired"), "expired");
    }
}
