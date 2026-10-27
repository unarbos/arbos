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

#[cfg(test)]
mod relaunch_tests {
    //! Jacob, 2026-10-26 (Mac v0.6.268): after relaunching, every surface drew the agent's
    //! old picture — the leaf placeholder — because the cold launch's first picture fetch
    //! failed (network not up yet) and the URL was then marked done for the session, and
    //! because nothing persisted the pictures the last run had. The relaunch, headless:
    //! save the registry and the fetched pictures to a store → drop it (quit) → open it
    //! again (relaunch) → every surface (header / sidebar record, status row, bubbles,
    //! members list, hero, Secrets picker) resolves to the newest key AND its bytes are
    //! there before any request leaves; a changed picture is a new key with no cached bytes
    //! (a fetch), and the old key is pruned.

    use super::*;
    use crate::model::{AgentMember, Project, SpaceKind};
    use crate::sync::relay::picture_urls;
    use crate::sync::store::{Store, KEY_AVATAR_PREFIX, KEY_PROJECTS};
    use crate::ui::avatars::{agent_identity, member_identity};

    fn record(url: Option<&str>, version: i64) -> Project {
        Project {
            id: "01HEZ".into(),
            name: "Hezbola".into(),
            avatar_leaf: Some(4),
            avatar_leaf_color: Some(1),
            image_url: url.map(str::to_owned),
            avatar_version: version,
            ..Default::default()
        }
    }

    fn group(p: &Project) -> Project {
        Project {
            id: "chat_group".into(),
            name: "Launch week".into(),
            kind: SpaceKind::Group,
            agents: vec![AgentMember {
                project_id: p.id.clone(),
                name: p.name.clone(),
                handle: Project::mention_slug(&p.name),
                avatar_leaf: p.avatar_leaf,
                avatar_leaf_color: p.avatar_leaf_color,
                image_url: p.image_url.clone(),
                avatar_version: p.avatar_version,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    /// What each surface asks for, as a URL: the header / sidebar / Secrets picker read the
    /// record (through `newest`), the status row resolves by id, the bubble by the row's
    /// sender against the open chat's roster, the members list from the roster copy.
    fn surface_urls(projects: &[Project]) -> Vec<(&'static str, Option<String>)> {
        let chat = projects.iter().find(|p| p.id == "chat_group");
        let record = newest(projects, "01HEZ").and_then(|f| f.image_url);
        let status = agent_identity(None, projects, "01HEZ", None, Some("Hezbola")).image_key;
        let bubble = agent_identity(
            chat,
            projects,
            "agent:01HEZ",
            Some("hezbola"),
            Some("Hezbola"),
        )
        .image_key;
        let member = chat
            .and_then(|c| c.agents.first())
            .map(|a| member_identity(chat, projects, a).image_key)
            .unwrap_or(None);
        // The activity row of a turn the box has not attributed yet (`sender: "agent"`,
        // Jacob's "Reading 500" leaf): in a one-agent chat it is that agent.
        let activity = agent_identity(chat, projects, "agent", None, None).image_key;
        vec![
            ("header / sidebar / hero / secrets picker", record),
            ("status row", status),
            ("activity row (unattributed live turn)", activity),
            ("bubble", bubble),
            ("members list", member),
        ]
    }

    fn save(store: &Store, projects: &[Project], pictures: &[(&str, &[u8])]) {
        store.put_json(KEY_PROJECTS, &projects.to_vec());
        for (url, bytes) in pictures {
            store.put(&format!("{KEY_AVATAR_PREFIX}{url}"), bytes);
        }
        // What `save_registry` does with pictures no URL names any more.
        let wanted: Vec<String> = picture_urls(projects);
        for key in store.keys_with_prefix(KEY_AVATAR_PREFIX) {
            if !wanted.contains(&key[KEY_AVATAR_PREFIX.len()..].to_string()) {
                store.remove(&key);
            }
        }
    }

    #[test]
    fn relaunch_paints_the_newest_picture_on_every_surface_from_the_store() {
        let dir = std::env::temp_dir().join(format!(
            "arbos-avatar-relaunch-{}-{}",
            std::process::id(),
            crate::model::now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("store.sqlite");

        let url_a =
            picture_url("https://relay.example", "01HEZ", Some("avatar-a.jpg"), 100).unwrap();
        let p = record(Some(&url_a), 100);
        let projects = vec![p.clone(), group(&p)];

        // Run 1: the registry is saved and the picture fetched.
        {
            let store = Store::open_at(&path).unwrap();
            save(&store, &projects, &[(&url_a, b"JPEG-A")]);
        } // quit

        // Run 2 (relaunch): before any request, every surface has the newest key and its bytes.
        {
            let store = Store::open_at(&path).unwrap();
            let restored: Vec<Project> = store.get_json(KEY_PROJECTS).unwrap();
            // The roster copies kept their picture through the disk (the wire struct used to
            // drop it).
            assert_eq!(
                restored[1].agents[0].image_url.as_deref(),
                Some(url_a.as_str()),
                "roster copy survives the store"
            );
            assert_eq!(restored[1].agents[0].avatar_version, 100);
            for (surface, url) in surface_urls(&restored) {
                let url = url.unwrap_or_else(|| panic!("{surface}: no picture URL"));
                assert_eq!(url, url_a, "{surface}");
                assert_eq!(
                    store.get(&format!("{KEY_AVATAR_PREFIX}{url}")).as_deref(),
                    Some(&b"JPEG-A"[..]),
                    "{surface}: cached bytes paint at first content"
                );
            }
            // Only the URLs the registry draws are cached, and the store answers every one.
            let urls = picture_urls(&restored);
            assert_eq!(urls, vec![url_a.clone()]);

            // The owner changes the picture while this run is up: the newest copy wins on
            // every surface, its key is new (so it is fetched, never served stale), and the
            // old key is pruned when the registry is saved.
            let url_b =
                picture_url("https://relay.example", "01HEZ", Some("avatar-b.jpg"), 200).unwrap();
            let mut live = restored.clone();
            apply(
                &mut live,
                "01HEZ",
                &Facts {
                    image_url: Some(url_b.clone()),
                    leaf: Some(4),
                    leaf_color: Some(1),
                    version: 200,
                },
                false,
            );
            for (surface, url) in surface_urls(&live) {
                assert_eq!(url.as_deref(), Some(url_b.as_str()), "{surface}");
            }
            assert!(
                store.get(&format!("{KEY_AVATAR_PREFIX}{url_b}")).is_none(),
                "a new key has no cached bytes: it is fetched"
            );
            save(&store, &live, &[(&url_b, b"JPEG-B")]);
            assert!(
                store.get(&format!("{KEY_AVATAR_PREFIX}{url_a}")).is_none(),
                "the old key is pruned"
            );
        } // quit

        // Run 3 (relaunch after the change): B everywhere, from the store.
        {
            let store = Store::open_at(&path).unwrap();
            let restored: Vec<Project> = store.get_json(KEY_PROJECTS).unwrap();
            let url_b =
                picture_url("https://relay.example", "01HEZ", Some("avatar-b.jpg"), 200).unwrap();
            for (surface, url) in surface_urls(&restored) {
                assert_eq!(url.as_deref(), Some(url_b.as_str()), "{surface}");
                assert_eq!(
                    store
                        .get(&format!("{KEY_AVATAR_PREFIX}{}", url.unwrap()))
                        .as_deref(),
                    Some(&b"JPEG-B"[..]),
                    "{surface}"
                );
            }
            // A pre-#1813 store (projects without the stamp, no cached pictures): the fields
            // default, the URLs are still what the surfaces ask for, and nothing is served
            // from a cache that does not exist — the fetch (with retries now) brings them.
            let mut legacy_json = serde_json::to_value(vec![record(
                Some("https://relay.example/projects/01HEZ/avatar.jpg?v=0"),
                0,
            )])
            .unwrap();
            legacy_json[0]
                .as_object_mut()
                .unwrap()
                .remove("avatar_version");
            let legacy: Vec<Project> = serde_json::from_value(legacy_json).unwrap();
            assert_eq!(legacy[0].avatar_version, 0);
            assert_eq!(
                newest(&legacy, "01HEZ")
                    .and_then(|f| f.image_url)
                    .as_deref(),
                Some("https://relay.example/projects/01HEZ/avatar.jpg?v=0")
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
