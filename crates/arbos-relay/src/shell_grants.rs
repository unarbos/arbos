//! Standing shell approvals (#1544): "always allow <agent> to run `rm -rf` in /tmp" chosen on a
//! destructive-shell card. The relay holds the grants (per agent, optional per chat), the box
//! pulls them (`GET /box/shell-grants`) and skips the card when one covers the command, the
//! owner sees and revokes them in the agent's Settings › Permissions › Shell approvals. Every
//! grant, revoke and grant-allowed run is in the project's audit.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use arbos_core::approval::{PathClass, ShellGrant, GRANTABLE_CLASSES};
use arbos_core::ids::{now_ms, ulid};

use crate::auth::Principal;
use crate::db::Db;
use crate::secrets::{box_project, err, internal, ApiResult};
use crate::state::AppState;

pub const SHELL_GRANTS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS shell_grants (
  id TEXT PRIMARY KEY, project_id TEXT NOT NULL, rule_class TEXT NOT NULL, path_class TEXT NOT NULL,
  chat_id TEXT, granted_by TEXT NOT NULL, approval_id TEXT, created_at INTEGER NOT NULL,
  revoked_at INTEGER, revoked_by TEXT
);
CREATE INDEX IF NOT EXISTS shell_grants_project ON shell_grants(project_id, revoked_at);
"#;

const COLS: &str =
    "id, project_id, rule_class, path_class, chat_id, granted_by, approval_id, created_at";

fn row_to_grant(r: &rusqlite::Row<'_>) -> rusqlite::Result<ShellGrant> {
    let class: String = r.get(3)?;
    Ok(ShellGrant {
        id: r.get(0)?,
        project_id: r.get(1)?,
        rule_class: r.get(2)?,
        path_class: PathClass::parse(&class).unwrap_or(PathClass::Scratch),
        chat_id: r.get(4)?,
        granted_by: r.get(5)?,
        approval_id: r.get(6)?,
        created_at: r.get(7)?,
    })
}

impl Db {
    /// Active grants of a project, newest first.
    pub fn shell_grants(&self, project_id: &str) -> anyhow::Result<Vec<ShellGrant>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {COLS} FROM shell_grants WHERE project_id = ?1 AND revoked_at IS NULL ORDER BY created_at DESC"
            ))?;
            let rows = st.query_map(params![project_id], row_to_grant)?;
            rows.collect()
        })
    }

    pub fn shell_grant(&self, id: &str) -> anyhow::Result<Option<ShellGrant>> {
        self.with(|c| {
            c.query_row(
                &format!("SELECT {COLS} FROM shell_grants WHERE id = ?1 AND revoked_at IS NULL"),
                params![id],
                row_to_grant,
            )
            .optional()
        })
    }

    /// Add a grant; an identical active one (class, path class, chat) is returned instead of a
    /// duplicate so the Settings list never shows two rows for one choice.
    pub fn add_shell_grant(
        &self,
        project_id: &str,
        rule_class: &str,
        path_class: PathClass,
        chat_id: Option<&str>,
        granted_by: &str,
        approval_id: Option<&str>,
    ) -> anyhow::Result<ShellGrant> {
        self.with(|c| {
            let existing = c
                .query_row(
                    &format!(
                        "SELECT {COLS} FROM shell_grants WHERE project_id = ?1 AND rule_class = ?2 AND path_class = ?3 AND chat_id IS ?4 AND revoked_at IS NULL"
                    ),
                    params![project_id, rule_class, path_class.as_str(), chat_id],
                    row_to_grant,
                )
                .optional()?;
            if let Some(g) = existing {
                return Ok(g);
            }
            let g = ShellGrant {
                id: ulid(),
                project_id: project_id.to_owned(),
                rule_class: rule_class.to_owned(),
                path_class,
                chat_id: chat_id.map(str::to_owned),
                granted_by: granted_by.to_owned(),
                approval_id: approval_id.map(str::to_owned),
                created_at: now_ms(),
            };
            c.execute(
                &format!("INSERT INTO shell_grants({COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)"),
                params![
                    g.id,
                    g.project_id,
                    g.rule_class,
                    g.path_class.as_str(),
                    g.chat_id,
                    g.granted_by,
                    g.approval_id,
                    g.created_at
                ],
            )?;
            Ok(g)
        })
    }

    /// Revoke one grant. `Ok(false)` when it was not active.
    pub fn revoke_shell_grant(&self, id: &str, by: &str) -> anyhow::Result<bool> {
        self.with(|c| {
            let n = c.execute(
                "UPDATE shell_grants SET revoked_at = ?2, revoked_by = ?3 WHERE id = ?1 AND revoked_at IS NULL",
                params![id, now_ms(), by],
            )?;
            Ok(n > 0)
        })
    }
}

pub fn ensure_schema(c: &Connection) {
    let _ = c.execute_batch(SHELL_GRANTS_SCHEMA);
}

fn grant_json(g: &ShellGrant) -> Value {
    let mut v = serde_json::to_value(g).unwrap_or(Value::Null);
    if let Some(o) = v.as_object_mut() {
        o.insert("label".into(), json!(label(g)));
        o.insert("detail".into(), json!(g.path_class.describe()));
    }
    v
}

/// Title of a grant in the Settings list: "File deletes in /tmp".
pub fn label(g: &ShellGrant) -> String {
    let what = match g.rule_class.as_str() {
        "file-delete" => "File deletes",
        other => other,
    };
    match g.path_class {
        PathClass::Anywhere => format!("{what} anywhere"),
        c => format!("{what} in {}", c.label()),
    }
}

fn audit(state: &AppState, project_id: &str, kind: &str, g: &ShellGrant, by: &str) {
    let _ = state.db.record_audit(
        project_id,
        kind,
        &json!({
            "ts": now_ms(), "kind": kind, "source": "relay",
            "grant_id": g.id, "rule_class": g.rule_class, "path_class": g.path_class.as_str(),
            "chat_id": g.chat_id, "approval_id": g.approval_id, "by": by,
        }),
    );
}

fn push_box(state: &AppState, project_id: &str) {
    let _ = state
        .db
        .enqueue_push(project_id, "shell_grants_changed", &json!({}));
}

fn owned_project(
    state: &AppState,
    principal: &Principal,
    id: &str,
) -> Result<crate::db::Project, crate::secrets::ApiErr> {
    let p = state
        .db
        .project(id)
        .map_err(internal)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found", "no such project"))?;
    if !state.allows(principal, &p.id) {
        return Err(err(StatusCode::FORBIDDEN, "not_owner", "not your project"));
    }
    Ok(p)
}

/// `GET /projects/{id}/shell-grants` → `{ items: [grant + label + detail] }`.
pub async fn list(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> ApiResult {
    let p = owned_project(&state, &principal, &id)?;
    let items: Vec<Value> = state
        .db
        .shell_grants(&p.id)
        .map_err(internal)?
        .iter()
        .map(grant_json)
        .collect();
    Ok(Json(json!({ "items": items })).into_response())
}

#[derive(Debug, Deserialize)]
pub struct GrantBody {
    pub rule_class: String,
    pub path_class: String,
    #[serde(default)]
    pub chat_id: Option<String>,
    #[serde(default)]
    pub approval_id: Option<String>,
}

fn validated(body: &GrantBody) -> Result<PathClass, crate::secrets::ApiErr> {
    if !GRANTABLE_CLASSES.contains(&body.rule_class.as_str()) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "bad_request",
            format!("`{}` cannot be granted standing approval", body.rule_class),
        ));
    }
    PathClass::parse(&body.path_class).ok_or_else(|| {
        err(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "path_class must be scratch, workspace or anywhere",
        )
    })
}

/// `POST /projects/{id}/shell-grants` (owner device) — a standing grant from Settings.
pub async fn create(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(body): Json<GrantBody>,
) -> ApiResult {
    let user = principal
        .user_id()
        .ok_or_else(|| err(StatusCode::FORBIDDEN, "forbidden", "device token required"))?
        .to_owned();
    let p = owned_project(&state, &principal, &id)?;
    let class = validated(&body)?;
    let g = state
        .db
        .add_shell_grant(
            &p.id,
            &body.rule_class,
            class,
            body.chat_id.as_deref().filter(|c| !c.is_empty()),
            &user,
            body.approval_id.as_deref(),
        )
        .map_err(internal)?;
    audit(&state, &p.id, "shell_grant_added", &g, &user);
    push_box(&state, &p.id);
    Ok((StatusCode::CREATED, Json(grant_json(&g))).into_response())
}

/// `DELETE /projects/{id}/shell-grants/{grant}` — revoke; the agent asks again next time.
pub async fn revoke(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, grant)): Path<(String, String)>,
) -> ApiResult {
    let user = principal
        .user_id()
        .ok_or_else(|| err(StatusCode::FORBIDDEN, "forbidden", "device token required"))?
        .to_owned();
    let p = owned_project(&state, &principal, &id)?;
    let Some(g) = state.db.shell_grant(&grant).map_err(internal)? else {
        return Err(err(StatusCode::NOT_FOUND, "not_found", "no such grant"));
    };
    if g.project_id != p.id {
        return Err(err(StatusCode::NOT_FOUND, "not_found", "no such grant"));
    }
    state
        .db
        .revoke_shell_grant(&grant, &user)
        .map_err(internal)?;
    audit(&state, &p.id, "shell_grant_revoked", &g, &user);
    push_box(&state, &p.id);
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `GET /box/shell-grants` — the box's current grants.
pub async fn box_list(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
) -> ApiResult {
    let (_, project_id) = box_project(&state, &principal, &headers)?;
    let items: Vec<Value> = state
        .db
        .shell_grants(&project_id)
        .map_err(internal)?
        .iter()
        .map(grant_json)
        .collect();
    Ok(Json(json!({ "items": items })).into_response())
}

#[derive(Debug, Deserialize)]
pub struct BoxGrantBody {
    pub rule_class: String,
    pub path_class: String,
    #[serde(default)]
    pub chat_id: Option<String>,
    pub approval_id: String,
    /// The device / user that answered the card (`decidedBy` of the approval).
    pub granted_by: String,
}

/// `POST /box/shell-grants` — the box records the standing choice made on a card. The approval
/// itself was decided by a device of the owner; the box only forwards that choice.
pub async fn box_create(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
    Json(body): Json<BoxGrantBody>,
) -> ApiResult {
    let (_, project_id) = box_project(&state, &principal, &headers)?;
    let class = validated(&GrantBody {
        rule_class: body.rule_class.clone(),
        path_class: body.path_class.clone(),
        chat_id: None,
        approval_id: None,
    })?;
    let g = state
        .db
        .add_shell_grant(
            &project_id,
            &body.rule_class,
            class,
            body.chat_id.as_deref().filter(|c| !c.is_empty()),
            &body.granted_by,
            Some(&body.approval_id),
        )
        .map_err(internal)?;
    audit(
        &state,
        &project_id,
        "shell_grant_added",
        &g,
        &body.granted_by,
    );
    Ok((StatusCode::CREATED, Json(grant_json(&g))).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    #[test]
    fn grants_dedupe_and_revoke() {
        let st = test_state(AuthMode::Github);
        let a = st
            .db
            .add_shell_grant(
                "p1",
                "file-delete",
                PathClass::Scratch,
                None,
                "u1",
                Some("ap1"),
            )
            .unwrap();
        let b = st
            .db
            .add_shell_grant(
                "p1",
                "file-delete",
                PathClass::Scratch,
                None,
                "u1",
                Some("ap2"),
            )
            .unwrap();
        assert_eq!(a.id, b.id, "an identical active grant is reused");
        let c = st
            .db
            .add_shell_grant(
                "p1",
                "file-delete",
                PathClass::Anywhere,
                Some("chat"),
                "u1",
                None,
            )
            .unwrap();
        assert_eq!(st.db.shell_grants("p1").unwrap().len(), 2);
        assert_eq!(label(&a), "File deletes in /tmp");
        assert_eq!(label(&c), "File deletes anywhere");
        assert!(st.db.revoke_shell_grant(&a.id, "u1").unwrap());
        assert!(!st.db.revoke_shell_grant(&a.id, "u1").unwrap());
        let left = st.db.shell_grants("p1").unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, c.id);
        assert!(st.db.shell_grants("p2").unwrap().is_empty());
    }
}
