//! Plain Rust view of the Project doc and the Chat docs (`arbos-design.md` §5,
//! `crdt-sync.md` §6). No GPUI, no Loro: the sync layer fills these in, the
//! views only read them.

use std::collections::HashSet;

use std::sync::Arc;

use serde::{Deserialize, Serialize};

pub use arbos_core::doc::ReplyRef;
pub use arbos_core::widget::{ChoiceOption, FieldKind, FormField, QuestionState, Widget};

pub type Id = String;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoxState {
    #[default]
    Awake,
    Waking,
    HibernatedLight,
    HibernatedDeep,
    StoppedByUser,
    Error,
}

impl BoxState {
    pub fn is_asleep(self) -> bool {
        match self {
            BoxState::HibernatedLight | BoxState::HibernatedDeep | BoxState::StoppedByUser => true,
            BoxState::Awake | BoxState::Waking | BoxState::Error => false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: Id,
    pub name: String,
    /// One glyph, drawn in the rail tile and the pinned tab.
    pub icon: String,
    /// Hex `#rrggbb` accent for the selected tile border.
    pub color: String,
    pub state: BoxState,
    /// Human text for the S12/S13 banner, e.g. "Restoring from archive…".
    pub state_detail: Option<String>,
    /// 0.0–1.0 while `state == Waking`.
    pub wake_progress: Option<f32>,
    pub model: String,
    pub effort: Effort,
    pub spend_today_usd: f64,
    pub spend_cap_usd: f64,
    /// A project of mine, or a people chat (DM / group) that shares the rail.
    #[serde(default)]
    pub kind: SpaceKind,
    /// People chat: the other members. Project: invited members (D72 step 3).
    #[serde(default)]
    pub people: Vec<Person>,
    /// People chat: shared agents.
    #[serde(default)]
    pub agents: Vec<AgentMember>,
    /// People chat with someone who has not signed in yet.
    #[serde(default)]
    pub pending: bool,
    /// Project shared with me by its owner (`shared: true`).
    #[serde(default)]
    pub shared: bool,
    #[serde(default)]
    pub owner_handle: Option<String>,
    /// Tree-character slug (`PATCH /projects/{id} {avatar}`); `None` = hash pick.
    #[serde(default)]
    pub avatar: Option<String>,
    /// Avatar colour id from the manifest (`PATCH /projects/{id} {avatar_color}`);
    /// `None` = mono.
    #[serde(default)]
    pub avatar_color: Option<String>,
    /// API §9: the agent's one-paragraph description (≤ 500 chars).
    #[serde(default)]
    pub description: Option<String>,
    /// API §9: the owner with presence (`owner: sender{}`), when the relay sends it.
    #[serde(default)]
    pub owner_person: Option<Person>,
    /// API §9: `machine {state, class, snapshot, always_on, …}` of the agent's box.
    #[serde(default)]
    pub machine_info: Option<ProjectMachine>,
    /// A connected (outside) agent (#2141): its brain runs elsewhere, the relay hosts the
    /// member. `GET /projects/{id}` carries this beside the DTO; `None` = a native agent.
    #[serde(default)]
    pub outside: Option<OutsideInfo>,
    /// People chats: my `me.muted` on the relay (synced across devices).
    #[serde(default)]
    pub muted: bool,
    /// People group: an admin muted me from posting (`me.silenced`).
    #[serde(default)]
    pub silenced: bool,
    /// When my admin mute ends (ms). `None` while silenced = until unmuted.
    #[serde(default)]
    pub silenced_until: Option<i64>,
    /// Sidebar archive (2026-10-23): hidden from the list for me, on every
    /// device (the relay's per-user flag); the agent keeps running.
    #[serde(default)]
    pub archived_at: Option<u64>,
    /// The agent's one chat, §9b `primary_chat_id` (stored on the relay, never
    /// flips); older relays and the fixture send none and the client picks:
    /// most messages, ties → earliest created.
    #[serde(default)]
    pub primary_chat: Option<Id>,
    /// Call voice (`GET /voices` id); `None` = the current default voice.
    #[serde(default)]
    pub voice: Option<String>,
    /// Voice API §11: the agent may look through my camera on calls (default on).
    #[serde(default = "default_true")]
    pub vision_enabled: bool,
    /// D79: the avatar gradient in effect (0–6): the owner's pick, else the
    /// id-derived one (`avatar_color_index_effective` from the relay).
    #[serde(default)]
    pub avatar_color_index: Option<u8>,
    /// D79: the owner's explicit pick (`avatar_color_index`), `None` = automatic.
    #[serde(default)]
    pub avatar_color_override: Option<u8>,
    /// D92: the owner's leaf pick (`avatar_leaf`, 0–8), `None` = automatic.
    #[serde(default)]
    pub avatar_leaf: Option<u8>,
    /// D92: the owner's leaf disc colour pick (`avatar_leaf_color`, 0–9), `None` = automatic.
    #[serde(default)]
    pub avatar_leaf_color: Option<u8>,
    /// A group's custom picture (`PATCH /chats/{id} {image_key}`), as a URL
    /// the avatar loader can fetch; `None` = the initials disc.
    #[serde(default)]
    pub image_url: Option<String>,
    /// The relay's stamp (ms) of the agent's last avatar change (`ProjectDto.avatar_version`);
    /// `0` when unknown. Among the copies of one agent (this record, every roster) the highest
    /// stamp is drawn, so a change never flips back (Jacob, 2026-10-25).
    #[serde(default)]
    pub avatar_version: i64,
    /// "Who may message X" (agent-to-agent messaging): other owners' people / agents the
    /// owner allowed to message this agent. Empty unless I own it.
    #[serde(default)]
    pub message_grants: Vec<MessageGrant>,
    /// Members I blocked, as `user:<id>` — the relay's `blockedSenders` hint on a people
    /// chat ([feedback blocked-state-mac-2026-10-26]): their rows are hidden for me alone.
    #[serde(default)]
    pub blocked_senders: Vec<String>,
    /// People chat: my membership role on the relay (`me.role`: `owner`, `admin` or
    /// `member`); `None` when the relay did not say (fixture, older cache).
    #[serde(default)]
    pub my_role: Option<String>,
}

/// One row of an agent's "Who may message X" card (`message_grants` on the project).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MessageGrant {
    /// `user:<id>` / `project:<id>`: the key for PUT / DELETE.
    #[serde(default)]
    pub grantee: String,
    /// `person` or `agent`.
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub handle: Option<String>,
    #[serde(default, deserialize_with = "null_to_empty")]
    pub avatar_url: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub last_at: Option<i64>,
}

/// One entry of `GET /voices`: a voice the agent can speak with on calls.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Voice {
    #[serde(default)]
    pub id: String,
    #[serde(default, deserialize_with = "null_to_empty")]
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "null_to_empty")]
    pub sample_url: String,
}

/// The connection of an outside agent, as `GET /outside-agents/{id}` / the project detail
/// report it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OutsideInfo {
    /// `connected` | `away` | `never` | `revoked`.
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub connected: bool,
    /// `acp` | `grok` | `custom`.
    #[serde(default)]
    pub transport: String,
    /// `asp/1`.
    #[serde(default)]
    pub protocol: String,
    #[serde(default)]
    pub last_seen_at: Option<i64>,
    #[serde(default)]
    pub connected_at: Option<i64>,
    /// The nine switches (spec §4.3), effective values: `"files.read" → true`.
    #[serde(default)]
    pub scopes: std::collections::HashMap<String, bool>,
    /// The Wake URL (`agent.wake`), absent on a relay without it.
    #[serde(default)]
    pub wake: Option<WakeInfo>,
}

/// An outside agent's Wake URL as the relay reports it: the URL is shown back, the
/// `Authorization` value never is (`auth_set` only).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WakeInfo {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub url_set: bool,
    #[serde(default)]
    pub auth_set: bool,
    /// `Authorization`.
    #[serde(default)]
    pub header: String,
    #[serde(default)]
    pub last: Option<WakeLast>,
}

/// The last nudge the relay sent to the Wake URL.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WakeLast {
    #[serde(default)]
    pub at: i64,
    #[serde(default)]
    pub status: Option<u16>,
    #[serde(default)]
    pub error: Option<String>,
}

impl WakeLast {
    /// "Delivered · HTTP 200 · 12 s ago" / "Failed · HTTP 500: boom · 3 min ago".
    pub fn line(&self, now: i64) -> String {
        let when = ago_seconds(now, self.at);
        match (&self.error, self.status) {
            (None, Some(s)) => format!("Delivered · HTTP {s} · {when}"),
            (None, None) => format!("Delivered · {when}"),
            (Some(e), _) => format!("Failed · {e} · {when}"),
        }
    }
}

/// "12 s ago", "3 min ago", "2 h ago", "4 d ago" — the connection lines' clock, with seconds
/// (unlike [`ago`], which starts at "just now").
fn ago_seconds(now: i64, t: i64) -> String {
    let s = ((now - t).max(0) / 1000).max(0);
    if s < 60 {
        format!("{s} s ago")
    } else if s < 3600 {
        format!("{} min ago", s / 60)
    } else if s < 86_400 {
        format!("{} h ago", s / 3600)
    } else {
        format!("{} d ago", s / 86_400)
    }
}

/// The switches of the Can card, in the mockup's order: wire name, title, subtitle.
pub const OUTSIDE_SCOPES: [(&str, &str, Option<&str>); 9] = [
    (
        "chat.read",
        "Read messages",
        Some("in chats it is a member of"),
    ),
    ("chat.post", "Post, edit and delete its own messages", None),
    ("chat.react", "React", None),
    ("presence", "Show typing and status", None),
    (
        "approval.request",
        "Ask you to approve actions",
        Some("approval cards in chat"),
    ),
    (
        "secret.use",
        "Use your secrets",
        Some("by name only, never the value"),
    ),
    ("members.manage", "Add members or agents", None),
    (
        "files.read",
        "Open files shared in chat",
        Some("pictures, files, GIFs it was sent"),
    ),
    ("calls.join", "Join voice calls", None),
];

impl OutsideInfo {
    /// A switch's value; a relay without scopes yet reads as the spec defaults.
    pub fn scope(&self, name: &str) -> bool {
        self.scopes.get(name).copied().unwrap_or(matches!(
            name,
            "chat.read" | "chat.post" | "chat.react" | "presence" | "approval.request"
        ))
    }

    /// The transport as the Details pane names it.
    pub fn transport_label(&self) -> &'static str {
        match self.transport.as_str() {
            "acp" => "ACP · on your Mac",
            "grok" => "Grok · HTTPS",
            _ => "Custom · HTTPS / WebSocket",
        }
    }

    /// "Connected · 12 s ago", "Away · 40 min ago", "Waiting for the first heartbeat",
    /// "Disconnected".
    pub fn status_line(&self, now: i64) -> String {
        let ago = |t: i64| ago_seconds(now, t);
        match (self.status.as_str(), self.last_seen_at) {
            ("revoked", _) => "Disconnected".to_owned(),
            ("connected", Some(t)) => format!("Connected · {}", ago(t)),
            ("connected", None) => "Connected".to_owned(),
            ("away", Some(t)) => format!("Away · {}", ago(t)),
            ("away", None) => "Away".to_owned(),
            _ => "Waiting for the first heartbeat".to_owned(),
        }
    }
}

/// API §9 `project.machine`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectMachine {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub since: Option<i64>,
    #[serde(default)]
    pub class: String,
    #[serde(default)]
    pub snapshot: Option<String>,
    #[serde(default)]
    pub image_outdated: bool,
    #[serde(default)]
    pub always_on: bool,
}

impl Project {
    /// A project I can write to but do not own (D72 step 3).
    pub fn is_guest(&self) -> bool {
        self.shared && !self.is_people()
    }

    /// Owner of a shared project, when the member list carries them.
    pub fn owner(&self) -> Option<&Person> {
        self.people.iter().find(|p| p.role == "owner")
    }

    /// Members other than the owner (the people invited in).
    pub fn guests(&self) -> impl Iterator<Item = &Person> {
        self.people.iter().filter(|p| p.role != "owner")
    }

    pub fn is_people(&self) -> bool {
        matches!(self.kind, SpaceKind::Dm | SpaceKind::Group)
    }

    /// A people group chat (several humans, no project).
    pub fn is_people_group(&self) -> bool {
        self.kind == SpaceKind::Group
    }

    /// May I change this group's name, picture and description? The owner and admins
    /// only — the relay refuses everyone else (`PATCH /chats/{id}` 403 since
    /// 2026-10-27, after members renamed Arbonites), so the controls hide for them.
    /// Without a role from the relay only the owner (by handle) counts.
    pub fn may_edit_group_settings(&self, me_handle: &str) -> bool {
        if !self.is_people_group() {
            return false;
        }
        match self.my_role.as_deref() {
            Some(role) => role == "owner" || role == "admin",
            None => self.owner_handle.as_deref() == Some(me_handle),
        }
    }

    /// Shown as a group page: a people group or a project with other humans in it.
    pub fn is_groupish(&self) -> bool {
        self.is_people_group() || (!self.is_people() && (self.shared || !self.people.is_empty()))
    }

    /// The other person of a DM.
    pub fn person(&self) -> Option<&Person> {
        self.people.first()
    }

    /// `@slug` the composer resolves to this agent (project name, lower-case, `-`).
    pub fn mention_slug(name: &str) -> String {
        name.trim()
            .to_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join("-")
    }

    /// Is this person already in the space? Me (`me_handle`) counts as in:
    /// the relay lists the other members only, not the caller.
    pub fn has_person(&self, me_handle: &str, person: &Person) -> bool {
        (!me_handle.is_empty() && person.handle == me_handle)
            || self
                .people
                .iter()
                .any(|p| p.id == person.id || p.handle == person.handle)
    }

    /// Is this agent (by its project id) already in the space?
    pub fn has_agent(&self, project_id: &str) -> bool {
        self.agents.iter().any(|a| a.project_id == project_id)
    }

    /// True while an admin mute still blocks posting (honours timed `silenced_until`).
    pub fn is_silenced_now(&self, now_ms: i64) -> bool {
        if !self.silenced {
            return false;
        }
        !matches!(self.silenced_until, Some(until) if until <= now_ms)
    }
}

/// May I delete `message` for everyone in `space`? The relay's rule
/// (`arbos-relay/src/moderation.rs`), mirrored so the menu never offers a
/// Delete the relay would refuse: my own messages always; in a people group
/// the owner (and admins) delete anyone's; an agent's message is its owner's
/// to delete; in a project chat the owner deletes anything, a guest their own.
/// System lines are a moderator's only.
pub fn may_delete_message(
    space: &Project,
    me_id: &str,
    me_handle: &str,
    message: &Message,
) -> bool {
    let sender = message.sender.as_str();
    // A bare `user` row is the project owner's (a guest's rows carry `user:<id>`).
    let mine = sender == format!("user:{me_id}")
        || (sender == "user" && !space.is_people() && !space.is_guest());
    let moderates = if space.is_people() {
        space.owner_handle.as_deref().is_none_or(|o| o == me_handle)
    } else {
        !space.is_guest()
    };
    if moderates {
        return true;
    }
    if mine {
        return true;
    }
    if let Some(pid) = sender.strip_prefix("agent:") {
        return space
            .agents
            .iter()
            .find(|a| a.project_id == pid)
            .is_some_and(|a| a.owner_handle == me_handle || a.owner_id == me_id);
    }
    // Bare `agent` / `assistant` in a project chat is the project's own agent:
    // the owner's (handled above); a guest leaves it.
    false
}

/// After "Create": the people group that just appeared (`after` but not
/// `before`) with exactly the picked `handles`. Empty handles (a group of
/// agents) never match — the relay names that group by id instead.
pub fn created_group(before: &[Project], after: &[Project], handles: &[String]) -> Option<Id> {
    if handles.is_empty() {
        return None;
    }
    after
        .iter()
        .filter(|p| p.is_people_group() && !before.iter().any(|b| b.id == p.id))
        .find(|p| {
            p.people.len() == handles.len()
                && handles
                    .iter()
                    .all(|h| p.people.iter().any(|x| x.handle == *h))
        })
        .map(|p| p.id.clone())
}

/// The relay calls "Add" makes for one group: new people by handle, new
/// agents by project id, both on `chat_id`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AddMembersPlan {
    pub chat_id: Id,
    pub handles: Vec<String>,
    pub agent_ids: Vec<Id>,
}

/// Turn the picker's selection into the calls for `space`: whoever is already
/// in (people, me, agents) is dropped, so a stale pick never hits the relay's
/// `already_member` error; the target is always the group the picker opened for.
pub fn add_members_plan(
    space: &Project,
    me_handle: &str,
    picked: &[Person],
    picked_agents: &[Id],
) -> AddMembersPlan {
    AddMembersPlan {
        chat_id: space.id.clone(),
        handles: picked
            .iter()
            .filter(|p| !space.has_person(me_handle, p))
            .map(|p| p.handle.clone())
            .collect(),
        agent_ids: picked_agents
            .iter()
            .filter(|id| !space.has_agent(id))
            .cloned()
            .collect(),
    }
}

/// What a rail tile is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpaceKind {
    #[default]
    Project,
    Dm,
    Group,
}

/// An Arbos user as the relay lists them (`GET /users/search`, chat members).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "PersonWire")]
pub struct Person {
    /// `gh_<github id>`.
    #[serde(default)]
    pub id: String,
    /// GitHub login without `@`.
    #[serde(default)]
    pub handle: String,
    #[serde(default)]
    pub name: Option<String>,
    /// Empty when GitHub has none (the relay sends `null`): initials are drawn instead.
    #[serde(default, deserialize_with = "null_to_empty")]
    pub avatar_url: String,
    /// D79: the avatar gradient in effect (0–6) when the relay sends it.
    #[serde(default)]
    pub avatar_color_index: Option<u8>,
    /// `member` (has signed in) or `invited` (placeholder).
    #[serde(default)]
    pub status: String,
    /// Project members: `owner` or `member`.
    #[serde(default)]
    pub role: String,
    /// API §9: last activity (ms, minute-floored); `None` = never.
    #[serde(default)]
    pub last_seen_at: Option<i64>,
    /// API §9: seen within the last 90 s.
    #[serde(default)]
    pub online: bool,
}

#[derive(Deserialize)]
struct PersonWire {
    #[serde(default)]
    id: String,
    #[serde(default)]
    handle: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default, deserialize_with = "null_to_empty")]
    avatar_url: String,
    #[serde(default)]
    avatar_color_index: Option<u8>,
    #[serde(default)]
    avatar_color_index_effective: Option<u8>,
    #[serde(default)]
    status: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    last_seen_at: Option<i64>,
    #[serde(default)]
    online: bool,
}

impl From<PersonWire> for Person {
    fn from(w: PersonWire) -> Self {
        Self {
            id: w.id,
            handle: w.handle,
            name: w.name,
            avatar_url: w.avatar_url,
            avatar_color_index: avatar_color_in_effect(
                w.avatar_color_index,
                w.avatar_color_index_effective,
            ),
            status: w.status,
            role: w.role,
            last_seen_at: w.last_seen_at,
            online: w.online,
        }
    }
}

impl Person {
    pub fn display_name(&self) -> String {
        self.name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| self.handle.clone())
    }

    pub fn initial(&self) -> String {
        self.display_name()
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "?".into())
    }
}

// ---------------------------------------------------------------------------
// Calls v1 (user↔user voice/video over LiveKit; internal/calls-v1-contract.md)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallKind {
    #[default]
    Voice,
    Video,
}

impl CallKind {
    pub fn label(self) -> &'static str {
        match self {
            CallKind::Voice => "voice call",
            CallKind::Video => "video call",
        }
    }

    pub fn label_title(self) -> &'static str {
        match self {
            CallKind::Voice => "Voice call",
            CallKind::Video => "Video call",
        }
    }
}

/// The relay's `sender{}`: who started a call or sits in it (also the shape
/// LiveKit participant metadata carries).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "SenderWire")]
pub struct Sender {
    /// `user` or `agent`.
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub id: String,
    #[serde(default, deserialize_with = "null_to_empty")]
    pub name: String,
    #[serde(default, deserialize_with = "null_to_empty")]
    pub handle: String,
    /// Tree-character slug (agents, and users who picked one).
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub avatar_color: Option<String>,
    /// D79: the agent's avatar gradient in effect (0–6), when the relay sends it.
    #[serde(default)]
    pub avatar_color_index: Option<u8>,
    /// D92: the agent's leaf avatar in effect (shape, disc colour), when the relay sends it.
    #[serde(default)]
    pub avatar_leaf: Option<u8>,
    #[serde(default)]
    pub avatar_leaf_color: Option<u8>,
    #[serde(default, deserialize_with = "null_to_empty")]
    pub avatar_url: String,
}

#[derive(Deserialize)]
struct SenderWire {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    id: String,
    #[serde(default, deserialize_with = "null_to_empty")]
    name: String,
    #[serde(default, deserialize_with = "null_to_empty")]
    handle: String,
    #[serde(default)]
    avatar: Option<String>,
    #[serde(default)]
    avatar_color: Option<String>,
    #[serde(default)]
    avatar_color_index: Option<u8>,
    #[serde(default)]
    avatar_color_index_effective: Option<u8>,
    #[serde(default)]
    avatar_leaf_effective: Option<u8>,
    #[serde(default)]
    avatar_leaf_color_effective: Option<u8>,
    #[serde(default, deserialize_with = "null_to_empty")]
    avatar_url: String,
}

impl From<SenderWire> for Sender {
    fn from(w: SenderWire) -> Self {
        Self {
            kind: w.kind,
            id: w.id,
            name: w.name,
            handle: w.handle,
            avatar: w.avatar,
            avatar_color: w.avatar_color,
            avatar_color_index: avatar_color_in_effect(
                w.avatar_color_index,
                w.avatar_color_index_effective,
            ),
            avatar_leaf: w.avatar_leaf_effective,
            avatar_leaf_color: w.avatar_leaf_color_effective,
            avatar_url: w.avatar_url,
        }
    }
}

impl Sender {
    pub fn display_name(&self) -> String {
        if !self.name.trim().is_empty() {
            self.name.clone()
        } else if !self.handle.is_empty() {
            self.handle.clone()
        } else {
            "Someone".into()
        }
    }

    pub fn as_person(&self) -> Person {
        Person {
            id: self.id.clone(),
            handle: self.handle.clone(),
            name: Some(self.name.clone()).filter(|n| !n.trim().is_empty()),
            avatar_url: self.avatar_url.clone(),
            status: "member".into(),
            role: String::new(),
            last_seen_at: None,
            online: false,
            avatar_color_index: None,
        }
    }

    /// LiveKit identity for this sender (`u_<user_id>` / `a_<project_id>`).
    pub fn identity(&self) -> String {
        if self.kind == "agent" {
            format!("a_{}", self.id)
        } else {
            format!("u_{}", self.id)
        }
    }

    pub fn of_person(p: &Person) -> Self {
        Self {
            kind: "user".into(),
            id: p.id.clone(),
            name: p.name.clone().unwrap_or_default(),
            handle: p.handle.clone(),
            avatar: None,
            avatar_color: None,
            avatar_color_index: p.avatar_color_index,
            avatar_leaf: None,
            avatar_leaf_color: None,
            avatar_url: p.avatar_url.clone(),
        }
    }

    pub fn of_agent(a: &AgentMember) -> Self {
        Self {
            kind: "agent".into(),
            id: a.project_id.clone(),
            name: a.name.clone(),
            handle: a.handle.clone(),
            avatar: a.avatar.clone(),
            avatar_color: a.avatar_color.clone(),
            avatar_color_index: a.avatar_color_index,
            avatar_leaf: a.avatar_leaf,
            avatar_leaf_color: a.avatar_leaf_color,
            avatar_url: String::new(),
        }
    }
}

/// Who a call in this space rings (#1430): every other member of a people chat (humans and
/// agents), the other person of a DM, the owner/guests and the agent of a project space. The
/// call screen shows each of them as a "Ringing…" tile until they join, so a group call
/// never reads as "it only called the agent" — the agent joins within a second, the people
/// take longer.
pub fn call_people(space: &Project, me_id: &str, call_agents: &[Sender]) -> Vec<Sender> {
    let mut out: Vec<Sender> = Vec::new();
    for p in &space.people {
        if p.id == me_id || p.status == "invited" {
            continue;
        }
        out.push(Sender::of_person(p));
    }
    for a in &space.agents {
        out.push(Sender::of_agent(a));
    }
    if !space.is_people() {
        out.push(Sender {
            kind: "agent".into(),
            id: space.id.clone(),
            name: space.name.clone(),
            handle: Project::mention_slug(&space.name),
            avatar: space.avatar.clone(),
            avatar_color: space.avatar_color.clone(),
            avatar_color_index: space.avatar_color_index,
            avatar_leaf: space.avatar_leaf,
            avatar_leaf_color: space.avatar_leaf_color,
            avatar_url: String::new(),
        });
    }
    // The relay's `call.agents` is authoritative for which agents belong to the call.
    for a in call_agents {
        if !out.iter().any(|s| s.identity() == a.identity()) {
            out.push(a.clone());
        }
    }
    out
}

/// `expected` minus those already in the room: the tiles still ringing.
pub fn still_ringing(expected: &[Sender], in_room: &[String]) -> Vec<Sender> {
    expected
        .iter()
        .filter(|s| !in_room.iter().any(|id| *id == s.identity()))
        .cloned()
        .collect()
}

/// One call as the relay describes it (`POST …/calls`, `GET /calls/{id}`, pushes).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Call {
    #[serde(default)]
    pub call_id: String,
    #[serde(default)]
    pub chat_id: Id,
    /// `people` for DMs and groups.
    #[serde(default)]
    pub project_id: String,
    #[serde(default)]
    pub kind: CallKind,
    #[serde(default)]
    pub room: String,
    #[serde(default)]
    pub livekit_url: String,
    /// Join token for this device; absent in pushes and after the call ended.
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub token_expires_at: Option<u64>,
    #[serde(default)]
    pub started_by: Sender,
    #[serde(default)]
    pub started_at: u64,
    #[serde(default)]
    pub ended_at: Option<u64>,
    #[serde(default)]
    pub reason: Option<String>,
    /// Live participants.
    #[serde(default)]
    pub participants: Vec<Sender>,
    /// Project agents that belong to this chat (Phase 2).
    #[serde(default)]
    pub agents: Vec<Sender>,
    /// `"open"` or `"hosted"` (bigger group calls: host + invited speakers only).
    #[serde(default)]
    pub speak_mode: Option<String>,
    /// Invited speakers (host is `started_by`).
    #[serde(default)]
    pub invited_speakers: Vec<Sender>,
    /// Whether this device may publish audio (from the join token reply).
    #[serde(default)]
    pub can_speak: Option<bool>,
}

impl Call {
    pub fn is_live(&self) -> bool {
        self.ended_at.is_none()
    }

    /// The rail space this call belongs to: the chat itself for people chats, the project for
    /// project chats (Calls Phase 2: a call in an agent's chat).
    pub fn space_id(&self) -> &str {
        space_of(&self.project_id, &self.chat_id)
    }
}

/// `people` (or nothing) means the chat is its own space; else the project is.
pub fn space_of<'a>(project_id: &'a str, chat_id: &'a str) -> &'a str {
    if project_id.is_empty() || project_id == "people" {
        chat_id
    } else {
        project_id
    }
}

/// `POST /calls/{id}/leave` answer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CallLeft {
    #[serde(default)]
    pub ended: bool,
    #[serde(default)]
    pub duration_s: u64,
}

/// `/me/pushes` `call_ended` payload.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CallEnded {
    #[serde(default)]
    pub call_id: String,
    #[serde(default)]
    pub chat_id: Id,
    #[serde(default)]
    pub project_id: String,
    #[serde(default)]
    pub duration_s: u64,
    #[serde(default)]
    pub reason: String,
    /// The one device that writes the "Call ended" line.
    #[serde(default)]
    pub writer_device: Option<String>,
    /// Set by the sync layer: `writer_device` is this device.
    #[serde(skip)]
    pub writer_is_me: bool,
}

/// `/me/pushes` `call_missed` payload: a call in one of my chats ended and I
/// never joined it (contract §8).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CallMissed {
    #[serde(default)]
    pub call_id: String,
    #[serde(default)]
    pub chat_id: Id,
    #[serde(default)]
    pub project_id: String,
    #[serde(default)]
    pub kind: CallKind,
    #[serde(default)]
    pub from: Sender,
    #[serde(default)]
    pub started_at: u64,
    #[serde(default)]
    pub ended_at: Option<u64>,
    #[serde(default)]
    pub duration_s: u64,
    #[serde(default, deserialize_with = "null_to_empty")]
    pub reason: String,
}

impl CallMissed {
    /// The system line this client writes into the chat.
    pub fn line(&self) -> String {
        format!("Missed call from {}", self.from.display_name())
    }

    /// Native notification body ("Missed voice call").
    pub fn notification_body(&self) -> String {
        format!("Missed {}", self.kind.label())
    }
}

/// One of a callee's devices in the 201 start response (`reachable[].devices[]`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ReachableDevice {
    #[serde(default)]
    pub device_id: String,
    #[serde(default, deserialize_with = "null_to_empty")]
    pub name: String,
    #[serde(default, deserialize_with = "null_to_empty")]
    pub kind: String,
    #[serde(default, deserialize_with = "null_to_empty")]
    pub platform: String,
    #[serde(default, deserialize_with = "null_to_empty")]
    pub app_version: String,
    /// Its Arbos is open: it talked to the relay in the last 30 s.
    #[serde(default)]
    pub polled_within_30s: bool,
    #[serde(default)]
    pub last_seen_at: Option<u64>,
}

/// Per callee, in the 201 start response: can anyone pick up?
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Reachable {
    #[serde(default)]
    pub user_id: String,
    #[serde(default, deserialize_with = "null_to_empty")]
    pub handle: String,
    #[serde(default, deserialize_with = "null_to_empty")]
    pub name: String,
    /// Some device of theirs has Arbos open right now.
    #[serde(default)]
    pub any_live: bool,
    /// Some iPhone of theirs can be rung through PushKit.
    #[serde(default)]
    pub any_voip: bool,
    /// Some device has a regular APNs token (Incoming-call banner / wake).
    #[serde(default)]
    pub any_push: bool,
    #[serde(default)]
    pub devices: Vec<ReachableDevice>,
}

impl Reachable {
    fn first_name(&self) -> String {
        let name = if self.name.trim().is_empty() {
            self.handle.clone()
        } else {
            self.name.clone()
        };
        name.split_whitespace()
            .next()
            .filter(|n| !n.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| "They".into())
    }

    /// What the caller's status line says while this callee has not picked
    /// up; `None` when their Arbos is open (nothing to add to the ringback).
    pub fn status_line(&self) -> Option<String> {
        match (self.any_live, self.any_voip || self.any_push) {
            (true, _) => None,
            (false, true) => Some(format!("Ringing {}'s iPhone", self.first_name())),
            (false, false) => Some(format!(
                "{}'s Arbos isn't open — they'll see a missed call",
                self.first_name()
            )),
        }
    }
}

/// The call-screen status line for a set of callees (`None` when everyone is live).
pub fn reachable_line(callees: &[Reachable]) -> Option<String> {
    let lines: Vec<String> = callees.iter().filter_map(Reachable::status_line).collect();
    if lines.is_empty() {
        None
    } else if lines.len() <= 2 {
        Some(lines.join(" · "))
    } else {
        Some(format!(
            "{} · +{} more",
            lines[..2].join(" · "),
            lines.len() - 2
        ))
    }
}

/// "Call ended · 12 min" ("Call ended" under a minute).
pub fn call_ended_line(duration_s: u64) -> String {
    if duration_s < 60 {
        "Call ended".into()
    } else {
        let mins = duration_s / 60;
        if mins >= 60 {
            format!("Call ended · {} h {} min", mins / 60, mins % 60)
        } else {
            format!("Call ended · {mins} min")
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyPolicy {
    #[default]
    Mention,
    EveryMessage,
}

/// A project agent someone added to a people chat.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "AgentMemberWire")]
pub struct AgentMember {
    pub project_id: Id,
    pub name: String,
    pub icon: String,
    pub color: String,
    /// Mention slug (`@research`).
    pub handle: String,
    pub owner_id: String,
    pub owner_handle: String,
    #[serde(default)]
    pub reply_policy: ReplyPolicy,
    /// Tree-character slug of the agent's project, when the relay carries it.
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub avatar_color: Option<String>,
    /// D79: the agent's avatar gradient in effect (0–6).
    #[serde(default)]
    pub avatar_color_index: Option<u8>,
    /// D92: the agent's leaf avatar in effect (shape 0–8, disc colour 0–9).
    #[serde(default)]
    pub avatar_leaf: Option<u8>,
    #[serde(default)]
    pub avatar_leaf_color: Option<u8>,
    /// D92 amendment: the agent's uploaded / generated picture, as a URL the avatar loader
    /// fetches; wins over the leaf.
    #[serde(default)]
    pub image_url: Option<String>,
    /// The relay's stamp of the agent's last avatar change (same value as the owner's
    /// `Project.avatar_version`); `0` when the roster copy has none.
    #[serde(default)]
    pub avatar_version: i64,
    /// API §9: the agent's description and talking model.
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// API §9: `project.owner` with presence.
    #[serde(default)]
    pub owner: Option<Person>,
}

#[derive(Deserialize)]
struct AgentMemberWire {
    #[serde(default)]
    project_id: Id,
    #[serde(default)]
    name: String,
    #[serde(default)]
    icon: String,
    #[serde(default)]
    color: String,
    #[serde(default)]
    handle: String,
    #[serde(default)]
    owner_id: String,
    #[serde(default)]
    owner_handle: String,
    #[serde(default)]
    reply_policy: ReplyPolicy,
    #[serde(default)]
    avatar: Option<String>,
    #[serde(default)]
    avatar_color: Option<String>,
    #[serde(default)]
    avatar_color_index: Option<u8>,
    #[serde(default)]
    avatar_color_index_effective: Option<u8>,
    #[serde(default)]
    avatar_leaf: Option<u8>,
    #[serde(default)]
    avatar_leaf_color: Option<u8>,
    #[serde(default)]
    avatar_leaf_effective: Option<u8>,
    #[serde(default)]
    avatar_leaf_color_effective: Option<u8>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    owner: Option<Person>,
    /// The picture and its stamp round-trip through the on-disk store (`Project.agents` is
    /// persisted as JSON). Until 2026-10-26 this wire struct dropped both, so every roster
    /// copy came back from a relaunch without its picture; with the stamps equal (agents
    /// whose avatar predates #1813 carry 0) that lossy copy won the resolver and every
    /// people-chat surface drew the leaf until the next registry read.
    #[serde(default)]
    image_url: Option<String>,
    #[serde(default)]
    avatar_version: i64,
}

impl From<AgentMemberWire> for AgentMember {
    fn from(w: AgentMemberWire) -> Self {
        Self {
            project_id: w.project_id,
            name: w.name,
            icon: w.icon,
            color: w.color,
            handle: w.handle,
            owner_id: w.owner_id,
            owner_handle: w.owner_handle,
            reply_policy: w.reply_policy,
            avatar: w.avatar,
            avatar_color: w.avatar_color,
            avatar_color_index: avatar_color_in_effect(
                w.avatar_color_index,
                w.avatar_color_index_effective,
            ),
            avatar_leaf: w.avatar_leaf_effective.or(w.avatar_leaf),
            avatar_leaf_color: w.avatar_leaf_color_effective.or(w.avatar_leaf_color),
            image_url: w.image_url,
            avatar_version: w.avatar_version,
            description: w.description,
            model: w.model,
            owner: w.owner,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    Low,
    #[default]
    Medium,
    High,
}

impl Effort {
    pub const ALL: [Effort; 3] = [Effort::Low, Effort::Medium, Effort::High];

    pub fn label(self) -> &'static str {
        match self {
            Effort::Low => "Low",
            Effort::Medium => "Medium",
            Effort::High => "High",
        }
    }
}

/// What a chat tab is (registry `kind`): a plain chat, a group of Projects (F6), or the S15
/// system tab that collects group traffic on a participant Project.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatKind {
    #[default]
    Chat,
    Group,
    Projects,
    /// A background job's tab (`jobtabs`): client-side, never on the relay.
    Job,
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Chat {
    pub id: Id,
    pub project_id: Id,
    pub title: String,
    pub is_pinned: bool,
    /// S15 system tab "Projects ⇄".
    pub is_system: bool,
    pub unread: bool,
    /// Unread message count from `GET /me/unread` (Dock badge). `0` when read or unknown.
    /// Live arrivals may set [`Self::unread`] with this still `0` until the next refresh;
    /// the badge treats that as one missed message.
    #[serde(default)]
    pub unread_count: u32,
    pub last_message_at: u64,
    /// §9c: archived for me (per user, synced by the relay); `None` = not archived.
    #[serde(default)]
    pub archived_at: Option<u64>,
    #[serde(default)]
    pub kind: ChatKind,
    /// Group chats: participant project ids, host first.
    #[serde(default)]
    pub participants: Vec<Id>,
}

impl Chat {
    pub fn is_group(&self) -> bool {
        self.kind == ChatKind::Group
    }

    /// Missed messages for the Dock badge: the relay count, or 1 when only the unread flag is lit.
    pub fn missed_count(&self) -> usize {
        if self.unread_count > 0 {
            self.unread_count as usize
        } else if self.unread {
            1
        } else {
            0
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
    System,
    Project,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageStatus {
    Queued,
    Streaming,
    Done,
    Error,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnKind {
    Reply,
    Act,
    /// Hand-off ack: a new subagent was started.
    New,
    /// Progress message of a subagent (status events only).
    Spawn,
    Wake,
    /// Prose written by the main agent about a finished subagent.
    SubagentResult,
    #[serde(other)]
    Other,
}

/// `turn` is written by the box as `{ "kind": "reply" }` and by older
/// fixtures as a bare string; accept both.
/// `null`, a missing key and `""` all read as an empty string (JSON from the
/// relay carries `"avatar_url": null` for people without a picture).
fn default_true() -> bool {
    true
}

pub fn null_to_empty<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(d)?.unwrap_or_default())
}

/// Prefer `avatar_color_index_effective` when the relay sends both that and
/// `avatar_color_index`. Serde's `alias` rejects the pair as a duplicate field,
/// which made every `POST …/calls` answer fail to parse (`The relay answered
/// without a call.`).
fn avatar_color_in_effect(override_idx: Option<u8>, effective: Option<u8>) -> Option<u8> {
    effective.or(override_idx)
}

fn de_turn<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<TurnKind>, D::Error> {
    let v = Option::<serde_json::Value>::deserialize(d)?;
    Ok(match v {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => {
            serde_json::from_value(serde_json::Value::String(s)).ok()
        }
        Some(serde_json::Value::Object(map)) => map
            .get("kind")
            .cloned()
            .and_then(|k| serde_json::from_value(k).ok()),
        Some(_) => None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Running,
    Ok,
    Failed,
}

/// `call` object of a tool part, as `arbos_core::doc::ChatDoc::add_tool_part` writes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: Id,
    pub name: String,
    #[serde(default)]
    pub input: serde_json::Value,
}

impl ToolCall {
    /// `input_line` cut to ~60 chars for dense lists.
    pub fn call_input_short(&self) -> String {
        let line = self.input_line();
        let mut out: String = line.chars().take(60).collect();
        if line.chars().count() > 60 {
            out.push('…');
        }
        out
    }

    /// One-line input for the card.
    pub fn input_line(&self) -> String {
        match &self.input {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Null => String::new(),
            serde_json::Value::Object(map) => map
                .iter()
                .map(|(k, v)| match v {
                    serde_json::Value::String(s) => format!("{k}={s}"),
                    other => format!("{k}={other}"),
                })
                .collect::<Vec<_>>()
                .join(" "),
            other => other.to_string(),
        }
    }
}

/// What an attachment is, for the card's icon and layout (F10). Unknown strings decode as `File`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    Image,
    Video,
    Slides,
    Doc,
    Sheet,
    Pdf,
    Audio,
    /// A picker sticker (ARB-544): drawn at a fixed size with no bubble.
    Sticker,
    #[serde(other)]
    File,
}

impl AttachmentKind {
    pub fn label(self) -> &'static str {
        match self {
            AttachmentKind::Image => "Image",
            AttachmentKind::Video => "Video",
            AttachmentKind::Slides => "Slides",
            AttachmentKind::Doc => "Document",
            AttachmentKind::Sheet => "Spreadsheet",
            AttachmentKind::Pdf => "PDF",
            AttachmentKind::Audio => "Audio",
            AttachmentKind::Sticker => "Sticker",
            AttachmentKind::File => "File",
        }
    }

    fn from_name_and_mime(name: &str, mime: &str) -> Self {
        let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        match ext.as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "heic" => {
                return AttachmentKind::Image
            }
            "mp4" | "mov" | "webm" | "mkv" => return AttachmentKind::Video,
            "pptx" | "ppt" | "key" | "odp" => return AttachmentKind::Slides,
            "docx" | "doc" | "odt" | "rtf" | "md" | "txt" => return AttachmentKind::Doc,
            "xlsx" | "xls" | "csv" | "ods" => return AttachmentKind::Sheet,
            "pdf" => return AttachmentKind::Pdf,
            "mp3" | "wav" | "m4a" | "ogg" | "flac" => return AttachmentKind::Audio,
            _ => {}
        }
        if mime.starts_with("image/") {
            AttachmentKind::Image
        } else if mime.starts_with("video/") {
            AttachmentKind::Video
        } else if mime.starts_with("audio/") {
            AttachmentKind::Audio
        } else if mime == "application/pdf" {
            AttachmentKind::Pdf
        } else {
            AttachmentKind::File
        }
    }
}

/// Where a GIF / sticker came from. Written on the attachment as
/// `{ provider, id, pageUrl }`; old messages and pastes have none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentSource {
    pub provider: String,
    pub id: String,
    pub page_url: String,
}

/// One GIF the relay's search returned (`GET /gif/search` item): the grid shows `preview`,
/// the message carries `url` + `source`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GifItem {
    pub id: String,
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub url: String,
    pub preview: GifPreview,
    pub page_url: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GifPreview {
    pub url: String,
    pub width: u32,
    pub height: u32,
}

/// A page of GIF results, or why there is none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GifPage {
    Items {
        items: Vec<GifItem>,
        next: Option<String>,
    },
    /// The relay has no provider key (`503 gif_unavailable`).
    Unavailable,
    /// Our budget or the provider's rate limit (`429 gif_busy`).
    Busy,
    /// Anything else went wrong.
    Failed,
}

/// One sticker pack of the relay's manifest (`GET /stickers/packs`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StickerPack {
    pub id: String,
    pub name: String,
    pub publisher: String,
    pub license: String,
    pub license_url: String,
    pub attribution: String,
    pub animated: bool,
    pub page_url: String,
    pub mime: String,
    pub items: Vec<StickerItem>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StickerItem {
    pub id: String,
    pub name: String,
    pub emoji: String,
    pub mime: String,
    pub path: String,
}

/// What the picker sends with one tap (ARB-544): a sticker (bytes already fetched, stored in
/// the chat slot like any picture) or a GIF (provider URL, nothing stored).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaPick {
    Sticker {
        pack: Box<StickerPack>,
        item: StickerItem,
        bytes: Arc<Vec<u8>>,
    },
    Gif(Box<GifItem>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub name: String,
    pub mime: String,
    #[serde(rename = "ref", default)]
    pub blob_ref: Option<String>,
    /// Size on the relay, when the producer knew it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<AttachmentKind>,
    /// Caption for the card; the file name when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The fallback picture of a chart bubble: names the `chart` part of the same message.
    /// A client that draws the chart hides this attachment; an older one shows the picture
    /// ([feedback chart-bubble-mac-2026-10-26]).
    #[serde(default, rename = "chartPart", skip_serializing_if = "Option::is_none")]
    pub chart_part: Option<String>,
    /// A picture that lives with its provider (a picked GIF, ARB-544): no `ref`, the bytes
    /// come through the relay's pass-through (`GET /gif/media?u=`). See [`Self::preview_key`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// GIF-picker attribution; when set the bubble shows "via \<provider\>".
    /// Boxed so [`Attachment`] stays small inside enums (Outgoing, library Entry).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Box<AttachmentSource>>,
}

/// The thumbnail-cache key prefix for a provider-hosted picture (`url:<https://…>`); the
/// sync layer fetches these through the relay's GIF pass-through instead of `/blob`.
pub const URL_KEY_PREFIX: &str = "url:";
/// The thumbnail-cache key prefix for a relay-hosted sticker (`sticker:<pack>/<id>`).
pub const STICKER_KEY_PREFIX: &str = "sticker:";

impl Attachment {
    pub fn kind(&self) -> AttachmentKind {
        self.kind
            .unwrap_or_else(|| AttachmentKind::from_name_and_mime(&self.name, &self.mime))
    }

    /// The key the picture's bytes are cached and fetched under: the blob `ref`, or
    /// `url:<url>` for a provider-hosted GIF with no copy in the chat slot.
    pub fn preview_key(&self) -> Option<String> {
        self.blob_ref.clone().or_else(|| {
            self.url
                .as_ref()
                .filter(|u| !u.is_empty())
                .map(|u| format!("{URL_KEY_PREFIX}{u}"))
        })
    }

    /// A picker sticker: fixed square, no bubble, no caption.
    pub fn is_sticker(&self) -> bool {
        self.kind() == AttachmentKind::Sticker
    }

    pub fn title(&self) -> &str {
        self.title
            .as_deref()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or(&self.name)
    }

    /// "1.2 MB" style size, empty when unknown.
    pub fn size_label(&self) -> String {
        match self.bytes {
            None => String::new(),
            Some(b) if b < 1024 => format!("{b} B"),
            Some(b) if b < 1024 * 1024 => format!("{:.0} KB", b as f64 / 1024.0),
            Some(b) if b < 1024 * 1024 * 1024 => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
            Some(b) => format!("{:.2} GB", b as f64 / (1024.0 * 1024.0 * 1024.0)),
        }
    }

    /// A voice memo (`kind: "audio"`, F11).
    #[allow(dead_code)]
    pub fn is_audio(&self) -> bool {
        self.kind() == AttachmentKind::Audio || self.mime.starts_with("audio/")
    }

    /// An image that should render as a picture in the transcript, not a file name.
    pub fn is_image(&self) -> bool {
        self.kind() == AttachmentKind::Image || self.mime.starts_with("image/")
    }

    /// The blob is still on its way up from this device (`local:` key): nothing
    /// anyone can fetch from the relay yet.
    pub fn is_uploading(&self) -> bool {
        self.blob_ref
            .as_deref()
            .is_some_and(|k| k.starts_with("local:"))
    }

    /// The bytes can be fetched from the relay (`GET /blob/{slot}/{key}`).
    pub fn is_on_relay(&self) -> bool {
        self.blob_ref
            .as_deref()
            .is_some_and(|k| !k.is_empty() && !k.starts_with("local:"))
    }
}

/// One row of `messages[].parts` (`crdt-sync.md` §6.2). Unknown kinds decode
/// to [`Part::Unknown`] and render as nothing, per the "additive only" rule.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Part {
    Text {
        id: Id,
        #[serde(default)]
        text: String,
    },
    Reasoning {
        id: Id,
        #[serde(default)]
        reasoning: String,
    },
    Tool {
        id: Id,
        call: ToolCall,
        /// ≤ 160 chars (`crdt-sync.md` §6.2).
        #[serde(default)]
        output_summary: String,
        #[serde(default)]
        output_ref: Option<String>,
        #[serde(default)]
        is_error: bool,
        /// Client-visible progress (S17). `None` means finished.
        #[serde(default)]
        status: Option<ToolStatus>,
        #[serde(default)]
        duration_ms: Option<u64>,
        /// Set when this call spawned a subagent; such a call never folds (S7).
        #[serde(default)]
        subagent_name: Option<String>,
    },
    Attachment {
        id: Id,
        attachment: Attachment,
    },
    Error {
        id: Id,
        code: String,
        message: String,
    },
    /// S4 "Arbos is deciding…", S6 "Working in sandbox…", S8 question, S12 wake note.
    Status {
        id: Id,
        code: StatusKind,
        #[serde(default)]
        message: String,
    },
    /// Collapsed diff card.
    Diff {
        id: Id,
        path: String,
        added: u32,
        removed: u32,
        blob_ref: Option<String>,
    },
    /// Collapsed file card.
    File {
        id: Id,
        path: String,
        lines: u32,
        blob_ref: Option<String>,
    },
    /// Raw subagent output; always `hidden: true` (Details drawer only).
    SubagentResult {
        id: Id,
        #[serde(default)]
        text: String,
        #[serde(default)]
        subagent_id: Option<String>,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        return_contract: Option<serde_json::Value>,
        #[serde(default)]
        duration_ms: Option<u64>,
    },
    /// S16 browser card.
    Browser {
        id: Id,
        title: String,
        url: String,
        last_action: String,
        live: bool,
    },
    /// `ask_user` with an optional typed widget (design §10). `widget == None`
    /// is a plain question answered from the composer (S8).
    Question {
        id: Id,
        question_id: String,
        #[serde(default)]
        question: String,
        #[serde(default, deserialize_with = "de_widget")]
        widget: Option<Widget>,
        #[serde(default)]
        answer: Option<serde_json::Value>,
        #[serde(default = "open_state")]
        state: QuestionState,
        #[serde(default)]
        answered_at: Option<u64>,
    },
    /// F5: the "Relevant skills" section agentd injected into this turn; always `hidden: true`.
    Skills {
        id: Id,
        #[serde(default)]
        text: String,
        #[serde(default)]
        slugs: Vec<String>,
    },
    /// S18 draft-and-approve card (feature `approve`; shape owned by `arbos_core::approval`).
    Approval {
        id: Id,
        approval_id: String,
        #[serde(default)]
        title: String,
        #[serde(default)]
        summary: String,
        #[serde(default)]
        risk: ApprovalRisk,
        #[serde(default)]
        payload: serde_json::Value,
        #[serde(default)]
        state: ApprovalState,
        #[serde(default)]
        expires_at: Option<u64>,
        #[serde(default)]
        decided_at: Option<u64>,
        #[serde(default)]
        decided_by: Option<String>,
        #[serde(default)]
        reason: Option<String>,
        /// #1544: the standing choice made on a destructive-shell card (a path class).
        #[serde(default)]
        grant: Option<String>,
    },
    /// The row a message becomes when its value moved into the vault (secret hand-off):
    /// "Secret saved as NAME". Names only — there is no value field.
    SecretRedacted {
        id: Id,
        #[serde(default)]
        name: String,
        #[serde(default)]
        env: String,
        #[serde(default)]
        secret_id: String,
        /// `user:<id>` whose message held the value.
        #[serde(default)]
        saved_by: String,
        /// Project id of the agent that saved it.
        #[serde(default)]
        agent: Option<String>,
    },
    /// Chart bubble ([feedback chart-bubble-mac-2026-10-26]): the structured chart
    /// `post_chart` posted. `chart` is the validated `arbos_core::chart::Chart` as JSON,
    /// `table` its markdown table, `fallback` the PNG attachment part this client hides.
    Chart {
        id: Id,
        #[serde(default)]
        spec_version: u32,
        #[serde(default)]
        chart: serde_json::Value,
        #[serde(default)]
        table: String,
        #[serde(default)]
        fallback: Option<String>,
    },
    /// Link preview ([feedback link-embeds-mac-2026-10-27], #2065): the card the relay
    /// unfurled for the message's first bare URL. Read-only here; `preview: "off"` on the
    /// message hides it.
    LinkPreview {
        id: Id,
        #[serde(flatten)]
        preview: Box<LinkPreview>,
    },
    #[serde(other)]
    Unknown,
}

/// What a `link_preview` part carries (relay `unfurl.rs`, spec 1). Everything but `title` is
/// optional; `facts` are pre-formatted footer strings in order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkPreview {
    #[serde(default)]
    pub spec_version: u32,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub canonical_url: String,
    /// `spotify` `youtube` `x` `github` `bluesky` `vimeo` `loom` `soundcloud` `apple_music`
    /// `google_maps` `wikipedia` `arbos` `file` `web`.
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub provider_name: String,
    /// `song` `album` … `pull_request` `issue` `repo` `article` `image` `file` `place` `page`.
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub subtitle: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub facts: Vec<String>,
    /// `open` `merged` `closed` `draft`.
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub diff: Option<PreviewDiff>,
    /// "18:24" on a video thumbnail.
    #[serde(default)]
    pub duration: Option<String>,
    #[serde(default)]
    pub image: Option<PreviewImage>,
    /// The final host ("github.com"): the shared link-icon tile picks its mark by it.
    #[serde(default)]
    pub site_host: Option<String>,
    /// Reserved for the shared link-icon tile (a blob in the chat's slot); unused here.
    #[serde(default)]
    pub site_icon: Option<PreviewImage>,
    /// `compact` (art row) or `expanded` (media on top).
    #[serde(default)]
    pub layout: String,
}

impl LinkPreview {
    pub fn expanded(&self) -> bool {
        self.layout == "expanded" && self.image.is_some()
    }

    /// "<title> — <provider>" for quote headers and one-line previews.
    pub fn one_line(&self) -> String {
        if self.provider_name.is_empty() {
            self.title.clone()
        } else {
            format!("{} — {}", self.title, self.provider_name)
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewImage {
    #[serde(rename = "ref", default)]
    pub blob_ref: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default)]
    pub mime: String,
    #[serde(default)]
    pub art: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PreviewDiff {
    #[serde(default)]
    pub additions: u64,
    #[serde(default)]
    pub deletions: u64,
    #[serde(default)]
    pub files: u64,
}

fn open_state() -> QuestionState {
    QuestionState::Open
}

/// A widget this client cannot draw (newer type, malformed) degrades to a
/// plain question rather than failing the whole message.
fn de_widget<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Widget>, D::Error> {
    let v = Option::<serde_json::Value>::deserialize(d)?;
    Ok(v.and_then(|v| serde_json::from_value::<Widget>(v).ok()))
}

/// One skill of the library (F5); the relay's `Skill` shape, reused as the client view.
pub type Skill = arbos_core::skill::Skill;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRisk {
    Send,
    Destructive,
    Money,
    Credential,
    Computer,
    #[default]
    #[serde(other)]
    Other,
}

impl ApprovalRisk {
    pub fn label(self) -> &'static str {
        match self {
            ApprovalRisk::Send => "send",
            ApprovalRisk::Destructive => "destructive",
            ApprovalRisk::Money => "money",
            ApprovalRisk::Credential => "credential",
            ApprovalRisk::Computer => "computer",
            ApprovalRisk::Other => "other",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    #[default]
    Pending,
    Approved,
    Rejected,
    Expired,
}

impl ApprovalState {
    pub fn label(self) -> &'static str {
        match self {
            ApprovalState::Pending => "Pending",
            ApprovalState::Approved => "Approved",
            ApprovalState::Rejected => "Rejected",
            ApprovalState::Expired => "Expired",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusKind {
    /// Start of every user turn.
    Deciding,
    /// Act accepted / subagent slot acquired / result turn starting.
    Working,
    /// One tool call; the message is the short label for the working line.
    Step,
    /// Subagent waits for a slot.
    Queued,
    /// Subagent finished (collapses).
    Done,
    /// Subagent failed.
    Failed,
    /// User pressed stop mid-turn.
    Stopped,
    #[serde(alias = "waiting_user")]
    AskUser,
    #[serde(alias = "handed_off")]
    Handoff,
    /// S18: an approval card is pending; the composer shows the yellow waiting state.
    WaitingApproval,
    /// Fixture-only thin note.
    Note,
    #[serde(other)]
    Other,
}

/// The `job` map on a background job's hidden row (duplex §2.3). Only the id and
/// the title are read; `kind` / `touches` are the box's business.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRef {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
}

/// Where an open job is in its life — the two states the relay lists
/// (`activity.jobs[].state`, duplex §2.4). Finished, failed, cancelled and
/// interrupted jobs are not listed: their outcome is a bubble in the chat.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Waiting for a slot or a resource lock; not steerable yet.
    Queued,
    Running,
}

/// One open background job as the relay's activity record lists it
/// (`GET /projects/{id}` → `activity.jobs[]`): the rows of the tasks drawer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default = "Job::default_state")]
    pub state: JobState,
    /// The job's newest step in plain words ("Reading tripadvisor.com"); empty
    /// while it starts; "Waiting for <title>" while it queues behind a lock.
    #[serde(default)]
    pub step: String,
    #[serde(default)]
    pub elapsed_s: u64,
    /// The user message that asked for it: Steer prefills a reply to it.
    #[serde(default)]
    pub ask_message_id: Option<String>,
    #[serde(default)]
    pub chat_id: Option<String>,
}

impl Job {
    fn default_state() -> JobState {
        JobState::Running
    }

    /// `activity.jobs` as the relay sends it — `null`, missing or not a list all
    /// read as "no open jobs"; a row that does not parse is skipped, never fatal
    /// (the drawer must degrade to nothing on an older relay).
    pub fn list_from_activity(activity: Option<&serde_json::Value>) -> Vec<Job> {
        activity
            .and_then(|a| a.get("jobs"))
            .and_then(serde_json::Value::as_array)
            .map(|rows| {
                rows.iter()
                    .filter_map(|r| serde_json::from_value::<Job>(r.clone()).ok())
                    .filter(|j| !j.id.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Only a running job has a session to steer; a queued one has not started.
    pub fn can_steer(&self) -> bool {
        self.state == JobState::Running && self.ask_message_id.is_some()
    }

    /// The drawer row's second line (contract, "Drawer, per state"): the live
    /// step, "Working" for a running job with no step yet, "Starting" for a
    /// queued job with no step, the "Waiting for <title>" step as the box wrote it.
    pub fn status_line(&self) -> String {
        let step = self.step.trim();
        if !step.is_empty() {
            return step.to_string();
        }
        match self.state {
            JobState::Queued => "Starting".to_string(),
            JobState::Running => "Working".to_string(),
        }
    }

    /// `elapsed_s` as m:ss (an hour and more as h:mm:ss).
    pub fn elapsed_label(&self) -> String {
        let s = self.elapsed_s;
        if s >= 3600 {
            format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
        } else {
            format!("{}:{:02}", s / 60, s % 60)
        }
    }

    /// "1 task" / "N tasks": the chip's words and the presence suffix.
    pub fn count_label(n: usize) -> String {
        if n == 1 {
            "1 task".to_string()
        } else {
            format!("{n} tasks")
        }
    }

    /// The presence text without the box's "· N tasks" suffix — the chip
    /// carries the count, so the words are not said twice on one line.
    pub fn strip_count_suffix(text: &str) -> &str {
        let Some((head, tail)) = text.rsplit_once(" · ") else {
            return text;
        };
        let is_count = tail
            .strip_suffix(" tasks")
            .or_else(|| tail.strip_suffix(" task"))
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
        if is_count {
            head
        } else {
            text
        }
    }
}

/// One entry of a job's exchange (`GET /projects/{id}/jobs/{jobId}` → `entries[]`,
/// `internal/duplex-rollout-client-contract.md`): the parent's prompt, the job's own
/// words, a tool step (plain-words `label`, raw I/O stays in Details), a steer the
/// user sent, the final result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RolloutKind {
    Prompt,
    Text,
    Tool,
    Steer,
    Result,
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RolloutEntry {
    pub seq: u64,
    /// ms since the epoch.
    #[serde(default)]
    pub at: u64,
    #[serde(default = "RolloutEntry::default_kind")]
    pub kind: RolloutKind,
    #[serde(default)]
    pub text: Option<String>,
    /// A tool step's words ("Reading tripadvisor.com").
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    /// `running` | `done` | `failed` on a tool step or the result.
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

impl RolloutEntry {
    fn default_kind() -> RolloutKind {
        RolloutKind::Other
    }

    pub fn is_live(&self) -> bool {
        self.status.as_deref() == Some("running")
    }

    /// "2.1 s" / "1:04" for a step's duration.
    pub fn duration_label(&self) -> Option<String> {
        let ms = self.duration_ms?;
        Some(if ms < 60_000 {
            format!("{:.1} s", ms as f64 / 1000.0)
        } else {
            format!("{}:{:02}", ms / 60_000, (ms / 1000) % 60)
        })
    }
}

/// The job's parent agent as the document names it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RolloutParent {
    #[serde(default)]
    pub project_id: String,
    #[serde(default)]
    pub name: String,
}

/// One job's full exchange — the rollout view's document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobRollout {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default = "Job::default_state")]
    pub state: JobState,
    #[serde(default)]
    pub step: String,
    #[serde(default)]
    pub elapsed_s: u64,
    #[serde(default)]
    pub started_at: Option<u64>,
    #[serde(default)]
    pub ended_at: Option<u64>,
    #[serde(default)]
    pub chat_id: Option<String>,
    #[serde(default)]
    pub ask_message_id: Option<String>,
    #[serde(default)]
    pub parent: RolloutParent,
    #[serde(default)]
    pub brief: String,
    #[serde(default)]
    pub entries: Vec<RolloutEntry>,
    #[serde(default)]
    pub tool_calls: u64,
    #[serde(default)]
    pub next_seq: u64,
    /// Client-only: the relay answered 404 — the server half is not live yet.
    #[serde(default, skip_serializing)]
    pub unavailable: bool,
}

impl JobRollout {
    /// A rollout the client builds from the chip's `Job` while the document loads
    /// (or when the relay has no endpoint yet): header only.
    pub fn placeholder(job: &Job, parent_name: &str) -> Self {
        Self {
            id: job.id.clone(),
            title: job.title.clone(),
            kind: String::new(),
            state: job.state,
            step: job.step.clone(),
            elapsed_s: job.elapsed_s,
            started_at: None,
            ended_at: None,
            chat_id: job.chat_id.clone(),
            ask_message_id: job.ask_message_id.clone(),
            parent: RolloutParent {
                project_id: String::new(),
                name: parent_name.to_string(),
            },
            brief: String::new(),
            entries: Vec::new(),
            tool_calls: 0,
            next_seq: 0,
            unavailable: false,
        }
    }

    /// The job is still running: the view polls and shows Steer / Stop.
    pub fn is_running(&self) -> bool {
        self.ended_at.is_none() && self.outcome().is_none()
    }

    /// The final entry's status once the job ended (`done` | `failed` | `cancelled`).
    pub fn outcome(&self) -> Option<&str> {
        self.entries
            .iter()
            .rev()
            .find(|e| e.kind == RolloutKind::Result)
            .and_then(|e| e.status.as_deref())
    }

    /// Merge an incremental answer (`?after=<seq>`): header fields follow the newer
    /// document; entries are appended by `seq`, an entry already held is replaced
    /// (a tool step goes `running` → `done`).
    pub fn merge(&mut self, newer: JobRollout) {
        let JobRollout {
            entries, next_seq, ..
        } = newer.clone();
        let mut kept = self.entries.clone();
        for e in entries {
            match kept.iter_mut().find(|k| k.seq == e.seq) {
                Some(slot) => *slot = e,
                None => kept.push(e),
            }
        }
        kept.sort_by_key(|e| e.seq);
        *self = JobRollout {
            entries: kept,
            next_seq: next_seq.max(self.next_seq),
            ..newer
        };
    }

    /// "Running · 0:51" / "Finished · 6:12" — the tab dropdown's trailing words.
    pub fn status_short(&self) -> String {
        let full = self.status_line();
        match full.rsplit_once(" · ") {
            Some((head, _)) => head.to_string(),
            None => full,
        }
    }

    /// "Running · 0:51 · 4 tool calls" / "Finished · 6:12 · 11 tool calls".
    pub fn status_line(&self) -> String {
        let word = match (self.is_running(), self.state, self.outcome()) {
            (true, JobState::Queued, _) => "Queued",
            (true, JobState::Running, _) => "Running",
            (false, _, Some("failed")) => "Failed",
            (false, _, Some("cancelled")) => "Stopped",
            (false, _, _) => "Finished",
        };
        let elapsed = Job {
            id: String::new(),
            title: String::new(),
            state: self.state,
            step: String::new(),
            elapsed_s: self.elapsed_s,
            ask_message_id: None,
            chat_id: None,
        }
        .elapsed_label();
        let calls = match self.tool_calls {
            1 => "1 tool call".to_string(),
            n => format!("{n} tool calls"),
        };
        format!("{word} · {elapsed} · {calls}")
    }
}

/// One person or agent a membership line names: `user:<id>` / `agent:<project id>` split.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoticeSubject {
    pub kind: String,
    pub id: String,
}

/// The facts behind a membership system line (relay `notices::Notice`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipNotice {
    /// `added` | `removed` | `left` | `joined`.
    pub kind: String,
    /// User id of the person who acted.
    pub actor: String,
    #[serde(default)]
    pub subjects: Vec<NoticeSubject>,
}

impl MembershipNotice {
    /// The line worded from `name_of` — `name_of("user", id)` / `name_of("agent", id)` is the
    /// live roster name. `None` when anyone named is unknown to the roster (a person who has
    /// since left, a deleted agent): the caller then shows the stored words.
    pub fn text(&self, name_of: impl Fn(&str, &str) -> Option<String>) -> Option<String> {
        let actor = name_of("user", &self.actor)?;
        let names = self
            .subjects
            .iter()
            .map(|s| name_of(&s.kind, &s.id))
            .collect::<Option<Vec<String>>>()?;
        match self.kind.as_str() {
            "added" if !names.is_empty() => Some(format!("{actor} added {}", join_names(&names))),
            "removed" if !names.is_empty() => {
                Some(format!("{actor} removed {}", join_names(&names)))
            }
            "left" => Some(format!("{actor} left")),
            "joined" => Some(format!("{actor} joined")),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: Id,
    #[serde(default)]
    pub chat_id: Id,
    pub role: Role,
    #[serde(default)]
    pub sender: String,
    pub status: MessageStatus,
    #[serde(default, deserialize_with = "de_turn")]
    pub turn: Option<TurnKind>,
    /// Routing outcome on the assistant message: reply | act | spawn | wake.
    #[serde(default)]
    pub decision: Option<String>,
    #[serde(default)]
    pub subagent_id: Option<String>,
    #[serde(default)]
    pub subagent_name: Option<String>,
    /// Messages exchanged with the subagent on a hand-off row (1 = brief sent, 2 = result back).
    #[serde(default)]
    pub handoff_messages: Option<u32>,
    /// A membership line's facts (relay `notices::Notice`): the actor and subjects by id, so
    /// the line is worded from the live roster and a rename applies to old lines too
    /// ("Const added Aureum" → "Jacob added Aureum", 2026-10-26). `None` on rows older than
    /// the field — those keep their stored words.
    #[serde(default)]
    pub notice: Option<MembershipNotice>,
    /// A background worker the user never named (agentd, Grok parity trait 15): the
    /// transcript hides this spawn row — Grok shows no "N messages with …" line for its own
    /// workers — while the working line still reads its `activity`. An agent-to-agent
    /// `send_message(to)` row carries no flag and keeps its "Messaged ◉ X" line.
    #[serde(default)]
    pub worker: bool,
    /// Duplex design §2.3: the background job this row belongs to (`job: { id, title,
    /// kind, touches }`). The row is wordless and `hidden`; it exists so the presence
    /// line can read its `activity` and so the client knows a job is live in this chat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<JobRef>,
    /// The box asks the transcript to draw nothing for this row (job rows).
    #[serde(default)]
    pub hidden: bool,
    /// Agent-to-agent messaging: this reply answered a message another agent sent into the
    /// chat; the name drawn in the "Replied to ◉ X" line under the bubble.
    #[serde(default)]
    pub agent_reply_to: Option<String>,
    /// Emoji reactions, the iPhone's shape: `{ emoji: { "user:<id>": true } }`.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub reactions: std::collections::BTreeMap<String, std::collections::BTreeMap<String, bool>>,
    #[serde(default)]
    pub in_reply_to: Option<String>,
    /// Set on user messages a routine injected (`sender: routine:<id>`, F3).
    #[serde(default)]
    pub routine_name: Option<String>,
    /// Another Project's message in a group chat (`role: project`, F6): its name and glyph.
    #[serde(default)]
    pub sender_name: Option<String>,
    #[serde(default)]
    pub sender_icon: Option<String>,
    /// People chats (D72): who wrote it (`sender: user:<id>`), their handle and avatar.
    #[serde(default)]
    pub sender_handle: Option<String>,
    #[serde(default)]
    pub sender_avatar: Option<String>,
    /// Avatar colour of the agent that wrote it (`senderAvatarColor`, D72 shared agents).
    #[serde(default)]
    pub sender_avatar_color: Option<String>,
    /// `agent:<project_id>` / `user:<id>` the composer resolved from `@`.
    #[serde(default)]
    pub mentions: Vec<String>,
    #[serde(default)]
    pub reply_to: Option<String>,
    /// Bubble menu → Forward: the original sender's display name; the bubble
    /// shows "Forwarded from <name>" above the text (`forwardedFrom`).
    #[serde(default)]
    pub forwarded_from: Option<String>,
    /// Bubble menu → Reply in DM: the quoted message in another chat, with its
    /// snapshot (`replyRef`); the bubble shows the quote header plus
    /// "Replying to <who> in <chat>".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_ref: Option<ReplyRef>,
    /// On an agent's reply in a people chat: who asked and whose machine answered.
    #[serde(default)]
    pub asked_by_handle: Option<String>,
    #[serde(default)]
    pub owner_handle: Option<String>,
    /// Token usage as the box reports it (shape varies by harness).
    #[serde(default)]
    pub usage: Option<serde_json::Value>,
    #[serde(default)]
    pub return_contract: Option<serde_json::Value>,
    #[serde(default)]
    pub waiting_user: bool,
    /// At least one approval card of this message is still pending (S18).
    #[serde(default)]
    pub waiting_approval: bool,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<Effort>,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub completed_at: Option<u64>,
    /// System lines about a call ("<name> started a video call") carry it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    /// What the agent is doing right now ("Cloning RaoFoundation/subtensor"),
    /// shown on the working line while the turn streams (agent-flow contract).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    /// True until the relay `ACK`s the client's push (S3 faint echo). Client-only.
    #[serde(default)]
    pub pending: bool,
    #[serde(default)]
    pub parts: Vec<Part>,
    /// Parts the box flagged `hidden: true` (tool details): Details drawer only.
    #[serde(default)]
    pub hidden_parts: Vec<Part>,
    /// D82: the agent's canned first line, written once by the device that created
    /// the project; never read as the user's words.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub greeting: bool,
    /// `"off"` once the sender removed the link preview for everyone (#2065); the
    /// `link_preview` part, if any is still there, is not drawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

/// The reaction capsule's set, in Grok Bot's order (the iPhone's
/// `MessageReaction.palette`); any other emoji comes from the system picker.
pub const REACTION_PALETTE: [&str; 5] = ["😂", "👍", "👎", "❤️", "🎉"];

/// What a message says and shows, as one thing — the accessor Forward, Copy and any
/// other "take this message" action read, so they all see the same parts
/// ([feedback forward-attachments-mac-2026-10-27]): the prose, every attachment that is
/// on the relay (a chart's fallback picture excluded), and every chart with its fallback.
#[derive(Clone, Debug, PartialEq)]
pub struct MessageBody {
    pub text: String,
    pub attachments: Vec<Attachment>,
    pub charts: Vec<ChartBody>,
}

/// A chart bubble as a message carries it: the `chart` part plus its fallback picture
/// when the message has one.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartBody {
    /// Always a `Part::Chart`.
    pub chart: Part,
    pub fallback: Option<Attachment>,
}

/// An attachment the relay holds (not a `local:` key still uploading).
fn on_relay(a: &Attachment) -> bool {
    a.blob_ref
        .as_deref()
        .is_some_and(|k| !k.is_empty() && !k.starts_with("local:"))
}

impl Message {
    /// The message's body, in part order; `None` when there is nothing to take —
    /// no words, no attachment on the relay, no chart. A picture-only message has a
    /// body like any other.
    pub fn body(&self) -> Option<MessageBody> {
        let text = self.plain_text().trim().to_owned();
        let chart_fallbacks: HashSet<&str> = self
            .parts
            .iter()
            .filter_map(|p| match p {
                Part::Chart {
                    fallback: Some(f), ..
                } => Some(f.as_str()),
                _ => None,
            })
            .collect();
        let attachments: Vec<Attachment> = self
            .parts
            .iter()
            .filter_map(|p| match p {
                Part::Attachment { id, attachment }
                    if !chart_fallbacks.contains(id.as_str()) && on_relay(attachment) =>
                {
                    Some(attachment.clone())
                }
                _ => None,
            })
            .collect();
        let charts: Vec<ChartBody> = self
            .parts
            .iter()
            .filter_map(|p| match p {
                Part::Chart { fallback, .. } => {
                    let fallback = fallback.as_deref().and_then(|f| {
                        self.parts.iter().find_map(|q| match q {
                            Part::Attachment { id, attachment }
                                if id == f && on_relay(attachment) =>
                            {
                                Some(attachment.clone())
                            }
                            _ => None,
                        })
                    });
                    Some(ChartBody {
                        chart: p.clone(),
                        fallback,
                    })
                }
                _ => None,
            })
            .collect();
        (!text.is_empty() || !attachments.is_empty() || !charts.is_empty()).then_some(MessageBody {
            text,
            attachments,
            charts,
        })
    }

    /// Does the message hold an attachment whose upload has not finished (a `local:` key)?
    pub fn has_uploading_attachment(&self) -> bool {
        self.parts.iter().any(|p| match p {
            Part::Attachment { attachment, .. } => attachment
                .blob_ref
                .as_deref()
                .is_some_and(|k| k.starts_with("local:")),
            _ => false,
        })
    }
}

impl Message {
    /// Reactions as `(emoji, senders)`, palette order first, then the rest
    /// alphabetically; emojis nobody holds any more are dropped.
    /// Skin-tone variants of the same base merge into one chip.
    pub fn reaction_list(&self) -> Vec<(String, Vec<String>)> {
        use arbos_core::doc::normalize_reaction_emoji;
        use std::collections::{BTreeMap, BTreeSet};
        let mut by_base: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (emoji, people) in &self.reactions {
            if emoji.is_empty() {
                continue;
            }
            let base = normalize_reaction_emoji(emoji);
            if base.is_empty() {
                continue;
            }
            let entry = by_base.entry(base).or_default();
            for (sid, on) in people {
                if *on && !sid.is_empty() {
                    entry.insert(sid.clone());
                }
            }
        }
        let mut used = BTreeSet::new();
        let mut out: Vec<(String, Vec<String>)> = Vec::new();
        // Chip glyphs are the untoned base (matches core + iPhone orderedReactions).
        for e in REACTION_PALETTE {
            let base = normalize_reaction_emoji(e);
            let Some(people) = by_base.get(&base) else {
                continue;
            };
            if people.is_empty() || !used.insert(base.clone()) {
                continue;
            }
            out.push((base, people.iter().cloned().collect()));
        }
        for (base, people) in &by_base {
            if people.is_empty() || used.contains(base) {
                continue;
            }
            out.push((base.clone(), people.iter().cloned().collect()));
        }
        out
    }

    /// Every tool call of this turn, visible or hidden.
    pub fn tool_parts(&self) -> impl Iterator<Item = &Part> {
        self.parts
            .iter()
            .chain(self.hidden_parts.iter())
            .filter(|p| matches!(p, Part::Tool { .. }))
    }

    pub fn is_streaming(&self) -> bool {
        self.status == MessageStatus::Streaming
    }

    /// A streaming row older than this is treated as dead (box died mid-turn).
    /// Matches the iPhone's 15-minute freshness window so a leftover `streaming`
    /// status cannot pin the working indicator forever.
    pub const STREAM_FRESH_MS: u64 = 15 * 60 * 1000;

    /// True while a recent assistant message still streams and no newer user
    /// line has arrived after it. A message left `streaming` by a dead box
    /// (or a leftover stream under a later user message) must not pin the
    /// working indicator forever.
    pub fn chat_is_actively_streaming(messages: &[Message], now: u64) -> bool {
        let recent = now.saturating_sub(Self::STREAM_FRESH_MS);
        let Some(idx) = messages
            .iter()
            .rposition(|m| m.role == Role::Assistant && m.is_streaming() && m.created_at >= recent)
        else {
            return false;
        };
        !messages[idx + 1..].iter().any(|m| m.role == Role::User)
    }

    /// A subagent hand-off is still open: the brief went out (`handoff_messages`
    /// still below 2) and the result has not come back. The spawn/wake row may
    /// already be `done` while the child still runs — show the working line
    /// until the hand-off closes. Same freshness window as streaming so a dead
    /// box cannot pin forever.
    pub fn chat_has_open_subagent(messages: &[Message], now: u64) -> bool {
        let recent = now.saturating_sub(Self::STREAM_FRESH_MS);
        messages.iter().any(|m| {
            if m.role != Role::Assistant {
                return false;
            }
            if m.subagent_id.is_none() && m.subagent_name.is_none() {
                return false;
            }
            if !matches!(
                m.turn,
                Some(TurnKind::New | TurnKind::Spawn | TurnKind::Wake)
            ) {
                return false;
            }
            if m.handoff_messages.unwrap_or(1) >= 2 {
                return false;
            }
            if m.is_streaming() {
                return m.created_at >= recent;
            }
            let t = m.completed_at.unwrap_or(m.created_at);
            now.saturating_sub(t) < 60_000
        })
    }

    /// A background job's row that still runs (duplex §2.3): jobs are
    /// independent of the turn, so a newer user line does not end one — only
    /// the box finishing the row does, or the freshness window (a dead box).
    pub fn is_live_job_row(&self, now: u64) -> bool {
        self.role == Role::Assistant
            && self.job.is_some()
            && self.is_streaming()
            && self.created_at >= now.saturating_sub(Self::STREAM_FRESH_MS)
    }

    /// At least one job row of this chat is still live: the presence row stays
    /// up and the tasks drawer has something to poll for.
    pub fn chat_has_live_job(messages: &[Message], now: u64) -> bool {
        messages.iter().any(|m| m.is_live_job_row(now))
    }

    /// Working line should show: an active stream, a brief open hand-off, or
    /// a background job still running.
    pub fn chat_is_actively_working(messages: &[Message], now: u64) -> bool {
        Self::chat_is_actively_streaming(messages, now)
            || Self::chat_has_open_subagent(messages, now)
            || Self::chat_has_live_job(messages, now)
    }

    /// The working line's text while this turn streams: the `activity` field,
    /// else the newest `Working` / `Step` status message (visible or hidden).
    /// Generic placeholders ("Working…", "thinking") fall through so the line
    /// reads "<name> is working" — same rule as the iPhone (`AppModel.currentActivity`).
    /// A pending approval card wins over a stale tool label ("Removing files" while
    /// nothing has run yet).
    pub fn current_activity(&self) -> Option<String> {
        if self.waiting_approval || self.pending_approvals().next().is_some() {
            return Some("Waiting for approval".to_string());
        }
        if let Some(a) = self
            .activity
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
        {
            return Some(a.to_string());
        }
        self.parts
            .iter()
            .chain(self.hidden_parts.iter())
            .rev()
            .find_map(|p| match p {
                Part::Status {
                    code: StatusKind::Working | StatusKind::Step,
                    message,
                    ..
                } => {
                    let t = message.trim();
                    if t.is_empty() {
                        return None;
                    }
                    // "Working…", "Working.", "thinking" → "<name> is working".
                    let bare = t
                        .trim_matches(|c: char| c == '.' || c == '…')
                        .to_ascii_lowercase();
                    if bare.is_empty() || bare == "working" || bare == "thinking" {
                        None
                    } else {
                        Some(t.to_string())
                    }
                }
                _ => None,
            })
    }

    /// Does this message carry the `chart` part `part_id`? (Its fallback picture is
    /// hidden only when the chart it stands in for is really here.)
    pub fn has_chart_part(&self, part_id: &str) -> bool {
        self.parts
            .iter()
            .any(|p| matches!(p, Part::Chart { id, .. } if id == part_id))
    }

    /// The link preview the relay attached, unless the sender switched it off.
    pub fn link_preview(&self) -> Option<(&Id, &LinkPreview)> {
        if self.preview.as_deref() == Some("off") {
            return None;
        }
        self.parts.iter().find_map(|p| match p {
            Part::LinkPreview { id, preview } if !preview.title.trim().is_empty() => {
                Some((id, &**preview))
            }
            _ => None,
        })
    }

    /// Open `question` parts of this message, newest last.
    pub fn open_questions(&self) -> impl DoubleEndedIterator<Item = &Part> {
        self.parts.iter().filter(|p| {
            matches!(
                p,
                Part::Question {
                    state: QuestionState::Open,
                    ..
                }
            )
        })
    }

    /// Approval cards still waiting for a decision (S18).
    pub fn pending_approvals(&self) -> impl Iterator<Item = &Part> {
        self.parts.iter().filter(|p| {
            matches!(
                p,
                Part::Approval {
                    state: ApprovalState::Pending,
                    ..
                }
            )
        })
    }

    /// True when this message carries any `question` part (typed or plain), so the legacy
    /// `waiting_user` status row is redundant.
    pub fn has_question_part(&self) -> bool {
        self.parts
            .iter()
            .any(|p| matches!(p, Part::Question { .. }))
    }

    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for part in &self.parts {
            if let Part::Text { text, .. } = part {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(text);
            }
        }
        out
    }

    /// A finished row with no words, nothing to look at and no named sender (`user:`,
    /// `agent:`, `project:`): in a chat that names its senders it is a blank line from no
    /// one (Arbonites, 2026-10-28: a box's hand-off row and job row, bare `sender: "agent"`).
    pub fn is_senderless_blank(&self) -> bool {
        matches!(self.role, Role::User | Role::Assistant)
            && !self.is_streaming()
            && !["user:", "agent:", "project:"]
                .iter()
                .any(|p| self.sender.starts_with(p))
            && self.parts.iter().all(|p| match p {
                Part::Text { text, .. } => text.trim().is_empty(),
                Part::Reasoning { .. }
                | Part::Status { .. }
                | Part::SubagentResult { .. }
                | Part::Unknown => true,
                Part::Tool { .. }
                | Part::Attachment { .. }
                | Part::Error { .. }
                | Part::Diff { .. }
                | Part::File { .. }
                | Part::Browser { .. }
                | Part::Question { .. }
                | Part::Skills { .. }
                | Part::Approval { .. }
                | Part::SecretRedacted { .. }
                | Part::Chart { .. }
                | Part::LinkPreview { .. } => false,
            })
    }

    pub fn text_len(&self) -> usize {
        self.parts
            .iter()
            .map(|p| match p {
                Part::Text { text, .. } => text.len(),
                _ => 0,
            })
            .sum()
    }
}

/// One entry of the project doc `threads` map (T1 index, `crdt-sync.md` §6.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ThreadInfo {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub purpose: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub harness: String,
    #[serde(default)]
    pub last_turn_at: Option<i64>,
}

/// The parts of the project doc the client renders.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ProjectDocView {
    pub threads: Vec<ThreadInfo>,
    pub status: serde_json::Value,
    pub settings: serde_json::Value,
    pub memory_t1: String,
    /// `settings.modelNote`: set when a model id was replaced by a default.
    pub model_note: String,
}

/// `connectors.status` as the relay reports it (F1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorStatus {
    Connected,
    NeedsAuth,
    Error,
    #[serde(other)]
    Other,
}

impl ConnectorStatus {
    pub fn label(self) -> &'static str {
        match self {
            ConnectorStatus::Connected => "Connected",
            ConnectorStatus::NeedsAuth => "Needs sign-in",
            ConnectorStatus::Error => "Error",
            ConnectorStatus::Other => "Unknown",
        }
    }
}

/// One connected account (`GET /connectors` item). Never carries a token.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Connector {
    pub id: Id,
    pub provider: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    pub status: ConnectorStatus,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub mock: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConnectorAction {
    pub name: String,
    pub kind: String,
}

/// One catalog entry (`GET /connectors/catalog`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConnectorProvider {
    pub provider: String,
    pub label: String,
    /// `oauth | oauth_or_token | token`.
    #[serde(default)]
    pub connect: String,
    #[serde(default)]
    pub oauth_configured: bool,
    #[serde(default)]
    pub actions: Vec<ConnectorAction>,
}

impl ConnectorProvider {
    pub fn accepts_token(&self) -> bool {
        self.connect == "token" || self.connect == "oauth_or_token"
    }

    pub fn has_oauth(&self) -> bool {
        self.connect == "oauth" || self.connect == "oauth_or_token"
    }
}

/// Settings → Connectors state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConnectorsView {
    pub mock: bool,
    pub providers: Vec<ConnectorProvider>,
    pub items: Vec<Connector>,
    pub loaded: bool,
}

impl ConnectorsView {
    pub fn connector_for(&self, provider: &str) -> Option<&Connector> {
        self.items.iter().find(|c| c.provider == provider)
    }
}

/// One `arbos-local` helper the relay knows (`GET /local/devices` item, F9).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocalDevice {
    pub device_id: Id,
    pub name: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub home: String,
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub last_seen: Option<i64>,
    /// `read_file` / `list_dir` skip the approval card (still audited) when on.
    #[serde(default)]
    pub allow_reads: bool,
}

impl LocalDevice {
    pub fn os_label(&self) -> &'static str {
        match self.os.as_str() {
            "macos" => "macOS",
            "linux" => "Linux",
            "windows" => "Windows",
            _ => "Computer",
        }
    }
}

/// Settings → This Mac state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalView {
    pub devices: Vec<LocalDevice>,
    /// Relay URL the install command points the helper at.
    pub relay_url: String,
    pub loaded: bool,
}

/// One agent's switch on a secret (D83 Access): `secret.agents[]`, project-page `items[]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SecretAgent {
    pub project_id: Id,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub access_id: Id,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub scope: SecretScope,
    #[serde(default)]
    pub last_used_at: Option<i64>,
    #[serde(default)]
    pub disabled_at: Option<i64>,
}

/// A person on the secret's People list (`secret.people[]`): they may ask any agent with
/// Access to use it, with no card to the owner (spec §7.7).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SecretDirector {
    pub user_id: Id,
    #[serde(default)]
    pub handle: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub disabled_at: Option<i64>,
    #[serde(default)]
    pub scope: SecretScope,
    #[serde(default)]
    pub last_asked_at: Option<i64>,
}

impl SecretDirector {
    /// Display name, else the handle.
    pub fn display_name(&self) -> String {
        self.name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| self.handle.clone())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SecretScope {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub chat_id: Option<Id>,
}

/// One secret as the relay lists it (D83). Values are write-only; this never carries one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Secret {
    pub id: Id,
    pub name: String,
    /// `$STRIPE_KEY` in the box.
    #[serde(default)]
    pub env: String,
    /// `key | login | text`.
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub ask_every_time: bool,
    /// "Real value in the agent's environment": agents with Access get the value itself as
    /// `$STRIPE_KEY`, no card per use (D83 decision 2 as decided).
    #[serde(default)]
    pub inject_env: bool,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub version: i64,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub last_used_at: Option<i64>,
    /// Logins: the username is metadata; the password is the secret.
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub vault_name: String,
    #[serde(default)]
    pub agents: Vec<SecretAgent>,
    /// The People list (`people[]`; the relay also sends it as `directors[]`).
    #[serde(default)]
    pub directors: Vec<SecretDirector>,
    /// `refuse | card`: what a non-listed person's ask does (spec §7.7).
    #[serde(default)]
    pub others_ask: String,
    /// Where it came from: `settings`, or `chat` + the agent it was handed to (secret hand-off).
    #[serde(default)]
    pub source: Option<SecretSource>,
}

/// Where a secret came from (`secret.source`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SecretSource {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub project_name: Option<String>,
    #[serde(default)]
    pub chat_id: Option<String>,
}

impl Secret {
    /// "given in chat" for a hand-off secret, else nothing (list subtitle).
    pub fn source_label(&self) -> Option<&'static str> {
        self.source
            .as_ref()
            .filter(|s| s.kind == "chat")
            .map(|_| "given in chat")
    }

    /// "In chat with Arbos" — the Secret page's Added row, when it came from a chat.
    pub fn added_label(&self) -> Option<String> {
        let s = self.source.as_ref().filter(|s| s.kind == "chat")?;
        Some(match s.project_name.as_deref().filter(|n| !n.is_empty()) {
            Some(n) => format!("In chat with {n}"),
            None => "In chat".to_owned(),
        })
    }

    pub fn kind_label(&self) -> &'static str {
        match self.kind.as_str() {
            "login" => "Login",
            "text" => "Text",
            _ => "Key",
        }
    }

    pub fn glyph(&self) -> &'static str {
        if self.inject_env {
            "terminal"
        } else if self.kind == "login" {
            "person"
        } else {
            "key"
        }
    }

    /// The variables the real value takes: `$STRIPE_KEY`, or `$GITHUB_LOGIN_USERNAME and
    /// $GITHUB_LOGIN_PASSWORD` for a login.
    pub fn real_var_list(&self) -> String {
        if self.kind == "login" {
            format!("${0}_USERNAME and ${0}_PASSWORD", self.env)
        } else {
            format!("${}", self.env)
        }
    }
}

/// A card the owner has not answered yet (`GET /secrets/requests`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SecretRequest {
    pub id: Id,
    pub project_id: Id,
    #[serde(default)]
    pub project_name: Option<String>,
    #[serde(default)]
    pub chat_id: Id,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub purpose: String,
    /// `env | header | browser_fill`.
    #[serde(rename = "use", default)]
    pub use_kind: String,
    #[serde(default)]
    pub host: Option<String>,
    /// `pending | allowed_once | allowed_chat | allowed_always | denied | expired`.
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub decided_at: Option<i64>,
    /// The person who asked the agent, when it was not the owner (spec §7.7).
    #[serde(default)]
    pub asked_by_handle: Option<String>,
    #[serde(default)]
    pub asked_by_name: Option<String>,
}

impl SecretRequest {
    pub fn is_pending(&self) -> bool {
        self.status == "pending"
    }

    /// `@sam` for a request a person made through the agent; empty for the owner's own.
    pub fn for_whom(&self) -> Option<String> {
        self.asked_by_handle
            .as_ref()
            .filter(|h| !h.is_empty())
            .map(|h| format!("@{h}"))
    }
}

/// One audit row: names and hosts only, never a value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SecretActivity {
    pub id: i64,
    pub at: i64,
    pub action: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub target_kind: String,
    #[serde(default)]
    pub project_id: Option<Id>,
    #[serde(default)]
    pub project_name: Option<String>,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub by: String,
    #[serde(default)]
    pub detail: serde_json::Value,
}

impl SecretActivity {
    /// "Research used STRIPE_KEY · api.stripe.com".
    pub fn line(&self) -> String {
        let agent = self
            .project_name
            .clone()
            .unwrap_or_else(|| "An agent".into());
        let at = self
            .host
            .as_ref()
            .map(|h| format!(" · {h}"))
            .unwrap_or_default();
        let name = &self.name;
        let person = self.detail["handle"]
            .as_str()
            .filter(|h| !h.is_empty())
            .map(|h| format!("@{h}"))
            .unwrap_or_else(|| "someone".into());
        match self.action.as_str() {
            "used" => format!("{agent} used {name}{at}"),
            "refused" => format!("Refused: {agent} · {name}{at}"),
            "requested" => format!("{agent} asked for {name}{at}"),
            "person_on" | "director_on" if self.by.starts_with("card:") => {
                format!("You let {person} ask for {name}")
            }
            "person_on" | "director_on" => format!("You let {person} ask for {name}"),
            "person_off" | "director_off" => format!("You turned {name} off for {person}"),
            "decided" => match self.detail["decision"].as_str().unwrap_or("") {
                "denied" => format!("{agent} asked for {name}. Denied."),
                "allowed_once" => format!("You let {agent} use {name} once"),
                "allowed_chat" => format!("You let {agent} use {name} in one chat"),
                _ => format!("You let {agent} use {name}"),
            },
            "access_on" if self.by.starts_with("agent:") => format!("{agent} is using {name}{at}"),
            "access_on" => format!("You gave {agent} access to {name}"),
            "access_off" => format!("You turned {name} off for {agent}"),
            "created" => format!("You added {name}"),
            "updated" => format!("You edited {name}"),
            "value_replaced" => format!("You replaced the value of {name}"),
            "scrubbed" => format!("You scrubbed {name} from context"),
            "deleted" => format!("You deleted {name}"),
            "paused" if self.by == "stop_machine" => "Secrets paused with the machine".into(),
            "paused" => "You paused all secrets".into(),
            "unpaused" => "You turned secrets back on".into(),
            "expired" => format!("{name} expired"),
            "expiring" => format!("{name} expires soon"),
            "vault_connected" => format!("You connected {name}"),
            "vault_disconnected" => format!("You disconnected {name}"),
            "vault_refreshed" => format!("{name} refreshed"),
            "env_on" => format!("You put the real value of {name} in the environment"),
            "env_off" => format!("You took the real value of {name} out of the environment"),
            "injected" => format!("{name} went into {agent}'s environment"),
            other => format!("{} · {name}", other.replace('_', " ")),
        }
    }

    /// "asked by @sam · 2 min ago" when a person other than the owner asked; else the time.
    pub fn when_line(&self) -> String {
        match self.detail["asked_by_handle"]
            .as_str()
            .filter(|h| !h.is_empty())
        {
            Some(h) => format!("asked by @{h} · {}", ago(self.at)),
            None => ago(self.at),
        }
    }
}

/// "asked 2 min ago" · "never asked" · "turned off Oct 12" for a People row.
pub fn person_usage(last_asked_at: Option<i64>, disabled_at: Option<i64>, enabled: bool) -> String {
    if !enabled {
        if let Some(d) = disabled_at {
            return format!("turned off {}", short_date(d));
        }
    }
    match last_asked_at {
        Some(t) => format!("asked {}", ago(t)),
        None => "never asked".into(),
    }
}

/// A connected vault (Phase 2: 1Password).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConnectedVault {
    pub id: Id,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub items_count: i64,
    #[serde(default)]
    pub token_expires_at: Option<i64>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub agents: Vec<SecretAgent>,
    /// `{ reads_per_hour, reads_used_hour, reads_per_day, reads_used_day, cache_ttl_secs }`.
    #[serde(default)]
    pub limits: serde_json::Value,
    /// Cached `[{ ref, title, category, fields[], urls[], env, inject_env }]`, names only.
    #[serde(default)]
    pub items: Vec<serde_json::Value>,
    /// Items the owner put into the agents' environment as real values.
    #[serde(default)]
    pub env_items: i64,
}

impl ConnectedVault {
    pub fn provider_label(&self) -> &str {
        if self.provider == "onepassword" {
            "1Password"
        } else {
            &self.provider
        }
    }
}

/// A new secret as typed in Settings → Secrets; sent once, never kept.
#[derive(Clone, Debug, PartialEq)]
pub struct NewSecret {
    pub name: String,
    /// `key | login | text`.
    pub kind: String,
    /// `value` for key/text; `url`, `username`, `password` for a login.
    pub fields: Vec<(String, String)>,
    pub note: String,
    pub hosts: Vec<String>,
    /// Agents that get Access at once.
    pub agents: Vec<Id>,
    /// Real value in the agent's environment from the start.
    pub inject_env: bool,
}

/// One signed-in device of my account (`GET /auth/devices`, Oct 23).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    /// `github` (a GitHub sign-in), `paircode`, `reader` (read-only, Grok Bot) …
    #[serde(default)]
    pub kind: String,
    pub created_at: i64,
    #[serde(default)]
    pub last_seen_at: Option<i64>,
    /// Set once the device signed out or was revoked; such a row is no
    /// longer a session and the Devices list leaves it out.
    #[serde(default)]
    pub revoked_at: Option<i64>,
    /// The device that made the request — this Mac.
    #[serde(default)]
    pub current: bool,
}

/// Settings → Devices state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DevicesView {
    pub items: Vec<DeviceView>,
    pub loaded: bool,
    /// Why the list could not be read, when it could not.
    pub error: Option<crate::sync::user_error::UserError>,
}

/// Settings → Secrets state (`GET /secrets` + `GET /secrets/requests`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SecretsView {
    pub paused: bool,
    pub secrets: Vec<Secret>,
    pub vaults: Vec<ConnectedVault>,
    /// Vault providers this relay can connect (`["onepassword"]` once Phase 2 is live).
    pub providers: Vec<String>,
    /// Remote flag `features.vault_connect`: the Connect 1Password flow is shown only when the
    /// relay says so (default off since 2026-10-21; code kept).
    pub vault_connect: bool,
    pub requests: Vec<SecretRequest>,
    pub activity: Vec<SecretActivity>,
    pub loaded: bool,
}

impl SecretsView {
    pub fn pending(&self) -> impl Iterator<Item = &SecretRequest> {
        self.requests.iter().filter(|r| r.is_pending())
    }

    /// Settings row value: `4 secrets · 1Password`, `No secrets yet`, `Paused`.
    pub fn row_value(&self) -> String {
        if self.paused {
            return "Paused".into();
        }
        if self.secrets.is_empty() && self.vaults.is_empty() {
            return "No secrets yet".into();
        }
        let mut s = format!(
            "{} secret{}",
            self.secrets.len(),
            if self.secrets.len() == 1 { "" } else { "s" }
        );
        if let Some(v) = self.vaults.first() {
            s.push_str(&format!(" · {}", v.provider_label()));
        }
        s
    }

    pub fn secret(&self, id: &str) -> Option<&Secret> {
        self.secrets.iter().find(|s| s.id == id)
    }
}

/// One standing shell approval (#1544, `GET /projects/{id}/shell-grants`): the owner chose
/// "Always allow <agent> in /tmp" on a destructive-shell card. The relay holds them and the box
/// applies them; the agent pane's Permissions card that listed them left on 2026-10-25
/// ([feedback agent-details-remove-sections-mac-2026-10-25]) — the calls stay.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ShellGrant {
    pub id: String,
    #[serde(default)]
    pub rule_class: String,
    #[serde(default)]
    pub path_class: String,
    #[serde(default)]
    pub chat_id: Option<String>,
    #[serde(default)]
    pub granted_by: String,
    #[serde(default)]
    pub created_at: u64,
    /// "File deletes in /tmp" (relay-rendered).
    #[serde(default)]
    pub label: String,
    /// "scratch only: /tmp and /work/tmp" (relay-rendered).
    #[serde(default)]
    pub detail: String,
}

/// `GET /projects/{id}/secrets`: what one agent may use, for its profile page.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectSecrets {
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub owner: bool,
    #[serde(default)]
    pub items: Vec<ProjectSecretAccess>,
    #[serde(default)]
    pub requests: Vec<SecretRequest>,
    #[serde(default)]
    pub activity: Vec<SecretActivity>,
}

/// One row of the agent's Secrets page: an Access on a secret or on a whole vault.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectSecretAccess {
    #[serde(default)]
    pub access_id: Id,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub last_used_at: Option<i64>,
    #[serde(default)]
    pub disabled_at: Option<i64>,
    #[serde(default)]
    pub secret: Option<Secret>,
    #[serde(default)]
    pub vault: Option<ConnectedVault>,
}

impl ProjectSecretAccess {
    pub fn title(&self) -> String {
        if let Some(s) = &self.secret {
            s.env.clone()
        } else if let Some(v) = &self.vault {
            v.name.clone()
        } else {
            "Secret".into()
        }
    }
}

/// "used 2 min ago" · "never used" · "turned off Oct 12".
pub fn secret_usage(last_used_at: Option<i64>, disabled_at: Option<i64>, enabled: bool) -> String {
    if !enabled {
        if let Some(d) = disabled_at {
            return format!("turned off {}", short_date(d));
        }
    }
    match last_used_at {
        Some(t) => format!("used {}", ago(t)),
        None => "never used".into(),
    }
}

pub fn ago(ms: i64) -> String {
    let d = (now_ms() as i64 - ms).max(0) / 1000;
    if d < 60 {
        "just now".into()
    } else if d < 3600 {
        format!("{} min ago", d / 60)
    } else if d < 86_400 {
        format!("{} h ago", d / 3600)
    } else if d < 7 * 86_400 {
        format!("{} d ago", d / 86_400)
    } else {
        short_date(ms)
    }
}

/// "Oct 12" (this year) or "Oct 12 2025".
pub fn short_date(ms: i64) -> String {
    let dt = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms).unwrap_or_default();
    let now = chrono::Utc::now();
    if dt.format("%Y").to_string() == now.format("%Y").to_string() {
        dt.format("%b %-d").to_string()
    } else {
        dt.format("%b %-d %Y").to_string()
    }
}

/// `stripe live` → `STRIPE_LIVE` (mirrors the relay's rule).
pub fn secret_env_name(name: &str) -> String {
    let mut s = String::new();
    for ch in name.trim().chars() {
        s.push(if ch.is_ascii_alphanumeric() {
            ch.to_ascii_uppercase()
        } else {
            '_'
        });
    }
    while s.contains("__") {
        s = s.replace("__", "_");
    }
    let s = s.trim_matches('_').to_owned();
    if s.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        format!("_{s}")
    } else {
        s
    }
}

/// How a routine fires (relay `routines.trigger`, F3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RoutineTrigger {
    Cron {
        expr: String,
        #[serde(default = "utc")]
        tz: String,
    },
    /// Fire once at unix ms, then stop.
    Once {
        #[serde(alias = "at_ms")]
        at: i64,
    },
    Webhook {
        #[serde(default)]
        secret: String,
    },
    Event {
        event: String,
    },
}

fn utc() -> String {
    "UTC".into()
}

impl RoutineTrigger {
    /// Short human label for the list ("Weekdays 09:00", "Webhook", "On box awake").
    pub fn label(&self) -> String {
        match self {
            RoutineTrigger::Cron { expr, tz } => {
                let base = match expr.trim() {
                    "* * * * *" => "Every minute".to_owned(),
                    "0 * * * *" => "Hourly".to_owned(),
                    "0 9 * * *" => "Daily 09:00".to_owned(),
                    "0 9 * * 1-5" => "Weekdays 09:00".to_owned(),
                    other => format!("cron {other}"),
                };
                if tz == "UTC" {
                    base
                } else {
                    format!("{base} {tz}")
                }
            }
            RoutineTrigger::Once { .. } => "Once".into(),
            RoutineTrigger::Webhook { .. } => "Webhook".into(),
            RoutineTrigger::Event { event } => match event.as_str() {
                "box_awake" => "On box awake".into(),
                "cross_project_message" => "On project message".into(),
                "connector_event" => "On connector event".into(),
                other => format!("On {other}"),
            },
        }
    }
}

/// A scheduled or event-triggered prompt (relay `routines` row).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Routine {
    pub id: Id,
    pub project_id: Id,
    pub chat_id: Id,
    pub name: String,
    pub prompt: String,
    pub trigger: RoutineTrigger,
    pub enabled: bool,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub last_run_at: Option<i64>,
    #[serde(default)]
    pub next_run_at: Option<i64>,
    /// Webhook routines only: the URL that fires them (carries the secret).
    #[serde(default)]
    pub fire_url: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutineRunStatus {
    Queued,
    Running,
    Done,
    Failed,
    Skipped,
    #[serde(other)]
    Other,
}

impl RoutineRunStatus {
    pub fn label(self) -> &'static str {
        match self {
            RoutineRunStatus::Queued => "queued",
            RoutineRunStatus::Running => "running",
            RoutineRunStatus::Done => "done",
            RoutineRunStatus::Failed => "failed",
            RoutineRunStatus::Skipped => "skipped",
            RoutineRunStatus::Other => "—",
        }
    }
}

/// One row of a routine's history (relay `routine_runs`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoutineRun {
    pub id: Id,
    pub routine_id: Id,
    pub started_at: i64,
    #[serde(default)]
    pub finished_at: Option<i64>,
    pub status: RoutineRunStatus,
    /// The assistant reply; "open" jumps to it.
    #[serde(default)]
    pub message_id: Option<Id>,
    #[serde(default)]
    pub note: Option<String>,
}

/// One platform row in Settings → Channels (`GET /channels` `platforms[]`, F7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelPlatformState {
    Configured,
    Missing,
    Mock,
    #[serde(other)]
    Other,
}

impl ChannelPlatformState {
    pub fn label(self) -> &'static str {
        match self {
            ChannelPlatformState::Configured => "Configured",
            ChannelPlatformState::Missing => "Missing credentials",
            ChannelPlatformState::Mock => "Mock",
            ChannelPlatformState::Other => "Unknown",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChannelPlatform {
    pub platform: String,
    pub label: String,
    pub state: ChannelPlatformState,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub inbound: String,
}

/// One paired external chat (`GET /channels` `bindings[]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChannelBinding {
    pub id: Id,
    pub platform: String,
    #[serde(rename = "externalChatId")]
    pub external_chat_id: String,
    #[serde(rename = "externalUserId")]
    pub external_user_id: String,
    #[serde(rename = "projectId")]
    pub project_id: Id,
    #[serde(rename = "chatId")]
    pub chat_id: Id,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub bound_at: u64,
}

/// The active pairing code (10 min, one use).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChannelPairing {
    pub code: String,
    #[serde(rename = "projectId")]
    pub project_id: Id,
    #[serde(rename = "chatId")]
    pub chat_id: Id,
    #[serde(rename = "expiresAt")]
    pub expires_at: u64,
}

impl ChannelPairing {
    pub fn seconds_left(&self) -> u64 {
        self.expires_at.saturating_sub(now_ms()) / 1000
    }
}

/// Settings → Channels state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChannelsView {
    pub mock: bool,
    pub platforms: Vec<ChannelPlatform>,
    pub bindings: Vec<ChannelBinding>,
    pub pairing: Option<ChannelPairing>,
    pub loaded: bool,
}

/// The user's one Daytona computer. Projects are workspaces on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MachineState {
    #[default]
    Asleep,
    Waking,
    Awake,
    Stopped,
    Error,
    #[serde(other)]
    Unknown,
}

impl MachineState {
    pub fn label(self) -> &'static str {
        match self {
            MachineState::Asleep => "asleep",
            MachineState::Waking => "waking",
            MachineState::Awake => "awake",
            MachineState::Stopped => "stopped",
            MachineState::Error => "error",
            MachineState::Unknown => "unknown",
        }
    }

    pub fn is_down(self) -> bool {
        matches!(
            self,
            MachineState::Asleep
                | MachineState::Stopped
                | MachineState::Error
                | MachineState::Unknown
        )
    }
}

/// `status.box {state, since}` at user level (mirrored into every project
/// doc) plus the hover details.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Machine {
    pub state: MachineState,
    pub since: Option<u64>,
    /// e.g. "4 vCPU · 8 GiB · 10 GiB" (`GET /machine` `spec`).
    pub spec: Option<String>,
    /// Snapshot / resource class, e.g. "arbos-agent-0.1.7".
    pub class: Option<String>,
    pub spend_today_usd: f64,
    pub spend_cap_usd: f64,
    pub detail: Option<String>,
    /// 0..1 while waking.
    pub progress: Option<f32>,
    /// Hosted machines never sleep (`machine.always_on`): no waking/asleep
    /// states in the UI, machine time is included rather than billed to credits.
    pub always_on: bool,
    /// `spec.cpu` / `spec.mem_gib` and `image.current` as reported (D82
    /// first-login detail lines: "Starting a 4-core machine with 8 GB of memory").
    pub cpu: Option<u32>,
    pub mem_gib: Option<u32>,
    pub image: Option<String>,
    /// `machine.provider` (`gvisor` = a sandbox on the arbos.life host, D89; `daytona`; …).
    pub provider: Option<String>,
    /// D89: the host is full and this account is in line for a machine (`/me` →
    /// `machine.waitlisted` / `machine.waitlist_position`). None = not waitlisted.
    pub waitlist_position: Option<u32>,
}

impl Machine {
    /// Settings › Machine › "Where it runs" (D89). "Change machine" comes with the next runner.
    pub fn where_it_runs(&self) -> &'static str {
        match self.provider.as_deref() {
            Some("daytona") => "Hosted · Daytona",
            Some("process") | Some("docker") => "Relay host",
            Some("gvisor") | None | Some("") => "Hosted on arbos.life",
            Some(_) => "Hosted",
        }
    }

    /// What the UI shows: an always-on machine is awake unless unreachable.
    pub fn shown_state(&self) -> MachineState {
        if self.always_on && self.state != MachineState::Error {
            MachineState::Awake
        } else {
            self.state
        }
    }
}

/// The box as `GET /machine/debug` reports it — the relay's view plus the
/// box's own `GET /box/state` (image, uptime, load, memory, disk, every
/// workspace with its turn lock). The agent pane's **Computer** tab
/// ([feedback agent-pane-details-tab-2026-10-24]) reads this; nothing else.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoxReport {
    pub fetched_at: u64,
    /// `relay.state`: awake / waking / asleep / stopped / error.
    pub state: String,
    pub awake_for_s: Option<u64>,
    /// `relay.agentd.image` → the image the box booted (e.g. `arbos-agent-0.1.114`).
    pub image: Option<String>,
    /// `box.reachable`: the box answered `/box/state`; the rest is `None` when it did not.
    pub reachable: bool,
    pub unreachable_reason: Option<String>,
    pub uptime_s: Option<u64>,
    /// 1-minute load average. Inside a Daytona sandbox this is the **host's** (`load_scope`
    /// = `host`, agentd ≥ 0.6.277): then it says nothing about the box and is not drawn.
    pub load_1m: Option<f64>,
    pub load_is_host: bool,
    pub mem_total_mb: Option<u64>,
    pub mem_available_mb: Option<u64>,
    pub disk_total_mb: Option<u64>,
    pub disk_available_mb: Option<u64>,
    /// `resources.cpu` (agentd ≥ 0.6.277): cores the box may use and the share busy over
    /// the last 5 s.
    pub cpu_cores: Option<f64>,
    pub cpu_pct: Option<f64>,
    /// `resources.disk.inodes_*`: file slots (#1857 — a box can be "full" with bytes free).
    pub inodes_total: Option<u64>,
    pub inodes_avail: Option<u64>,
    /// `resources.net`: bytes per second over the last 5 s and bytes since the UTC day began.
    pub net_rx_bps: Option<f64>,
    pub net_tx_bps: Option<f64>,
    pub net_rx_today: Option<u64>,
    pub net_tx_today: Option<u64>,
    pub workspaces: Vec<BoxWorkspace>,
}

/// One agent's workspace on the box.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoxWorkspace {
    pub project_id: Id,
    pub name: String,
    pub healthy: bool,
    /// A turn is running right now (`turn.running`).
    pub running: bool,
    /// How long the current turn has run (`turn.current.elapsed_ms`).
    pub elapsed_ms: Option<u64>,
    pub tool_calls: Option<u64>,
    /// Background jobs (sub-agents) running for this agent (`jobs_running`, agentd ≥ 0.6.277).
    pub jobs_running: Option<u64>,
    /// Its share of the box (`usage`, from the workspace cgroup and a cached `du`).
    pub cpu_pct: Option<f64>,
    pub mem_mb: Option<u64>,
    pub pids: Option<u64>,
    pub disk_mb: Option<u64>,
}

impl BoxReport {
    /// `GET /machine/debug` → the report. Lenient: every field is optional on
    /// the wire (older relays, an unreachable box), nothing here fails.
    pub fn from_debug(v: &serde_json::Value, now: u64) -> BoxReport {
        let relay = &v["relay"];
        let on_box = &v["box"];
        let st = &on_box["state"];
        let mut out = BoxReport {
            fetched_at: now,
            state: relay["state"].as_str().unwrap_or("unknown").to_owned(),
            awake_for_s: relay["awake_for_s"].as_u64(),
            image: relay["agentd"]["image"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| st["image_version"].as_str().map(str::to_owned)),
            reachable: on_box["reachable"].as_bool().unwrap_or(false),
            unreachable_reason: on_box["error"].as_str().map(str::to_owned),
            ..Default::default()
        };
        if !out.reachable {
            return out;
        }
        out.uptime_s = st["uptime_s"].as_u64();
        out.load_1m = st["load"].get(0).and_then(|l| l.as_f64());
        out.load_is_host = st["load_scope"].as_str() == Some("host");
        // `memory_mb` carries the cgroup cap on agentd ≥ 0.6.277 (the host's figures before);
        // `resources.memory` is the same view with `used` instead of `available`.
        let res = &st["resources"];
        out.mem_total_mb = res["memory"]["total_mb"]
            .as_u64()
            .or_else(|| st["memory_mb"]["total"].as_u64());
        out.mem_available_mb = match (
            res["memory"]["total_mb"].as_u64(),
            res["memory"]["used_mb"].as_u64(),
        ) {
            (Some(t), Some(u)) => Some(t.saturating_sub(u)),
            _ => st["memory_mb"]["available"].as_u64(),
        };
        out.disk_total_mb = st["disk"]["total_mb"].as_u64();
        out.disk_available_mb = st["disk"]["available_mb"].as_u64();
        out.cpu_cores = res["cpu"]["cores"].as_f64();
        out.cpu_pct = res["cpu"]["pct"].as_f64();
        out.inodes_total = res["disk"]["inodes_total"].as_u64();
        out.inodes_avail = res["disk"]["inodes_avail"].as_u64();
        out.net_rx_bps = res["net"]["rx_bps"].as_f64();
        out.net_tx_bps = res["net"]["tx_bps"].as_f64();
        out.net_rx_today = res["net"]["rx_bytes_today"].as_u64();
        out.net_tx_today = res["net"]["tx_bytes_today"].as_u64();
        out.workspaces = st["workspaces"]
            .as_array()
            .map(|ws| {
                ws.iter()
                    .map(|w| BoxWorkspace {
                        project_id: w["project_id"].as_str().unwrap_or_default().to_owned(),
                        name: w["name"].as_str().unwrap_or_default().to_owned(),
                        healthy: w["healthy"].as_bool().unwrap_or(false),
                        running: w["turn"]["running"].as_bool().unwrap_or(false),
                        elapsed_ms: w["turn"]["current"]["elapsed_ms"].as_u64(),
                        tool_calls: w["turn"]["current"]["tool_calls"].as_u64(),
                        jobs_running: w["jobs_running"].as_u64(),
                        cpu_pct: w["usage"]["cpu_pct"].as_f64(),
                        mem_mb: w["usage"]["mem_mb"].as_u64(),
                        pids: w["usage"]["pids"].as_u64(),
                        disk_mb: w["usage"]["disk_mb"].as_u64(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        out
    }

    /// Workspaces whose box share the sampler reported (agentd ≥ 0.6.277), biggest memory
    /// first — the per-agent breakdown.
    pub fn usage_rows(&self) -> Vec<&BoxWorkspace> {
        let mut rows: Vec<&BoxWorkspace> = self
            .workspaces
            .iter()
            .filter(|w| w.cpu_pct.is_some() || w.mem_mb.is_some() || w.disk_mb.is_some())
            .collect();
        rows.sort_by(|a, b| {
            b.running
                .cmp(&a.running)
                .then(b.mem_mb.unwrap_or(0).cmp(&a.mem_mb.unwrap_or(0)))
                .then(a.name.cmp(&b.name))
        });
        rows
    }

    /// Background jobs running across every workspace, when any workspace reports the count.
    pub fn jobs_running(&self) -> Option<u64> {
        let counts: Vec<u64> = self
            .workspaces
            .iter()
            .filter_map(|w| w.jobs_running)
            .collect();
        (!counts.is_empty()).then(|| counts.iter().sum())
    }

    /// The workspace of one agent, if the box lists it.
    pub fn workspace(&self, project_id: &str) -> Option<&BoxWorkspace> {
        self.workspaces.iter().find(|w| w.project_id == project_id)
    }

    /// Workspaces with a turn running right now — the box's running jobs.
    pub fn running(&self) -> impl Iterator<Item = &BoxWorkspace> {
        self.workspaces.iter().filter(|w| w.running)
    }
}

/// "12.4 GB" / "840 MB" from megabytes.
pub fn size_mb(mb: u64) -> String {
    if mb >= 1024 {
        let gb = mb as f64 / 1024.0;
        if gb >= 10.0 {
            format!("{gb:.0} GB")
        } else {
            format!("{gb:.1} GB")
        }
    } else {
        format!("{mb} MB")
    }
}

/// "3 d 4 h" / "2 h 10 m" / "45 s" from seconds.
pub fn duration_s(s: u64) -> String {
    let (d, h, m) = (s / 86_400, (s % 86_400) / 3600, (s % 3600) / 60);
    if d > 0 {
        format!("{d} d {h} h")
    } else if h > 0 {
        format!("{h} h {m} m")
    } else if m > 0 {
        format!("{m} m")
    } else {
        format!("{s} s")
    }
}

/// Settings → Account: who is signed in and what is left (`GET /me`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AccountView {
    /// `gh_<github id>`; what `sender: user:<id>` carries.
    pub id: String,
    pub login: String,
    pub name: String,
    pub avatar_url: String,
    pub device_name: String,
    pub balance_usd: f64,
    pub spent_today_usd: f64,
    pub daily_cap_usd: Option<f64>,
}

/// Connection to the relay, S14 (coarse, app-wide: the sidebar's "Loading…"
/// copy and the Settings details line). The chat header's status pill reads
/// [`LinkState`] per room instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    Connecting,
    Live,
    Reconnecting,
}

/// One chat's link to the relay as the header pill shows it ([feedback
/// connection-pill]). Ordered by priority: when several signals disagree
/// (this room's socket, the relay's HTTP poll) the highest wins, so the
/// derived `Ord` is the rule "offline > relay down > reconnecting > syncing >
/// restart soon".
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LinkState {
    /// Socket up and caught up: nothing to show.
    Live,
    /// The relay said it restarts in a few seconds (`restart_soon` frame):
    /// the socket is still up; a clean close follows ([#1340]).
    RestartSoon,
    /// Socket up, catch-up in flight (HELLO sent, rows still arriving).
    Syncing,
    /// The socket dropped or the dial timed out; redialing with backoff.
    Reconnecting,
    /// The relay refused the connection or answered 5xx: this Mac is online,
    /// the relay is not.
    RelayDown,
    /// No network from this Mac (DNS failed, network unreachable).
    Offline,
}

impl LinkState {
    /// The pill's text; `None` when there is nothing to say.
    pub fn label(self) -> Option<&'static str> {
        match self {
            LinkState::Live => None,
            LinkState::RestartSoon => Some("Reconnecting shortly…"),
            LinkState::Syncing => Some("Syncing…"),
            LinkState::Reconnecting => Some("Reconnecting…"),
            LinkState::RelayDown => Some("Relay down"),
            LinkState::Offline => Some("Offline"),
        }
    }
}

/// One channel of a server as the relay registry lists it (`GET /servers`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerChannel {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub server_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, deserialize_with = "null_to_empty")]
    pub topic: String,
    /// Admins-only posting.
    #[serde(default)]
    pub restricted: bool,
    #[serde(default)]
    pub position: i64,
    /// `None` for a few seconds after creation while the room is being made.
    #[serde(default)]
    pub room_id: Option<String>,
    #[serde(default)]
    pub created_at: i64,
    /// Hidden from my channel list (per user; the server's Channels tab brings it back).
    #[serde(default)]
    pub archived: bool,
}

/// Invite link of a server; the relay only sends it to owners and admins.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerInvite {
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub url: String,
}

/// One of the owner's invite links (several per server since 2026-10-22): optional expiry and
/// a use count; `usable` is the relay's verdict right now.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerInviteLink {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub uses_left: Option<i64>,
    #[serde(default = "default_true")]
    pub usable: bool,
}

impl ServerInviteLink {
    /// "7 days left · 3 uses", "No limits", "Expired", "Used up".
    pub fn terms(&self, now_ms: i64) -> String {
        if !self.usable {
            return if self.uses_left == Some(0) {
                "Used up".into()
            } else {
                "Expired".into()
            };
        }
        let mut parts: Vec<String> = Vec::new();
        if let Some(t) = self.expires_at {
            let days = ((t - now_ms).max(0) + 86_399_999) / 86_400_000;
            parts.push(if days <= 1 {
                "Expires today".into()
            } else {
                format!("{days} days left")
            });
        }
        if let Some(n) = self.uses_left {
            parts.push(format!("{n} use{} left", if n == 1 { "" } else { "s" }));
        }
        if parts.is_empty() {
            "No limits".into()
        } else {
            parts.join(" · ")
        }
    }
}

/// A server (D88 C1) as the relay returns it; `members` only on the detail route and on create.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerInfo {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub image_key: Option<String>,
    /// `image_key` as the avatar loader fetches it (`/blob/servers:{id}/{key}?token=…`), filled in
    /// by the relay client; not on the wire.
    #[serde(skip)]
    pub image_url: Option<String>,
    /// D92 amendment: the leaf picture from the Create screen; `None` = initials.
    #[serde(default)]
    pub avatar_leaf: Option<u8>,
    #[serde(default)]
    pub avatar_leaf_color: Option<u8>,
    #[serde(default)]
    pub owner_user_id: String,
    #[serde(default)]
    pub space_room_id: Option<String>,
    #[serde(default)]
    pub created_at: i64,
    /// `owner`, `admin` or `member`.
    #[serde(default)]
    pub my_role: String,
    #[serde(default)]
    pub member_count: u64,
    #[serde(default)]
    pub channels: Vec<ServerChannel>,
    /// Sidebar archive (2026-10-23): hidden from the list for me, on every device.
    #[serde(default)]
    pub archived_at: Option<u64>,
    #[serde(default)]
    pub general_channel_id: Option<String>,
    #[serde(default)]
    pub avatar_color_index_effective: Option<u8>,
    #[serde(default)]
    pub invite: Option<ServerInvite>,
    /// Owner only: every live link (expired / used-up ones included, so the owner sees why).
    #[serde(default)]
    pub invites: Vec<ServerInviteLink>,
    /// Members with `role` set to `owner`/`admin`/`member`; empty until the detail is fetched.
    #[serde(default)]
    pub members: Vec<Person>,
}

impl ServerInfo {
    pub fn is_owner(&self) -> bool {
        self.my_role == "owner"
    }

    pub fn can_admin(&self) -> bool {
        self.my_role == "owner" || self.my_role == "admin"
    }

    pub fn channel(&self, channel_id: &str) -> Option<&ServerChannel> {
        self.channels.iter().find(|c| c.id == channel_id)
    }

    /// `#general` (the relay names it), else the first channel.
    pub fn general(&self) -> Option<&ServerChannel> {
        self.general_channel_id
            .as_deref()
            .and_then(|id| self.channel(id))
            .or_else(|| self.channels.iter().find(|c| c.name == "general"))
            .or_else(|| self.channels.first())
    }

    /// Channels in registry order (`position`, then name).
    pub fn ordered_channels(&self) -> Vec<&ServerChannel> {
        self.sorted_channels(|c| !c.archived)
    }

    /// The channels I archived, in list order.
    pub fn archived_channels(&self) -> Vec<&ServerChannel> {
        self.sorted_channels(|c| c.archived)
    }

    fn sorted_channels(&self, keep: impl Fn(&ServerChannel) -> bool) -> Vec<&ServerChannel> {
        let mut out: Vec<&ServerChannel> = self.channels.iter().filter(|c| keep(c)).collect();
        out.sort_by(|a, b| {
            a.position
                .cmp(&b.position)
                .then_with(|| a.name.cmp(&b.name))
        });
        out
    }
}

/// Which channel of which server is open in the centre.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChannelSel {
    pub server_id: Id,
    pub channel_id: Id,
    /// Known once the registry (or the room list) has the room.
    pub room_id: Option<String>,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn new_id() -> Id {
    ulid::Ulid::new().to_string()
}

/// `ARBOS_TRACE_ROWS=1`: one stderr line per hop of a row's path through this app (socket
/// frame → doc import → snapshot → app state → transcript), each with a ms timestamp, so a
/// "the notice came late" report can say which hop waited. Off by default; read once.
pub fn trace_rows() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("ARBOS_TRACE_ROWS").is_ok_and(|v| v == "1"))
}

/// A membership notice this device shows before the relay's own row arrives (ARB-393): the
/// adding Mac writes "<me> added <names>" into its transcript the moment Add is pressed, so the
/// line never waits on the round trip. The relay stays the author of record — when its system
/// row with the same words lands, the local copy is dropped; a failed add or a row that never
/// comes removes it after [`OptimisticNotice::TTL_MS`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptimisticNotice {
    /// The local row's id (`local-notice:<ulid>`); never pushed to the relay.
    pub id: Id,
    pub text: String,
    pub created_at: u64,
    /// The facts the relay's row will carry (see [`Self::new`]).
    pub notice: Option<MembershipNotice>,
}

impl OptimisticNotice {
    /// Row id prefix: a snapshot never carries these, so they are easy to tell apart.
    pub const ID_PREFIX: &'static str = "local-notice:";
    /// How long an unconfirmed local notice may stay on screen.
    pub const TTL_MS: u64 = 60_000;
    /// The relay stamps its row with its own clock; a row this much older than the local one
    /// still counts as the same notice.
    const SKEW_MS: u64 = 15_000;

    /// `notice`: the same facts the relay's row will carry (actor = me, subjects by id), so
    /// this copy is worded from the live roster like that row and matched to it by facts,
    /// not by words a stale local name could get wrong ("Const added Aureum", 2026-10-26).
    pub fn new(text: String, now: u64, notice: Option<MembershipNotice>) -> Self {
        Self {
            id: format!("{}{}", Self::ID_PREFIX, new_id()),
            text,
            created_at: now,
            notice,
        }
    }

    /// The transcript row for this notice: a centred system stamp like the relay's.
    pub fn message(&self, chat_id: &str) -> Message {
        Message {
            id: self.id.clone(),
            chat_id: chat_id.to_owned(),
            role: Role::System,
            sender: "system".into(),
            status: MessageStatus::Done,
            turn: None,
            decision: None,
            subagent_id: None,
            subagent_name: None,
            handoff_messages: None,
            notice: self.notice.clone(),
            worker: false,
            job: None,
            hidden: false,
            agent_reply_to: None,
            reactions: Default::default(),
            in_reply_to: None,
            routine_name: None,
            sender_name: None,
            sender_icon: None,
            sender_handle: None,
            sender_avatar: None,
            sender_avatar_color: None,
            mentions: Vec::new(),
            reply_to: None,
            forwarded_from: None,
            reply_ref: None,
            asked_by_handle: None,
            owner_handle: None,
            usage: None,
            return_contract: None,
            waiting_user: false,
            waiting_approval: false,
            model: None,
            effort: None,
            created_at: self.created_at,
            completed_at: None,
            call_id: None,
            activity: None,
            pending: false,
            parts: vec![Part::Text {
                id: format!("{}:text", self.id),
                text: self.text.clone(),
            }],
            hidden_parts: Vec::new(),
            greeting: false,
            preview: None,
        }
    }

    /// The relay's row for this notice: a system line with the same facts (or, from a relay
    /// that writes none, the same words), written no earlier than the local one (minus
    /// clock skew).
    pub fn confirmed_by(&self, m: &Message) -> bool {
        if m.role != Role::System
            || m.id.starts_with(Self::ID_PREFIX)
            || m.created_at + Self::SKEW_MS < self.created_at
        {
            return false;
        }
        match (&self.notice, &m.notice) {
            (Some(mine), Some(theirs)) => mine == theirs,
            _ => m.plain_text().trim() == self.text,
        }
    }
}

/// "A", "A and B", "A, B and C" — the relay's `notices::join_names`, so the local line reads
/// exactly like the row that replaces it.
pub fn join_names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// "<who> added <names>"; `None` with nobody to name.
pub fn added_line(who: &str, names: &[String]) -> Option<String> {
    (!names.is_empty()).then(|| format!("{who} added {}", join_names(names)))
}

/// Fold this chat's local notices into a fresh snapshot from the relay: a notice the snapshot
/// confirms (same words, relay row) or one past its TTL is dropped; the rest are re-inserted in
/// time order so they survive the list replacement. Returns how many were confirmed.
pub fn reconcile_optimistic_notices(
    messages: &mut Vec<Message>,
    notices: &mut Vec<OptimisticNotice>,
    now: u64,
) -> usize {
    let mut confirmed = 0;
    notices.retain(|n| {
        if messages.iter().any(|m| n.confirmed_by(m)) {
            confirmed += 1;
            return false;
        }
        now.saturating_sub(n.created_at) < OptimisticNotice::TTL_MS
    });
    // Snapshots never carry local rows; a stale copy from an earlier merge must not double up.
    messages.retain(|m| !m.id.starts_with(OptimisticNotice::ID_PREFIX));
    let chat_id = messages
        .iter()
        .find(|m| !m.chat_id.is_empty())
        .map(|m| m.chat_id.clone())
        .unwrap_or_default();
    for n in notices.iter() {
        let at = messages
            .iter()
            .rposition(|m| m.created_at <= n.created_at)
            .map(|ix| ix + 1)
            .unwrap_or(0);
        messages.insert(at, n.message(&chat_id));
    }
    confirmed
}

#[cfg(test)]
mod optimistic_notice_tests {
    use super::*;

    fn relay_row(id: &str, text: &str, created_at: u64) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id, "chatId": "c", "role": "system", "sender": "system", "status": "done",
            "createdAt": created_at, "parts": [{ "kind": "text", "id": "p", "text": text }]
        }))
        .unwrap()
    }

    fn user_row(id: &str, created_at: u64) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id, "chatId": "c", "role": "user", "sender": "user:u_me", "status": "done",
            "createdAt": created_at, "parts": [{ "kind": "text", "id": "p", "text": "hi" }]
        }))
        .unwrap()
    }

    #[test]
    fn the_local_line_reads_like_the_relay_s() {
        assert_eq!(
            added_line("Jacob", &["Affine".to_string()]).as_deref(),
            Some("Jacob added Affine")
        );
        assert_eq!(
            added_line("Jacob", &["Home".into(), "hjkb".into(), "Doug".into()]).as_deref(),
            Some("Jacob added Home, hjkb and Doug")
        );
        assert_eq!(added_line("Jacob", &[]), None);
        let n = OptimisticNotice::new("Jacob added Affine".into(), 1_000, None);
        let m = n.message("c");
        assert_eq!(m.role, Role::System);
        assert_eq!(m.plain_text(), "Jacob added Affine");
        assert!(m.id.starts_with(OptimisticNotice::ID_PREFIX));
    }

    #[test]
    fn a_local_notice_survives_snapshots_until_the_relay_row_confirms_it() {
        let now = 100_000;
        let mut notices = vec![OptimisticNotice::new(
            "Jacob added Affine".into(),
            now,
            None,
        )];
        // Snapshot 1: something unrelated arrived; the local line stays, in time order.
        let mut snap = vec![user_row("m1", now - 5_000), user_row("m2", now + 300)];
        let confirmed = reconcile_optimistic_notices(&mut snap, &mut notices, now + 400);
        assert_eq!(confirmed, 0);
        assert_eq!(
            snap.iter().map(|m| m.plain_text()).collect::<Vec<_>>(),
            vec!["hi", "Jacob added Affine", "hi"]
        );
        assert_eq!(snap[1].chat_id, "c");
        assert_eq!(notices.len(), 1);
        // Snapshot 2: the relay's row (its clock a little behind ours) — the local copy goes,
        // the relay's stays, nothing doubles up.
        let mut snap = vec![
            user_row("m1", now - 5_000),
            user_row("m2", now + 300),
            relay_row("r1", "Jacob added Affine", now - 2_000),
        ];
        let confirmed = reconcile_optimistic_notices(&mut snap, &mut notices, now + 900);
        assert_eq!(confirmed, 1);
        assert!(notices.is_empty());
        assert_eq!(
            snap.iter()
                .filter(|m| m.plain_text() == "Jacob added Affine")
                .count(),
            1
        );
        assert!(snap
            .iter()
            .all(|m| !m.id.starts_with(OptimisticNotice::ID_PREFIX)));
    }

    #[test]
    fn other_notices_and_old_rows_do_not_confirm_and_a_stale_notice_expires() {
        let now = 100_000;
        let mut notices = vec![OptimisticNotice::new(
            "Jacob added Affine".into(),
            now,
            None,
        )];
        let mut snap = vec![
            relay_row("old", "Jacob added Affine", now - 60_000),
            relay_row("other", "Jacob removed Affine", now + 10),
        ];
        assert_eq!(
            reconcile_optimistic_notices(&mut snap, &mut notices, now + 100),
            0
        );
        assert_eq!(notices.len(), 1);
        assert_eq!(snap.len(), 3);
        // Past the TTL with no confirmation: the local line is withdrawn.
        let mut snap = vec![user_row("m1", now - 5_000)];
        reconcile_optimistic_notices(&mut snap, &mut notices, now + OptimisticNotice::TTL_MS);
        assert!(notices.is_empty());
        assert_eq!(snap.len(), 1);
    }
}

#[cfg(test)]
mod add_members_tests {
    use super::*;

    fn person(id: &str, handle: &str) -> Person {
        Person {
            id: id.into(),
            handle: handle.into(),
            status: "member".into(),
            ..Person::default()
        }
    }

    fn agent(project_id: &str) -> AgentMember {
        AgentMember {
            project_id: project_id.into(),
            name: project_id.into(),
            handle: project_id.into(),
            ..AgentMember::default()
        }
    }

    /// The group "Testing": me (unarbos), allan and const, plus one agent.
    fn testing_group() -> Project {
        Project {
            id: "chat_testing".into(),
            name: "Testing".into(),
            kind: SpaceKind::Group,
            people: vec![person("gh_1", "allan"), person("gh_2", "const")],
            agents: vec![agent("proj_lium")],
            ..Project::default()
        }
    }

    #[test]
    fn existing_members_count_as_already_in_the_group() {
        let g = testing_group();
        // The picker disables these rows: both humans, me, and the agent.
        assert!(g.has_person("unarbos", &person("gh_1", "allan")));
        assert!(g.has_person("unarbos", &person("gh_2", "const")));
        assert!(g.has_person("unarbos", &person("gh_me", "unarbos")));
        assert!(g.has_agent("proj_lium"));
        // A match by handle alone is enough (search results carry no stable id).
        assert!(g.has_person("unarbos", &person("", "allan")));
        // Someone new stays pickable.
        assert!(!g.has_person("unarbos", &person("gh_3", "alice")));
        assert!(!g.has_agent("proj_other"));
    }

    #[test]
    fn add_targets_this_group_and_skips_who_is_in_it() {
        let g = testing_group();
        let plan = add_members_plan(
            &g,
            "unarbos",
            &[
                person("gh_3", "alice"),
                person("gh_1", "allan"),
                person("gh_me", "unarbos"),
            ],
            &["proj_other".to_string(), "proj_lium".to_string()],
        );
        assert_eq!(
            plan.chat_id, "chat_testing",
            "adds to THIS group, not a new one"
        );
        assert_eq!(plan.handles, vec!["alice".to_string()]);
        assert_eq!(plan.agent_ids, vec!["proj_other".to_string()]);
    }

    #[test]
    fn created_group_is_the_new_group_with_exactly_the_picked_people() {
        let old = testing_group();
        let mut fresh = Project {
            id: "chat_new".into(),
            name: "alice, const".into(),
            kind: SpaceKind::Group,
            people: vec![person("gh_3", "alice"), person("gh_2", "const")],
            ..Project::default()
        };
        let before = vec![old.clone()];
        let after = vec![old.clone(), fresh.clone()];
        let picked = ["const".to_string(), "alice".to_string()];
        assert_eq!(
            created_group(&before, &after, &picked),
            Some("chat_new".into())
        );
        // An older group with the same people is not "the one just created".
        assert_eq!(created_group(&after, &after, &picked), None);
        // A superset of members is a different group.
        fresh.people.push(person("gh_9", "zed"));
        assert_eq!(created_group(&before, &[old.clone(), fresh], &picked), None);
        // Agents only: nothing to match on; the id event names the group.
        assert_eq!(created_group(&before, &after, &[]), None);
    }

    #[test]
    fn nothing_new_makes_an_empty_plan() {
        let g = testing_group();
        let plan = add_members_plan(&g, "unarbos", &[person("gh_1", "allan")], &[]);
        assert_eq!(plan.chat_id, "chat_testing");
        assert!(plan.handles.is_empty() && plan.agent_ids.is_empty());
    }
}

#[cfg(test)]
mod group_settings_permission_tests {
    //! The group pane's edit controls (picture crayon, inline name) follow the relay's
    //! rule: owner and admins only ([feedback group-settings-permissions-2026-10-27]).

    use super::*;

    fn group(my_role: Option<&str>, owner: &str) -> Project {
        Project {
            id: "g".into(),
            kind: SpaceKind::Group,
            owner_handle: Some(owner.into()),
            my_role: my_role.map(str::to_owned),
            ..Project::default()
        }
    }

    #[test]
    fn owner_and_admin_edit_members_do_not() {
        assert!(group(Some("owner"), "me").may_edit_group_settings("me"));
        assert!(group(Some("admin"), "alice").may_edit_group_settings("me"));
        assert!(!group(Some("member"), "alice").may_edit_group_settings("me"));
        // The relay's role wins over the handle match.
        assert!(!group(Some("member"), "me").may_edit_group_settings("me"));
    }

    #[test]
    fn without_a_role_only_the_owner_by_handle_edits() {
        assert!(group(None, "me").may_edit_group_settings("me"));
        assert!(!group(None, "alice").may_edit_group_settings("me"));
        let mut unknown_owner = group(None, "alice");
        unknown_owner.owner_handle = None;
        assert!(!unknown_owner.may_edit_group_settings("me"));
    }

    #[test]
    fn dms_and_projects_have_no_group_settings() {
        let mut dm = group(Some("owner"), "me");
        dm.kind = SpaceKind::Dm;
        assert!(!dm.may_edit_group_settings("me"));
        let mut project = group(Some("owner"), "me");
        project.kind = SpaceKind::Project;
        assert!(!project.may_edit_group_settings("me"));
    }
}

#[cfg(test)]
mod delete_permission_tests {
    use super::*;

    fn msg(sender: &str, role: Role) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": "m", "chatId": "c", "role": match role { Role::User => "user", Role::Assistant => "assistant", _ => "system" },
            "sender": sender, "status": "done", "parts": []
        }))
        .unwrap()
    }

    fn group(owner: &str) -> Project {
        Project {
            id: "g".into(),
            kind: SpaceKind::Group,
            owner_handle: Some(owner.into()),
            agents: vec![AgentMember {
                project_id: "p_bob".into(),
                owner_id: "u_bob".into(),
                owner_handle: "bob".into(),
                ..AgentMember::default()
            }],
            ..Project::default()
        }
    }

    #[test]
    fn member_deletes_own_only_owner_deletes_all() {
        let g = group("alice");
        // Bob, a member: his own yes, Alice's no, system no, his agent's yes.
        assert!(may_delete_message(
            &g,
            "u_bob",
            "bob",
            &msg("user:u_bob", Role::User)
        ));
        assert!(!may_delete_message(
            &g,
            "u_bob",
            "bob",
            &msg("user:u_alice", Role::User)
        ));
        assert!(!may_delete_message(
            &g,
            "u_bob",
            "bob",
            &msg("system", Role::System)
        ));
        assert!(may_delete_message(
            &g,
            "u_bob",
            "bob",
            &msg("agent:p_bob", Role::Assistant)
        ));
        // Carol: not even Bob's agent.
        assert!(!may_delete_message(
            &g,
            "u_carol",
            "carol",
            &msg("agent:p_bob", Role::Assistant)
        ));
        // Alice owns the group: everything.
        assert!(may_delete_message(
            &g,
            "u_alice",
            "alice",
            &msg("user:u_bob", Role::User)
        ));
        assert!(may_delete_message(
            &g,
            "u_alice",
            "alice",
            &msg("agent:p_bob", Role::Assistant)
        ));
        assert!(may_delete_message(
            &g,
            "u_alice",
            "alice",
            &msg("system", Role::System)
        ));
    }

    #[test]
    fn project_owner_deletes_all_guest_own_only() {
        let mine = Project {
            id: "p".into(),
            ..Project::default()
        };
        assert!(may_delete_message(
            &mine,
            "u_me",
            "me",
            &msg("agent", Role::Assistant)
        ));
        assert!(may_delete_message(
            &mine,
            "u_me",
            "me",
            &msg("user:u_guest", Role::User)
        ));
        let shared = Project {
            id: "p".into(),
            shared: true,
            ..Project::default()
        };
        assert!(may_delete_message(
            &shared,
            "u_guest",
            "guest",
            &msg("user:u_guest", Role::User)
        ));
        assert!(!may_delete_message(
            &shared,
            "u_guest",
            "guest",
            &msg("agent", Role::Assistant)
        ));
        assert!(!may_delete_message(
            &shared,
            "u_guest",
            "guest",
            &msg("user", Role::User)
        ));
    }
}

#[cfg(test)]
mod null_avatar_tests {
    use super::*;

    #[test]
    fn person_with_null_avatar_and_name_parses() {
        let v = serde_json::json!({
            "id": "test_1", "handle": "mactest-a", "avatar_url": null, "name": null, "status": "member"
        });
        let p: Person = serde_json::from_value(v).expect("null avatar_url is fine");
        assert_eq!(p.handle, "mactest-a");
        assert!(p.avatar_url.is_empty());
        assert_eq!(p.initial(), "M");
    }
}

#[cfg(test)]
mod group_call_ring_tests {
    //! #1430: a group call rings the whole group; the screen shows every member as a ringing
    //! tile until they join, not just the agent that picked up first.

    use super::*;

    fn person(id: &str, handle: &str) -> Person {
        Person {
            id: id.into(),
            handle: handle.into(),
            name: Some(handle.to_uppercase()),
            status: "member".into(),
            ..Person::default()
        }
    }

    fn arbonites() -> Project {
        Project {
            id: "chat_arbonites".into(),
            name: "Arbonites".into(),
            kind: SpaceKind::Group,
            people: vec![
                person("gh_const", "const"),
                person("gh_scout", "scouttao"),
                Person {
                    status: "invited".into(),
                    ..person("gh_ghost", "ghost")
                },
            ],
            agents: vec![AgentMember {
                project_id: "proj_home".into(),
                name: "Home".into(),
                handle: "home".into(),
                ..AgentMember::default()
            }],
            ..Project::default()
        }
    }

    #[test]
    fn a_group_call_rings_every_member_and_the_agent() {
        let expected = call_people(&arbonites(), "gh_me", &[]);
        let ids: Vec<String> = expected.iter().map(Sender::identity).collect();
        assert_eq!(ids, vec!["u_gh_const", "u_gh_scout", "a_proj_home"]);
        assert_eq!(expected[0].display_name(), "CONST");
        assert_eq!(expected[2].kind, "agent");
    }

    #[test]
    fn the_agent_joining_first_leaves_the_people_ringing() {
        let expected = call_people(&arbonites(), "gh_me", &[]);
        let ringing = still_ringing(&expected, &["a_proj_home".to_owned()]);
        let ids: Vec<String> = ringing.iter().map(Sender::identity).collect();
        assert_eq!(
            ids,
            vec!["u_gh_const", "u_gh_scout"],
            "Jacob's case: Home picked up, the people are still being rung"
        );
        let ringing = still_ringing(
            &expected,
            &["a_proj_home".to_owned(), "u_gh_scout".to_owned()],
        );
        assert_eq!(ringing.len(), 1);
        assert_eq!(ringing[0].identity(), "u_gh_const");
    }

    #[test]
    fn the_relay_s_agents_are_added_once_and_a_project_space_rings_its_agent() {
        let relay_agent = Sender {
            kind: "agent".into(),
            id: "proj_home".into(),
            name: "Home".into(),
            ..Sender::default()
        };
        let expected = call_people(&arbonites(), "gh_me", std::slice::from_ref(&relay_agent));
        assert_eq!(expected.iter().filter(|s| s.id == "proj_home").count(), 1);
        let project = Project {
            id: "proj_forest".into(),
            name: "Forest".into(),
            ..Project::default()
        };
        let expected = call_people(&project, "gh_me", &[]);
        assert_eq!(expected.len(), 1);
        assert_eq!(expected[0].identity(), "a_proj_forest");
        assert_eq!(expected[0].handle, "forest");
    }
}
