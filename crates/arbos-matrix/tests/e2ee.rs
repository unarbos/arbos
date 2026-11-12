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
            people_native: false,
        }
    }
}

async fn start(s: &Session, dir: &std::path::Path) -> Arc<MatrixClient> {
    start_as(s, dir, true).await
}

async fn start_as(s: &Session, dir: &std::path::Path, first_device: bool) -> Arc<MatrixClient> {
    let crypto = Crypto {
        store_dir: dir.join(&s.device_id),
        store_key: Some("test store key".into()),
        first_device,
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
    assert_eq!(alice1.backup().await.unwrap(), Backup::Off);
    alice1.recover(passphrase).await.unwrap();
    assert_eq!(alice1.backup().await.unwrap(), Backup::On);
    // A typed phrase can be checked against the account though this device has the keys.
    alice1.check_passphrase(passphrase).await.unwrap();
    let wrong = alice1.check_passphrase("not the passphrase").await;
    assert!(
        matches!(wrong, Err(arbos_matrix::Error::Keys(_))),
        "{wrong:?}"
    );
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
    assert_eq!(alice2.backup().await.unwrap(), Backup::Locked);
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
    assert_eq!(alice1.backup().await.unwrap(), Backup::Off);
    // Her second device starts before the first has set the backup up: both were told `Off`.
    let a2 = hs.appservice_device(&as_token, &alice, "A2").await;
    let alice2 = start(&a2, &dir).await;
    assert_eq!(alice2.backup().await.unwrap(), Backup::Off);
    alice1.recover(passphrase).await.unwrap();
    assert_eq!(alice1.backup().await.unwrap(), Backup::On);
    let b1 = hs.appservice_device(&as_token, &bob, "B1").await;
    let bob1 = start(&b1, &dir).await;

    // The second device opens the first's backup (and so signs itself) rather than setting up
    // another one over it: the account's secret storage key is still the first device's.
    let key_path = format!(
        "/_matrix/client/v3/user/{}/account_data/m.secret_storage.default_key",
        a1.user_id
    );
    let key = hs
        .call("GET", &key_path, Some(&a1.access_token), Value::Null)
        .await;
    alice2.recover(passphrase).await.unwrap();
    assert_eq!(alice2.backup().await.unwrap(), Backup::On);
    assert_eq!(alice1.backup().await.unwrap(), Backup::On);
    assert_eq!(
        hs.call("GET", &key_path, Some(&a1.access_token), Value::Null)
            .await,
        key
    );

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

/// A Mac has no wallet to make a recovery phrase with, so it never makes the account's keys
/// (`Crypto::first_device`). Signed in first, it leaves the account for the phone to set up
/// (an identity and backup of the Mac's making would shut the phone out: "a backup already
/// exists"). It is sent no room key until it has opened the phone's backup with the
/// passphrase, and every one after.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_device_that_cannot_set_up_keys_waits_for_one_that_can() {
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
    let dir = std::env::temp_dir().join(format!("arbos-e2ee-later-{tag}"));
    let (alice, bob) = (format!("gh_c{tag}"), format!("gh_d{tag}"));
    let passphrase = "the passphrase arbos-wallet derives from the phrase";

    let m = hs.appservice_device(&as_token, &alice, "MAC").await;
    let mac = start_as(&m, &dir, false).await;
    assert_eq!(mac.backup().await.unwrap(), Backup::Off);
    // Long enough for an identity it must not make.
    tokio::time::sleep(Duration::from_secs(5)).await;
    let p = hs.appservice_device(&as_token, &alice, "PHONE").await;
    let phone = start(&p, &dir).await;
    assert_eq!(phone.backup().await.unwrap(), Backup::Off);
    phone.recover(passphrase).await.unwrap();
    assert_eq!(phone.backup().await.unwrap(), Backup::On);
    let b1 = hs.appservice_device(&as_token, &bob, "B1").await;
    let bob1 = start(&b1, &dir).await;

    let room = hs
        .call(
            "POST",
            "/_matrix/client/v3/createRoom",
            Some(&p.access_token),
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
    for c in [&phone, &bob1, &mac] {
        eventually(|| async { c.open_room(&room).await.ok() }).await;
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    bob1.send_text(&room, "before the phrase", "01E2EELATER0000000000000AA")
        .await
        .unwrap();
    eventually(|| async {
        texts(&phone, &room)
            .await
            .contains(&"before the phrase".to_owned())
            .then_some(())
    })
    .await;

    // The Mac learns the phone set the keys up; without the passphrase it reads and says
    // nothing.
    eventually(|| async { (mac.backup().await.ok()? == Backup::Locked).then_some(()) }).await;
    let rows = eventually(|| async {
        let rows = mac.open_room(&room).await.ok()?;
        (!rows.is_empty()).then_some(rows)
    })
    .await;
    assert!(rows.iter().all(|r| r.locked), "{rows:?}");
    assert!(mac.send_text(&room, "unsigned", "01M").await.is_err());
    assert!(mac.recover("not the passphrase").await.is_err());
    assert_eq!(mac.backup().await.unwrap(), Backup::Locked);

    // With it: the old message from the backup, a new one by its own key, and its own sends.
    mac.recover(passphrase).await.unwrap();
    assert_eq!(mac.backup().await.unwrap(), Backup::On);
    bob1.send_text(&room, "after the phrase", "01E2EELATER0000000000000AB")
        .await
        .unwrap();
    eventually(|| async {
        let seen = texts(&mac, &room).await;
        ["before the phrase", "after the phrase"]
            .iter()
            .all(|t| seen.contains(&(*t).to_owned()))
            .then_some(())
    })
    .await;
    mac.send_text(&room, "from the mac", "01E2EELATER0000000000000AC")
        .await
        .unwrap();
    eventually(|| async {
        texts(&bob1, &room)
            .await
            .contains(&"from the mac".to_owned())
            .then_some(())
    })
    .await;

    for c in [mac, phone, bob1] {
        c.stop();
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Phase 3: a device whose session says `people_native` lists its DMs and groups from the
/// homeserver and reads them as transcript rows. A room is the chat the relay created it as,
/// and a row is from the account that sent it, whatever the event's content says.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn people_chats_are_listed_and_read_from_the_homeserver() {
    let Ok(url) = std::env::var("ARBOS_MATRIX_TEST_E2EE_HS") else {
        return;
    };
    let hs = Hs {
        url,
        http: reqwest13::Client::new(),
    };
    let dir = std::env::temp_dir().join(format!("arbos-people-{}", std::process::id()));
    let tag = std::process::id();
    let (alice, bob) = (format!("pa{tag}"), format!("pb{tag}"));
    let a = Session {
        people_native: true,
        ..hs.register(&alice).await
    };
    // Bob's relay has not flipped him.
    let b = hs.register(&bob).await;
    let (alice1, bob1) = (start(&a, &dir).await, start(&b, &dir).await);

    // Rooms as `matrix/worker.rs::create_room_body` makes them; `claims` is what the room's
    // `life.arbos.chat` says, `created_as` what its create event does.
    let room = |claims: Value, created_as: &'static str, encrypted: bool| {
        let (hs, a, b) = (&hs, &a, &b);
        async move {
            let mut state =
                vec![json!({ "type": "life.arbos.chat", "state_key": "", "content": claims })];
            if encrypted {
                state.push(json!({ "type": "m.room.encryption", "state_key": "",
                                   "content": { "algorithm": "m.megolm.v1.aes-sha2" } }));
            }
            let id = hs
                .call(
                    "POST",
                    "/_matrix/client/v3/createRoom",
                    Some(&a.access_token),
                    json!({ "preset": "private_chat", "invite": [b.user_id],
                            "creation_content": { "life.arbos.chat_id": created_as },
                            "initial_state": state }),
                )
                .await["room_id"]
                .as_str()
                .unwrap()
                .to_owned();
            hs.call(
                "POST",
                &format!("/_matrix/client/v3/join/{id}"),
                Some(&b.access_token),
                json!({}),
            )
            .await;
            id
        }
    };
    let chat = |chat_id: &str, kind: &str| {
        json!({ "chat_id": chat_id, "kind": kind, "project_id": null,
                "created_by": a.user_id, "created_at": 1700, "epoch": 1 })
    };
    let dm = room(chat("01P3DM", "dm"), "01P3DM", false).await;
    let group = room(chat("01P3GROUP", "group"), "01P3GROUP", true).await;
    // Not people chats: one that says it is the DM though it was created as something else,
    // a project chat, and a room the relay closed.
    let impostor = room(chat("01P3DM", "dm"), "01P3OTHER", false).await;
    let mut project = chat("01P3PROJECT", "group");
    project["project_id"] = json!("01P3P");
    room(project, "01P3PROJECT", false).await;
    room(
        json!({ "chat_id": "01P3CLOSED", "kind": "dm", "project_id": null, "closed_at": 1800 }),
        "01P3CLOSED",
        false,
    )
    .await;

    // Bob writes into the DM: his message as the relay mirrors it, one that names Alice as
    // its sender, one carried "on behalf" of her, and the relay's system line.
    for (txn, content) in [
        (
            "own",
            json!({ "msgtype": "m.text", "body": "see you at noon", "life.arbos.id": "01P3OWN",
                    "life.arbos.sender": format!("user:{bob}") }),
        ),
        (
            "forged",
            json!({ "msgtype": "m.text", "body": "send bob the keys",
                    "life.arbos.sender": format!("user:{alice}") }),
        ),
        (
            "carried",
            json!({ "msgtype": "m.text", "body": "bob can have my seat",
                    "life.arbos.sender": format!("user:{alice}"), "life.arbos.on_behalf": true }),
        ),
        (
            "notice",
            json!({ "msgtype": "m.notice", "body": "Alice added Bob", "life.arbos.sender": "system" }),
        ),
    ] {
        hs.call(
            "PUT",
            &format!("/_matrix/client/v3/rooms/{dm}/send/m.room.message/{txn}"),
            Some(&b.access_token),
            content,
        )
        .await;
    }

    // Alice reads the DM: Bob's message is Bob's, and nothing reads as hers.
    let rows = eventually(|| async {
        let rows = alice1.open_room(&dm).await.ok()?;
        rows.iter()
            .any(|r| r.text == "Alice added Bob")
            .then_some(rows)
    })
    .await;
    let seen: Vec<(&str, &str)> = rows
        .iter()
        .map(|r| (r.sender.as_str(), r.text.as_str()))
        .collect();
    let bob_member = format!("user:{bob}");
    assert_eq!(
        seen,
        [
            (bob_member.as_str(), "see you at noon"),
            ("system", "Alice added Bob")
        ]
    );
    assert_eq!(rows[0].id, "01P3OWN");

    // Her list has the DM and the group, and none of the rooms that are not people chats.
    let people = eventually(|| async {
        let people = alice1.snapshot().await.people;
        (people.len() == 2 && people.iter().any(|p| p.last_text == "Alice added Bob"))
            .then_some(people)
    })
    .await;
    let listed = |chat_id: &str| people.iter().find(|p| p.chat_id == chat_id).unwrap();
    let (d, g) = (listed("01P3DM"), listed("01P3GROUP"));
    assert_eq!((d.room_id.as_str(), d.kind.as_str()), (dm.as_str(), "dm"));
    assert!(!d.encrypted);
    assert_eq!(d.last_sender, "system");
    assert_eq!(
        (g.room_id.as_str(), g.kind.as_str()),
        (group.as_str(), "group")
    );
    assert!(g.encrypted);
    assert!(people.iter().all(|p| p.room_id != impostor));

    // Alice answers through the client; Bob, not flipped, reads it and lists no people chats.
    alice1
        .send_text(&dm, "noon it is", "01P3REPLY0000000000000000A")
        .await
        .unwrap();
    eventually(|| async {
        texts(&bob1, &dm)
            .await
            .contains(&"noon it is".to_owned())
            .then_some(())
    })
    .await;
    assert!(bob1.snapshot().await.people.is_empty());

    for c in [alice1, bob1] {
        c.stop();
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Records every timeline the client hands the app, to see a pending row come and go.
struct Seen(Mutex<Vec<Vec<MessageRow>>>);

impl Listener for Seen {
    fn on_rooms(&self, _: Snapshot) {}
    fn on_timeline(&self, _: String, rows: Vec<MessageRow>) {
        self.0.lock().unwrap().push(rows);
    }
    fn on_connection(&self, _: bool, _: Option<String>) {}
}

fn row_json(r: &MessageRow) -> Value {
    serde_json::from_str(&r.json).unwrap()
}

/// Phase 3 verbs in a people chat, two native devices: a send shows before the homeserver has
/// it, a reply names the row it answers, a reaction is the reacting account's and comes off
/// again, a removed message leaves both timelines, and a receipt lands on the row asked for.
/// In an encrypted chat the reply is ciphertext on the homeserver and a reaction is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn people_chats_reply_react_remove_and_read() {
    let Ok(url) = std::env::var("ARBOS_MATRIX_TEST_E2EE_HS") else {
        return;
    };
    let hs = Hs {
        url,
        http: reqwest13::Client::new(),
    };
    let dir = std::env::temp_dir().join(format!("arbos-verbs-{}", std::process::id()));
    let tag = std::process::id();
    let (alice, bob) = (format!("va{tag}"), format!("vb{tag}"));
    let native = |s: Session| Session {
        people_native: true,
        ..s
    };
    let a = native(hs.register(&alice).await);
    let b = native(hs.register(&bob).await);
    let seen = Arc::new(Seen(Mutex::new(Vec::new())));
    let alice1 = MatrixClient::start(
        a.clone(),
        Crypto {
            store_dir: dir.join(&a.device_id),
            store_key: Some("test store key".into()),
            first_device: true,
        },
        seen.clone(),
    )
    .await
    .unwrap();
    let bob1 = start(&b, &dir).await;
    // Cross-signed devices: an encrypted room's keys go to no other kind.
    alice1.recover("alice's passphrase").await.unwrap();
    bob1.recover("bob's passphrase").await.unwrap();

    let room = |chat_id: &'static str, kind: &'static str, encrypted: bool| {
        let (hs, a, b) = (&hs, &a, &b);
        async move {
            let mut state = vec![json!({ "type": "life.arbos.chat", "state_key": "",
                "content": { "chat_id": chat_id, "kind": kind, "project_id": null,
                             "created_by": a.user_id, "created_at": 1700, "epoch": 1 } })];
            if encrypted {
                state.push(json!({ "type": "m.room.encryption", "state_key": "",
                                   "content": { "algorithm": "m.megolm.v1.aes-sha2" } }));
            }
            let id = hs
                .call(
                    "POST",
                    "/_matrix/client/v3/createRoom",
                    Some(&a.access_token),
                    json!({ "preset": "private_chat", "invite": [b.user_id],
                            "creation_content": { "life.arbos.chat_id": chat_id },
                            "initial_state": state }),
                )
                .await["room_id"]
                .as_str()
                .unwrap()
                .to_owned();
            hs.call(
                "POST",
                &format!("/_matrix/client/v3/join/{id}"),
                Some(&b.access_token),
                json!({}),
            )
            .await;
            id
        }
    };
    let dm = room("01P3VERBS", "dm", false).await;
    let group = room("01P3SECRET", "group", true).await;
    // A room that is not a people chat takes none of the new verbs.
    let plain = hs
        .call(
            "POST",
            "/_matrix/client/v3/createRoom",
            Some(&a.access_token),
            json!({ "preset": "private_chat" }),
        )
        .await["room_id"]
        .as_str()
        .unwrap()
        .to_owned();
    for r in [&dm, &group, &plain] {
        eventually(|| async { alice1.open_room(r).await.ok() }).await;
    }
    for r in [&dm, &group] {
        eventually(|| async { bob1.open_room(r).await.ok() }).await;
    }
    let row_of = |c: Arc<MatrixClient>, room: String, id: &'static str| async move {
        eventually(|| async {
            let rows = c.open_room(&room).await.ok()?;
            rows.into_iter().find(|r| r.id == id && !r.pending)
        })
        .await
    };

    // Send: the row is there, pending, before the event; then the event, once.
    let first = alice1
        .send_text(&dm, "lunch at noon?", "01P3VERBSFIRST000000000000")
        .await
        .unwrap();
    {
        let seen = seen.0.lock().unwrap();
        let echo = &seen[0];
        let row = echo.last().unwrap();
        assert!(row.pending && row.id == "01P3VERBSFIRST000000000000");
        assert_eq!(row.text, "lunch at noon?");
    }
    let sent = row_of(alice1.clone(), dm.clone(), "01P3VERBSFIRST000000000000").await;
    assert_eq!(sent.event_id, first);
    let rows = alice1.open_room(&dm).await.unwrap();
    assert_eq!(
        rows.iter().filter(|r| r.text == "lunch at noon?").count(),
        1
    );
    // A send the homeserver refuses leaves no row behind.
    hs.call(
        "PUT",
        &format!("/_matrix/client/v3/rooms/{dm}/state/m.room.power_levels"),
        Some(&a.access_token),
        json!({ "events_default": 50 }),
    )
    .await;
    eventually(|| async {
        bob1.send_text(&dm, "can't", "01P3VERBSREFUSED0000000000")
            .await
            .is_err()
            .then_some(())
    })
    .await;
    assert!(bob1
        .open_room(&dm)
        .await
        .unwrap()
        .iter()
        .all(|r| r.text != "can't"));
    hs.call(
        "PUT",
        &format!("/_matrix/client/v3/rooms/{dm}/state/m.room.power_levels"),
        Some(&a.access_token),
        json!({ "events_default": 0 }),
    )
    .await;

    // Reply: Bob answers the row; Alice's copy names it by its id.
    row_of(bob1.clone(), dm.clone(), "01P3VERBSFIRST000000000000").await;
    eventually(|| async {
        bob1.send(
            &dm,
            "noon works",
            Vec::new(),
            Some(&first),
            "01P3VERBSREPLY000000000000",
        )
        .await
        .ok()
    })
    .await;
    let reply = row_of(alice1.clone(), dm.clone(), "01P3VERBSREPLY000000000000").await;
    assert_eq!(row_json(&reply)["replyTo"], "01P3VERBSFIRST000000000000");
    assert_eq!(reply.sender, format!("user:{bob}"));
    // Not to an event this device never read in this room, and not in a room that is no chat.
    assert!(bob1
        .send(
            &dm,
            "x",
            Vec::new(),
            Some("$nope"),
            "01P3VERBSNOPE0000000000000"
        )
        .await
        .is_err());
    assert!(alice1.react(&plain, &first, "👍", true).await.is_err());
    assert!(alice1.redact(&plain, &first).await.is_err());

    // React: Bob's reaction reads as Bob's on both devices; asking twice sends it once.
    bob1.react(&dm, &first, "👍", true).await.unwrap();
    let reacted = |c: Arc<MatrixClient>, on: bool| {
        let (dm, first, bob) = (dm.clone(), first.clone(), bob.clone());
        async move {
            eventually(|| async {
                let rows = c.open_room(&dm).await.ok()?;
                let v = row_json(rows.iter().find(|r| r.event_id == first)?);
                (v["reactions"]["👍"][format!("user:{bob}")].as_bool() == on.then_some(true))
                    .then_some(v)
            })
            .await
        }
    };
    for c in [&alice1, &bob1] {
        let v = reacted(c.clone(), true).await;
        assert_eq!(v["reactions"].as_object().unwrap().len(), 1, "{v}");
    }
    bob1.react(&dm, &first, "👍", true).await.unwrap();
    assert!(bob1.react(&dm, &first, "", true).await.is_err());
    assert!(bob1.react(&dm, "$nope", "👍", true).await.is_err());
    let raw = hs
        .call(
            "GET",
            &format!("/_matrix/client/v3/rooms/{dm}/messages?dir=b&limit=50"),
            Some(&a.access_token),
            Value::Null,
        )
        .await;
    let reactions = raw["chunk"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == "m.reaction")
        .count();
    assert_eq!(reactions, 1, "the second ask sent nothing");
    bob1.react(&dm, &first, "👍", false).await.unwrap();
    for c in [&alice1, &bob1] {
        reacted(c.clone(), false).await;
    }

    // Read: a private receipt on the row asked for, and a bad id is refused here.
    bob1.mark_read_at(&dm, &first).await.unwrap();
    assert!(bob1.mark_read_at(&dm, "not an event id").await.is_err());
    let synced = hs
        .call(
            "GET",
            "/_matrix/client/v3/sync?timeout=0",
            Some(&b.access_token),
            Value::Null,
        )
        .await;
    let receipts = synced["rooms"]["join"][&dm]["ephemeral"]["events"].to_string();
    assert!(
        receipts.contains(&first) && receipts.contains("m.read.private"),
        "{receipts}"
    );

    // Remove: Bob cannot remove Alice's message; Alice can, and it leaves both timelines.
    assert!(bob1.redact(&dm, &first).await.is_err());
    alice1.redact(&dm, &first).await.unwrap();
    for c in [&alice1, &bob1] {
        eventually(|| async {
            let rows = c.open_room(&dm).await.ok()?;
            rows.iter().all(|r| r.event_id != first).then_some(())
        })
        .await;
    }
    // The answer stays, without a row to name.
    let reply = row_of(alice1.clone(), dm.clone(), "01P3VERBSREPLY000000000000").await;
    assert!(row_json(&reply)["replyTo"].is_null());

    // Encrypted chat: the message and its reply are ciphertext on the homeserver, the reply
    // still names its row, and a reaction is refused rather than sent in the clear.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let secret = alice1
        .send_text(
            &group,
            "the door code is 4417",
            "01P3SECRETFIRST00000000000",
        )
        .await
        .unwrap();
    let got = row_of(bob1.clone(), group.clone(), "01P3SECRETFIRST00000000000").await;
    assert_eq!(got.text, "the door code is 4417");
    bob1.send(
        &group,
        "got the code",
        Vec::new(),
        Some(&secret),
        "01P3SECRETREPLY00000000000",
    )
    .await
    .unwrap();
    let reply = row_of(alice1.clone(), group.clone(), "01P3SECRETREPLY00000000000").await;
    assert_eq!(reply.text, "got the code");
    assert_eq!(row_json(&reply)["replyTo"], "01P3SECRETFIRST00000000000");
    assert!(bob1.react(&group, &secret, "👍", true).await.is_err());
    let raw = hs
        .call(
            "GET",
            &format!("/_matrix/client/v3/rooms/{group}/messages?dir=b&limit=50"),
            Some(&a.access_token),
            Value::Null,
        )
        .await
        .to_string();
    assert!(raw.contains("m.room.encrypted"));
    for clear in ["door code", "got the code", "m.reaction", "m.room.message"] {
        assert!(
            !raw.contains(clear),
            "{clear} in the clear on the homeserver"
        );
    }
    // A reaction the homeserver (or a member's other client) writes there in the clear is
    // nobody's reaction.
    hs.call(
        "PUT",
        &format!("/_matrix/client/v3/rooms/{group}/send/m.reaction/forged"),
        Some(&b.access_token),
        json!({ "m.relates_to": { "rel_type": "m.annotation", "event_id": secret, "key": "👎" } }),
    )
    .await;
    alice1
        .send_text(&group, "after", "01P3SECRETAFTER00000000000")
        .await
        .unwrap();
    row_of(alice1.clone(), group.clone(), "01P3SECRETAFTER00000000000").await;
    let rows = alice1.open_room(&group).await.unwrap();
    let first_row = rows.iter().find(|r| r.event_id == secret).unwrap();
    assert_eq!(row_json(first_row)["reactions"], json!({}));
    assert!(
        rows.iter().all(|r| !r.locked),
        "no locked row for it either"
    );

    for c in [alice1, bob1] {
        c.stop();
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A row whose key arrives after it — a live message to a device no key was sent to, and the
/// history it read before it had the passphrase — becomes the message by itself: the listener
/// is handed the opened rows, with no second `open_room`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_key_that_arrives_after_its_message_opens_the_row() {
    let Ok(url) = std::env::var("ARBOS_MATRIX_TEST_E2EE_HS") else {
        return;
    };
    let hs = Hs {
        url,
        http: reqwest13::Client::new(),
    };
    let tag = std::process::id();
    let dir = std::env::temp_dir().join(format!("arbos-late-key-{tag}"));
    let (alice, bob) = (format!("la{tag}"), format!("lb{tag}"));
    let passphrase = "the passphrase arbos-wallet derives from the phrase";
    let a1 = hs.register(&alice).await;
    let b1 = hs.register(&bob).await;
    let (alice1, bob1) = (start(&a1, &dir).await, start(&b1, &dir).await);
    alice1.recover(passphrase).await.unwrap();
    bob1.recover("bob's passphrase").await.unwrap();
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
    eventually(|| async { alice1.open_room(&room).await.ok() }).await;
    eventually(|| async { bob1.open_room(&room).await.ok() }).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    alice1
        .send_text(&room, "before the new phone", "01LATEKEYBEFORE00000000000")
        .await
        .unwrap();

    // Alice's new phone, not yet given the passphrase: it reads the history locked, and Bob's
    // devices send it no key for what he says next.
    let a2 = hs.login(&alice).await;
    let seen = Arc::new(Seen(Mutex::new(Vec::new())));
    let alice2 = MatrixClient::start(
        a2.clone(),
        Crypto {
            store_dir: dir.join(&a2.device_id),
            store_key: Some("test store key".into()),
            first_device: false,
        },
        seen.clone(),
    )
    .await
    .unwrap();
    let opened = eventually(|| async { alice2.open_room(&room).await.ok() }).await;
    assert!(opened.iter().any(|r| r.locked), "{opened:?}");
    bob1.send_text(
        &room,
        "said while it was locked",
        "01LATEKEYLIVE0000000000000",
    )
    .await
    .unwrap();
    let last = || seen.0.lock().unwrap().last().cloned().unwrap_or_default();
    eventually(|| async { (last().iter().filter(|r| r.locked).count() == 2).then_some(()) }).await;

    // Both keys are in the backup (Alice's first device put them there) before she types the
    // phrase; then the rows open through the listener alone.
    let version = hs
        .call(
            "GET",
            "/_matrix/client/v3/room_keys/version",
            Some(&a1.access_token),
            Value::Null,
        )
        .await["version"]
        .as_str()
        .unwrap()
        .to_owned();
    eventually(|| async {
        let keys = hs
            .call(
                "GET",
                &format!("/_matrix/client/v3/room_keys/keys/{room}?version={version}"),
                Some(&a1.access_token),
                Value::Null,
            )
            .await;
        (keys["sessions"].as_object().map_or(0, |s| s.len()) >= 2).then_some(())
    })
    .await;
    alice2.recover(passphrase).await.unwrap();
    let rows = eventually(|| async {
        let rows = last();
        rows.iter().all(|r| !r.locked).then_some(rows)
    })
    .await;
    let texts: Vec<&str> = rows.iter().map(|r| r.text.as_str()).collect();
    assert_eq!(texts, ["before the new phone", "said while it was locked"]);

    for c in [alice1, bob1, alice2] {
        c.stop();
    }
    let _ = std::fs::remove_dir_all(&dir);
}
