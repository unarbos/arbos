//! Sidebar pins (Jacob 2026-10-28, [feedback sidebar-pinning-relay-2026-10-28]): **one ordered
//! list per user** of the rows kept at the top of the Mac sidebar and the iPhone Home — agents,
//! people chats and servers — shared by every device of that user.
//!
//! A pin is a *row key*: `agent:<project id>`, `chat:<people chat id>` or `server:<server id>`.
//! `GET /me/pins` answers the list top first; `PUT /me/pins {pins}` **replaces the whole list**
//! (one call for pin, unpin and reorder; the array's order is the order), drops keys the user
//! cannot see any more (a deleted agent, a chat they left), refuses more than [`PIN_CAP`] with
//! `409 pin_cap`, and queues `pins_changed {pins}` for the user's other devices. The design is
//! the project store's `docs/sidebar-pinning-design.md`.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use rusqlite::params;
use serde::Deserialize;

use arbos_core::api::{PinsChangedPayloadDto, PinsDto, PushEventDto};

use crate::auth::Principal;
use crate::signin::api_error;
use crate::state::AppState;

/// Jacob's cap (2026-10-28 17:10 UTC): five pins, no tiers — the iPhone's number since its
/// device-local pins; a longer Pinned section would push the live list off screen.
pub const PIN_CAP: usize = 5;

pub const PINS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS user_pins (
  user_id TEXT NOT NULL, row_key TEXT NOT NULL, position INTEGER NOT NULL,
  PRIMARY KEY (user_id, row_key)
);
"#;

/// What a row key points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinKind {
    Agent,
    Chat,
    Server,
}

/// `agent:<id>` / `chat:<id>` / `server:<id>` → kind and id; anything else is not a pin.
pub fn parse_key(key: &str) -> Option<(PinKind, &str)> {
    let (kind, id) = key.split_once(':')?;
    if id.is_empty() || id.contains(':') || id.len() > 128 {
        return None;
    }
    let kind = match kind {
        "agent" => PinKind::Agent,
        "chat" => PinKind::Chat,
        "server" => PinKind::Server,
        _ => return None,
    };
    Some((kind, id))
}

/// May this device's user see the row a key names? Unknown rows: no.
fn visible(state: &AppState, principal: &Principal, key: &str) -> bool {
    let Some((kind, id)) = parse_key(key) else {
        return false;
    };
    let acl = match kind {
        PinKind::Agent => id.to_owned(),
        PinKind::Chat => format!("{}{id}", crate::people::PEOPLE_ACL_PREFIX),
        PinKind::Server => format!("{}{id}", crate::matrix::servers::SERVER_BLOB_PREFIX),
    };
    state.allows(principal, &acl)
}

/// The list the relay keeps for a user: unique keys, the first occurrence wins, only rows the
/// user can see, in the order given.
fn clean(state: &AppState, principal: &Principal, pins: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(pins.len());
    for key in pins {
        let key = key.trim();
        if key.is_empty() || out.iter().any(|k| k == key) || !visible(state, principal, key) {
            continue;
        }
        out.push(key.to_owned());
    }
    out
}

impl crate::db::Db {
    /// The user's pins, top first.
    pub fn pins(&self, user_id: &str) -> anyhow::Result<Vec<String>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT row_key FROM user_pins WHERE user_id = ?1 ORDER BY position ASC, row_key ASC",
            )?;
            let rows = st
                .query_map(params![user_id], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    /// Replace the user's list; `pins[0]` is the top.
    pub fn set_pins(&self, user_id: &str, pins: &[String]) -> anyhow::Result<()> {
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            tx.execute("DELETE FROM user_pins WHERE user_id = ?1", params![user_id])?;
            for (i, key) in pins.iter().enumerate() {
                tx.execute(
                    "INSERT INTO user_pins(user_id, row_key, position) VALUES (?1, ?2, ?3)",
                    params![user_id, key, i as i64],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
    }
}

fn user_of(principal: &Principal) -> Result<String, Box<Response>> {
    match principal {
        Principal::Device { user_id, .. } => Ok(user_id.clone()),
        Principal::Box { .. } | Principal::Admin => Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "a signed-in device is required",
        ))),
    }
}

fn device_of(principal: &Principal) -> String {
    match principal {
        Principal::Device { id, .. } => id.clone(),
        Principal::Box { user_id } => format!("box:{user_id}"),
        Principal::Admin => "admin".to_owned(),
    }
}

fn internal(e: impl std::fmt::Display) -> Response {
    api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string())
}

fn dto(pins: Vec<String>) -> Response {
    Json(PinsDto {
        pins,
        cap: PIN_CAP as u32,
    })
    .into_response()
}

/// `GET /me/pins` → `{pins, cap}`. Rows the user can no longer see are dropped here (and the
/// stored list trimmed), so a deleted agent never lingers as a pin.
pub async fn get_pins(State(state): State<AppState>, principal: Principal) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let stored = match state.db.pins(&me) {
        Ok(p) => p,
        Err(e) => return internal(e),
    };
    let kept = clean(&state, &principal, &stored);
    if kept != stored {
        if let Err(e) = state.db.set_pins(&me, &kept) {
            return internal(e);
        }
    }
    dto(kept)
}

#[derive(Debug, Deserialize)]
pub struct PutPinsBody {
    #[serde(default)]
    pub pins: Vec<String>,
}

/// `PUT /me/pins {pins}` → `{pins, cap}`: the whole list, replaced. Over the cap after
/// cleaning → `409 pin_cap`, nothing changes. The other devices get `pins_changed`.
pub async fn put_pins(
    State(state): State<AppState>,
    principal: Principal,
    Json(b): Json<PutPinsBody>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    if b.pins.len() > 64 {
        return api_error(StatusCode::BAD_REQUEST, "bad_request", "too many pins");
    }
    let pins = clean(&state, &principal, &b.pins);
    if pins.len() > PIN_CAP {
        return api_error(
            StatusCode::CONFLICT,
            "pin_cap",
            format!("you can pin up to {PIN_CAP} chats"),
        );
    }
    let before = match state.db.pins(&me) {
        Ok(p) => p,
        Err(e) => return internal(e),
    };
    if pins != before {
        if let Err(e) = state.db.set_pins(&me, &pins) {
            return internal(e);
        }
        let _ = state.db.push_user_except_event(
            &me,
            &device_of(&principal),
            &PushEventDto::PinsChanged(PinsChangedPayloadDto { pins: pins.clone() }),
        );
    }
    dto(pins)
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;
    use crate::people::tests_support::{dm, person, project_of};
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    async fn body(r: Response) -> (StatusCode, Value) {
        let status = r.status();
        let bytes = axum::body::to_bytes(r.into_body(), 1 << 20)
            .await
            .expect("body");
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn put(state: &AppState, who: &Principal, pins: &[&str]) -> (StatusCode, Value) {
        let r = put_pins(
            State(state.clone()),
            who.clone(),
            Json(PutPinsBody {
                pins: pins.iter().map(|s| s.to_string()).collect(),
            }),
        )
        .await;
        body(r).await
    }

    async fn get(state: &AppState, who: &Principal) -> Vec<String> {
        let (status, v) = body(get_pins(State(state.clone()), who.clone()).await).await;
        assert_eq!(status, StatusCode::OK);
        v["pins"]
            .as_array()
            .expect("pins")
            .iter()
            .map(|p| p.as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn row_keys_parse_or_are_refused() {
        assert_eq!(parse_key("agent:01X"), Some((PinKind::Agent, "01X")));
        assert_eq!(parse_key("chat:01X"), Some((PinKind::Chat, "01X")));
        assert_eq!(parse_key("server:01X"), Some((PinKind::Server, "01X")));
        for bad in ["agent:", "01X", "team:01X", "agent:a:b", ""] {
            assert_eq!(parse_key(bad), None, "{bad}");
        }
    }

    #[tokio::test]
    async fn put_replaces_and_keeps_the_order_given() {
        let state = test_state(AuthMode::Github);
        let (me, dev) = person(&state, 7001);
        let (other, _) = person(&state, 7002);
        let a = project_of(&state, &me, "Arbos");
        let b = project_of(&state, &me, "Comms");
        let d = dm(&state, &me, &other);
        let a_key = format!("agent:{a}");
        let b_key = format!("agent:{b}");
        let d_key = format!("chat:{d}");

        let (status, v) = put(&state, &dev, &[&d_key, &a_key]).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        assert_eq!(v["pins"], json!([d_key, a_key]));
        assert_eq!(v["cap"], json!(PIN_CAP));
        assert_eq!(get(&state, &dev).await, vec![d_key.clone(), a_key.clone()]);

        // A second PUT is the new truth: reordered, one dropped, one added, duplicates folded.
        let (status, v) = put(&state, &dev, &[&b_key, &a_key, &b_key]).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        assert_eq!(get(&state, &dev).await, vec![b_key.clone(), a_key.clone()]);

        // Empty list clears.
        let (status, _) = put(&state, &dev, &[]).await;
        assert_eq!(status, StatusCode::OK);
        assert!(get(&state, &dev).await.is_empty());
    }

    #[tokio::test]
    async fn the_cap_refuses_a_sixth_pin_and_changes_nothing() {
        let state = test_state(AuthMode::Github);
        let (me, dev) = person(&state, 7011);
        let keys: Vec<String> = (0..6)
            .map(|i| format!("agent:{}", project_of(&state, &me, &format!("Agent {i}"))))
            .collect();
        let five: Vec<&str> = keys[..5].iter().map(String::as_str).collect();
        let (status, _) = put(&state, &dev, &five).await;
        assert_eq!(status, StatusCode::OK);

        let six: Vec<&str> = keys.iter().map(String::as_str).collect();
        let (status, v) = put(&state, &dev, &six).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(v["error"]["type"], json!("pin_cap"), "{v}");
        assert_eq!(
            get(&state, &dev).await,
            keys[..5].to_vec(),
            "the list stayed"
        );

        // Unknown and foreign keys do not count toward the cap: they are dropped first.
        let mut noisy: Vec<&str> = vec!["agent:nope", "team:x", ""];
        noisy.extend(five.iter());
        let (status, v) = put(&state, &dev, &noisy).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        assert_eq!(get(&state, &dev).await, keys[..5].to_vec());
    }

    #[tokio::test]
    async fn pins_are_per_user_and_only_name_rows_the_user_can_see() {
        let state = test_state(AuthMode::Github);
        let (me, dev) = person(&state, 7021);
        let (them, their_dev) = person(&state, 7022);
        let mine = format!("agent:{}", project_of(&state, &me, "Mine"));
        let theirs = format!("agent:{}", project_of(&state, &them, "Theirs"));

        let (status, v) = put(&state, &dev, &[&theirs, &mine]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["pins"], json!([mine]), "their agent is not mine to pin");
        assert!(
            get(&state, &their_dev).await.is_empty(),
            "their list is untouched"
        );

        let (status, _) = put(&state, &their_dev, &[&theirs]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(get(&state, &their_dev).await, vec![theirs.clone()]);
        assert_eq!(get(&state, &dev).await, vec![mine.clone()]);

        // A row that disappears is dropped on the next read.
        state.db.purge_project(&mine["agent:".len()..]).unwrap();
        assert!(get(&state, &dev).await.is_empty());
    }

    #[tokio::test]
    async fn a_change_is_pushed_to_my_other_devices_only() {
        let state = test_state(AuthMode::Github);
        let (me, dev) = person(&state, 7031);
        let other_device = {
            let id = arbos_core::ids::ulid();
            state
                .db
                .insert_device(&id, "d2", &arbos_core::ids::ulid(), &me, "github")
                .unwrap();
            Principal::Device {
                id,
                token: String::new(),
                user_id: me.clone(),
            }
        };
        let key = format!("agent:{}", project_of(&state, &me, "Arbos"));
        let (status, _) = put(&state, &dev, &[&key]).await;
        assert_eq!(status, StatusCode::OK);

        let (_, mine) =
            body(crate::people::me_pushes(State(state.clone()), dev.clone()).await).await;
        assert_eq!(
            mine.as_array().map(Vec::len),
            Some(0),
            "not echoed to the writer"
        );
        let (_, theirs) =
            body(crate::people::me_pushes(State(state.clone()), other_device.clone()).await).await;
        let items = theirs.as_array().expect("pushes");
        assert_eq!(items.len(), 1, "{theirs}");
        assert_eq!(items[0]["kind"], json!("pins_changed"));
        assert_eq!(items[0]["payload"]["pins"], json!([key]));

        // The same list again is a no-op: nothing stored, nothing pushed.
        let (status, _) = put(&state, &dev, &[&key]).await;
        assert_eq!(status, StatusCode::OK);
        let (_, again) =
            body(crate::people::me_pushes(State(state.clone()), other_device.clone()).await).await;
        assert_eq!(again.as_array().map(Vec::len), Some(0));
    }
}
