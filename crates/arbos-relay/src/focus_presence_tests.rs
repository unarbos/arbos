//! AF-486: a message in the chat you are reading pushed a banner to the phone reading it, and
//! left the chat unread. Both come from one place — the relay's record of which chat a device
//! has on screen.
//!
//! The iPhone holds a room socket per chat in its list and every one of them ticks a presence
//! frame; only the socket of the chat on screen names `focused_chat`. The relay used to read a
//! frame without that field as "this device is looking at nothing" and drop the whole record,
//! so any other chat's tick erased the focus the open chat had just set. With no focus,
//! `announce` pushes the alert and never marks the chat read: the two halves of the report.
//!
//! A frame speaks for its own room alone; when none of them says a chat is on screen, the
//! record stays absent and the push is delivered.

use std::net::SocketAddr;

use axum::routing::get;
use axum::Router;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::tungstenite::Message as TMsg;
use tokio_tungstenite::WebSocketStream;

use arbos_core::ids::{now_ms, ulid};
use arbos_core::wire::{Frame, FrameType};

use crate::db_push::PushToken;
use crate::notify::{announce, device_focused, ChatRef, Sender};
use crate::people::tests_support::{dm, person};
use crate::people::PEOPLE_PROJECT;
use crate::state::test_support::test_state;
use crate::state::{AppState, AuthMode};

type Socket = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn serve(state: AppState) -> SocketAddr {
    let app = Router::new()
        .route("/room/{project}/{chat}", get(crate::rooms::ws_room))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// One phone of `user`: the device id the relay keys focus by, and the raw token its sockets
/// sign in with (legacy `adv_` shape — the relay stores its hash).
fn phone(state: &AppState, user: &str) -> (String, String) {
    let id = ulid();
    let token = format!("adv_{}", ulid().to_ascii_lowercase());
    state
        .db
        .insert_device(
            &id,
            "phone",
            &arbos_core::auth::hash(&token),
            user,
            "paircode",
        )
        .unwrap();
    (id, token)
}

/// The phone's APNs token, so the test can tell a push to it from a push to anything else.
fn apns_token(state: &AppState, device_id: &str, user: &str) -> String {
    let token = format!("{:0<64}", "beef");
    state
        .db
        .upsert_push_token(&PushToken {
            device_id: device_id.to_owned(),
            user_id: user.to_owned(),
            platform: "ios".into(),
            token: token.clone(),
            environment: "development".into(),
            bundle_id: crate::apns::DEFAULT_TOPIC.into(),
            app_version: "1".into(),
            updated_at: now_ms(),
        })
        .unwrap();
    token
}

async fn join(addr: SocketAddr, token: &str, chat: &str) -> Socket {
    let url = format!("ws://{addr}/room/{PEOPLE_PROJECT}/{chat}?token={token}");
    tokio_tungstenite::connect_async(url).await.unwrap().0
}

/// A presence tick from one socket, then a probe whose reply proves the relay handled it
/// (frames on one socket are handled in order).
async fn presence(ws: &mut Socket, focused_chat: Option<&str>) {
    let mut header = json!({ "device": "dev:phone", "at": now_ms() });
    if let Some(c) = focused_chat {
        header["focused_chat"] = json!(c);
    }
    ws.send(TMsg::Binary(
        Frame::new(FrameType::Presence, header, bytes::Bytes::new()).encode(),
    ))
    .await
    .unwrap();
    ws.send(TMsg::Binary(
        Frame::new(FrameType::Probe, json!({}), bytes::Bytes::new()).encode(),
    ))
    .await
    .unwrap();
    while let Some(Ok(m)) = ws.next().await {
        if let TMsg::Binary(b) = m {
            if Frame::decode(b)
                .ok()
                .flatten()
                .is_some_and(|f| f.ty == FrameType::ProbeOk)
            {
                return;
            }
        }
    }
    panic!("no ProbeOk");
}

/// Was the phone's APNs token among everything handed to the provider so far?
async fn pushed_to(state: &AppState, token: &str) -> bool {
    state.push.drain().await;
    state
        .push
        .captured()
        .await
        .iter()
        .any(|o| o.device_token == token)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_rooms_presence_does_not_erase_the_chat_on_screen() {
    let state = test_state(AuthMode::Github);
    let (alice, _) = person(&state, 4861);
    let (bob, _) = person(&state, 4862);
    let (carol, _) = person(&state, 4863);
    let reading = dm(&state, &alice, &bob);
    let listed = dm(&state, &bob, &carol);
    let (device_id, token) = phone(&state, &bob);
    let apns = apns_token(&state, &device_id, &bob);
    let addr = serve(state.clone()).await;
    let alice_s = Sender::user(&state, &alice);

    let mut on_reading = join(addr, &token, &reading).await;
    presence(&mut on_reading, Some(&reading)).await;
    assert!(device_focused(&state, &device_id, &reading).await);

    // The same phone's socket on another chat in the list ticks too, naming no focused chat.
    let mut on_listed = join(addr, &token, &listed).await;
    presence(&mut on_listed, None).await;
    assert!(
        device_focused(&state, &device_id, &reading).await,
        "a frame from another room says nothing about the chat on screen"
    );

    // So the message Bob is watching arrive is not pushed to him, and is read.
    announce(&state, &ChatRef::people(&reading), &alice_s, "m1", "hello").await;
    assert!(
        !pushed_to(&state, &apns).await,
        "no banner for the open chat"
    );
    assert_eq!(state.db.unread_in_chat(&bob, &reading).unwrap(), 0);

    // Leaving the chat (or backgrounding) is a tick from that same room with no focused chat:
    // the record goes, and the next message pushes.
    presence(&mut on_reading, None).await;
    assert!(!device_focused(&state, &device_id, &reading).await);
    announce(&state, &ChatRef::people(&reading), &alice_s, "m2", "there?").await;
    assert!(pushed_to(&state, &apns).await);
    assert_eq!(state.db.unread_in_chat(&bob, &reading).unwrap(), 1);
}
