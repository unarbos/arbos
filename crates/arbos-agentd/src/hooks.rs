//! Loopback hook API for `arbos-pi-ext` (sandbox-image.md §3.3): route validation,
//! ask_user, handles, working set, scratchpad.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::agent::Agent;
use crate::mcp::{refusal_for, SessionRole};

type S = Arc<Agent>;

/// Which role a bearer token earns, or `None` for no role at all.
///
/// `ARBOS_MCP_TOKEN` defaults to empty (`config.rs`), and a missing header reads as `""` too, so
/// an unconfigured daemon once matched the main arm for a request carrying no credential — the
/// highest role, granted anonymously. The subagent arm is no safer: `subagent_mcp_token()` is
/// `sub_` + sha256("subagent:" + token), a constant anyone can compute offline once the token it
/// derives from is empty. Both arms therefore fail closed before any comparison is reached. A box
/// always has a minted token, so this refuses nothing that was ever configured.
fn role_for_token(configured: &str, derived: &str, headers: &HeaderMap) -> Option<SessionRole> {
    if configured.trim().is_empty() {
        return None;
    }
    let tok = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("")
        .trim();
    if tok.is_empty() {
        return None;
    }
    if tok == configured {
        Some(SessionRole::Main)
    } else if tok == derived {
        Some(SessionRole::Subagent)
    } else {
        None
    }
}

/// Who is calling, derived from the bearer token as the MCP server derives it — never from a
/// request field, which the caller writes. `None` = no token we know.
fn authed(agent: &Agent, headers: &HeaderMap) -> Option<SessionRole> {
    role_for_token(
        &agent.cfg.mcp_token,
        &agent.cfg.subagent_mcp_token(),
        headers,
    )
}

fn role_name(role: SessionRole) -> &'static str {
    match role {
        SessionRole::Main => "main",
        SessionRole::Subagent => "subagent",
    }
}

fn ok() -> Response {
    Json(json!({ "ok": true })).into_response()
}

fn err(e: impl std::fmt::Display) -> Response {
    Json(json!({ "ok": false, "error": e.to_string() })).into_response()
}

async fn route(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    let Some(role) = authed(&a, &headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    // Only the main session routes; a subagent's call must never touch the running turn's state.
    if role != SessionRole::Main {
        return err("route is only for the main session");
    }
    match a.hook_route(b.get("args").unwrap_or(&Value::Null)).await {
        Ok(()) => ok(),
        Err(e) => err(e),
    }
}

async fn ask_user(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if authed(&a, &headers).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let q = b
        .get("question")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let sid = b
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    match a.hook_ask_user(&q, b.get("widget"), &sid).await {
        Ok(id) => Json(json!({ "ok": true, "question_id": id })).into_response(),
        Err(e) => err(e),
    }
}

/// `send_message(text, activity?, to?)`: progress bubble mid-turn (no `to`), or a
/// cross-subnet message to another agent when `to` is set — or when `text` is the
/// antipattern `@Handle words` (feedback app-01M5NX3GHC37Y8FH3KKR0PRWG1).
async fn start_job(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if authed(&a, &headers).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let s = |k: &str| b.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    let touches: Vec<String> = b
        .get("touches")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    match a
        .hook_start_job(&s("title"), &s("brief"), &s("kind"), touches)
        .await
    {
        Ok(v) => Json(json!({ "ok": true, "job": v })).into_response(),
        Err(e) => err(e),
    }
}

async fn steer_job(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if authed(&a, &headers).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let s = |k: &str| b.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    match a.hook_steer_job(&s("job_id"), &s("text")).await {
        Ok(v) => Json(json!({ "ok": true, "job": v })).into_response(),
        Err(e) => err(e),
    }
}

async fn stop_job(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if authed(&a, &headers).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let job_id = b
        .get("job_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    match a.hook_stop_job(&job_id).await {
        Ok(v) => Json(json!({ "ok": true, "result": v })).into_response(),
        Err(e) => err(e),
    }
}

async fn list_jobs(State(a): State<S>, headers: HeaderMap) -> Response {
    if authed(&a, &headers).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match a.hook_list_jobs().await {
        Ok(v) => Json(json!({ "ok": true, "jobs": v["jobs"] })).into_response(),
        Err(e) => err(e),
    }
}

async fn send_message(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    let Some(role) = authed(&a, &headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let text = b.get("text").and_then(Value::as_str).unwrap_or("");
    let activity = b
        .get("activity")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let to = b
        .get("to")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    // Explicit `to`, or `@Handle words` mistyped as a progress note in this chat.
    let (agent_to, agent_text) = match to {
        Some(t) => (Some(t), text.to_owned()),
        None => match crate::agent_messages::mention_address(text) {
            Some((handle, msg)) => (Some(handle), msg),
            None => (None, String::new()),
        },
    };
    if let Some(to) = agent_to {
        // D69 on this port too: only the main session speaks to another agent. The progress
        // note below stays open to a subagent — it reaches this chat only.
        if let Some(reason) = refusal_for(role, "send_message") {
            return err(reason);
        }
        return match a.send_message_from_turn(Some(&to), &agent_text).await {
            Ok(note) => Json(json!({ "ok": true, "note": note })).into_response(),
            Err(e) => err(e),
        };
    }
    let reply_to = b
        .get("reply_to")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match a.hook_send_message(text, activity, reply_to).await {
        Ok(()) => ok(),
        Err(e) => err(e),
    }
}

/// Risky harness tool calls (feature `approve`): `{ tool, input, session_id }` →
/// `{ state: "allowed" }` or `{ state: "pending", approvalId }` for the extension to poll.
/// The card's `role` is the token's, not the body's.
async fn approval(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    let Some(role) = authed(&a, &headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let str_of = |k: &str| b.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    let input = b.get("input").cloned().unwrap_or(Value::Null);
    Json(
        a.hook_approval(
            &str_of("tool"),
            &input,
            &str_of("session_id"),
            role_name(role),
        )
        .await,
    )
    .into_response()
}

async fn approval_status(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if authed(&a, &headers).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let id = b.get("approvalId").and_then(Value::as_str).unwrap_or("");
    Json(a.approval_status(id).await).into_response()
}

async fn approval_abandon(
    State(a): State<S>,
    headers: HeaderMap,
    Json(b): Json<Value>,
) -> Response {
    if authed(&a, &headers).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let id = b.get("approvalId").and_then(Value::as_str).unwrap_or("");
    Json(a.abandon_approval(id).await).into_response()
}

async fn deny(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if authed(&a, &headers).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let sid = b
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let _ = a.store.t3_append(
        &sid,
        None,
        Some(&sid),
        "deny",
        Some("system"),
        &format!("denied tool call: {b}"),
        None,
    );
    tracing::warn!(deny = %b, "deny-list hit");
    ok()
}

async fn handle(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if authed(&a, &headers).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let sid = b
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let h = b.get("handle").cloned().unwrap_or(Value::Null);
    let id = h.get("id").and_then(Value::as_str).unwrap_or("").to_owned();
    let path = h
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if id.is_empty() || path.is_empty() {
        return err("handle needs id and path");
    }
    let blob_key = std::fs::read(&path)
        .ok()
        .map(|bytes| a.upload_blob(bytes, "text/plain"));
    let r = a.store.upsert_handle(
        &id,
        Some(&sid),
        Some(&sid),
        h.get("kind").and_then(Value::as_str).unwrap_or("output"),
        h.get("chars").and_then(Value::as_i64).unwrap_or(0),
        h.get("lines").and_then(Value::as_i64).unwrap_or(0),
        &path,
        h.get("note").and_then(Value::as_str).unwrap_or(""),
        blob_key.as_deref(),
    );
    match r {
        Ok(()) => ok(),
        Err(e) => err(e),
    }
}

async fn handle_path(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if authed(&a, &headers).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let id = b.get("handle").and_then(Value::as_str).unwrap_or("");
    match a.store.handle(id) {
        Ok(Some(h)) => Json(json!({ "ok": true, "text": h.path })).into_response(),
        _ => err("unknown handle"),
    }
}

async fn working_set(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if authed(&a, &headers).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let sid = b
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let chat_id = a
        .turn
        .lock()
        .await
        .as_ref()
        .map(|t| t.chat_id.clone())
        .unwrap_or(sid);
    // Drop zombie `running` subagents (task gone, status never cleared) before
    // the model reads the working set — otherwise a terminated pod keeps the
    // agent thinking work is still in flight and pins the client's indicator.
    a.reclaim_orphan_subagents(false).await;
    // `skills` is its own prompt section (F5): the working-set line cap must not cut it.
    let skills = a.skills_section_text().await;
    // D87: project memory stays in the (debounced) system prompt; `memory` is reserved for a
    // later experiment and empty today.
    Json(json!({ "ok": true, "text": a.working_set_text(&chat_id), "skills": skills, "memory": "", "stable_prefix": a.cfg.stable_prefix() }))
        .into_response()
}

async fn remember(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if authed(&a, &headers).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let sid = b
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let chat_id = a
        .turn
        .lock()
        .await
        .as_ref()
        .map(|t| t.chat_id.clone())
        .unwrap_or(sid);
    let name = b.get("name").and_then(Value::as_str).unwrap_or("");
    let value = b.get("value").and_then(Value::as_str);
    match a.store.scratch_set(&chat_id, name, value) {
        Ok(()) => ok(),
        Err(e) => err(e),
    }
}

async fn noop(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if authed(&a, &headers).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    tracing::debug!(hook = %b, "hook event");
    ok()
}

async fn turn_without_route(
    State(a): State<S>,
    headers: HeaderMap,
    Json(b): Json<Value>,
) -> Response {
    if authed(&a, &headers).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let corrected = b.get("corrected").and_then(Value::as_bool).unwrap_or(false);
    let sid = b
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    a.hook_turn_without_route(&sid, corrected).await;
    ok()
}

async fn route_corrected(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if authed(&a, &headers).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    tracing::info!(action = ?b.get("action"), "route recorded by the post-hoc guard");
    if let Some(t) = a.turn.lock().await.as_mut() {
        t.metrics.route_corrected = true;
    }
    ok()
}

/// Memory tree (D77) node counts, build cursor and today's spend; for operators (`arbosctl`,
/// the roll-out check). Same bearer as the other hooks.
async fn tree_stats(State(a): State<S>, headers: HeaderMap) -> Response {
    if authed(&a, &headers).is_none() {
        return err("unauthorized");
    }
    let mut v = a.tree.stats();
    v["mode"] = json!(a.tree_mode().as_str());
    v["spend"] = a.tree.spend_report();
    Json(v).into_response()
}

pub fn router(agent: Arc<Agent>) -> Router {
    Router::new()
        .route("/hook/route", post(route))
        .route("/hook/ask_user", post(ask_user))
        .route("/hook/send_message", post(send_message))
        .route("/hook/start_job", post(start_job))
        .route("/hook/list_jobs", post(list_jobs))
        .route("/hook/steer_job", post(steer_job))
        .route("/hook/stop_job", post(stop_job))
        .route("/hook/approval", post(approval))
        .route("/hook/approval-status", post(approval_status))
        .route("/hook/approval-abandon", post(approval_abandon))
        .route("/hook/deny", post(deny))
        .route("/hook/handle", post(handle))
        .route("/hook/handle-path", post(handle_path))
        .route("/hook/working-set", post(working_set))
        .route("/hook/remember", post(remember))
        .route("/hook/compacted", post(noop))
        .route("/hook/turn-without-route", post(turn_without_route))
        .route("/hook/route-corrected", post(route_corrected))
        .route("/hook/tree-stats", axum::routing::get(tree_stats))
        .with_state(agent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::tests::{test_agent, test_agent_with};

    /// Every `to` the stub relay was asked to deliver (`POST /projects/{id}/agent-messages`).
    /// A log, not a last-value slot: the tests run in parallel, so each filters by its own text.
    static AGENT_SENDS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

    async fn mock_agent_message(Json(b): Json<Value>) -> Json<Value> {
        AGENT_SENDS
            .lock()
            .unwrap()
            .push(b["text"].as_str().unwrap_or("").to_owned());
        Json(
            json!({ "id": "am-test", "status": "delivered", "to": { "id": "p-other", "name": "Doug" } }),
        )
    }

    fn relay_saw(text: &str) -> usize {
        AGENT_SENDS
            .lock()
            .unwrap()
            .iter()
            .filter(|t| *t == text)
            .count()
    }

    fn bearer(token: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        h
    }

    /// Every hook route is behind `authed`, and `/hook/approval` decides whether a risky shell
    /// command runs. An unset token must authenticate nobody; it used to authenticate everybody,
    /// because a missing header and an unset token were both the empty string.
    #[test]
    fn an_unset_token_authenticates_nobody() {
        // `subagent_mcp_token()` is `sub_` + the first 16 bytes of sha256("subagent:" + token), so
        // with the token unset it is this constant: no secret in it, and any caller can compute it.
        let derived_from_nothing = "sub_14dabdbbbef7fd4458133cfe393c201a";

        assert!(
            role_for_token("", derived_from_nothing, &HeaderMap::new()).is_none(),
            "no Authorization header must not pass while the token is unset"
        );
        assert!(
            role_for_token("", derived_from_nothing, &bearer("")).is_none(),
            "an empty bearer must not pass"
        );
        assert!(
            role_for_token("", derived_from_nothing, &bearer("   ")).is_none(),
            "a whitespace bearer must not pass"
        );
        assert!(
            role_for_token("", derived_from_nothing, &bearer(derived_from_nothing)).is_none(),
            "the derived token must not pass while the token it derives from is unset"
        );
        assert!(
            role_for_token("", derived_from_nothing, &bearer("anything")).is_none(),
            "nothing passes while the token is unset"
        );
    }

    #[test]
    fn a_configured_token_passes_and_a_wrong_one_does_not() {
        assert_eq!(
            role_for_token("t0ken", "sub_abc", &bearer("t0ken")),
            Some(SessionRole::Main)
        );
        assert_eq!(
            role_for_token("t0ken", "sub_abc", &bearer("sub_abc")),
            Some(SessionRole::Subagent),
            "a subagent's derived token passes, as the subagent"
        );
        assert!(role_for_token("t0ken", "sub_abc", &bearer("nope")).is_none());
        assert!(role_for_token("t0ken", "sub_abc", &bearer("")).is_none());
        assert!(role_for_token("t0ken", "sub_abc", &HeaderMap::new()).is_none());
    }

    /// The constant above is really what the config derives, so the test is about the value an
    /// attacker would send rather than an arbitrary string.
    #[test]
    fn the_derived_token_for_an_unset_token_is_that_constant() {
        use sha2::Digest;
        let d = sha2::Sha256::digest(b"subagent:");
        assert_eq!(
            format!("sub_{}", hex::encode(&d[..16])),
            "sub_14dabdbbbef7fd4458133cfe393c201a"
        );
    }

    async fn body(r: Response) -> Value {
        let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    }

    /// Deriving the role must not hand one out for free. `ARBOS_MCP_TOKEN` defaults to empty, and
    /// an absent header reads as `""`, so before the guard an anonymous request matched the main
    /// arm and every hook route — `/hook/approval` among them — opened to it at the highest role.
    #[test]
    fn an_unset_token_grants_no_role() {
        // `sub_` + sha256("subagent:" + token)[..16] with the token unset: no secret in it, so any
        // caller can compute this value offline. It must not buy the subagent role either.
        let derived_from_nothing = "sub_14dabdbbbef7fd4458133cfe393c201a";
        for headers in [
            HeaderMap::new(),
            bearer(""),
            bearer("   "),
            bearer(derived_from_nothing),
        ] {
            assert_eq!(
                role_for_token("", derived_from_nothing, &headers),
                None,
                "an unset token must authenticate nobody"
            );
        }
        // A configured token still works, and a blank or padded bearer never stands in for it.
        assert_eq!(
            role_for_token("real-main", "sub_real", &bearer("real-main")),
            Some(SessionRole::Main)
        );
        assert_eq!(
            role_for_token("real-main", "sub_real", &bearer("sub_real")),
            Some(SessionRole::Subagent)
        );
        assert_eq!(
            role_for_token("real-main", "sub_real", &bearer("  ")),
            None,
            "a whitespace bearer must not match a configured token"
        );
    }

    /// The caller's privilege comes from its token, not from its body: a genuine subagent
    /// token never touches the running turn's state, whether it claims `role: "main"` or
    /// leaves the field out (where the old default read "main").
    #[tokio::test]
    async fn a_subagent_token_cannot_route_whatever_its_body_claims() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATHOOKROLE00000001";
        let _host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        let sub = bearer(&agent.cfg.subagent_mcp_token());
        let args = json!({ "action": "act", "plan": "read the doc" });

        for claim in [
            json!({ "role": "main", "args": args }),
            json!({ "args": args }),
        ] {
            let v = body(route(State(agent.clone()), sub.clone(), Json(claim.clone())).await).await;
            assert_eq!(v["ok"], json!(false), "{claim} → {v}");
            assert!(
                v["error"].as_str().unwrap_or("").contains("main session"),
                "{claim} → {v}"
            );
            let tl = agent.turn.lock().await;
            let t = tl.as_ref().unwrap();
            assert!(
                t.route.is_none(),
                "the subagent set the turn's route: {claim}"
            );
            assert_eq!(
                t.route_attempts, 0,
                "the subagent reached hook_route: {claim}"
            );
        }

        // The main session's own token still routes; the guard refuses the token, not the body.
        let v = body(
            route(
                State(agent.clone()),
                bearer(&agent.cfg.mcp_token),
                Json(json!({ "args": args })),
            )
            .await,
        )
        .await;
        assert_eq!(v["ok"], json!(true), "{v}");
        assert_eq!(agent.turn.lock().await.as_ref().unwrap().route_attempts, 1);
        let _ = std::fs::remove_dir_all(root);
    }

    /// D69 on the hook port as on MCP: a subagent may post a progress note in its own chat,
    /// and may not reach another agent — neither with `to` nor with the `@Handle words`
    /// antipattern, both of which send under the Project's `box_token`.
    #[tokio::test]
    async fn a_subagent_token_cannot_message_another_agent_through_the_hook() {
        let (agent, root) = test_agent_with(
            Router::new().route("/projects/{id}/agent-messages", post(mock_agent_message)),
        )
        .await;
        let chat = "01TESTCHATHOOKROLE00000002";
        let _host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        let sub = bearer(&agent.cfg.subagent_mcp_token());

        for (call, sent) in [
            (
                json!({ "to": "Doug", "text": "hook-role-test: explicit to" }),
                "hook-role-test: explicit to",
            ),
            (
                json!({ "text": "@Doug hook-role-test: mention" }),
                "hook-role-test: mention",
            ),
        ] {
            let v = body(send_message(State(agent.clone()), sub.clone(), Json(call.clone())).await)
                .await;
            assert_eq!(v["ok"], json!(false), "{call} → {v}");
            assert!(
                v["error"]
                    .as_str()
                    .unwrap_or("")
                    .contains("main agent only"),
                "{call} → {v}"
            );
            assert_eq!(relay_saw(sent), 0, "the subagent reached the relay: {call}");
        }

        // The short branch stays open: a progress note has no `to` and never leaves the chat.
        let v = body(
            send_message(
                State(agent.clone()),
                sub,
                Json(json!({ "text": "halfway through the scan" })),
            )
            .await,
        )
        .await;
        assert_eq!(v["ok"], json!(true), "{v}");

        // The main session still speaks for the Project.
        let v = body(
            send_message(
                State(agent.clone()),
                bearer(&agent.cfg.mcp_token),
                Json(json!({ "to": "Doug", "text": "hook-role-test: main" })),
            )
            .await,
        )
        .await;
        assert_eq!(v["ok"], json!(true), "{v}");
        assert_eq!(relay_saw("hook-role-test: main"), 1);
        let _ = std::fs::remove_dir_all(root);
    }
}
