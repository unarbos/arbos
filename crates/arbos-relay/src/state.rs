//! Shared relay state: db, config, per-room broadcast channels, box providers.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::{broadcast, Mutex, RwLock};

use arbos_core::wire::BoxState;

use crate::daytona::Daytona;
use crate::db::Db;

#[derive(Clone)]
pub struct Config {
    pub public_url: String,
    pub data_dir: PathBuf,
    pub admin_token: Option<String>,
    pub openrouter_key: Option<String>,
    pub anthropic_key: Option<String>,
    pub openai_key: Option<String>,
    pub openai_base: String,
    /// Upstream for `/proxy/v1/images/generations` (OpenAI's own API; OpenRouter has no images endpoint).
    pub openai_images_base: String,
    /// Where `openai/*` chat models go when `OPENAI_API_KEY` exists (`ARBOS_OPENAI_DIRECT_BASE`,
    /// default `https://api.openai.com/v1`). OpenRouter's OpenAI route refuses our account
    /// ("blocked for a previous policy violation", 2026-10-20); the direct route is first-party.
    pub openai_direct_base: String,
    /// `ARBOS_OPENAI_ROUTE`: `direct` (default when a key exists), `openrouter`, or `auto`
    /// (= direct when `OPENAI_API_KEY` is set, else OpenRouter).
    pub openai_route: OpenAiRoute,
    pub anthropic_base: String,
    pub daytona_snapshot: String,
    /// Inline Dockerfile for Daytona's declarative builder when the snapshot is not registered.
    pub daytona_dockerfile: Option<PathBuf>,
    /// Directory served at `/bootstrap/{file}` to boxes (arbos-agentd binary, arbos-pi-ext.tar).
    pub bootstrap_dir: Option<PathBuf>,
    pub agentd_bin: PathBuf,
    pub local_box_image: String,
    pub box_idle_grace_ms: i64,
    /// `ARBOS_DEFAULT_PROVIDER`: where a user without a box gets one (`daytona|process|docker|gvisor`);
    /// unset = Daytona when configured, else a local process. D89 sets `gvisor` on the relay host.
    pub default_provider: Option<String>,
    /// Host fleet knobs (D89).
    pub fleet: crate::fleet::FleetCfg,
    /// `ARBOS_CONNECTORS_MOCK=1`: connectors answer with canned data (F1).
    pub connectors_mock: bool,
    /// `ARBOS_CHANNELS_MOCK=1`: channel sends land in `channel_outbox`; `/channels/mock/inbound` is open (F7).
    pub channels_mock: bool,
    /// `ARBOS_IMAGE_MOCK=1`: image generation answers with a labelled placeholder PNG (F10).
    pub image_mock: bool,
    /// Waits before the avatar generator's retries of a transient provider failure (timeout,
    /// 429, 5xx) on one model — 2 s then 5 s by default (`ARBOS_AVATAR_RETRY_MS=2000,5000`);
    /// tests set it empty or to zeros.
    pub avatar_retry_backoff_ms: Vec<u64>,
    /// How long one provider call for an avatar may take before it counts as a transient failure
    /// (`ARBOS_AVATAR_ATTEMPT_TIMEOUT_S`, default 40), and the whole generate's budget before a
    /// clean 504 (`ARBOS_AVATAR_TOTAL_TIMEOUT_S`, default 75). Jacob's "nuke vampire"
    /// (2026-10-26 17:4x UTC) hung on the provider past the apps' 30 s; the relay had no
    /// timeout at all, the client gave up, the dropped handler logged nothing.
    pub avatar_attempt_timeout: std::time::Duration,
    pub avatar_total_timeout: std::time::Duration,
    /// `ARBOS_VOICE_MOCK=1`: `/proxy/v1/audio/*` answer locally (F11).
    pub voice_mock: bool,
    /// OpenAI-compatible base for audio (STT/TTS); api.openai.com by default
    /// because OpenRouter carries no audio endpoints.
    pub openai_audio_base: String,
    /// `ARBOS_VAULT_MOCK=1`: two demo vault items and a local echo target (F8).
    pub vault_mock: bool,
    /// How devices sign in: GitHub OAuth (self-serve) or one-time pairing codes (self-hosters).
    pub auth_mode: AuthMode,
    pub github_client_id: Option<String>,
    pub github_client_secret: Option<String>,
    /// Extra allowed `redirect` prefixes for `/auth/github/start` (`ARBOS_AUTH_REDIRECTS`).
    pub auth_redirects: Vec<String>,
    /// Credits a new GitHub account starts with (`ARBOS_DEFAULT_GRANT_USD`).
    pub default_grant_usd: f64,
    /// Soft per-user daily debit cap for new accounts (`ARBOS_DEFAULT_DAILY_CAP_USD`).
    pub default_daily_cap_usd: f64,
    /// Relay-wide daily inference debit cap; reaching it trips the kill switch (`ARBOS_GLOBAL_DAILY_CAP_USD`).
    pub global_daily_cap_usd: f64,
    /// Relay-wide daily machine cost that only logs a warning (`ARBOS_GLOBAL_MACHINE_DAILY_CAP_USD`).
    pub global_machine_daily_cap_usd: f64,
    /// Machines always on (`ARBOS_ALWAYS_ON`, default on in GitHub mode): no idle auto-stop,
    /// a box per signed-in user from sign-in on, re-woken after a relay restart.
    pub always_on: bool,
    /// Our Daytona cost for a running box (`ARBOS_MACHINE_USD_PER_HOUR`).
    pub machine_usd_per_hour: f64,
    /// Our Daytona cost for a stopped box's disk (`ARBOS_MACHINE_STOPPED_USD_PER_DAY`).
    pub machine_stopped_usd_per_day: f64,
    /// `owner/repo` whose `edge` release feeds `GET /download/mac` (`ARBOS_RELEASES_REPO`).
    pub releases_repo: String,
    /// Tree avatar slugs in manifest order (`ARBOS_AVATAR_MANIFEST` / `ARBOS_AVATAR_SLUGS`).
    pub avatar_slugs: Vec<String>,
    /// Avatar colour palette in manifest order (`colors[]` / `ARBOS_AVATAR_COLORS`).
    pub avatar_colors: Vec<String>,
    /// Public site that fronts invite links (`ARBOS_SITE_URL`, default `https://arbos.life`):
    /// `<site>/i/<code>` is served by the relay's `GET /i/{code}`.
    pub site_url: String,
    /// `ARBOS_MULTI_CHAT=1`: agents may open extra chats/tabs (the "Projects ⇄" tab, box-created
    /// chats). Off by default: one chat per agent; a box asking for a new chat gets the primary one.
    pub multi_chat: bool,
}

/// How `openai/*` models reach OpenAI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAiRoute {
    Auto,
    Direct,
    OpenRouter,
}

impl OpenAiRoute {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "direct" | "openai" => OpenAiRoute::Direct,
            "openrouter" => OpenAiRoute::OpenRouter,
            _ => OpenAiRoute::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    Github,
    Paircode,
}

impl AuthMode {
    pub fn as_str(self) -> &'static str {
        match self {
            AuthMode::Github => "github",
            AuthMode::Paircode => "paircode",
        }
    }
}

/// One frame to fan out to the other sockets in a room.
#[derive(Clone, Debug)]
pub struct RoomBroadcast {
    pub from_conn: u64,
    pub bytes: Bytes,
    /// Row seq for `ROW` frames; 0 for presence/status/error frames, which pass straight through.
    pub seq: i64,
}

/// A per-room value computed from the stored doc, valid for one `(head, floor)` — a checkpoint
/// changes the stored doc without moving the head, so both are part of the key.
pub type HeadCache<T> = std::sync::Mutex<Option<((i64, i64), T)>>;

/// Cached rebuild of a room's stored doc (checkpoint ∪ rows) at one `(head, floor)`. When the
/// head advances, only the new rows are imported; the first build runs off the socket task.
pub struct RoomDocCache {
    pub key: (i64, i64),
    pub doc: arbos_core::doc::ChatDoc,
}

pub struct Room {
    pub tx: broadcast::Sender<RoomBroadcast>,
    /// What each replica last told this relay about the room (room_health, #1470).
    pub replicas: crate::room_health::Replicas,
    /// `(head, rebuilt-doc verdict)` cache for `GET /chats/{id}/health`.
    pub health_cache: HeadCache<crate::room_health::DocHealth>,
    /// Rebuilt chat doc for covered-push checks and ROWS_DONE's version vector. Dropped when the
    /// room has no sockets left.
    pub doc_cache: std::sync::Mutex<Option<RoomDocCache>>,
    /// Box users (owners) with a live socket on this room: a row from a person reaches their
    /// agent live only through such a socket; without one the box is told to open the room
    /// (`boxes::after_person_row`, #2120).
    pub boxes: std::sync::Mutex<HashMap<String, u32>>,
}

impl Room {
    pub fn box_joined(&self, user_id: &str) {
        *self
            .boxes
            .lock()
            .unwrap()
            .entry(user_id.to_owned())
            .or_insert(0) += 1;
    }

    pub fn box_left(&self, user_id: &str) {
        let mut b = self.boxes.lock().unwrap();
        if let Some(n) = b.get_mut(user_id) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                b.remove(user_id);
            }
        }
    }

    /// Does `user_id`'s box have a socket open on this room right now?
    pub fn has_box(&self, user_id: &str) -> bool {
        self.boxes.lock().unwrap().contains_key(user_id)
    }
}

/// How the relay tells its room sockets that it is about to go down on purpose (a deploy, an
/// image-roll fallback restart, SIGTERM). `closing` flips to true once; every room socket then
/// sends a 1012 Service Restart close and ends, and `open_sockets` counts down so the restart
/// can drain before the process exits. A socket that arrives while closing is closed at once.
pub struct RestartSignal {
    pub closing: tokio::sync::watch::Sender<bool>,
    pub open_sockets: std::sync::atomic::AtomicUsize,
    /// A planned restart is under way: no box upgrade or repair may start (#2262). Set and read
    /// under the `upgrading` lock so a run cannot slip in between the check and the restart.
    pub box_work_frozen: std::sync::atomic::AtomicBool,
    /// Users whose `create_box` is between its first row write and the new sandbox id landing
    /// in the row, with the start time (#2262: a restart in that window orphaned a sandbox).
    pub creating: std::sync::Mutex<HashMap<String, i64>>,
}

impl Default for RestartSignal {
    fn default() -> Self {
        Self {
            closing: tokio::sync::watch::Sender::new(false),
            open_sockets: std::sync::atomic::AtomicUsize::new(0),
            box_work_frozen: std::sync::atomic::AtomicBool::new(false),
            creating: std::sync::Mutex::new(HashMap::new()),
        }
    }
}

impl RestartSignal {
    pub fn is_closing(&self) -> bool {
        *self.closing.borrow()
    }

    pub fn box_work_frozen(&self) -> bool {
        self.box_work_frozen
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Marks one `create_box` as in flight for as long as it lives; dropped on every exit path,
/// including a cancelled task.
pub struct CreatingGuard {
    restart: Arc<RestartSignal>,
    user_id: String,
}

impl CreatingGuard {
    pub fn new(restart: &Arc<RestartSignal>, user_id: &str) -> Self {
        restart
            .creating
            .lock()
            .expect("creating lock")
            .insert(user_id.to_owned(), arbos_core::ids::now_ms());
        Self {
            restart: restart.clone(),
            user_id: user_id.to_owned(),
        }
    }
}

impl Drop for CreatingGuard {
    fn drop(&mut self) {
        if let Ok(mut m) = self.restart.creating.lock() {
            m.remove(&self.user_id);
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub cfg: Arc<Config>,
    pub http: reqwest::Client,
    pub daytona: Option<Daytona>,
    pub rooms: Arc<RwLock<HashMap<String, Arc<Room>>>>,
    /// user_id → live state of the user's box as the relay believes it (STATUS frames, crdt-sync §8).
    pub box_states: Arc<RwLock<HashMap<String, (BoxState, i64)>>>,
    /// user_id → local child process (process provider only).
    pub local_children: Arc<Mutex<HashMap<String, tokio::process::Child>>>,
    /// user_id → (open box sockets, last disconnect ms).
    pub box_conns: Arc<Mutex<HashMap<String, (u32, i64)>>>,
    /// user_id → ms when the current wake started (for wake timing).
    pub wake_started: Arc<Mutex<HashMap<String, i64>>>,
    /// user_id → ms of the last client row; rows pushed while a box was dying must re-wake it.
    pub last_client_push: Arc<Mutex<HashMap<String, i64>>>,
    /// `POST /proxy/decisions` answers by request hash (`proxy::DecisionsCache`): every box in
    /// a people room asks the judge the same question about one message, and one answer for
    /// all of them is what makes their verdicts agree.
    pub decisions_cache: Arc<std::sync::Mutex<crate::proxy::DecisionsCache>>,
    /// Identical decision requests in flight at the same moment (the three agents of one box
    /// judge a message within milliseconds of each other): one per-key lock so the first
    /// call fills the cache and the rest read it, instead of three upstream calls with three
    /// different answers (2026-10-28 06:30: two takers for one "any of you" ask).
    pub decisions_inflight: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    /// user_id → wakes that produced no hello within the deadline (stale-box detector).
    pub stale_bounces: Arc<Mutex<HashMap<String, u32>>>,
    pub conn_seq: Arc<std::sync::atomic::AtomicU64>,
    /// Encrypts connector tokens and vault items at rest (F1, F8).
    pub relay_key: Arc<crate::connectors::crypto::RelayKey>,
    /// device_id → live `arbos-local` helper socket (F9).
    pub local_conns: crate::local::LocalConns,
    /// HS256 key for access tokens (`ARBOS_JWT_KEY`, else derived from the relay key).
    pub jwt_key: Arc<[u8; 32]>,
    /// `(fetched_at_ms, dmg_url)` for `GET /download/mac`.
    pub download_cache: Arc<Mutex<Option<(i64, String)>>>,
    /// user_id → box upgrade in progress (`POST /machine/upgrade`).
    pub upgrading: Arc<Mutex<HashMap<String, UpgradeStatus>>>,
    /// user_id → the last upgrade the relay refused before touching the box (ARB-339: `/work`
    /// over the carry limit). Shown as `upgrade.last_refused`; cleared by the next success.
    pub upgrade_refused: Arc<Mutex<HashMap<String, UpgradeRefusal>>>,
    /// Hash of a just-rotated refresh token → (when, the pair it was rotated into). Within
    /// `signin::REFRESH_GRACE_MS` the old token gets that same pair again instead of counting
    /// as a replay, so concurrent refreshers from one device converge on one rotation.
    pub refresh_grace: Arc<Mutex<HashMap<String, (i64, serde_json::Value)>>>,
    /// Message notifications transport (APNs, a capture list in tests, or off).
    pub push: Arc<crate::push::Push>,
    /// device_id → the chat its presence says is on screen (suppresses that chat's pushes).
    pub focus: crate::notify::Focus,
    /// (chat_id, sender user) → when the row-level fallback last notified for it.
    pub fallback_sent: Arc<Mutex<HashMap<(String, String), i64>>>,
    /// Self-hosted LiveKit for calls (`LIVEKIT_*`); `None` = calls answer 503.
    pub livekit: Option<crate::livekit::LiveKit>,
    /// device_id → last presence frame that was written to `devices.last_seen_at`.
    pub presence_seen: Arc<Mutex<HashMap<String, i64>>>,
    /// project_id → what its agent is doing right now (agent-flow contract §3), in memory only.
    pub activity: Arc<Mutex<HashMap<String, ProjectActivity>>>,
    /// user_id → last HTTP call from that user's box (`/box/hello`, `/box/projects`,
    /// `/box/keepalive`). A box with no project opens no room socket; this is its heartbeat.
    pub box_http_seen: Arc<Mutex<HashMap<String, i64>>>,
    /// sandbox_id → the Daytona preview token learned after the row's one got a 401
    /// (`boxes::send_to_box`). Memory only: the row is rewritten only by a start the relay finishes.
    pub fresh_preview_tokens: Arc<Mutex<HashMap<String, String>>>,
    /// Users whose box said "turn running" in its last keepalive, with the time (D89: eviction skips them).
    pub turn_running: Arc<Mutex<HashMap<String, i64>>>,
    /// Sandboxes evicted under host pressure (D89), with the time; only the user's own activity wakes them.
    pub evicted: Arc<Mutex<HashMap<String, i64>>>,
    /// Last host pressure sample (D89), for `/admin/fleet`.
    pub fleet_pressure: Arc<Mutex<Option<crate::fleet::Pressure>>>,
    /// Agent voice calls (Phase 2): caps and the LiveKit agent name.
    pub voice: Arc<crate::voice::VoiceCfg>,
    /// user_id → the box's `WS /box/voice` link (tool calls and transcript rows to agentd).
    pub voice_conns: crate::voice::VoiceConns,
    /// The Daytona snapshot new boxes are made from, switchable at runtime (`POST /admin/snapshot/default`)
    /// so an image roll no longer restarts the relay — every restart dropped every client's websocket
    /// (2026-10-22 18:5x: four restarts in 95 min, "Reconnecting…" banners in Jacob's chats).
    pub default_snapshot: Arc<std::sync::RwLock<String>>,
    /// Process start, for `healthz.uptime_s` (a restart storm shows as a short uptime).
    pub started_at: std::time::Instant,
    /// Planned-restart signal: room sockets close with 1012 when it flips (`restart.rs`).
    pub restart: Arc<RestartSignal>,
    /// `call:project` → when the voice worker was last dispatched for it (no double dispatch).
    pub voice_dispatched: Arc<Mutex<HashMap<String, i64>>>,
    /// What the voice worker last reported about itself (`POST /voice/worker/hello`).
    pub voice_worker: Arc<Mutex<Option<serde_json::Value>>>,
    /// What landed in a chat while an agent call is live there (#1832): per (call, agent), the
    /// finished bubbles and human messages the voice worker long-polls into the call's context.
    pub voice_inbox: crate::voice::VoiceInbox,
    /// The image provider's last health probe (`images::probe_provider`, every 5 min and on
    /// `GET /admin/images/health?probe=1`); `healthz.images` reads it.
    pub image_health: Arc<Mutex<Option<crate::images::ImageHealth>>>,
    /// device_id → the app version it last reported (`X-Arbos-Client`), see `releases.rs`.
    pub client_seen: Arc<crate::releases::ClientSeen>,
    /// Chats on Matrix (D88): mode, homeserver tokens and the outbox wake-up.
    pub matrix: Arc<crate::matrix::Matrix>,
}

impl AppState {
    /// The snapshot new boxes are created from: the runtime override when set, else the configured one.
    pub fn default_snapshot(&self) -> String {
        let cur = self
            .default_snapshot
            .read()
            .map(|s| s.clone())
            .unwrap_or_default();
        if cur.is_empty() {
            self.cfg.daytona_snapshot.clone()
        } else {
            cur
        }
    }
}

/// The agent's current activity as the box last reported it.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ProjectActivity {
    pub chat_id: String,
    pub text: String,
    pub at: i64,
    /// Background jobs in flight for the chip ("N tasks running"; duplex design §2.4):
    /// `{ id, title, state, step, elapsed_s }` each. Absent when none run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jobs: Option<serde_json::Value>,
}

/// An activity older than this is stale (the box died mid-turn) and is not shown.
pub const ACTIVITY_TTL_MS: i64 = 20 * 60 * 1000;

/// One running box upgrade.
#[derive(Debug, Clone)]
pub struct UpgradeStatus {
    pub phase: String,
    pub since: i64,
    pub from_snapshot: Option<String>,
    pub to_snapshot: String,
}

/// An upgrade refused at the measure step: the box kept running, nothing was stopped or copied.
#[derive(Debug, Clone)]
pub struct UpgradeRefusal {
    pub reason: String,
    pub at: i64,
    pub work_mib: i64,
}

impl AppState {
    pub async fn room(&self, name: &str) -> Arc<Room> {
        if let Some(r) = self.rooms.read().await.get(name) {
            return r.clone();
        }
        let mut w = self.rooms.write().await;
        w.entry(name.to_owned())
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(1024);
                Arc::new(Room {
                    tx,
                    replicas: Default::default(),
                    health_cache: std::sync::Mutex::new(None),
                    doc_cache: std::sync::Mutex::new(None),
                    boxes: std::sync::Mutex::new(HashMap::new()),
                })
            })
            .clone()
    }

    /// The user whose box serves a project.
    pub fn user_of(&self, project_id: &str) -> Option<String> {
        self.db
            .project(project_id)
            .ok()
            .flatten()
            .map(|p| p.user_id)
    }

    /// May this principal touch this project (by id or name)? Unknown projects: no.
    pub fn allows(&self, principal: &crate::auth::Principal, project_id: &str) -> bool {
        if let Some(chat_id) = project_id.strip_prefix(crate::people::PEOPLE_ACL_PREFIX) {
            return crate::people::room_access(self, principal, chat_id);
        }
        // A server's picture slot (`servers:{id}`): its non-banned members read it and may
        // upload into it; `PATCH /servers/{id} {image_key}` (admins) is what sets the picture.
        if let Some(server_id) = project_id.strip_prefix(crate::matrix::servers::SERVER_BLOB_PREFIX)
        {
            return match principal {
                crate::auth::Principal::Device { user_id, .. } => self
                    .db
                    .server_member(server_id, user_id)
                    .ok()
                    .flatten()
                    .is_some_and(|m| m.banned_at.is_none()),
                crate::auth::Principal::Admin => true,
                crate::auth::Principal::Box { .. } => false,
            };
        }
        // A person's own picture slot (`users:{id}`): the generated previews of the avatar
        // picker's Generate tab. Only that person's devices (and the admin) read it; `PUT
        // /me/avatar {image_key}` is what makes one of them the profile picture.
        if let Some(user_id) = project_id.strip_prefix(crate::images::USER_BLOB_PREFIX) {
            return match principal {
                crate::auth::Principal::Device { user_id: me, .. } => me == user_id,
                crate::auth::Principal::Admin => true,
                crate::auth::Principal::Box { .. } => false,
            };
        }
        match self.user_of(project_id) {
            Some(u) if principal.can_access_project(&u) => true,
            // Projects together (D72 step 3): a person the owner invited drives the project too.
            Some(_) => match principal {
                crate::auth::Principal::Device { user_id, .. } => {
                    let id = self
                        .db
                        .project(project_id)
                        .ok()
                        .flatten()
                        .map(|p| p.id)
                        .unwrap_or_else(|| project_id.to_owned());
                    self.db.is_project_member(&id, user_id).unwrap_or(false)
                }
                crate::auth::Principal::Box { .. } | crate::auth::Principal::Admin => false,
            },
            None => false,
        }
    }

    /// Owner-level access: the project's own user (any of their devices, their box) or the admin;
    /// guests of a shared project are not owners.
    pub fn owns(&self, principal: &crate::auth::Principal, project_id: &str) -> bool {
        match self.user_of(project_id) {
            Some(u) => principal.can_access_project(&u),
            None => false,
        }
    }

    pub async fn box_state(&self, user_id: &str) -> (BoxState, i64) {
        self.box_states
            .read()
            .await
            .get(user_id)
            .copied()
            .unwrap_or((BoxState::Asleep, 0))
    }

    /// Box state seen from one project (its user's box).
    pub async fn project_box_state(&self, project_id: &str) -> (BoxState, i64) {
        match self.user_of(project_id) {
            Some(u) => self.box_state(&u).await,
            None => (BoxState::Asleep, 0),
        }
    }

    pub async fn set_box_state(&self, user_id: &str, st: BoxState, detail: Option<&str>) {
        let now = arbos_core::ids::now_ms();
        let changed = {
            let mut w = self.box_states.write().await;
            let prev = w.get(user_id).map(|x| x.0);
            w.insert(user_id.to_owned(), (st, now));
            prev != Some(st)
        };
        if st == BoxState::Awake {
            self.wake_started.lock().await.remove(user_id);
        }
        let arbos = match st {
            BoxState::Awake => "awake",
            BoxState::Waking => "waking",
            BoxState::Asleep => "hibernated_light",
            BoxState::Error => "error",
        };
        // The project state column mirrors the user's box for every project of that user.
        for p in self.db.projects_of_user(user_id).unwrap_or_default() {
            let _ = self.db.set_project_state(&p.id, arbos);
            if st != BoxState::Awake {
                let _ = self.db.set_workspace_state(&p.id, "stopped");
            }
        }
        if changed {
            self.broadcast_status(user_id, st, detail).await;
        }
    }

    /// `STATUS` frame to every room of every project of the user (crdt-sync §7.4, 0x0c).
    pub async fn broadcast_status(&self, user_id: &str, st: BoxState, detail: Option<&str>) {
        let frame = arbos_core::wire::Frame::new(
            arbos_core::wire::FrameType::Status,
            serde_json::to_value(arbos_core::wire::StatusHeader {
                box_state: st,
                since: arbos_core::ids::now_ms(),
                detail: detail.map(str::to_owned),
            })
            .expect("status header"),
            Bytes::new(),
        )
        .encode();
        let prefixes: Vec<String> = self
            .db
            .projects_of_user(user_id)
            .unwrap_or_default()
            .into_iter()
            .map(|p| format!("{}/", p.id))
            .collect();
        let rooms = self.rooms.read().await;
        for (name, room) in rooms.iter() {
            if prefixes.iter().any(|p| name.starts_with(p)) {
                let _ = room.tx.send(RoomBroadcast {
                    from_conn: 0,
                    bytes: frame.clone(),
                    seq: 0,
                });
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! One relay state over a fresh temp SQLite file, for unit tests across modules.

    use std::collections::HashMap;
    use std::sync::atomic::AtomicU64;
    use std::sync::Arc;

    use tokio::sync::{Mutex, RwLock};

    use arbos_core::ids::ulid;

    use super::{AppState, AuthMode, Config, RestartSignal};
    use crate::connectors::crypto::RelayKey;
    use crate::db::Db;

    pub fn test_state(auth_mode: AuthMode) -> AppState {
        let dir = std::env::temp_dir().join(format!("arbos-credits-test-{}", ulid()));
        let db = Db::open(&dir.join("relay.db")).expect("db");
        let cfg = Config {
            public_url: "http://127.0.0.1:7707".into(),
            data_dir: dir,
            admin_token: Some("adm_test".into()),
            openrouter_key: None,
            anthropic_key: None,
            openai_key: None,
            openai_base: String::new(),
            anthropic_base: String::new(),
            daytona_snapshot: String::new(),
            daytona_dockerfile: None,
            bootstrap_dir: None,
            agentd_bin: "arbos-agentd".into(),
            local_box_image: String::new(),
            box_idle_grace_ms: 0,
            default_provider: None,
            fleet: crate::fleet::FleetCfg::default(),
            connectors_mock: true,
            channels_mock: true,
            openai_images_base: String::new(),
            openai_direct_base: "https://api.openai.com/v1".into(),
            openai_route: super::OpenAiRoute::Auto,
            image_mock: false,
            avatar_retry_backoff_ms: Vec::new(),
            avatar_attempt_timeout: std::time::Duration::from_secs(40),
            avatar_total_timeout: std::time::Duration::from_secs(75),
            voice_mock: true,
            openai_audio_base: String::new(),
            vault_mock: false,
            auth_mode,
            github_client_id: Some("id".into()),
            github_client_secret: Some("secret".into()),
            auth_redirects: Vec::new(),
            default_grant_usd: 100.0,
            default_daily_cap_usd: 20.0,
            global_daily_cap_usd: 500.0,
            global_machine_daily_cap_usd: 400.0,
            always_on: false,
            machine_usd_per_hour: 0.332,
            machine_stopped_usd_per_day: 0.013,
            releases_repo: "unarbos/arbos".into(),
            site_url: "https://arbos.life".into(),
            multi_chat: false,
            avatar_slugs: crate::avatar::DEFAULT_SLUGS
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            avatar_colors: crate::avatar::DEFAULT_COLORS
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
        };
        AppState {
            db: db.clone(),
            cfg: Arc::new(cfg),
            http: reqwest::Client::new(),
            daytona: None,
            rooms: Arc::new(RwLock::new(HashMap::new())),
            box_states: Arc::new(RwLock::new(HashMap::new())),
            local_children: Arc::new(Mutex::new(HashMap::new())),
            box_conns: Arc::new(Mutex::new(HashMap::new())),
            wake_started: Arc::new(Mutex::new(HashMap::new())),
            last_client_push: Arc::new(Mutex::new(HashMap::new())),
            decisions_cache: Arc::new(std::sync::Mutex::new(Default::default())),
            decisions_inflight: Arc::new(Mutex::new(HashMap::new())),
            conn_seq: Arc::new(AtomicU64::new(0)),
            relay_key: Arc::new(RelayKey::derive("test-key")),
            jwt_key: Arc::new(RelayKey::derive("test-key").subkey("jwt")),
            download_cache: Arc::new(Mutex::new(None)),
            upgrading: Arc::new(Mutex::new(HashMap::new())),
            upgrade_refused: Arc::new(Mutex::new(HashMap::new())),
            stale_bounces: Arc::new(Mutex::new(HashMap::new())),
            local_conns: Arc::new(Mutex::new(HashMap::new())),
            refresh_grace: Arc::new(Mutex::new(HashMap::new())),
            push: Arc::new(crate::push::Push::capture(db.clone())),
            focus: Arc::new(Mutex::new(HashMap::new())),
            fallback_sent: Arc::new(Mutex::new(HashMap::new())),
            livekit: None,
            presence_seen: Arc::new(Mutex::new(HashMap::new())),
            activity: Arc::new(Mutex::new(HashMap::new())),
            box_http_seen: Arc::new(Mutex::new(HashMap::new())),
            fresh_preview_tokens: Arc::new(Mutex::new(HashMap::new())),
            turn_running: Arc::new(Mutex::new(HashMap::new())),
            evicted: Arc::new(Mutex::new(HashMap::new())),
            fleet_pressure: Arc::new(Mutex::new(None)),
            voice: Arc::new(crate::voice::VoiceCfg::default()),
            voice_conns: Arc::new(Mutex::new(HashMap::new())),
            default_snapshot: Arc::new(std::sync::RwLock::new(String::new())),
            started_at: std::time::Instant::now(),
            restart: Arc::new(RestartSignal::default()),
            voice_dispatched: Arc::new(Mutex::new(HashMap::new())),
            voice_worker: Arc::new(Mutex::new(None)),
            voice_inbox: crate::voice::VoiceInbox::default(),
            image_health: Arc::new(Mutex::new(None)),
            client_seen: Arc::new(std::sync::Mutex::new(HashMap::new())),
            matrix: crate::matrix::Matrix::new(crate::matrix::Config::off()),
        }
    }
}
