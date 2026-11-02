//! Skills library (F5): reusable Markdown how-tos stored on the relay, shared across Projects.
//! A device gets full CRUD over the skills of the Projects its user can reach; a box reads the
//! shared skills plus its own Project's and may save skills it learned (`skills_save`), which
//! land as that Project's own.
//!
//! "Shared across Projects" is the whole of it: a skill is never shared across *people*. Every
//! route here asks the same ownership question of a device, and a listing shows exactly the
//! skills that device could also open by id (#1953).
//!
//! A slug names one skill per **owner** — the user behind its origin Project — and not one on
//! the relay. It was relay-wide until the column-level `UNIQUE` came off `skills.slug`, and a
//! shared name space is a cross-tenant channel in both directions: `POST /skills` answered 409
//! naming the slug whenever anyone at all held it, which told a caller whether a stranger held
//! a name, and one person taking a name took it from everybody with nothing on screen to say
//! why. [`Db::skill_holding_slug`] is the whole of the new rule.

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

/// Whether this skill is the caller's own, for choosing between skills that share a slug. Not
/// an authorisation question — the gates on each route still answer that, and this one is
/// deliberately narrower than they are: an ownerless skill passes `owns_skill` for everybody,
/// which makes it a poor tie-break against a caller's own row.
fn is_own(state: &AppState, scope: Option<&str>, principal: &Principal, s: &Skill) -> bool {
    match principal {
        Principal::Box { .. } => can_edit(scope, principal, s),
        Principal::Device { .. } => {
            s.origin_project_id.is_some() && owns_skill(state, principal, s)
        }
        Principal::Admin => false,
    }
}

/// A skill by id, or by slug within what this caller can see. The by-id routes have always
/// taken either, and `arbosctl skills show|delete <slug>` is built on that. A slug now names
/// one skill per owner, so the slug arm is held to the caller's own scope: a stranger's skill
/// never answers one, and the caller's own wins over a skill they merely have a view of.
fn resolve(
    state: &AppState,
    principal: &Principal,
    headers: &HeaderMap,
    id_or_slug: &str,
) -> Result<Option<Skill>, (StatusCode, String)> {
    if let Some(found) = state.db.skill_by_id(id_or_slug).map_err(internal)? {
        return Ok(Some(found));
    }
    let rows = state
        .db
        .skills_by_slug(id_or_slug, &scope_of(state, principal, headers))
        .map_err(internal)?;
    let project = box_project(state, principal, headers);
    Ok(rows
        .iter()
        .find(|s| is_own(state, project.as_deref(), principal, s))
        .or_else(|| rows.first())
        .cloned())
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
    let s = resolve(&state, &principal, &headers, &id)?
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
    // The owner comes out of the same lookup: a slug is unique within the person whose Project
    // the skill lands in, which is not always the caller (a guest files into the host's).
    let (origin, owner) = match origin.as_deref() {
        Some(named) => {
            let found = state
                .db
                .project(named)
                .map_err(internal)?
                .filter(|_| state.allows(&principal, named));
            let Some(project) = found else {
                return Err((StatusCode::NOT_FOUND, "no such project".to_owned()));
            };
            (Some(project.id), Some(project.user_id))
        }
        None => (None, None),
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
    let visible = scope_of(&state, &principal, &headers);
    if let Some(existing) = state
        .db
        .skill_holding_slug(&s.slug, &visible, owner.as_deref())
        .map_err(internal)?
    {
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
    state.db.insert_skill(&s).map_err(internal)?;
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
    let existing = resolve(&state, &principal, &headers, &id)?
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
        // A rename asks the same question a create does, and used to leak the same answer.
        let owner = existing
            .origin_project_id
            .as_deref()
            .and_then(|p| state.user_of(p));
        let visible = scope_of(&state, &principal, &headers);
        if let Some(other) = state
            .db
            .skill_holding_slug(&next.slug, &visible, owner.as_deref())
            .map_err(internal)?
        {
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

/// `DELETE /skills/{id}` (devices only).
pub async fn delete(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> ApiResult {
    if principal.is_box() {
        return Err((StatusCode::FORBIDDEN, "boxes cannot delete skills".into()));
    }
    // Boxes are refused above, so no `X-Arbos-Project` header is in play here.
    let existing = resolve(&state, &principal, &HeaderMap::new(), &id)?
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
    use crate::db::Db;
    use crate::people::tests_support::{person, project_of};
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;
    use rusqlite::Connection;

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
        state.db.insert_skill(&s).expect("skill");
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
        let row = state.db.skill_by_id(&owned).unwrap().unwrap();

        assert!(
            can_edit(None, &bob_dev, &row),
            "can_edit is open to every device principal, so #1953's gate is not a check"
        );

        let foreign = delete(State(state.clone()), bob_dev, Path(owned.clone())).await;
        assert_eq!(status(foreign), StatusCode::NOT_FOUND);
        assert!(
            state.db.skill_by_id(&owned).unwrap().is_some(),
            "another user's skill survives"
        );

        let own = delete(State(state.clone()), alice_dev, Path(owned.clone())).await;
        assert_eq!(status(own), StatusCode::NO_CONTENT);
        assert!(state.db.skill_by_id(&owned).unwrap().is_none());
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
        let row = state.db.skill_by_id(&owned).unwrap().unwrap();

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
            state.db.skill_by_id(&owned).unwrap().unwrap().name,
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
            state.db.skill_by_id(&owned).unwrap().unwrap().name,
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
        state.db.insert_skill(&shared).expect("skill");

        let bobs_box = Principal::Box {
            user_id: bob.clone(),
        };
        let mut headers = HeaderMap::new();
        headers.insert("x-arbos-project", bob_project.parse().unwrap());

        let row = state.db.skill_by_id(&shared.id).unwrap().unwrap();
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
        state.db.insert_skill(&private).expect("skill");
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
        assert!(state.db.skill_by_id(&orphan).unwrap().is_none());
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

    // ---- the slug name space ---------------------------------------------------------------

    /// Ask the relay to create a skill under `project`, naming `slug`.
    async fn create_slug(
        state: &AppState,
        principal: Principal,
        headers: HeaderMap,
        project: Option<&str>,
        slug: &str,
    ) -> ApiResult {
        create(
            State(state.clone()),
            principal,
            headers,
            Json(SkillBody {
                name: Some(slug.to_owned()),
                slug: Some(slug.to_owned()),
                origin_project_id: project.map(str::to_owned),
                ..SkillBody::default()
            }),
        )
        .await
    }

    /// A box of `user`, working in `project`.
    fn box_in(user: &str, project: &str) -> (Principal, HeaderMap) {
        let mut headers = HeaderMap::new();
        headers.insert("x-arbos-project", project.parse().unwrap());
        (
            Principal::Box {
                user_id: user.to_owned(),
            },
            headers,
        )
    }

    /// The first effect of a relay-wide name space: an existence oracle across tenants.
    /// `POST /skills` looked a slug up over every row, so a 409 naming it told the caller that
    /// somebody held that name. Ask for enough names and you have read the shape of a
    /// stranger's library without ever being allowed to open one skill in it.
    ///
    /// Fails before the fix with 409 CONFLICT, whose body names the slug Alice holds.
    #[tokio::test]
    async fn a_create_does_not_say_whether_a_stranger_holds_the_slug() {
        let state = test_state(AuthMode::Github);
        let (alice, _alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let alices = project_of(&state, &alice, "Alice work");
        let bobs = project_of(&state, &bob, "Bob work");
        skill_with(
            &state,
            Some(&alices),
            "deploy-the-relay",
            false,
            ALICES_BODY,
        );

        let r = create_slug(
            &state,
            bob_dev.clone(),
            HeaderMap::new(),
            Some(&bobs),
            "deploy-the-relay",
        )
        .await;
        let (status_code, text) = match r {
            Ok(resp) => (resp.status(), String::new()),
            Err((s, body)) => (s, body),
        };
        assert_eq!(
            status_code,
            StatusCode::CREATED,
            "a name a stranger holds has to answer exactly like a free one; got {text:?}"
        );
        assert!(
            !text.contains("deploy-the-relay"),
            "and no refusal may name the slug back"
        );
    }

    /// The second effect: one tenant could take a name away from everybody. Alice holding
    /// `deploy-the-relay` left Bob unable to call his own how-to that, with nothing he could
    /// see to explain why — his own library looked full of names that were not there.
    ///
    /// Fails before the fix: Bob's skill is never created, so his listing stays empty.
    #[tokio::test]
    async fn a_slug_one_tenant_holds_is_free_for_another() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let alices = project_of(&state, &alice, "Alice work");
        let bobs = project_of(&state, &bob, "Bob work");
        let hers = skill_with(
            &state,
            Some(&alices),
            "deploy-the-relay",
            false,
            ALICES_BODY,
        );

        create_slug(
            &state,
            bob_dev.clone(),
            HeaderMap::new(),
            Some(&bobs),
            "deploy-the-relay",
        )
        .await
        .expect("Bob takes the name in his own library");

        assert_eq!(
            listed(&state, bob_dev.clone(), HeaderMap::new()).await,
            vec!["deploy-the-relay".to_owned()],
            "Bob has the name he asked for"
        );
        assert_eq!(
            listed(&state, alice_dev, HeaderMap::new()).await,
            vec!["deploy-the-relay".to_owned()],
            "and Alice still has hers"
        );
        let row = state.db.skill_by_id(&hers).unwrap().unwrap();
        assert_eq!(row.body, ALICES_BODY, "which Bob's create did not touch");

        let text = body_text(
            list(
                State(state.clone()),
                bob_dev,
                HeaderMap::new(),
                Query(std::collections::HashMap::new()),
            )
            .await,
        )
        .await;
        assert!(
            !text.contains(ALICES_BODY),
            "and sharing a slug shares nothing else"
        );
    }

    /// The agent's `skills_save` is this same create with a box token, so the denial reached
    /// the agents too: a box could not write down a how-to under the obvious name because
    /// another user's box had written one down first, and `arbos-agentd` surfaced it as
    /// "relay refused: HTTP 409".
    ///
    /// Fails before the fix with 409 CONFLICT.
    #[tokio::test]
    async fn a_box_saves_a_how_to_whose_slug_a_stranger_holds() {
        let state = test_state(AuthMode::Github);
        let (alice, _alice_dev) = person(&state, 1);
        let (bob, _bob_dev) = person(&state, 2);
        let alices = project_of(&state, &alice, "Alice work");
        let bobs = project_of(&state, &bob, "Bob work");
        skill_with(&state, Some(&alices), "weekly-report", false, ALICES_BODY);

        let (bobs_box, headers) = box_in(&bob, &bobs);
        let r = create_slug(&state, bobs_box, headers, None, "weekly-report").await;
        assert_eq!(status(r), StatusCode::CREATED);
        assert_eq!(
            state
                .db
                .list_skills(&SkillScope::Project(bobs))
                .unwrap()
                .iter()
                .map(|s| s.slug.clone())
                .collect::<Vec<_>>(),
            vec!["weekly-report".to_owned()],
            "the box's own Project holds it"
        );
    }

    /// A rename asks the same question a create does. `PUT /skills/{id}` checked the new slug
    /// over every row too, so the oracle answered on that route as well.
    ///
    /// Fails before the fix with 409 CONFLICT.
    #[tokio::test]
    async fn a_rename_does_not_say_whether_a_stranger_holds_the_slug() {
        let state = test_state(AuthMode::Github);
        let (alice, _alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let alices = project_of(&state, &alice, "Alice work");
        let bobs = project_of(&state, &bob, "Bob work");
        skill_of(&state, Some(&alices), "deploy-the-relay");
        let his = skill_of(&state, Some(&bobs), "bobs-notes");

        let r = update(
            State(state.clone()),
            bob_dev,
            HeaderMap::new(),
            Path(his.clone()),
            Json(SkillBody {
                slug: Some("deploy-the-relay".into()),
                ..SkillBody::default()
            }),
        )
        .await;
        assert_eq!(status(r), StatusCode::OK);
        assert_eq!(
            state.db.skill_by_id(&his).unwrap().unwrap().slug,
            "deploy-the-relay"
        );
    }

    /// Nothing above widens the by-id routes: a device asking for another user's skill gets
    /// 404 whether it names the id or the slug, which is what they answered before. The slug
    /// arm is the one worth pinning — it is the arm that now resolves within a scope, and a
    /// scope lookup that fell back to the relay would turn these four 404s into answers.
    #[tokio::test]
    async fn the_by_id_routes_still_answer_404_for_another_users_skill() {
        let state = test_state(AuthMode::Github);
        let (alice, _alice_dev) = person(&state, 1);
        let (_bob, bob_dev) = person(&state, 2);
        let alices = project_of(&state, &alice, "Alice work");
        let hers = skill_with(
            &state,
            Some(&alices),
            "deploy-the-relay",
            false,
            ALICES_BODY,
        );

        for name in [hers.as_str(), "deploy-the-relay"] {
            let read = get(
                State(state.clone()),
                bob_dev.clone(),
                HeaderMap::new(),
                Path(name.to_owned()),
            )
            .await;
            assert_eq!(status(read), StatusCode::NOT_FOUND, "GET by {name}");

            let edited = update(
                State(state.clone()),
                bob_dev.clone(),
                HeaderMap::new(),
                Path(name.to_owned()),
                Json(SkillBody {
                    name: Some("renamed by a stranger".into()),
                    ..SkillBody::default()
                }),
            )
            .await;
            assert_eq!(status(edited), StatusCode::NOT_FOUND, "PUT by {name}");

            let dropped =
                delete(State(state.clone()), bob_dev.clone(), Path(name.to_owned())).await;
            assert_eq!(status(dropped), StatusCode::NOT_FOUND, "DELETE by {name}");
        }
        let row = state.db.skill_by_id(&hers).unwrap().unwrap();
        assert_eq!(
            (row.name.as_str(), row.body.as_str()),
            ("deploy-the-relay", ALICES_BODY)
        );
    }

    /// `arbosctl skills show|delete <slug>` hands a slug to a by-id route, and a slug now names
    /// one skill per owner. The caller's own answers; a stranger's never does, even when the
    /// stranger took the name first.
    #[tokio::test]
    async fn a_slug_on_a_by_id_route_resolves_to_the_callers_own_skill() {
        let state = test_state(AuthMode::Github);
        let (alice, _alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let alices = project_of(&state, &alice, "Alice work");
        let bobs = project_of(&state, &bob, "Bob work");
        skill_with(
            &state,
            Some(&alices),
            "deploy-the-relay",
            false,
            ALICES_BODY,
        );
        create_slug(
            &state,
            bob_dev.clone(),
            HeaderMap::new(),
            Some(&bobs),
            "deploy-the-relay",
        )
        .await
        .expect("Bob's own");

        let text = body_text(
            get(
                State(state.clone()),
                bob_dev.clone(),
                HeaderMap::new(),
                Path("deploy-the-relay".to_owned()),
            )
            .await,
        )
        .await;
        let his: Value = serde_json::from_str(&text).expect("a skill");
        assert_eq!(
            his["origin_project_id"].as_str(),
            Some(bobs.as_str()),
            "his own, not the row that took the name first"
        );
        assert!(!text.contains(ALICES_BODY));
    }

    /// The compatibility rule the shipped clients rest on: a slug the caller can *already see*
    /// stays taken. No listing can come to hold one slug twice through a create, so the Mac
    /// editor's slug-keyed selection (`arbos-mac/src/ui/skills.rs`) and the agent's
    /// `skills_read(slug)` keep picking the row they always picked.
    ///
    /// Each arm is a 409 before the fix and a 409 after it; the point is that the narrowing
    /// above did not reach them. The ownerless arm is the sharpest: those rows sit in every
    /// device's listing and have no owner to be unique within, so they stay reserved for
    /// everybody until the table records an author.
    #[tokio::test]
    async fn a_slug_the_caller_can_already_see_is_still_taken() {
        let state = test_state(AuthMode::Github);
        let (alice, _alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let alices = project_of(&state, &alice, "Alice work");
        let bobs = project_of(&state, &bob, "Bob work");
        let bobs_other = project_of(&state, &bob, "Bob side");
        skill_of(&state, Some(&bobs_other), "his-own-elsewhere");
        skill_of(&state, None, "nobodys-notes");
        skill_of(&state, Some(&alices), "hers-he-is-invited-to");
        state
            .db
            .add_project_member(&alices, &bob, &alice)
            .expect("invite");

        for slug in [
            "his-own-elsewhere",
            "nobodys-notes",
            "hers-he-is-invited-to",
        ] {
            let r = create_slug(&state, bob_dev.clone(), HeaderMap::new(), Some(&bobs), slug).await;
            assert_eq!(status(r), StatusCode::CONFLICT, "{slug} is already in view");
        }
    }

    /// The other half of that rule, from the owner's side: a guest filing into the host's
    /// Project cannot take a name the host already holds somewhere the guest cannot see.
    /// Uniqueness is per owner, not per caller, or one person's own library could come to
    /// hold one slug twice by someone else's hand.
    #[tokio::test]
    async fn a_guest_cannot_take_a_slug_the_hosts_other_project_holds() {
        let state = test_state(AuthMode::Github);
        let (alice, _alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let shared_with_bob = project_of(&state, &alice, "Alice shared");
        let alices_private = project_of(&state, &alice, "Alice private");
        skill_of(&state, Some(&alices_private), "deploy-the-relay");
        state
            .db
            .add_project_member(&shared_with_bob, &bob, &alice)
            .expect("invite");

        assert!(
            !listed(&state, bob_dev.clone(), HeaderMap::new())
                .await
                .contains(&"deploy-the-relay".to_owned()),
            "the guest cannot see the skill he is about to collide with"
        );
        let r = create_slug(
            &state,
            bob_dev,
            HeaderMap::new(),
            Some(&shared_with_bob),
            "deploy-the-relay",
        )
        .await;
        assert_eq!(status(r), StatusCode::CONFLICT);
    }

    /// The rebuild, run against a database that still carries the relay-wide `UNIQUE`. It is
    /// why this fix cannot be handler-only: a column-level `UNIQUE` is an implicit index that
    /// SQLite will not let you drop, so until the table is rebuilt a second owner's row fails
    /// at the database — as the first assertion here shows. Afterwards every row has survived,
    /// two owners may hold one slug, and one Project still may not hold it twice.
    #[test]
    fn the_rebuild_drops_the_relay_wide_unique_and_keeps_every_row() {
        const OLD_COLS: &str = "id, slug, name, description, tags, when_to_use, body, origin_project_id, shared, version, created_at, updated_at";
        let dir = std::env::temp_dir().join(format!("arbos-skills-migration-{}", ulid()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("relay.db");
        {
            let c = Connection::open(&path).expect("open");
            c.execute_batch(&format!(
                "CREATE TABLE skills (
                   id TEXT PRIMARY KEY, slug TEXT NOT NULL UNIQUE, name TEXT NOT NULL,
                   description TEXT NOT NULL DEFAULT '', tags TEXT NOT NULL DEFAULT '[]',
                   when_to_use TEXT NOT NULL DEFAULT '', body TEXT NOT NULL DEFAULT '',
                   origin_project_id TEXT, shared INTEGER NOT NULL DEFAULT 0,
                   version INTEGER NOT NULL DEFAULT 1,
                   created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
                 );
                 INSERT INTO skills({OLD_COLS}) VALUES
                   ('sk_hers', 'deploy-the-relay', 'Deploy the relay', 'how she does it',
                    '[\"ops\"]', 'before a release', 'the steps', 'p_alice', 1, 3, 10, 20);"
            ))
            .expect("a database on the old schema");
            assert!(
                c.execute(
                    &format!(
                        "INSERT INTO skills({OLD_COLS}) VALUES
                           ('sk_his', 'deploy-the-relay', 'Deploy the relay', '', '[]', '', '',
                            'p_bob', 0, 1, 10, 20)"
                    ),
                    [],
                )
                .is_err(),
                "the old database refuses a second owner the name, which is the finding"
            );
        }

        let db = Db::open(&path).expect("the migration runs on open");
        let hers = db.skill_by_id("sk_hers").expect("read").expect("kept");
        assert_eq!(hers.slug, "deploy-the-relay");
        assert_eq!(hers.name, "Deploy the relay");
        assert_eq!(hers.body, "the steps");
        assert_eq!(hers.tags, vec!["ops".to_owned()]);
        assert_eq!(hers.when_to_use, "before a release");
        assert_eq!(hers.origin_project_id.as_deref(), Some("p_alice"));
        assert!(hers.shared);
        assert_eq!(
            (hers.version, hers.created_at, hers.updated_at),
            (3, 10, 20)
        );

        let his = Skill {
            id: "sk_his".into(),
            slug: "deploy-the-relay".into(),
            name: "Deploy the relay".into(),
            origin_project_id: Some("p_bob".into()),
            version: 1,
            created_at: 30,
            updated_at: 30,
            ..Skill::default()
        };
        db.insert_skill(&his).expect("a second owner takes it");
        assert!(
            db.insert_skill(&Skill {
                id: "sk_again".into(),
                ..his
            })
            .is_err(),
            "and one Project still cannot hold one slug twice"
        );
    }

    /// `skills_save` is an upsert by slug, and the fear with any scoping change is that it
    /// starts landing on a different row. It does not: the row a box may write is still the
    /// one in its own Project, chosen by `can_edit`, and a stranger holding the same slug —
    /// which only became possible above — changes neither the row nor its version.
    ///
    /// This one cannot run before the fix: its fixture needs two rows to share a slug, which
    /// the old relay-wide `UNIQUE` refused at the database.
    #[tokio::test]
    async fn a_boxs_upsert_still_lands_on_its_own_projects_skill() {
        let state = test_state(AuthMode::Github);
        let (alice, _alice_dev) = person(&state, 1);
        let (bob, _bob_dev) = person(&state, 2);
        let alices = project_of(&state, &alice, "Alice work");
        let bobs = project_of(&state, &bob, "Bob work");
        let hers = skill_with(&state, Some(&alices), "weekly-report", false, ALICES_BODY);
        let his = skill_with(&state, Some(&bobs), "weekly-report", false, "his own steps");

        let (bobs_box, headers) = box_in(&bob, &bobs);
        let r = create(
            State(state.clone()),
            bobs_box,
            headers,
            Json(SkillBody {
                name: Some("Weekly report".into()),
                slug: Some("weekly-report".into()),
                body: Some("the steps, corrected".into()),
                ..SkillBody::default()
            }),
        )
        .await;
        assert_eq!(status(r), StatusCode::OK, "an upsert, not a create");

        let updated = state.db.skill_by_id(&his).unwrap().unwrap();
        assert_eq!(updated.body, "the steps, corrected");
        assert_eq!(updated.version, 2);
        let untouched = state.db.skill_by_id(&hers).unwrap().unwrap();
        assert_eq!(untouched.body, ALICES_BODY);
        assert_eq!(untouched.version, 1);
    }
}
