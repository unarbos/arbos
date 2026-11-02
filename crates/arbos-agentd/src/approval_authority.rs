//! An approval card's answer is authorised by the relay, not read out of the chat doc.
//!
//! A card is the one gate between the model and real authority: a destructive shell command on
//! the box, an op on the owner's own computer through `arbos-local`, or — with the card's
//! standing option — a grant that answers every later card of its class with no card at all.
//!
//! The answer used to arrive as a `commands` row (`approve` / `reject`) and the box took the
//! row's `issuedBy` for the decider. Every author field in the doc is written by the client that
//! pushes it, and the room socket admits every member of a shared chat, so the decider was
//! whatever the writing client said it was. The relay cannot repair that on the way in either:
//! a socket's `device` is taken from its own HELLO frame (`rooms.rs`), so no name in the doc is
//! attested by anything.
//!
//! So the decision does not come from the doc. The owner's app answers the card over its own
//! authenticated connection (`POST /projects/{id}/approvals/{approval}/decision`), which the
//! relay authorises against the authenticated principal and records. The box reads that record
//! over its own authenticated channel and acts on it alone. A `commands` row is then only a
//! courier: it says a card was answered somewhere, and the box still asks the relay what the
//! answer was. A row nobody authorised decides nothing.
//!
//! The same shape already carries a secret request's decision (`vault.rs`: "the relay row is the
//! authority") — this puts every card's decision on that footing.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use arbos_core::approval::{ApprovalOutcome, PathClass};

use crate::agent::Agent;
use crate::approval::GRANT_FIELD;

/// A decision the relay authorised against an authenticated owner device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorisedDecision {
    pub outcome: ApprovalOutcome,
    /// The standing grant the owner chose, if any. The row's own `grant` is ignored.
    pub grant: Option<PathClass>,
    pub grant_chat_only: bool,
    /// The user the relay authenticated. Carried as `decidedBy` with the `arbos:` prefix the
    /// box uses for decisions no client device issued.
    pub decided_by: String,
}

/// What the relay holds for one card.
struct Authority {
    decision: Option<AuthorisedDecision>,
    /// Somebody other than the owner is a member of this project, and so has room access to its
    /// chats (`state.allows`, D72 step 3). `None` from a relay that does not serve the route at
    /// all, which is one older than authorised decisions.
    project_shared: Option<bool>,
}

/// Why a decision cannot be acted on. Both answers are the same to the card — it stays pending
/// for the owner — and differ only in what the log and the audit entry say.
enum Refusal {
    /// Somebody other than the owner can write into this chat and no owner authorised a
    /// decision, so there is nothing here to tell the owner's answer from anybody else's.
    Unauthorised,
    /// The relay could not be asked. Fail closed: a card is not a thing to guess about.
    Unreachable(String),
}

impl Refusal {
    fn code(&self) -> &'static str {
        match self {
            Refusal::Unauthorised => "not_authorised",
            Refusal::Unreachable(_) => "relay_unreachable",
        }
    }

    fn line(&self) -> String {
        match self {
            Refusal::Unauthorised => "this chat has members besides the owner, and no owner \
                 authorised this decision on the relay; the card stays pending"
                .to_owned(),
            Refusal::Unreachable(e) => format!("the relay could not be asked who answered: {e}"),
        }
    }
}

impl Agent {
    /// Ask the relay what the owner chose for this card.
    ///
    /// The status code is read here rather than through `vault::relay_get` because the three
    /// answers mean different things: a decision, no decision, and a relay that does not serve
    /// the route. `approval_id` is a card id this box's own store holds, so it is safe in the
    /// path; the caller checks that first and never passes a client's string through.
    async fn relay_decision(&self, approval_id: &str) -> Result<Authority, String> {
        let r = self
            .http_client()
            .get(format!(
                "{}/box/approvals/{approval_id}/decision",
                self.cfg.relay_url
            ))
            .bearer_auth(&self.cfg.box_token)
            .header("x-arbos-project", &self.cfg.project_id)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        // A relay older than this route knows nothing of authorised decisions, so there is none
        // to find and none to expect. Treated below as today's behaviour, loudly.
        if r.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Authority {
                decision: None,
                project_shared: None,
            });
        }
        if !r.status().is_success() {
            return Err(format!("HTTP {}", r.status()));
        }
        let v: Value = r.json().await.map_err(|e| e.to_string())?;
        let project_shared = Some(v["project_shared"].as_bool().unwrap_or(true));
        let d = &v["decision"];
        if d.is_null() {
            return Ok(Authority {
                decision: None,
                project_shared,
            });
        }
        let outcome = match d["outcome"].as_str().unwrap_or_default() {
            "approved" => ApprovalOutcome::Approved,
            "rejected" => ApprovalOutcome::Rejected {
                reason: d["reason"].as_str().map(str::to_owned),
            },
            other => return Err(format!("relay sent an unknown outcome `{other}`")),
        };
        let decided_by = d["decided_by"].as_str().unwrap_or_default();
        if decided_by.is_empty() {
            return Err("relay sent a decision with no decider".to_owned());
        }
        Ok(Authority {
            decision: Some(AuthorisedDecision {
                outcome,
                grant: d["grant_path_class"].as_str().and_then(PathClass::parse),
                grant_chat_only: d["grant_chat_only"].as_bool().unwrap_or(false),
                decided_by: format!("arbos:relay:{decided_by}"),
            }),
            project_shared,
        })
    }

    /// Whether this card's answer has to come with the relay's authority behind it: it does
    /// wherever somebody other than the owner can write into the chat the card is in.
    ///
    /// Two sources, because a card lands in either kind of chat. A project chat's writers are
    /// the owner's devices plus the guests the owner invited to the project, which only the
    /// relay knows. A people chat's writers are its roster, which the box holds already
    /// (`people.rs`, filled from the relay over the box's own authenticated channel).
    fn authority_required(&self, chat_id: &str, project_shared: Option<bool>) -> bool {
        if project_shared.unwrap_or(false) {
            return true;
        }
        let Some(info) = self.people.info(chat_id) else {
            return false;
        };
        info.members.iter().any(|m| {
            m["member"]
                .as_str()
                .and_then(|who| who.strip_prefix("user:"))
                .is_some_and(|user| user != info.owner_user)
        })
    }

    /// A client says a card was answered (`approve` / `reject` command). What the answer *was*
    /// comes from the relay's record, so the row's outcome, standing option and `issuedBy` are
    /// all only logged.
    ///
    /// Where nobody but the owner can write into the chat, a row with no authorised decision
    /// behind it still decides, as it always did: the room socket admits the owner's own devices
    /// and this box, and nobody else, so the row is the owner's by the only route it could have
    /// arrived by. That is the one case where the doc says who answered. Everywhere else — a
    /// project the owner shared, a group chat — an unauthorised row decides nothing, and the card
    /// waits for an answer that comes with the relay's authority behind it.
    pub async fn decide_approval_from_client(
        self: &Arc<Self>,
        approval_id: &str,
        row_outcome: &ApprovalOutcome,
        issued_by: &str,
        payload: &Value,
    ) -> Result<()> {
        // The card first: an id the store does not hold is answering nothing, and only an id the
        // store does hold is this box's own to put in a request path.
        let Some(row) = self.store.approval(approval_id).ok().flatten() else {
            return Err(anyhow!("unknown approval {approval_id}"));
        };
        let authority = match self.relay_decision(&row.id).await {
            Ok(a) => a,
            Err(e) => return Err(self.refuse(approval_id, issued_by, Refusal::Unreachable(e))),
        };
        let Some(authorised) = authority.decision else {
            if self.authority_required(&row.chat_id, authority.project_shared) {
                return Err(self.refuse(approval_id, issued_by, Refusal::Unauthorised));
            }
            if authority.project_shared.is_none() {
                tracing::warn!(
                    approval_id,
                    "this relay does not authorise card decisions; the command row is taken at \
                     its word, as it was before"
                );
            }
            // The owner's own chat, answered by one of the owner's own devices.
            return self
                .decide_approval_from_command(approval_id, row_outcome.clone(), issued_by, payload)
                .await;
        };
        if authorised.outcome.state() != row_outcome.state() {
            tracing::warn!(
                approval_id,
                issued_by,
                row = row_outcome.state().as_str(),
                authorised = authorised.outcome.state().as_str(),
                "the command row disagrees with the authorised decision; the relay's stands"
            );
        }
        self.apply_authorised(approval_id, &authorised, payload)
            .await
    }

    /// The relay pushed `approval_decided`: the owner answered, perhaps on a device that never
    /// writes into this chat. The decision applies on its own, with no command row at all.
    pub async fn decide_approval_from_push(self: &Arc<Self>, approval_id: &str) -> Result<()> {
        match self.relay_decision(approval_id).await {
            Ok(Authority {
                decision: Some(d), ..
            }) => {
                self.apply_authorised(approval_id, &d, &json!({ "approvalId": approval_id }))
                    .await
            }
            Ok(_) => Err(anyhow!(
                "approval_decided push for {approval_id}, but the relay holds no decision for it"
            )),
            Err(e) => Err(anyhow!("relay decision for {approval_id}: {e}")),
        }
    }

    /// Hand the authorised decision to the ordinary card machinery, with the standing option
    /// taken from the relay's record rather than from the row: a row cannot widen the owner's
    /// "only in /tmp" into "anywhere".
    async fn apply_authorised(
        self: &Arc<Self>,
        approval_id: &str,
        authorised: &AuthorisedDecision,
        payload: &Value,
    ) -> Result<()> {
        let mut payload = payload.clone();
        if let Some(o) = payload.as_object_mut() {
            match authorised.grant {
                Some(c) => {
                    o.insert(GRANT_FIELD.into(), json!(c.as_str()));
                }
                None => {
                    o.remove(GRANT_FIELD);
                }
            }
            o.insert("grant_chat_only".into(), json!(authorised.grant_chat_only));
        }
        self.decide_approval_from_command(
            approval_id,
            authorised.outcome.clone(),
            &authorised.decided_by,
            &payload,
        )
        .await
    }

    /// Audit and log a decision the box will not act on, and give the caller the line the
    /// command's failure carries.
    fn refuse(self: &Arc<Self>, approval_id: &str, issued_by: &str, why: Refusal) -> anyhow::Error {
        let line = why.line();
        tracing::warn!(approval_id, issued_by, code = why.code(), "{line}");
        self.append_audit(
            "approval_decision_refused",
            json!({
                "approval_id": approval_id, "issued_by": issued_by, "code": why.code(),
                "detail": line,
            }),
        );
        anyhow!("{line}")
    }
}

#[cfg(test)]
mod tests {
    //! The attack, and the repair. Each test drives the real client path — an `approve` command
    //! row through `Agent::handle_command`, exactly as `ClientCommand::Approve` builds it in
    //! `arbos-mac/src/sync/relay.rs` and `decideApproval` does on the phone.

    use super::*;
    use crate::status::tests::test_agent_with;
    use arbos_core::doc::CommandView;
    use arbos_core::ids::{now_ms, ulid};
    use axum::response::IntoResponse;
    use std::sync::Mutex;

    /// One test's relay: what it holds for the card under test. Each test owns its own, so the
    /// tests do not share state while they run beside each other.
    struct Relay {
        decision: Mutex<Option<Value>>,
        project_shared: bool,
        /// The relay is there and answers the route with an error.
        broken: bool,
    }

    impl Relay {
        /// A project the owner shared with a guest: the attack surface.
        fn shared() -> Arc<Self> {
            Arc::new(Relay {
                decision: Mutex::new(None),
                project_shared: true,
                broken: false,
            })
        }

        /// The owner's own project, nobody else invited.
        fn solo() -> Arc<Self> {
            Arc::new(Relay {
                decision: Mutex::new(None),
                project_shared: false,
                broken: false,
            })
        }

        fn broken() -> Arc<Self> {
            Arc::new(Relay {
                decision: Mutex::new(None),
                project_shared: false,
                broken: true,
            })
        }

        /// The owner answered the card over their own authenticated connection.
        fn authorise(&self, decided_by: &str, outcome: &str, grant: Option<&str>) {
            *self.decision.lock().unwrap() = Some(json!({
                "outcome": outcome, "reason": Value::Null, "grant_path_class": grant,
                "grant_chat_only": false, "decided_by": decided_by,
            }));
        }

        /// The box's two calls: what the owner authorised, and the standing grant the box
        /// forwards afterwards.
        fn router(self: &Arc<Self>) -> axum::Router {
            let held = self.clone();
            axum::Router::new()
                .route(
                    "/box/approvals/{approval}/decision",
                    axum::routing::get(move || {
                        let held = held.clone();
                        async move {
                            if held.broken {
                                return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
                            }
                            axum::Json(json!({
                                "decision": held.decision.lock().unwrap().clone(),
                                "project_shared": held.project_shared,
                            }))
                            .into_response()
                        }
                    }),
                )
                .route(
                    "/box/shell-grants",
                    axum::routing::get(|| async { axum::Json(json!({ "items": [] })) }).post(
                        |axum::Json(b): axum::Json<Value>| async move {
                            (axum::http::StatusCode::CREATED, axum::Json(b))
                        },
                    ),
                )
        }
    }

    /// A destructive-shell card waiting in `chat`.
    async fn card_for(agent: &Arc<Agent>, chat: &str, command: &str) -> String {
        agent.store.upsert_chat(chat, "Main", true, 0).unwrap();
        let _host = agent.sync.open_room(chat).await.unwrap();
        let v = agent
            .hook_approval("bash", &json!({ "command": command }), chat, "main")
            .await;
        assert_eq!(v["state"], "pending", "{v}");
        v["approvalId"].as_str().unwrap().to_owned()
    }

    /// A card already waiting, against the relay the test wants.
    async fn card_in(chat: &str, relay: Arc<Relay>) -> (Arc<Agent>, std::path::PathBuf, String) {
        let (agent, root) = test_agent_with(relay.router()).await;
        let id = card_for(&agent, chat, "rm -rf /work/app/dist").await;
        (agent, root, id)
    }

    /// A card already waiting in a chat of a project the owner shared with somebody.
    async fn chat_with_a_card(
        chat: &str,
        command: &str,
    ) -> (Arc<Agent>, Arc<Relay>, std::path::PathBuf, String) {
        let relay = Relay::shared();
        let (agent, root) = test_agent_with(relay.router()).await;
        let id = card_for(&agent, chat, command).await;
        (agent, relay, root, id)
    }

    /// The roster of a group chat as the relay's box-facing listing carries it: the owner, one
    /// guest, and this agent.
    fn group_roster(agent: &Arc<Agent>) -> crate::people::PeopleChatInfo {
        crate::people::PeopleChatInfo {
            members: vec![
                json!({ "member": "user:u-test", "handle": "jacob", "owner": "u-test" }),
                json!({ "member": "user:u-guest", "handle": "sam" }),
                json!({ "member": format!("agent:{}", agent.cfg.project_id), "handle": "doug" }),
            ],
            settings: json!({ "reply_policy": "mention" }),
            owner_user: "u-test".into(),
            slug: "doug".into(),
            kind: "group".into(),
            owner_address: None,
        }
    }

    /// The `approve` command an app pushes when somebody taps a card.
    fn approve_cmd(approval_id: &str, device: &str, grant: Option<&str>) -> CommandView {
        let mut payload = json!({ "approvalId": approval_id });
        if let Some(g) = grant {
            payload[GRANT_FIELD] = json!(g);
        }
        CommandView {
            id: ulid(),
            kind: "approve".into(),
            payload,
            issued_by: device.into(),
            issued_at: now_ms(),
            status: "pending".into(),
        }
    }

    /// The defect, driven as any member of a shared chat drives it: a command row that names
    /// somebody else as its issuer. Nobody authorised a decision on the relay, so the row
    /// decides nothing — whoever it claims to come from.
    #[tokio::test]
    async fn a_decision_nobody_authorised_is_refused() {
        let chat = "01TESTCHATNOAUTHORITY00000";
        let cmd = json!({ "command": "rm -rf /work/app/dist" });
        let (agent, relay, root, id) = chat_with_a_card(chat, "rm -rf /work/app/dist").await;

        let refused = agent
            .handle_command(chat, approve_cmd(&id, "dev:not-the-owner", None))
            .await;
        assert!(
            refused.is_err(),
            "an unauthorised Approve must be refused, got {refused:?}"
        );
        assert_eq!(
            agent.approval_status(&id).await["state"],
            "pending",
            "the card stays for the owner"
        );
        assert_eq!(
            agent.hook_approval("bash", &cmd, chat, "main").await["state"],
            "pending",
            "the gated call is still gated"
        );

        // Reject is the same decision from the other side, and equally not a row's to make: a
        // rejection the owner never authorised would otherwise beat the owner's own later
        // answer, because the first decision wins.
        let mut reject = approve_cmd(&id, "dev:not-the-owner", None);
        reject.kind = "reject".into();
        assert!(agent.handle_command(chat, reject).await.is_err());
        assert_eq!(agent.approval_status(&id).await["state"], "pending");

        // The owner answers the same card over their own authenticated connection, and it
        // decides — even though the command row still names the other device.
        relay.authorise("u-owner", "approved", None);
        agent
            .handle_command(chat, approve_cmd(&id, "dev:not-the-owner", None))
            .await
            .unwrap();
        assert_eq!(agent.approval_status(&id).await["state"], "approved");
        assert_eq!(
            agent.store.approval(&id).unwrap().unwrap().decided_by,
            Some("arbos:relay:u-owner".to_owned()),
            "the decider is the user the relay authenticated"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The dangerous half: the card's standing option, which answers every later card of its
    /// class with no card at all. A row cannot mint one, and cannot widen the one the owner
    /// did authorise.
    #[tokio::test]
    async fn a_row_can_neither_mint_nor_widen_a_standing_grant() {
        let chat = "01TESTCHATNOAUTHGRANT00000";
        let (agent, relay, root, id) =
            chat_with_a_card(chat, "cd /work/app && rm -rf node_modules").await;

        let refused = agent
            .handle_command(
                chat,
                approve_cmd(&id, "dev:not-the-owner", Some("anywhere")),
            )
            .await;
        assert!(
            refused.is_err(),
            "an unauthorised standing grant must be refused, got {refused:?}"
        );
        assert_eq!(
            agent
                .hook_approval("bash", &json!({ "command": "rm -rf /opt/x" }), chat, "main")
                .await["state"],
            "pending",
            "a later destructive command still asks"
        );

        // The owner authorised the narrow option — file deletes in /tmp. The row asks for
        // "anywhere"; what stands is what the owner chose, so a delete outside /tmp still asks.
        relay.authorise("u-owner", "approved", Some("scratch"));
        agent
            .handle_command(
                chat,
                approve_cmd(&id, "dev:not-the-owner", Some("anywhere")),
            )
            .await
            .unwrap();
        assert_eq!(
            agent
                .hook_approval("bash", &json!({ "command": "rm -rf /tmp/x" }), chat, "main")
                .await["state"],
            "allowed",
            "the owner's own choice stands"
        );
        assert_eq!(
            agent
                .hook_approval("bash", &json!({ "command": "rm -rf /opt/x" }), chat, "main")
                .await["state"],
            "pending",
            "the row must not widen the owner's choice to anywhere"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A card in a chat whose writers the box cannot establish waits: the relay answered the
    /// question with an error, so the box knows neither whose chat this is nor who answered.
    #[tokio::test]
    async fn a_relay_that_cannot_answer_decides_nothing() {
        let chat = "01TESTCHATRELAYDOWN0000000";
        let (agent, root, id) = card_in(chat, Relay::broken()).await;

        let refused = agent
            .handle_command(chat, approve_cmd(&id, "dev-mac", None))
            .await;
        assert!(refused.is_err(), "got {refused:?}");
        assert_eq!(agent.approval_status(&id).await["state"], "pending");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A relay older than this route has no authority to give and none to withhold. The box
    /// behaves as it did before — otherwise a box that updates first would answer no card at
    /// all — except in a chat the roster says has other members, where it still refuses.
    #[tokio::test]
    async fn a_relay_without_the_route_keeps_the_owners_own_chat_working() {
        let solo = "01TESTCHATOLDRELAYSOLO0000";
        // `test_agent`'s mock relay serves no approval route at all: every call is a 404.
        let (agent, root) = crate::status::tests::test_agent().await;
        let id = card_for(&agent, solo, "rm -rf /work/app/dist").await;
        agent
            .handle_command(solo, approve_cmd(&id, "dev-mac", None))
            .await
            .unwrap();
        assert_eq!(agent.approval_status(&id).await["state"], "approved");

        // A second box, because a card goes to the agent's pinned chat and this one's is the
        // group chat.
        let group = "01TESTCHATOLDRELAYGROUP000";
        let (shared_agent, shared_root) = crate::status::tests::test_agent().await;
        let shared_id = card_for(&shared_agent, group, "rm -rf /work/app/dist").await;
        shared_agent
            .people
            .test_insert(group, group_roster(&shared_agent));
        let refused = shared_agent
            .handle_command(group, approve_cmd(&shared_id, "dev-mac", None))
            .await;
        assert!(refused.is_err(), "got {refused:?}");
        assert_eq!(
            shared_agent.approval_status(&shared_id).await["state"],
            "pending"
        );
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&shared_root);
    }

    /// A group chat is shared whatever the project is: the roster the box holds names a member
    /// besides the owner, so a row there is not the owner's by construction either.
    #[tokio::test]
    async fn a_group_chat_needs_the_authority_too() {
        let chat = "01TESTCHATGROUPAUTHORITY00";
        let (agent, root, id) = card_in(chat, Relay::solo()).await;
        agent.people.test_insert(chat, group_roster(&agent));

        let refused = agent
            .handle_command(chat, approve_cmd(&id, "dev:not-the-owner", None))
            .await;
        assert!(
            refused.is_err(),
            "a group-chat row must not decide, got {refused:?}"
        );
        assert_eq!(agent.approval_status(&id).await["state"], "pending");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The owner's own project, nobody else invited: the room socket admits the owner's devices
    /// and this box and nothing else, so a row there is the owner's by the only route it could
    /// have come by, and it still decides. A box update does not stop these cards being
    /// answered by an app that has not learned the relay route yet.
    #[tokio::test]
    async fn an_unshared_chat_still_decides_from_the_row() {
        let chat = "01TESTCHATUNSHARED00000000";
        let (agent, root, id) = card_in(chat, Relay::solo()).await;

        agent
            .handle_command(chat, approve_cmd(&id, "dev-mac", None))
            .await
            .unwrap();
        let status = agent.approval_status(&id).await;
        assert_eq!(status["state"], "approved");
        assert_eq!(status["decidedBy"], "dev-mac");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The owner answered on a device that never writes into this chat (the web app, Settings):
    /// the relay's push carries the decision with no command row behind it.
    #[tokio::test]
    async fn a_push_applies_the_owners_decision_without_a_row() {
        let chat = "01TESTCHATDECISIONPUSH0000";
        let (agent, relay, root, id) = chat_with_a_card(chat, "rm -rf /work/app/dist").await;
        relay.authorise("u-owner", "rejected", None);

        agent.decide_approval_from_push(&id).await.unwrap();
        let status = agent.approval_status(&id).await;
        assert_eq!(status["state"], "rejected");
        assert_eq!(status["decidedBy"], "arbos:relay:u-owner");
        let _ = std::fs::remove_dir_all(&root);
    }
}
