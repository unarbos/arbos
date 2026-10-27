//! Shared HTTP response shapes for projects, chats, people, me, machine,
//! unread, calls, goals, routines, secrets, vaults, sign-in, pairing, media,
//! pushes, releases, connectors, channels, skills, local devices, and feedback
//! (crdt-sync).
//!
//! No I/O. The relay builds these and serializes them; clients will decode the
//! same JSON. Regenerate Swift with `./scripts/gen-swift-types.sh`.
//!
//! typeshare does not accept bare `i64` (JS number precision); fields use
//! `serialized_as = "I54"` so Swift gets `Int64`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use typeshare::typeshare;

use crate::wire::BoxState;

/// Owner card on a project (`project_json` → `owner`), after presence fields.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectOwnerDto {
    pub kind: String,
    pub id: String,
    pub name: String,
    pub handle: String,
    pub avatar: Option<String>,
    pub avatar_color: Option<String>,
    pub avatar_url: Option<String>,
    #[typeshare(serialized_as = "Option<I54>")]
    pub avatar_color_index: Option<i64>,
    #[typeshare(serialized_as = "I54")]
    pub avatar_color_index_effective: i64,
    #[typeshare(serialized_as = "Option<I54>")]
    pub last_seen_at: Option<i64>,
    pub online: bool,
}

/// Machine line on a project (`project_json` → `machine`).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineDto {
    #[typeshare(serialized_as = "String")]
    pub state: BoxState,
    #[typeshare(serialized_as = "I54")]
    pub since: i64,
    pub class: String,
    pub snapshot: Option<String>,
    pub image_outdated: bool,
    pub always_on: bool,
}

/// Project record as clients see it (`project_json`), plus optional `list_projects` extras.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectDto {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub color: String,
    pub instructions: String,
    pub avatar: String,
    pub avatar_color: String,
    pub state: String,
    #[serde(rename = "box")]
    #[typeshare(serialized_as = "String")]
    pub box_state: BoxState,
    #[typeshare(serialized_as = "I54")]
    pub box_since: i64,
    #[typeshare(skip)]
    pub models: Value,
    pub spend_today_usd: f64,
    pub spend_cap_usd: f64,
    #[typeshare(serialized_as = "I54")]
    pub created_at: i64,
    #[typeshare(serialized_as = "I54")]
    pub last_active_at: i64,
    pub description: String,
    pub voice: Option<String>,
    pub vision_enabled: bool,
    #[typeshare(serialized_as = "Option<I54>")]
    pub avatar_color_index: Option<i64>,
    #[typeshare(serialized_as = "I54")]
    pub avatar_color_index_effective: i64,
    /// D92 leaf avatar: the owner's picks (`null` = automatic) and the pair in effect.
    #[typeshare(serialized_as = "Option<I54>")]
    pub avatar_leaf: Option<i64>,
    #[typeshare(serialized_as = "Option<I54>")]
    pub avatar_leaf_color: Option<i64>,
    /// Always sent by relays with D92; optional so older relays still decode.
    #[typeshare(serialized_as = "Option<I54>")]
    pub avatar_leaf_effective: Option<i64>,
    #[typeshare(serialized_as = "Option<I54>")]
    pub avatar_leaf_color_effective: Option<i64>,
    /// D92 amendment: the agent's uploaded / generated picture, a blob name in the project's own
    /// slot (`GET /blob/{project}/{key}`); wins over the leaf. Absent on older relays.
    #[serde(default)]
    pub image_key: Option<String>,
    /// Stamp (ms) of the agent's last avatar change (picture, leaf or colour pick). Clients
    /// hold several copies of one agent (project record, each roster); the highest stamp is
    /// the one drawn, so a change never flips back to the old picture. Absent on older relays.
    #[serde(default)]
    #[typeshare(serialized_as = "Option<I54>")]
    pub avatar_version: Option<i64>,
    pub primary_chat_id: Option<String>,
    pub owner: ProjectOwnerDto,
    pub model: Option<String>,
    pub machine: MachineDto,
    #[typeshare(skip)]
    pub activity: Value,
    /// `list_projects` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared: Option<bool>,
    /// `list_projects` only (string user id overwrites `owner` on the wire after serialize).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub my_role: Option<String>,
    /// `list_projects` shared rows only; `None` omits, `Some(None)` is JSON null.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_handle: Option<Option<String>>,
    /// `list_projects` for a signed-in device only: archived for this user (sidebar row hidden on
    /// every device; the agent keeps running). `None` omits, `Some(None)` is JSON null.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[typeshare(serialized_as = "Option<Option<I54>>")]
    pub archived_at: Option<Option<i64>>,
    /// "Who may message X" (agent-to-agent messaging): other owners' people / agents the
    /// owner allowed; empty for everyone but the owner's own clients.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub message_grants: Vec<MessageGrantDto>,
}

/// Project chat (`chat_json` / `chat_json_for`).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatDto {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub is_pinned: bool,
    pub is_system: bool,
    pub kind: String,
    pub participants: Vec<String>,
    #[typeshare(serialized_as = "I54")]
    pub position: i64,
    #[typeshare(serialized_as = "I54")]
    pub created_at: i64,
    #[typeshare(serialized_as = "I54")]
    pub doc_epoch: i64,
    pub room: String,
    /// Present only for `chat_json_for` (null or a timestamp).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[typeshare(serialized_as = "Option<Option<I54>>")]
    pub archived_at: Option<Option<i64>>,
    /// Present only for `chat_json_for`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub me: Option<ChatMeDto>,
}

/// `me` on a project chat or a people chat.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMeDto {
    /// People chats always send this (null when the viewer is not a member). Project chats omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
    /// People group: an admin muted this member from posting (not notification mute).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub silenced: Option<bool>,
    /// When my admin mute ends (ms). Absent or null while muted = until unmuted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[typeshare(serialized_as = "Option<I54>")]
    pub silenced_until: Option<i64>,
    #[typeshare(serialized_as = "Option<I54>")]
    pub archived_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_receipts: Option<bool>,
}

/// Nested agent project card inside a people-chat member.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMemberProjectDto {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub color: String,
    pub avatar: String,
    pub avatar_color: String,
    #[typeshare(serialized_as = "Option<I54>")]
    pub avatar_color_index: Option<i64>,
    #[typeshare(serialized_as = "I54")]
    pub avatar_color_index_effective: i64,
    #[typeshare(serialized_as = "Option<I54>")]
    pub avatar_leaf: Option<i64>,
    #[typeshare(serialized_as = "Option<I54>")]
    pub avatar_leaf_color: Option<i64>,
    /// Always sent by relays with D92; optional so older relays still decode.
    #[typeshare(serialized_as = "Option<I54>")]
    pub avatar_leaf_effective: Option<i64>,
    #[typeshare(serialized_as = "Option<I54>")]
    pub avatar_leaf_color_effective: Option<i64>,
    /// D92 amendment: the agent's uploaded / generated picture (blob name in `/blob/{id}/…`).
    #[serde(default)]
    pub image_key: Option<String>,
    /// Stamp (ms) of the last avatar change — the same value `ProjectDto.avatar_version`
    /// carries, so a roster copy and the owner's record can be ordered.
    #[serde(default)]
    #[typeshare(serialized_as = "Option<I54>")]
    pub avatar_version: Option<i64>,
    pub description: String,
    pub model: Option<String>,
    pub owner: ProjectOwnerDto,
}

/// People-chat member: human and agent fields on one struct (absent keys for the other variant).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMemberDto {
    pub member: String,
    pub role: String,
    pub added_by: String,
    #[typeshare(serialized_as = "I54")]
    pub joined_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    /// Human: always present (null or URL). Agent: omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[typeshare(serialized_as = "Option<Option<I54>>")]
    pub last_seen_at: Option<Option<i64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub online: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_receipts: Option<bool>,
    /// Human in a people group: admin muted them from posting. Agent: omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub silenced: Option<bool>,
    /// When the admin mute ends (ms). Absent or null while muted = until unmuted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[typeshare(serialized_as = "Option<I54>")]
    pub silenced_until: Option<i64>,
    /// Human admin: owner granted mute/ban rights (default true). Agent / member: omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_mute_ban: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[typeshare(serialized_as = "Option<Option<I54>>")]
    pub avatar_color_index: Option<Option<i64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[typeshare(serialized_as = "Option<I54>")]
    pub avatar_color_index_effective: Option<i64>,
    /// Agent: D92 leaf picks and the pair in effect. Human: omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[typeshare(serialized_as = "Option<Option<I54>>")]
    pub avatar_leaf: Option<Option<i64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[typeshare(serialized_as = "Option<Option<I54>>")]
    pub avatar_leaf_color: Option<Option<i64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[typeshare(serialized_as = "Option<I54>")]
    pub avatar_leaf_effective: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[typeshare(serialized_as = "Option<I54>")]
    pub avatar_leaf_color_effective: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<ChatMemberProjectDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Agent: owning user id string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_handle: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[typeshare(skip)]
    pub settings: Option<Value>,
}

/// People chat (`people::chat_json`).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeopleChatDto {
    pub id: String,
    #[typeshare(serialized_as = "Option<I54>")]
    pub archived_at: Option<i64>,
    pub kind: String,
    pub created_by: String,
    #[typeshare(serialized_as = "I54")]
    pub created_at: i64,
    pub title: String,
    pub image_key: Option<String>,
    /// D92 amendment: the group's leaf picture, when one was picked (`image_key` wins).
    #[serde(default)]
    #[typeshare(serialized_as = "Option<I54>")]
    pub avatar_leaf: Option<i64>,
    #[serde(default)]
    #[typeshare(serialized_as = "Option<I54>")]
    pub avatar_leaf_color: Option<i64>,
    /// Optional about text for a people group (≤200 chars). Absent or empty = none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Group policy: when false, members (non-owner/admin) do not see who has
    /// read messages. Owners and admins still can. Absent → on (default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub members_see_reads: Option<bool>,
    pub pending: bool,
    pub members: Vec<ChatMemberDto>,
    /// Banned people (owner/admin with mute-ban rights); empty for everyone else.
    /// Unban from the member list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub banned: Vec<ChatMemberDto>,
    pub room: String,
    #[typeshare(serialized_as = "I54")]
    pub head_seq: i64,
    #[typeshare(serialized_as = "I54")]
    pub last_activity_at: i64,
    #[typeshare(serialized_as = "I54")]
    pub read_seq: i64,
    pub unread: bool,
    pub lock_state: String,
    pub me: ChatMeDto,
}

/// One row in `project_members_json` → `members`.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectMemberDto {
    pub id: String,
    pub handle: Option<String>,
    pub avatar_url: Option<String>,
    pub name: Option<String>,
    pub status: String,
    #[typeshare(serialized_as = "I54")]
    pub created_at: i64,
    pub user_id: String,
    pub role: String,
    #[typeshare(serialized_as = "I54")]
    pub joined_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_by: Option<String>,
    #[typeshare(serialized_as = "Option<I54>")]
    pub last_seen_at: Option<i64>,
    pub online: bool,
    #[typeshare(serialized_as = "I54")]
    pub avatar_color_index_effective: i64,
}

/// `project_members_json`.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectMembersDto {
    pub project_id: String,
    pub owner: String,
    pub members: Vec<ProjectMemberDto>,
}

/// One row of an agent's "Who may message X" card (agent-to-agent messaging): a person
/// (`kind: "person"`, every agent of theirs) or one agent (`kind: "agent"`) of another owner.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageGrantDto {
    pub project_id: String,
    /// `user:<id>` or `project:<id>`, the key for PUT / DELETE.
    pub grantee: String,
    pub kind: String,
    pub id: String,
    pub name: Option<String>,
    pub handle: Option<String>,
    pub avatar_url: Option<String>,
    pub enabled: bool,
    pub note: String,
    #[typeshare(serialized_as = "I54")]
    pub created_at: i64,
    /// Last time a message came through this grant.
    #[typeshare(serialized_as = "Option<I54>")]
    pub last_at: Option<i64>,
}

/// `GET /projects/{id}/message-access`.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageGrantsDto {
    pub project_id: String,
    pub grants: Vec<MessageGrantDto>,
}

/// One entry in `group_json` → `participants_detail`.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupParticipantDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon: String,
    pub color: String,
    #[serde(rename = "box")]
    #[typeshare(serialized_as = "String")]
    pub box_state: BoxState,
}

/// Group chat (`group_json`).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupDto {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub is_pinned: bool,
    pub is_system: bool,
    pub kind: String,
    pub participants: Vec<String>,
    #[typeshare(serialized_as = "I54")]
    pub position: i64,
    #[typeshare(serialized_as = "I54")]
    pub created_at: i64,
    #[typeshare(serialized_as = "I54")]
    pub doc_epoch: i64,
    pub room: String,
    pub participants_detail: Vec<GroupParticipantDto>,
}

/// Sandbox block on `create_project`.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateProjectSandboxDto {
    pub provider: String,
    pub sandbox_id: Option<String>,
    pub device_id: Option<String>,
    pub box_url: Option<String>,
}

/// Room URLs on `create_project`.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateProjectRoomsDto {
    pub project: String,
    pub pinned_chat: String,
}

/// `POST /projects` success body.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateProjectResponseDto {
    pub project: ProjectDto,
    pub pinned_chat: ChatDto,
    pub sandbox: CreateProjectSandboxDto,
    pub rooms: CreateProjectRoomsDto,
}

/// `GET /projects/{id}` success body.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GetProjectResponseDto {
    pub project: ProjectDto,
    pub chats: Vec<ChatDto>,
    #[serde(rename = "box")]
    #[typeshare(serialized_as = "String")]
    pub box_state: BoxState,
    #[typeshare(serialized_as = "I54")]
    pub box_since: i64,
    #[typeshare(skip)]
    pub spend_today: Value,
    pub skills_rev: String,
}

/// `POST /projects/{id}/chats` success body.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateChatResponseDto {
    pub chat: ChatDto,
    pub room: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<bool>,
}

/// Public person card (`user_card`, directory, search, GitHub fallback, + presence).
///
/// Optional keys stay omitted when a builder does not set them
/// (`skip_serializing_if`), so no card gains keys it did not have before.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct UserCardDto {
    pub id: String,
    pub handle: String,
    pub avatar_url: Option<String>,
    pub name: Option<String>,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[typeshare(serialized_as = "Option<I54>")]
    pub created_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test: Option<bool>,
    /// Absent before `with_presence`; after it, `Some(None)` is JSON null.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[typeshare(serialized_as = "Option<Option<I54>>")]
    pub last_seen_at: Option<Option<i64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub online: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[typeshare(serialized_as = "Option<I54>")]
    pub avatar_color_index_effective: Option<i64>,
}

/// `user` on `/auth/token`, `/me`, and profile update replies.
///
/// Full rows always send `github_id`/`login`/… (null when unset). The
/// fallback `{ "id" }` omits those keys (`None` outer Option).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountUserDto {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[typeshare(serialized_as = "Option<Option<I54>>")]
    pub github_id: Option<Option<i64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[typeshare(serialized_as = "Option<I54>")]
    pub created_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[typeshare(serialized_as = "Option<Option<I54>>")]
    pub last_login_at: Option<Option<i64>>,
}

/// Credits block on `GET /me`.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreditsDto {
    pub balance_usd: f64,
    pub granted_usd: f64,
    pub spent_usd: f64,
    pub spent_today_usd: f64,
    pub daily_cap_usd: Option<f64>,
    pub allow_negative: bool,
    pub machine_today_usd: f64,
    pub machine_total_usd: f64,
}

/// `limits` on `GET /me`.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeLimitsDto {
    pub read_only: bool,
    pub read_only_reason: Option<String>,
    pub signup: String,
    pub auth_mode: String,
}

/// `machine` summary on `GET /me`.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeMachineDto {
    pub exists: bool,
    #[typeshare(serialized_as = "String")]
    pub state: BoxState,
    #[typeshare(serialized_as = "I54")]
    pub since: i64,
    pub provider: Option<String>,
    pub sandbox_id: Option<String>,
    pub always_on: bool,
    pub held: bool,
    pub waitlisted: bool,
    #[typeshare(serialized_as = "Option<I54>")]
    pub waitlist_position: Option<i64>,
    pub evicted: bool,
}

/// Device row on `GET /me` (the caller's device when known).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeDeviceDto {
    pub id: String,
    pub name: String,
    #[typeshare(serialized_as = "I54")]
    pub created_at: i64,
    #[typeshare(serialized_as = "Option<I54>")]
    pub revoked_at: Option<i64>,
    #[typeshare(serialized_as = "Option<I54>")]
    pub last_seen_at: Option<i64>,
    pub kind: String,
}

/// Remote feature flags on `GET /me` (and `GET /secrets`). Every key is optional on the
/// wire so a client decodes an older relay that does not know a flag; the relay always sends
/// all of them. Key names are a contract with shipped apps: only add, never rename.
#[typeshare]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MeFeaturesDto {
    /// #1265: the Connect 1Password flow (default off).
    #[serde(default)]
    pub vault_connect: Option<bool>,
    /// D92: agents without a photo draw a leaf, not initials (default on).
    #[serde(default)]
    pub leaf_avatars: Option<bool>,
    /// D92 follow-on: the leaves play their committed Lottie (idle loop + gust) instead of the
    /// native sway (default on).
    #[serde(default)]
    pub leaf_lottie: Option<bool>,
    /// Voice mode — spoken replies, the voice picker, reply playback (default off since
    /// 2026-10-22, Jacob: hidden everywhere until it works end to end). Absent = off.
    #[serde(default)]
    pub voice_mode: Option<bool>,
}

/// Privacy choices on `GET /me` / `PATCH /me`. Absent on an older relay → open defaults.
/// Key names are a contract with shipped apps: only add, never rename.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MePrivacyDto {
    /// Who may add me to a people group: `anyone` | `people_i_know` | `nobody`.
    pub group_add: String,
}

impl Default for MePrivacyDto {
    fn default() -> Self {
        Self {
            group_add: "anyone".into(),
        }
    }
}

/// `GET /me` success body.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeResponseDto {
    pub user: AccountUserDto,
    pub device: Option<MeDeviceDto>,
    pub credits: CreditsDto,
    pub limits: MeLimitsDto,
    #[serde(default)]
    pub features: MeFeaturesDto,
    pub machine: MeMachineDto,
    #[serde(default)]
    pub privacy: MePrivacyDto,
}

/// `PATCH /me` and avatar route replies: `user`, optional `avatar_url`.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeUserResponseDto {
    pub user: AccountUserDto,
    /// Present on avatar PUT/DELETE (`null` when cleared to no GitHub picture).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<Option<String>>,
}

/// `GET /users` directory page.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsersPageDto {
    pub users: Vec<UserCardDto>,
    #[typeshare(serialized_as = "I54")]
    pub total: i64,
    #[typeshare(serialized_as = "I54")]
    pub offset: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[typeshare(serialized_as = "Option<I54>")]
    pub next_offset: Option<i64>,
}

/// One unread row on `GET /me/unread`.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnreadChatDto {
    pub chat_id: String,
    pub project_id: String,
    #[typeshare(serialized_as = "I54")]
    pub unread: i64,
}

/// `GET /me/unread`.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnreadDto {
    pub chats: Vec<UnreadChatDto>,
    #[typeshare(serialized_as = "I54")]
    pub total: i64,
}

/// Spec line on `GET /machine`.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineSpecDto {
    #[typeshare(serialized_as = "I54")]
    pub cpu: i64,
    #[typeshare(serialized_as = "I54")]
    pub mem_gib: i64,
    #[typeshare(serialized_as = "I54")]
    pub disk_gib: i64,
    pub class: String,
}

/// One workspace row on `GET /machine`.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineWorkspaceDto {
    pub project_id: String,
    pub state: String,
}

/// Image block on `GET /machine`.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineImageDto {
    pub current: Option<String>,
    pub default: String,
}

/// `GET /machine` success body.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineStatusDto {
    pub user_id: String,
    #[typeshare(serialized_as = "String")]
    pub state: BoxState,
    #[typeshare(serialized_as = "I54")]
    pub since: i64,
    pub provider: Option<String>,
    pub sandbox_id: Option<String>,
    pub spec: MachineSpecDto,
    pub spend_today_usd: f64,
    pub spend_cap_usd: f64,
    pub workspaces: Vec<MachineWorkspaceDto>,
    pub migrated_from: Vec<String>,
    pub image: MachineImageDto,
    pub image_outdated: bool,
    #[typeshare(skip)]
    pub upgrade: Value,
    pub always_on: bool,
    pub held: bool,
    pub machine_today_usd: f64,
}

/// `POST /machine/start|stop|restart` reply.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineStateDto {
    #[typeshare(serialized_as = "String")]
    pub state: BoxState,
    #[typeshare(serialized_as = "I54")]
    pub since: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<bool>,
}

/// `sender{}` on calls, pushes, and LiveKit participant metadata (`Sender::json`).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SenderDto {
    pub kind: String,
    pub id: String,
    pub name: String,
    pub handle: String,
    pub avatar: Option<String>,
    pub avatar_color: Option<String>,
    pub avatar_url: Option<String>,
    #[typeshare(serialized_as = "Option<I54>")]
    pub avatar_color_index: Option<i64>,
    #[typeshare(serialized_as = "I54")]
    pub avatar_color_index_effective: i64,
    /// D92: agents get the leaf pair in effect; people send JSON null.
    #[typeshare(serialized_as = "Option<I54>")]
    pub avatar_leaf_effective: Option<i64>,
    #[typeshare(serialized_as = "Option<I54>")]
    pub avatar_leaf_color_effective: Option<i64>,
}

/// One device under `reachable[].devices` on a start-call reply.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReachableDeviceDto {
    pub device_id: String,
    pub name: String,
    pub kind: String,
    pub platform: Option<String>,
    pub app_version: Option<String>,
    pub polled_within_30s: bool,
    #[typeshare(serialized_as = "Option<I54>")]
    pub last_seen_at: Option<i64>,
    pub voip_token: bool,
    pub push_token: bool,
}

/// One callee in `reachable[]` on a 201 start-call reply.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallReachabilityDto {
    pub user_id: String,
    pub handle: String,
    pub name: String,
    pub any_live: bool,
    pub any_voip: bool,
    pub any_push: bool,
    pub devices: Vec<ReachableDeviceDto>,
}

/// Call object (`call_json`): token fields only when the relay mints a join token.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallDto {
    pub call_id: String,
    pub chat_id: String,
    pub project_id: String,
    pub kind: String,
    pub room: String,
    pub livekit_url: Option<String>,
    #[typeshare(serialized_as = "I54")]
    pub max_participants: i64,
    pub started_by: SenderDto,
    pub participants: Vec<SenderDto>,
    pub agents: Vec<SenderDto>,
    #[typeshare(serialized_as = "I54")]
    pub started_at: i64,
    #[typeshare(serialized_as = "Option<I54>")]
    pub ended_at: Option<i64>,
    pub reason: Option<String>,
    pub started_device: Option<String>,
    pub platform: Option<String>,
    pub app_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[typeshare(serialized_as = "Option<I54>")]
    pub token_expires_at: Option<i64>,
    /// `"open"` = anyone may speak; `"hosted"` = only the host and invited speakers (bigger groups).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speak_mode: Option<String>,
    /// People the host invited to speak. The host is `started_by` and is not listed here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invited_speakers: Vec<SenderDto>,
    /// Whether the requesting user may publish audio (set when a join token is minted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_speak: Option<bool>,
}

/// `POST …/calls` success: new call (201) or join the live one (200).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StartCallResponseDto {
    pub call: CallDto,
    pub joined: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reachable: Option<Vec<CallReachabilityDto>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[typeshare(skip)]
    pub agents_dispatched: Option<Value>,
}

/// `{ "call": … }` envelope (`GET /calls/{id}`, join, active-call).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallEnvelopeDto {
    pub call: CallDto,
}

/// `POST /calls/{id}/leave`.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeaveCallResponseDto {
    pub ended: bool,
    #[typeshare(serialized_as = "I54")]
    pub duration_s: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub already_ended: Option<bool>,
}

/// `POST /devices/{id}/voip`.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoipRegisterResponseDto {
    pub device_id: String,
    pub push: bool,
}

/// `/me/pushes` `call_started` payload.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallStartedPayloadDto {
    pub call: CallDto,
    pub sender: SenderDto,
}

/// `/me/pushes` `call_ended` payload.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallEndedPayloadDto {
    pub call_id: String,
    pub chat_id: String,
    pub project_id: String,
    #[typeshare(serialized_as = "I54")]
    pub duration_s: i64,
    pub reason: String,
    pub writer_device: Option<String>,
}

/// `/me/pushes` `call_missed` payload (`from` is the starter).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallMissedPayloadDto {
    pub call_id: String,
    pub chat_id: String,
    pub project_id: String,
    pub kind: String,
    pub from: SenderDto,
    #[typeshare(serialized_as = "I54")]
    pub started_at: i64,
    #[typeshare(serialized_as = "I54")]
    pub ended_at: i64,
    #[typeshare(serialized_as = "I54")]
    pub duration_s: i64,
    pub reason: Option<String>,
}

/// PushKit VoIP wake payload (`caller` is the starter).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallRingPayloadDto {
    pub call_id: String,
    pub chat_id: String,
    pub project_id: String,
    pub kind: String,
    pub room: String,
    pub livekit_url: Option<String>,
    pub caller: SenderDto,
    #[typeshare(serialized_as = "I54")]
    pub started_at: i64,
}

/// APNs `ARBOS_CALL` alert body without `aps` (`sender` on started/missed only).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallAlertDto {
    pub event: String,
    pub chat_id: String,
    pub project_id: String,
    pub call: CallDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<SenderDto>,
}

/// Goal as clients see it (`goal_json`): `db::Goal` plus optional `project_name`.
///
/// `plan`, `progress_log`, and the `linked_*_ids` lists stay free-form JSON
/// (`#[typeshare(skip)]`); Swift still reads them from the raw dict.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoalDto {
    pub id: String,
    pub project_id: String,
    pub chat_id: Option<String>,
    pub title: String,
    pub outcome: String,
    #[typeshare(skip)]
    pub plan: Value,
    pub status: String,
    #[typeshare(skip)]
    pub progress_log: Value,
    pub next_step: String,
    pub check_in_cron: Option<String>,
    pub check_in_tz: String,
    pub routine_id: Option<String>,
    pub autonomy: String,
    #[typeshare(skip)]
    pub linked_chat_ids: Value,
    #[typeshare(skip)]
    pub linked_subagent_ids: Value,
    pub waiting_question: Option<String>,
    #[typeshare(serialized_as = "I54")]
    pub created_at: i64,
    #[typeshare(serialized_as = "I54")]
    pub updated_at: i64,
    #[typeshare(serialized_as = "Option<I54>")]
    pub last_moved_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
}

/// `GET /goals` and `GET /projects/{id}/goals` body.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoalsListDto {
    pub goals: Vec<GoalDto>,
}

/// Routine as clients see it (`routine_json`): `db::Routine` plus optional `fire_url`.
///
/// `trigger` stays free-form JSON (`#[typeshare(skip)]`).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutineDto {
    pub id: String,
    pub project_id: String,
    pub chat_id: String,
    pub name: String,
    pub prompt: String,
    #[typeshare(skip)]
    pub trigger: Value,
    pub enabled: bool,
    #[typeshare(serialized_as = "I54")]
    pub created_at: i64,
    #[typeshare(serialized_as = "Option<I54>")]
    pub last_run_at: Option<i64>,
    #[typeshare(serialized_as = "Option<I54>")]
    pub next_run_at: Option<i64>,
    /// Webhook routines only (omitted otherwise).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fire_url: Option<String>,
}

/// One routine history row (`db::RoutineRun`).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutineRunDto {
    pub id: String,
    pub routine_id: String,
    #[typeshare(serialized_as = "I54")]
    pub started_at: i64,
    #[typeshare(serialized_as = "Option<I54>")]
    pub finished_at: Option<i64>,
    pub status: String,
    pub message_id: Option<String>,
    pub note: Option<String>,
}

/// `POST /routines/{rid}/fire` reply.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FireRoutineResponseDto {
    pub run_id: String,
}

// ---- Secrets / vaults ---------------------------------------------------------------------------
//
// Free-form on the wire (`#[typeshare(skip)]` + `Value`): activity `detail`, vault `items`,
// vault `external_vaults`. Fixed-key objects the relay builds are real structs.

/// Access scope on a secret or vault access row.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecretAccessScopeDto {
    pub kind: String,
    pub chat_id: Option<String>,
}

/// One pinned host on a vault access (`item_hosts[]`).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VaultItemHostDto {
    #[serde(rename = "ref")]
    pub item_ref: String,
    pub host: String,
}

/// Access row (`access_json`): secret accesses omit `item_hosts`; vault accesses include it.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecretAccessDto {
    pub project_id: String,
    pub name: String,
    pub access_id: String,
    pub enabled: bool,
    pub note: String,
    pub hosts: Vec<String>,
    pub scope: SecretAccessScopeDto,
    pub created_from: String,
    #[typeshare(serialized_as = "I54")]
    pub created_at: i64,
    #[typeshare(serialized_as = "Option<I54>")]
    pub last_used_at: Option<i64>,
    #[typeshare(serialized_as = "Option<I54>")]
    pub disabled_at: Option<i64>,
    /// Vault accesses only (omitted on secret accesses).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_hosts: Option<Vec<VaultItemHostDto>>,
}

/// One person on a secret's People list (`secret.people[]`; `secret.directors[]` is the same
/// list under its #1041 name, kept one release). The new fields are optional so a client built
/// after this change still reads a relay built before it.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecretDirectorDto {
    pub user_id: String,
    pub handle: String,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
    pub note: String,
    #[typeshare(serialized_as = "I54")]
    pub created_at: i64,
    /// Row switch (absent = on).
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    #[typeshare(serialized_as = "Option<I54>")]
    pub disabled_at: Option<i64>,
    /// `all` or `chat` (absent = all).
    #[serde(default)]
    pub scope: Option<SecretAccessScopeDto>,
    #[serde(default)]
    #[typeshare(serialized_as = "Option<I54>")]
    pub last_asked_at: Option<i64>,
    /// `settings | card:<request id>` (absent = settings).
    #[serde(default)]
    pub created_from: Option<String>,
}

/// Where a secret came from: `settings` (the New secret form) or `chat` (handed to an agent in
/// a chat — the request card or "save this"). Names and ids only.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecretSourceDto {
    /// `settings | chat`.
    pub kind: String,
    /// The agent it was given to, when `chat`.
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    pub chat_id: Option<String>,
}

/// Secret as clients see it (`secret_json`). Never carries a secret value.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecretDto {
    pub id: String,
    pub name: String,
    pub env: String,
    pub kind: String,
    pub note: String,
    pub hosts: Vec<String>,
    pub ask_every_time: bool,
    pub inject_env: bool,
    #[typeshare(serialized_as = "Option<I54>")]
    pub expires_at: Option<i64>,
    #[typeshare(serialized_as = "I54")]
    pub version: i64,
    #[typeshare(serialized_as = "I54")]
    pub created_at: i64,
    #[typeshare(serialized_as = "I54")]
    pub updated_at: i64,
    #[typeshare(serialized_as = "Option<I54>")]
    pub last_used_at: Option<i64>,
    /// Login kind only: username metadata (not a secret). Otherwise JSON null.
    pub username: Option<String>,
    pub vault_name: String,
    #[serde(rename = "ref")]
    pub secret_ref: String,
    pub agents: Vec<SecretAccessDto>,
    /// Same list as `people`; the #1041 name, kept one release.
    pub directors: Vec<SecretDirectorDto>,
    /// People who may ask an agent with access to use this secret (spec §7.7).
    #[serde(default)]
    pub people: Vec<SecretDirectorDto>,
    /// `refuse | card`: what a non-listed person's ask does (absent = refuse).
    #[serde(default)]
    pub others_ask: Option<String>,
    /// Where it came from (absent on an older relay = settings).
    #[serde(default)]
    pub source: Option<SecretSourceDto>,
}

/// Target block on a secret request.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecretRequestTargetDto {
    pub kind: String,
    pub id: Option<String>,
    #[serde(rename = "ref")]
    pub target_ref: String,
}

/// Pending / decided request card (`request_json`).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecretRequestDto {
    pub id: String,
    pub project_id: String,
    pub project_name: Option<String>,
    pub chat_id: String,
    pub target: SecretRequestTargetDto,
    pub name: String,
    pub purpose: String,
    #[serde(rename = "use")]
    pub use_kind: String,
    pub host: Option<String>,
    pub status: String,
    pub decided_by: Option<String>,
    #[typeshare(serialized_as = "I54")]
    pub created_at: i64,
    #[typeshare(serialized_as = "Option<I54>")]
    pub decided_at: Option<i64>,
    #[typeshare(serialized_as = "I54")]
    pub expires_at: i64,
    /// The person who asked the agent, when it was not the owner (spec §7.7).
    #[serde(default)]
    pub asked_by: Option<String>,
    #[serde(default)]
    pub asked_by_handle: Option<String>,
    #[serde(default)]
    pub asked_by_name: Option<String>,
}

/// One audit row (`activity[]`). `detail` stays free-form (`#[typeshare(skip)]`).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecretActivityDto {
    #[typeshare(serialized_as = "I54")]
    pub id: i64,
    pub user_id: String,
    #[typeshare(serialized_as = "I54")]
    pub at: i64,
    pub action: String,
    pub target_kind: String,
    pub target_id: String,
    pub target_name: String,
    /// Overlay of `target_name` for the UI.
    pub name: String,
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    pub chat_id: Option<String>,
    pub request_id: Option<String>,
    pub access_id: Option<String>,
    pub use_kind: Option<String>,
    pub host: Option<String>,
    pub by: String,
    #[typeshare(skip)]
    pub detail: Value,
}

/// Rate-limit block on a connected vault.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VaultLimitsDto {
    #[typeshare(serialized_as = "I54")]
    pub reads_per_hour: i64,
    #[typeshare(serialized_as = "I54")]
    pub reads_used_hour: i64,
    #[typeshare(serialized_as = "I54")]
    pub reads_per_day: i64,
    #[typeshare(serialized_as = "I54")]
    pub reads_used_day: i64,
    #[typeshare(serialized_as = "I54")]
    pub cache_ttl_secs: i64,
}

/// Connected vault (`vault_json`). `external_vaults` and `items` stay free-form.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VaultDto {
    pub id: String,
    pub provider: String,
    pub name: String,
    #[typeshare(skip)]
    pub external_vaults: Value,
    #[typeshare(serialized_as = "I54")]
    pub items_count: i64,
    #[typeshare(serialized_as = "Option<I54>")]
    pub items_refreshed_at: Option<i64>,
    #[typeshare(serialized_as = "Option<I54>")]
    pub token_expires_at: Option<i64>,
    pub limits: VaultLimitsDto,
    pub status: String,
    pub last_error: Option<String>,
    #[typeshare(serialized_as = "Option<I54>")]
    pub last_refreshed_at: Option<i64>,
    #[typeshare(serialized_as = "I54")]
    pub created_at: i64,
    pub agents: Vec<SecretAccessDto>,
    #[typeshare(skip)]
    pub items: Value,
    #[typeshare(serialized_as = "I54")]
    pub env_items: i64,
}

/// Reduced secret nested on `GET /projects/{id}/secrets` items.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectSecretSummaryDto {
    pub id: String,
    pub name: String,
    pub env: String,
    pub kind: String,
    pub note: String,
    pub hosts: Vec<String>,
    #[typeshare(serialized_as = "Option<I54>")]
    pub last_used_at: Option<i64>,
    pub vault_name: String,
    pub inject_env: bool,
}

/// Reduced vault nested on a project-secrets vault row.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectVaultSummaryDto {
    pub id: String,
    pub provider: String,
    pub name: String,
    #[typeshare(serialized_as = "I54")]
    pub items_count: i64,
    pub status: String,
}

/// One row of `GET /projects/{id}/secrets` → `items[]`.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectSecretItemDto {
    pub project_id: String,
    pub name: String,
    pub access_id: String,
    pub enabled: bool,
    pub note: String,
    pub hosts: Vec<String>,
    pub scope: SecretAccessScopeDto,
    pub created_from: String,
    #[typeshare(serialized_as = "I54")]
    pub created_at: i64,
    #[typeshare(serialized_as = "Option<I54>")]
    pub last_used_at: Option<i64>,
    #[typeshare(serialized_as = "Option<I54>")]
    pub disabled_at: Option<i64>,
    /// Vault rows only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_hosts: Option<Vec<VaultItemHostDto>>,
    /// Vault rows only (`"vault"`); secret rows omit the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<ProjectSecretSummaryDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault: Option<ProjectVaultSummaryDto>,
}

/// `GET /secrets` body.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecretsOverviewDto {
    pub paused: bool,
    #[typeshare(serialized_as = "Option<I54>")]
    pub paused_at: Option<i64>,
    pub paused_by: Option<String>,
    pub vaults: Vec<VaultDto>,
    pub providers: Vec<String>,
    pub features: MeFeaturesDto,
    pub secrets: Vec<SecretDto>,
    #[typeshare(serialized_as = "I54")]
    pub requests_pending: i64,
    pub requests: Vec<SecretRequestDto>,
    pub activity: Vec<SecretActivityDto>,
}

/// `GET /projects/{id}/secrets` body.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectSecretsDto {
    pub paused: bool,
    pub owner: bool,
    pub items: Vec<ProjectSecretItemDto>,
    pub requests: Vec<SecretRequestDto>,
    pub activity: Vec<SecretActivityDto>,
}

/// `{ secret }` create / patch / value / directors / revoke / scrub replies.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecretEnvelopeDto {
    pub secret: SecretDto,
}

/// `GET /secrets/{id}` → `{ secret, activity }`.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecretDetailDto {
    pub secret: SecretDto,
    pub activity: Vec<SecretActivityDto>,
}

/// `{ access }` after access PUT.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecretAccessEnvelopeDto {
    pub access: SecretAccessDto,
}

/// Kill-switch reply (`set_paused` / `PUT /me/secrets/settings`).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecretsPausedDto {
    pub paused: bool,
    #[typeshare(serialized_as = "Option<I54>")]
    pub paused_at: Option<i64>,
    pub paused_by: Option<String>,
}

/// `GET /secrets/activity` page.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecretActivityPageDto {
    pub items: Vec<SecretActivityDto>,
    #[typeshare(serialized_as = "Option<I54>")]
    pub next_before: Option<i64>,
}

/// `GET /secrets/requests` → `{ items }`.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecretRequestsDto {
    pub items: Vec<SecretRequestDto>,
}

/// `POST /secrets/requests/{id}/decide` → `{ request, access }` (`access` may be JSON null).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecideSecretResponseDto {
    pub request: SecretRequestDto,
    pub access: Option<SecretAccessDto>,
}

/// `{ vault }` connect reply.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VaultEnvelopeDto {
    pub vault: VaultDto,
}

/// `GET /vaults/{id}` → `{ vault, items, activity }`. `items` is free-form.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VaultDetailDto {
    pub vault: VaultDto,
    #[typeshare(skip)]
    pub items: Value,
    pub activity: Vec<SecretActivityDto>,
}

/// `{ vault, items }` after env toggle. `items` is free-form.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VaultItemsEnvelopeDto {
    pub vault: VaultDto,
    #[typeshare(skip)]
    pub items: Value,
}

// ---- Sign-in / pairing / media ------------------------------------------------------------------
//
// Pairing and token replies carry device / access tokens — fixtures use obviously fake values.
// Upstream passthroughs (`ModelListDto`, `TranscriptionDto`) are read-only; unknown keys ignored.

/// `POST /pair` success body.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairResponseDto {
    pub device_id: String,
    pub device_token: String,
}

/// `POST /auth/token` and `POST /auth/refresh` success body.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenResponseDto {
    pub access_token: String,
    pub token_type: String,
    #[typeshare(serialized_as = "I54")]
    pub expires_in: i64,
    pub refresh_token: String,
    pub device_id: String,
    pub user: AccountUserDto,
}

/// Provider flags on `GET /healthz`.
#[typeshare]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HealthProvidersDto {
    pub openai_compatible: bool,
    pub anthropic: bool,
}

/// `GET /healthz` body. Clients read `auth_mode` (and legacy `auth`); other keys stay for probes.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthDto {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    #[typeshare(serialized_as = "I54")]
    pub proto: i64,
    #[serde(default)]
    #[typeshare(serialized_as = "I54")]
    pub devices: i64,
    #[serde(default)]
    pub daytona: bool,
    /// Present on current relays; absent on very old ones that only had `auth`.
    #[serde(default)]
    pub auth_mode: Option<String>,
    /// Older relays only (`"github"`); omitted on current relays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<String>,
    #[serde(default)]
    pub providers: HealthProvidersDto,
}

/// One entry of `GET /voices`.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceDto {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub seconds: Option<f64>,
    pub sample_url: String,
}

/// `GET /voices` catalogue.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceCatalogDto {
    #[serde(rename = "default")]
    pub default_voice: String,
    pub voices: Vec<VoiceDto>,
}

/// One row of `GET /proxy/v1/models` `data[]` (id only; unknown upstream keys ignored).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelListItemDto {
    pub id: String,
}

/// `GET /proxy/v1/models` body (passthrough; clients read `data[].id`).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelListDto {
    pub data: Vec<ModelListItemDto>,
}

/// `POST /proxy/v1/audio/transcriptions` body (passthrough / mock; clients read `text`).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptionDto {
    pub text: String,
}

// ---- Per-user pushes / releases / push registration ---------------------------------------------
//
// `PushEventDto` is the adjacently tagged `{ kind, payload }` the relay stores and `/me/pushes`
// returns. `PushItemDto` adds `id` + `created_at` beside it (serde flatten; typeshare skips the
// nested event so Swift decodes `PushEventDto` from the same object). Writers build a variant's
// payload struct so a new kind cannot be added without a type. Wire JSON is unchanged.

/// `people_message` payload on `/me/pushes`.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeopleMessagePayloadDto {
    pub chat_id: String,
    pub from: String,
    pub from_handle: String,
    #[typeshare(serialized_as = "I54")]
    pub seq: i64,
    pub sender: SenderDto,
    /// The project id for an agent's reply in a project chat; absent (`people`) for DMs and
    /// people groups. Agents' announcements queue this push since 2026-10-26 (before, only a
    /// device's own row did, so an agent reply never showed an in-app banner).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// The announced message's first line, when the sender announced one (agents do; a
    /// device's row-level push has none and the banner says "New message").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// `people_chat_updated` payload.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeopleChatUpdatedPayloadDto {
    pub chat_id: String,
}

/// `people_member_added` payload.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeopleMemberAddedPayloadDto {
    pub chat_id: String,
    pub user_id: String,
    pub handle: String,
    pub by_handle: String,
}

/// `people_member_removed` payload.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeopleMemberRemovedPayloadDto {
    pub chat_id: String,
    pub user_id: String,
    pub handle: String,
    pub by_handle: String,
    /// True when an admin banned them (they cannot rejoin). Omit / null on leave or plain remove.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub banned: Option<bool>,
}

/// `people_chat_created` payload.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeopleChatCreatedPayloadDto {
    pub chat_id: String,
    pub by: String,
    pub by_handle: String,
}

/// `people_chat_deleted` payload.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeopleChatDeletedPayloadDto {
    pub chat_id: String,
    pub by_handle: String,
}

/// `people_agent_added` payload (user devices).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeopleAgentAddedPayloadDto {
    pub chat_id: String,
    pub project_id: String,
    pub by_handle: String,
}

/// `people_agent_removed` payload (user devices).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeopleAgentRemovedPayloadDto {
    pub chat_id: String,
    pub project_id: String,
    pub by_handle: String,
}

/// `people_project_shared` payload.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeopleProjectSharedPayloadDto {
    pub project_id: String,
    pub by_handle: String,
}

/// `project_member_removed` payload.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectMemberRemovedPayloadDto {
    pub project_id: String,
    pub by_handle: String,
}

/// `space_archived` payload: a sidebar space (agent `project` or `server`) archived for this
/// user on another device (`archived_at` is null when un-archiving).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpaceArchivedPayloadDto {
    pub space_id: String,
    /// `project` or `server`.
    pub kind: String,
    #[typeshare(serialized_as = "Option<I54>")]
    pub archived_at: Option<i64>,
}

/// `chat_archived` payload (`archived_at` is null when un-archiving).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatArchivedPayloadDto {
    pub chat_id: String,
    pub project_id: String,
    #[typeshare(serialized_as = "Option<I54>")]
    pub archived_at: Option<i64>,
}

/// `people_invite_accepted` payload.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeopleInviteAcceptedPayloadDto {
    pub code: String,
    pub handle: String,
    pub user_id: String,
    pub target_kind: String,
    pub target_id: String,
}

/// `feedback_capture` payload.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeedbackCapturePayloadDto {
    pub request_id: String,
    pub project_id: String,
    pub project_name: String,
    pub chat_id: Option<String>,
    pub preferred_device_id: Option<String>,
    #[typeshare(serialized_as = "I54")]
    pub expires_at: i64,
}

/// `server_member_removed` payload.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerMemberRemovedPayloadDto {
    pub server_id: String,
    pub user_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub banned: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
}

/// `server_member_joined` payload.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerMemberJoinedPayloadDto {
    pub server_id: String,
    pub user_id: String,
    pub handle: String,
}

/// `server_updated` payload (optional keys only when the writer sets them).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerUpdatedPayloadDto {
    pub server_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

/// `server_deleted` payload.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerDeletedPayloadDto {
    pub server_id: String,
}

/// `machine_ready` payload.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineReadyPayloadDto {
    pub message: String,
}

/// `release_published` payload (also the row shape under `releases[]`).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleasePublishedPayloadDto {
    pub platform: String,
    pub channel: String,
    pub version: String,
    pub build: String,
    pub url: String,
    #[typeshare(serialized_as = "I54")]
    pub published_at: i64,
}

/// `secret_request` payload (`vault_id` only from the vaults writer).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecretRequestPushPayloadDto {
    pub request_id: String,
    pub project_id: String,
    pub project_name: String,
    pub name: String,
    pub host: Option<String>,
    #[serde(rename = "use")]
    pub use_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault_id: Option<String>,
}

/// `message_redacted` payload: a message's value moved into the vault and the row became the
/// "Secret saved as NAME" ghost. Devices drop the old text from search and notifications and
/// persist a shallow snapshot of the room. Names only.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageRedactedPayloadDto {
    pub chat_id: String,
    /// Absent for a people chat.
    pub project_id: Option<String>,
    pub message_id: String,
    pub secret_id: String,
    pub name: String,
}

/// `secrets_paused` payload.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecretsPausedPayloadDto {
    pub paused: bool,
}

/// `secret_expiring` payload.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecretExpiringPayloadDto {
    pub secret_id: String,
    pub name: String,
    #[typeshare(serialized_as = "Option<I54>")]
    pub expires_at: Option<i64>,
    pub expired: bool,
}

/// `profile_updated` payload: a person changed their picture (`PUT` / `DELETE /me/avatar`).
/// Sent to their own other devices and to everyone who shares a people chat or a server with
/// them; `avatar_url` is the new picture (`null` = none, the coloured initials show).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileUpdatedPayloadDto {
    pub user_id: String,
    #[serde(default)]
    pub avatar_url: Option<String>,
}

/// `agent_profile_updated` payload: an agent's avatar changed (`PATCH /projects/{id}` with
/// `image_key`, a leaf pick or a colour pick) — the agents' `profile_updated`. Sent to the
/// owner's other devices, the project's members and everyone who shares a people chat with the
/// agent. `image_key` is the picture now in effect (`null` = the leaf / initials; the leaf
/// picks come with the registry re-read), `avatar_version` the stamp every copy must reach.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentProfileUpdatedPayloadDto {
    pub project_id: String,
    #[typeshare(serialized_as = "I54")]
    pub avatar_version: i64,
    #[serde(default)]
    pub image_key: Option<String>,
}

/// `notifications_cleared` payload: the user read `chat_id` on one of their devices
/// (`PUT /chats/{id}/read`), so every other device drops the banners it still shows for that
/// chat and sets its app badge to `badge` — the user's unread total across chats after the
/// read (Fluxer-style clear-on-read, carried on `/me/pushes` today and as a silent push when
/// APNs is live). `project_id` is `people` for DMs and groups, else the agent's project.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotificationsClearedPayloadDto {
    pub chat_id: String,
    pub project_id: String,
    #[typeshare(serialized_as = "I54")]
    pub badge: i64,
}

/// One kind the relay queues on `/me/pushes` via `push_user` / `push_user_except` /
/// `push_everyone`. Adjacent tagging matches the stored `{ kind, payload }` row.
///
/// `call_started` embeds a full `CallDto`; clippy's large-enum lint would want a `Box`, but
/// boxing breaks the typeshare / Swift shape and the wire JSON is unchanged either way.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload")]
#[allow(clippy::large_enum_variant)]
pub enum PushEventDto {
    #[serde(rename = "people_message")]
    PeopleMessage(PeopleMessagePayloadDto),
    #[serde(rename = "people_chat_updated")]
    PeopleChatUpdated(PeopleChatUpdatedPayloadDto),
    #[serde(rename = "people_member_added")]
    PeopleMemberAdded(PeopleMemberAddedPayloadDto),
    #[serde(rename = "people_member_removed")]
    PeopleMemberRemoved(PeopleMemberRemovedPayloadDto),
    #[serde(rename = "people_chat_created")]
    PeopleChatCreated(PeopleChatCreatedPayloadDto),
    #[serde(rename = "people_chat_deleted")]
    PeopleChatDeleted(PeopleChatDeletedPayloadDto),
    #[serde(rename = "people_agent_added")]
    PeopleAgentAdded(PeopleAgentAddedPayloadDto),
    #[serde(rename = "people_agent_removed")]
    PeopleAgentRemoved(PeopleAgentRemovedPayloadDto),
    #[serde(rename = "people_project_shared")]
    PeopleProjectShared(PeopleProjectSharedPayloadDto),
    #[serde(rename = "project_member_removed")]
    ProjectMemberRemoved(ProjectMemberRemovedPayloadDto),
    #[serde(rename = "chat_archived")]
    ChatArchived(ChatArchivedPayloadDto),
    #[serde(rename = "space_archived")]
    SpaceArchived(SpaceArchivedPayloadDto),
    #[serde(rename = "people_invite_accepted")]
    PeopleInviteAccepted(PeopleInviteAcceptedPayloadDto),
    #[serde(rename = "feedback_capture")]
    FeedbackCapture(FeedbackCapturePayloadDto),
    #[serde(rename = "message_redacted")]
    MessageRedacted(MessageRedactedPayloadDto),
    #[serde(rename = "server_member_removed")]
    ServerMemberRemoved(ServerMemberRemovedPayloadDto),
    #[serde(rename = "server_member_joined")]
    ServerMemberJoined(ServerMemberJoinedPayloadDto),
    #[serde(rename = "server_updated")]
    ServerUpdated(ServerUpdatedPayloadDto),
    #[serde(rename = "server_deleted")]
    ServerDeleted(ServerDeletedPayloadDto),
    #[serde(rename = "call_started")]
    CallStarted(CallStartedPayloadDto),
    #[serde(rename = "call_ended")]
    CallEnded(CallEndedPayloadDto),
    #[serde(rename = "call_missed")]
    CallMissed(CallMissedPayloadDto),
    #[serde(rename = "machine_ready")]
    MachineReady(MachineReadyPayloadDto),
    #[serde(rename = "release_published")]
    ReleasePublished(ReleasePublishedPayloadDto),
    #[serde(rename = "secret_request")]
    SecretRequest(SecretRequestPushPayloadDto),
    #[serde(rename = "secrets_paused")]
    SecretsPaused(SecretsPausedPayloadDto),
    #[serde(rename = "secret_expiring")]
    SecretExpiring(SecretExpiringPayloadDto),
    #[serde(rename = "profile_updated")]
    ProfileUpdated(ProfileUpdatedPayloadDto),
    #[serde(rename = "agent_profile_updated")]
    AgentProfileUpdated(AgentProfileUpdatedPayloadDto),
    #[serde(rename = "notifications_cleared")]
    NotificationsCleared(NotificationsClearedPayloadDto),
}

impl PushEventDto {
    /// Kind string and payload object as stored in `user_pushes` / returned by `/me/pushes`.
    pub fn kind_and_payload(&self) -> (String, Value) {
        let v = serde_json::to_value(self).expect("PushEventDto serializes");
        let kind = v
            .get("kind")
            .and_then(Value::as_str)
            .expect("PushEventDto kind")
            .to_owned();
        let payload = v.get("payload").cloned().unwrap_or(Value::Null);
        (kind, payload)
    }
}

/// One `/me/pushes` list row: `id`, `created_at`, plus flattened `{ kind, payload }`.
///
/// typeshare cannot emit serde flatten; Swift decodes `id`/`created_at` here and
/// `PushEventDto` from the same JSON object.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushItemDto {
    pub id: String,
    #[typeshare(serialized_as = "I54")]
    pub created_at: i64,
    #[serde(flatten)]
    #[typeshare(skip)]
    pub event: PushEventDto,
}

/// One row of `GET /releases/current` (`all_releases`).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleaseDto {
    pub platform: String,
    pub channel: String,
    pub version: String,
    pub build: String,
    pub url: String,
    #[typeshare(serialized_as = "I54")]
    pub published_at: i64,
}

/// `GET /releases/current` body.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleasesDto {
    pub releases: Vec<ReleaseDto>,
}

/// `POST /devices/{id}/push` success body.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushRegisterResponseDto {
    pub device_id: String,
    pub environment: String,
    pub push: bool,
}

// ---- Connectors / channels / skills / local / feedback ------------------------------------------
//
// Free-form on the wire (`#[typeshare(skip)]` + `Value`): connector `config`, action `args`.
// Channel binding / pairing keep camelCase keys via serde renames (wire unchanged).

/// One action on a connector provider catalog entry.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectorActionDto {
    pub name: String,
    pub kind: String,
    pub description: String,
    /// JSON Schema object for the action's args (`type` / `properties` / `required`).
    #[typeshare(skip)]
    pub args: Value,
}

/// One provider in `GET /connectors/catalog` (`provider_json`).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderDto {
    pub provider: String,
    pub label: String,
    /// `oauth` | `oauth_or_token` | `token`.
    pub connect: String,
    pub oauth_configured: bool,
    pub mock: bool,
    pub scopes: Vec<String>,
    pub actions: Vec<ConnectorActionDto>,
}

/// Client-facing connector (`connector_json`). Never includes a token.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectorDto {
    pub id: String,
    pub provider: String,
    pub label: String,
    pub scopes: Vec<String>,
    pub status: String,
    /// Provider-specific settings (e.g. `baseUrl`); never secrets.
    #[typeshare(skip)]
    pub config: Value,
    #[typeshare(serialized_as = "I54")]
    pub created_at: i64,
    #[typeshare(serialized_as = "Option<I54>")]
    pub last_used_at: Option<i64>,
    pub last_error: Option<String>,
    pub mock: bool,
}

/// `GET /connectors/catalog` body.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectorsCatalogDto {
    pub mock: bool,
    pub redirect_uri: String,
    pub providers: Vec<ProviderDto>,
}

/// `GET /connectors` body.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectorsListDto {
    pub mock: bool,
    pub items: Vec<ConnectorDto>,
}

/// `POST /connectors/{provider}/connect` reply: OAuth start (`authUrl`) or a stored connector.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectConnectorReplyDto {
    #[serde(rename = "authUrl", default, skip_serializing_if = "Option::is_none")]
    pub auth_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(
        rename = "redirectUri",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub redirect_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mock: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connector: Option<ConnectorDto>,
}

/// One platform row on `GET /channels`.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelPlatformDto {
    pub platform: String,
    pub label: String,
    pub state: String,
    pub detail: String,
    pub inbound: String,
}

/// One binding on `GET /channels` (`binding_json`). CamelCase keys match today's wire.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelBindingDto {
    pub id: String,
    pub platform: String,
    #[serde(rename = "externalChatId")]
    pub external_chat_id: String,
    #[serde(rename = "externalUserId")]
    pub external_user_id: String,
    #[serde(rename = "projectId")]
    pub project_id: String,
    #[serde(rename = "chatId")]
    pub chat_id: String,
    pub label: String,
    #[typeshare(serialized_as = "I54")]
    pub bound_at: i64,
}

/// Active pairing code on `GET /channels` / `POST /channels/pair/new` (`pair_json`).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelPairingDto {
    pub code: String,
    #[serde(rename = "projectId")]
    pub project_id: String,
    #[serde(rename = "chatId")]
    pub chat_id: String,
    #[typeshare(serialized_as = "I54")]
    pub created_at: i64,
    #[serde(rename = "expiresAt")]
    #[typeshare(serialized_as = "I54")]
    pub expires_at: i64,
}

/// `GET /channels` overview body.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelsOverviewDto {
    pub mock: bool,
    pub platforms: Vec<ChannelPlatformDto>,
    pub bindings: Vec<ChannelBindingDto>,
    pub pairing: Option<ChannelPairingDto>,
}

/// One skill as `GET /skills` returns it (`skill_json`): core [`crate::skill::Skill`] plus markdown.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillDto {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    pub when_to_use: String,
    pub body: String,
    pub origin_project_id: Option<String>,
    pub shared: bool,
    #[typeshare(serialized_as = "I54")]
    pub version: i64,
    #[typeshare(serialized_as = "I54")]
    pub created_at: i64,
    #[typeshare(serialized_as = "I54")]
    pub updated_at: i64,
    /// Markdown document (frontmatter + body) for the client editor.
    pub markdown: String,
}

impl From<&crate::skill::Skill> for SkillDto {
    fn from(s: &crate::skill::Skill) -> Self {
        Self {
            id: s.id.clone(),
            slug: s.slug.clone(),
            name: s.name.clone(),
            description: s.description.clone(),
            tags: s.tags.clone(),
            when_to_use: s.when_to_use.clone(),
            body: s.body.clone(),
            origin_project_id: s.origin_project_id.clone(),
            shared: s.shared,
            version: s.version,
            created_at: s.created_at,
            updated_at: s.updated_at,
            markdown: s.to_markdown(),
        }
    }
}

/// One row of `GET /local/devices` (includes live `connected`).
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalDeviceDto {
    pub device_id: String,
    pub name: String,
    pub hostname: String,
    pub os: String,
    pub home: String,
    pub connected: bool,
    #[typeshare(serialized_as = "Option<I54>")]
    pub last_seen: Option<i64>,
    #[typeshare(serialized_as = "I54")]
    pub first_seen: i64,
    pub allow_reads: bool,
}

/// `GET /local/devices` body.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalDevicesDto {
    pub items: Vec<LocalDeviceDto>,
}

/// `POST /feedback` success body (`id`, `created_at`, `has_image`). Folded repeats may add
/// `duplicate` / `occurrences`.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeedbackSubmitReplyDto {
    pub id: String,
    #[typeshare(serialized_as = "I54")]
    pub created_at: i64,
    pub has_image: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicate: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[typeshare(serialized_as = "Option<I54>")]
    pub occurrences: Option<i64>,
}

/// Inner `error` object on every relay error body.
///
/// `type` strings in use (from `signin::api_error` and `secrets::ApiErr` call sites):
/// `already_exists`, `auth_mode`, `bad_grant`, `bad_handle`, `bad_host`, `bad_name`,
/// `bad_redirect`, `bad_refresh`, `bad_request`, `bad_state`, `bad_user`, `conflict`, `db`,
/// `forbidden`, `handle_taken`, `internal`, `invalid_token`, `io`, `missing_token`,
/// `not_found`, `not_jpeg`, `not_owner`, `not_pending`, `oauth_unconfigured`, `paused`,
/// `token_expired`, `too_large`, `window_closed`. Other routes add more (`no_call`,
/// `call_ended`, …). Clients branch on `type`, never on `message` text. Unknown types stay
/// decodable.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiErrorBodyDto {
    #[serde(rename = "type")]
    pub type_: String,
    pub message: String,
}

/// Relay error body: `{"error":{"type","message"}}`.
#[typeshare]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiErrorDto {
    pub error: ApiErrorBodyDto,
}
