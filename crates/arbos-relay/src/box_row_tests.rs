//! #2262: a running box's row never loses its sandbox fields. gh_17856706's row was blanked when
//! a planned relay restart (SIGTERM 01:03:04 on Oct 29) cut a create between the first row write
//! and the new sandbox id; the new box then authenticated against a row that named no sandbox,
//! and rolls and always-on lost it.
//!
//! The create that was cut short belonged to the retired Daytona provider, but neither guard is
//! about a provider: the database refuses the blanking write, and the `box_creates` ledger names
//! the sandbox a create is about to make for every provider.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde_json::Value;

use arbos_core::ids::now_ms;

use crate::auth::Principal;
use crate::boxes::{self, Provider};
use crate::db::Sandbox;
use crate::state::test_support::test_state;
use crate::state::{AppState, AuthMode, UpgradeStatus};

fn live_row(user: &str) -> Sandbox {
    Sandbox {
        user_id: user.into(),
        provider: "gvisor".into(),
        sandbox_id: Some("arbos-box-old".into()),
        daytona_state: None,
        device_id: "box:arbos-box-old".into(),
        box_token_hash: "hash-of-the-running-box".into(),
        mcp_token: "mcp-old".into(),
        snapshot: Some("arbos-agent-0.1.179".into()),
        last_started_at: Some(1_000),
        last_stopped_at: None,
        last_seen_at: Some(now_ms()),
        box_url: Some("http://127.0.0.1:7788".into()),
        preview_token: None,
        local_pid: None,
        migrated_from: Vec::new(),
    }
}

/// A create that fails (here: no agentd binary to spawn) over a live row leaves the row on the box
/// the user has, and clears the create ledger so the failed attempt is not reported as cut short.
async fn failed_create_over_live_row(state: &AppState, user: &str) {
    state.db.upsert_box(&live_row(user)).expect("live row");
    let mut permit = state.restart.admit_creation(user).await;
    boxes::create_box(
        state,
        &mut permit,
        Provider::Process,
        Some("arbos-agent-0.1.181"),
    )
    .await
    .expect_err("no agentd binary to spawn");
}

/// The incident, replayed: before the fix the row came back as the create's placeholder — no
/// sandbox_id, snapshot, device_id, box_url or last_started_at — with a fresh token hash.
#[tokio::test]
async fn a_create_that_fails_leaves_the_live_row_whole() {
    let mut state = test_state(AuthMode::Github);
    std::sync::Arc::get_mut(&mut state.cfg)
        .expect("one holder")
        .agentd_bin = "/nonexistent/arbos-agentd".into();
    failed_create_over_live_row(&state, "gh_live").await;

    let row = state.db.user_box("gh_live").unwrap().expect("row");
    let before = live_row("gh_live");
    assert_eq!(row.sandbox_id, before.sandbox_id);
    assert_eq!(row.snapshot, before.snapshot);
    assert_eq!(row.device_id, before.device_id);
    assert_eq!(row.box_url, before.box_url);
    assert_eq!(row.last_started_at, before.last_started_at);
    assert_eq!(row.box_token_hash, before.box_token_hash);
    assert!(state
        .db
        .unfinished_box_creates(now_ms() + 1)
        .unwrap()
        .is_empty());
}

/// What a create cut short by a restart leaves behind is findable: its `box_creates` row names the
/// sandbox the provider may be running and the sandbox it was replacing, and the log-only check
/// reports it once it is stale and no create is running in this process.
#[tokio::test]
async fn a_cut_short_create_is_recorded_and_reported() {
    let state = test_state(AuthMode::Github);
    state.db.upsert_box(&live_row("gh_live")).unwrap();
    state
        .db
        .begin_box_create(
            "gh_live",
            "arbos-box-gh-live",
            "gvisor",
            Some("arbos-agent-0.1.181"),
            Some("arbos-box-old"),
        )
        .unwrap();

    let left = state.db.unfinished_box_creates(now_ms() + 1).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].sandbox_name, "arbos-box-gh-live");
    assert_eq!(left[0].replacing.as_deref(), Some("arbos-box-old"));
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
    assert_eq!(found[0]["sandbox_name"], Value::from("arbos-box-gh-live"));
    assert_eq!(found[0]["provider"], Value::from("gvisor"));

    // A create running in this process is this process's business, not a finding.
    let mut permit = state.restart.admit_creation("gh_live").await;
    let _creating = crate::state::CreatingGuard::new(&mut permit);
    assert!(boxes::sandboxless_live_boxes(&state).await.is_empty());
}

/// The DB itself refuses any write that would drop a live row's sandbox (a stale write-back of a
/// row read mid-create included).
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
    assert_eq!(row.sandbox_id.as_deref(), Some("arbos-box-old"));
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
    assert!(state
        .db
        .user_box("gh_new")
        .unwrap()
        .unwrap()
        .sandbox_id
        .is_none());
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
    use std::sync::Arc;
    use std::time::Duration;

    const USER_A: &str = "gh_fixture_create_a";
    const USER_B: &str = "gh_fixture_create_b";
    #[derive(Default)]
    struct Progress {
        reads: AtomicUsize,
        writes: AtomicUsize,
        creates: AtomicUsize,
        /// Which caller (1 or 2) made the first provider arrival, so a test cancels the one
        /// that is waiting whichever of the two won the admission.
        owner: AtomicUsize,
    }
    tokio::task_local! {
        static CALLER: usize;
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
                if event != rusqlite::ffi::SQLITE_TRACE_PROFILE {
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
                if sql
                    .windows(b"INSERT INTO box_creates(".len())
                    .any(|s| s == b"INSERT INTO box_creates(")
                {
                    progress.creates.fetch_add(1, Ordering::SeqCst);
                    let caller = CALLER.try_with(|c| *c).unwrap_or(0);
                    let _ = progress.owner.compare_exchange(
                        0,
                        caller,
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    );
                }
                0
            }
            let progress = Arc::new(Progress::default());
            let pointer = Arc::into_raw(progress.clone());
            db.with(|c| {
                let rc = unsafe {
                    rusqlite::ffi::sqlite3_trace_v2(
                        c.handle(),
                        rusqlite::ffi::SQLITE_TRACE_PROFILE,
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
        let state = test_state(AuthMode::Github);
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
        // The Daytona stand-in these used went with the provider. A process create takes
        // `local_children` as its first provider step, so holding it parks every admitted create
        // where the hung `POST /sandbox` did; the create ledger's insert counts the arrivals.
        let _provider = state.local_children.clone().lock_owned().await;
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
            tokio::spawn(CALLER.scope(1, async move {
                boxes::ensure_user_box(&state, USER_A, Provider::Process).await
            }))
        };
        let second_user = if same_user { USER_A } else { USER_B };
        let second = {
            let state = state.clone();
            tokio::spawn(CALLER.scope(2, async move {
                boxes::ensure_user_box(&state, second_user, Provider::Process).await
            }))
        };
        // Old code reaches four reads and two writes. Earlier serialized admission is allowed
        // to leave the second caller waiting before its read; that is the intended repair.
        let _ = until(&trace.progress.reads, 4, 1).await;
        let posts = &trace.progress.creates;
        release_tx.send(()).unwrap();
        holder.join().unwrap();
        assert!(
            until(posts, 1, 5).await,
            "first actual provider request never arrived"
        );
        // A process create writes its row twice before the provider step (the placeholder, then
        // the sandbox id), so a third write is the second caller's.
        let both_wrote = until(&trace.progress.writes, 3, 1).await;
        if both_wrote {
            assert!(
                until(posts, 2, 5).await,
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
            // Which of two callers parked on one lock is admitted first is the platform's choice,
            // so the waiter is whichever did not reach the provider.
            let (waiter, active) = if trace.progress.owner.load(Ordering::SeqCst) == 2 {
                (first, second)
            } else {
                (second, first)
            };
            waiter.abort();
            let _ = waiter.await;
            let protected = state.restart.creating.lock().unwrap().contains_key(USER_A);
            active.abort();
            let _ = active.await;
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
        let count = trace.progress.creates.load(Ordering::SeqCst);
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
        assert_eq!((count, reads, writes), (2, 4, 4));
        assert!(protected);
    }
    #[tokio::test]
    async fn an_existing_first_use_row_returns_without_provider_creation() {
        let state = test_state(AuthMode::Github);
        let before = live_row(USER_A);
        state.db.upsert_box(&before).unwrap();
        let row = boxes::ensure_user_box(&state, USER_A, Provider::Process)
            .await
            .unwrap();
        assert_eq!(row.sandbox_id, before.sandbox_id);
        assert_eq!(row.box_token_hash, before.box_token_hash);
        assert!(state.restart.creating.lock().unwrap().is_empty());
    }

    /// Park an admitted process create at its first provider step; `created` is the ledger row
    /// that names the sandbox before the provider is asked.
    async fn provider_that_hangs_on_create(
        state: &AppState,
    ) -> tokio::sync::OwnedMutexGuard<std::collections::HashMap<String, tokio::process::Child>>
    {
        state.local_children.clone().lock_owned().await
    }
    async fn created(state: &AppState) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while state
                .db
                .unfinished_box_creates(now_ms() + 1)
                .unwrap()
                .is_empty()
            {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
    }

    // The staged (row-less) builder went with the retired provider, so only the direct builder
    // is left to share the first-use admission.
    #[tokio::test]
    async fn direct_builder_shares_the_first_use_admission() {
        let state = test_state(AuthMode::Github);
        state
            .db
            .set_user_limits(
                USER_A,
                Some(crate::credits::SpendLimit::Unlimited.stored()),
                None,
            )
            .unwrap();
        let _provider = provider_that_hangs_on_create(&state).await;
        let first = {
            let state = state.clone();
            tokio::spawn(
                async move { boxes::ensure_user_box(&state, USER_A, Provider::Process).await },
            )
        };
        created(&state).await;
        let before = state.db.unfinished_box_creates(now_ms() + 1).unwrap();
        let waited = {
            let waiter = async {
                let mut permit = state.restart.admit_creation(USER_A).await;
                boxes::create_box(&state, &mut permit, Provider::Process, Some("fixture")).await
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
            unchanged_intent[0].started_at, before[0].started_at,
            "a hung second provider must not masquerade as waiting for admission"
        );
        assert!(protected);
        assert!(state.restart.creating.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn cancelled_creation_retains_uncertain_intent_and_existing_placeholder() {
        let state = test_state(AuthMode::Github);
        state
            .db
            .set_user_limits(
                USER_A,
                Some(crate::credits::SpendLimit::Unlimited.stored()),
                None,
            )
            .unwrap();
        let _provider = provider_that_hangs_on_create(&state).await;
        let task = {
            let state = state.clone();
            tokio::spawn(
                async move { boxes::ensure_user_box(&state, USER_A, Provider::Process).await },
            )
        };
        created(&state).await;
        let before = state.db.unfinished_box_creates(now_ms() + 1).unwrap();
        task.abort();
        let _ = task.await;
        let row = tokio::time::timeout(
            Duration::from_secs(1),
            boxes::ensure_user_box(&state, USER_A, Provider::Process),
        )
        .await
        .unwrap()
        .unwrap();
        let after = state.db.unfinished_box_creates(now_ms() + 1).unwrap();
        // A process create names its sandbox before the provider step; what a cancelled one never
        // has is a started process.
        assert!(row.local_pid.is_none() && row.box_url.is_none());
        assert_eq!(before.len(), 1);
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].sandbox_name, before[0].sandbox_name);
        assert!(state.restart.creating.lock().unwrap().is_empty());
    }
}
