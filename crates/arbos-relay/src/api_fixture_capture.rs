//! Fixture tests: each projects/chats helper's DTO JSON matches the captured wire shape.

#![cfg(test)]

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::Json;
use serde_json::{json, Value};

use arbos_core::api::{
    ChatArchivedPayloadDto, FeedbackCapturePayloadDto, PeopleAgentAddedPayloadDto,
    PeopleAgentRemovedPayloadDto, PeopleChatCreatedPayloadDto, PeopleChatDeletedPayloadDto,
    PeopleChatUpdatedPayloadDto, PeopleInviteAcceptedPayloadDto, PeopleMemberAddedPayloadDto,
    PeopleMemberRemovedPayloadDto, PeopleMessagePayloadDto, PeopleProjectSharedPayloadDto,
    ProjectMemberRemovedPayloadDto, PushEventDto, ReleasePublishedPayloadDto,
    SecretExpiringPayloadDto, SecretsPausedPayloadDto, SenderDto, ServerDeletedPayloadDto,
    ServerMemberJoinedPayloadDto, ServerMemberRemovedPayloadDto, ServerUpdatedPayloadDto,
};
use arbos_core::auth::hash;
use arbos_core::skill::Skill;
use arbos_core::wire::BoxState;

use crate::api::{chat_dto, chat_dto_for, project_dto, PairBody};
use crate::auth::Principal;
use crate::channels;
use crate::connectors::providers::{self, Provider};
use crate::connectors::{self, ConnectBody};
use crate::db::{ChannelBinding, ChannelPairCode, Chat, Connector, Project};
use crate::db_accounts::GithubIdentity;
use crate::db_people::{ChatWithMembers, PeopleChat};
use crate::notify::RegisterBody;
use crate::people::{self, tests_support::person};
use crate::signin::{verify_access, RefreshBody, TokenBody};
use crate::state::test_support::test_state;
use crate::state::{AppState, AuthMode};

const FIXED_AT: i64 = 1_700_000_000_000;
/// `active_channel_pair_code` only returns codes that have not expired yet.
const PAIR_EXPIRES_AT: i64 = 4_102_444_800_000;

fn fixtures_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/api")
}

fn load(name: &str) -> Value {
    let path = fixtures_dir().join(name);
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&raw).expect("fixture json")
}

fn assert_dto(name: &str, dto: impl serde::Serialize) {
    let got = serde_json::to_value(dto).expect("serialize dto");
    let path = fixtures_dir().join(name);
    if !path.exists() || std::env::var_os("UPDATE_API_FIXTURES").is_some() {
        let pretty = serde_json::to_string_pretty(&got).expect("pretty");
        std::fs::write(&path, format!("{pretty}\n")).expect("write fixture");
    }
    let want = load(name);
    assert_eq!(got, want, "fixture {name}");
}

async fn body_json(r: Response) -> Value {
    let bytes = axum::body::to_bytes(r.into_body(), 1 << 22)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("json body")
}

fn device_id(p: &Principal) -> String {
    match p {
        Principal::Device { id, .. } => id.clone(),
        Principal::Box { .. } | Principal::Admin => unreachable!("person() returns a device"),
    }
}

fn fix_user_created_at(state: &AppState, user_id: &str) {
    state
        .db
        .with(|c| {
            c.execute(
                "UPDATE users SET created_at = ?2 WHERE id = ?1",
                rusqlite::params![user_id, FIXED_AT],
            )
        })
        .unwrap();
}

fn fix_member_joined(state: &AppState, chat_id: &str, member: &str) {
    state
        .db
        .with(|c| {
            c.execute(
                "UPDATE people_members SET joined_at = ?3 WHERE chat_id = ?1 AND member = ?2",
                rusqlite::params![chat_id, member, FIXED_AT],
            )
        })
        .unwrap();
}

fn insert_project(
    state: &AppState,
    id: &str,
    owner: &str,
    name: &str,
    chat_id: &str,
    avatar_color_index: Option<i64>,
) -> Project {
    let p = Project {
        id: id.into(),
        name: name.into(),
        icon: "Ab".into(),
        instructions: "be helpful".into(),
        models: json!({
            "talking_model": { "id": "anthropic/claude-haiku-5.5", "effort": "off" },
            "subagent_model": { "id": "anthropic/claude-sonnet-5.5", "effort": "medium" },
            "decision_model": { "id": "anthropic/claude-haiku-5.5", "effort": "off" }
        }),
        state: "hibernated_light".into(),
        created_at: FIXED_AT,
        last_active_at: FIXED_AT,
        spend_cap_daily: 25.0,
        color: "#8b7cf6".into(),
        user_id: owner.to_owned(),
        workspace_state: "stopped".into(),
        avatar: Some("oak".into()),
        avatar_color: Some("teal".into()),
        description: "A fixture agent".into(),
        primary_chat_id: Some(chat_id.into()),
        avatar_color_index,
        avatar_leaf: None,
        avatar_leaf_color: None,
        image_key: None,
        avatar_version: 0,
    };
    state.db.insert_project(&p).unwrap();
    state
        .db
        .insert_chat(&Chat {
            id: chat_id.into(),
            project_id: id.into(),
            title: "Primary".into(),
            is_pinned: true,
            position: 0,
            created_at: FIXED_AT,
            doc_epoch: 1,
            kind: "chat".into(),
            participants: Vec::new(),
            forked_from: None,
        })
        .unwrap();
    p
}

fn fixture_state() -> (AppState, String, String, Principal, Project) {
    let state = test_state(AuthMode::Github);
    let (alice, alice_dev) = person(&state, 101);
    let (bob, _) = person(&state, 102);
    fix_user_created_at(&state, &alice);
    fix_user_created_at(&state, &bob);
    let p = insert_project(
        &state,
        "proj_fixture",
        &alice,
        "Fixture",
        "chat_primary",
        Some(3),
    );
    (state, alice, bob, alice_dev, p)
}

#[tokio::test]
async fn project_json_matches_fixture() {
    let (state, _, _, _, p) = fixture_state();
    assert_dto("project_json.json", project_dto(&state, &p).await);
}

#[test]
fn chat_json_matches_fixture() {
    let chat = Chat {
        id: "chat_fixture".into(),
        project_id: "proj_fixture".into(),
        title: "Hello".into(),
        is_pinned: false,
        position: 1,
        created_at: FIXED_AT,
        doc_epoch: 1,
        kind: "chat".into(),
        participants: Vec::new(),
        forked_from: None,
    };
    assert_dto("chat_json.json", chat_dto(&chat));
}

#[tokio::test]
async fn chat_json_for_matches_fixture() {
    let (state, alice, _, _, _) = fixture_state();
    let chat = Chat {
        id: "chat_fixture".into(),
        project_id: "proj_fixture".into(),
        title: "Hello".into(),
        is_pinned: false,
        position: 1,
        created_at: FIXED_AT,
        doc_epoch: 1,
        kind: "chat".into(),
        participants: Vec::new(),
        forked_from: None,
    };
    assert_dto(
        "chat_json_for.json",
        chat_dto_for(&state, &chat, Some(&alice)),
    );
}

#[tokio::test]
async fn member_json_matches_fixtures() {
    let (state, alice, bob, _, p) = fixture_state();
    let dm = PeopleChat {
        id: "people_dm".into(),
        kind: "dm".into(),
        created_by: alice.clone(),
        created_at: FIXED_AT,
        title: None,
        image_key: None,
        avatar_leaf: None,
        avatar_leaf_color: None,
        description: None,
        allowed_agents: "members_own".into(),
    };
    state.db.insert_people_chat(&dm).unwrap();
    state
        .db
        .add_member(
            &dm.id,
            &format!("user:{alice}"),
            "owner",
            &alice,
            &json!({}),
        )
        .unwrap();
    state
        .db
        .add_member(&dm.id, &format!("user:{bob}"), "member", &alice, &json!({}))
        .unwrap();
    fix_member_joined(&state, &dm.id, &format!("user:{alice}"));
    fix_member_joined(&state, &dm.id, &format!("user:{bob}"));
    let human = state
        .db
        .members_of(&dm.id)
        .unwrap()
        .into_iter()
        .find(|m| m.user_id() == Some(alice.as_str()))
        .unwrap();
    assert_dto("member_json_human.json", people::member_dto(&state, &human));

    let agent_chat = PeopleChat {
        id: "people_agent".into(),
        kind: "group".into(),
        created_by: alice.clone(),
        created_at: FIXED_AT,
        title: Some("With agent".into()),
        image_key: None,
        avatar_leaf: None,
        avatar_leaf_color: None,
        description: None,
        allowed_agents: "members_own".into(),
    };
    state.db.insert_people_chat(&agent_chat).unwrap();
    state
        .db
        .add_member(
            &agent_chat.id,
            &format!("user:{alice}"),
            "owner",
            &alice,
            &json!({}),
        )
        .unwrap();
    state
        .db
        .add_member(
            &agent_chat.id,
            &format!("agent:{}", p.id),
            "agent",
            &alice,
            &json!({ "reply_policy": "mention", "restrict_guests": false }),
        )
        .unwrap();
    fix_member_joined(&state, &agent_chat.id, &format!("user:{alice}"));
    fix_member_joined(&state, &agent_chat.id, &format!("agent:{}", p.id));
    let agent = state
        .db
        .members_of(&agent_chat.id)
        .unwrap()
        .into_iter()
        .find(|m| m.project_id() == Some(p.id.as_str()))
        .unwrap();
    assert_dto("member_json_agent.json", people::member_dto(&state, &agent));
}

#[tokio::test]
async fn people_chat_json_matches_fixtures() {
    let (state, alice, bob, alice_dev, _) = fixture_state();
    let dm = PeopleChat {
        id: "people_dm".into(),
        kind: "dm".into(),
        created_by: alice.clone(),
        created_at: FIXED_AT,
        title: None,
        image_key: None,
        avatar_leaf: None,
        avatar_leaf_color: None,
        description: None,
        allowed_agents: "members_own".into(),
    };
    state.db.insert_people_chat(&dm).unwrap();
    state
        .db
        .add_member(
            &dm.id,
            &format!("user:{alice}"),
            "owner",
            &alice,
            &json!({}),
        )
        .unwrap();
    state
        .db
        .add_member(&dm.id, &format!("user:{bob}"), "member", &alice, &json!({}))
        .unwrap();
    fix_member_joined(&state, &dm.id, &format!("user:{alice}"));
    fix_member_joined(&state, &dm.id, &format!("user:{bob}"));
    let cm = ChatWithMembers {
        chat: state.db.people_chat(&dm.id).unwrap().unwrap(),
        members: state.db.members_of(&dm.id).unwrap(),
    };
    let device_id = match &alice_dev {
        Principal::Device { id, .. } => id.clone(),
        _ => "dev_fixture".into(),
    };
    assert_dto(
        "people_chat_json_with_me.json",
        people::people_chat_dto(&state, &cm, &alice, &device_id),
    );
    assert_dto(
        "people_chat_json_without_me.json",
        people::people_chat_dto(&state, &cm, "gh_999", &device_id),
    );
}

#[tokio::test]
async fn project_members_json_matches_fixture() {
    let (state, _, _, _, p) = fixture_state();
    assert_dto(
        "project_members_json.json",
        people::project_members_dto(&state, &p),
    );
}

#[tokio::test]
async fn group_json_matches_fixture() {
    let (state, alice, bob, _, p) = fixture_state();
    let other = insert_project(&state, "proj_other", &bob, "Other", "chat_other", None);
    let _ = alice;
    let group = Chat {
        id: "group_fixture".into(),
        project_id: p.id.clone(),
        title: "Group · Other".into(),
        is_pinned: false,
        position: 2,
        created_at: FIXED_AT,
        doc_epoch: 1,
        kind: "group".into(),
        participants: vec![p.id.clone(), other.id.clone()],
        forked_from: None,
    };
    state.db.insert_chat(&group).unwrap();
    assert_dto(
        "group_json.json",
        crate::groups::group_dto(&state, &group).await,
    );
}

#[test]
fn user_card_fixtures() {
    let state = test_state(AuthMode::Github);
    // Legacy device-token account (no GitHub login).
    state
        .db
        .with(|c| {
            c.execute(
                "INSERT INTO users(id, snapshot, spend_cap_usd, created_at) VALUES ('u_mac', NULL, 0, ?1)",
                rusqlite::params![FIXED_AT],
            )
        })
        .unwrap();
    state
        .db
        .insert_device("dev-mac", "Jacob's Mac", "tok", "u_mac", "paircode")
        .unwrap();
    let legacy = state.db.user_card("u_mac").unwrap().unwrap();
    assert_dto("user_card_legacy.json", legacy);

    let (alice, _) = person(&state, 301);
    fix_user_created_at(&state, &alice);
    let card = state.db.user_card(&alice).unwrap().unwrap();
    let with_presence = people::with_presence(&state, card);
    assert_dto("user_card_presence.json", with_presence);

    assert_dto(
        "user_card_github.json",
        people::github_status_card(
            999_001,
            "ghostuser",
            Some("https://avatars.githubusercontent.com/u/999001?v=4"),
            Some("Ghost"),
        ),
    );
}

fn fix_device_times(state: &AppState, device_id: &str) {
    state
        .db
        .with(|c| {
            c.execute(
                "UPDATE devices SET created_at = ?2, last_seen_at = ?2 WHERE id = ?1",
                rusqlite::params![device_id, FIXED_AT],
            )
        })
        .unwrap();
}

fn stabilize_me(mut v: Value) -> Value {
    if let Some(device) = v.get_mut("device") {
        device["id"] = json!("dev_fixture");
        device["created_at"] = json!(FIXED_AT);
        device["last_seen_at"] = json!(FIXED_AT);
        device["name"] = json!("Test Mac");
    }
    if let Some(user) = v.get_mut("user") {
        user["created_at"] = json!(FIXED_AT);
        user["last_login_at"] = json!(FIXED_AT);
    }
    if let Some(machine) = v.get_mut("machine") {
        if machine.get("since").and_then(Value::as_i64).unwrap_or(0) > 0 {
            machine["since"] = json!(FIXED_AT);
        }
    }
    v
}

/// `GET /me`, `GET /machine`, and `GET /me/unread` through their handlers (step 6b-2d leftover).
#[tokio::test]
async fn me_and_machine_unread_fixtures() {
    let state = test_state(AuthMode::Github);
    let (user, _) = state
        .db
        .upsert_github_user(
            &GithubIdentity {
                id: 301,
                login: "user301".into(),
                avatar_url: None,
                name: None,
            },
            20.0,
        )
        .unwrap();
    fix_user_created_at(&state, &user.id);
    state
        .db
        .with(|c| {
            c.execute(
                "UPDATE users SET last_login_at = ?2 WHERE id = ?1",
                rusqlite::params![user.id, FIXED_AT],
            )
        })
        .unwrap();
    // Match the checked-in fixtures: $20 grant, no daily cap on /me, spend_cap 0 on /machine.
    state
        .db
        .grant_credits(&user.id, 20.0, "fixture", "test")
        .unwrap();
    state
        .db
        .set_user_limits(&user.id, Some(None), None)
        .unwrap();
    state
        .db
        .with(|c| {
            c.execute(
                "UPDATE users SET spend_cap_usd = 0 WHERE id = ?1",
                rusqlite::params![user.id],
            )
        })
        .unwrap();
    state
        .db
        .insert_device(
            "dev_fixture",
            "Test Mac",
            &hash("jwt:dev_fixture"),
            &user.id,
            "github",
        )
        .unwrap();
    fix_device_times(&state, "dev_fixture");
    let device = Principal::Device {
        id: "dev_fixture".into(),
        token: String::new(),
        user_id: user.id.clone(),
    };

    let me = body_json(crate::signin::me(State(state.clone()), device.clone()).await).await;
    assert_dto("me_without_machine.json", stabilize_me(me));

    insert_project(
        &state,
        "proj_fixture",
        &user.id,
        "Fixture",
        "chat_fixture",
        None,
    );
    let machine = crate::api::machine(State(state.clone()), device.clone())
        .await
        .expect("machine");
    assert_dto("machine_status.json", body_json(machine).await);

    let (alice, _) = person(&state, 201);
    let chat = crate::db_people::PeopleChat {
        id: "people_dm".into(),
        kind: "dm".into(),
        created_by: alice.clone(),
        created_at: FIXED_AT,
        title: None,
        image_key: None,
        avatar_leaf: None,
        avatar_leaf_color: None,
        description: None,
        allowed_agents: "members_own".into(),
    };
    state.db.insert_people_chat(&chat).unwrap();
    for u in [&alice, &user.id] {
        state
            .db
            .add_member(&chat.id, &format!("user:{u}"), "owner", &alice, &json!({}))
            .unwrap();
    }
    for mid in ["m1", "m2"] {
        state
            .db
            .insert_chat_event("people_dm", None, mid, Some(&alice))
            .unwrap();
    }
    let unread =
        body_json(crate::notify::me_unread(State(state.clone()), device.clone()).await).await;
    assert_dto("unread.json", unread);

    state
        .db
        .upsert_box(&crate::db::Sandbox {
            user_id: user.id.clone(),
            provider: "gvisor".into(),
            sandbox_id: Some("sb_fixture".into()),
            daytona_state: None,
            device_id: "box:sb_fixture".into(),
            box_token_hash: "hash".into(),
            mcp_token: "mcp".into(),
            snapshot: None,
            last_started_at: None,
            last_stopped_at: None,
            last_seen_at: None,
            box_url: None,
            preview_token: None,
            local_pid: None,
            migrated_from: Vec::new(),
        })
        .unwrap();
    state.set_box_state(&user.id, BoxState::Awake, None).await;
    let me_with = body_json(crate::signin::me(State(state.clone()), device).await).await;
    assert_dto("me_with_machine.json", stabilize_me(me_with));
}

/// Pair / token / refresh grace through their handlers (step 6b-2d-1). Random tokens and ids are
/// replaced with obviously fake values before comparing; never log the real ones.
#[tokio::test]
async fn pair_response_matches_fixture() {
    let state = test_state(AuthMode::Paircode);
    state.db.new_pairing_code("PAIRFIX1", 600_000).unwrap();
    let r = crate::api::pair(
        State(state.clone()),
        Json(PairBody {
            code: "PAIRFIX1".into(),
            name: Some("Test Mac".into()),
        }),
    )
    .await
    .expect("pair");
    let mut v = body_json(r).await;
    let paired = v["device_id"].as_str().expect("device_id").to_owned();
    assert!(
        v["device_token"]
            .as_str()
            .is_some_and(|t| t.starts_with("adv_")),
        "device token"
    );
    assert!(
        matches!(state.db.device_status(&paired).unwrap(), Some((_, false))),
        "paired device is live"
    );
    v["device_id"] = json!("dev_fixture");
    v["device_token"] = json!("adv_fixture_not_a_real_token");
    assert_dto("pair_response.json", v);
}

fn stabilize_tokens(mut v: Value) -> Value {
    v["access_token"] = json!("fixture_access_token_not_real");
    v["refresh_token"] = json!("arf_fixture_refresh_token_not_real");
    v
}

#[tokio::test]
async fn token_and_refresh_grace_match_fixtures() {
    let state = test_state(AuthMode::Github);
    let (user, _) = state
        .db
        .upsert_github_user(
            &GithubIdentity {
                id: 301,
                login: "user301".into(),
                avatar_url: None,
                name: Some("Fixture User".into()),
            },
            20.0,
        )
        .unwrap();
    state
        .db
        .with(|c| {
            c.execute(
                "UPDATE users SET created_at = ?2, last_login_at = ?2 WHERE id = ?1",
                rusqlite::params![user.id, FIXED_AT],
            )
        })
        .unwrap();
    state
        .db
        .insert_device(
            "dev_fixture",
            "Test Mac",
            &hash("jwt:dev_fixture"),
            &user.id,
            "github",
        )
        .unwrap();
    state
        .db
        .insert_auth_grant(&hash("grant_fixture"), "dev_fixture", &user.id, 60_000)
        .unwrap();

    let r = crate::signin::token(
        State(state.clone()),
        Json(TokenBody {
            grant_code: "grant_fixture".into(),
        }),
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);
    let issued = body_json(r).await;
    let claims = verify_access(
        &state.jwt_key,
        issued["access_token"].as_str().expect("access_token"),
    )
    .expect("access token verifies");
    assert_eq!(
        (claims.user_id.as_str(), claims.device_id.as_str()),
        ("gh_301", "dev_fixture")
    );
    let refresh_token = issued["refresh_token"]
        .as_str()
        .expect("refresh_token")
        .to_owned();
    assert!(refresh_token.starts_with("arf_"), "refresh token prefix");
    assert_dto("token_response.json", stabilize_tokens(issued));

    let refresh = || {
        crate::signin::refresh(
            State(state.clone()),
            Json(RefreshBody {
                refresh_token: refresh_token.clone(),
            }),
        )
    };
    let rotated = body_json(refresh().await).await;
    let again = body_json(refresh().await).await;
    assert_ne!(
        rotated["refresh_token"],
        json!(refresh_token),
        "refresh rotates"
    );
    assert_eq!(
        again, rotated,
        "a re-presented token inside the grace window gets the same pair"
    );
    assert_dto("token_response_refresh_grace.json", stabilize_tokens(again));
}

#[test]
fn health_voices_transcription_fixtures() {
    let state = test_state(AuthMode::Github);
    assert_dto("healthz.json", crate::api::healthz_dto(&state));

    let dir = state.cfg.data_dir.join("voices-fixture");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("voices.json"),
        r#"{
  "default": "marin",
  "voices": [
    {
      "id": "marin",
      "name": "Marin",
      "description": "Warm and clear.",
      "seconds": 2.5,
      "line": "hello",
      "model": "fixture"
    }
  ]
}
"#,
    )
    .unwrap();
    let mut voice_cfg = (*state.voice).clone();
    voice_cfg.voices_dir = dir;
    voice_cfg.default_voice = "marin".into();
    let mut state = state.clone();
    state.voice = std::sync::Arc::new(voice_cfg);
    assert_dto("voices.json", crate::voice::voices_catalog_dto(&state));

    assert_dto(
        "transcription_mock.json",
        crate::proxy::transcription_mock_json(1.5),
    );
}

fn fixture_sender() -> SenderDto {
    SenderDto {
        kind: "user".into(),
        id: "gh_201".into(),
        name: "@user201".into(),
        handle: "user201".into(),
        avatar: None,
        avatar_color: None,
        avatar_url: None,
        avatar_color_index: None,
        avatar_color_index_effective: 1,
        avatar_leaf_effective: None,
        avatar_leaf_color_effective: None,
    }
}

/// Every typed `/me/pushes` event, keyed by its fixture. `push_secret_request.json` is the
/// vaults.rs form; `push_secret_request_secret.json` is the secrets.rs form (no `vault_id`).
fn push_fixture_events() -> Vec<(&'static str, PushEventDto)> {
    let sender = fixture_sender();
    vec![
        (
            "push_people_message.json",
            PushEventDto::PeopleMessage(PeopleMessagePayloadDto {
                chat_id: "people_dm".into(),
                from: "user:gh_201".into(),
                from_handle: "user201".into(),
                seq: 42,
                sender: sender.clone(),
                project_id: None,
                text: None,
            }),
        ),
        (
            "push_people_chat_updated.json",
            PushEventDto::PeopleChatUpdated(PeopleChatUpdatedPayloadDto {
                chat_id: "people_dm".into(),
            }),
        ),
        (
            "push_people_member_added.json",
            PushEventDto::PeopleMemberAdded(PeopleMemberAddedPayloadDto {
                chat_id: "people_dm".into(),
                user_id: "gh_202".into(),
                handle: "user202".into(),
                by_handle: "user201".into(),
            }),
        ),
        (
            "push_people_member_removed.json",
            PushEventDto::PeopleMemberRemoved(PeopleMemberRemovedPayloadDto {
                chat_id: "people_dm".into(),
                user_id: "gh_202".into(),
                handle: "user202".into(),
                by_handle: "user201".into(),
                banned: None,
            }),
        ),
        (
            "push_people_chat_created.json",
            PushEventDto::PeopleChatCreated(PeopleChatCreatedPayloadDto {
                chat_id: "people_dm".into(),
                by: "user:gh_201".into(),
                by_handle: "user201".into(),
            }),
        ),
        (
            "push_people_chat_deleted.json",
            PushEventDto::PeopleChatDeleted(PeopleChatDeletedPayloadDto {
                chat_id: "people_dm".into(),
                by_handle: "user201".into(),
            }),
        ),
        (
            "push_people_agent_added.json",
            PushEventDto::PeopleAgentAdded(PeopleAgentAddedPayloadDto {
                chat_id: "people_dm".into(),
                project_id: "proj_fixture".into(),
                by_handle: "user201".into(),
            }),
        ),
        (
            "push_people_agent_removed.json",
            PushEventDto::PeopleAgentRemoved(PeopleAgentRemovedPayloadDto {
                chat_id: "people_dm".into(),
                project_id: "proj_fixture".into(),
                by_handle: "user201".into(),
            }),
        ),
        (
            "push_people_project_shared.json",
            PushEventDto::PeopleProjectShared(PeopleProjectSharedPayloadDto {
                project_id: "proj_fixture".into(),
                by_handle: "user201".into(),
            }),
        ),
        (
            "push_project_member_removed.json",
            PushEventDto::ProjectMemberRemoved(ProjectMemberRemovedPayloadDto {
                project_id: "proj_fixture".into(),
                by_handle: "user201".into(),
            }),
        ),
        (
            "push_chat_archived.json",
            PushEventDto::ChatArchived(ChatArchivedPayloadDto {
                chat_id: "chat_fixture".into(),
                project_id: "proj_fixture".into(),
                archived_at: Some(FIXED_AT),
            }),
        ),
        (
            "push_people_invite_accepted.json",
            PushEventDto::PeopleInviteAccepted(PeopleInviteAcceptedPayloadDto {
                code: "invite_fixture".into(),
                handle: "user202".into(),
                user_id: "gh_202".into(),
                target_kind: "project".into(),
                target_id: "proj_fixture".into(),
            }),
        ),
        (
            "push_feedback_capture.json",
            PushEventDto::FeedbackCapture(FeedbackCapturePayloadDto {
                request_id: "cap_fixture".into(),
                project_id: "proj_fixture".into(),
                project_name: "Fixture".into(),
                chat_id: Some("chat_fixture".into()),
                preferred_device_id: Some("dev_fixture".into()),
                expires_at: FIXED_AT + 3_600_000,
            }),
        ),
        (
            "push_server_member_removed.json",
            PushEventDto::ServerMemberRemoved(ServerMemberRemovedPayloadDto {
                server_id: "srv_fixture".into(),
                user_id: "gh_202".into(),
                banned: Some(false),
                handle: Some("user202".into()),
            }),
        ),
        (
            "push_server_member_joined.json",
            PushEventDto::ServerMemberJoined(ServerMemberJoinedPayloadDto {
                server_id: "srv_fixture".into(),
                user_id: "gh_202".into(),
                handle: "user202".into(),
            }),
        ),
        (
            "push_server_updated.json",
            PushEventDto::ServerUpdated(ServerUpdatedPayloadDto {
                server_id: "srv_fixture".into(),
                channel_id: Some("ch_fixture".into()),
                deleted: None,
                user_id: None,
                role: None,
            }),
        ),
        (
            "push_server_deleted.json",
            PushEventDto::ServerDeleted(ServerDeletedPayloadDto {
                server_id: "srv_fixture".into(),
            }),
        ),
        (
            "push_machine_ready.json",
            crate::fleet::machine_ready_push(),
        ),
        (
            "push_release_published.json",
            PushEventDto::ReleasePublished(ReleasePublishedPayloadDto {
                platform: "ios".into(),
                channel: "testflight".into(),
                version: "1.2.3".into(),
                build: "456".into(),
                url: "https://testflight.apple.com/join/fixture".into(),
                published_at: FIXED_AT,
            }),
        ),
        (
            "push_secret_request.json",
            crate::vaults::secret_request_push(
                "req_secret_fixture",
                "proj_fixture",
                "Fixture",
                "STRIPE_KEY",
                Some("api.stripe.com".into()),
                "env",
                "vault_fixture",
            ),
        ),
        (
            "push_secret_request_secret.json",
            crate::secrets::secret_request_push(
                "req_secret_fixture",
                "proj_fixture",
                "Fixture",
                "STRIPE_KEY",
                Some("api.stripe.com".into()),
                "env",
            ),
        ),
        (
            "push_secrets_paused.json",
            PushEventDto::SecretsPaused(SecretsPausedPayloadDto { paused: true }),
        ),
        (
            "push_secret_expiring.json",
            PushEventDto::SecretExpiring(SecretExpiringPayloadDto {
                secret_id: "sec_fixture".into(),
                name: "STRIPE_KEY".into(),
                expires_at: Some(FIXED_AT),
                expired: false,
            }),
        ),
        (
            "push_profile_updated.json",
            PushEventDto::ProfileUpdated(arbos_core::api::ProfileUpdatedPayloadDto {
                user_id: "gh_201".into(),
                avatar_url: Some("https://relay.example/avatars/gh_201.jpg?v=1700000000000".into()),
            }),
        ),
        (
            "push_agent_profile_updated.json",
            PushEventDto::AgentProfileUpdated(arbos_core::api::AgentProfileUpdatedPayloadDto {
                project_id: "01FIXTUREAGENT".into(),
                avatar_version: FIXED_AT,
                image_key: Some("avatar-1700000000000.jpg".into()),
            }),
        ),
        (
            "push_notifications_cleared.json",
            PushEventDto::NotificationsCleared(arbos_core::api::NotificationsClearedPayloadDto {
                chat_id: "people_dm".into(),
                project_id: "people".into(),
                badge: 2,
            }),
        ),
    ]
}

/// Drains this device's queue through `GET /me/pushes` and pins the one row's random id/time.
async fn delivered_push(state: &AppState, device: &Principal, name: &str) -> Value {
    let items = body_json(people::me_pushes(State(state.clone()), device.clone()).await).await;
    let [item] = items.as_array().expect("push list").as_slice() else {
        panic!("{name}: expected one push, got {items}");
    };
    let mut item = item.clone();
    assert!(
        item["id"].as_str().is_some_and(|id| !id.is_empty()),
        "{name}: id"
    );
    assert!(item["created_at"].as_i64().is_some(), "{name}: created_at");
    item["id"] = json!(name.trim_end_matches(".json"));
    item["created_at"] = json!(FIXED_AT);
    item
}

#[tokio::test]
async fn push_wire_fixtures() {
    let state = test_state(AuthMode::Github);
    let (user, device) = person(&state, 401);
    for (name, event) in push_fixture_events() {
        state.db.push_user_event(&user, &event, None).unwrap();
        assert_dto(name, delivered_push(&state, &device, name).await);
    }
    // Apps skip kinds they do not know, so rows the relay cannot type must pass through as-is.
    state
        .db
        .push_user(&user, "not_a_real_push_kind", &json!({ "x": 1 }), None)
        .unwrap();
    assert_dto(
        "push_unknown_kind.json",
        delivered_push(&state, &device, "push_unknown_kind.json").await,
    );
}

#[tokio::test]
async fn releases_and_push_register_match_fixtures() {
    let state = test_state(AuthMode::Github);
    let (_, device) = person(&state, 402);
    state
        .db
        .set_release(
            "ios",
            "testflight",
            "1.2.3",
            "456",
            "https://testflight.apple.com/join/fixture",
            FIXED_AT,
        )
        .unwrap();
    state
        .db
        .set_release(
            "mac",
            "stable",
            "1.2.3",
            "456",
            "https://example.com/arbos.dmg",
            FIXED_AT,
        )
        .unwrap();
    let r = crate::releases::current(State(state.clone()), device.clone()).await;
    assert_dto("releases_current.json", body_json(r).await);

    let id = device_id(&device);
    let r = crate::notify::register_push(
        State(state.clone()),
        device,
        Path(id.clone()),
        Json(RegisterBody {
            platform: "ios".into(),
            token: "ab".repeat(32),
            environment: Some("production".into()),
            bundle_id: None,
            app_version: None,
        }),
    )
    .await;
    let mut v = body_json(r).await;
    assert_eq!(v["device_id"], json!(id));
    v["device_id"] = json!("dev_fixture");
    assert_dto("push_register_response.json", v);
}

// ---- connectors / channels / skills / local / feedback (step 6b-2d-3) ---------------------------

fn fixture_connector() -> Connector {
    Connector {
        id: "conn_fixture".into(),
        provider: "github".into(),
        label: "GitHub · token".into(),
        scopes: vec!["read".into(), "search".into()],
        status: "connected".into(),
        config: json!({}),
        created_at: FIXED_AT,
        last_used_at: Some(FIXED_AT),
        last_error: None,
    }
}

#[tokio::test]
async fn connector_fixtures() {
    let state = test_state(AuthMode::Github);
    let (_, device) = person(&state, 501);
    assert_dto(
        "provider_json.json",
        providers::provider_dto(Provider::Github, false, true),
    );
    assert_dto(
        "connector_json.json",
        connectors::connector_dto(&fixture_connector(), true),
    );

    let r = connectors::catalog(State(state.clone()), device.clone())
        .await
        .expect("catalog");
    assert_dto("connectors_catalog.json", body_json(r).await);

    state.db.upsert_connector(&fixture_connector()).unwrap();
    let r = connectors::list(State(state.clone()), device)
        .await
        .expect("list");
    assert_dto("connectors_list.json", body_json(r).await);
}

#[tokio::test]
async fn connect_reply_fixtures() {
    let state = test_state(AuthMode::Github);
    let (_, device) = person(&state, 502);

    let r = connectors::connect(
        State(state.clone()),
        device.clone(),
        Path("google".into()),
        None,
    )
    .await
    .expect("oauth connect");
    let mut v = body_json(r).await;
    let st = v["state"].as_str().expect("state").to_owned();
    let auth_url = v["authUrl"]
        .as_str()
        .expect("authUrl")
        .replace(&st, "oauth_state_fixture");
    v["authUrl"] = json!(auth_url);
    v["state"] = json!("oauth_state_fixture");
    assert_dto("connect_reply_auth.json", v);

    let r = connectors::connect(
        State(state.clone()),
        device,
        Path("github".into()),
        Some(Json(ConnectBody {
            token: Some("ghp_fixture_not_real".into()),
            base_url: None,
            label: None,
        })),
    )
    .await
    .expect("token connect");
    let mut v = body_json(r).await;
    assert!(
        !v.to_string().contains("ghp_fixture_not_real"),
        "connect reply never echoes the token"
    );
    v["connector"]["id"] = json!("conn_fixture");
    v["connector"]["created_at"] = json!(FIXED_AT);
    assert_dto("connect_reply_connector.json", v);
}

#[tokio::test]
async fn channel_fixtures() {
    let (state, _, _, device, p) = fixture_state();
    let binding = ChannelBinding {
        id: "bind_fixture".into(),
        platform: "telegram".into(),
        external_chat_id: "tg_chat_fixture".into(),
        external_user_id: "tg_user_fixture".into(),
        project_id: p.id.clone(),
        chat_id: "chat_primary".into(),
        label: "Fixture chat".into(),
        bound_at: FIXED_AT,
    };
    let code = ChannelPairCode {
        code: "PAIR42".into(),
        project_id: p.id.clone(),
        chat_id: "chat_primary".into(),
        created_at: FIXED_AT,
        expires_at: PAIR_EXPIRES_AT,
    };
    assert_dto("channel_binding_json.json", channels::binding_dto(&binding));
    assert_dto("channel_pair_json.json", channels::pair_dto(&code));

    state.db.upsert_channel_binding(&binding).unwrap();
    state.db.new_channel_pair_code(&code).unwrap();
    let r = channels::overview(State(state.clone()), device)
        .await
        .expect("overview");
    assert_dto("channels_overview.json", body_json(r).await);
}

#[test]
fn skill_json_matches_fixture() {
    let skill = Skill {
        id: "skill_fixture".into(),
        slug: "release-notes".into(),
        name: "Release notes".into(),
        description: "Draft release notes from merged PRs.".into(),
        tags: vec!["writing".into()],
        when_to_use: "When a release is cut.".into(),
        body: "List merged PRs, group by area, keep it short.".into(),
        origin_project_id: Some("proj_fixture".into()),
        shared: true,
        version: 2,
        created_at: FIXED_AT,
        updated_at: FIXED_AT,
    };
    assert_dto("skill_json.json", crate::skills::skill_dto(&skill));
}

#[tokio::test]
async fn local_devices_match_fixture() {
    let state = test_state(AuthMode::Github);
    let (_, device) = person(&state, 503);
    state
        .db
        .local_device_hello("ldev_fixture", "fixture-host", "macos", "/Users/fixture")
        .unwrap();
    state
        .db
        .with(|c| {
            c.execute(
                "UPDATE local_devices SET last_seen = ?1, first_seen = ?1",
                rusqlite::params![FIXED_AT],
            )
        })
        .unwrap();
    let r = crate::local::list(State(state.clone()), device)
        .await
        .expect("local devices");
    assert_dto("local_devices.json", body_json(r).await);
}

#[tokio::test]
async fn feedback_submit_reply_matches_fixture() {
    let state = test_state(AuthMode::Github);
    let (_, device) = person(&state, 504);
    let body: crate::feedback::Submit =
        serde_json::from_value(json!({ "note": "fixture note" })).unwrap();
    let r =
        crate::feedback::submit(State(state.clone()), device, HeaderMap::new(), Json(body)).await;
    assert_eq!(r.status(), StatusCode::CREATED);
    let mut v = body_json(r).await;
    v["id"] = json!("fb_fixture");
    v["created_at"] = json!(FIXED_AT);
    assert_dto("feedback_submit_reply.json", v);
}

#[tokio::test]
async fn create_project_accepts_avatar_color_index() {
    use axum::extract::State;
    use axum::Json;

    use crate::api::{create_project, CreateProjectBody};

    let state = test_state(AuthMode::Github);
    let (alice, alice_dev) = person(&state, 201);
    let r = create_project(
        State(state.clone()),
        alice_dev,
        Json(CreateProjectBody {
            name: "Coloured".into(),
            icon: None,
            color: None,
            instructions: None,
            models: None,
            provider: None,
            avatar: None,
            avatar_color: None,
            description: None,
            avatar_color_index: Some(4),
            avatar_leaf: None,
            avatar_leaf_color: None,
            source_project_id: None,
        }),
    )
    .await
    .expect("create");
    assert_eq!(r.status(), axum::http::StatusCode::OK);
    let p = state
        .db
        .projects_of_user(&alice)
        .unwrap()
        .into_iter()
        .find(|p| p.name == "Coloured")
        .expect("project");
    assert_eq!(p.avatar_color_index, Some(4));
}
