//! Skills library (F5): reusable Markdown how-tos stored on the relay, shared across Projects.
//! A device gets full CRUD over the skills of the Projects its user can reach; a box reads the
//! shared skills plus its own Project's and may save skills it learned (`skills_save`), which
//! land as that Project's own.
//!
//! "Shared across Projects" is the whole of it: a skill is never shared across *people*. Every
//! route here asks the same ownership question of a device, and a listing shows exactly the
//! skills that device could also open by id (#1953).

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use arbos_core::api::SkillDto;
use arbos_core::ids::{now_ms, ulid};
use arbos_core::skill::{slugify, Skill};

use crate::auth::Principal;
use crate::db::SkillScope;
use crate::state::AppState;

type ApiResult = Result<Response, (StatusCode, String)>;

fn internal(e: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

/// The project a box call is for. One box serves every project of its user (D70), so the
/// per-workspace client names the project in `X-Arbos-Project`; it must belong to the box's user.
fn box_project(state: &AppState, principal: &Principal, headers: &HeaderMap) -> Option<String> {
    let Principal::Box { user_id } = principal else {
        return None;
    };
    let id = headers
        .get("x-arbos-project")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())?;
    let p = state.db.project(id).ok().flatten()?;
    (p.user_id == *user_id).then_some(p.id)
}

/// Which skills a listing may show this caller. A device is held to the projects its user can
/// reach, which is the question [`owns_skill`] asks on the by-id routes; a box keeps the view
/// [`can_see`] gives it, scoped to the workspace named in the request (without a project header
/// it sees the shared skills only).
fn scope_of(state: &AppState, principal: &Principal, headers: &HeaderMap) -> SkillScope {
    match principal {
        Principal::Admin => SkillScope::Everything,
        Principal::Device { user_id, .. } => SkillScope::Reachable(user_id.clone()),
        Principal::Box { .. } => SkillScope::Project(
            box_project(state, principal, headers).unwrap_or_else(|| "-".to_owned()),
        ),
    }
}

fn can_see(scope: Option<&str>, principal: &Principal, s: &Skill) -> bool {
    match principal {
        Principal::Device { .. } | Principal::Admin => true,
        Principal::Box { .. } => {
            s.shared || (scope.is_some() && s.origin_project_id.as_deref() == scope)
        }
    }
}

fn can_edit(scope: Option<&str>, principal: &Principal, s: &Skill) -> bool {
    match principal {
        Principal::Device { .. } | Principal::Admin => true,
        Principal::Box { .. } => scope.is_some() && s.origin_project_id.as_deref() == scope,
    }
}

/// Whether a device may act on this skill, which `can_edit` above does **not** answer: that one
/// returns `true` for every device principal and only constrains boxes.
///
/// `skills` has no owner column, so the only owner a skill can have is the user behind its
/// origin project. A skill with no origin project therefore has nobody to check the caller
/// against and keeps its previous behaviour — the honest limit of a fix that adds no column
/// (#1953). Deciding that an ownerless skill is admin-only is a product call.
/// Whether [`owns_skill`] is the right question for this caller. `can_see` and `can_edit` already
/// confine a box correctly; their device arm is the one that returns `true` for everything, so the
/// ownership check belongs on devices only. The admin token stays unscoped.
fn device_only(principal: &Principal) -> bool {
    matches!(principal, Principal::Device { .. })
}

fn owns_skill(state: &AppState, principal: &Principal, s: &Skill) -> bool {
    match s.origin_project_id.as_deref() {
        Some(project) => state.allows(principal, project),
        None => true,
    }
}

/// `POST`/`PUT` body: structured fields, or `{ "markdown": "---\n…" }` with the frontmatter.
#[derive(Deserialize, Default)]
pub struct SkillBody {
    #[serde(default)]
    pub markdown: Option<String>,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub when_to_use: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub shared: Option<bool>,
    /// Devices may file a skill under a Project; boxes always get their own.
    #[serde(default)]
    pub origin_project_id: Option<String>,
}

impl SkillBody {
    fn apply(self, base: Skill) -> Skill {
        let mut s = match &self.markdown {
            Some(md) => {
                let parsed = Skill::from_markdown(md);
                Skill {
                    id: base.id.clone(),
                    slug: if parsed.slug.is_empty() {
                        base.slug.clone()
                    } else {
                        parsed.slug
                    },
                    name: parsed.name,
                    description: parsed.description,
                    tags: parsed.tags,
                    when_to_use: parsed.when_to_use,
                    body: parsed.body,
                    origin_project_id: base.origin_project_id.clone(),
                    shared: base.shared,
                    version: base.version,
                    created_at: base.created_at,
                    updated_at: base.updated_at,
                }
            }
            None => base,
        };
        if let Some(v) = self.name {
            s.name = v.trim().to_owned();
        }
        if let Some(v) = self.description {
            s.description = v.trim().to_owned();
        }
        if let Some(v) = self.tags {
            s.tags = v
                .into_iter()
                .map(|t| t.trim().to_lowercase())
                .filter(|t| !t.is_empty())
                .collect();
        }
        if let Some(v) = self.when_to_use {
            s.when_to_use = v.trim().to_owned();
        }
        if let Some(v) = self.body {
            s.body = v.trim().to_owned();
        }
        if let Some(v) = self.shared {
            s.shared = v;
        }
        if let Some(v) = self.slug {
            s.slug = slugify(&v);
        }
        if s.slug.is_empty() {
            s.slug = slugify(&s.name);
        }
        s
    }
}

pub(crate) fn skill_dto(s: &Skill) -> SkillDto {
    SkillDto::from(s)
}

fn skill_json(s: &Skill) -> Value {
    serde_json::to_value(skill_dto(s)).expect("SkillDto")
}

fn index_json(s: &Skill) -> Value {
    json!({
        "id": s.id, "slug": s.slug, "name": s.name, "description": s.description, "tags": s.tags,
        "when_to_use": s.when_to_use, "shared": s.shared, "origin_project_id": s.origin_project_id,
        "version": s.version, "updated_at": s.updated_at,
    })
}

/// Queue `skills_changed` for every box so the index cache refreshes before its 60 s timer.
fn notify_boxes(state: &AppState, slug: &str) {
    if let Ok(ids) = state.db.sandbox_project_ids() {
        for id in ids {
            let _ = state
                .db
                .enqueue_push(&id, "skills_changed", &json!({ "slug": slug }));
        }
    }
}

/// `GET /skills` (device: all; box: shared + own) and `GET /skills/search?q=`.
pub async fn list(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> ApiResult {
    let scope = scope_of(&state, &principal, &headers);
    let rows = match q.get("q").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(query) => state
            .db
            .search_skills(query, &scope, 50)
            .map_err(internal)?,
        None => state.db.list_skills(&scope).map_err(internal)?,
    };
    Ok(Json(rows.iter().map(skill_json).collect::<Vec<_>>()).into_response())
}

/// `GET /skills/index` (box): the lean view the relevance selector caches, plus a change marker.
pub async fn index(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
) -> ApiResult {
    let scope = scope_of(&state, &principal, &headers);
    let rows = state.db.list_skills(&scope).map_err(internal)?;
    let rev = state.db.skills_rev(&scope).map_err(internal)?;
    Ok(Json(json!({
        "rev": rev,
        "skills": rows.iter().map(index_json).collect::<Vec<_>>(),
    }))
    .into_response())
}

pub async fn get(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    let scope = box_project(&state, &principal, &headers);
    let s = state
        .db
        .skill(&id)
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "no such skill".to_owned()))?;
    if !can_see(scope.as_deref(), &principal, &s) {
        return Err((StatusCode::FORBIDDEN, "not visible to this box".into()));
    }
    // Only the device arm needs this. `can_see` above is already right for a box — shared skills
    // plus its own project's — and a box does fetch another user's *shared* skill by id
    // (`arbos-agentd/src/skills.rs` reads `/skills/index` then the body of each changed entry),
    // so checking ownership here would drop it from that agent's set, silently: agentd treats a
    // non-success as `continue`. As `delete` does, a device asking for another user's skill gets
    // 404 rather than 403 (#1953).
    if device_only(&principal) && !owns_skill(&state, &principal, &s) {
        return Err((StatusCode::NOT_FOUND, "no such skill".to_owned()));
    }
    Ok(Json(skill_json(&s)).into_response())
}

/// `POST /skills`: create. A box posting an existing slug it owns updates that skill instead
/// (the agent's `skills_save` is an upsert).
pub async fn create(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
    Json(b): Json<SkillBody>,
) -> ApiResult {
    let now = now_ms();
    let scope = box_project(&state, &principal, &headers);
    let origin = match &principal {
        Principal::Box { .. } => scope.clone(),
        Principal::Device { .. } | Principal::Admin => b.origin_project_id.clone(),
    };
    // A device names its own origin project, and every ownership question about the skill is
    // answered from that field afterwards, so it has to be a project the caller actually has.
    // A foreign project answers like an unknown one. `allows` takes a project's name as well
    // as its id; store the id it resolved to rather than the caller's spelling, because the
    // listing scopes on that column in SQL and cannot resolve a name the same way.
    let origin = match origin.as_deref() {
        Some(named) => {
            let found = state
                .db
                .project(named)
                .map_err(internal)?
                .filter(|_| state.allows(&principal, named));
            let Some(project) = found else {
                return Err((StatusCode::NOT_FOUND, "no such project".to_owned()));
            };
            Some(project.id)
        }
        None => None,
    };
    let base = Skill {
        id: ulid(),
        origin_project_id: origin,
        version: 1,
        created_at: now,
        updated_at: now,
        ..Skill::default()
    };
    let s = b.apply(base);
    if s.name.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "name required".into()));
    }
    if let Some(existing) = state.db.skill(&s.slug).map_err(internal)? {
        if principal.is_box() && can_edit(scope.as_deref(), &principal, &existing) {
            let merged = Skill {
                id: existing.id.clone(),
                origin_project_id: existing.origin_project_id.clone(),
                created_at: existing.created_at,
                ..s
            };
            let row = state
                .db
                .update_skill(&merged)
                .map_err(internal)?
                .ok_or((StatusCode::NOT_FOUND, "no such skill".to_owned()))?;
            notify_boxes(&state, &row.slug);
            return Ok(Json(skill_json(&row)).into_response());
        }
        return Err((
            StatusCode::CONFLICT,
            format!("a skill with slug {} exists", s.slug),
        ));
    }
    // The creator is read from the authenticated principal, never from the body: a device and a
    // box both act for exactly one user, and the admin token acts for the operator, who is not a
    // tenant and so leaves the row unattributed. Until now the only record of who wrote a skill
    // was the log line below, which no query can reach (AF-588).
    state
        .db
        .insert_skill(&s, principal.user_id())
        .map_err(internal)?;
    notify_boxes(&state, &s.slug);
    tracing::info!(slug = %s.slug, by = %principal.device_name(), "skill created");
    Ok((StatusCode::CREATED, Json(skill_json(&s))).into_response())
}

/// `PUT /skills/{id}`: edit fields (partial body is fine); bumps the version.
pub async fn update(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<SkillBody>,
) -> ApiResult {
    let scope = box_project(&state, &principal, &headers);
    let existing = state
        .db
        .skill(&id)
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "no such skill".to_owned()))?;
    if !can_edit(scope.as_deref(), &principal, &existing) {
        return Err((StatusCode::FORBIDDEN, "not this box's skill".into()));
    }
    // `can_edit` already confines a box to its own project's skills; it is the device arm that is
    // open, returning `true` for every device. This closes that arm, the same way `delete` does
    // (#1953). A skill with no origin project has no owner to check against and stays editable.
    if device_only(&principal) && !owns_skill(&state, &principal, &existing) {
        return Err((StatusCode::NOT_FOUND, "no such skill".to_owned()));
    }
    let next = b.apply(existing.clone());
    if next.name.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "name required".into()));
    }
    if next.slug != existing.slug {
        if let Some(other) = state.db.skill(&next.slug).map_err(internal)? {
            if other.id != existing.id {
                return Err((
                    StatusCode::CONFLICT,
                    format!("a skill with slug {} exists", next.slug),
                ));
            }
        }
    }
    let row = state
        .db
        .update_skill(&next)
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "no such skill".to_owned()))?;
    notify_boxes(&state, &row.slug);
    tracing::info!(slug = %row.slug, version = row.version, by = %principal.device_name(), "skill updated");
    Ok(Json(skill_json(&row)).into_response())
}

/// `POST /skills/{id}/refile` body: the project to file the skill under, by id or by the
/// caller's own name for it.
#[derive(Deserialize)]
pub struct RefileBody {
    #[serde(default)]
    pub project_id: String,
}

/// One row in the `audit` table per re-file attempt, under the project it left and the project
/// it arrived at, so a move is answerable afterwards from the data rather than from a log.
fn audit_refile(
    state: &AppState,
    principal: &Principal,
    s: &Skill,
    to: Option<&str>,
    allowed: bool,
    reason: &str,
) {
    let (who, device) = match principal {
        Principal::Device { id, user_id, .. } => (format!("user:{user_id}"), Some(id.clone())),
        Principal::Box { user_id } => (format!("box:{user_id}"), None),
        Principal::Admin => ("admin".to_owned(), None),
    };
    let entry = json!({
        "ts": now_ms(), "target": "skill", "id": s.id, "slug": s.slug,
        "principal": who, "device_id": device,
        "from": s.origin_project_id, "to": to, "allowed": allowed, "reason": reason,
    });
    for project in [s.origin_project_id.as_deref(), to].into_iter().flatten() {
        if let Err(e) = state.db.record_audit(project, "skills", &entry) {
            tracing::warn!(project, error = %e, "skill refile audit row not written");
        }
    }
}

/// The first slug in `slug`, `slug-2`, `slug-3`, … that no skill holds. Skill slugs are unique
/// across the whole relay today, so a claimed copy cannot keep the slug of the row it was copied
/// from; #2568 scopes the namespace per owner, and the day it lands the first candidate is free
/// and a claim keeps the author's own slug with no change here.
fn free_slug(state: &AppState, slug: &str) -> Result<String, (StatusCode, String)> {
    for n in 1..=50 {
        let candidate = if n == 1 {
            slug.to_owned()
        } else {
            format!("{slug}-{n}")
        };
        if state.db.skill(&candidate).map_err(internal)?.is_none() {
            return Ok(candidate);
        }
    }
    Err((
        StatusCode::CONFLICT,
        format!("too many skills are called {slug}"),
    ))
}

/// `POST /skills/{id}/refile` (devices only): put a skill into one of the caller's own Projects.
///
/// This exists because a skill could be filed under a stranger's Project of the same name, and
/// the row cannot say whether that was the lookup's mistake or its author's choice — a guest may
/// legitimately file a skill into a Project they were invited to. Nothing can tell those apart,
/// so nothing automated moves them. The author does it, because the author is the only one who
/// knows which it was.
///
/// What the route does depends on whether the relay knows who wrote the row, and the difference
/// is the whole design rather than an implementation detail:
///
/// - **Creator recorded, and it is the caller: the row moves.** Authorship is proven, so there
///   is nothing to be careful about.
/// - **Creator recorded, and it is somebody else: refused.** A guest of a Project cannot take
///   its owner's skill, which is what makes the creator column worth having.
/// - **No creator — every row written before the column: the caller gets a copy, and the
///   original is left exactly where it is.** A move here would be a cross-tenant capability
///   handed out by a repair route: on an unprovable claim, one member of a Project could take a
///   skill out from under its owner, whose agents would lose it before anyone read an audit row.
///   A copy gives a genuine author their work back in their own library — their agents see it
///   again — while the Project it was filed under keeps what it has. Nobody has to prove
///   authorship because nothing is taken from anybody, and the cost is one duplicate row, which
///   whoever does not want it can delete.
///
/// Two rules hold in every case. The caller must be able to act on the skill at all
/// ([`owns_skill`], the same question `GET`, `PUT` and `DELETE` ask by id; anything else answers
/// 404). And the destination must be a Project the caller **owns**, resolved against their own
/// Projects and never by a relay-wide name lookup — filing into a Project you do not own is how a
/// skill got misfiled in the first place, so a repair route that allowed it would hand out the
/// same defect under a friendlier name.
///
/// A move answers `200`, a claim answers `201` with the new row, so a client can tell which
/// happened without comparing ids.
pub async fn refile(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<RefileBody>,
) -> ApiResult {
    let Principal::Device { user_id, .. } = &principal else {
        return Err((
            StatusCode::FORBIDDEN,
            "a signed-in device is required".into(),
        ));
    };
    let existing = state
        .db
        .skill(&id)
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "no such skill".to_owned()))?;
    if !owns_skill(&state, &principal, &existing) {
        // Another user's skill looks like no skill at all, as `delete` and `get` answer (#1953).
        return Err((StatusCode::NOT_FOUND, "no such skill".to_owned()));
    }
    let creator = state.db.skill_creator(&existing.id).map_err(internal)?;
    // A row with neither an origin project nor a recorded author names nobody, so there is no
    // claim to test and the relay holds it for everyone (#2557). The test is the author and not
    // the project: once a row *does* name its author, that author re-files it, which is how an
    // attributed row is recovered. A guard on the origin project alone would read as the same
    // rule and would instead lock the recovery out.
    if existing.origin_project_id.is_none() && creator.is_none() {
        audit_refile(&state, &principal, &existing, None, false, "no_owner");
        return Err((
            StatusCode::FORBIDDEN,
            "this skill records no Project and no author, so nobody may re-file it".into(),
        ));
    }
    if creator.as_deref().is_some_and(|c| c != user_id) {
        audit_refile(&state, &principal, &existing, None, false, "not_the_author");
        return Err((
            StatusCode::FORBIDDEN,
            "only the author of a skill re-files it".into(),
        ));
    }
    let wanted = b.project_id.trim();
    let destination = state
        .db
        .projects_of_user(user_id)
        .map_err(internal)?
        .into_iter()
        .find(|p| p.id == wanted || p.name == wanted)
        .ok_or((StatusCode::NOT_FOUND, "no such project".to_owned()))?;
    if existing.origin_project_id.as_deref() == Some(destination.id.as_str()) {
        return Ok(Json(skill_json(&existing)).into_response());
    }

    if creator.is_none() {
        let now = now_ms();
        let claim = Skill {
            id: ulid(),
            slug: free_slug(&state, &existing.slug)?,
            origin_project_id: Some(destination.id.clone()),
            version: 1,
            created_at: now,
            updated_at: now,
            ..existing.clone()
        };
        state
            .db
            .insert_skill(&claim, Some(user_id))
            .map_err(internal)?;
        audit_refile(
            &state,
            &principal,
            &existing,
            Some(&destination.id),
            true,
            "claimed",
        );
        notify_boxes(&state, &claim.slug);
        tracing::info!(
            slug = %claim.slug, from = %existing.slug, to = %destination.id,
            by = %principal.device_name(), "skill claimed"
        );
        return Ok((StatusCode::CREATED, Json(skill_json(&claim))).into_response());
    }

    let row = state
        .db
        .refile_skill(&existing.id, &destination.id)
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "no such skill".to_owned()))?;
    audit_refile(
        &state,
        &principal,
        &existing,
        Some(&destination.id),
        true,
        "refiled",
    );
    // Both libraries change: the skill leaves one Project's set and joins another.
    notify_boxes(&state, &row.slug);
    tracing::info!(
        slug = %row.slug, to = %destination.id, by = %principal.device_name(),
        "skill refiled"
    );
    Ok(Json(skill_json(&row)).into_response())
}

/// `DELETE /skills/{id}` (devices only).
pub async fn delete(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> ApiResult {
    if principal.is_box() {
        return Err((StatusCode::FORBIDDEN, "boxes cannot delete skills".into()));
    }
    let existing = state
        .db
        .skill(&id)
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "no such skill".to_owned()))?;
    if !owns_skill(&state, &principal, &existing) {
        // Another user's skill looks like no skill at all: no enumeration oracle either.
        return Err((StatusCode::NOT_FOUND, "no such skill".to_owned()));
    }
    state.db.delete_skill(&existing.id).map_err(internal)?;
    notify_boxes(&state, &existing.slug);
    tracing::info!(slug = %existing.slug, "skill deleted");
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::people::tests_support::{person, project_of};
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    fn status(r: ApiResult) -> StatusCode {
        match r {
            Ok(resp) => resp.status(),
            Err((s, _)) => s,
        }
    }

    /// Stands in for whatever a person or an agent wrote down in a skill.
    const ALICES_BODY: &str = "the steps Alice worked out and wrote down for herself";

    fn skill_of(state: &AppState, origin: Option<&str>, slug: &str) -> String {
        skill_with(state, origin, slug, false, "")
    }

    fn skill_with(
        state: &AppState,
        origin: Option<&str>,
        slug: &str,
        shared: bool,
        body: &str,
    ) -> String {
        let now = now_ms();
        let s = Skill {
            id: ulid(),
            slug: slug.to_owned(),
            name: slug.to_owned(),
            body: body.to_owned(),
            origin_project_id: origin.map(str::to_owned),
            shared,
            version: 1,
            created_at: now,
            updated_at: now,
            ..Skill::default()
        };
        // No creator: these stand for rows that existed before the relay stamped one, which is
        // every row on the relay today.
        state.db.insert_skill(&s, None).expect("skill");
        s.id
    }

    async fn body_text(r: ApiResult) -> String {
        let resp = r.expect("a listing answers");
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 22)
            .await
            .expect("body");
        String::from_utf8(bytes.to_vec()).expect("utf-8")
    }

    /// The slugs in a `GET /skills` answer, in the order the route returned them.
    async fn listed(state: &AppState, principal: Principal, headers: HeaderMap) -> Vec<String> {
        let text = body_text(
            list(
                State(state.clone()),
                principal,
                headers,
                Query(std::collections::HashMap::new()),
            )
            .await,
        )
        .await;
        serde_json::from_str::<Vec<Value>>(&text)
            .expect("an array of skills")
            .iter()
            .map(|s| s["slug"].as_str().unwrap_or_default().to_owned())
            .collect()
    }

    /// #1953: `delete` refused boxes and nothing else, so any signed-in device could delete
    /// another user's skill.
    ///
    /// The gate the issue proposed, `can_edit`, would not have changed that: it returns `true`
    /// for every device principal and only constrains boxes. The first assertion here keeps
    /// that on the record, so nobody applies the suggested fix, sees a green test and believes
    /// the hole is closed.
    #[tokio::test]
    async fn delete_refuses_a_skill_of_another_users_project() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let (_bob, bob_dev) = person(&state, 2);
        let project = project_of(&state, &alice, "Comms");
        let owned = skill_of(&state, Some(&project), "deploy-the-relay");
        let row = state.db.skill(&owned).unwrap().unwrap();

        assert!(
            can_edit(None, &bob_dev, &row),
            "can_edit is open to every device principal, so #1953's gate is not a check"
        );

        let foreign = delete(State(state.clone()), bob_dev, Path(owned.clone())).await;
        assert_eq!(status(foreign), StatusCode::NOT_FOUND);
        assert!(
            state.db.skill(&owned).unwrap().is_some(),
            "another user's skill survives"
        );

        let own = delete(State(state.clone()), alice_dev, Path(owned.clone())).await;
        assert_eq!(status(own), StatusCode::NO_CONTENT);
        assert!(state.db.skill(&owned).unwrap().is_none());
    }

    /// #1953, read and write by id: `can_see` and `can_edit` both return `true` for every
    /// device principal, so one user could fetch and edit another user's skill. Both now answer
    /// like `delete` does.
    ///
    /// This is the by-id half. The content leak was `GET /skills`, which handed every skill's
    /// full `body` to any device token and is the list the apps actually read;
    /// `a_device_lists_its_own_library_and_not_the_relays` is that half, and the two routes
    /// now answer the same question.
    #[tokio::test]
    async fn get_and_update_refuse_a_skill_of_another_users_project() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let (_bob, bob_dev) = person(&state, 2);
        let project = project_of(&state, &alice, "Comms");
        let owned = skill_of(&state, Some(&project), "deploy-the-relay");
        let row = state.db.skill(&owned).unwrap().unwrap();

        assert!(
            can_see(None, &bob_dev, &row) && can_edit(None, &bob_dev, &row),
            "neither gate constrains a device, which is why both routes needed this check"
        );

        let read = get(
            State(state.clone()),
            bob_dev.clone(),
            HeaderMap::new(),
            Path(owned.clone()),
        )
        .await;
        assert_eq!(status(read), StatusCode::NOT_FOUND);

        let edited = update(
            State(state.clone()),
            bob_dev,
            HeaderMap::new(),
            Path(owned.clone()),
            Json(SkillBody {
                name: Some("renamed by a stranger".into()),
                ..SkillBody::default()
            }),
        )
        .await;
        assert_eq!(status(edited), StatusCode::NOT_FOUND);
        assert_eq!(
            state.db.skill(&owned).unwrap().unwrap().name,
            "deploy-the-relay",
            "another user's skill is unchanged"
        );

        // The owner still reads and edits it.
        assert_eq!(
            status(
                get(
                    State(state.clone()),
                    alice_dev.clone(),
                    HeaderMap::new(),
                    Path(owned.clone())
                )
                .await
            ),
            StatusCode::OK
        );
        let own = update(
            State(state.clone()),
            alice_dev,
            HeaderMap::new(),
            Path(owned.clone()),
            Json(SkillBody {
                name: Some("Deploy the relay".into()),
                ..SkillBody::default()
            }),
        )
        .await;
        assert_eq!(status(own), StatusCode::OK);
        assert_eq!(
            state.db.skill(&owned).unwrap().unwrap().name,
            "Deploy the relay"
        );
    }

    /// A box must keep reading another user's **shared** skill by id. `GET /skills/index` lists
    /// it (`can_see` allows `shared` from any project), and `arbos-agentd/src/skills.rs` then
    /// fetches the body of every changed entry by id — and treats a non-success as `continue`, so
    /// refusing here would drop that skill from the agent's set with no error anywhere. The
    /// ownership check is a device-arm fix and must not reach boxes.
    #[tokio::test]
    async fn a_box_still_reads_another_users_shared_skill_by_id() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let (bob, _) = person(&state, 2);
        let alice_project = project_of(&state, &alice, "Alice work");
        let bob_project = project_of(&state, &bob, "Bob work");

        let now = now_ms();
        let shared = Skill {
            id: ulid(),
            slug: "shared-one".into(),
            name: "Shared one".into(),
            origin_project_id: Some(alice_project),
            shared: true,
            version: 1,
            created_at: now,
            updated_at: now,
            ..Skill::default()
        };
        state.db.insert_skill(&shared, None).expect("skill");

        let bobs_box = Principal::Box {
            user_id: bob.clone(),
        };
        let mut headers = HeaderMap::new();
        headers.insert("x-arbos-project", bob_project.parse().unwrap());

        let row = state.db.skill(&shared.id).unwrap().unwrap();
        assert!(
            !owns_skill(&state, &bobs_box, &row),
            "Bob's box does not own it — which is exactly why the check must not apply here"
        );
        assert_eq!(
            status(
                get(
                    State(state.clone()),
                    bobs_box.clone(),
                    headers.clone(),
                    Path(shared.id.clone())
                )
                .await
            ),
            StatusCode::OK,
            "the box reads the shared skill it just saw in its index"
        );

        // A skill that is *not* shared is still none of that box's business.
        let private = Skill {
            id: ulid(),
            slug: "private-one".into(),
            name: "Private one".into(),
            origin_project_id: row.origin_project_id.clone(),
            shared: false,
            version: 1,
            created_at: now,
            updated_at: now,
            ..Skill::default()
        };
        state.db.insert_skill(&private, None).expect("skill");
        assert_eq!(
            status(get(State(state.clone()), bobs_box, headers, Path(private.id)).await),
            StatusCode::FORBIDDEN,
            "and `can_see` still keeps an unshared one away from it"
        );
    }

    /// The honest limit of a fix with no schema change: `skills` has no owner column, so a
    /// skill with no origin project has nobody to check the caller against and stays deletable
    /// by any device. Recorded as a test so the gap is visible instead of assumed closed;
    /// whoever decides that ownerless skills are admin-only will change this expectation.
    #[tokio::test]
    async fn an_ownerless_skill_stays_deletable_by_any_device() {
        let state = test_state(AuthMode::Github);
        let (_bob, bob_dev) = person(&state, 2);
        let orphan = skill_of(&state, None, "no-owner-anywhere");

        let r = delete(State(state.clone()), bob_dev, Path(orphan.clone())).await;
        assert_eq!(status(r), StatusCode::NO_CONTENT);
        assert!(state.db.skill(&orphan).unwrap().is_none());
    }

    /// The delete check reads `origin_project_id`, and `create` takes that field straight from
    /// the request body, so a device must not be able to file a skill under a project it does
    /// not own — otherwise the library it is checked against is whatever the caller claims.
    #[tokio::test]
    async fn create_refuses_an_origin_project_the_caller_does_not_own() {
        let state = test_state(AuthMode::Github);
        let (alice, _alice_dev) = person(&state, 1);
        let (_bob, bob_dev) = person(&state, 2);
        let project = project_of(&state, &alice, "Comms");

        let r = create(
            State(state.clone()),
            bob_dev,
            HeaderMap::new(),
            Json(SkillBody {
                name: Some("planted".into()),
                origin_project_id: Some(project.clone()),
                ..SkillBody::default()
            }),
        )
        .await;
        assert_eq!(status(r), StatusCode::NOT_FOUND);
        assert!(
            state
                .db
                .list_skills(&SkillScope::Project(project))
                .unwrap()
                .is_empty(),
            "nothing lands in another user's library"
        );
    }

    /// #1953's substance. `GET /skills` answered every device with every skill on the relay,
    /// bodies and all, and it is the list the apps read: the Mac's Settings → Skills panel
    /// (`arbos-mac/src/sync/relay.rs` `refresh_skills`) and `arbosctl skills list`.
    ///
    /// A device now sees what its user can reach, which is the question `owns_skill` already
    /// asks on the by-id routes. Another user's **shared** skill is out too: `shared` reads
    /// "Shared with every project" in the Mac's own toggle and the agent's `skills_save` tool
    /// calls it "visible to every Project", so it says nothing about other people, and a device
    /// that cannot open the skill by id has no use for it in a list.
    #[tokio::test]
    async fn a_device_lists_its_own_library_and_not_the_relays() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let alices = project_of(&state, &alice, "Alice work");
        let bobs = project_of(&state, &bob, "Bob work");

        skill_with(&state, Some(&alices), "alices-notes", false, ALICES_BODY);
        skill_with(&state, Some(&alices), "alices-shared", true, ALICES_BODY);
        skill_of(&state, Some(&bobs), "bobs-notes");
        skill_of(&state, None, "nobodys-notes");

        let text = body_text(
            list(
                State(state.clone()),
                bob_dev.clone(),
                HeaderMap::new(),
                Query(std::collections::HashMap::new()),
            )
            .await,
        )
        .await;
        assert!(
            !text.contains(ALICES_BODY),
            "no part of another user's skill reaches Bob"
        );

        let mut mine = listed(&state, bob_dev, HeaderMap::new()).await;
        mine.sort();
        assert_eq!(
            mine,
            vec!["bobs-notes".to_owned(), "nobodys-notes".to_owned()],
            "Bob's own project's skill, and the ownerless one that has no owner to check against"
        );

        let mut hers = listed(&state, alice_dev, HeaderMap::new()).await;
        hers.sort();
        assert_eq!(
            hers,
            vec![
                "alices-notes".to_owned(),
                "alices-shared".to_owned(),
                "nobodys-notes".to_owned()
            ],
            "and Alice still has everything of hers"
        );
    }

    /// `allows` lets a guest of a shared project (D72) drive it, and `owns_skill` is built on
    /// `allows` rather than `owns` for that reason. The listing has to agree, or being invited
    /// to a project would show you its chats and hide its skills.
    #[tokio::test]
    async fn a_guest_of_a_project_sees_that_projects_skills() {
        let state = test_state(AuthMode::Github);
        let (alice, _alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let alices = project_of(&state, &alice, "Alice work");
        skill_of(&state, Some(&alices), "alices-notes");

        assert!(
            listed(&state, bob_dev.clone(), HeaderMap::new())
                .await
                .is_empty(),
            "nothing before the invitation"
        );

        state
            .db
            .add_project_member(&alices, &bob, &alice)
            .expect("invite");

        assert_eq!(
            listed(&state, bob_dev, HeaderMap::new()).await,
            vec!["alices-notes".to_owned()],
            "the guest reads the project's skills, as `owns_skill` already lets them"
        );
    }

    /// `GET /skills/search` takes the best 50 rows by relevance. Filtering the answer after
    /// that limit would be the quiet kind of broken: a relay with more skills than the limit
    /// would hand a user an empty search while their own skill sat just outside it. The scope
    /// belongs in the query.
    #[tokio::test]
    async fn a_search_crowded_by_other_users_skills_still_finds_the_callers_own() {
        let state = test_state(AuthMode::Github);
        let (alice, _alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let alices = project_of(&state, &alice, "Alice work");
        let bobs = project_of(&state, &bob, "Bob work");

        for i in 0..60 {
            skill_of(&state, Some(&alices), &format!("deploy-hers-{i:02}"));
        }
        skill_of(&state, Some(&bobs), "deploy-his-own");

        let text = body_text(
            list(
                State(state.clone()),
                bob_dev,
                HeaderMap::new(),
                Query(std::collections::HashMap::from([(
                    "q".to_owned(),
                    "deploy".to_owned(),
                )])),
            )
            .await,
        )
        .await;
        let slugs: Vec<String> = serde_json::from_str::<Vec<Value>>(&text)
            .expect("an array")
            .iter()
            .map(|s| s["slug"].as_str().unwrap_or_default().to_owned())
            .collect();
        assert_eq!(slugs, vec!["deploy-his-own".to_owned()]);
    }

    /// The scoping is the device arm only. A box keeps the view `can_see` gives it — the
    /// shared skills of the whole relay plus its own project's — because that is the library
    /// the agent works from, and the admin token keeps everything.
    #[tokio::test]
    async fn a_box_and_the_admin_token_list_what_they_always_did() {
        let state = test_state(AuthMode::Github);
        let (alice, _alice_dev) = person(&state, 1);
        let (bob, _bob_dev) = person(&state, 2);
        let alices = project_of(&state, &alice, "Alice work");
        let bobs = project_of(&state, &bob, "Bob work");

        skill_with(&state, Some(&alices), "alices-shared", true, "");
        skill_of(&state, Some(&alices), "alices-notes");
        skill_of(&state, Some(&bobs), "bobs-notes");

        let bobs_box = Principal::Box {
            user_id: bob.clone(),
        };
        let mut headers = HeaderMap::new();
        headers.insert("x-arbos-project", bobs.parse().unwrap());
        let mut seen = listed(&state, bobs_box, headers).await;
        seen.sort();
        assert_eq!(
            seen,
            vec!["alices-shared".to_owned(), "bobs-notes".to_owned()],
            "a box still reads the relay's shared skills; `a_box_still_reads_another_users_shared_skill_by_id` is the other half"
        );

        let mut all = listed(&state, Principal::Admin, HeaderMap::new()).await;
        all.sort();
        assert_eq!(
            all,
            vec![
                "alices-notes".to_owned(),
                "alices-shared".to_owned(),
                "bobs-notes".to_owned()
            ]
        );
    }

    /// `GET /skills/index` is the box's lean view, but a device token reaches it too, and it
    /// carries every skill's name, description and `when_to_use`. Scoping the body route and
    /// leaving the index open would just move the enumeration one route across.
    #[tokio::test]
    async fn the_index_view_is_scoped_like_the_listing() {
        let state = test_state(AuthMode::Github);
        let (alice, _alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let alices = project_of(&state, &alice, "Alice work");
        skill_of(&state, Some(&alices), "alices-notes");
        let bobs = project_of(&state, &bob, "Bob work");
        skill_of(&state, Some(&bobs), "bobs-notes");

        let text = body_text(index(State(state.clone()), bob_dev, HeaderMap::new()).await).await;
        assert!(text.contains("bobs-notes"));
        assert!(
            !text.contains("alices-notes"),
            "the index names only the skills this device can open"
        );
    }

    /// `POST /skills` as this caller, then the creator the relay stamped on the row.
    async fn create_and_read_creator(
        state: &AppState,
        principal: Principal,
        headers: HeaderMap,
        name: &str,
        origin: Option<&str>,
    ) -> Option<String> {
        let r = create(
            State(state.clone()),
            principal,
            headers,
            Json(SkillBody {
                name: Some(name.to_owned()),
                origin_project_id: origin.map(str::to_owned),
                ..SkillBody::default()
            }),
        )
        .await;
        assert_eq!(status(r), StatusCode::CREATED, "{name} was created");
        state
            .db
            .skill_creator(&slugify(name))
            .expect("creator read")
    }

    /// AF-588. Until this column the only record of who wrote a skill was the `skill created`
    /// log line, which carries `by = dev:<device_id>` and which no query can reach: every
    /// question of the form "whose row is this?" had to be answered from `origin_project_id`,
    /// and that field is empty on the rows the question is actually asked about.
    #[tokio::test]
    async fn a_new_skill_records_the_user_whose_device_created_it() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let alices = project_of(&state, &alice, "Alice work");
        let bobs = project_of(&state, &bob, "Bob work");

        assert_eq!(
            create_and_read_creator(
                &state,
                alice_dev,
                HeaderMap::new(),
                "Alice runbook",
                Some(&alices)
            )
            .await,
            Some(alice.clone())
        );
        assert_eq!(
            create_and_read_creator(
                &state,
                bob_dev,
                HeaderMap::new(),
                "Bob runbook",
                Some(&bobs)
            )
            .await,
            Some(bob),
            "and the next writer is recorded as themselves, not as whoever wrote first"
        );
    }

    /// A skill an agent saves (`skills_save` → `POST /skills` with a box token) is attributable
    /// too: one box serves exactly one user (D70), so the box token names a person as precisely
    /// as a device token does.
    #[tokio::test]
    async fn a_skill_an_agent_saves_records_the_user_behind_its_box() {
        let state = test_state(AuthMode::Github);
        let (bob, _bob_dev) = person(&state, 2);
        let bobs = project_of(&state, &bob, "Bob work");
        let mut headers = HeaderMap::new();
        headers.insert("x-arbos-project", bobs.parse().unwrap());

        assert_eq!(
            create_and_read_creator(
                &state,
                Principal::Box {
                    user_id: bob.clone(),
                },
                headers,
                "Learned how to cut a release",
                None,
            )
            .await,
            Some(bob)
        );
    }

    /// The admin token acts for the operator, who is not a tenant, so a skill it creates is
    /// recorded as belonging to nobody rather than to a guessed user. This is exactly the row
    /// shape the ownerless-skill question (AF-587) is about, and the column says so honestly
    /// instead of inventing an owner for it.
    #[tokio::test]
    async fn the_admin_token_leaves_a_new_skill_unattributed() {
        let state = test_state(AuthMode::Github);

        assert_eq!(
            create_and_read_creator(
                &state,
                Principal::Admin,
                HeaderMap::new(),
                "Relay library note",
                None,
            )
            .await,
            None,
            "no creator, and no project either: the operator is not a tenant"
        );
    }

    /// The column is write provenance, so an edit must not fill it in. A row that predates the
    /// column has no creator, and the first person to edit it afterwards is not its author —
    /// attributing those rows is a decision about data, taken once, with counts in front of it,
    /// not something the write path does quietly on the next `PUT`.
    #[tokio::test]
    async fn an_edit_never_invents_a_creator() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let alices = project_of(&state, &alice, "Alice work");
        let id = skill_of(&state, Some(&alices), "older-than-the-column");

        let r = update(
            State(state.clone()),
            alice_dev,
            HeaderMap::new(),
            Path(id.clone()),
            Json(SkillBody {
                name: Some("Edited by Alice".into()),
                ..SkillBody::default()
            }),
        )
        .await;
        assert_eq!(status(r), StatusCode::OK);
        assert_eq!(
            state.db.skill(&id).unwrap().unwrap().version,
            2,
            "the edit landed"
        );
        assert_eq!(
            state.db.skill_creator(&id).unwrap(),
            None,
            "and the row is still unattributed"
        );
    }

    /// The same rule across two people, which is the case that matters: a guest of a project
    /// may edit its skills (D72, through `allows`), and editing one does not make them its
    /// creator. Whoever later reads this column to decide whose row it is gets the author, not
    /// the last person to touch it.
    #[tokio::test]
    async fn a_guest_editing_a_skill_does_not_become_its_creator() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let alices = project_of(&state, &alice, "Alice work");
        let creator = create_and_read_creator(
            &state,
            alice_dev,
            HeaderMap::new(),
            "Shared runbook",
            Some(&alices),
        )
        .await;
        assert_eq!(creator, Some(alice.clone()));
        state
            .db
            .add_project_member(&alices, &bob, &alice)
            .expect("invite");

        let r = update(
            State(state.clone()),
            bob_dev,
            HeaderMap::new(),
            Path("shared-runbook".to_owned()),
            Json(SkillBody {
                body: Some("Bob's correction".into()),
                ..SkillBody::default()
            }),
        )
        .await;
        assert_eq!(status(r), StatusCode::OK, "a guest may edit it");
        assert_eq!(
            state.db.skill_creator("shared-runbook").unwrap(),
            Some(alice),
            "the creator is the author, not the editor"
        );
    }

    async fn refile_to(
        state: &AppState,
        principal: Principal,
        skill: &str,
        project: &str,
    ) -> ApiResult {
        refile(
            State(state.clone()),
            principal,
            Path(skill.to_owned()),
            Json(RefileBody {
                project_id: project.to_owned(),
            }),
        )
        .await
    }

    fn origin_of(state: &AppState, skill: &str) -> Option<String> {
        state.db.skill(skill).unwrap().unwrap().origin_project_id
    }

    /// A row with no Project and no author names nobody, so the relay holds it against everyone
    /// — the rule #2557 set for editing and deleting, which re-filing has to keep rather than
    /// hand a stranger a way around. Without this the claim arm is reachable on these rows: both
    /// gates pass vacuously, because `owns_skill` has no owner to test against and there is no
    /// author to compare the caller to, so Bob walks off with Alice's text under his own name.
    #[tokio::test]
    async fn an_unattributed_ownerless_skill_is_refiled_by_nobody() {
        let state = test_state(AuthMode::Github);
        let (_alice, _alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let his = project_of(&state, &bob, "Tern");
        let orphan = skill_with(&state, None, "ship-the-newsletter", false, ALICES_BODY);

        let r = refile_to(&state, bob_dev, &orphan, &his).await;
        assert_eq!(status(r), StatusCode::FORBIDDEN, "held, not claimable");
        assert!(
            state
                .db
                .list_skills(&SkillScope::Project(his.clone()))
                .unwrap()
                .is_empty(),
            "and he has nothing of hers"
        );
        assert_eq!(origin_of(&state, &orphan), None, "the row is where it was");
    }

    /// The other half of the same rule, and the reason the guard asks about the author instead
    /// of the Project. Once a row names its author — which is what the attribution migration
    /// writes — that author re-files it and gets it back. A guard on the missing Project alone
    /// would read as the same rule and would lock this out, leaving the migration writing a
    /// column that no route ever acts on.
    #[tokio::test]
    async fn an_attributed_ownerless_skill_is_refiled_by_its_author() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let hers = project_of(&state, &alice, "Tern");
        let now = now_ms();
        let restored = Skill {
            id: ulid(),
            slug: "ship-the-newsletter".to_owned(),
            name: "ship-the-newsletter".to_owned(),
            body: ALICES_BODY.to_owned(),
            origin_project_id: None,
            version: 1,
            created_at: now,
            updated_at: now,
            ..Skill::default()
        };
        state
            .db
            .insert_skill(&restored, Some(&alice))
            .expect("an ownerless row the migration has attributed to Alice");

        let r = refile_to(&state, alice_dev, &restored.id, &hers).await;
        assert_eq!(status(r), StatusCode::OK, "moved, not copied");
        assert_eq!(
            origin_of(&state, &restored.id).as_deref(),
            Some(hers.as_str()),
            "the row itself is in her library now, so her agents read it again"
        );
    }

    /// AF-589, the repair itself. Alice named "Tern" when she filed a skill, the relay resolved
    /// that to Bob's "Tern" because she is a guest of it, and from then on her own agents never
    /// saw the skill while Bob's did. Nothing in the row says so, and a guest's deliberate
    /// filing looks exactly the same, so the relay does not take the row off Bob on Alice's say
    /// so: she claims a copy into her own Tern, and his library is left alone. Her agents read
    /// the skill again; nobody loses anything; nobody had to prove authorship.
    #[tokio::test]
    async fn a_misfiled_skill_is_claimed_as_a_copy_and_the_original_stays_put() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, _bob_dev) = person(&state, 2);
        let hers = project_of(&state, &alice, "Tern");
        let his = project_of(&state, &bob, "Tern");
        state
            .db
            .add_project_member(&his, &alice, &bob)
            .expect("Alice is a guest of Bob's Tern");
        let misfiled = skill_with(
            &state,
            Some(&his),
            "ship-the-newsletter",
            false,
            ALICES_BODY,
        );

        let r = refile_to(&state, alice_dev, &misfiled, &hers).await;
        assert_eq!(status(r), StatusCode::CREATED, "a claim, not a move");
        assert_eq!(
            origin_of(&state, &misfiled).as_deref(),
            Some(his.as_str()),
            "Bob's row is exactly where it was"
        );

        let claimed = state
            .db
            .list_skills(&SkillScope::Project(hers.clone()))
            .unwrap();
        assert_eq!(claimed.len(), 1, "and Alice has one of her own");
        assert_eq!(claimed[0].body, ALICES_BODY, "with the text she wrote");
        assert_ne!(claimed[0].id, misfiled, "as a new row");
        assert_eq!(
            claimed[0].slug, "ship-the-newsletter-2",
            "under a free slug, because slugs are relay-wide until #2568"
        );
        assert_eq!(
            state.db.skill_creator(&claimed[0].id).unwrap(),
            Some(alice.clone()),
            "recorded as hers, so from here on only she can move it"
        );

        let arrived = state.db.audit(&hers, 10).unwrap();
        let left = state.db.audit(&his, 10).unwrap();
        assert_eq!(arrived.len(), 1, "written down where it arrived");
        assert_eq!(left.len(), 1, "and under the project it was copied from");
        assert_eq!(arrived[0]["reason"], "claimed");
        assert_eq!(arrived[0]["from"], his);
        assert_eq!(arrived[0]["to"], hers);
        assert_eq!(arrived[0]["principal"], format!("user:{alice}"));
    }

    /// The destination has to be the caller's own Project. Filing into a Project you do not own
    /// is how a skill got misfiled in the first place, so the repair refuses to do it again —
    /// even though `create` would have allowed it and even though the caller is a guest there.
    #[tokio::test]
    async fn a_refile_refuses_a_project_the_caller_only_guests_in() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, _bob_dev) = person(&state, 2);
        let hers = project_of(&state, &alice, "Alice work");
        let his = project_of(&state, &bob, "Bob work");
        state.db.add_project_member(&his, &alice, &bob).expect("ok");
        let mine = skill_of(&state, Some(&hers), "my-own-runbook");

        let r = refile_to(&state, alice_dev.clone(), &mine, &his).await;
        assert_eq!(status(r), StatusCode::NOT_FOUND);
        assert_eq!(
            origin_of(&state, &mine).as_deref(),
            Some(hers.as_str()),
            "nothing moved"
        );

        // And a Project that belongs to nobody she can reach answers the same way: no 403, so
        // the route is not a test for whether a project id exists.
        let (carol, _carol_dev) = person(&state, 3);
        let carols = project_of(&state, &carol, "Carol work");
        assert_eq!(
            status(refile_to(&state, alice_dev, &mine, &carols).await),
            StatusCode::NOT_FOUND
        );
    }

    /// Rule 2, and the reason the creator column landed before this route: once the relay knows
    /// who wrote a skill, nobody else moves it. Without this, a guest of a Project could file
    /// its owner's own skill into their own library and the owner's agents would simply lose it.
    #[tokio::test]
    async fn only_the_recorded_author_may_refile() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let hers = project_of(&state, &alice, "Alice work");
        let his = project_of(&state, &bob, "Bob work");
        state
            .db
            .add_project_member(&hers, &bob, &alice)
            .expect("Bob is a guest of Alice's project");

        // Alice writes it, so the relay records her.
        let creator = create_and_read_creator(
            &state,
            alice_dev,
            HeaderMap::new(),
            "Alice runbook",
            Some(&hers),
        )
        .await;
        assert_eq!(creator, Some(alice));

        let r = refile_to(&state, bob_dev, "alice-runbook", &his).await;
        assert_eq!(status(r), StatusCode::FORBIDDEN);
        assert_eq!(
            origin_of(&state, "alice-runbook").as_deref(),
            Some(hers.as_str()),
            "the guest cannot take it"
        );
        let refused = state.db.audit(&hers, 10).unwrap();
        assert_eq!(refused.len(), 1, "the attempt is written down");
        assert_eq!(refused[0]["allowed"], false);
        assert_eq!(refused[0]["reason"], "not_the_author");
    }

    /// The two arms answer the same caller differently, and the only thing that chooses between
    /// them is whether `creator` holds a value — so the refusal above is worth exactly as much
    /// as the relay's refusal to clear that column. A guest who could empty it would turn their
    /// own 403 into a 201 and walk away with a copy of a skill they were just told was not
    /// theirs, which is the one direction in this route where missing data pays.
    ///
    /// Nothing can empty it: `creator` is written once, by the `INSERT` in `insert_skill`, and
    /// no `UPDATE` in the relay names the column. This drives the widest writes a guest has
    /// against the row — a whole-document markdown replacement, and a body that names a slug
    /// and an origin project of its own — and then asks the route again.
    #[tokio::test]
    async fn no_edit_a_guest_can_make_turns_their_refusal_into_a_claim() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let hers = project_of(&state, &alice, "Alice work");
        let his = project_of(&state, &bob, "Bob work");
        state
            .db
            .add_project_member(&hers, &bob, &alice)
            .expect("Bob is a guest of Alice's project");
        assert_eq!(
            create_and_read_creator(
                &state,
                alice_dev,
                HeaderMap::new(),
                "Alice runbook",
                Some(&hers)
            )
            .await,
            Some(alice.clone())
        );
        let id = state.db.skill("alice-runbook").unwrap().unwrap().id;

        assert_eq!(
            status(refile_to(&state, bob_dev.clone(), &id, &his).await),
            StatusCode::FORBIDDEN,
            "the author is recorded, so the guest is refused"
        );

        // Every write the guest can reach, by id so that renaming the row does not hide it.
        for attempt in [
            SkillBody {
                markdown: Some("---\nname: Bobs runbook\n---\n\nBob's text".into()),
                ..SkillBody::default()
            },
            SkillBody {
                slug: Some("bobs-runbook".into()),
                origin_project_id: Some(his.clone()),
                shared: Some(true),
                ..SkillBody::default()
            },
        ] {
            let r = update(
                State(state.clone()),
                bob_dev.clone(),
                HeaderMap::new(),
                Path(id.clone()),
                Json(attempt),
            )
            .await;
            assert_eq!(status(r), StatusCode::OK, "a guest may edit it (D72)");
            assert_eq!(
                state.db.skill_creator(&id).unwrap(),
                Some(alice.clone()),
                "and the edit leaves the author exactly as they were"
            );
        }
        assert_eq!(
            state.db.skill(&id).unwrap().unwrap().origin_project_id,
            Some(hers.clone()),
            "an edit does not move the row either: only this route does that"
        );

        assert_eq!(
            status(refile_to(&state, bob_dev, &id, &his).await),
            StatusCode::FORBIDDEN,
            "so the second ask is refused for the same reason as the first"
        );
        let all = state.db.list_skills(&SkillScope::Everything).unwrap();
        assert_eq!(
            all.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec![id.as_str()],
            "and no claimed copy was written: Alice's row is still the only one"
        );
        let refused = state.db.audit(&hers, 10).unwrap();
        assert_eq!(refused.len(), 2, "both attempts are written down");
        assert!(refused
            .iter()
            .all(|r| r["allowed"] == false && r["reason"] == "not_the_author"),);
    }

    /// A skill nobody can reach does not become reachable through this route, and it answers
    /// like a skill that is not there rather than confirming one exists.
    #[tokio::test]
    async fn a_refile_cannot_pull_a_strangers_skill_across() {
        let state = test_state(AuthMode::Github);
        let (alice, _alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let hers = project_of(&state, &alice, "Alice work");
        let his = project_of(&state, &bob, "Bob work");
        let not_his = skill_of(&state, Some(&hers), "alices-notes");

        let r = refile_to(&state, bob_dev, &not_his, &his).await;
        assert_eq!(status(r), StatusCode::NOT_FOUND);
        assert_eq!(origin_of(&state, &not_his).as_deref(), Some(hers.as_str()));
        assert!(
            state.db.audit(&hers, 10).unwrap().is_empty(),
            "and a stranger cannot write rows into another tenant's audit log by trying"
        );
    }

    /// A box must not re-file: an agent runs inside one Project and the whole point of the route
    /// is a person deciding where their skill belongs.
    #[tokio::test]
    async fn a_box_cannot_refile() {
        let state = test_state(AuthMode::Github);
        let (bob, _bob_dev) = person(&state, 2);
        let his = project_of(&state, &bob, "Bob work");
        let other = project_of(&state, &bob, "Bob side project");
        let skill = skill_of(&state, Some(&his), "bobs-notes");

        let r = refile_to(
            &state,
            Principal::Box {
                user_id: bob.clone(),
            },
            &skill,
            &other,
        )
        .await;
        assert_eq!(status(r), StatusCode::FORBIDDEN);
        assert_eq!(origin_of(&state, &skill).as_deref(), Some(his.as_str()));
    }

    /// A row the relay watched the caller write moves, because authorship is not in question,
    /// and moving it changes where it is filed and nothing else: same row, same slug, same text,
    /// same version. A reader who took this for an edit would expect the version bump `PUT`
    /// makes, and there is none.
    #[tokio::test]
    async fn the_author_of_a_recorded_skill_moves_it_rather_than_copying_it() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let one = project_of(&state, &alice, "One");
        let two = project_of(&state, &alice, "Two");
        assert_eq!(
            create_and_read_creator(
                &state,
                alice_dev.clone(),
                HeaderMap::new(),
                "Runbook",
                Some(&one)
            )
            .await,
            Some(alice)
        );
        let before = state.db.skill("runbook").unwrap().unwrap();

        assert_eq!(
            status(refile_to(&state, alice_dev, "runbook", "Two").await),
            StatusCode::OK,
            "a caller may name their own project rather than paste an id"
        );
        let after = state.db.skill("runbook").unwrap().unwrap();
        assert_eq!(after.origin_project_id.as_deref(), Some(two.as_str()));
        assert_eq!(after.id, before.id, "the same row, not a copy of it");
        assert_eq!(
            (after.slug, after.name, after.body, after.version),
            (before.slug, before.name, before.body, before.version),
        );
        assert!(
            state
                .db
                .list_skills(&SkillScope::Project(one))
                .unwrap()
                .is_empty(),
            "and it has left the project it was in"
        );
    }
}
