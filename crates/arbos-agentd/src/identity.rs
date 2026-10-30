//! The agent's live identity (#1431): its display name, `@handle` and rename history, read from
//! the relay's current project record and refreshed on every registry poll and `project_renamed`
//! push — never from the box-start config. Before this, `cfg.project_name` (the name at boot) fed
//! the system prompt's identity block, the per-turn owner note, the group prelude and the
//! quick-reply prompt, while the member list, the `@handle` and the bubble attribution already
//! carried the new name: a renamed agent was told two names and kept the old one ("I'm Home, not
//! @gigchadagent" — GigChadAgent, Arbonites group, 2026-10-23).
//!
//! The history is kept in `<state>/identity.json` so a box that boots after a rename still knows
//! what it used to be called: older memories, notes and transcript rows that use the old name are
//! about this agent, and the prompt says so in one line.

use std::path::PathBuf;
use std::sync::RwLock;

use arbos_core::ids::now_ms;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use serde_json::Value;

/// How many past names the prompt line and the file keep.
const HISTORY_MAX: usize = 6;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Rename {
    pub from: String,
    pub to: String,
    pub at_ms: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Identity {
    pub name: String,
    #[serde(default)]
    pub renames: Vec<Rename>,
}

impl Identity {
    /// Past names, newest first, without the current one and without duplicates.
    pub fn former_names(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for r in self.renames.iter().rev() {
            let n = r.from.trim();
            if n.is_empty() || n.eq_ignore_ascii_case(self.name.trim()) {
                continue;
            }
            if !out.iter().any(|x| x.eq_ignore_ascii_case(n)) {
                out.push(n.to_owned());
            }
        }
        out
    }
}

/// `@handle` of an agent in shared chats: the shared rule in `arbos_core::avatar::identity`
/// (lower-case, whitespace → `-`), the same one the relay roster and both clients run.
pub fn agent_slug(name: &str) -> String {
    arbos_core::avatar::identity::agent_slug(name)
}

pub struct IdentityState {
    inner: RwLock<Identity>,
    path: PathBuf,
}

impl IdentityState {
    /// Boot: the relay's current name wins; the file supplies the history. A name that changed
    /// while the box was down is recorded as a rename at boot.
    pub fn load(cfg: &Config) -> Self {
        let path = cfg.state_dir.join("identity.json");
        let mut id = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Identity>(&s).ok())
            .unwrap_or_default();
        let live = cfg.project_name.trim();
        if id.name.trim().is_empty() {
            id.name = live.to_owned();
        } else if id.name.trim() != live {
            id.renames.push(Rename {
                from: id.name.clone(),
                to: live.to_owned(),
                at_ms: now_ms(),
            });
            id.name = live.to_owned();
        }
        trim_history(&mut id);
        let st = Self {
            inner: RwLock::new(id),
            path,
        };
        st.persist();
        st
    }

    pub fn name(&self) -> String {
        self.inner.read().expect("identity").name.clone()
    }

    pub fn handle(&self) -> String {
        agent_slug(&self.name())
    }

    pub fn snapshot(&self) -> Identity {
        self.inner.read().expect("identity").clone()
    }

    /// The relay says the project is called `live` now. Records the rename and returns it when
    /// the name changed; `None` when it is the same (the common case, every 3 s poll).
    pub fn observe(&self, live: &str) -> Option<Rename> {
        let live = live.trim();
        if live.is_empty() {
            return None;
        }
        let rename = {
            let mut id = self.inner.write().expect("identity");
            if id.name.trim() == live {
                return None;
            }
            let r = Rename {
                from: id.name.clone(),
                to: live.to_owned(),
                at_ms: now_ms(),
            };
            id.renames.push(r.clone());
            id.name = live.to_owned();
            trim_history(&mut id);
            r
        };
        self.persist();
        Some(rename)
    }

    fn persist(&self) {
        let id = self.inner.read().expect("identity").clone();
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(s) = serde_json::to_string_pretty(&id) {
            let _ = std::fs::write(&self.path, s);
        }
    }
}

fn trim_history(id: &mut Identity) {
    if id.renames.len() > HISTORY_MAX {
        let drop = id.renames.len() - HISTORY_MAX;
        id.renames.drain(..drop);
    }
}

/// The "This project" block of the system prompt. The user sees this agent under the project's
/// name (the sidebar row "Doug" is the agent), so the model is told that name is its own — not
/// a place to work "in" — plus its `@handle` in shared chats and its project id, and that
/// "Arbos" is the app, never the user. A renamed agent is told its former names in one line so
/// older notes and messages stay attributable without the agent answering to the old name.
pub fn project_section(
    id: &Identity,
    project_id: &str,
    work_dir: &str,
    talking_model: &str,
) -> String {
    let name = id.name.trim();
    let identity = if name.is_empty() || name.eq_ignore_ascii_case("arbos") {
        "You are this user's agent in Arbos.".to_owned()
    } else {
        format!(
            "You are {name}, this user's agent in Arbos; the user sees you as \"{name}\" and, in shared chats, as @{handle}. \"{name}\" is your own name — not a place, a workspace or a product — so never say \"in {name}\" or \"on {name}\". When someone asks you to look yourself up (\"search yourself\", \"search them online\" right after you were named, \"what does the web say about you\"), the subject is you, an Arbos agent named {name}: search for \"{name}\" together with \"Arbos agent\" and @{handle}, say what the web has about you or plainly that it has nothing, and treat a name that merely looks or sounds like yours as unrelated — never report it as you.",
            handle = agent_slug(name)
        )
    };
    let former = id.former_names();
    let former_line = if former.is_empty() {
        String::new()
    } else {
        let list = former
            .iter()
            .map(|n| format!("\"{n}\""))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            " You used to be called {list}: older notes, memories and messages that use that name are about you, but your name now is \"{name}\" and you never introduce yourself by a former name."
        )
    };
    format!(
        "\n\n## This project\n\n{identity}{former_line} Your project id is `{project_id}`. Arbos is the app you run in; it is never the user's name — call the user by the name given with their message. Your folder is `{work_dir}`: every file you create or read for this project goes there (relative paths resolve there; do not use `/work` itself, other folders under `/work` belong to other projects). You run on model `{talking_model}`.\n"
    )
}

/// One owner, named live (Hezbola 2026-10-26: "the chat uses two names for the owner"). The
/// owner is a user id; the name is the relay profile's current one; former display names are
/// listed so older rows and notes that use them read as the same person, not a second owner.
pub fn owner_section(owner: &OwnerProfile) -> String {
    let Some(id) = owner.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return String::new();
    };
    let name = owner
        .address_name
        .as_deref()
        .or(owner.label_name.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let who = match name {
        Some(n) => format!("You have exactly one owner: {n} (user id `{id}`)."),
        None => format!(
            "You have exactly one owner (user id `{id}`); their display name is not set, so use no name for them."
        ),
    };
    let former: Vec<&str> = owner
        .former_names
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && Some(*s) != name)
        .collect();
    let former_line = if former.is_empty() {
        String::new()
    } else {
        format!(
            " Older messages, notes and memories may show them as {}: the same person, not another owner.",
            former
                .iter()
                .map(|n| format!("\"{n}\""))
                .collect::<Vec<_>>()
                .join(" or ")
        )
    };
    format!(
        "\n## Your owner\n\n{who} Their name comes from their profile and can change; the name on this message is the current one.{former_line} People are identified by user id, never by name or handle.\n"
    )
}

/// The owner's profile as the relay's box listing carries it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OwnerProfile {
    pub id: Option<String>,
    pub address_name: Option<String>,
    pub label_name: Option<String>,
    pub former_names: Vec<String>,
}

impl OwnerProfile {
    pub fn from_json(owner: &Value) -> Self {
        let pick = |k: &str| {
            owner
                .get(k)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        Self {
            id: pick("id"),
            address_name: pick("address_name"),
            label_name: pick("name"),
            former_names: owner
                .get("former_names")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with(name: &str, dir: &std::path::Path) -> Config {
        let bx = crate::config::BoxConfig {
            user_id: Some("u-test".into()),
            relay_url: "http://127.0.0.1:1".into(),
            box_token: String::new(),
            mcp_token: String::new(),
            work_root: dir.to_path_buf(),
            bind: "127.0.0.1".into(),
            port: 0,
            local_box: true,
            pi_bin: String::new(),
            prime_bin: String::new(),
            claude_bin: String::new(),
            pi_ext_override: None,
            device_id: "box:test".into(),
            default_talking_model: "m".into(),
            default_subagent_model: "m".into(),
            legacy_project_id: None,
        };
        bx.workspace(&serde_json::json!({ "id": "01MPROJ", "name": name }), 0)
    }

    /// GigChadAgent, 2026-10-23: the prompt must carry the live name, the handle, the id and the
    /// former name, and never present the former name as the current one.
    #[test]
    fn project_section_carries_live_identity_and_former_names() {
        let id = Identity {
            name: "GigChadAgent".into(),
            renames: vec![Rename {
                from: "Home".into(),
                to: "GigChadAgent".into(),
                at_ms: 1,
            }],
        };
        let s = project_section(
            &id,
            "01M5862G",
            "/work/01M5862G",
            "anthropic/claude-haiku-5.5",
        );
        assert!(s.starts_with("\n\n## This project\n\n"));
        assert!(s.contains("You are GigChadAgent, this user's agent in Arbos"));
        assert!(s.contains("as @gigchadagent"));
        assert!(s.contains("You used to be called \"Home\""));
        assert!(s.contains("your name now is \"GigChadAgent\""));
        assert!(s.contains("Your project id is `01M5862G`"));
        assert!(!s.contains("You are Home"));
        assert!(s.contains("You run on model `anthropic/claude-haiku-5.5`."));
        // Never renamed: no former-name line at all.
        let plain = Identity {
            name: "Doug".into(),
            renames: vec![],
        };
        let s = project_section(&plain, "01HPROJ", "/work/01HPROJ", "m");
        assert!(s.contains("You are Doug, this user's agent in Arbos"));
        assert!(s.contains("never say \"in Doug\" or \"on Doug\""));
        assert!(!s.contains("used to be called"));
        // Hezbola, 2026-10-25: "Search them online" after being named made the agent search the
        // bare word and report Hezbollah pages as the answer. The agent knows who it is.
        assert!(s.contains("the subject is you, an Arbos agent named Doug"));
        assert!(s.contains("search for \"Doug\" together with \"Arbos agent\" and @doug"));
        assert!(s.contains("never report it as you"));
        // An agent literally called Arbos (or unnamed) gets no self line that contradicts the app.
        let arbos = Identity {
            name: "Arbos".into(),
            renames: vec![],
        };
        let s = project_section(&arbos, "p", "/work/p", "m");
        assert!(s.contains("You are this user's agent in Arbos."));
        assert!(!s.contains("never say \"in Arbos\""));
    }

    /// Hezbola 2026-10-26: one owner, by id, current name, former names named as the same person.
    #[test]
    fn owner_section_names_one_owner_with_the_current_name() {
        let o = OwnerProfile::from_json(&serde_json::json!({
            "id": "gh_268993031", "login": "unarbos", "name": "Jacob", "address_name": "Jacob", "former_names": ["Arbos"]
        }));
        let s = owner_section(&o);
        assert!(s.contains("You have exactly one owner: Jacob (user id `gh_268993031`)."));
        assert!(s.contains("may show them as \"Arbos\": the same person, not another owner"));
        assert!(s.contains("identified by user id, never by name or handle"));
        assert!(!s.contains("unarbos"), "the login is never a name");
        let unnamed = OwnerProfile::from_json(&serde_json::json!({ "id": "gh_1", "login": "x" }));
        assert!(owner_section(&unnamed).contains("display name is not set, so use no name"));
        assert_eq!(owner_section(&OwnerProfile::default()), "");
    }

    #[test]
    fn former_names_skip_the_current_name_and_duplicates() {
        let id = Identity {
            name: "C".into(),
            renames: vec![
                Rename {
                    from: "A".into(),
                    to: "B".into(),
                    at_ms: 1,
                },
                Rename {
                    from: "B".into(),
                    to: "A".into(),
                    at_ms: 2,
                },
                Rename {
                    from: "A".into(),
                    to: "C".into(),
                    at_ms: 3,
                },
                Rename {
                    from: "C".into(),
                    to: "C ".into(),
                    at_ms: 4,
                },
            ],
        };
        assert_eq!(id.former_names(), vec!["A".to_owned(), "B".to_owned()]);
    }

    #[test]
    fn slug_matches_the_relay_rule() {
        assert_eq!(agent_slug("GigChadAgent"), "gigchadagent");
        assert_eq!(agent_slug("  Rail  Layout Bot "), "rail-layout-bot");
    }

    /// Observing the relay's name every poll is a no-op until it changes; a change is recorded,
    /// persisted and survives a reboot that already sees the new name.
    #[test]
    fn observe_records_and_persists_renames() {
        let dir = std::env::temp_dir().join(format!("arbos-identity-{}", now_ms()));
        let st = IdentityState::load(&cfg_with("Home", &dir));
        assert_eq!(st.name(), "Home");
        assert_eq!(st.handle(), "home");
        assert!(st.observe("Home").is_none());
        assert!(st.observe("  ").is_none());
        let r = st.observe("GigChadAgent").expect("rename");
        assert_eq!((r.from.as_str(), r.to.as_str()), ("Home", "GigChadAgent"));
        assert_eq!(st.name(), "GigChadAgent");
        assert_eq!(st.handle(), "gigchadagent");
        // Reboot with the new name already in the relay record: history kept, nothing added.
        let again = IdentityState::load(&cfg_with("GigChadAgent", &dir));
        assert_eq!(again.snapshot().former_names(), vec!["Home".to_owned()]);
        assert_eq!(again.snapshot().renames.len(), 1);
        // Reboot after a rename that happened while the box was down: recorded at boot.
        let third = IdentityState::load(&cfg_with("Chad", &dir));
        assert_eq!(third.name(), "Chad");
        assert_eq!(
            third.snapshot().former_names(),
            vec!["GigChadAgent".to_owned(), "Home".to_owned()]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
