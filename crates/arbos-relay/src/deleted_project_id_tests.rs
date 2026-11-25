//! A project id is not a name, and a soft-deleted project's id stays reserved.
//!
//! Reported 2026-11-02 (HIGH, cross-tenant read and write). `Db::project` resolved `id OR name`,
//! and a soft delete only sets `deleted_at` — the row, and the blobs on disk, stay for seven days
//! until `purge_deleted_projects` takes them. So the id branch stopped matching while the id was
//! still the key everything was filed under, and a stranger who named a live project after that
//! id took the lookup's answer for it. `state.allows` then said yes, because the project the
//! lookup handed back really was the stranger's, and `blobs::blob_path` filed the bytes under the
//! path the caller asked for, which was the victim's id. Read and write, both tenants' bytes.
//!
//! The first two tests are that attack. The third is the thing a careless narrowing breaks and
//! so has to keep passing: an ordinary name still finds the project it names. The fourth pins
//! the refusal, which must not become a new way to ask whether an id was ever used.

use axum::body::to_bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};

use crate::auth::Principal;
use crate::avatar_image::SizeQuery;
use crate::people::tests_support::{person, project_of};
use crate::state::test_support::test_state;
use crate::state::{AppState, AuthMode};

async fn put_blob(
    state: &AppState,
    who: &Principal,
    slot: &str,
    key: &str,
    bytes: &'static [u8],
) -> StatusCode {
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    crate::blobs::put(
        State(state.clone()),
        who.clone(),
        Path((slot.to_owned(), key.to_owned())),
        headers,
        axum::body::Bytes::from_static(bytes),
    )
    .await
    .status()
}

async fn get_blob(
    state: &AppState,
    who: &Principal,
    slot: &str,
    key: &str,
) -> (StatusCode, Vec<u8>) {
    let r = crate::blobs::get(
        State(state.clone()),
        who.clone(),
        Path((slot.to_owned(), key.to_owned())),
        Query(SizeQuery::default()),
    )
    .await;
    let status = r.status();
    let body = to_bytes(r.into_body(), 1 << 20)
        .await
        .expect("body")
        .to_vec();
    (status, body)
}

/// The bytes as they sit on disk, which is what the attack was after.
fn on_disk(state: &AppState, slot: &str, key: &str) -> Option<Vec<u8>> {
    std::fs::read(state.cfg.data_dir.join("blobs").join(slot).join(key)).ok()
}

#[tokio::test]
async fn a_name_minted_from_a_deleted_id_does_not_read_that_projects_blobs() {
    let state = test_state(AuthMode::Github);
    let (victor, victor_dev) = person(&state, 9101);
    let (mallory, mallory_dev) = person(&state, 9102);

    let victors = project_of(&state, &victor, "Lium");
    assert_eq!(
        put_blob(
            &state,
            &victor_dev,
            &victors,
            "notes.txt",
            b"victor's notes"
        )
        .await,
        StatusCode::NO_CONTENT
    );
    // Victor deletes the agent. The bytes stay for seven days — that is the restore window.
    assert!(state.db.soft_delete_project(&victors).expect("soft delete"));
    assert!(on_disk(&state, &victors, "notes.txt").is_some(), "unpurged");

    // Mallory names an agent of her own after the id Victor's delete appeared to free.
    project_of(&state, &mallory, &victors);

    let (status, body) = get_blob(&state, &mallory_dev, &victors, "notes.txt").await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "naming an agent after a deleted id does not reach what that id keyed"
    );
    assert_ne!(body, b"victor's notes".to_vec(), "Victor's bytes");
}

#[tokio::test]
async fn a_name_minted_from_a_deleted_id_does_not_write_that_projects_blobs() {
    let state = test_state(AuthMode::Github);
    let (victor, victor_dev) = person(&state, 9103);
    let (mallory, mallory_dev) = person(&state, 9104);

    let victors = project_of(&state, &victor, "Lium");
    assert_eq!(
        put_blob(
            &state,
            &victor_dev,
            &victors,
            "notes.txt",
            b"victor's notes"
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert!(state.db.soft_delete_project(&victors).expect("soft delete"));
    project_of(&state, &mallory, &victors);

    assert_eq!(
        put_blob(
            &state,
            &mallory_dev,
            &victors,
            "notes.txt",
            b"mallory was here"
        )
        .await,
        StatusCode::FORBIDDEN,
        "nor does it overwrite what that id keyed"
    );
    assert_eq!(
        on_disk(&state, &victors, "notes.txt"),
        Some(b"victor's notes".to_vec()),
        "Victor's bytes are still Victor's, byte for byte"
    );
}

/// The regression guard, not a reproduction: this passes before the fix as well, and it is the
/// thing a narrowing of `Db::project` is most likely to break. An ordinary name — one that is no
/// project's id — still resolves, both in the lookup and over a route that takes a name.
#[tokio::test]
async fn an_ordinary_name_still_finds_the_project_it_names() {
    let state = test_state(AuthMode::Github);
    let (mallory, mallory_dev) = person(&state, 9105);
    let hers = project_of(&state, &mallory, "Tern");

    let found = state.db.project("Tern").expect("lookup").expect("by name");
    assert_eq!(found.id, hers, "a name is still a way to say which project");

    assert_eq!(
        put_blob(&state, &mallory_dev, "Tern", "notes.txt", b"hers").await,
        StatusCode::NO_CONTENT
    );
    let (status, body) = get_blob(&state, &mallory_dev, "Tern", "notes.txt").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"hers".to_vec());

    // And the id it was always filed under keeps working, unchanged.
    assert_eq!(
        state.db.project(&hers).expect("lookup").expect("by id").id,
        hers
    );
}

/// The reserved id must not become its own oracle. `GET /projects/{x}` answers a deleted id the
/// same way it answers a ULID nobody ever had — same status, same words — and the squatting
/// project is reachable by its own id throughout, so only the borrowed name is withdrawn.
#[tokio::test]
async fn a_reserved_id_answers_exactly_like_an_id_nobody_ever_had() {
    let state = test_state(AuthMode::Github);
    let (victor, _) = person(&state, 9106);
    let (mallory, mallory_dev) = person(&state, 9107);

    let victors = project_of(&state, &victor, "Lium");
    assert!(state.db.soft_delete_project(&victors).expect("soft delete"));
    let mallorys = project_of(&state, &mallory, &victors);

    let ask = |id: String| {
        let state = state.clone();
        let who = mallory_dev.clone();
        async move {
            match crate::api::get_project(State(state), who, Path(id)).await {
                Ok(r) => (r.status(), String::new()),
                Err((status, msg)) => (status, msg),
            }
        }
    };
    let deleted = ask(victors.clone()).await;
    let never_existed = ask(arbos_core::ids::ulid()).await;
    assert_eq!(
        deleted, never_existed,
        "a reserved id and an unused one are one answer"
    );
    assert_eq!(deleted.0, StatusCode::NOT_FOUND);

    // Mallory's own project is not hidden — it answers to its id, which is the one name for it
    // that nobody else can mint.
    assert_eq!(ask(mallorys).await.0, StatusCode::OK);
}
