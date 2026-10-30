//! Failed-upgrade rollback (Oct 19–25 orphans). Every journalled orphan came from an upgrade that hit
//! a Daytona toolbox HTTP 502 and logged "upgrade rolled back". In five, the row ended on the
//! half-filled new sandbox and the full source ran on with no row (a user's whole project was left
//! only on the unreferenced source); in two, the row stayed on the source but the new sandbox was
//! never deleted. At 21:35:17 on Oct 25 a wake bounced the box in the same second as the rollback.
//!
//! The upgrade runs here for real against a stand-in Daytona with a 502 at one step. After every
//! case: the row is on the source with its own token, the source runs, and every sandbox the stand-in
//! still has is either the row's or recorded in `box_history`. Test data only.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::http::{Method, StatusCode, Uri};
use serde_json::{json, Value};

use arbos_core::ids::now_ms;

use crate::boxes;
use crate::daytona::Daytona;
use crate::db::Sandbox;
use crate::state::test_support::test_state;
use crate::state::{AppState, AuthMode};
use crate::upgrade::{self, Source, Staged};
use crate::upgrade_carry::sha256_hex;

const USER: &str = "gh_rollback_test";
const SRC: &str = "src-test-sandbox";
const LISTING: &[u8] = b"d\t4096\t1\t.\0f\t28\t2\t./project-test/notes.txt\0";
const PART: &[u8] = b"test tarball, not user data";
const WEEK_MS: i64 = 7 * 24 * 3_600_000;

/// A Daytona stand-in that keeps sandbox states, answers the toolbox calls a carry makes, and
/// answers 502 to any request whose `"METHOD path command"` contains a configured needle.
#[derive(Default)]
struct Fake {
    states: Mutex<HashMap<String, &'static str>>,
    deleted: Mutex<Vec<String>>,
    created: AtomicUsize,
    /// `(who, needle)`: `who` is a sandbox id, or `"new"` for any sandbox the upgrade created.
    fail: Mutex<Vec<(String, String)>>,
    /// `POST /sandbox/{id}/start` for this id waits here until released.
    start_gate: Mutex<Option<(String, Arc<tokio::sync::Notify>)>>,
    gate_reached: tokio::sync::Notify,
}

impl Fake {
    fn state(&self, id: &str) -> Option<&'static str> {
        self.states.lock().unwrap().get(id).copied()
    }

    fn set(&self, id: &str, st: &'static str) {
        self.states.lock().unwrap().insert(id.to_owned(), st);
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
                json!({ "exitCode": 0, "result": exec_answer(&command) }).to_string(),
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

/// What the box would print for each step of the carry.
fn exec_answer(command: &str) -> String {
    if command.contains("du -sm") {
        "5".into()
    } else if command.contains("wc -c <") {
        LISTING.len().to_string()
    } else if command.contains("tar --null") {
        format!("{}\n{}", sha256_hex(PART), PART.len())
    } else if command.contains("tail -c +") {
        sha256_hex(PART)
    } else if command.contains("df -Pm") {
        "10000\n100000 90000".into()
    } else if command.contains("printf '%y\\t%s\\n'") {
        "1000 1000000".into()
    } else {
        "ok".into()
    }
}

async fn fake_daytona() -> (Daytona, Arc<Fake>) {
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

fn row(sandbox: &str, token_hash: &str) -> Sandbox {
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

async fn setup(fail: &[(&str, &str)]) -> (AppState, Arc<Fake>) {
    let mut state = test_state(AuthMode::Github);
    let (dt, fake) = fake_daytona().await;
    state.daytona = Some(dt);
    state
        .db
        .insert_test_user(USER, "rollback-test", None, None, 10.0)
        .expect("user");
    state.db.upsert_box(&row(SRC, "hash-src")).expect("row");
    fake.set(SRC, "started");
    *fake.fail.lock().unwrap() = fail
        .iter()
        .map(|(w, n)| ((*w).to_owned(), (*n).to_owned()))
        .collect();
    (state, fake)
}

async fn upgrade_until_done(state: &AppState) {
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
fn assert_rolled_back_to_source(state: &AppState, fake: &Fake) {
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

fn new_box(fake: &Fake) -> String {
    assert_eq!(fake.created.load(Ordering::SeqCst), 1, "one new box");
    "new-0".into()
}

#[tokio::test]
async fn a_502_during_pack_keeps_the_source_and_makes_no_new_box() {
    let (state, fake) = setup(&[(SRC, "tar --null")]).await;
    upgrade_until_done(&state).await;
    assert_rolled_back_to_source(&state, &fake);
    assert_eq!(fake.created.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_502_during_unpack_before_any_data_deletes_the_empty_new_box() {
    let (state, fake) = setup(&[("new", "df -Pm")]).await;
    upgrade_until_done(&state).await;
    assert_rolled_back_to_source(&state, &fake);
    let new = new_box(&fake);
    assert_eq!(*fake.deleted.lock().unwrap(), vec![new.clone()]);
    assert!(state.db.box_history_entry(&new).unwrap().is_none());
}

/// Two of the orphans: the new box got part of `/work`, then a 502. It is not deleted (it holds
/// user data), not left running, and recorded so the sweeper deletes it after the window.
#[tokio::test]
async fn a_502_during_unpack_after_data_keeps_the_new_box_stopped_and_recorded() {
    let (state, fake) = setup(&[("new", "tar -xzf")]).await;
    upgrade_until_done(&state).await;
    assert_rolled_back_to_source(&state, &fake);
    let new = new_box(&fake);
    assert!(fake.deleted.lock().unwrap().is_empty());
    assert_eq!(fake.state(&new), Some("stopped"));
    let (delete_after, partial) = state.db.box_history_entry(&new).unwrap().expect("recorded");
    assert!(partial);
    assert!(delete_after > now_ms() + WEEK_MS - 60_000);
    assert!(
        state.db.latest_box_history(USER).unwrap().is_none(),
        "a half-carried box is never offered for repair"
    );
}

/// The delete of an empty new box answers 502 too: it is recorded, due now, for the sweeper.
#[tokio::test]
async fn a_502_on_the_cleanup_delete_never_orphans_anything() {
    let (state, fake) = setup(&[("new", "df -Pm"), ("new", "DELETE ")]).await;
    upgrade_until_done(&state).await;
    assert_rolled_back_to_source(&state, &fake);
    let new = new_box(&fake);
    let (delete_after, _) = state.db.box_history_entry(&new).unwrap().expect("recorded");
    assert!(delete_after <= now_ms());
    assert!(state
        .db
        .box_history_due()
        .unwrap()
        .contains(&(USER.to_owned(), new)));
}

/// The five source orphans' shape: a 502 once the row names the new box (its start). The row goes back to the source; the new box, which holds the carried
/// `/work`, is kept stopped and recorded.
#[tokio::test]
async fn a_502_during_the_swap_puts_the_row_back_on_the_source() {
    let (state, fake) = setup(&[("new", "/start")]).await;
    upgrade_until_done(&state).await;
    assert_rolled_back_to_source(&state, &fake);
    let new = new_box(&fake);
    assert!(fake.deleted.lock().unwrap().is_empty());
    assert_eq!(fake.state(&new), Some("stopped"));
    assert!(
        state
            .db
            .box_history_entry(&new)
            .unwrap()
            .expect("recorded")
            .1
    );
}

/// Oct 25 21:35:17: a wake read the row while it named the new box, bounced that sandbox ("sandbox
/// is started but the box is silent; restarting it") and finished after the rollback had put the
/// source back. Its write-back put the half-filled box into the row; the full source ran on with
/// no row. The wake must not write a row that moved on, and the new box must not keep running.
#[tokio::test]
async fn a_wake_racing_the_rollback_does_not_write_the_new_box_back() {
    let (state, fake) = setup(&[]).await;
    let source = row(SRC, "hash-src");
    let new = row("new-race", "hash-new");
    fake.set(SRC, "stopped");
    fake.set("new-race", "started");
    state
        .db
        .insert_box_history(&source, now_ms() + WEEK_MS)
        .unwrap();
    state.db.upsert_box(&new).unwrap();
    let release = Arc::new(tokio::sync::Notify::new());
    *fake.start_gate.lock().unwrap() = Some(("new-race".into(), release.clone()));

    let st = state.clone();
    let wake = tokio::spawn(async move { boxes::start_box(&st, USER).await });
    tokio::time::timeout(Duration::from_secs(10), fake.gate_reached.notified())
        .await
        .expect("the wake reached the new box's start");

    let dt = state.daytona.as_ref().unwrap();
    let staged = Staged {
        row: &new,
        keep: true,
        swapped: true,
    };
    upgrade::rollback(
        &state,
        dt,
        USER,
        &Source::CurrentBox,
        Some(&source),
        Some(staged),
    )
    .await;
    release.notify_one();
    let woke = tokio::time::timeout(Duration::from_secs(10), wake)
        .await
        .expect("wake finished")
        .unwrap();

    assert_rolled_back_to_source(&state, &fake);
    assert!(woke.is_err(), "the wake must notice the row moved on");
    assert_eq!(fake.state("new-race"), Some("stopped"));
    assert!(
        state
            .db
            .box_history_entry("new-race")
            .unwrap()
            .expect("recorded")
            .1
    );
}
