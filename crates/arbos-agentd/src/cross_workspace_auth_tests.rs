//! The workspace boundary at the loopback servers (design D70): one box holds one workspace per
//! project, and `/work/<project>` with its own Unix user and mode-700 dirs is how one owner's
//! projects are kept apart. The MCP and hook servers are the other half of that boundary: a token
//! minted for workspace A must not be accepted by workspace B's servers, whose ports
//! (`7800 + 10·index`) are guessable rather than secret.

use std::sync::Arc;

use serde_json::{json, Value};

use arbos_core::ids::ulid;

use crate::agent::Agent;
use crate::config::{BoxConfig, Config};
use crate::store::Store;
use crate::sync::SyncHub;

const PROJECT_A: &str = "01CROSSWORKSPACEAUTHPROJA0";
const PROJECT_B: &str = "01CROSSWORKSPACEAUTHPROJB0";

/// One box, two workspaces: the same `BoxConfig` at two indexes, as the supervisor builds them.
fn box_cfg(root: &std::path::Path) -> BoxConfig {
    BoxConfig {
        user_id: Some("u-cross-workspace".into()),
        relay_url: "http://127.0.0.1:1".into(),
        box_token: "box-token-for-the-whole-box".into(),
        mcp_token: "mcp-token-for-the-whole-box".into(),
        work_root: root.to_path_buf(),
        bind: "127.0.0.1".into(),
        port: 0,
        local_box: true,
        pi_bin: "/nonexistent/pi".into(),
        prime_bin: "/nonexistent/prime-agent".into(),
        claude_bin: "/nonexistent/claude".into(),
        pi_ext_override: None,
        device_id: "box:cross-workspace".into(),
        default_talking_model: "mock-talking".into(),
        default_subagent_model: "mock-sub".into(),
        legacy_project_id: None,
    }
}

fn agent_for(cfg: Config) -> Arc<Agent> {
    let cfg = Arc::new(cfg);
    std::fs::create_dir_all(cfg.state_dir.join("agentd")).unwrap();
    let store = Store::open(&cfg.state_dir.join("agentd").join("agentd.db")).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::channel(64);
    let sync = SyncHub::new(cfg.clone(), store.clone(), tx);
    Agent::new(cfg, store, sync, Arc::new(tokio::sync::Semaphore::new(4)))
}

/// Two workspaces of one box, plus the temp work root to clean up.
fn two_workspaces() -> (Arc<Agent>, Arc<Agent>, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!("arbos-xws-{}", ulid()));
    std::fs::create_dir_all(&root).unwrap();
    let bx = box_cfg(&root);
    let a = agent_for(bx.workspace(&json!({ "id": PROJECT_A, "name": "Alice" }), 0));
    let b = agent_for(bx.workspace(&json!({ "id": PROJECT_B, "name": "Bob" }), 1));
    (a, b, root)
}

/// Serve `app` on a loopback port and answer its base URL.
async fn serve(app: axum::Router) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(l, app).await.ok();
    });
    base
}

async fn post(url: &str, token: &str, body: Value) -> (u16, String) {
    let r = reqwest::Client::new()
        .post(url)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    (status, r.text().await.unwrap_or_default())
}

/// A child of workspace A holds A's MCP token (`harness.rs` puts `ARBOS_MCP_TOKEN` in every
/// harness child's environment). Pointed at workspace B's MCP port it must be refused: B's
/// tools act on B's chats, memory and working set.
#[tokio::test]
async fn workspace_a_token_is_refused_by_workspace_b_mcp_server() {
    let (a, b, root) = two_workspaces();
    let b_mcp = serve(crate::mcp::router(b.clone())).await;
    let url = format!("{b_mcp}/mcp");

    let (status, body) = post(
        &url,
        &a.cfg.mcp_token,
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
    )
    .await;
    assert_eq!(
        status, 401,
        "workspace A's main token reached workspace B's MCP server: {body}"
    );

    let (status, body) = post(
        &url,
        &a.cfg.subagent_mcp_token(),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    )
    .await;
    assert_eq!(
        status, 401,
        "workspace A's subagent token reached workspace B's MCP server: {body}"
    );

    // B's own tokens still work, so the refusal above is the boundary and not a broken server.
    let (status, _) = post(
        &url,
        &b.cfg.mcp_token,
        json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }),
    )
    .await;
    assert_eq!(status, 200, "workspace B's own main token must be accepted");
    let (status, _) = post(
        &url,
        &b.cfg.subagent_mcp_token(),
        json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/list" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "workspace B's own subagent token must be accepted"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// The same for the hook API, and with a write: `remember` lands in the serving workspace's
/// store, so acceptance is a cross-workspace write to B's scratchpad.
#[tokio::test]
async fn workspace_a_token_cannot_write_workspace_b_scratchpad() {
    let (a, b, root) = two_workspaces();
    let b_hooks = serve(crate::hooks::router(b.clone())).await;

    let (status, body) = post(
        &format!("{b_hooks}/hook/remember"),
        &a.cfg.mcp_token,
        json!({ "session_id": "chat-in-b", "name": "planted", "value": "by workspace A" }),
    )
    .await;
    assert_eq!(
        status, 401,
        "workspace A's token reached workspace B's hook API: {body}"
    );
    assert_eq!(
        b.store.scratch_get("chat-in-b", "planted").unwrap(),
        None,
        "workspace A wrote a note into workspace B's scratchpad"
    );

    let (status, _) = post(
        &format!("{b_hooks}/hook/remember"),
        &b.cfg.mcp_token,
        json!({ "session_id": "chat-in-b", "name": "planted", "value": "by workspace B" }),
    )
    .await;
    assert_eq!(status, 200, "workspace B's own token must be accepted");
    assert_eq!(
        b.store.scratch_get("chat-in-b", "planted").unwrap(),
        Some("by workspace B".to_owned()),
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// `box_admin_authed` (`http.rs:274`) takes the box-wide `BoxConfig::mcp_token` on the box port,
/// which reaches every workspace (`/box/state`, `/w/{project}/inspect`, hold, release). A
/// workspace session must therefore not be handed that same string.
#[test]
fn a_workspace_token_is_not_the_box_admin_credential() {
    let root = std::env::temp_dir().join(format!("arbos-xws-adm-{}", ulid()));
    let bx = box_cfg(&root);
    let w = bx.workspace(&json!({ "id": PROJECT_A }), 0);
    assert_ne!(
        w.mcp_token, bx.mcp_token,
        "a workspace session holds the box admin credential"
    );
}

/// A box booted without `ARBOS_MCP_TOKEN` leaves `BoxConfig::mcp_token` empty, and an absent
/// Authorization header also reads as the empty string, so the two must never compare equal.
#[tokio::test]
async fn an_unset_box_token_does_not_authenticate_a_request_without_one() {
    let root = std::env::temp_dir().join(format!("arbos-xws-empty-{}", ulid()));
    std::fs::create_dir_all(&root).unwrap();
    let mut bx = box_cfg(&root);
    bx.mcp_token = String::new();
    let a = agent_for(bx.workspace(&json!({ "id": PROJECT_A, "name": "Alice" }), 0));
    let base = serve(crate::mcp::router(a)).await;

    let r = reqwest::Client::new()
        .post(format!("{base}/mcp"))
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status().as_u16(),
        401,
        "a request with no bearer was served as the main session"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// The tokens themselves: two workspaces of one box must not share them.
#[test]
fn two_workspaces_of_one_box_get_different_mcp_tokens() {
    let root = std::env::temp_dir().join(format!("arbos-xws-tok-{}", ulid()));
    let bx = box_cfg(&root);
    let a = bx.workspace(&json!({ "id": PROJECT_A }), 0);
    let b = bx.workspace(&json!({ "id": PROJECT_B }), 1);
    assert_ne!(a.mcp_token, b.mcp_token, "main MCP tokens are shared");
    assert_ne!(
        a.subagent_mcp_token(),
        b.subagent_mcp_token(),
        "subagent MCP tokens are shared"
    );
    assert_ne!(
        a.mcp_token,
        b.subagent_mcp_token(),
        "A's main token equals B's subagent token"
    );
}

/// The derivation is keyed on the project and not on the workspace index, which comes from a
/// boot-order counter that compacts across restarts: the same project must keep its token when it
/// comes back at a different index.
#[test]
fn the_token_follows_the_project_and_not_the_workspace_index() {
    let root = std::env::temp_dir().join(format!("arbos-xws-idx-{}", ulid()));
    let bx = box_cfg(&root);
    let first_boot = bx.workspace(&json!({ "id": PROJECT_A }), 0);
    let after_restart = bx.workspace(&json!({ "id": PROJECT_A }), 3);
    assert_eq!(first_boot.mcp_token, after_restart.mcp_token);
    assert_eq!(
        first_boot.subagent_mcp_token(),
        after_restart.subagent_mcp_token()
    );
}
