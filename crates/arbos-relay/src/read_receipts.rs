//! Read receipts (#1484): whether the other people in a chat are told when this person has
//! opened their messages.
//!
//! The rule is enforced here, on the relay, rather than in the clients, because **the relay is
//! itself one of the senders**: its Matrix mirror publishes an `m.read` receipt on the reader's
//! behalf (`matrix::worker`), from call sites a client never touches. A client-side check,
//! however careful, cannot stop that, and a people room also carries presence to agent boxes and
//! to outside agents, which are not our code. So "off" has to mean the relay refuses to pass the
//! receipt on, whatever any client sends.
//!
//! The value is an account default with a per-chat override, the shape `PUT /me/notifications`
//! already uses (`push::eligibility`). The account default lives in its own table; the override
//! stays in the membership row's `settings.read_receipts`, where shipping Macs already write it,
//! and is cleared with `"inherit"`.

use anyhow::Result;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use arbos_core::ids::now_ms;

use crate::auth::Principal;
use crate::db::Db;
use crate::signin::api_error;
use crate::state::AppState;

pub const READ_RECEIPTS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS user_read_receipts_default (
  user_id TEXT PRIMARY KEY, enabled INTEGER NOT NULL, at INTEGER NOT NULL
);
"#;

/// What shipped before this setting existed, so an account that never chose keeps sending.
pub const DEFAULT: bool = true;

/// One chat's answer: share receipts here, do not, or follow the account default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatSetting {
    On,
    Off,
    /// No answer for this chat (`PUT /me/read-receipts` decides).
    Inherit,
}

/// The `read_receipts` field of `PUT /chats/{id}/settings`: `true`, `false`, or `"inherit"` to
/// drop the override. Shipping Macs send the bool, so both shapes have to parse.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ChatSettingBody {
    Flag(bool),
    Named(String),
}

impl ChatSettingBody {
    /// `None` for a word we do not know, which the caller answers with a 400.
    pub fn parse(&self) -> Option<ChatSetting> {
        match self {
            ChatSettingBody::Flag(true) => Some(ChatSetting::On),
            ChatSettingBody::Flag(false) => Some(ChatSetting::Off),
            ChatSettingBody::Named(s) => match s.trim().to_ascii_lowercase().as_str() {
                "on" | "true" => Some(ChatSetting::On),
                "off" | "false" => Some(ChatSetting::Off),
                "inherit" | "default" | "" => Some(ChatSetting::Inherit),
                _ => None,
            },
        }
    }
}

/// The override a membership row holds. Anything that is not a bool reads as no override.
pub fn chat_override(settings: &Value) -> ChatSetting {
    match settings.get("read_receipts").and_then(Value::as_bool) {
        Some(true) => ChatSetting::On,
        Some(false) => ChatSetting::Off,
        None => ChatSetting::Inherit,
    }
}

/// Write the override into a membership's settings object; `Inherit` removes the key, so the
/// account default applies again.
pub fn set_chat_override(settings: &mut Value, setting: ChatSetting) {
    if !settings.is_object() {
        *settings = json!({});
    }
    let Some(map) = settings.as_object_mut() else {
        return;
    };
    match setting {
        ChatSetting::On => {
            map.insert("read_receipts".into(), json!(true));
        }
        ChatSetting::Off => {
            map.insert("read_receipts".into(), json!(false));
        }
        ChatSetting::Inherit => {
            map.remove("read_receipts");
        }
    }
}

/// Does this person share receipts in this chat right now? The per-chat override first, then
/// the account default, then `DEFAULT`. Every enforcement point asks this one question.
pub fn enabled_for(state: &AppState, user_id: &str, chat_id: &str) -> bool {
    let member = format!("user:{user_id}");
    let per_chat = state
        .db
        .membership(chat_id, &member)
        .ok()
        .flatten()
        .map_or(ChatSetting::Inherit, |m| chat_override(&m.settings));
    match per_chat {
        ChatSetting::On => true,
        ChatSetting::Off => false,
        ChatSetting::Inherit => state
            .db
            .user_read_receipts_default(user_id)
            .unwrap_or(DEFAULT),
    }
}

impl Db {
    /// The account default; `DEFAULT` when the person never chose.
    pub fn user_read_receipts_default(&self, user_id: &str) -> Result<bool> {
        self.with(|c| {
            c.query_row(
                "SELECT enabled FROM user_read_receipts_default WHERE user_id = ?1",
                params![user_id],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .map(|o| o.map_or(DEFAULT, |n| n != 0))
        })
    }

    pub fn set_user_read_receipts_default(&self, user_id: &str, enabled: bool) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO user_read_receipts_default(user_id, enabled, at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(user_id) DO UPDATE SET enabled = excluded.enabled, at = excluded.at",
                params![user_id, i64::from(enabled), now_ms()],
            )?;
            Ok(())
        })
    }
}

#[derive(Deserialize)]
pub struct DefaultBody {
    pub read_receipts: bool,
}

fn default_json(state: &AppState, user_id: &str) -> Response {
    Json(json!({
        "read_receipts": state.db.user_read_receipts_default(user_id).unwrap_or(DEFAULT),
    }))
    .into_response()
}

/// `GET /me/read-receipts` → `{read_receipts}`: the account-wide default.
pub async fn me_read_receipts(State(state): State<AppState>, principal: Principal) -> Response {
    let Principal::Device { user_id, .. } = &principal else {
        return api_error(StatusCode::FORBIDDEN, "forbidden", "devices only");
    };
    default_json(&state, user_id)
}

/// `PUT /me/read-receipts {read_receipts}` — set the account-wide default. It covers chats that
/// do not exist yet, which is what a per-chat-only setting could never do.
pub async fn put_me_read_receipts(
    State(state): State<AppState>,
    principal: Principal,
    Json(b): Json<DefaultBody>,
) -> Response {
    let Principal::Device { user_id, .. } = &principal else {
        return api_error(StatusCode::FORBIDDEN, "forbidden", "devices only");
    };
    if let Err(e) = state
        .db
        .set_user_read_receipts_default(user_id, b.read_receipts)
    {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    default_json(&state, user_id)
}

#[cfg(test)]
mod tests {
    //! The one question every enforcement point asks: does this person share receipts in this
    //! chat? Override first, then the account default, then on.

    use super::*;
    use crate::people::tests_support::{dm, person};
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    fn parse(v: Value) -> Option<ChatSetting> {
        serde_json::from_value::<ChatSettingBody>(v)
            .expect("a bool or a string")
            .parse()
    }

    #[test]
    fn the_settings_field_takes_a_bool_or_inherit_and_nothing_else() {
        assert_eq!(parse(json!(true)), Some(ChatSetting::On));
        assert_eq!(parse(json!(false)), Some(ChatSetting::Off));
        assert_eq!(parse(json!("inherit")), Some(ChatSetting::Inherit));
        assert_eq!(parse(json!("INHERIT")), Some(ChatSetting::Inherit));
        assert_eq!(parse(json!("off")), Some(ChatSetting::Off));
        assert_eq!(parse(json!("sometimes")), None, "answered with a 400");
    }

    #[test]
    fn inherit_removes_the_key_rather_than_storing_a_value() {
        let mut settings = json!({ "other": 1 });
        set_chat_override(&mut settings, ChatSetting::Off);
        assert_eq!(settings["read_receipts"], json!(false));
        set_chat_override(&mut settings, ChatSetting::Inherit);
        assert!(
            settings.get("read_receipts").is_none(),
            "an absent key is what makes the account default apply",
        );
        assert_eq!(settings["other"], json!(1), "other settings are untouched");
        // A membership whose settings were never an object still takes an override.
        let mut blank = json!(null);
        set_chat_override(&mut blank, ChatSetting::Off);
        assert_eq!(blank["read_receipts"], json!(false));
    }

    #[test]
    fn the_chat_override_wins_over_the_account_default() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 9001);
        let (bob, _) = person(&state, 9002);
        let chat = dm(&state, &alice, &bob);
        let member = format!("user:{alice}");

        // Nobody chose: receipts are on, as they shipped.
        assert!(enabled_for(&state, &alice, &chat));

        state
            .db
            .set_user_read_receipts_default(&alice, false)
            .unwrap();
        assert!(!enabled_for(&state, &alice, &chat), "the account default");
        assert!(enabled_for(&state, &bob, &chat), "Bob chose nothing");

        let mut settings = json!({});
        set_chat_override(&mut settings, ChatSetting::On);
        state
            .db
            .set_member_settings(&chat, &member, &settings)
            .unwrap();
        assert!(
            enabled_for(&state, &alice, &chat),
            "this chat is an exception"
        );

        set_chat_override(&mut settings, ChatSetting::Inherit);
        state
            .db
            .set_member_settings(&chat, &member, &settings)
            .unwrap();
        assert!(!enabled_for(&state, &alice, &chat), "back to the default");
    }

    #[test]
    fn a_chat_the_person_is_not_in_falls_back_to_their_account_default() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 9003);
        assert!(enabled_for(&state, &alice, "01M5XATTTX957RTR99XNE0W9BD"));
        state
            .db
            .set_user_read_receipts_default(&alice, false)
            .unwrap();
        assert!(!enabled_for(&state, &alice, "01M5XATTTX957RTR99XNE0W9BD"));
    }
}
