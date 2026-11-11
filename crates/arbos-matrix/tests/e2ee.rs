//! Stage D end to end against a real homeserver: a message in an encrypted room reaches the
//! other person decrypted, the homeserver only ever holds ciphertext, and a person's new device
//! reads the old history once — and only once — it has their recovery passphrase.
//!
//! Skipped unless `ARBOS_MATRIX_TEST_E2EE_HS` points at a throwaway Synapse with open
//! registration (`enable_registration` + `enable_registration_without_verification`).

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

    // Alice's new phone: locked until it has the passphrase; a wrong one changes nothing.
    let a2 = hs.login(&alice).await;
    let alice2 = start(&a2, &dir).await;
    assert_eq!(alice2.backup().await, Backup::Locked);
    let before = eventually(|| async { alice2.open_room(&room).await.ok() }).await;
    assert!(before.iter().all(|r| r.text != "the launch is at noon"));
    assert!(
        before.iter().any(MessageRow::is_locked),
        "a locked row stands in"
    );
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
