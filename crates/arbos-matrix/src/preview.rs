//! What a notification says of one encrypted message while the app is not running (phase 5).
//!
//! The relay cannot read an encrypted chat, so its alert says "New message" and carries the
//! event as the homeserver holds it. The phone's notification extension hands that event here
//! with the device's own store; this opens the keys in it and nothing else — no client, no
//! sync, no request — and answers with the words, or with nothing. The extension lives under a
//! small memory ceiling and next to the app, which may have the same store open: nothing here
//! is made, and the one thing ever written is what the app would write on reading the same
//! message (who a room key turned out to be from).

use std::path::Path;

use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::ruma::{RoomId, UserId};
use matrix_sdk::{SqliteCryptoStore, SqliteStoreConfig};
use matrix_sdk_crypto::store::CryptoStore;
use matrix_sdk_crypto::types::events::room::encrypted::EncryptedEvent;
use matrix_sdk_crypto::OlmMachineBuilder;
use serde_json::Value;

use crate::rows;

/// The words of a message, for its notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    /// Who sent it: the account its room key is from (`user:<id>`).
    pub sender: String,
    pub text: String,
}

/// The largest event taken: an alert is at most 4 kB, all of it.
const EVENT_MAX: usize = 4096;

/// The longest text shown, like the relay's alerts (`notify::brief`).
const TEXT_MAX: usize = 140;

/// The preview of `event` (an `m.room.encrypted` event of `room_id`, as JSON), read with the
/// keys in `store_dir`, the store of `device_id` of `user_id` under `store_key`. `None` for
/// anything but a message this device can read under the rule the app shows one by
/// ([`crate::client::TRUST`]): no key for it, a sender device its owner has not signed, a
/// store that is not this device's, a reaction, a row of carried history, a message that
/// names another sender.
pub async fn preview(
    store_dir: &Path,
    store_key: &str,
    user_id: &str,
    device_id: &str,
    room_id: &str,
    event: &str,
) -> Option<Preview> {
    if event.len() > EVENT_MAX {
        return None;
    }
    let user_id = UserId::parse(user_id).ok()?;
    let room_id = RoomId::parse(room_id).ok()?;
    let raw = Raw::<EncryptedEvent>::from_json_string(event.to_owned()).ok()?;
    // Opening a store that is not there would make one, and a device's keys with it.
    if !store_dir.join("matrix-sdk-crypto.sqlite3").is_file() {
        return None;
    }
    // One connection to read with and the one the store keeps for writing.
    let config = SqliteStoreConfig::with_low_memory_config(store_dir)
        .pool_max_size(2)
        .passphrase(Some(store_key));
    let store = SqliteCryptoStore::open_with_config(&config).await.ok()?;
    store.load_account().await.ok()??;
    let machine = OlmMachineBuilder::new(&user_id, device_id.into())
        .with_crypto_store(store)
        .build()
        .await
        .ok()?;
    let decrypted = machine
        .decrypt_room_event(&raw, &room_id, &crate::client::TRUST)
        .await
        .ok()?;
    let ev: Value = serde_json::from_str(decrypted.event.json().get()).ok()?;
    let content = &ev["content"];
    if ev["type"] != "m.room.message"
        || !rows::sender_is_own(&ev)
        || content.get(rows::HISTORY).is_some()
    {
        return None;
    }
    let text: Vec<&str> = content["body"].as_str()?.split_whitespace().collect();
    let mut text = text.join(" ");
    if text.is_empty() {
        // A file with no caption.
        let parts = content["life.arbos.parts"].as_array()?;
        parts.iter().find_map(rows::part_file)?;
        text = "Attachment".to_owned();
    }
    if text.chars().count() > TEXT_MAX {
        text = text.chars().take(TEXT_MAX - 1).collect::<String>() + "…";
    }
    Some(Preview {
        sender: rows::member_of_mxid(ev["sender"].as_str()?),
        text,
    })
}
