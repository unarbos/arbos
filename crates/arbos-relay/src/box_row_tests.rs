//! #2262: a running box's row never loses its sandbox fields. gh_17856706's row was blanked when
//! a planned relay restart (SIGTERM 01:03:04 on Oct 29) cut an upgrade between `create_box`'s
//! first row write and the new sandbox id; the new box then authenticated against a row that
//! named no sandbox, and rolls and always-on lost it.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde_json::Value;

use arbos_core::ids::now_ms;

use crate::auth::Principal;
use crate::boxes::{self, Provider};
use crate::daytona::Daytona;
use crate::db::Sandbox;
use crate::state::test_support::test_state;
use crate::state::{AppState, AuthMode, UpgradeStatus};

fn live_row(user: &str) -> Sandbox {
    Sandbox {
        user_id: user.into(),
        provider: "daytona".into(),
        sandbox_id: Some("92fcb3f8-old".into()),
        daytona_state: Some("started".into()),
        device_id: "box:92fcb3f8-old".into(),
        box_token_hash: "hash-of-the-running-box".into(),
        mcp_token: "mcp-old".into(),
        snapshot: Some("arbos-agent-0.1.179".into()),
        last_started_at: Some(1_000),
        last_stopped_at: None,
        last_seen_at: Some(now_ms()),
        box_url: Some("https://7788-old.proxy.daytona.works".into()),
        preview_token: Some("pv".into()),
        local_pid: None,
        migrated_from: Vec::new(),
    }
}

/// A Daytona stand-in whose `POST /sandbox` creates the sandbox and never answers: the relay is
/// stopped while it waits, as at 01:03:04. Every other call is a 404.
async fn daytona_that_hangs_on_create() -> (Daytona, Arc<tokio::sync::Notify>) {
    let created = Arc::new(tokio::sync::Notify::new());
    let c = created.clone();
    let app = axum::Router::new()
        .route(
            "/sandbox",
            axum::routing::post(move || {
                let c = c.clone();
                async move {
                    c.notify_one();
                    std::future::pending::<()>().await;
                }
            }),
        )
        .fallback(|| async { (StatusCode::NOT_FOUND, "{}") });
    let l = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = l.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(l, app).await.ok() });
    (
        Daytona::new("k".into(), Some(format!("http://{addr}")), None),
        created,
    )
}

/// Start an upgrade-style `create_box` over a live row and kill it while Daytona is creating.
async fn restart_mid_create(state: &mut AppState, user: &str) {
    let (dt, created) = daytona_that_hangs_on_create().await;
    state.daytona = Some(dt);
    state.db.upsert_box(&live_row(user)).expect("live row");
    let (st, u) = (state.clone(), user.to_owned());
    let task = tokio::spawn(async move {
        let mut permit = st.restart.admit_creation(&u).await;
        boxes::create_box(
            &st,
            &mut permit,
            Provider::Daytona,
            Some("arbos-agent-0.1.181"),
            true,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(10), created.notified())
        .await
        .expect("create reached Daytona");
    task.abort();
    let _ = task.await;
}

/// The incident, replayed: before the fix the row came back as `create_box`'s placeholder — no
/// sandbox_id, snapshot, device_id, box_url or last_started_at — with a fresh token hash.
#[tokio::test]
async fn a_restart_mid_create_leaves_the_live_row_whole() {
    let mut state = test_state(AuthMode::Github);
    restart_mid_create(&mut state, "gh_live").await;

    let row = state.db.user_box("gh_live").unwrap().expect("row");
    let before = live_row("gh_live");
    assert_eq!(row.sandbox_id, before.sandbox_id);
    assert_eq!(row.snapshot, before.snapshot);
    assert_eq!(row.device_id, before.device_id);
    assert_eq!(row.box_url, before.box_url);
    assert_eq!(row.last_started_at, before.last_started_at);
    assert_eq!(row.box_token_hash, before.box_token_hash);
}

/// What the cut-short create leaves behind is findable: the create is no longer in flight, its
/// `box_creates` row names the sandbox Daytona may be running and the sandbox it was replacing,
/// and the log-only check reports it once it is stale.
#[tokio::test]
async fn a_cut_short_create_is_recorded_and_reported() {
    let mut state = test_state(AuthMode::Github);
    restart_mid_create(&mut state, "gh_live").await;

    assert!(state.restart.creating.lock().unwrap().is_empty());
    let left = state.db.unfinished_box_creates(now_ms() + 1).unwrap();
    assert_eq!(left.len(), 1);
    assert!(left[0].sandbox_name.starts_with("arbos-gh-live-"));
    assert_eq!(left[0].replacing.as_deref(), Some("92fcb3f8-old"));
    assert_eq!(left[0].snapshot.as_deref(), Some("arbos-agent-0.1.181"));

    // Not stale yet: the check stays quiet. Ten minutes later it names the sandbox.
    assert!(boxes::sandboxless_live_boxes(&state).await.is_empty());
    state
        .db
        .with(|c| {
            c.execute(
                "UPDATE box_creates SET started_at = started_at - 11 * 60000",
                [],
            )
        })
        .unwrap();
    let found = boxes::sandboxless_live_boxes(&state).await;
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0]["kind"], "unfinished_create");
    assert_eq!(
        found[0]["sandbox_name"],
        Value::from(left[0].sandbox_name.clone())
    );
}

/// The DB itself refuses any write that would drop a live row's sandbox (a stale write-back of a
/// row read mid-create included); only the explicit clearing write may.
#[tokio::test]
async fn a_row_with_a_sandbox_is_never_written_without_it() {
    let state = test_state(AuthMode::Github);
    state.db.upsert_box(&live_row("gh_a")).unwrap();

    let mut blank = live_row("gh_a");
    blank.sandbox_id = None;
    blank.snapshot = None;
    blank.box_url = None;
    blank.device_id = String::new();
    blank.box_token_hash = "fresh-hash".into();
    assert!(state.db.upsert_box(&blank).is_err());
    let row = state.db.user_box("gh_a").unwrap().unwrap();
    assert_eq!(row.sandbox_id.as_deref(), Some("92fcb3f8-old"));
    assert_eq!(row.box_token_hash, "hash-of-the-running-box");

    // A first box's placeholder (no row yet, or a row with no sandbox) is still written.
    state
        .db
        .upsert_box(&{
            let mut b = blank.clone();
            b.user_id = "gh_new".into();
            b
        })
        .unwrap();

    state.db.upsert_box_clearing_sandbox(&blank).unwrap();
    assert!(state
        .db
        .user_box("gh_a")
        .unwrap()
        .unwrap()
        .sandbox_id
        .is_none());
}

/// An upgrade cut short after the old box went into `box_history` leaves the row on the old box;
/// the sweeper must not delete or archive the sandbox the row still names.
#[tokio::test]
async fn the_history_sweeper_spares_the_sandbox_a_row_names() {
    let state = test_state(AuthMode::Github);
    let row = live_row("gh_a");
    state.db.upsert_box(&row).unwrap();
    state.db.insert_box_history(&row, now_ms() - 1).unwrap();
    assert!(state.db.box_history_due().unwrap().is_empty());
    assert!(state
        .db
        .box_history_to_archive(Some("gh_a"), 0)
        .unwrap()
        .is_empty());

    let mut moved_on = row.clone();
    moved_on.sandbox_id = Some("0999f6bf-new".into());
    state.db.upsert_box(&moved_on).unwrap();
    assert_eq!(
        state.db.box_history_due().unwrap(),
        vec![("gh_a".to_owned(), "92fcb3f8-old".to_owned())]
    );
}

/// A planned restart with an upgrade in flight answers 409 and changes nothing; while a restart
/// is under way no upgrade starts.
#[tokio::test]
async fn a_planned_restart_waits_for_box_work() {
    let state = test_state(AuthMode::Github);
    state.upgrading.lock().await.insert(
        "gh_a".into(),
        UpgradeStatus {
            phase: "pack".into(),
            since: now_ms(),
            from_snapshot: None,
            to_snapshot: "arbos-agent-0.1.181".into(),
        },
    );
    let r = crate::restart::admin_restart(
        State(state.clone()),
        Principal::Admin,
        Some(Json(crate::restart::RestartBody {
            notice_s: Some(0),
            drain_s: Some(0),
            force: None,
        })),
    )
    .await;
    assert_eq!(r.status(), StatusCode::CONFLICT);
    assert!(!state.restart.is_closing());
    assert!(!state.restart.box_work_frozen());

    state.upgrading.lock().await.clear();
    {
        let mut permit = state.restart.admit_creation("gh_b").await;
        let _creating = crate::state::CreatingGuard::new(&mut permit);
        let work = crate::restart::box_work_in_flight(&state).await;
        assert_eq!(work.len(), 1);
        assert_eq!(work[0]["kind"], "create");
    }
    assert!(crate::restart::box_work_in_flight(&state).await.is_empty());

    state.db.upsert_box(&live_row("gh_c")).unwrap();
    crate::restart::freeze_box_work(&state).await;
    let e = crate::upgrade::start(&state, "gh_c", true)
        .await
        .expect_err("frozen");
    assert!(e.to_string().contains(crate::upgrade::RESTARTING_PHRASE));
    assert!(state.upgrading.lock().await.is_empty());
}

/// The log-only check: a row without a sandbox whose box shows a sign of life is reported; a
/// row with a sandbox, or one long silent, is not. Nothing is written.
#[tokio::test]
async fn the_check_reports_live_rows_without_a_sandbox() {
    let state = test_state(AuthMode::Github);
    state.db.upsert_box(&live_row("gh_ok")).unwrap();
    let mut limbo = live_row("gh_limbo");
    limbo.sandbox_id = None;
    state.db.upsert_box(&limbo).unwrap();
    let mut silent = limbo.clone();
    silent.user_id = "gh_silent".into();
    silent.last_seen_at = Some(now_ms() - 3_600_000);
    state.db.upsert_box(&silent).unwrap();

    let found = boxes::sandboxless_live_boxes(&state).await;
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0]["kind"], "sandboxless_live_row");
    assert_eq!(found[0]["user_id"], "gh_limbo");
    assert!(state
        .db
        .user_box("gh_limbo")
        .unwrap()
        .unwrap()
        .sandbox_id
        .is_none());
}

mod create_admission_regressions {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const USER_A: &str = "gh_fixture_create_a";
    const USER_B: &str = "gh_fixture_create_b";
    #[derive(Default)]
    struct Progress {
        reads: AtomicUsize,
        writes: AtomicUsize,
    }
    struct Trace {
        db: crate::db::Db,
        pointer: *const Progress,
        progress: Arc<Progress>,
    }
    impl Trace {
        fn install(db: &crate::db::Db) -> Self {
            unsafe extern "C" fn observed(
                event: u32,
                context: *mut std::ffi::c_void,
                statement: *mut std::ffi::c_void,
                _: *mut std::ffi::c_void,
            ) -> i32 {
                if event != rusqlite::ffi::SQLITE_TRACE_PROFILE as u32 {
                    return 0;
                }
                let sql = unsafe { rusqlite::ffi::sqlite3_sql(statement.cast()) };
                if sql.is_null() {
                    return 0;
                }
                let sql = unsafe { std::ffi::CStr::from_ptr(sql) }.to_bytes();
                let progress = unsafe { &*context.cast::<Progress>() };
                if sql
                    .windows(b"FROM boxes WHERE user_id".len())
                    .any(|s| s == b"FROM boxes WHERE user_id")
                {
                    progress.reads.fetch_add(1, Ordering::SeqCst);
                }
                if sql
                    .windows(b"INSERT INTO boxes(".len())
                    .any(|s| s == b"INSERT INTO boxes(")
                {
                    progress.writes.fetch_add(1, Ordering::SeqCst);
                }
                0
            }
            let progress = Arc::new(Progress::default());
            let pointer = Arc::into_raw(progress.clone());
            db.with(|c| {
                let rc = unsafe {
                    rusqlite::ffi::sqlite3_trace_v2(
                        c.handle(),
                        rusqlite::ffi::SQLITE_TRACE_PROFILE as u32,
                        Some(observed),
                        pointer.cast_mut().cast(),
                    )
                };
                assert_eq!(rc, rusqlite::ffi::SQLITE_OK);
                Ok(())
            })
            .unwrap();
            Self {
                db: db.clone(),
                pointer,
                progress,
            }
        }
    }
    impl Drop for Trace {
        fn drop(&mut self) {
            self.db
                .with(|c| {
                    unsafe {
                        rusqlite::ffi::sqlite3_trace_v2(c.handle(), 0, None, std::ptr::null_mut());
                    }
                    Ok(())
                })
                .unwrap();
            unsafe {
                drop(Arc::from_raw(self.pointer));
            }
        }
    }
    async fn until(counter: &AtomicUsize, value: usize, seconds: u64) -> bool {
        tokio::time::timeout(Duration::from_secs(seconds), async {
            while counter.load(Ordering::SeqCst) < value {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .is_ok()
    }
    async fn concurrent_calls(same_user: bool, cancel_second: bool) -> (usize, bool, usize, usize) {
        let mut state = test_state(AuthMode::Github);
        for user in [USER_A, USER_B] {
            state
                .db
                .set_user_limits(
                    user,
                    Some(crate::credits::SpendLimit::Unlimited.stored()),
                    None,
                )
                .unwrap();
        }
        let posts = Arc::new(AtomicUsize::new(0));
        let recorded = posts.clone();
        let app = axum::Router::new()
            .route(
                "/sandbox",
                axum::routing::post(move || {
                    let recorded = recorded.clone();
                    async move {
                        recorded.fetch_add(1, Ordering::SeqCst);
                        std::future::pending::<()>().await;
                    }
                }),
            )
            .fallback(|| async { (StatusCode::NOT_FOUND, "{}") });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        state.daytona = Some(Daytona::new("synthetic-key".into(), Some(base), None));
        let trace = Trace::install(&state.db);
        // Pause a real contended metadata lock before either first-use caller can install its
        // placeholder. Holding it on a dedicated thread avoids async mutex-guard misuse.
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let restart = state.restart.clone();
        let holder = std::thread::spawn(move || {
            let _guard = restart.creating.lock().unwrap();
            held_tx.send(()).unwrap();
            let _ = release_rx.recv_timeout(Duration::from_secs(5));
        });
        held_rx.recv().unwrap();
        let first = {
            let state = state.clone();
            tokio::spawn(
                async move { boxes::ensure_user_box(&state, USER_A, Provider::Daytona).await },
            )
        };
        let second_user = if same_user { USER_A } else { USER_B };
        let second = {
            let state = state.clone();
            tokio::spawn(async move {
                boxes::ensure_user_box(&state, second_user, Provider::Daytona).await
            })
        };
        // Old code reaches four reads and two writes. Earlier serialized admission is allowed
        // to leave the second caller waiting before its read; that is the intended repair.
        let _ = until(&trace.progress.reads, 4, 1).await;
        release_tx.send(()).unwrap();
        holder.join().unwrap();
        assert!(
            until(&posts, 1, 5).await,
            "first actual provider request never arrived"
        );
        let both_wrote = until(&trace.progress.writes, 2, 1).await;
        if both_wrote {
            assert!(
                until(&posts, 2, 5).await,
                "both admitted creates must reach their local provider"
            );
        }
        let reads = trace.progress.reads.load(Ordering::SeqCst);
        let writes = trace.progress.writes.load(Ordering::SeqCst);
        eprintln!(
            "synthetic creation evidence: reads={reads} writes={writes} provider_posts={}",
            posts.load(Ordering::SeqCst)
        );
        let protected = if cancel_second {
            second.abort();
            let _ = second.await;
            let protected = state.restart.creating.lock().unwrap().contains_key(USER_A);
            first.abort();
            let _ = first.await;
            protected
        } else {
            first.abort();
            let _ = first.await;
            let protected = state
                .restart
                .creating
                .lock()
                .unwrap()
                .contains_key(second_user);
            second.abort();
            let _ = second.await;
            protected
        };
        let count = posts.load(Ordering::SeqCst);
        drop(trace);
        (count, protected, reads, writes)
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn same_user_first_use_has_one_provider_creation() {
        let (count, _, _, _) = concurrent_calls(true, true).await;
        assert_eq!(
            count, 1,
            "two actual first-use callers allocated the same user's box concurrently"
        );
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancelling_a_same_user_waiter_preserves_the_active_restart_guard() {
        let (_, protected, _, _) = concurrent_calls(true, true).await;
        assert!(
            protected,
            "a cancelled caller removed the other live creation's restart/orphan guard"
        );
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn independent_users_still_create_in_parallel_and_keep_exact_guards() {
        let (count, protected, reads, writes) = concurrent_calls(false, false).await;
        assert_eq!((count, reads, writes), (2, 4, 2));
        assert!(protected);
    }
    #[tokio::test]
    async fn an_existing_first_use_row_returns_without_provider_creation() {
        let state = test_state(AuthMode::Github);
        let before = live_row(USER_A);
        state.db.upsert_box(&before).unwrap();
        let row = boxes::ensure_user_box(&state, USER_A, Provider::Daytona)
            .await
            .unwrap();
        assert_eq!(row.sandbox_id, before.sandbox_id);
        assert_eq!(row.box_token_hash, before.box_token_hash);
        assert!(state.restart.creating.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn intentional_staged_replacement_still_reaches_provider_and_preserves_live_row() {
        let mut state = test_state(AuthMode::Github);
        state
            .db
            .set_user_limits(
                USER_A,
                Some(crate::credits::SpendLimit::Unlimited.stored()),
                None,
            )
            .unwrap();
        let before = live_row(USER_A);
        state.db.upsert_box(&before).unwrap();
        let (provider, created) = daytona_that_hangs_on_create().await;
        state.daytona = Some(provider);
        let task = {
            let state = state.clone();
            tokio::spawn(async move {
                boxes::create_staged_box(&state, USER_A, Some("arbos-agent-fixture")).await
            })
        };
        tokio::time::timeout(Duration::from_secs(10), created.notified())
            .await
            .unwrap();
        let protected = state.restart.creating.lock().unwrap().contains_key(USER_A);
        let row = state.db.user_box(USER_A).unwrap().unwrap();
        task.abort();
        let _ = task.await;
        assert!(protected);
        assert_eq!(row.sandbox_id, before.sandbox_id);
        assert_eq!(row.box_token_hash, before.box_token_hash);
        assert_eq!(row.device_id, before.device_id);
        assert!(state.restart.creating.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn direct_and_staged_builders_share_the_first_use_admission() {
        for staged in [false, true] {
            let mut state = test_state(AuthMode::Github);
            state
                .db
                .set_user_limits(
                    USER_A,
                    Some(crate::credits::SpendLimit::Unlimited.stored()),
                    None,
                )
                .unwrap();
            let (provider, created) = daytona_that_hangs_on_create().await;
            state.daytona = Some(provider);
            let first = {
                let state = state.clone();
                tokio::spawn(async move {
                    boxes::ensure_user_box(&state, USER_A, Provider::Daytona).await
                })
            };
            tokio::time::timeout(Duration::from_secs(10), created.notified())
                .await
                .unwrap();
            let before = state.db.unfinished_box_creates(now_ms() + 1).unwrap();
            let waited = {
                let waiter = async {
                    if staged {
                        boxes::create_staged_box(&state, USER_A, Some("fixture")).await
                    } else {
                        let mut permit = state.restart.admit_creation(USER_A).await;
                        boxes::create_box(
                            &state,
                            &mut permit,
                            Provider::Daytona,
                            Some("fixture"),
                            true,
                        )
                        .await
                    }
                };
                tokio::pin!(waiter);
                tokio::time::timeout(Duration::from_millis(50), &mut waiter)
                    .await
                    .is_err()
            };
            let unchanged_intent = state.db.unfinished_box_creates(now_ms() + 1).unwrap();
            let protected = state.restart.creating.lock().unwrap().contains_key(USER_A);
            first.abort();
            let _ = first.await;
            assert!(waited, "same-user builder bypassed the owned create lease");
            assert_eq!(before.len(), 1);
            assert_eq!(unchanged_intent.len(), 1);
            assert_eq!(
                unchanged_intent[0].sandbox_name, before[0].sandbox_name,
                "a hung second provider must not masquerade as waiting for admission"
            );
            assert!(protected);
            assert!(state.restart.creating.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn cancelled_creation_retains_uncertain_intent_and_existing_placeholder() {
        let mut state = test_state(AuthMode::Github);
        state
            .db
            .set_user_limits(
                USER_A,
                Some(crate::credits::SpendLimit::Unlimited.stored()),
                None,
            )
            .unwrap();
        let (provider, created) = daytona_that_hangs_on_create().await;
        state.daytona = Some(provider);
        let task = {
            let state = state.clone();
            tokio::spawn(
                async move { boxes::ensure_user_box(&state, USER_A, Provider::Daytona).await },
            )
        };
        tokio::time::timeout(Duration::from_secs(10), created.notified())
            .await
            .unwrap();
        let before = state.db.unfinished_box_creates(now_ms() + 1).unwrap();
        task.abort();
        let _ = task.await;
        let row = tokio::time::timeout(
            Duration::from_secs(1),
            boxes::ensure_user_box(&state, USER_A, Provider::Daytona),
        )
        .await
        .unwrap()
        .unwrap();
        let after = state.db.unfinished_box_creates(now_ms() + 1).unwrap();
        assert!(row.sandbox_id.is_none());
        assert_eq!(before.len(), 1);
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].sandbox_name, before[0].sandbox_name);
        assert!(state.restart.creating.lock().unwrap().is_empty());
    }
}
