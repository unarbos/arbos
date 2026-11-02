//! Who may answer an approval card (S18, #1544): the relay's record of the decision, written
//! by an authenticated owner device and read back by the box.
//!
//! A card is the one gate between the model and real authority — a destructive shell command on
//! the box, an op on the owner's own computer through `arbos-local`, or, with the card's standing
//! option, a grant that answers every later card of its class with no card at all. The decision
//! used to travel only as a `commands` row in the chat doc, whose `issuedBy` the writing client
//! chooses, so any member of a shared chat could answer the owner's card by writing one.
//!
//! The decision therefore does not travel in the doc any more. The owner's app calls
//! `POST /projects/{id}/approvals/{approval}/decision`, which the relay authorises against the
//! authenticated principal, exactly as a secret request's decision already works
//! (`POST /secrets/requests/{id}/decide`, `secrets.rs`). The box reads the authorised decision
//! back over its own authenticated channel (`GET /box/approvals/{approval}/decision`) and acts
//! only on what it finds there.
//!
//! The relay holds no approvals of its own: a card lives in the box's store. A row here is an
//! authorisation for an approval id, scoped to the project whose owner wrote it. A decision
//! recorded under one project is invisible to every other project's box, so a stranger
//! authorising the same approval id under a project of their own buys nothing.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;

use arbos_core::approval::PathClass;
use arbos_core::ids::now_ms;

use crate::auth::Principal;
use crate::db::Db;
use crate::secrets::{box_project, device_user, err, internal, ApiErr, ApiResult};
use crate::state::AppState;

pub const APPROVAL_DECISIONS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS approval_decisions (
  project_id TEXT NOT NULL, approval_id TEXT NOT NULL, chat_id TEXT,
  outcome TEXT NOT NULL, reason TEXT, grant_path_class TEXT,
  grant_chat_only INTEGER NOT NULL DEFAULT 0,
  decided_by TEXT NOT NULL, created_at INTEGER NOT NULL,
  PRIMARY KEY (project_id, approval_id)
);
"#;

const COLS: &str = "project_id, approval_id, chat_id, outcome, reason, grant_path_class, \
                    grant_chat_only, decided_by, created_at";

/// One authorised decision: what the owner chose for one card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApprovalDecision {
    pub project_id: String,
    pub approval_id: String,
    /// The chat the card was answered in, when the app knows it. Informational.
    pub chat_id: Option<String>,
    /// `approved` or `rejected`.
    pub outcome: String,
    pub reason: Option<String>,
    /// The standing option the owner chose on a destructive-shell card, if any.
    pub grant_path_class: Option<String>,
    /// The standing option was limited to the card's own chat.
    pub grant_chat_only: bool,
    /// The user whose authenticated device answered. Attested by the relay, not claimed.
    pub decided_by: String,
    pub created_at: i64,
}

fn row_to_decision(r: &rusqlite::Row<'_>) -> rusqlite::Result<ApprovalDecision> {
    Ok(ApprovalDecision {
        project_id: r.get(0)?,
        approval_id: r.get(1)?,
        chat_id: r.get(2)?,
        outcome: r.get(3)?,
        reason: r.get(4)?,
        grant_path_class: r.get(5)?,
        grant_chat_only: r.get::<_, i64>(6)? != 0,
        decided_by: r.get(7)?,
        created_at: r.get(8)?,
    })
}

impl Db {
    pub fn approval_decision(
        &self,
        project_id: &str,
        approval_id: &str,
    ) -> anyhow::Result<Option<ApprovalDecision>> {
        self.with(|c| {
            c.query_row(
                &format!(
                    "SELECT {COLS} FROM approval_decisions WHERE project_id = ?1 AND approval_id = ?2"
                ),
                params![project_id, approval_id],
                row_to_decision,
            )
            .optional()
        })
    }

    /// Record an authorised decision. The first one for a card wins and is what comes back, so a
    /// second answer — a double tap, a second owner device, a later change of mind — never
    /// overturns what the box may already have acted on.
    pub fn authorise_approval_decision(
        &self,
        d: &ApprovalDecision,
    ) -> anyhow::Result<ApprovalDecision> {
        self.with(|c| {
            c.execute(
                &format!(
                    "INSERT INTO approval_decisions({COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
                     ON CONFLICT(project_id, approval_id) DO NOTHING"
                ),
                params![
                    d.project_id,
                    d.approval_id,
                    d.chat_id,
                    d.outcome,
                    d.reason,
                    d.grant_path_class,
                    i64::from(d.grant_chat_only),
                    d.decided_by,
                    d.created_at
                ],
            )?;
            c.query_row(
                &format!(
                    "SELECT {COLS} FROM approval_decisions WHERE project_id = ?1 AND approval_id = ?2"
                ),
                params![d.project_id, d.approval_id],
                row_to_decision,
            )
        })
    }
}

pub fn ensure_schema(c: &Connection) {
    let _ = c.execute_batch(APPROVAL_DECISIONS_SCHEMA);
}

#[derive(Debug, Deserialize)]
pub struct DecideBody {
    /// `approved` or `rejected`.
    pub outcome: String,
    #[serde(default)]
    pub reason: Option<String>,
    /// The card's standing option (`scratch`, `workspace`, `anywhere`), when one was chosen.
    #[serde(default)]
    pub grant: Option<String>,
    #[serde(default)]
    pub grant_chat_only: bool,
    #[serde(default)]
    pub chat_id: Option<String>,
}

fn validated(body: &DecideBody) -> Result<(String, Option<String>), ApiErr> {
    let outcome = match body.outcome.trim() {
        "approved" | "approve" => "approved",
        "rejected" | "reject" | "deny" | "denied" => "rejected",
        _ => {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "outcome must be approved or rejected",
            ))
        }
    };
    let grant = match body
        .grant
        .as_deref()
        .map(str::trim)
        .filter(|g| !g.is_empty())
    {
        None => None,
        Some(g) => Some(
            PathClass::parse(g)
                .ok_or_else(|| {
                    err(
                        StatusCode::BAD_REQUEST,
                        "bad_request",
                        "grant must be scratch, workspace or anywhere",
                    )
                })?
                .as_str()
                .to_owned(),
        ),
    };
    Ok((outcome.to_owned(), grant))
}

/// Write access: the agent's owner only. A card clears the gate in front of the owner's own box
/// and the owner's own computer, so a guest the owner invited to the project must not answer one
/// — and `state.allows` would let them, because it admits invited project members
/// (`state.rs:474`).
fn owned_project(
    state: &AppState,
    principal: &Principal,
    id: &str,
) -> Result<crate::db::Project, ApiErr> {
    let p = state
        .db
        .project(id)
        .map_err(internal)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found", "no such project"))?;
    if !state.owns(principal, &p.id) {
        return Err(err(StatusCode::FORBIDDEN, "not_owner", "not your project"));
    }
    Ok(p)
}

/// `POST /projects/{id}/approvals/{approval}/decision` (owner device) — answer a card.
///
/// `200 { decision }` with the authorised decision, which for a card already answered is the
/// first answer rather than this one; `409 already_decided` when this call asked for a different
/// outcome than the one that stands.
pub async fn decide(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, approval_id)): Path<(String, String)>,
    Json(body): Json<DecideBody>,
) -> ApiResult {
    let user = device_user(&principal)?;
    let p = owned_project(&state, &principal, &id)?;
    // A card id is a ULID from the box's store. The relay holds no cards of its own, so it
    // cannot check that this one exists — it can refuse to keep a row for something no card id
    // could be.
    if approval_id.is_empty() || approval_id.len() > 64 {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "approval id must be 1 to 64 characters",
        ));
    }
    let (outcome, grant) = validated(&body)?;
    let asked = ApprovalDecision {
        project_id: p.id.clone(),
        approval_id: approval_id.clone(),
        chat_id: body.chat_id.filter(|c| !c.is_empty()),
        outcome,
        reason: body
            .reason
            .map(|r| r.trim().to_owned())
            .filter(|r| !r.is_empty()),
        grant_path_class: grant,
        grant_chat_only: body.grant_chat_only,
        decided_by: user.clone(),
        created_at: now_ms(),
    };
    let stands = state
        .db
        .authorise_approval_decision(&asked)
        .map_err(internal)?;
    if stands.created_at == asked.created_at {
        let _ = state.db.record_audit(
            &p.id,
            "approval_decision_authorised",
            &json!({
                "ts": stands.created_at, "kind": "approval_decision_authorised", "source": "relay",
                "approval_id": stands.approval_id, "chat_id": stands.chat_id,
                "outcome": stands.outcome, "grant": stands.grant_path_class,
                "grant_chat_only": stands.grant_chat_only, "by": stands.decided_by,
            }),
        );
        let _ = state.db.enqueue_push(
            &p.id,
            "approval_decided",
            &json!({ "approval_id": stands.approval_id, "outcome": stands.outcome }),
        );
    }
    if stands.outcome != asked.outcome {
        return Err(err(
            StatusCode::CONFLICT,
            "already_decided",
            format!("this card was already {}", stands.outcome),
        ));
    }
    Ok(Json(json!({ "decision": stands })).into_response())
}

/// `GET /box/approvals/{approval}/decision` → `{ decision: … | null, project_shared }`.
///
/// `null` rather than `404` so the box can tell "nobody has authorised this" apart from "the
/// relay could not be asked", and fail closed on either with the right line in the log.
///
/// `project_shared` says whether anybody but the owner can write into this project's chats: a
/// guest the owner invited to the project (D72 step 3) has room access, so a command row in a
/// project chat is not necessarily the owner's. The box needs that to know when a card's answer
/// must be authorised rather than read out of the doc.
pub async fn box_decision(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
    Path(approval_id): Path<String>,
) -> ApiResult {
    let (_, project_id) = box_project(&state, &principal, &headers)?;
    let decision = state
        .db
        .approval_decision(&project_id, &approval_id)
        .map_err(internal)?;
    let shared = !state
        .db
        .project_members(&project_id)
        .map_err(internal)?
        .is_empty();
    Ok(Json(json!({ "decision": decision, "project_shared": shared })).into_response())
}

#[cfg(test)]
mod tests {
    //! A card's decision is the owner's. `state.allows` admits a guest the owner invited to the
    //! project (D72 step 3), and a group-chat member is not even that, so the guard here is
    //! `state.owns`: the project's own user, their devices and their box.

    use crate::db::Project;
    use crate::people::tests_support::person;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    use super::*;

    const PROJECT: &str = "proj_approval_decisions";
    const CARD: &str = "01TESTAPPROVALCARD00000001";

    fn status_of(r: &ApiResult) -> StatusCode {
        match r {
            Ok(res) => res.status(),
            Err(ApiErr(code, _, _)) => *code,
        }
    }

    fn insert_project(state: &AppState, owner: &str) {
        state
            .db
            .insert_project(&Project {
                id: PROJECT.into(),
                name: "Doug".into(),
                icon: "P".into(),
                instructions: String::new(),
                models: json!({}),
                state: "awake".into(),
                created_at: 0,
                last_active_at: 0,
                spend_cap_daily: 25.0,
                color: "#000".into(),
                user_id: owner.into(),
                workspace_state: "stopped".into(),
                avatar: None,
                avatar_color: None,
                description: String::new(),
                primary_chat_id: None,
                avatar_color_index: None,
                avatar_leaf: None,
                avatar_leaf_color: None,
                image_key: None,
                avatar_version: 0,
            })
            .expect("project");
    }

    fn card() -> Path<(String, String)> {
        Path((PROJECT.to_owned(), CARD.to_owned()))
    }

    fn approve(grant: Option<&str>) -> DecideBody {
        DecideBody {
            outcome: "approved".into(),
            reason: None,
            grant: grant.map(str::to_owned),
            grant_chat_only: false,
            chat_id: None,
        }
    }

    /// The one that matters: a guest of a shared project answering the owner's card, including
    /// the standing option that would answer every later card of its class by itself.
    #[tokio::test]
    async fn only_the_owner_authorises_a_decision() {
        let state = test_state(AuthMode::Github);
        let (owner, owner_dev) = person(&state, 61);
        let (guest, guest_dev) = person(&state, 62);
        insert_project(&state, &owner);
        assert!(state
            .db
            .add_project_member(PROJECT, &guest, &owner)
            .unwrap());

        let refused = decide(
            State(state.clone()),
            guest_dev,
            card(),
            Json(approve(Some("anywhere"))),
        )
        .await;
        assert_eq!(status_of(&refused), StatusCode::FORBIDDEN);
        assert!(
            state.db.approval_decision(PROJECT, CARD).unwrap().is_none(),
            "a guest's answer must leave no authority behind"
        );

        let made = decide(
            State(state.clone()),
            owner_dev,
            card(),
            Json(approve(Some("anywhere"))),
        )
        .await;
        assert_eq!(status_of(&made), StatusCode::OK);
        let stands = state
            .db
            .approval_decision(PROJECT, CARD)
            .unwrap()
            .expect("the owner's answer stands");
        assert_eq!(stands.outcome, "approved");
        assert_eq!(stands.grant_path_class.as_deref(), Some("anywhere"));
        assert_eq!(
            stands.decided_by, owner,
            "the decider is the authenticated user, not a string the client sent"
        );
    }

    /// The box reads only its own project's authorisations, so authorising the same card id
    /// under a project of one's own does not reach another owner's box.
    #[tokio::test]
    async fn an_authorisation_does_not_leave_its_project() {
        let state = test_state(AuthMode::Github);
        let (owner, owner_dev) = person(&state, 63);
        insert_project(&state, &owner);

        decide(State(state.clone()), owner_dev, card(), Json(approve(None)))
            .await
            .expect("the owner's own card");

        assert!(state
            .db
            .approval_decision("proj_somebody_else", CARD)
            .unwrap()
            .is_none());
        assert!(state
            .db
            .approval_decision(PROJECT, "01TESTSOMEOTHERCARD0000001")
            .unwrap()
            .is_none());
    }

    /// First answer wins: a second one cannot widen a standing grant the box has already acted
    /// on, and a second answer the other way is a conflict rather than a reversal.
    #[tokio::test]
    async fn the_first_answer_is_the_one_that_stands() {
        let state = test_state(AuthMode::Github);
        let (owner, owner_dev) = person(&state, 64);
        insert_project(&state, &owner);

        decide(
            State(state.clone()),
            owner_dev.clone(),
            card(),
            Json(approve(Some("scratch"))),
        )
        .await
        .expect("first answer");

        let again = decide(
            State(state.clone()),
            owner_dev.clone(),
            card(),
            Json(approve(Some("anywhere"))),
        )
        .await;
        assert_eq!(status_of(&again), StatusCode::OK);
        assert_eq!(
            state
                .db
                .approval_decision(PROJECT, CARD)
                .unwrap()
                .unwrap()
                .grant_path_class
                .as_deref(),
            Some("scratch"),
            "a later answer must not widen the grant"
        );

        let reversed = decide(
            State(state.clone()),
            owner_dev,
            card(),
            Json(DecideBody {
                outcome: "rejected".into(),
                reason: None,
                grant: None,
                grant_chat_only: false,
                chat_id: None,
            }),
        )
        .await;
        assert_eq!(status_of(&reversed), StatusCode::CONFLICT);
        assert_eq!(
            state
                .db
                .approval_decision(PROJECT, CARD)
                .unwrap()
                .unwrap()
                .outcome,
            "approved"
        );
    }

    #[tokio::test]
    async fn an_unknown_outcome_or_grant_is_refused() {
        let state = test_state(AuthMode::Github);
        let (owner, owner_dev) = person(&state, 65);
        insert_project(&state, &owner);

        let bad_outcome = decide(
            State(state.clone()),
            owner_dev.clone(),
            card(),
            Json(DecideBody {
                outcome: "maybe".into(),
                reason: None,
                grant: None,
                grant_chat_only: false,
                chat_id: None,
            }),
        )
        .await;
        assert_eq!(status_of(&bad_outcome), StatusCode::BAD_REQUEST);

        let bad_grant = decide(
            State(state.clone()),
            owner_dev,
            card(),
            Json(approve(Some("everywhere"))),
        )
        .await;
        assert_eq!(status_of(&bad_grant), StatusCode::BAD_REQUEST);
        assert!(state.db.approval_decision(PROJECT, CARD).unwrap().is_none());
    }
}
