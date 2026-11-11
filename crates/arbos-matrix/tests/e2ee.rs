//! Stage D end to end against a real homeserver: a message in an encrypted room reaches the
//! other person decrypted, the homeserver only ever holds ciphertext, and a person's new device
//! reads the old history once — and only once — it has their recovery passphrase.
//!
//! Skipped unless `ARBOS_MATRIX_TEST_E2EE_HS` points at a throwaway Synapse with open
//! registration (`enable_registration` + `enable_registration_without_verification`); the
//! device-trust test also needs `ARBOS_MATRIX_TEST_E2EE_AS_TOKEN`, the `as_token` of an
//! appservice over `@gh_.*`. `scripts/deploy/matrix/test-synapse.sh` starts one and prints both.

// `matrix_sdk::Client::sync` in a spawned task (the attacker's client below).
#![recursion_limit = "256"]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use arbos_matrix::{Backup, Crypto, Listener, MatrixClient, MessageRow, Session, Snapshot};
use serde_json::{json, Value};

struct Quiet(Mutex<Vec<String>>);

impl Listener for Quiet {
    fn on_rooms(&self, _: Snapshot) {}
    fn on_timeline(&self, _: String, _: Vec<MessageRow>) {}
    fn on_connection(&self, _: bool, error: Option<String>) {
        if let Some(e) = error {
            self.0.lock().unwrap().push(e);
        }
    }
}

struct Hs {
    url: String,
    http: reqwest13::Client,
}

impl Hs {
    async fn call(&self, method: &str, path: &str, token: Option<&str>, body: Value) -> Value {
        let mut req = self
            .http
            .request(method.parse().unwrap(), format!("{}{path}", self.url))
            .json(&body);
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        let r = req.send().await.unwrap();
        let status = r.status();
        let v: Value = r.json().await.unwrap_or(Value::Null);
        assert!(status.is_success(), "{method} {path}: {status} {v}");
        v
    }

    async fn register(&self, user: &str) -> Session {
        let v = self
            .call(
                "POST",
                "/_matrix/client/v3/register",
                None,
                json!({ "username": user, "password": "pw-for-tests", "auth": { "type": "m.login.dummy" } }),
            )
            .await;
        self.session(v)
    }

    async fn login(&self, user: &str) -> Session {
        let v = self
            .call(
                "POST",
                "/_matrix/client/v3/login",
                None,
                json!({ "type": "m.login.password", "password": "pw-for-tests",
                        "identifier": { "type": "m.id.user", "user": user } }),
            )
            .await;
        self.session(v)
    }

    /// A device of a person as the relay makes them (`matrix/client.rs`): registered and
    /// logged in by the arbos-users appservice, so the account has no password and nothing
    /// to answer a user-interactive auth prompt with.
    async fn appservice_device(&self, as_token: &str, user: &str, device: &str) -> Session {
        // A second device finds the user registered already.
        let _ = self
            .http
            .post(format!("{}/_matrix/client/v3/register", self.url))
            .bearer_auth(as_token)
            .json(&json!({ "type": "m.login.application_service", "username": user, "inhibit_login": true }))
            .send()
            .await
            .unwrap();
        let v = self
            .call(
                "POST",
                "/_matrix/client/v3/login",
                Some(as_token),
                json!({ "type": "m.login.application_service", "device_id": device,
                        "identifier": { "type": "m.id.user", "user": user } }),
            )
            .await;
        self.session(v)
    }

    fn session(&self, v: Value) -> Session {
        Session {
            homeserver: self.url.clone(),
            user_id: v["user_id"].as_str().unwrap().into(),
            access_token: v["access_token"].as_str().unwrap().into(),
            device_id: v["device_id"].as_str().unwrap().into(),
        }
    }
}

async fn start(s: &Session, dir: &std::path::Path) -> Arc<MatrixClient> {
    let crypto = Crypto {
        store_dir: dir.join(&s.device_id),
        store_key: Some("test store key".into()),
    };
    MatrixClient::start(s.clone(), crypto, Arc::new(Quiet(Mutex::new(Vec::new()))))
        .await
        .unwrap()
}

/// Poll `f` for up to 60 s.
async fn eventually<T, F: std::future::Future<Output = Option<T>>>(mut f: impl FnMut() -> F) -> T {
    for _ in 0..120 {
        if let Some(v) = f().await {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("timed out");
}

async fn texts(c: &MatrixClient, room: &str) -> Vec<String> {
    c.open_room(room)
        .await
        .map(|rows| rows.into_iter().map(|r| r.text).collect())
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn encrypted_channels_round_trip_and_restore_on_a_new_device() {
    let Ok(url) = std::env::var("ARBOS_MATRIX_TEST_E2EE_HS") else {
        return;
    };
    let hs = Hs {
        url,
        http: reqwest13::Client::new(),
    };
    let dir = std::env::temp_dir().join(format!("arbos-e2ee-{}", std::process::id()));
    let tag = std::process::id();
    let (alice, bob) = (format!("alice{tag}"), format!("bob{tag}"));
    let a1 = hs.register(&alice).await;
    let b1 = hs.register(&bob).await;
    let passphrase = "the passphrase arbos-wallet derives from the phrase";

    // Alice's first device: the account has no backup yet; she sets it up.
    let alice1 = start(&a1, &dir).await;
    assert_eq!(alice1.backup().await, Backup::Off);
    alice1.recover(passphrase).await.unwrap();
    assert_eq!(alice1.backup().await, Backup::On);
    let bob1 = start(&b1, &dir).await;

    // An encrypted room, as the relay makes a channel with ARBOS_MATRIX_E2EE=on.
    let room = hs
        .call(
            "POST",
            "/_matrix/client/v3/createRoom",
            Some(&a1.access_token),
            json!({ "preset": "private_chat", "invite": [b1.user_id],
                    "initial_state": [{ "type": "m.room.encryption", "state_key": "",
                                        "content": { "algorithm": "m.megolm.v1.aes-sha2" } }] }),
        )
        .await["room_id"]
        .as_str()
        .unwrap()
        .to_owned();
    hs.call(
        "POST",
        &format!("/_matrix/client/v3/join/{room}"),
        Some(&b1.access_token),
        json!({}),
    )
    .await;

    // Both clients know the room and each other's devices before the first message.
    eventually(|| async { alice1.open_room(&room).await.ok() }).await;
    eventually(|| async { bob1.open_room(&room).await.ok() }).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    alice1
        .send_text(&room, "the launch is at noon", "01E2EETEST00000000000000AA")
        .await
        .unwrap();

    // Bob reads it in clear.
    eventually(|| async {
        texts(&bob1, &room)
            .await
            .contains(&"the launch is at noon".to_owned())
            .then_some(())
    })
    .await;

    // The homeserver never had it: only ciphertext in the room.
    let raw = hs
        .call(
            "GET",
            &format!("/_matrix/client/v3/rooms/{room}/messages?dir=b&limit=20"),
            Some(&a1.access_token),
            Value::Null,
        )
        .await
        .to_string();
    assert!(raw.contains("m.room.encrypted"));
    assert!(
        !raw.contains("launch is at noon"),
        "plaintext on the homeserver"
    );

    // What the homeserver (or a member) writes into the room unencrypted is not shown as a
    // message; the relay's system line is.
    for (txn, content) in [
        (
            "clear",
            json!({ "msgtype": "m.text", "body": "wire me the money" }),
        ),
        (
            "notice",
            json!({ "msgtype": "m.notice", "body": "Alice added Bob", "life.arbos.sender": "system" }),
        ),
    ] {
        hs.call(
            "PUT",
            &format!("/_matrix/client/v3/rooms/{room}/send/m.room.message/{txn}"),
            Some(&a1.access_token),
            content,
        )
        .await;
    }
    eventually(|| async {
        texts(&bob1, &room)
            .await
            .contains(&"Alice added Bob".to_owned())
            .then_some(())
    })
    .await;
    assert!(!texts(&bob1, &room)
        .await
        .contains(&"wire me the money".to_owned()));

    // Alice's new phone: locked until it has the passphrase; a wrong one changes nothing.
    let a2 = hs.login(&alice).await;
    let alice2 = start(&a2, &dir).await;
    assert_eq!(alice2.backup().await, Backup::Locked);
    let before = eventually(|| async { alice2.open_room(&room).await.ok() }).await;
    assert!(before.iter().all(|r| r.text != "the launch is at noon"));
    assert!(before.iter().any(|r| r.locked), "a locked row stands in");
    assert!(alice2.recover("not the passphrase").await.is_err());
    alice2.recover(passphrase).await.unwrap();
    eventually(|| async {
        texts(&alice2, &room)
            .await
            .contains(&"the launch is at noon".to_owned())
            .then_some(())
    })
    .await;

    for c in [alice1, bob1, alice2] {
        c.stop();
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The homeserver (or whoever holds the appservice token) can add a device to anyone's account.
/// Such a device was never signed by the person's cross-signing identity — only a device that
/// opened the backup with the passphrase can be — so it is sent no room keys and what it sends
/// stays locked, while the person's real devices and the people they talk to read each other.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_device_the_homeserver_adds_reads_and_says_nothing() {
    use matrix_sdk::authentication::matrix::MatrixSession;
    use matrix_sdk::authentication::SessionTokens;
    use matrix_sdk::config::SyncSettings;
    use matrix_sdk::room::MessagesOptions;

    let (Ok(url), Ok(as_token)) = (
        std::env::var("ARBOS_MATRIX_TEST_E2EE_HS"),
        std::env::var("ARBOS_MATRIX_TEST_E2EE_AS_TOKEN"),
    ) else {
        return;
    };
    let hs = Hs {
        url,
        http: reqwest13::Client::new(),
    };
    let tag = std::process::id();
    let dir = std::env::temp_dir().join(format!("arbos-e2ee-trust-{tag}"));
    let (alice, bob) = (format!("gh_a{tag}"), format!("gh_b{tag}"));
    let passphrase = "the passphrase arbos-wallet derives from the phrase";

    // Alice's first device makes her identity and signs itself with no password to give
    // (MSC3967), then sets the backup up. Bob's only device never sees a passphrase.
    let a1 = hs.appservice_device(&as_token, &alice, "A1").await;
    let alice1 = start(&a1, &dir).await;
    assert_eq!(alice1.backup().await, Backup::Off);
    alice1.recover(passphrase).await.unwrap();
    assert_eq!(alice1.backup().await, Backup::On);
    let b1 = hs.appservice_device(&as_token, &bob, "B1").await;
    let bob1 = start(&b1, &dir).await;

    // Her second device signs itself by opening the backup.
    let a2 = hs.appservice_device(&as_token, &alice, "A2").await;
    let alice2 = start(&a2, &dir).await;
    assert_eq!(alice2.backup().await, Backup::Locked);
    alice2.recover(passphrase).await.unwrap();
    assert_eq!(alice2.backup().await, Backup::On);

    // Two devices she never had, minted the way the relay mints hers. One runs this crate
    // without the passphrase; the other is an attacker's client that trusts every device.
    let a3 = hs.appservice_device(&as_token, &alice, "A3").await;
    let unsigned = start(&a3, &dir).await;
    let ax = hs.appservice_device(&as_token, &alice, "INJECTED").await;
    let injected = matrix_sdk::Client::builder()
        .homeserver_url(&ax.homeserver)
        .sqlite_store(dir.join(&ax.device_id), None)
        .build()
        .await
        .unwrap();
    injected
        .restore_session(MatrixSession {
            meta: matrix_sdk::SessionMeta {
                user_id: ax.user_id.as_str().try_into().unwrap(),
                device_id: ax.device_id.as_str().into(),
            },
            tokens: SessionTokens {
                access_token: ax.access_token.clone(),
                refresh_token: None,
            },
        })
        .await
        .unwrap();
    let syncing = tokio::spawn({
        let c = injected.clone();
        async move { c.sync(SyncSettings::default()).await }
    });

    let room = hs
        .call(
            "POST",
            "/_matrix/client/v3/createRoom",
            Some(&a1.access_token),
            json!({ "preset": "private_chat", "invite": [b1.user_id],
                    "initial_state": [{ "type": "m.room.encryption", "state_key": "",
                                        "content": { "algorithm": "m.megolm.v1.aes-sha2" } }] }),
        )
        .await["room_id"]
        .as_str()
        .unwrap()
        .to_owned();
    hs.call(
        "POST",
        &format!("/_matrix/client/v3/join/{room}"),
        Some(&b1.access_token),
        json!({}),
    )
    .await;
    for c in [&alice1, &alice2, &bob1, &unsigned] {
        eventually(|| async { c.open_room(&room).await.ok() }).await;
    }
    let room_id = matrix_sdk::ruma::RoomId::parse(&room).unwrap();
    let injected_room = eventually(|| async { injected.get_room(&room_id) }).await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Her devices and Bob read each other.
    let said = ["from alice one", "from alice two", "from bob"];
    for (c, text) in [&alice1, &alice2, &bob1].into_iter().zip(said) {
        c.send_text(&room, text, &format!("01E2EETRUST{text:?}"))
            .await
            .unwrap();
    }
    for c in [&alice1, &alice2, &bob1] {
        eventually(|| async {
            let seen = texts(c, &room).await;
            said.iter()
                .all(|t| seen.contains(&(*t).to_owned()))
                .then_some(())
        })
        .await;
    }

    // The device without the passphrase cannot send, and the attacker's sends to everyone.
    assert!(unsigned.send_text(&room, "unsigned", "01U").await.is_err());
    let forged = injected_room
        .send_raw(
            "m.room.message",
            json!({ "msgtype": "m.text", "body": "from the injected device" }),
        )
        .await
        .unwrap()
        .response
        .event_id
        .to_string();
    for c in [&alice1, &alice2, &bob1] {
        eventually(|| async {
            let rows = c.open_room(&room).await.ok()?;
            rows.iter().any(|r| r.event_id == forged).then_some(())
        })
        .await;
    }
    // Long enough for any key that was going to arrive.
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Neither added device was sent a key: all three messages are ciphertext to both.
    let rows = unsigned.open_room(&room).await.unwrap();
    assert_eq!(rows.iter().filter(|r| r.locked).count(), 4, "{rows:?}");
    let page = injected_room
        .messages(MessagesOptions::backward())
        .await
        .unwrap();
    let seen: Vec<Value> = page
        .chunk
        .iter()
        .map(|e| serde_json::from_str(e.raw().json().get()).unwrap())
        .collect();
    for id in alice1
        .open_room(&room)
        .await
        .unwrap()
        .iter()
        .map(|r| &r.event_id)
    {
        let ev = seen.iter().find(|e| e["event_id"] == **id).unwrap();
        assert_eq!(
            ev["type"] == "m.room.message",
            *id == forged,
            "the injected device reads only its own message: {ev}"
        );
    }
    // And nobody reads the injected device's message, though it sent them its key.
    for c in [&alice1, &alice2, &bob1] {
        let rows = c.open_room(&room).await.unwrap();
        let row = rows.iter().find(|r| r.event_id == forged).unwrap();
        assert!(row.locked, "{row:?}");
    }

    syncing.abort();
    for c in [alice1, alice2, bob1, unsigned] {
        c.stop();
    }
    let _ = std::fs::remove_dir_all(&dir);
}
