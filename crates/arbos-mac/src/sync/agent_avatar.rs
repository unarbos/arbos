//! One copy of an agent's avatar everywhere (Jacob, 2026-10-25: "sometimes it shows its old
//! pfp, sometimes the new one switches in"; a foreign agent drew the automatic leaf).
//!
//! The store holds an agent's card in several places: the owner's `Project` record and an
//! `AgentMember` copy on every people chat the agent sits on. A change used to move one copy
//! (the one the change went through) and leave the rest; the resolver reads the open chat's
//! roster first, so the bubble drew the old picture while the sidebar drew the new one, and a
//! later roster refresh flipped them. Every write now goes through [`apply`], which lands the
//! same facts — picture URL, leaf picks, the relay's `avatar_version` stamp — in every copy
//! and never rolls a copy back to an older stamp; [`reconcile`] runs after every fetch so the
//! newest copy wins whatever order the answers arrived in.
//!
//! The picture URL is the relay's public route `/projects/{id}/avatar.jpg?v={version}` —
//! readable by anyone who sees the agent (the `/blob/{project}/…` slot is the owner's and the
//! guests' only, which is why a group member got 403 and the leaf), and a change is a new URL,
//! so the avatar cache (keyed by URL) never serves the old bytes.

use crate::model::Project;

/// What an agent's avatar is made of, plus the relay's stamp of its last change.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub image_url: Option<String>,
    pub leaf: Option<u8>,
    pub leaf_color: Option<u8>,
    pub version: i64,
}

/// The picture URL every surface fetches an agent's picture from; `None` without a picture.
pub fn picture_url(
    http_url: &str,
    project_id: &str,
    key: Option<&str>,
    version: i64,
) -> Option<String> {
    key.map(str::trim).filter(|k| !k.is_empty())?;
    if http_url.is_empty() {
        return None;
    }
    Some(format!(
        "{http_url}/projects/{project_id}/avatar.jpg?v={version}"
    ))
}

/// Every copy of the agent the store holds, the owner's record first.
pub fn copies(projects: &[Project], project_id: &str) -> Vec<Facts> {
    let mut out: Vec<Facts> = projects
        .iter()
        .filter(|p| !p.is_people() && p.id == project_id)
        .map(|p| Facts {
            image_url: p.image_url.clone(),
            leaf: p.avatar_leaf,
            leaf_color: p.avatar_leaf_color,
            version: p.avatar_version,
        })
        .collect();
    for chat in projects.iter().filter(|p| p.is_people()) {
        for a in chat.agents.iter().filter(|a| a.project_id == project_id) {
            out.push(Facts {
                image_url: a.image_url.clone(),
                leaf: a.avatar_leaf,
                leaf_color: a.avatar_leaf_color,
                version: a.avatar_version,
            });
        }
    }
    out
}

/// The newest copy (highest stamp; the owner's record among equals).
pub fn newest(projects: &[Project], project_id: &str) -> Option<Facts> {
    copies(projects, project_id)
        .into_iter()
        .fold(None, |best: Option<Facts>, c| match best {
            Some(b) if b.version >= c.version => Some(b),
            _ => Some(c),
        })
}

/// Write `facts` into every copy of the agent. A copy that already carries a newer stamp is
/// left alone unless `force` (the optimistic local write before the relay has stamped it).
pub fn apply(projects: &mut [Project], project_id: &str, facts: &Facts, force: bool) {
    for p in projects.iter_mut() {
        if !p.is_people() && p.id == project_id && (force || facts.version >= p.avatar_version) {
            p.image_url = facts.image_url.clone();
            p.avatar_leaf = facts.leaf;
            p.avatar_leaf_color = facts.leaf_color;
            p.avatar_version = facts.version;
        }
        if p.is_people() {
            for a in p.agents.iter_mut() {
                if a.project_id == project_id && (force || facts.version >= a.avatar_version) {
                    a.image_url = facts.image_url.clone();
                    a.avatar_leaf = facts.leaf;
                    a.avatar_leaf_color = facts.leaf_color;
                    a.avatar_version = facts.version;
                }
            }
        }
    }
}

/// After a fetch replaced copies: per agent, the newest copy wins everywhere.
pub fn reconcile(projects: &mut [Project]) {
    let mut ids: Vec<String> = projects
        .iter()
        .filter(|p| !p.is_people())
        .map(|p| p.id.clone())
        .collect();
    for chat in projects.iter().filter(|p| p.is_people()) {
        ids.extend(chat.agents.iter().map(|a| a.project_id.clone()));
    }
    ids.sort();
    ids.dedup();
    for id in ids {
        if let Some(best) = newest(projects, &id) {
            apply(projects, &id, &best, false);
        }
    }
}

/// The relay's `agent_profile_updated` push: the picture and the stamp land in place (the leaf
/// picks are kept until the re-read that follows); an older push changes nothing.
pub fn apply_push(
    projects: &mut [Project],
    http_url: &str,
    project_id: &str,
    image_key: Option<&str>,
    version: i64,
) {
    let base = newest(projects, project_id).unwrap_or_default();
    if version < base.version {
        return;
    }
    let facts = Facts {
        image_url: picture_url(http_url, project_id, image_key, version),
        leaf: base.leaf,
        leaf_color: base.leaf_color,
        version,
    };
    apply(projects, project_id, &facts, false);
}

/// The stamp a `PATCH /projects/{id}` answered with (`avatar_version`); 0 from an older relay.
pub fn stamp_of(reply: &serde_json::Value) -> i64 {
    reply
        .get("avatar_version")
        .or_else(|| reply.get("project").and_then(|p| p.get("avatar_version")))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    //! The three chat surfaces — bubble (open chat's roster first), presence row (by id) and
    //! header / sidebar (my record) — resolve to the new picture after a change, a stale roster
    //! cannot flip them back, a push lands in place, and the URL is the public versioned route.

    use super::*;
    use crate::model::{AgentMember, Project, SpaceKind};
    use crate::ui::avatars::{agent_cards, agent_identity, AgentIdentity};

    fn mine(id: &str, name: &str, url: Option<&str>, version: i64) -> Project {
        Project {
            id: id.into(),
            name: name.into(),
            avatar_leaf: Some(4),
            avatar_leaf_color: Some(1),
            image_url: url.map(str::to_owned),
            avatar_version: version,
            ..Default::default()
        }
    }

    fn roster_copy(p: &Project) -> AgentMember {
        AgentMember {
            project_id: p.id.clone(),
            name: p.name.clone(),
            handle: Project::mention_slug(&p.name),
            owner_handle: "unarbos".into(),
            avatar_leaf: p.avatar_leaf,
            avatar_leaf_color: p.avatar_leaf_color,
            image_url: p.image_url.clone(),
            avatar_version: p.avatar_version,
            ..Default::default()
        }
    }

    fn group(agents: Vec<AgentMember>) -> Project {
        Project {
            id: "chat_group".into(),
            name: "Launch week".into(),
            kind: SpaceKind::Group,
            agents,
            ..Default::default()
        }
    }

    /// (header, bubble, presence) as the surfaces ask.
    fn surfaces(projects: &[Project], id: &str) -> (Option<String>, AgentIdentity, AgentIdentity) {
        let chat = projects.iter().find(|p| p.id == "chat_group");
        let header = projects
            .iter()
            .find(|p| !p.is_people() && p.id == id)
            .and_then(|p| p.image_url.clone());
        let bubble = agent_identity(
            chat,
            projects,
            &format!("agent:{id}"),
            Some("hezbola"),
            Some("Hezbola"),
        );
        let presence = agent_identity(None, projects, id, None, Some("Hezbola"));
        (header, bubble, presence)
    }

    #[test]
    fn a_change_moves_every_surface_and_a_stale_roster_cannot_flip_it_back() {
        let old_url = picture_url("https://relay.example", "01HEZ", Some("avatar-1.jpg"), 100);
        let old = mine("01HEZ", "Hezbola", old_url.as_deref(), 100);
        let mut projects = vec![old.clone(), group(vec![roster_copy(&old)])];
        let (h, b, p) = surfaces(&projects, "01HEZ");
        assert_eq!(h, old_url);
        assert_eq!(b.image_key, old_url);
        assert_eq!(p.image_key, old_url);

        // The owner sets a new picture (relay stamp 200): every copy, every surface.
        let new_url = picture_url("https://relay.example", "01HEZ", Some("avatar-2.jpg"), 200);
        apply(
            &mut projects,
            "01HEZ",
            &Facts {
                image_url: new_url.clone(),
                leaf: Some(4),
                leaf_color: Some(1),
                version: 200,
            },
            true,
        );
        let (h, b, p) = surfaces(&projects, "01HEZ");
        assert_eq!(h, new_url, "header / sidebar");
        assert_eq!(b.image_key, new_url, "bubble (roster first)");
        assert_eq!(p.image_key, new_url, "presence row");
        assert_eq!(b.avatar_version, 200);
        assert_eq!(p.avatar_version, 200);

        // A stale roster arrives (a fetch that started before the change) and is reconciled:
        // the newest copy wins — the bubble, which reads the roster first, keeps the new one.
        projects[1] = group(vec![roster_copy(&old)]);
        reconcile(&mut projects);
        let (h, b, p) = surfaces(&projects, "01HEZ");
        assert_eq!(h, new_url);
        assert_eq!(b.image_key, new_url, "never flips back");
        assert_eq!(p.image_key, new_url);
        assert_eq!(
            projects[1].agents[0].image_url, new_url,
            "the roster copy was brought up"
        );

        // Reset to the leaf (stamp 300): the picture is gone everywhere, the pick is drawn.
        apply(
            &mut projects,
            "01HEZ",
            &Facts {
                image_url: None,
                leaf: Some(2),
                leaf_color: Some(3),
                version: 300,
            },
            false,
        );
        let (h, b, p) = surfaces(&projects, "01HEZ");
        assert_eq!(h, None);
        assert_eq!(b.image_key, None);
        assert_eq!(p.image_key, None);
        assert_eq!((b.leaf.leaf, b.leaf.color), (2, 3));
        assert_eq!((p.leaf.leaf, p.leaf.color), (2, 3));

        // An older write (a late answer) cannot roll a copy back.
        apply(
            &mut projects,
            "01HEZ",
            &Facts {
                image_url: new_url.clone(),
                leaf: Some(4),
                leaf_color: Some(1),
                version: 200,
            },
            false,
        );
        assert_eq!(surfaces(&projects, "01HEZ").0, None);
    }

    #[test]
    fn a_push_lands_in_place_and_reaches_a_foreign_agent_known_only_from_a_roster() {
        let mine_p = mine("01HEZ", "Hezbola", None, 0);
        let foreign = Project {
            id: "01FOR".into(),
            name: "Foreign".into(),
            avatar_leaf: Some(7),
            avatar_leaf_color: Some(2),
            ..Default::default()
        };
        let mut projects = vec![
            mine_p.clone(),
            group(vec![roster_copy(&mine_p), roster_copy(&foreign)]),
        ];

        apply_push(
            &mut projects,
            "https://relay.example",
            "01HEZ",
            Some("avatar-b.jpg"),
            200,
        );
        let want = picture_url("https://relay.example", "01HEZ", Some("avatar-b.jpg"), 200);
        let (h, b, p) = surfaces(&projects, "01HEZ");
        assert_eq!(h, want);
        assert_eq!(b.image_key, want);
        assert_eq!(p.image_key, want);
        assert_eq!(
            (b.leaf.leaf, b.leaf.color),
            (4, 1),
            "leaf picks kept until the re-read"
        );

        // Older push: ignored.
        apply_push(
            &mut projects,
            "https://relay.example",
            "01HEZ",
            Some("avatar-a.jpg"),
            150,
        );
        assert_eq!(surfaces(&projects, "01HEZ").1.image_key, want);

        // Someone else's agent, on the roster only (Jacob's "Hezbola" shot was this case):
        // the owner's change reaches the copy I hold, and its URL is one I may read.
        apply_push(
            &mut projects,
            "https://relay.example",
            "01FOR",
            Some("avatar-f.jpg"),
            300,
        );
        let chat = projects.iter().find(|p| p.id == "chat_group");
        let bubble = agent_identity(
            chat,
            &projects,
            "agent:01FOR",
            Some("foreign"),
            Some("Foreign"),
        );
        let url = bubble.image_key.expect("picture");
        assert_eq!(url, "https://relay.example/projects/01FOR/avatar.jpg?v=300");
        assert!(!url.contains("/blob/"));
        assert!(!url.contains("token="));
        assert_eq!(bubble.avatar_version, 300);
        // The cards every surface resolves against now agree on the foreign agent.
        let cards = agent_cards(chat, &projects);
        let copies: Vec<_> = cards.iter().filter(|c| c.id == "01FOR").collect();
        assert!(!copies.is_empty());
        assert!(copies
            .iter()
            .all(|c| c.image_key.as_deref() == Some(url.as_str())));
    }

    #[test]
    fn url_rule_and_stamp_parsing() {
        assert_eq!(
            picture_url(
                "https://relay.example",
                "01HEZ",
                Some("avatar-1.jpg"),
                1_700_000_000_000
            ),
            Some("https://relay.example/projects/01HEZ/avatar.jpg?v=1700000000000".into())
        );
        assert_eq!(picture_url("https://relay.example", "01HEZ", None, 1), None);
        assert_eq!(
            picture_url("https://relay.example", "01HEZ", Some("  "), 1),
            None
        );
        assert_eq!(picture_url("", "01HEZ", Some("avatar-1.jpg"), 1), None);
        assert_ne!(
            picture_url("https://relay.example", "01HEZ", Some("a.jpg"), 1),
            picture_url("https://relay.example", "01HEZ", Some("b.jpg"), 2),
            "a change is a new URL"
        );
        assert_eq!(stamp_of(&serde_json::json!({ "avatar_version": 42 })), 42);
        assert_eq!(
            stamp_of(&serde_json::json!({ "project": { "avatar_version": 7 } })),
            7
        );
        assert_eq!(stamp_of(&serde_json::json!({ "name": "x" })), 0);
    }
}
