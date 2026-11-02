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
        state.db.insert_skill(&shared).expect("skill");

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
}
