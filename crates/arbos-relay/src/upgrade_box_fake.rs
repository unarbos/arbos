//! A Daytona stand-in for the box-upgrade tests: it keeps sandbox states, answers the toolbox calls
//! a `/work` carry makes, records every command the carry ran, and answers 502 to any request whose
//! `"METHOD path command"` contains a configured needle. The upgrade runs for real against it.
//!
//! [`Disk`] is what the box-side reads answer. Its defaults are a small box with room to spare, so a
//! test only names the figure it is about. Test data only; no box is touched.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::http::{Method, StatusCode, Uri};
use serde_json::{json, Value};

use arbos_core::ids::now_ms;

use crate::daytona::Daytona;
use crate::db::Sandbox;
use crate::state::test_support::test_state;
use crate::state::{AppState, AuthMode};
use crate::upgrade;
use crate::upgrade_carry::{sha256_hex, STAGE_DIR, STAGE_PARENT};

pub(crate) const USER: &str = "gh_rollback_test";
pub(crate) const SRC: &str = "src-test-sandbox";
/// The snapshot every upgrade here targets. Named, so the relay can ask the stand-in for its disk
/// size the way it asks Daytona; [`Disk::snapshot_disk_gib`] decides what the answer is.
pub(crate) const TARGET: &str = "arbos-agent-test-new";
pub(crate) const LISTING: &[u8] = b"d\t4096\t1\t.\0f\t28\t2\t./project-test/notes.txt\0";
pub(crate) const PART: &[u8] = b"test tarball, not user data";
pub(crate) const WEEK_MS: i64 = 7 * 24 * 3_600_000;

/// What the boxes say about their disks when the carry measures them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Disk {
    /// `du -sm /work` on the source.
    pub(crate) work_mib: i64,
    /// Free MiB where the source stages the pack's parts (`df` of [`STAGE_PARENT`]).
    pub(crate) source_free_mib: u64,
    /// The biggest single carried file on the source: it bounds the biggest part.
    pub(crate) max_file_bytes: u64,
    /// Free MiB on the new box's `/work`.
    pub(crate) dest_free_mib: u64,
    /// The disk size Daytona holds for the target snapshot (GiB). `None` makes
    /// `GET /snapshots/{name}` a 404, as it is for a snapshot the stand-in knows nothing about.
    pub(crate) snapshot_disk_gib: Option<i64>,
}

impl Default for Disk {
    fn default() -> Self {
        Self {
            work_mib: 5,
            source_free_mib: 10_000,
            max_file_bytes: PART.len() as u64,
            dest_free_mib: 10_000,
            snapshot_disk_gib: None,
        }
    }
}

#[derive(Default)]
pub(crate) struct Fake {
    pub(crate) states: Mutex<HashMap<String, &'static str>>,
    pub(crate) deleted: Mutex<Vec<String>>,
    pub(crate) created: AtomicUsize,
    /// `(who, needle)`: `who` is a sandbox id, or `"new"` for any sandbox the upgrade created.
    pub(crate) fail: Mutex<Vec<(String, String)>>,
    /// `POST /sandbox/{id}/start` for this id waits here until released.
    pub(crate) start_gate: Mutex<Option<(String, Arc<tokio::sync::Notify>)>>,
    pub(crate) gate_reached: tokio::sync::Notify,
    pub(crate) disk: Mutex<Disk>,
    /// Every toolbox command, in the order it was run — including the ones answered 502.
    execs: Mutex<Vec<String>>,
}

impl Fake {
    pub(crate) fn state(&self, id: &str) -> Option<&'static str> {
        self.states.lock().unwrap().get(id).copied()
    }

    pub(crate) fn set(&self, id: &str, st: &'static str) {
        self.states.lock().unwrap().insert(id.to_owned(), st);
    }

    pub(crate) fn set_disk(&self, d: Disk) {
        *self.disk.lock().unwrap() = d;
    }

    /// Did the carry run a command holding `needle`?
    pub(crate) fn ran(&self, needle: &str) -> bool {
        self.execs
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.contains(needle))
    }

    /// Did the carry quiesce a box (the first thing that stops the user's agentd)?
    pub(crate) fn quiesced(&self) -> bool {
        self.ran("pkill -TERM -x arbos-agentd")
    }

    /// How many times the carry ran its staging cleanup. The whole command is compared, so the
    /// `rm -rf` the listing step does before it makes the directory is not counted as a cleanup.
    pub(crate) fn staging_clears(&self) -> usize {
        let cleanup = format!("sudo -n rm -rf {STAGE_DIR}");
        self.execs
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.trim() == cleanup)
            .count()
    }

    fn fails(&self, id: &str, what: &str) -> bool {
        self.fail.lock().unwrap().iter().any(|(who, needle)| {
            (who == id || (who == "new" && id.starts_with("new-")))
                && what.contains(needle.as_str())
        })
    }

    fn sandbox(id: &str, st: &str) -> String {
        json!({ "id": id, "state": st }).to_string()
    }

    /// What the box would print for each step of the carry.
    fn exec_answer(&self, command: &str) -> String {
        let d = *self.disk.lock().unwrap();
        if command.contains("du -sm") {
            d.work_mib.to_string()
        } else if command.contains("wc -c <") {
            LISTING.len().to_string()
        } else if command.contains("tar --null") {
            format!("{}\n{}", sha256_hex(PART), PART.len())
        } else if command.contains("tail -c +") {
            sha256_hex(PART)
        } else if command.contains(&format!("df -Pm {STAGE_PARENT}")) {
            format!("{}\n{}", d.source_free_mib, d.max_file_bytes)
        } else if command.contains("df -Pm") {
            format!("{}\n100000 90000", d.dest_free_mib)
        } else if command.contains("printf '%y\\t%s\\n'") {
            "1000 1000000".into()
        } else {
            "ok".into()
        }
    }

    async fn handle(
        &self,
        base: &str,
        method: Method,
        uri: Uri,
        body: Bytes,
    ) -> (StatusCode, String) {
        let path = uri.path().to_owned();
        let seg: Vec<&str> = path.trim_matches('/').split('/').collect();
        let command = serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|v| v.get("command").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_default();
        if !command.is_empty() {
            self.execs.lock().unwrap().push(command.clone());
        }
        let what = format!("{method} {path} {command}");
        let id = seg.get(1).copied().unwrap_or("").to_owned();
        if self.fails(&id, &what) {
            return (StatusCode::BAD_GATEWAY, "Bad Gateway".into());
        }
        match (method.clone(), seg.as_slice()) {
            (Method::POST, ["sandbox"]) => {
                let id = format!("new-{}", self.created.fetch_add(1, Ordering::SeqCst));
                self.set(&id, "started");
                (StatusCode::OK, Self::sandbox(&id, "started"))
            }
            (Method::GET, ["sandbox", id]) => match self.state(id) {
                Some(st) => (StatusCode::OK, Self::sandbox(id, st)),
                None => (StatusCode::NOT_FOUND, "{}".into()),
            },
            (Method::DELETE, ["sandbox", id]) => {
                self.states.lock().unwrap().remove(*id);
                self.deleted.lock().unwrap().push((*id).to_owned());
                (StatusCode::OK, Self::sandbox(id, "destroyed"))
            }
            (Method::POST, ["sandbox", id, "start"]) => {
                let gate = self
                    .start_gate
                    .lock()
                    .unwrap()
                    .as_ref()
                    .filter(|(g, _)| g == id)
                    .map(|(_, n)| n.clone());
                if let Some(release) = gate {
                    self.gate_reached.notify_one();
                    release.notified().await;
                }
                self.set(id, "started");
                (StatusCode::OK, Self::sandbox(id, "started"))
            }
            (Method::POST, ["sandbox", id, "stop"]) => {
                self.set(id, "stopped");
                (StatusCode::OK, Self::sandbox(id, "stopped"))
            }
            (Method::GET, ["snapshots", name]) => {
                match self.disk.lock().unwrap().snapshot_disk_gib {
                    Some(gib) => (
                        StatusCode::OK,
                        json!({ "id": name, "name": name, "disk": gib }).to_string(),
                    ),
                    None => (StatusCode::NOT_FOUND, "{}".into()),
                }
            }
            (Method::GET, ["sandbox", _, "toolbox-proxy-url"]) => (
                StatusCode::OK,
                json!({ "url": format!("{base}/toolbox") }).to_string(),
            ),
            (Method::POST, ["sandbox", id, "autostop", _]) => (
                StatusCode::OK,
                Self::sandbox(id, self.state(id).unwrap_or("started")),
            ),
            (Method::POST, ["toolbox", _, "process", "execute"]) => (
                StatusCode::OK,
                json!({ "exitCode": 0, "result": self.exec_answer(&command) }).to_string(),
            ),
            (Method::POST, ["toolbox", _, "files", "upload"]) => (StatusCode::OK, "{}".into()),
            (Method::GET, ["toolbox", _, "files", "download"]) => {
                let bytes = if uri.query().unwrap_or("").contains("work.list") {
                    LISTING
                } else {
                    PART
                };
                (StatusCode::OK, String::from_utf8_lossy(bytes).into_owned())
            }
            _ => (StatusCode::NOT_FOUND, "{}".into()),
        }
    }
}

pub(crate) async fn fake_daytona() -> (Daytona, Arc<Fake>) {
    let fake = Arc::new(Fake::default());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let base = format!("http://{}", l.local_addr().expect("addr"));
    let (f, b) = (fake.clone(), base.clone());
    let app = axum::Router::new().fallback(move |method: Method, uri: Uri, body: Bytes| {
        let (f, b) = (f.clone(), b.clone());
        async move { f.handle(&b, method, uri, body).await }
    });
    tokio::spawn(async move { axum::serve(l, app).await.ok() });
    (Daytona::new("k".into(), Some(base), None), fake)
}

pub(crate) fn row(sandbox: &str, token_hash: &str) -> Sandbox {
    Sandbox {
        user_id: USER.into(),
        provider: "daytona".into(),
        sandbox_id: Some(sandbox.into()),
        daytona_state: Some("started".into()),
        device_id: format!("box:{sandbox}"),
        box_token_hash: token_hash.into(),
        mcp_token: "mcp-test".into(),
        snapshot: Some("arbos-agent-test-old".into()),
        last_started_at: Some(1_000),
        last_stopped_at: None,
        last_seen_at: Some(now_ms()),
        box_url: None,
        preview_token: None,
        local_pid: None,
        migrated_from: Vec::new(),
    }
}

pub(crate) async fn setup(fail: &[(&str, &str)]) -> (AppState, Arc<Fake>) {
    let mut state = test_state(AuthMode::Github);
    let (dt, fake) = fake_daytona().await;
    state.daytona = Some(dt);
    state
        .db
        .insert_test_user(USER, "rollback-test", None, None, 10.0)
        .expect("user");
    state.db.upsert_box(&row(SRC, "hash-src")).expect("row");
    *state.default_snapshot.write().unwrap() = TARGET.to_owned();
    fake.set(SRC, "started");
    *fake.fail.lock().unwrap() = fail
        .iter()
        .map(|(w, n)| ((*w).to_owned(), (*n).to_owned()))
        .collect();
    (state, fake)
}

pub(crate) async fn upgrade_until_done(state: &AppState) {
    assert!(upgrade::start(state, USER, true).await.expect("start"));
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while state.upgrading.lock().await.contains_key(USER) {
        assert!(
            std::time::Instant::now() < deadline,
            "upgrade still running"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The row is on the source with the source's token, the source runs and is not in history, and
/// no sandbox is left that neither the row nor `box_history` names.
pub(crate) fn assert_rolled_back_to_source(state: &AppState, fake: &Fake) {
    let r = state.db.user_box(USER).unwrap().expect("row");
    assert_eq!(r.sandbox_id.as_deref(), Some(SRC), "row left the source");
    assert_eq!(r.box_token_hash, "hash-src");
    assert_eq!(fake.state(SRC), Some("started"), "source not running");
    assert!(state.db.box_history_entry(SRC).unwrap().is_none());
    for id in fake.states.lock().unwrap().keys() {
        assert!(
            id == SRC || state.db.box_history_entry(id).unwrap().is_some(),
            "{id} is orphaned: no row and no box_history entry"
        );
    }
}

pub(crate) fn new_box(fake: &Fake) -> String {
    assert_eq!(fake.created.load(Ordering::SeqCst), 1, "one new box");
    "new-0".into()
}

/// The reason the measure step recorded, as `GET /machine` and the deploy worker read it.
pub(crate) async fn refusal(state: &AppState) -> Option<String> {
    let v = upgrade::status_json(state, USER).await;
    v.get("last_refused")
        .and_then(|r| r.get("reason"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}
