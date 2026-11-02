//! What a card says has to still be what the box said.
//!
//! A card has two halves that live in different places. What approving it *runs* comes from the
//! box's own records — `decide_approval` loads the row from SQLite and rebuilds the request from
//! the pending entry or that row, never from the document. What the owner *reads* comes from the
//! document: both apps render the card's title, summary and options out of the `approval` part
//! (`arbos-mac/src/ui/approve.rs`, `apps/ios/.../Model.swift`).
//!
//! Every member of a chat can write that document — `ChatDoc::set_part_fields` is public in the
//! crate the apps link and the room socket validates no content — so the two halves can be made
//! to disagree. A member rewrites the card to describe something harmless, the owner approves
//! what they read, and the box runs what it stored. Authorising the *decision*
//! (`approval_authority.rs`) does not help: the answer is then correctly attributed to the owner
//! and is still an answer to a question somebody else wrote.
//!
//! So the box fingerprints the fields it authored onto the card and checks them again before it
//! acts. A card whose text changed after it was raised is refused, loudly, and stays pending.
//!
//! What this buys, exactly: it proves the document still says what the box said. It does not
//! prove the owner's screen said it — a client could have rendered an earlier state. For an
//! attacker who wants to deceive, the tampered text has to be live when the owner looks at it,
//! which is when this check sees it too; but the property is divergence detection, not consent.
//! Proving consent needs the client to send a fingerprint of what it displayed along with the
//! decision, which belongs on the relay-authorised record and rides the next client change for
//! the decision route (deferred by design, not forgotten).

use std::sync::Arc;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use arbos_core::doc::ChatDoc;

use crate::agent::Agent;
use crate::approval::canonical;
use crate::store::ApprovalRow;

/// The fields the box authors onto the card part when it raises it (`add_approval_part`).
///
/// Deliberately not `state`, `decidedAt`, `decidedBy` or `grant`: the box writes those onto the
/// same part *after* the decision (`set_approval_state`, `set_approval_grant`), so covering them
/// would make every card fail its own check.
const BOUND_FIELDS: &[&str] = &["title", "summary", "risk", "payload", "expiresAt"];

/// The error code on the card's own error part and in the audit line.
const CHANGED_CODE: &str = "approval_card_changed";

/// What the owner sees in the chat when a changed card is refused.
const CHANGED_LINE: &str = "This card changed after it was raised, so the decision was not \
     applied. The request shown here no longer matches what the agent asked for. Ask the agent \
     again if you still want it.";

/// The card as the document renders it, over the fields the box authored.
///
/// Taken from the document on both sides — once just after writing the card, once before acting
/// on a decision — so the hash covers what a reader of the chat can see, and the Loro value
/// conversion (`json_to_lv` out, `lv_to_json` back) applies identically to both. `canonical`
/// sorts map keys, which is the one thing that otherwise differs between two reads of the same
/// part. A field that is missing hashes as `null`, so deleting one is a change like any other.
pub(crate) fn fingerprint(part: &Value) -> String {
    use sha2::Digest;
    let text = BOUND_FIELDS
        .iter()
        .map(|k| format!("{k}={}", canonical(part.get(*k).unwrap_or(&Value::Null))))
        .collect::<Vec<_>>()
        .join("\n");
    hex::encode(sha2::Sha256::digest(text.as_bytes()))[..32].to_owned()
}

/// The `approval` part for this card among a message's parts.
pub(crate) fn part_of<'a>(parts: &'a [Value], approval_id: &str) -> Option<&'a Value> {
    parts.iter().find(|p| {
        p.get("kind").and_then(Value::as_str) == Some("approval")
            && p.get("approvalId").and_then(Value::as_str) == Some(approval_id)
    })
}

impl Agent {
    /// Refuse a decision on a card whose text no longer matches what the box wrote.
    ///
    /// Says so in the chat as well as in the log: a card that simply stopped working reads as a
    /// bug and gets chased as one. The error part is written once per card — a member who can
    /// rewrite the card can retry the decision as often as they like.
    async fn refuse_changed(
        self: &Arc<Self>,
        row: &ApprovalRow,
        found: Option<&str>,
    ) -> anyhow::Error {
        tracing::warn!(
            approval_id = row.id,
            chat_id = row.chat_id,
            expected = row.fingerprint.as_deref().unwrap_or(""),
            found = found.unwrap_or("<no card>"),
            "approval card changed after it was raised; refusing the decision"
        );
        self.append_audit(
            CHANGED_CODE,
            json!({
                "approval_id": row.id, "chat_id": row.chat_id, "message_id": row.message_id,
                "title": row.title, "expected": row.fingerprint, "found": found,
            }),
        );
        if let Some(host) = self.sync.host(&row.chat_id).await {
            let h = host.lock().await;
            let already = h
                .doc
                .message(&row.message_id)
                .map(|m| {
                    m.parts.iter().any(|p| {
                        p.get("kind").and_then(Value::as_str) == Some("error")
                            && p.get("code").and_then(Value::as_str) == Some(CHANGED_CODE)
                    })
                })
                .unwrap_or(false);
            if !already {
                h.doc
                    .add_error_part(&row.message_id, CHANGED_CODE, CHANGED_LINE);
            }
        }
        anyhow!(
            "approval {} changed after it was raised: the card in the chat no longer matches \
             what the agent wrote, so the decision was not applied",
            row.id
        )
    }

    /// The room snapshot this box last persisted for a chat, whichever project it belongs to.
    ///
    /// Both room names are tried rather than asking `people.is_people_chat`, because the path
    /// that gets here — `people_agent_removed` — drops the roster entry before it closes the
    /// room, so by now the chat answers "not a people chat" either way.
    fn persisted_room(&self, chat_id: &str) -> Option<Vec<u8>> {
        [self.cfg.project_id.as_str(), crate::people::PEOPLE_PROJECT]
            .into_iter()
            .find_map(|project| {
                let room = arbos_core::wire::room_name(project, chat_id);
                self.store
                    .room_doc(&room)
                    .ok()
                    .flatten()
                    .and_then(|(_, snap, _)| snap)
                    .filter(|s| !s.is_empty())
            })
    }

    /// Does the card still say what the box wrote? Called before a decision is applied.
    ///
    /// A card raised before the box recorded fingerprints has none to compare, and passes: that
    /// population is closed and draining, and the alternative is cards nobody can answer after
    /// an upgrade.
    ///
    /// A chat whose room is closed is still compared, against the snapshot the box persisted —
    /// a member who can have the agent removed from a people chat (`people_agent_removed`) could
    /// otherwise retire the comparison by closing the room, and the snapshot holds the changed
    /// card just the same. Only a chat with no snapshot at all passes. The refusal is quieter
    /// there: with no room there is no document to write the error part into, so it is the log
    /// and the audit line alone.
    pub(crate) async fn check_card_unchanged(self: &Arc<Self>, row: &ApprovalRow) -> Result<()> {
        let Some(expected) = row.fingerprint.as_deref() else {
            tracing::debug!(
                approval_id = row.id,
                "card has no recorded fingerprint; raised before the box bound its text"
            );
            return Ok(());
        };
        // `None` from either source is a card whose part is not in the document the box has,
        // which is a change like any other: a decision about something it cannot account for.
        let found = match self.sync.host(&row.chat_id).await {
            Some(host) => {
                let h = host.lock().await;
                h.doc
                    .message(&row.message_id)
                    .as_ref()
                    .and_then(|m| part_of(&m.parts, &row.id))
                    .map(fingerprint)
            }
            None => {
                let Some(snapshot) = self.persisted_room(&row.chat_id) else {
                    tracing::debug!(
                        approval_id = row.id,
                        chat_id = row.chat_id,
                        "no open room and no snapshot for this chat; nothing to compare against"
                    );
                    return Ok(());
                };
                ChatDoc::from_snapshot(None, false, &snapshot)
                    .ok()
                    .and_then(|doc| doc.message(&row.message_id))
                    .as_ref()
                    .and_then(|m| part_of(&m.parts, &row.id))
                    .map(fingerprint)
            }
        };
        if found.as_deref() == Some(expected) {
            return Ok(());
        }
        Err(self.refuse_changed(row, found.as_deref()).await)
    }
}

#[cfg(test)]
mod tests {
    //! The attack is one `set_part_fields` call by any member of the chat. Each test raises a
    //! real card through `begin_approval` and decides it through `decide_approval`, which is
    //! where every path — a `commands` row, a relay push, the inline waiter — ends up.

    use super::*;
    use crate::approval::WaitMode;
    use crate::status::tests::test_agent;
    use arbos_core::approval::{ApprovalOutcome, ApprovalRequest, Risk};
    use arbos_core::ids::ulid;

    /// A destructive card like the one `gate_tool` raises for `rm -rf`.
    fn card() -> ApprovalRequest {
        ApprovalRequest {
            title: "Run a destructive command (file-delete)".into(),
            summary: "rm -rf /work/build".into(),
            risk: Risk::Destructive,
            payload: json!({
                "kind": "tool_call",
                "tool": "bash",
                "class": "file-delete",
                "path_class": "workspace",
                "why": "deletes files outside /work/tmp",
                "input": { "command": "rm -rf /work/build" },
            }),
        }
    }

    /// The chat the card lands in, and the id of the message holding it.
    async fn raise(agent: &Arc<crate::agent::Agent>, req: ApprovalRequest) -> (String, String) {
        let chat_id = ulid();
        agent.store.upsert_chat(&chat_id, "Main", true, 0).unwrap();
        agent.sync.open_room(&chat_id).await.expect("doc");
        let id = agent
            .begin_approval(&chat_id, req, WaitMode::Deferred, None)
            .await
            .expect("card");
        (chat_id, id)
    }

    /// What a member of the chat does: write over the card's own fields.
    async fn tamper(agent: &Arc<crate::agent::Agent>, chat_id: &str, fields: &[(&str, Value)]) {
        let host = agent.sync.host(chat_id).await.expect("doc");
        let h = host.lock().await;
        let msg = h.doc.messages().pop().expect("message");
        let part = msg
            .parts
            .iter()
            .find(|p| p.get("kind").and_then(Value::as_str) == Some("approval"))
            .expect("card part");
        let part_id = part["id"].as_str().expect("part id").to_owned();
        assert!(h.doc.set_part_fields(&msg.id, &part_id, fields));
    }

    async fn error_codes(agent: &Arc<crate::agent::Agent>, chat_id: &str) -> Vec<String> {
        let host = agent.sync.host(chat_id).await.expect("doc");
        let h = host.lock().await;
        h.doc
            .messages()
            .iter()
            .flat_map(|m| m.parts.clone())
            .filter(|p| p.get("kind").and_then(Value::as_str) == Some("error"))
            .filter_map(|p| p.get("code").and_then(Value::as_str).map(str::to_owned))
            .collect()
    }

    #[tokio::test]
    async fn the_card_the_box_wrote_decides_normally() {
        let (agent, _root) = test_agent().await;
        let (chat_id, id) = raise(&agent, card()).await;

        agent
            .decide_approval(&id, ApprovalOutcome::Approved, "dev:owner-phone")
            .await
            .expect("the untouched card decides");

        let row = agent.store.approval(&id).unwrap().unwrap();
        assert_eq!(row.state, "approved");
        assert!(row.fingerprint.is_some(), "the card was fingerprinted");
        assert_eq!(row.summary, "rm -rf /work/build", "the summary is kept");
        assert!(error_codes(&agent, &chat_id).await.is_empty());
    }

    /// The defect: the owner reads one command and the box runs another.
    #[tokio::test]
    async fn a_card_whose_text_changed_cannot_be_decided() {
        let (agent, _root) = test_agent().await;
        let (chat_id, id) = raise(&agent, card()).await;

        // A guest rewrites the card to describe something harmless. The stored payload — what
        // approving it actually runs — is untouched, which is the whole point of the attack.
        tamper(
            &agent,
            &chat_id,
            &[
                ("summary", json!("rm -rf /work/tmp/cache")),
                ("title", json!("Clear the scratch folder?")),
            ],
        )
        .await;

        let err = agent
            .decide_approval(&id, ApprovalOutcome::Approved, "dev:owner-phone")
            .await
            .expect_err("a changed card is refused");
        let msg = err.to_string();
        assert!(
            msg.contains("changed after it was raised"),
            "the refusal says what happened: {msg}"
        );

        let row = agent.store.approval(&id).unwrap().unwrap();
        assert_eq!(
            row.state, "pending",
            "the card is still the owner's to answer"
        );
        assert_eq!(
            row.payload["input"]["command"], "rm -rf /work/build",
            "and the command never changed"
        );
        assert_eq!(error_codes(&agent, &chat_id).await, vec![CHANGED_CODE]);
    }

    /// Rewriting the payload is the same attack with the dangerous half hidden.
    #[tokio::test]
    async fn a_rewritten_payload_is_a_changed_card() {
        let (agent, _root) = test_agent().await;
        let (chat_id, id) = raise(&agent, card()).await;

        tamper(
            &agent,
            &chat_id,
            &[(
                "payload",
                json!({ "kind": "tool_call", "tool": "bash",
                "input": { "command": "echo hello" } }),
            )],
        )
        .await;

        assert!(agent
            .decide_approval(&id, ApprovalOutcome::Approved, "dev:owner-phone")
            .await
            .is_err());
        assert_eq!(agent.store.approval(&id).unwrap().unwrap().state, "pending");
        assert_eq!(error_codes(&agent, &chat_id).await, vec![CHANGED_CODE]);
    }

    /// The standing grant is recorded before the decision is applied, so a changed card must be
    /// refused before that and not only inside `decide_approval`.
    #[tokio::test]
    async fn a_changed_card_mints_no_standing_grant() {
        let (agent, _root) = test_agent().await;
        let (chat_id, id) = raise(&agent, card()).await;
        tamper(
            &agent,
            &chat_id,
            &[("summary", json!("rm -rf /work/tmp/cache"))],
        )
        .await;

        let err = agent
            .decide_approval_from_command(
                &id,
                ApprovalOutcome::Approved,
                "dev:owner-phone",
                &json!({ "approvalId": id, "grant": "anywhere" }),
            )
            .await
            .expect_err("a changed card is refused");
        assert!(err.to_string().contains("changed after it was raised"));
        assert!(
            agent.approvals.shell_grants.lock().await.is_empty(),
            "no standing grant from a card nobody can trust"
        );
        assert_eq!(agent.store.approval(&id).unwrap().unwrap().state, "pending");
    }

    /// The fields the box writes when it answers a card are not part of the binding, or every
    /// card would fail its own check the moment it was decided.
    #[tokio::test]
    async fn the_decision_fields_are_not_bound() {
        let (agent, _root) = test_agent().await;
        let (chat_id, id) = raise(&agent, card()).await;

        tamper(
            &agent,
            &chat_id,
            &[
                ("state", json!("approved")),
                ("decidedBy", json!("dev:not-the-owner")),
                ("decidedAt", json!(1)),
                ("grant", json!("anywhere")),
            ],
        )
        .await;

        agent
            .decide_approval(&id, ApprovalOutcome::Approved, "dev:owner-phone")
            .await
            .expect("the card still decides");
        assert!(error_codes(&agent, &chat_id).await.is_empty());
    }

    /// A card raised by a daemon that did not record fingerprints still decides: an upgrade must
    /// not leave the owner holding cards nobody can answer.
    #[tokio::test]
    async fn a_card_raised_before_the_binding_still_decides() {
        let (agent, _root) = test_agent().await;
        let (chat_id, id) = raise(&agent, card()).await;
        agent
            .store
            .with(|c| {
                c.execute(
                    "UPDATE approvals SET fingerprint = NULL WHERE id = ?1",
                    rusqlite::params![&id],
                )
                .map(|_| ())
            })
            .unwrap();

        tamper(&agent, &chat_id, &[("summary", json!("anything at all"))]).await;

        agent
            .decide_approval(&id, ApprovalOutcome::Approved, "dev:owner-phone")
            .await
            .expect("no fingerprint, nothing to check");
    }

    /// A member who can have the agent removed from a people chat closes the room, and with it
    /// the live document. The comparison does not go away with it: the snapshot the box persisted
    /// holds the same changed card.
    #[tokio::test]
    async fn a_changed_card_is_refused_after_the_room_closes() {
        let (agent, _root) = test_agent().await;
        let (chat_id, id) = raise(&agent, card()).await;
        tamper(&agent, &chat_id, &[("summary", json!("rm -rf /work/tmp"))]).await;

        let host = agent.sync.host(&chat_id).await.expect("doc");
        agent.sync.persist(&host).await.expect("snapshot");
        agent.sync.close_room(&chat_id).await;
        assert!(agent.sync.host(&chat_id).await.is_none(), "room is closed");

        let err = agent
            .decide_approval(&id, ApprovalOutcome::Approved, "dev:owner-phone")
            .await
            .expect_err("a changed card is refused from the snapshot too");
        assert!(err.to_string().contains("changed after it was raised"));
        assert_eq!(agent.store.approval(&id).unwrap().unwrap().state, "pending");
    }

    /// An untouched card still decides once its room is closed — the snapshot agrees with what
    /// the box wrote, and a closed room is not by itself a reason to refuse an owner.
    #[tokio::test]
    async fn an_unchanged_card_decides_after_the_room_closes() {
        let (agent, _root) = test_agent().await;
        let (chat_id, id) = raise(&agent, card()).await;
        let host = agent.sync.host(&chat_id).await.expect("doc");
        agent.sync.persist(&host).await.expect("snapshot");
        agent.sync.close_room(&chat_id).await;

        agent
            .decide_approval(&id, ApprovalOutcome::Approved, "dev:owner-phone")
            .await
            .expect("the untouched card decides");
        assert_eq!(
            agent.store.approval(&id).unwrap().unwrap().state,
            "approved"
        );
    }

    /// The residual, stated as a test rather than as prose: a chat with no snapshot at all has
    /// nothing to compare, and passes. Reaching it means the room closed before the box ever
    /// persisted the card, which is not something a member can time.
    #[tokio::test]
    async fn a_chat_with_no_snapshot_has_nothing_to_compare() {
        let (agent, _root) = test_agent().await;
        let (chat_id, id) = raise(&agent, card()).await;
        tamper(&agent, &chat_id, &[("summary", json!("rm -rf /work/tmp"))]).await;
        agent.sync.close_room(&chat_id).await;
        let room = arbos_core::wire::room_name(&agent.cfg.project_id, &chat_id);
        assert!(
            agent.store.room_doc(&room).unwrap().is_none(),
            "nothing was ever persisted for this chat"
        );

        agent
            .decide_approval(&id, ApprovalOutcome::Approved, "dev:owner-phone")
            .await
            .expect("nothing to compare against");
    }

    /// Expiry is the box's own clock, not an answer, and it has to be able to close a card the
    /// owner can no longer answer.
    #[tokio::test]
    async fn a_changed_card_can_still_expire() {
        let (agent, _root) = test_agent().await;
        let (chat_id, id) = raise(&agent, card()).await;
        tamper(&agent, &chat_id, &[("title", json!("something else"))]).await;

        agent
            .decide_approval(&id, ApprovalOutcome::Expired, "arbos:timeout")
            .await
            .expect("a changed card still expires");
        assert_eq!(agent.store.approval(&id).unwrap().unwrap().state, "expired");
    }

    /// The canonicalisation, which is the part that would otherwise make this flaky: the
    /// fingerprint has to survive the document's own round trip, for a payload with nested
    /// objects, floats and integers too large for the `i64` the CRDT stores.
    #[tokio::test]
    async fn the_fingerprint_survives_the_document_round_trip() {
        let (agent, _root) = test_agent().await;
        let req = ApprovalRequest {
            title: "Spend money".into(),
            summary: "transfer".into(),
            risk: Risk::Money,
            payload: json!({
                "kind": "tool_call",
                "amounts": { "cents": 1_234_567_890_123_i64, "rate": 0.015625, "zero": 0.0 },
                "big": 9_007_199_254_740_993_i64,
                "nested": { "b": [1, 2.5, { "deep": true, "nil": null }], "a": "x" },
                "ints": [0, -1, 9_223_372_036_854_775_807_i64],
            }),
        };
        let (chat_id, id) = raise(&agent, req).await;
        let row = agent.store.approval(&id).unwrap().unwrap();
        let stored = row.fingerprint.clone().expect("fingerprinted");

        let host = agent.sync.host(&chat_id).await.expect("doc");
        let (again, bytes) = {
            let h = host.lock().await;
            let msg = h.doc.message(&row.message_id).expect("message");
            // Read twice: map order is not promised between two reads of the same part.
            (
                fingerprint(part_of(&msg.parts, &id).expect("card part")),
                h.doc.export_updates_since(&Default::default()),
            )
        };
        assert_eq!(stored, again, "stable across reads");

        // And across the CRDT itself: import the room into a fresh document, the way every
        // other peer in the chat receives it.
        let mut peer = ChatDoc::new(Some(99), false);
        peer.import(&bytes).expect("import");
        let msg = peer.message(&row.message_id).expect("message");
        let part = part_of(&msg.parts, &id).expect("card part");
        assert_eq!(stored, fingerprint(part), "stable across the round trip");

        // The same payload with its keys written in another order is the same card.
        let reordered = json!({ "b": 1, "a": { "y": 2, "x": [3, 4] } });
        let flipped = json!({ "a": { "x": [3, 4], "y": 2 }, "b": 1 });
        assert_eq!(
            fingerprint(&json!({ "payload": reordered })),
            fingerprint(&json!({ "payload": flipped })),
            "key order is not the card"
        );
    }
}
