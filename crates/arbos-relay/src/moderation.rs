//! Deleting a message for everyone (Jacob, 2026-10-23): the relay decides, and the relay
//! edits the doc. Until now a bubble's Delete edited the shared Loro doc straight from the
//! client and the relay stored the bytes without reading them — so any member could delete
//! anyone's message. `DELETE /room/{project}/{chat}/messages/{message_id}` is the one path
//! now: the relay takes the message's author from its own record of the announce, applies the
//! rule below, and writes the deletion itself (`notices::write_doc`), or answers 403.
//!
//! The author comes from `message_authors` (`Db::message_author`), written by
//! `notify::announce_with` from the principal that announced the message. It does **not** come
//! from the doc. Reading it from the doc was the whole defect: the rule below decides "is this
//! yours", the doc's `sender` field is writable by every member of the chat
//! (`ChatDoc::set_message_fields` is public in the crate the apps link, and the socket's `Push`
//! arm validates no content), so a member could put their own id on someone else's message and
//! then ask this route to delete it. The relay's record is the only account of the author that
//! the author's peers cannot edit.
//!
//! A message with no record — a system line, an app too old to announce, anything written
//! before the record existed — is nobody's but a moderator's, because there is no sound way to
//! tell whose it is.
//!
//! The rule (`may_delete`): you delete your own messages. In a people group the owner — and
//! an admin, once that role exists — deletes anyone's (moderation). A message an agent wrote
//! is the agent owner's to delete, besides the group owner/admin. In a project chat the
//! project's owner deletes anything; a guest of a shared project deletes their own only.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use arbos_core::wire::PROJECT_DOC_CHAT_ID;

use crate::auth::Principal;
use crate::matrix::backfill::load_doc;
use crate::notices::write_doc;
use crate::people::{self, PEOPLE_PROJECT};
use crate::signin::api_error;
use crate::state::AppState;

/// Who is asking, as far as deleting goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Deleter<'a> {
    /// My user id.
    pub me: &'a str,
    /// People chat: my membership role (`owner` | `admin` | `member`). Project chat: `None`.
    pub role: Option<&'a str>,
    /// Project chat: I own the project (a guest is `false`). People chat: `false`.
    pub project_owner: bool,
}

impl Deleter<'_> {
    /// Owners and admins moderate: they may delete anyone's message in their group.
    pub fn moderates(&self) -> bool {
        self.project_owner || matches!(self.role, Some("owner") | Some("admin"))
    }
}

/// May `who` delete a message whose normalised sender is `sender` (`user:<id>`,
/// `agent:<project>`, or `system`)? `agent_owner` is the user who owns that agent's project,
/// when the sender is an agent.
pub fn may_delete(who: &Deleter<'_>, sender: &str, agent_owner: Option<&str>) -> bool {
    if who.moderates() {
        return true;
    }
    if let Some(uid) = sender.strip_prefix("user:") {
        return uid == who.me;
    }
    if sender.starts_with("agent:") {
        return agent_owner == Some(who.me);
    }
    // System lines (notices, call lines) are nobody's to delete but a moderator's.
    false
}

/// What the rule needs to know before a message is edited for everyone: the project the chat
/// belongs to (`None` for a people chat), the asker and the message's normalised sender.
struct Authorised {
    project_id: Option<String>,
    me: String,
    sender: String,
}

/// The shared half of the message-wide edits: membership, the message's sender, the rule.
/// `Ok(None)` when the message is already gone (editing twice is fine).
async fn authorise(
    state: &AppState,
    principal: &Principal,
    project: &str,
    chat_id: &str,
    message_id: &str,
    verb: &str,
) -> Result<Option<Authorised>, Box<Response>> {
    let Principal::Device { user_id: me, .. } = principal else {
        return Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "a signed-in device is required",
        )));
    };
    if message_id.trim().is_empty() || chat_id == PROJECT_DOC_CHAT_ID {
        return Err(Box::new(api_error(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "not a message",
        )));
    }
    let is_people = project == PEOPLE_PROJECT;
    let (role, project_owner, project_id) = if is_people {
        let cm = people::load_chat(state, chat_id)?;
        let Some(mine) = cm.members.iter().find(|m| m.user_id() == Some(me.as_str())) else {
            return Err(Box::new(api_error(
                StatusCode::FORBIDDEN,
                "not_member",
                "you are not in this chat",
            )));
        };
        (Some(mine.role.clone()), false, None)
    } else {
        if !state.allows(principal, project) {
            return Err(Box::new(api_error(
                StatusCode::FORBIDDEN,
                "not_member",
                "you are not in this project",
            )));
        }
        (
            None,
            state.owns(principal, project),
            Some(project.to_owned()),
        )
    };
    let doc = match load_doc(state, chat_id, project_id.as_deref()) {
        Ok(Some(d)) => d,
        Ok(None) => {
            return Err(Box::new(api_error(
                StatusCode::NOT_FOUND,
                "not_found",
                "no such chat",
            )))
        }
        Err(e) => {
            return Err(Box::new(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "db",
                e.to_string(),
            )))
        }
    };
    if doc.message(message_id).is_none() {
        return Ok(None);
    }
    let who = Deleter {
        me,
        role: role.as_deref(),
        project_owner,
    };
    // Whose message this is comes from the relay's own record of the announce, never from the
    // doc: `ChatDoc::set_message_fields` is public in the crate the apps link and the push path
    // validates no content, so a member can write any `sender` on anyone's row — including, in
    // the version this replaces, their own id onto someone else's message just before asking to
    // delete it.
    let attested = state.db.message_author(chat_id, message_id).map_err(|e| {
        Box::new(api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db",
            e.to_string(),
        ))
    })?;
    let Some(sender) = attested else {
        // No record of the announce: a system line, an app too old to announce, or a message
        // from before the record existed. There is no sound way to tell whose it is, so only
        // moderation can act -- the same answer `may_delete` already gives for a system line.
        if !who.moderates() {
            return Err(Box::new(api_error(
                StatusCode::FORBIDDEN,
                "not_yours",
                format!("only this chat's owner can {verb} a message the relay has no author for"),
            )));
        }
        return Ok(Some(Authorised {
            project_id,
            me: me.clone(),
            sender: "unknown".to_owned(),
        }));
    };
    let agent_owner = sender
        .strip_prefix("agent:")
        .and_then(|pid| state.user_of(pid));
    if !may_delete(&who, &sender, agent_owner.as_deref()) {
        return Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "not_yours",
            format!("you can only {verb} your own messages"),
        )));
    }
    Ok(Some(Authorised {
        project_id,
        me: me.clone(),
        sender,
    }))
}

/// `DELETE /room/{project}/{chat}/messages/{message_id}` — delete a message for everyone.
/// 204 when gone (or already gone), 403 `not_yours` when the rule says no, 404 when the chat
/// or the message is unknown. The deletion lands as a relay-signed row on the room.
pub async fn delete_message(
    State(state): State<AppState>,
    principal: Principal,
    Path((project, chat_id, message_id)): Path<(String, String, String)>,
) -> Response {
    let a = match authorise(
        &state,
        &principal,
        &project,
        &chat_id,
        &message_id,
        "delete",
    )
    .await
    {
        Ok(Some(a)) => a,
        Ok(None) => return StatusCode::NO_CONTENT.into_response(),
        Err(r) => return *r,
    };
    match write_doc(&state, a.project_id.as_deref(), &chat_id, |doc| {
        doc.delete_message(&message_id)?;
        Ok(())
    })
    .await
    {
        Ok(seq) => {
            tracing::info!(chat = %chat_id, message = %message_id, me = %a.me, sender = %a.sender, seq, "message deleted");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

/// `DELETE /room/{project}/{chat}/messages/{message_id}/preview` — the sender's **Remove
/// preview** (#2064): the same rule as deleting the message (yours, or you moderate), then the
/// relay writes `preview: "off"` on the row and drops its `link_preview` parts, for everyone.
/// 204 when done (or nothing to remove), 403 `not_yours` otherwise.
pub async fn remove_preview(
    State(state): State<AppState>,
    principal: Principal,
    Path((project, chat_id, message_id)): Path<(String, String, String)>,
) -> Response {
    let a = match authorise(&state, &principal, &project, &chat_id, &message_id, "edit").await {
        Ok(Some(a)) => a,
        Ok(None) => return StatusCode::NO_CONTENT.into_response(),
        Err(r) => return *r,
    };
    match write_doc(&state, a.project_id.as_deref(), &chat_id, |doc| {
        doc.set_message_fields(
            &message_id,
            &[(
                crate::unfurl::PREVIEW_FIELD,
                serde_json::Value::from(crate::unfurl::PREVIEW_OFF),
            )],
        );
        doc.remove_parts_of_kind(&message_id, crate::unfurl::PART_KIND);
        Ok(())
    })
    .await
    {
        Ok(seq) => {
            tracing::info!(chat = %chat_id, message = %message_id, me = %a.me, seq, "link preview removed");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: &str = "u_me";

    fn member() -> Deleter<'static> {
        Deleter {
            me: ME,
            role: Some("member"),
            project_owner: false,
        }
    }

    #[test]
    fn a_member_deletes_only_their_own_messages() {
        let me = member();
        assert!(may_delete(&me, "user:u_me", None));
        assert!(!may_delete(&me, "user:u_other", None));
        assert!(!may_delete(&me, "agent:p_theirs", Some("u_other")));
        assert!(!may_delete(&me, "system", None));
    }

    #[test]
    fn owners_and_admins_moderate_everything() {
        for role in ["owner", "admin"] {
            let mod_ = Deleter {
                me: ME,
                role: Some(role),
                project_owner: false,
            };
            assert!(may_delete(&mod_, "user:u_other", None), "{role}");
            assert!(
                may_delete(&mod_, "agent:p_theirs", Some("u_other")),
                "{role}"
            );
            assert!(may_delete(&mod_, "system", None), "{role}");
        }
    }

    #[test]
    fn an_agents_message_belongs_to_the_agents_owner() {
        let me = member();
        assert!(may_delete(&me, "agent:p_mine", Some(ME)));
        assert!(!may_delete(&me, "agent:p_theirs", Some("u_other")));
    }

    #[test]
    fn project_owner_deletes_anything_a_guest_only_their_own() {
        let owner = Deleter {
            me: ME,
            role: None,
            project_owner: true,
        };
        assert!(may_delete(&owner, "user:u_guest", None));
        assert!(may_delete(&owner, "agent:p_mine", Some(ME)));
        let guest = Deleter {
            me: "u_guest",
            role: None,
            project_owner: false,
        };
        assert!(may_delete(&guest, "user:u_guest", None));
        assert!(!may_delete(&guest, "user:u_me", None));
        assert!(!may_delete(&guest, "agent:p_mine", Some(ME)));
    }
}

#[cfg(test)]
mod handler_tests {
    //! The endpoint end to end: a member's Delete on someone else's message is 403 and the
    //! message stays; on their own it is 204 and the deletion lands as a relay-signed row;
    //! the group owner deletes anyone's.

    use arbos_core::doc::{ChatDoc, UserMessageOpts};
    use arbos_core::ids::ulid;
    use arbos_core::wire;
    use axum::extract::{Path, State};
    use axum::http::StatusCode;

    use super::*;
    use crate::people::tests_support::{dm, person};
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    /// A message as it exists after a real send: the row in the doc, **and** the relay's own
    /// record of who announced it. In production `notify::announce_with` writes the second from
    /// the authenticated principal; here it is written directly so the test does not need the
    /// whole push-and-notify path.
    fn seed(state: &AppState, chat: &str, sender: &str, id: &str) {
        seed_row(state, chat, sender, id);
        let user = sender.strip_prefix("user:");
        state
            .db
            .insert_chat_event(chat, None, id, user, sender)
            .unwrap();
    }

    /// A message in the doc that the relay never saw announced: an app too old to announce, a
    /// system line, or anything written before `message_authors` existed.
    fn seed_row(state: &AppState, chat: &str, sender: &str, id: &str) {
        let doc = load_doc(state, chat, None)
            .unwrap()
            .unwrap_or_else(|| ChatDoc::new(Some(3), false));
        let base = doc.oplog_vv();
        doc.append_user_message(
            id,
            "hello",
            "dev",
            &UserMessageOpts {
                sender: Some(sender.to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
        let bytes = doc.export_updates_since(&base);
        state
            .db
            .append_row(
                &wire::room_name(PEOPLE_PROJECT, chat),
                "dev",
                &ulid(),
                &bytes,
            )
            .unwrap();
    }

    fn ids(state: &AppState, chat: &str) -> Vec<String> {
        load_doc(state, chat, None)
            .unwrap()
            .map(|d| d.messages().into_iter().map(|m| m.id).collect())
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn only_your_own_unless_you_own_the_group() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let chat = dm(&state, &alice, &bob);
        // A DM's two people are both owners; make Bob a plain member (a group).
        state
            .db
            .add_member(
                &chat,
                &format!("user:{bob}"),
                "member",
                &alice,
                &serde_json::json!({}),
            )
            .unwrap();
        seed(&state, &chat, &format!("user:{alice}"), "m_alice");
        seed(&state, &chat, &format!("user:{bob}"), "m_bob");
        seed(&state, &chat, &format!("user:{alice}"), "m_alice2");

        // Bob on Alice's message: refused, nothing changes.
        let r = delete_message(
            State(state.clone()),
            bob_dev.clone(),
            Path((
                PEOPLE_PROJECT.to_owned(),
                chat.clone(),
                "m_alice".to_owned(),
            )),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        assert_eq!(ids(&state, &chat), ["m_alice", "m_bob", "m_alice2"]);

        // Bob on his own: gone, as a relay-signed row.
        let r = delete_message(
            State(state.clone()),
            bob_dev.clone(),
            Path((PEOPLE_PROJECT.to_owned(), chat.clone(), "m_bob".to_owned())),
        )
        .await;
        assert_eq!(r.status(), StatusCode::NO_CONTENT);
        assert_eq!(ids(&state, &chat), ["m_alice", "m_alice2"]);
        let rows = state
            .db
            .rows_after(&wire::room_name(PEOPLE_PROJECT, &chat), 0, None, 10)
            .unwrap();
        assert_eq!(rows.last().unwrap().device, crate::notices::RELAY_DEVICE);

        // Deleting it again is fine.
        let r = delete_message(
            State(state.clone()),
            bob_dev,
            Path((PEOPLE_PROJECT.to_owned(), chat.clone(), "m_bob".to_owned())),
        )
        .await;
        assert_eq!(r.status(), StatusCode::NO_CONTENT);

        // Alice (owner) moderates: she may delete Bob's — there is none left, so her own
        // second message stands in; and an outsider is not a member at all.
        let r = delete_message(
            State(state.clone()),
            alice_dev,
            Path((
                PEOPLE_PROJECT.to_owned(),
                chat.clone(),
                "m_alice2".to_owned(),
            )),
        )
        .await;
        assert_eq!(r.status(), StatusCode::NO_CONTENT);
        let (_carol, carol_dev) = person(&state, 3);
        let r = delete_message(
            State(state.clone()),
            carol_dev,
            Path((
                PEOPLE_PROJECT.to_owned(),
                chat.clone(),
                "m_alice".to_owned(),
            )),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        assert_eq!(ids(&state, &chat), ["m_alice"]);
    }

    /// The defect: `authorise` decided whose message this was from `message.rest["sender"]`, a
    /// doc field every member of the chat can rewrite. Bob relabels Alice's message as his own
    /// and asks the relay to delete it — which is the exact move this module was built to stop,
    /// made through the module itself.
    #[tokio::test]
    async fn a_member_cannot_relabel_someone_elses_message_as_their_own() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 21);
        let (bob, bob_dev) = person(&state, 22);
        let chat = dm(&state, &alice, &bob);
        state
            .db
            .add_member(
                &chat,
                &format!("user:{bob}"),
                "member",
                &alice,
                &serde_json::json!({}),
            )
            .unwrap();
        seed(&state, &chat, &format!("user:{alice}"), "m_alice");

        // Bob rewrites the sender on Alice's row, exactly as a client can: the push path
        // validates no content, so the relay stores and forwards whatever he writes.
        let doc = load_doc(&state, &chat, None).unwrap().unwrap();
        let base = doc.oplog_vv();
        doc.set_message_fields(
            "m_alice",
            &[("sender", serde_json::Value::from(format!("user:{bob}")))],
        );
        let bytes = doc.export_updates_since(&base);
        state
            .db
            .append_row(
                &wire::room_name(PEOPLE_PROJECT, &chat),
                "dev",
                &ulid(),
                &bytes,
            )
            .unwrap();
        assert_eq!(
            load_doc(&state, &chat, None)
                .unwrap()
                .unwrap()
                .message("m_alice")
                .unwrap()
                .rest["sender"],
            serde_json::Value::from(format!("user:{bob}")),
            "the forged field really is in the doc the relay serves"
        );

        let r = delete_message(
            State(state.clone()),
            bob_dev,
            Path((
                PEOPLE_PROJECT.to_owned(),
                chat.clone(),
                "m_alice".to_owned(),
            )),
        )
        .await;
        assert_eq!(
            r.status(),
            StatusCode::FORBIDDEN,
            "the doc's account of the author is not the relay's"
        );
        assert_eq!(ids(&state, &chat), ["m_alice"]);
    }

    /// The cost of the fix, stated as a test rather than left to be discovered: a message the
    /// relay has no author for is a moderator's alone, even for the member who wrote it.
    #[tokio::test]
    async fn a_message_with_no_recorded_author_is_the_owners_alone() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 31);
        let (bob, bob_dev) = person(&state, 32);
        let chat = dm(&state, &alice, &bob);
        state
            .db
            .add_member(
                &chat,
                &format!("user:{bob}"),
                "member",
                &alice,
                &serde_json::json!({}),
            )
            .unwrap();
        seed_row(&state, &chat, &format!("user:{bob}"), "m_unannounced");

        let r = delete_message(
            State(state.clone()),
            bob_dev,
            Path((
                PEOPLE_PROJECT.to_owned(),
                chat.clone(),
                "m_unannounced".to_owned(),
            )),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "even though it is his");
        assert_eq!(ids(&state, &chat), ["m_unannounced"]);

        let r = delete_message(
            State(state.clone()),
            alice_dev,
            Path((
                PEOPLE_PROJECT.to_owned(),
                chat.clone(),
                "m_unannounced".to_owned(),
            )),
        )
        .await;
        assert_eq!(r.status(), StatusCode::NO_CONTENT, "the owner moderates");
        assert!(ids(&state, &chat).is_empty());
    }

    #[tokio::test]
    async fn the_owner_deletes_a_members_message() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 11);
        let (bob, _) = person(&state, 12);
        let chat = dm(&state, &alice, &bob);
        state
            .db
            .add_member(
                &chat,
                &format!("user:{bob}"),
                "member",
                &alice,
                &serde_json::json!({}),
            )
            .unwrap();
        seed(&state, &chat, &format!("user:{bob}"), "m_bob");
        let r = delete_message(
            State(state.clone()),
            alice_dev,
            Path((PEOPLE_PROJECT.to_owned(), chat.clone(), "m_bob".to_owned())),
        )
        .await;
        assert_eq!(r.status(), StatusCode::NO_CONTENT);
        assert!(ids(&state, &chat).is_empty());
    }
}
