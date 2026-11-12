//! Chats on Matrix (D88): the relay keeps its API and writes every chat into a Synapse homeserver
//! next to it, through three appservice registrations (`scripts/deploy/matrix`). Users never see
//! the word Matrix.
//!
//! Stage A (this module): **dual-write**. Every change the relay sees today — a chat or group made,
//! a member added or removed, a whole message announced, a read mark, an archive, a rename — is
//! also queued as a job in `matrix_outbox` and applied to the homeserver by one worker, in order
//! per chat, with retries. A Synapse outage never blocks today's path: the queue just grows.
//!
//! Switches: `ARBOS_MATRIX=off|dual|on` (relay env; `off` = nothing is queued, `dual` = stage A
//! writes, `on` = reads allowed per user, stage B), `ARBOS_MATRIX_E2EE=on` (stage D: new DMs
//! and private groups of up to [`E2EE_MEMBER_CAP`] members are created end-to-end encrypted and
//! the relay stops mirroring messages into them; channels stay plaintext. An older app cannot
//! read or post in an encrypted room, and a room never goes back, so
//! `ARBOS_MATRIX_E2EE_MIN_CLIENTS` names the oldest app of each platform that decrypts and a
//! chat is encrypted only when everyone in it runs at least that, [`clients_decrypt`]) and the kill switch file `/etc/arbos/matrix.disabled` (the worker pauses; the outbox keeps queueing).
//!
//! Identity (design §2.3): `@<user_id>:arbos.life` for people (the relay `user_id` is the
//! localpart, lower-cased), `@_a_<project_id>:arbos.life` for agents.

pub mod appservice;
pub mod backfill;
pub mod client;
pub mod db;
pub mod history;
pub mod native;
pub mod reads;
pub mod servers;
pub mod servers_db;
pub mod shadow;
pub mod spaces;
pub mod worker;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Notify;

use crate::state::AppState;

/// Kill switch: when this file exists the worker pauses (jobs keep queueing).
pub const DISABLED_FILE: &str = "/etc/arbos/matrix.disabled";
/// The most members an end-to-end encrypted people chat may have. Megolm shares each session
/// key with every member device, so past this a group has to be a plaintext one.
pub const E2EE_MEMBER_CAP: usize = 200;
/// How long a `Resync` waits after the first row of a burst.
const RESYNC_DELAY_MS: i64 = 4_000;

/// Phase 3: the homeserver room a people chat's members read and write directly, or `None`
/// while the chat still lives in its Loro room. A device that sends there writes nothing to
/// the Loro room, so the answer is a room only when nobody would miss the message:
/// every member is a person (an agent reads and answers through the Loro room), every one
/// of them is flagged (`users.matrix_people_native`), and the chat has an open room. A
/// failed lookup reads as `None`: the Loro room is the path that loses nothing.
pub fn people_native_room(
    state: &AppState,
    cm: &crate::db_people::ChatWithMembers,
) -> Option<String> {
    if !state.matrix.cfg.mode.writes() || cm.members.is_empty() {
        return None;
    }
    for m in &cm.members {
        if !state.db.matrix_people_native(m.user_id()?).ok()? {
            return None;
        }
    }
    let room = state.db.matrix_room(&cm.chat.id).ok()??;
    (room.closed_at.is_none() && room.project_id.is_none()).then_some(room.room_id)
}

/// An app as it names itself in `X-Arbos-Client`: `(platform, version, build)`.
pub type AppBuild = (String, String, String);

/// `ARBOS_MATRIX_E2EE_MIN_CLIENTS`: what the apps send as `X-Arbos-Client`, one per platform,
/// comma-separated (`mac/0.6.420, ios/1.0.0 (202611200000)`). A value that does not read as
/// that stops the relay rather than switching the check off.
fn min_clients(s: Option<&str>) -> anyhow::Result<Vec<AppBuild>> {
    s.unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(|c| {
            crate::releases::parse_client(c).ok_or_else(|| {
                anyhow::anyhow!(
                    "ARBOS_MATRIX_E2EE_MIN_CLIENTS: {c:?} is not platform/version (build)"
                )
            })
        })
        .collect()
}

/// Whether an app at `have` is at least `min`, both `(version, build)`: dotted numbers, then
/// the build number when `min` names one. A part that is not a number counts as 0.
fn at_least(have: (&str, &str), min: (&str, &str)) -> bool {
    let parts = |v: &str| {
        let mut n: Vec<u64> = v.split('.').map(|p| p.parse().unwrap_or(0)).collect();
        while n.last() == Some(&0) {
            n.pop();
        }
        n
    };
    let build = |b: &str| b.parse::<u64>().unwrap_or(0);
    match parts(have.0).cmp(&parts(min.0)) {
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Equal => min.1.is_empty() || build(have.1) >= build(min.1),
    }
}

/// A signed-in device counts for [`clients_decrypt`] when it was seen this recently; one
/// left in a drawer does not keep its owner's chats in the clear for good.
const ACTIVE_DEVICE_MS: i64 = 30 * 24 * 3_600_000;

/// Whether every one of `users` runs apps that decrypt: each has a signed-in device, and each
/// of their devices seen in the last [`ACTIVE_DEVICE_MS`] (or, when none was, the one seen
/// last) last said it runs at least its platform's minimum. A device that never said what it
/// runs, or runs a platform with no minimum, does not. A failed lookup reads as no: an
/// unencrypted chat is one everybody can read.
pub fn clients_decrypt<'a>(state: &AppState, users: impl Iterator<Item = &'a str>) -> bool {
    let min = &state.matrix.cfg.e2ee_min_clients;
    if min.is_empty() {
        return true;
    }
    let now = arbos_core::ids::now_ms();
    for user in users {
        let Ok(mut devices) = state.db.matrix_device_clients(user) else {
            return false;
        };
        // Newest first; the active ones, or the newest alone.
        devices.sort_by_key(|(seen, _)| std::cmp::Reverse(*seen));
        let active = devices
            .iter()
            .filter(|(seen, _)| now - seen <= ACTIVE_DEVICE_MS)
            .count()
            .max(1);
        let decrypts = |client: &Option<AppBuild>| {
            client.as_ref().is_some_and(|(platform, version, build)| {
                min.iter()
                    .find(|(p, _, _)| p == platform)
                    .is_some_and(|(_, v, b)| at_least((version, build), (v, b)))
            })
        };
        if devices.is_empty() || !devices.iter().take(active).all(|(_, c)| decrypts(c)) {
            return false;
        }
    }
    true
}

/// `ARBOS_MATRIX`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Today's relay; nothing touches the homeserver.
    Off,
    /// Stage A: every write is mirrored into the homeserver.
    Dual,
    /// Stage B: dual writes, and users flagged `matrix_reads` are answered from the homeserver.
    On,
}

impl Mode {
    pub fn parse(s: Option<&str>) -> anyhow::Result<Self> {
        match s.map(str::trim).filter(|s| !s.is_empty()) {
            None | Some("off") => Ok(Self::Off),
            Some("dual") => Ok(Self::Dual),
            Some("on") => Ok(Self::On),
            Some(other) => anyhow::bail!("ARBOS_MATRIX must be off, dual or on, not {other:?}"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Dual => "dual",
            Self::On => "on",
        }
    }

    pub fn writes(self) -> bool {
        !matches!(self, Self::Off)
    }
}

/// The three appservice registrations the relay holds tokens for (design §2.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appservice {
    Users,
    Agents,
    Bridges,
}

impl Appservice {
    pub fn id(self) -> &'static str {
        match self {
            Self::Users => "arbos-users",
            Self::Agents => "arbos-agents",
            Self::Bridges => "arbos-bridges",
        }
    }
}

/// Homeserver connection: URL, server name and the token pairs (`ARBOS_MATRIX_*`).
#[derive(Debug, Clone)]
pub struct Config {
    pub mode: Mode,
    pub hs_url: String,
    pub server_name: String,
    pub as_token_users: String,
    pub hs_token_users: String,
    pub as_token_agents: String,
    pub hs_token_agents: String,
    pub as_token_bridges: String,
    pub hs_token_bridges: String,
    /// Stage D: DMs and private groups made from now on carry `m.room.encryption`.
    pub e2ee: bool,
    /// The oldest app of each platform that decrypts (`ARBOS_MATRIX_E2EE_MIN_CLIENTS`), as
    /// `(platform, version, build)`. A chat is made encrypted only when every person in it
    /// runs at least these ([`clients_decrypt`]); empty means nobody is checked.
    pub e2ee_min_clients: Vec<AppBuild>,
    pub disabled_file: PathBuf,
}

impl Config {
    /// From the relay environment. `ARBOS_MATRIX` other than `off` needs the URL and the users
    /// token pair; the agents/bridges pairs fall back to the users pair for a one-registration
    /// self-host.
    pub fn from_env() -> anyhow::Result<Self> {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let mode = Mode::parse(get("ARBOS_MATRIX").as_deref())?;
        if mode == Mode::Off {
            return Ok(Self::off());
        }
        let hs_url = get("ARBOS_MATRIX_HS_URL").unwrap_or_else(|| "http://127.0.0.1:7708".into());
        let server_name = get("ARBOS_MATRIX_SERVER_NAME").unwrap_or_else(|| "arbos.life".into());
        let as_users = get("ARBOS_MATRIX_AS_TOKEN_USERS").unwrap_or_default();
        let hs_users = get("ARBOS_MATRIX_HS_TOKEN_USERS").unwrap_or_default();
        if mode.writes() && (as_users.is_empty() || hs_users.is_empty()) {
            anyhow::bail!(
                "ARBOS_MATRIX={} needs ARBOS_MATRIX_AS_TOKEN_USERS and ARBOS_MATRIX_HS_TOKEN_USERS",
                mode.as_str()
            );
        }
        Ok(Self {
            mode,
            hs_url: hs_url.trim_end_matches('/').to_owned(),
            server_name,
            as_token_agents: get("ARBOS_MATRIX_AS_TOKEN_AGENTS")
                .unwrap_or_else(|| as_users.clone()),
            hs_token_agents: get("ARBOS_MATRIX_HS_TOKEN_AGENTS")
                .unwrap_or_else(|| hs_users.clone()),
            as_token_bridges: get("ARBOS_MATRIX_AS_TOKEN_BRIDGES")
                .unwrap_or_else(|| as_users.clone()),
            hs_token_bridges: get("ARBOS_MATRIX_HS_TOKEN_BRIDGES")
                .unwrap_or_else(|| hs_users.clone()),
            as_token_users: as_users,
            hs_token_users: hs_users,
            e2ee: get("ARBOS_MATRIX_E2EE").as_deref() == Some("on"),
            e2ee_min_clients: min_clients(get("ARBOS_MATRIX_E2EE_MIN_CLIENTS").as_deref())?,
            disabled_file: PathBuf::from(DISABLED_FILE),
        })
    }

    pub fn off() -> Self {
        Self {
            mode: Mode::Off,
            hs_url: String::new(),
            server_name: "arbos.life".into(),
            as_token_users: String::new(),
            hs_token_users: String::new(),
            as_token_agents: String::new(),
            hs_token_agents: String::new(),
            as_token_bridges: String::new(),
            hs_token_bridges: String::new(),
            e2ee: false,
            e2ee_min_clients: Vec::new(),
            disabled_file: PathBuf::from(DISABLED_FILE),
        }
    }

    /// Which appservice an inbound `hs_token` belongs to.
    pub fn appservice_for_hs_token(&self, token: &str) -> Option<Appservice> {
        if token.is_empty() {
            return None;
        }
        if token == self.hs_token_users {
            Some(Appservice::Users)
        } else if token == self.hs_token_agents {
            Some(Appservice::Agents)
        } else if token == self.hs_token_bridges {
            Some(Appservice::Bridges)
        } else {
            None
        }
    }

    /// The `as_token` that may act as this Matrix user.
    pub fn as_token_for(&self, mxid: &str) -> &str {
        let local = mxid.trim_start_matches('@');
        if local.starts_with("_a_") || local.starts_with("_x_") {
            &self.as_token_agents
        } else if local.starts_with("_b_") {
            &self.as_token_bridges
        } else {
            &self.as_token_users
        }
    }

    pub fn disabled(&self) -> bool {
        Path::new(&self.disabled_file).exists()
    }

    // ---- identity -----------------------------------------------------------------------------

    /// Matrix user id of a relay user (`gh_123` → `@gh_123:arbos.life`). Relay ids are ASCII;
    /// ULID-based legacy ids are lower-cased (ULIDs decode case-insensitively).
    pub fn user_mxid(&self, user_id: &str) -> String {
        format!("@{}:{}", localpart(user_id), self.server_name)
    }

    /// Matrix user id of a project agent (`@_a_<project id>:arbos.life`).
    pub fn agent_mxid(&self, project_id: &str) -> String {
        format!("@_a_{}:{}", localpart(project_id), self.server_name)
    }

    /// `user:<id>` / `agent:<project>` membership strings → Matrix user id.
    pub fn member_mxid(&self, member: &str) -> Option<String> {
        if let Some(u) = member.strip_prefix("user:") {
            Some(self.user_mxid(u))
        } else {
            member.strip_prefix("agent:").map(|p| self.agent_mxid(p))
        }
    }

    /// Reverse of [`Self::member_mxid`] for ids this relay minted.
    pub fn member_of_mxid(&self, mxid: &str) -> Option<String> {
        let suffix = format!(":{}", self.server_name);
        let local = mxid.strip_prefix('@')?.strip_suffix(suffix.as_str())?;
        if let Some(p) = local.strip_prefix("_a_") {
            // Project ids are upper-case ULIDs (case-insensitive to decode).
            Some(format!("agent:{}", p.to_ascii_uppercase()))
        } else if local.starts_with('_') {
            None
        } else {
            Some(format!("user:{local}"))
        }
    }
}

/// Lower-case a relay id for use as a Matrix localpart. Relay ids are `gh_<digits>`,
/// `u_<ULID>` or bare ULIDs (project ids); all are `[A-Za-z0-9_-]`.
pub fn localpart(id: &str) -> String {
    id.chars()
        .map(|c| match c {
            'A'..='Z' => c.to_ascii_lowercase(),
            'a'..='z' | '0'..='9' | '_' | '-' | '.' | '=' | '/' => c,
            _ => '=',
        })
        .collect()
}

/// One queued change for the homeserver (`matrix_outbox.kind` + `payload`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Job {
    /// Make the room match the relay's view of the chat: create it if needed, members, name.
    SyncRoom {
        chat_id: String,
        project_id: Option<String>,
    },
    /// One finished message. `ts` is set for backfill (timestamp massaging), `None` live.
    Message {
        chat_id: String,
        project_id: Option<String>,
        message_id: String,
        /// `user:<id>` | `agent:<project>` | `system`.
        sender: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parts: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ts: Option<i64>,
    },
    /// A person read the chat: `m.read` receipt on the newest event the relay wrote.
    Read { chat_id: String, user_id: String },
    /// A person archived (`m.lowpriority` tag) or unarchived the chat.
    Archive {
        chat_id: String,
        user_id: String,
        archived: bool,
    },
    /// The chat is gone: everyone leaves, the room is tombstoned in state.
    CloseRoom { chat_id: String },
    /// Stage B: register (on) or remove (off) the relay push gateway on a user's devices.
    Pushers { user_id: String, on: bool },
    /// C1: make the Space match the server row (members, name, children).
    SyncServer { server_id: String },
    /// C1: make a channel room match its row (members, name, posting rule, parent).
    SyncChannel { channel_id: String },
    /// A membership notice in a channel ("Jacob added Doug"): an `m.notice` from the server
    /// owner with `life.arbos.sender: system`, which the apps draw as a centred system line.
    ChannelNotice {
        channel_id: String,
        text: String,
        /// Stable per notice: the txn id, so a retried job never doubles the line.
        notice_id: String,
    },
    /// A device pushed rows: mirror the doc's messages nobody announced (debounced per chat).
    Resync {
        chat_id: String,
        project_id: Option<String>,
    },
    /// Phase 3: a member's device sent this message into the chat's room on the homeserver;
    /// write it into the Loro room (`native.rs`). `reply_to` is the event it answers.
    NativeMessage {
        chat_id: String,
        event_id: String,
        user_id: String,
        message_id: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parts: Option<Vec<Value>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply_to: Option<String>,
        ts: i64,
    },
    /// Phase 3: an event was removed in the chat's room on the homeserver; remove its Loro row
    /// (or take back the reaction it was).
    NativeRemoval { chat_id: String, event_id: String },
    /// Phase 3: a member's device reacted with `key` to the event `target` on the homeserver;
    /// put the reaction on the Loro row.
    NativeReaction {
        chat_id: String,
        event_id: String,
        user_id: String,
        target: String,
        key: String,
    },
    /// Phase 3: read the chat's room from its beginning and write into the Loro room whatever
    /// its members sent there that the Loro room does not have (`native.rs`).
    NativeCatchUp { chat_id: String },
}

impl Job {
    /// Jobs of one chat run in order; this is the queue key (a user key for per-user jobs).
    pub fn chat_key(&self) -> &str {
        match self {
            Job::SyncRoom { chat_id, .. }
            | Job::Message { chat_id, .. }
            | Job::Read { chat_id, .. }
            | Job::Archive { chat_id, .. }
            | Job::CloseRoom { chat_id }
            | Job::Resync { chat_id, .. }
            | Job::NativeMessage { chat_id, .. }
            | Job::NativeRemoval { chat_id, .. }
            | Job::NativeReaction { chat_id, .. }
            | Job::NativeCatchUp { chat_id } => chat_id,
            Job::Pushers { user_id, .. } => user_id,
            Job::SyncServer { server_id } => server_id,
            Job::SyncChannel { channel_id } | Job::ChannelNotice { channel_id, .. } => channel_id,
        }
    }
}

/// Process-lifetime counters of what the `Resync` safety net found (`GET /admin/matrix/status`
/// → `resync`). `unannounced` is the one to watch: rows the resync was the only path for.
#[derive(Debug, Default)]
pub struct Counters {
    pub resync_passes: AtomicU64,
    pub resync_queued: AtomicU64,
    pub resync_pending_skipped: AtomicU64,
    pub resync_announced: AtomicU64,
    pub resync_unannounced_expected: AtomicU64,
    pub resync_unannounced: AtomicU64,
}

impl Counters {
    pub fn note_resync(&self, r: &backfill::Resync) {
        let add = |c: &AtomicU64, n: usize| {
            c.fetch_add(n as u64, Ordering::Relaxed);
        };
        add(&self.resync_passes, 1);
        add(&self.resync_queued, r.queued);
        add(&self.resync_pending_skipped, r.pending_skipped);
        add(&self.resync_announced, r.announced);
        add(&self.resync_unannounced_expected, r.unannounced_expected);
        add(&self.resync_unannounced, r.unannounced);
    }

    pub fn json(&self) -> Value {
        let get = |c: &AtomicU64| c.load(Ordering::Relaxed);
        json!({
            "passes": get(&self.resync_passes),
            "queued": get(&self.resync_queued),
            "pending_skipped": get(&self.resync_pending_skipped),
            "announced": get(&self.resync_announced),
            "unannounced_solo_project": get(&self.resync_unannounced_expected),
            "unannounced": get(&self.resync_unannounced),
        })
    }
}

/// Shared handle: config, the worker wake-up, and the outbox API the rest of the relay calls.
pub struct Matrix {
    pub cfg: Config,
    pub wake: Notify,
    pub counters: Counters,
    /// The device carrying each chat's history into its room, and until when (`history.rs`).
    pub history_leases: std::sync::Mutex<std::collections::HashMap<String, (String, i64)>>,
    /// The people whose homeserver account has been told a carried row is no new message.
    pub history_quiet: std::sync::Mutex<std::collections::HashSet<String>>,
}

impl Matrix {
    pub fn new(cfg: Config) -> Arc<Self> {
        Arc::new(Self {
            cfg,
            wake: Notify::new(),
            counters: Counters::default(),
            history_leases: Default::default(),
            history_quiet: Default::default(),
        })
    }

    /// Queue a job (no-op when `ARBOS_MATRIX=off`). Never fails the caller's request.
    pub fn enqueue(&self, state: &AppState, job: Job) {
        if !self.cfg.mode.writes() {
            return;
        }
        match state.db.matrix_enqueue(&job) {
            Ok(_) => self.wake.notify_one(),
            Err(e) => tracing::warn!(error = %e, ?job, "matrix outbox: enqueue failed"),
        }
    }

    /// A device pushed a row into a chat. One `Resync` per chat is kept pending, due after
    /// `RESYNC_DELAY_MS`, so a burst of rows costs one doc rebuild.
    pub fn touch_rows(&self, state: &AppState, chat_id: &str, project_id: Option<&str>) {
        if !self.cfg.mode.writes() {
            return;
        }
        if state
            .db
            .matrix_pending_kind(chat_id, "resync")
            .unwrap_or(true)
        {
            return;
        }
        let job = Job::Resync {
            chat_id: chat_id.to_owned(),
            project_id: project_id.map(str::to_owned),
        };
        if let Err(e) = state.db.matrix_enqueue_after(&job, RESYNC_DELAY_MS) {
            tracing::warn!(error = %e, ?job, "matrix outbox: enqueue failed");
        }
    }

    /// A chat's members, name or existence changed.
    pub fn touch_chat(&self, state: &AppState, chat_id: &str, project_id: Option<&str>) {
        self.enqueue(
            state,
            Job::SyncRoom {
                chat_id: chat_id.to_owned(),
                project_id: project_id.map(str::to_owned),
            },
        );
    }

    /// A project's guests changed: every room of that project re-syncs its members.
    pub fn touch_project(&self, state: &AppState, project_id: &str) {
        if !self.cfg.mode.writes() {
            return;
        }
        for chat_id in state
            .db
            .matrix_chats_of_project(project_id)
            .unwrap_or_default()
        {
            self.touch_chat(state, &chat_id, Some(project_id));
        }
    }

    /// A whole message landed (from `notify::announce`).
    pub fn message(
        &self,
        state: &AppState,
        chat_id: &str,
        project_id: Option<&str>,
        message_id: &str,
        sender: &str,
        text: &str,
    ) {
        if message_id.starts_with("row:") {
            // The content-less row fallback is not a message.
            return;
        }
        self.enqueue(
            state,
            Job::Message {
                chat_id: chat_id.to_owned(),
                project_id: project_id.map(str::to_owned),
                message_id: message_id.to_owned(),
                sender: sender.to_owned(),
                text: text.to_owned(),
                parts: None,
                ts: None,
            },
        );
    }

    pub fn read(&self, state: &AppState, chat_id: &str, user_id: &str) {
        self.enqueue(
            state,
            Job::Read {
                chat_id: chat_id.to_owned(),
                user_id: user_id.to_owned(),
            },
        );
    }

    pub fn archive(&self, state: &AppState, chat_id: &str, user_id: &str, archived: bool) {
        self.enqueue(
            state,
            Job::Archive {
                chat_id: chat_id.to_owned(),
                user_id: user_id.to_owned(),
                archived,
            },
        );
    }

    /// Stage B flip: the user's devices get (or lose) the relay as their Matrix push gateway.
    pub fn pushers(&self, state: &AppState, user_id: &str, on: bool) {
        self.enqueue(
            state,
            Job::Pushers {
                user_id: user_id.to_owned(),
                on,
            },
        );
    }

    /// C1: a server's name, members or channel list changed.
    pub fn touch_server(&self, state: &AppState, server_id: &str) {
        self.enqueue(
            state,
            Job::SyncServer {
                server_id: server_id.to_owned(),
            },
        );
    }

    /// C1: a channel was made or changed (name, posting rule).
    pub fn touch_channel(&self, state: &AppState, channel_id: &str) {
        self.enqueue(
            state,
            Job::SyncChannel {
                channel_id: channel_id.to_owned(),
            },
        );
    }

    /// Membership notice in a channel (#1349); runs after that channel's pending sync.
    pub fn channel_notice(&self, state: &AppState, channel_id: &str, text: &str) {
        self.enqueue(
            state,
            Job::ChannelNotice {
                channel_id: channel_id.to_owned(),
                text: text.to_owned(),
                notice_id: arbos_core::ids::ulid(),
            },
        );
    }

    pub fn close_chat(&self, state: &AppState, chat_id: &str) {
        self.enqueue(
            state,
            Job::CloseRoom {
                chat_id: chat_id.to_owned(),
            },
        );
    }

    /// `GET /admin/matrix/status` body.
    pub fn status_json(&self, state: &AppState) -> Value {
        let q = state.db.matrix_outbox_stats().unwrap_or_default();
        json!({
            "mode": self.cfg.mode.as_str(),
            "homeserver": self.cfg.hs_url,
            "server_name": self.cfg.server_name,
            "disabled": self.cfg.disabled(),
            "outbox": { "pending": q.pending, "failed": q.failed, "done": q.done, "oldest_pending_ms": q.oldest_pending_at },
            "rooms": state.db.matrix_room_count().unwrap_or(0),
            "events": state.db.matrix_event_count().unwrap_or(0),
            "resync": self.counters.json(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config::off()
    }

    #[test]
    fn ids_round_trip() {
        let c = cfg();
        assert_eq!(c.user_mxid("gh_12345"), "@gh_12345:arbos.life");
        assert_eq!(
            c.agent_mxid("01JAXYZ9ABCDEFGHJKMNPQRSTV"),
            "@_a_01jaxyz9abcdefghjkmnpqrstv:arbos.life"
        );
        assert_eq!(
            c.member_of_mxid("@_a_01jaxyz9abcdefghjkmnpqrstv:arbos.life")
                .as_deref(),
            Some("agent:01JAXYZ9ABCDEFGHJKMNPQRSTV")
        );
        assert_eq!(
            c.member_of_mxid("@gh_12345:arbos.life").as_deref(),
            Some("user:gh_12345")
        );
        assert_eq!(c.member_of_mxid("@_arbos:arbos.life"), None);
        assert_eq!(c.member_of_mxid("@gh_1:elsewhere.org"), None);
        assert_eq!(
            c.member_mxid("user:gh_7").as_deref(),
            Some("@gh_7:arbos.life")
        );
        assert_eq!(c.member_mxid("thing:x"), None);
    }

    #[test]
    fn mode_parses() {
        assert_eq!(Mode::parse(None).unwrap(), Mode::Off);
        assert_eq!(Mode::parse(Some("")).unwrap(), Mode::Off);
        assert_eq!(Mode::parse(Some("dual")).unwrap(), Mode::Dual);
        assert_eq!(Mode::parse(Some(" on ")).unwrap(), Mode::On);
        assert!(Mode::parse(Some("yes")).is_err());
    }

    #[test]
    fn the_oldest_apps_that_decrypt_are_read_or_the_relay_does_not_start() {
        assert!(min_clients(None).unwrap().is_empty());
        assert!(min_clients(Some(" ")).unwrap().is_empty());
        assert_eq!(
            min_clients(Some("mac/0.6.420, ios/1.0.0 (202611200000)")).unwrap(),
            [
                ("mac".to_owned(), "0.6.420".to_owned(), String::new()),
                (
                    "ios".to_owned(),
                    "1.0.0".to_owned(),
                    "202611200000".to_owned()
                ),
            ]
        );
        assert!(min_clients(Some("mac")).is_err());
        assert!(min_clients(Some("mac/0.6.420, on")).is_err());
        // (have, min) -> at least
        for (have, min, ok) in [
            (("0.6.420", ""), ("0.6.420", ""), true),
            (("0.6.421", ""), ("0.6.420", ""), true),
            (("0.7", ""), ("0.6.420", ""), true),
            (("0.6.42", ""), ("0.6.420", ""), false),
            (("0.6.419", "9999"), ("0.6.420", ""), false),
            (("1.0", "202611200000"), ("1.0.0", "202611200000"), true),
            (("1.0.0", "202611190000"), ("1.0.0", "202611200000"), false),
            (("1.0.1", "1"), ("1.0.0", "202611200000"), true),
            (("1.0.0", ""), ("1.0.0", "202611200000"), false),
            (("", ""), ("0.0.1", ""), false),
            (("soon", ""), ("0.0.1", ""), false),
        ] {
            assert_eq!(at_least(have, min), ok, "{have:?} against {min:?}");
        }
    }

    #[test]
    fn a_chat_is_encrypted_only_when_everyone_in_it_runs_an_app_that_decrypts() {
        use crate::people::tests_support::person;
        use crate::state::test_support::test_state;
        let mut state = test_state(crate::state::AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1501);
        let (bob, bob_dev) = person(&state, 1502);
        let device = |p: &crate::auth::Principal| match p {
            crate::auth::Principal::Device { id, .. } => id.clone(),
            _ => unreachable!(),
        };
        let (alice_dev, bob_dev) = (device(&alice_dev), device(&bob_dev));
        let both = || clients_decrypt(&state, [alice.as_str(), bob.as_str()].into_iter());
        // No minimum named: nobody is checked.
        assert!(both());
        state.matrix = Matrix::new(Config {
            e2ee_min_clients: min_clients(Some("mac/0.6.420, ios/1.0.0 (500)")).unwrap(),
            ..Config::off()
        });
        let both = || clients_decrypt(&state, [alice.as_str(), bob.as_str()].into_iter());
        let now = arbos_core::ids::now_ms();
        let seen = |dev: &str, at: i64| {
            state
                .db
                .with(|c| {
                    c.execute(
                        "UPDATE devices SET last_seen_at = ?2 WHERE id = ?1",
                        rusqlite::params![dev, at],
                    )
                })
                .unwrap();
        };
        let runs = |dev: &str, user: &str, platform: &str, version: &str, build: &str, at: i64| {
            state
                .db
                .note_device_version(dev, user, platform, version, build, at)
                .unwrap();
            seen(dev, at);
        };
        // Devices that never said what they run.
        assert!(!both());
        runs(&alice_dev, &alice, "mac", "0.6.420", "1", now);
        assert!(!both(), "bob's is still unknown");
        runs(&bob_dev, &bob, "ios", "1.0.0", "499", now);
        assert!(!both(), "bob's build is one short");
        runs(&bob_dev, &bob, "ios", "1.0.0", "500", now + 1);
        assert!(both());
        // What a device said last is what counts, not the best it ever said.
        runs(&bob_dev, &bob, "ios", "0.9.0", "900", now + 2);
        assert!(!both());
        runs(&bob_dev, &bob, "ios", "1.0.1", "1", now + 3);
        assert!(both());
        // A second device of Alice's on an old build, in use: not yet.
        let old = arbos_core::ids::ulid();
        state
            .db
            .insert_device(&old, "old mac", &arbos_core::ids::ulid(), &alice, "github")
            .unwrap();
        runs(&old, &alice, "mac", "0.6.300", "1", now);
        assert!(!both());
        // The same device last seen two months ago does not hold her back; signed out, neither.
        seen(&old, now - 2 * ACTIVE_DEVICE_MS);
        assert!(both());
        seen(&old, now);
        assert!(!both());
        state.db.revoke_device_fully(&old).unwrap();
        assert!(both());
        // A platform with no minimum is not known to decrypt.
        runs(&bob_dev, &bob, "android", "9.9.9", "1", now + 4);
        assert!(!both());
        // Someone with no device at all reads nothing encrypted.
        assert!(!clients_decrypt(&state, ["gh_nobody"].into_iter()));
    }

    #[test]
    fn tokens_by_namespace() {
        let mut c = cfg();
        c.as_token_users = "U".into();
        c.as_token_agents = "A".into();
        c.as_token_bridges = "B".into();
        assert_eq!(c.as_token_for("@gh_1:arbos.life"), "U");
        assert_eq!(c.as_token_for("@_a_x:arbos.life"), "A");
        assert_eq!(c.as_token_for("@_x_x:arbos.life"), "A");
        assert_eq!(c.as_token_for("@_b_x:arbos.life"), "B");
    }

    #[test]
    fn job_json_is_tagged() {
        let j = Job::Read {
            chat_id: "c".into(),
            user_id: "u".into(),
        };
        let v = serde_json::to_value(&j).unwrap();
        assert_eq!(v["kind"], "read");
        assert_eq!(serde_json::from_value::<Job>(v).unwrap(), j);
    }
}
