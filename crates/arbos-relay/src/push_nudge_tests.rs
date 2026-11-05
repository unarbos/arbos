//! A queued push should not have to wait for the box's next poll (#2120 follow-up): the relay
//! nudges the socket the box already holds on the project doc room, and the box fetches the
//! queue it would have fetched anyway. These tests measure the wait the nudge removes, and
//! assert the two properties the design rests on — the queue is still the source of truth, and
//! a person's device never sees the frame.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use axum::routing::get;
use axum::Router;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::tungstenite::Message as TMsg;

use arbos_core::auth::hash;
use arbos_core::ids::{now_ms, ulid};
use arbos_core::wire::{self, Frame, FrameType};

use crate::db::{Chat, Sandbox};
use crate::people::tests_support::{person, project_of};
use crate::state::test_support::test_state;
use crate::state::{AppState, AuthMode};

/// What agentd's push poller waits between fetches (`routines::PUSH_POLL`). The nudge is worth
/// making only if it is a small fraction of this.
const PUSH_POLL: Duration = Duration::from_secs(5);

async fn serve(state: AppState) -> SocketAddr {
    let app = Router::new()
        .route("/room/{project}/{chat}", get(super::rooms::ws_room))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

fn box_token(state: &AppState, user: &str) -> String {
    let token = format!("box_{}", ulid().to_ascii_lowercase());
    state
        .db
        .upsert_box(&Sandbox {
            user_id: user.to_owned(),
            provider: "local".into(),
            sandbox_id: Some(ulid()),
            daytona_state: None,
            device_id: String::new(),
            box_token_hash: hash(&token),
            mcp_token: ulid(),
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
    token
}

fn device_token(state: &AppState, user: &str) -> String {
    let token = format!("dev_{}", ulid().to_ascii_lowercase());
    state
        .db
        .insert_device(&ulid(), "phone", &hash(&token), user, "paircode")
        .unwrap();
    token
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Join `room` and come back once the relay has finished the catch-up, so the connection is in
/// the state a running box's is: joined, caught up, and listening.
async fn joined(addr: SocketAddr, room: &str, token: &str, device: &str) -> Socket {
    let url = format!("ws://{addr}/room/{room}?token={token}");
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    ws.send(TMsg::Binary(
        Frame::new(
            FrameType::Hello,
            json!({ "cursor": 0, "device": device, "room": room, "proto": arbos_core::PROTO_VERSION }),
            bytes::Bytes::new(),
        )
        .encode(),
    ))
    .await
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while let Some(f) = next_frame(&mut ws, deadline).await {
        match f.ty {
            FrameType::State => {
                ws.send(TMsg::Binary(
                    Frame::new(
                        FrameType::RowsReq,
                        json!({ "after": 0, "excludeOwn": false }),
                        bytes::Bytes::new(),
                    )
                    .encode(),
                ))
                .await
                .unwrap();
            }
            FrameType::RowsDone => return ws,
            _ => {}
        }
    }
    panic!("the relay never finished the catch-up for {room}");
}

/// The next decodable frame, or `None` once `deadline` passes.
async fn next_frame(ws: &mut Socket, deadline: Instant) -> Option<Frame> {
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        let m = tokio::time::timeout(left, ws.next()).await.ok()??.ok()?;
        if let TMsg::Binary(b) = m {
            if let Ok(Some(f)) = Frame::decode(b) {
                return Some(f);
            }
        }
    }
}

fn new_chat(state: &AppState, project: &str) -> Chat {
    let chat = Chat {
        id: ulid(),
        project_id: project.to_owned(),
        title: "t".into(),
        is_pinned: false,
        position: 0,
        created_at: now_ms(),
        doc_epoch: 1,
        kind: "chat".into(),
        participants: Vec::new(),
        forked_from: None,
    };
    state.db.insert_chat(&chat).unwrap();
    chat
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_queued_push_reaches_the_box_without_waiting_for_its_poll() {
    let state = test_state(AuthMode::Github);
    let (alice, _) = person(&state, 1);
    let project = project_of(&state, &alice, "Jam");
    let box_tok = box_token(&state, &alice);
    let addr = serve(state.clone()).await;
    let doc_room = format!("{project}/{}", wire::PROJECT_DOC_CHAT_ID);
    let mut bx = joined(addr, &doc_room, &box_tok, "box:test").await;

    // A new chat: the event the box used to learn about on its next poll.
    let chat = new_chat(&state, &project);
    let at = Instant::now();
    assert!(
        crate::boxes::chat_created(&state, &chat).await,
        "the push was queued"
    );
    let f = next_frame(&mut bx, at + Duration::from_secs(5))
        .await
        .expect("the box was nudged");
    let waited = at.elapsed();
    assert_eq!(f.ty, FrameType::Nudge);
    assert_eq!(f.header["kind"], wire::NUDGE_PUSHES);
    println!(
        "discovery wait: {} ms by the nudge, against a {} ms poll interval",
        waited.as_millis(),
        PUSH_POLL.as_millis()
    );
    assert!(
        waited * 10 < PUSH_POLL,
        "a nudge that is not an order of magnitude faster than the poll is not worth the frame: {waited:?}"
    );

    // The nudge carried none of the push. What the box then fetches is the whole of it, and it
    // is still there to fetch — a box that missed the frame loses nothing.
    let pushes = state.db.take_pushes(&project).unwrap();
    assert_eq!(pushes.len(), 1, "the queue is still the source of truth");
    let (_, kind, payload) = &pushes[0];
    assert_eq!(kind, crate::boxes::CHAT_OPEN_PUSH);
    assert_eq!(payload["chat_id"], chat.id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_persons_device_on_the_same_room_is_not_sent_the_nudge() {
    let state = test_state(AuthMode::Github);
    let (alice, _) = person(&state, 1);
    let project = project_of(&state, &alice, "Jam");
    let box_tok = box_token(&state, &alice);
    let dev_tok = device_token(&state, &alice);
    let addr = serve(state.clone()).await;
    let doc_room = format!("{project}/{}", wire::PROJECT_DOC_CHAT_ID);
    let mut bx = joined(addr, &doc_room, &box_tok, "box:test").await;
    let mut phone = joined(addr, &doc_room, &dev_tok, "phone:test").await;

    let chat = new_chat(&state, &project);
    assert!(crate::boxes::chat_created(&state, &chat).await);

    assert_eq!(
        next_frame(&mut bx, Instant::now() + Duration::from_secs(5))
            .await
            .expect("the box was nudged")
            .ty,
        FrameType::Nudge
    );
    // Given the box already has it, anything the phone gets in the next half second is either
    // nothing or something else: a frame addressed to boxes is not fanned out to people.
    while let Some(f) = next_frame(&mut phone, Instant::now() + Duration::from_millis(500)).await {
        assert_ne!(f.ty, FrameType::Nudge, "the phone was sent a box's frame");
    }
}
