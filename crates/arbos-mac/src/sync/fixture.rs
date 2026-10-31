//! Local, offline source. Every chat is a real Loro doc (`doc.rs`); the
//! "agent" is a scripted tokio task that writes into the doc the way the box
//! would: a `deciding` status, tool cards flipping from running to done, text
//! appended in 120 ms frames, a final status. The UI cannot tell it apart from
//! the relay path.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use arbos_core::approval::{
    ApprovalRequest, ApprovalState as CoreApprovalState, Risk, APPROVAL_TTL_MS,
};
use arbos_core::doc::{
    ChatDoc as CoreDoc, MessageStatus as CoreStatus, UserAttachment, UserMessageOpts,
};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::time::sleep;

use crate::model::{
    AgentMember, Call, CallEnded, CallKind, LinkState, Machine, MachineState, MediaPick, Person,
    ProjectDocView, ReplyPolicy, ReplyRef, Sender, SpaceKind, ThreadInfo, Voice,
};

use super::doc::ChatDoc;
use super::user_error::{ErrorClass, UserError};
use super::{
    ClientCommand, DraftPicture, PictureSource, PictureTarget, PushRefusal, SyncEvent, SyncSource,
};
use crate::model::{
    new_id, now_ms, ApprovalRisk, ApprovalState, Attachment, AttachmentKind, AttachmentSource,
    BoxReport, BoxState, BoxWorkspace, ChannelBinding, ChannelPairing, ChannelPlatform,
    ChannelPlatformState, ChannelsView, Chat, ChatKind, ChoiceOption, Connector, ConnectorAction,
    ConnectorProvider, ConnectorStatus, ConnectorsView, DeviceView, Effort, FieldKind, FormField,
    Id, Job, JobRollout, JobState, Link, LocalDevice, LocalView, Message, MessageStatus, Part,
    Project, QuestionState, Role, RolloutEntry, RolloutKind, RolloutParent, Routine, RoutineRun,
    RoutineRunStatus, RoutineTrigger, Secret, SecretActivity, SecretAgent, SecretDirector,
    SecretRequest, SecretScope, SecretsView, ServerChannel, ServerInfo, Skill, StatusKind,
    ToolCall, ToolStatus, TurnKind, Widget,
};

pub const STREAM_FRAME_MS: u64 = arbos_core::STREAM_COMMIT_MS;
const DEVICE: &str = "mac:fixture";

pub const PROJECT_ARBOS: &str = "01J9ARBOS00000000000000000";
pub const PROJECT_AFFINE: &str = "01J9AFFINE0000000000000000";
pub const PROJECT_COMMS: &str = "01J9COMMS00000000000000000";

pub const CHAT_ARBOS_MAIN: &str = "01J9CHAT0ARBOS000000000000";
pub const CHAT_ARBOS_RESEARCH: &str = "01J9CHAT0RESEARCH000000000";
pub const CHAT_ARBOS_LAUNCH: &str = "01J9CHAT0LAUNCH00000000000";
pub const CHAT_ARBOS_WIDGETS: &str = "01J9CHAT0WIDGETS0000000000";
pub const CHAT_ARBOS_DRAFTS: &str = "01J9CHAT0DRAFTS00000000000";
pub const CHAT_ARBOS_MEDIA: &str = "01J9CHAT0MEDIA000000000000";
pub const CHAT_ARBOS_VOICE: &str = "01J9CHAT0VOICE000000000000";
pub const CHAT_AFFINE_MAIN: &str = "01J9CHAT0AFFINE00000000000";
pub const CHAT_COMMS_MAIN: &str = "01J9CHAT0COMMS000000000000";
/// F6 demo: a group hosted by Arbos with Affine and Comms.
pub const CHAT_ARBOS_GROUP: &str = "01J9CHAT0GROUP000000000000";

pub const MODELS: [&str; 4] = [
    "GPT-6.1-Sol",
    "Claude Fable 5.1",
    "Gemini 4 Argon",
    "Grok 4.7",
];

type Docs = Arc<Mutex<HashMap<Id, ChatDoc>>>;

struct DocChanged {
    chat_id: Id,
    touched: super::doc::Touched,
}

struct Shared {
    docs: Docs,
    projects: Mutex<Vec<Project>>,
    chats: Mutex<Vec<Chat>>,
    open_chat: Mutex<Option<Id>>,
    running: Mutex<HashMap<Id, Arc<AtomicBool>>>,
    skills: Mutex<Vec<Skill>>,
    events: mpsc::UnboundedSender<SyncEvent>,
    doc_tx: mpsc::UnboundedSender<DocChanged>,
    subscriptions: Mutex<Vec<loro::Subscription>>,
    connectors: Mutex<ConnectorsView>,
    channels: Mutex<ChannelsView>,
    secrets: Mutex<SecretsView>,
    /// Standing shell approvals per agent (#1544): Doug starts with one for /tmp.
    shell_grants: Mutex<HashMap<Id, Vec<crate::model::ShellGrant>>>,
    /// Settings → Devices: this account's signed-in devices (this Mac first).
    devices: Mutex<Vec<DeviceView>>,
    routines: Mutex<Vec<Routine>>,
    routine_runs: Mutex<HashMap<Id, Vec<RoutineRun>>>,
    local: Mutex<LocalView>,
    /// Voice memos sent in this session, by blob key (F11; no blob store offline).
    voice_blobs: Mutex<HashMap<String, Arc<Vec<u8>>>>,
    machine: Mutex<Machine>,
    /// People blocked in this session (the relay's `GET /blocks` stand-in, #1545).
    blocked: Mutex<Vec<Person>>,
    /// `ARBOS_FIXTURE_JOBS=<n>`: the open background jobs the relay's activity
    /// record would list (duplex §2.4), per chat, with their hidden rows' ids.
    jobs: Mutex<Vec<FixtureJob>>,
    /// My sidebar pins (row keys, top first) — the relay's `/me/pins` stand-in.
    pins: Mutex<Vec<String>>,
}

/// One seeded job: the relay's row plus when it started (elapsed grows live)
/// and the hidden transcript row that stands for it.
#[derive(Clone, Debug)]
struct FixtureJob {
    job: Job,
    started_at: u64,
    row_id: Id,
}

/// What the offline STT "hears" (the bundled test clip says this).
const FIXTURE_TRANSCRIPT: &str = "Hello Arbos. What is on my plate today?";
const VOICE_SEED_BLOB: &str = "blob/seed/voice-memo";

pub struct FixtureSource;

impl FixtureSource {
    pub fn new() -> Self {
        Self
    }
}

impl SyncSource for FixtureSource {
    fn name(&self) -> &'static str {
        "fixture"
    }

    fn run(
        self: Box<Self>,
        events: mpsc::UnboundedSender<SyncEvent>,
        mut commands: mpsc::UnboundedReceiver<ClientCommand>,
    ) -> BoxFuture<'static, Result<()>> {
        Box::pin(async move {
            let (doc_tx, mut doc_rx) = mpsc::unbounded_channel();
            let shared = Arc::new(Shared {
                docs: Arc::new(Mutex::new(HashMap::new())),
                projects: Mutex::new(seed_projects()),
                chats: Mutex::new(seed_chats()),
                open_chat: Mutex::new(None),
                running: Mutex::new(HashMap::new()),
                skills: Mutex::new(seed_skills()),
                events: events.clone(),
                doc_tx,
                subscriptions: Mutex::new(Vec::new()),
                connectors: Mutex::new(seed_connectors()),
                channels: Mutex::new(seed_channels()),
                secrets: Mutex::new(seed_secrets()),
                shell_grants: Mutex::new(seed_shell_grants()),
                devices: Mutex::new(seed_devices()),
                routines: Mutex::new(seed_routines()),
                routine_runs: Mutex::new(seed_routine_runs()),
                local: Mutex::new(seed_local()),
                voice_blobs: Mutex::new(HashMap::new()),
                blocked: Mutex::new(Vec::new()),
                jobs: Mutex::new(Vec::new()),
                pins: Mutex::new(Vec::new()),
                machine: Mutex::new(Machine {
                    // `ARBOS_FIXTURE_MACHINE=error|waking` seeds a banner state (screenshot rig).
                    state: match std::env::var("ARBOS_FIXTURE_MACHINE").as_deref() {
                        Ok("error") => MachineState::Error,
                        Ok("waking") => MachineState::Waking,
                        _ => MachineState::Awake,
                    },
                    since: Some(now_ms().saturating_sub(42 * 60_000)),
                    spec: Some("4 vCPU · 8 GiB · 10 GiB".into()),
                    class: Some("arbos-agent-0.1.7".into()),
                    spend_today_usd: 5.39,
                    spend_cap_usd: 25.0,
                    detail: None,
                    progress: None,
                    provider: Some("gvisor".into()),
                    // `ARBOS_FIXTURE_WAITLIST=<n>` seeds the D89 waitlist card (screenshot rig).
                    waitlist_position: std::env::var("ARBOS_FIXTURE_WAITLIST")
                        .ok()
                        .and_then(|v| v.parse().ok()),
                    always_on: std::env::var("ARBOS_FIXTURE_ALWAYS_ON")
                        .map(|v| v != "0")
                        .unwrap_or(true),
                    cpu: Some(4),
                    mem_gib: Some(8),
                    image: Some("arbos-agent-0.1.7".into()),
                }),
            });
            for doc in seed_chat_docs().into_iter().chain(seed_people_chat_docs()) {
                add_doc(&shared, doc);
            }
            shared
                .projects
                .lock()
                .unwrap()
                .extend(seed_people_projects());
            shared.chats.lock().unwrap().extend(seed_people_chats());

            let _ = events.send(SyncEvent::Link(Link::Connecting));
            // The signed-in user, so Settings shows its profile header and credits.
            let _ = events.send(SyncEvent::Account(Some(crate::model::AccountView {
                id: ME_ID.into(),
                login: ME_HANDLE.into(),
                name: "Jacob Steeves".into(),
                avatar_url: String::new(),
                device_name: "Fixture".into(),
                balance_usd: 84.5,
                spent_today_usd: 1.6,
                daily_cap_usd: Some(20.0),
            })));
            // Same default as the relay (off); `ARBOS_FIXTURE_VOICE_MODE=1` shows the voice UI.
            let _ = events.send(SyncEvent::Features {
                voice_mode: std::env::var("ARBOS_FIXTURE_VOICE_MODE")
                    .is_ok_and(|v| matches!(v.trim(), "1" | "true" | "on" | "yes")),
            });
            emit_projects(&shared);
            emit_people_seed(&shared);
            emit_machine(&shared);
            emit_chats(&shared);
            {
                let docs = shared.docs.lock().unwrap();
                for (chat_id, doc) in docs.iter() {
                    let _ = events.send(SyncEvent::ChatSnapshot {
                        chat_id: chat_id.clone(),
                        messages: doc.read_all(),
                    });
                }
            }
            emit_project_docs(&shared);
            emit_connectors(&shared);
            emit_skills(&shared);
            emit_channels(&shared);
            emit_local(&shared);
            emit_secrets(&shared);
            emit_open_request(&shared);
            emit_pins_hook(&shared);
            emit_servers_hook(&shared);
            let _ = events.send(SyncEvent::Link(Link::Live));
            emit_link_hook(&shared);
            emit_blocked_hook(&shared);
            emit_failure_hook(&shared);
            emit_typing_hook(&shared);
            emit_working_hook(&shared);
            emit_jobs_hook(&shared);

            loop {
                tokio::select! {
                    changed = doc_rx.recv() => {
                        let Some(changed) = changed else { break };
                        forward_doc_change(&shared, changed);
                    }
                    cmd = commands.recv() => {
                        let Some(cmd) = cmd else { break };
                        handle_command(&shared, cmd);
                    }
                }
            }
            Ok(())
        })
    }
}

/// `ARBOS_FIXTURE_SERVERS=1` lists three servers (D88 servers otherwise live
/// on the relay only) so a rig can shoot the chat list's Servers segment and
/// the server rows ([feedback sidebar-strip-filter-2026-10-24]).
fn emit_servers_hook(shared: &Arc<Shared>) {
    if !std::env::var("ARBOS_FIXTURE_SERVERS").is_ok_and(|v| v == "1") {
        return;
    }
    let now = now_ms() as i64;
    let servers = [
        (
            "fx-server-hq",
            "Arbos HQ",
            "general",
            "Ken",
            "gate is green, shipping the next build",
            1,
        ),
        (
            "fx-server-affine",
            "Affine",
            "validators",
            "Sami",
            "epoch 412 settled",
            2,
        ),
        (
            "fx-server-hyper",
            "Hyper",
            "trading",
            "You",
            "pausing the dry-run trader",
            3,
        ),
    ];
    let mut list = Vec::new();
    let mut snapshot = arbos_matrix::Snapshot::default();
    for (i, (id, name, channel, who, text, idx)) in servers.iter().enumerate() {
        let last_ts = now - (i as i64) * 2_700_000 - 600_000;
        let room_id = format!("!{id}-space:arbos.life");
        let channel_room = format!("!{id}-{channel}:arbos.life");
        list.push(ServerInfo {
            id: id.to_string(),
            name: name.to_string(),
            owner_user_id: "fx-me".into(),
            space_room_id: Some(room_id.clone()),
            created_at: last_ts - 86_400_000,
            my_role: if i == 0 { "owner" } else { "member" }.into(),
            member_count: 4 + i as u64,
            channels: vec![ServerChannel {
                id: format!("{id}-{channel}"),
                server_id: id.to_string(),
                name: channel.to_string(),
                room_id: Some(channel_room.clone()),
                created_at: last_ts - 86_400_000,
                ..ServerChannel::default()
            }],
            general_channel_id: Some(format!("{id}-{channel}")),
            avatar_color_index_effective: Some(*idx),
            ..ServerInfo::default()
        });
        snapshot.spaces.push(arbos_matrix::SpaceRow {
            room_id,
            server_id: id.to_string(),
            name: name.to_string(),
            member_count: 4 + i as u64,
            unread: u64::from(i == 0),
            last_text: text.to_string(),
            // The fixture names its senders outright (no roster to resolve against).
            last_sender: String::new(),
            last_sender_name: who.to_string(),
            last_ts,
            channel_ids: vec![format!("{id}-{channel}")],
        });
        snapshot.channels.push(arbos_matrix::ChannelRow {
            room_id: channel_room,
            channel_id: format!("{id}-{channel}"),
            server_id: id.to_string(),
            name: format!("#{channel}"),
            restricted: false,
            can_post: true,
            unread: u64::from(i == 0),
            last_text: text.to_string(),
            last_sender: String::new(),
            last_sender_name: who.to_string(),
            last_ts,
            order: format!("{i}"),
        });
    }
    let _ = shared.events.send(SyncEvent::ServersAvailable(true));
    let _ = shared.events.send(SyncEvent::Servers(list));
    let _ = shared.events.send(SyncEvent::MatrixRooms(snapshot));
}

/// `ARBOS_LINK=offline|relay-down|reconnecting|syncing|restart-soon` puts every fixture room
/// in that link state (the header's connection pill, [feedback
/// connection-pill]); `ARBOS_LINK_CLEARS_MS=<ms>` sets them back to live after
/// that long, so a rig can shoot the pill and then its clearing.
fn emit_link_hook(shared: &Arc<Shared>) {
    let Ok(spec) = std::env::var("ARBOS_LINK") else {
        return;
    };
    let state = match spec.trim() {
        "offline" => LinkState::Offline,
        "relay-down" => LinkState::RelayDown,
        "reconnecting" => LinkState::Reconnecting,
        "syncing" => LinkState::Syncing,
        "restart-soon" => LinkState::RestartSoon,
        _ => return,
    };
    let keys: Vec<String> = shared.docs.lock().unwrap().keys().cloned().collect();
    for key in &keys {
        let _ = shared.events.send(SyncEvent::RoomLink {
            key: key.clone(),
            state,
        });
    }
    let Some(clear_after) = std::env::var("ARBOS_LINK_CLEARS_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
    else {
        return;
    };
    let events = shared.events.clone();
    tokio::spawn(async move {
        sleep(Duration::from_millis(clear_after)).await;
        for key in keys {
            let _ = events.send(SyncEvent::RoomLink {
                key,
                state: LinkState::Live,
            });
        }
    });
}

/// Screenshot rig for the shared error element ([feedback error-messages-mac-2026-10-26]):
/// `ARBOS_RIG_FAILURE=toast|toast-repeat|toast-many|composer|delivery|machine|secrets|people` raises that surface's
/// note 1.5 s after start (so the opened chat / pane is up): `toast` = an error toast (a
/// download that timed out, with Try again); `composer` = the opened chat's last own message
/// refused, offline; `delivery` = that chat's pushes refused; `machine` = the machine in error;
/// `secrets` = a 502 on Save (claimed by the open Secrets form); `people` = a refused search.
fn emit_failure_hook(shared: &Arc<Shared>) {
    let Ok(spec) = std::env::var("ARBOS_RIG_FAILURE") else {
        return;
    };
    let spec = spec.trim().to_owned();
    let shared = shared.clone();
    tokio::spawn(async move {
        sleep(Duration::from_millis(1500)).await;
        // Every chat gets the note (the rig does not know which one is open).
        let targets: Vec<(Id, Option<Id>)> = {
            let chats: Vec<Id> = shared
                .chats
                .lock()
                .unwrap()
                .iter()
                .map(|c| c.id.clone())
                .collect();
            let docs = shared.docs.lock().unwrap();
            chats
                .into_iter()
                .map(|id| {
                    let mine = docs.get(&id).and_then(|d| {
                        d.read_all()
                            .into_iter()
                            .rev()
                            .find(|m| m.role == crate::model::Role::User)
                            .map(|m| m.id.clone())
                    });
                    (id, mine)
                })
                .collect()
        };
        match spec.as_str() {
            // [feedback toast-stack-mac-2026-10-27]: Jacob's shape — the secrets poll failing
            // with the resolver's words eight times in a row — and five distinct failures for
            // the "N more" fold.
            "toast-repeat" | "toast-many" => {
                let offline = |path: &str| {
                    anyhow::anyhow!(
                        "error sending request for url (https://relay.arbos.life{path}): client error (Connect): dns error: failed to lookup address information: nodename nor servname provided, or not known"
                    )
                    .context(format!("GET {path}"))
                };
                if spec == "toast-repeat" {
                    for _ in 0..8 {
                        let _ = shared.events.send(SyncEvent::Failure {
                            error: UserError::classify("load your secrets", &offline("/secrets")),
                            retry: None,
                        });
                        sleep(Duration::from_millis(250)).await;
                    }
                } else {
                    for (verb, path) in [
                        ("load your secrets", "/secrets"),
                        ("load the voices", "/voices"),
                        ("download report-q3.pdf", "/blobs/x/report-q3.pdf"),
                        ("save the secret", "/secrets"),
                        ("load the devices", "/devices"),
                    ] {
                        let _ = shared.events.send(SyncEvent::Failure {
                            error: UserError::classify(verb, &offline(path)),
                            retry: None,
                        });
                        sleep(Duration::from_millis(250)).await;
                    }
                }
            }
            "toast" => {
                let raw = anyhow::anyhow!(
                    "error sending request for url (https://relay.arbos.life/blobs/x/report-q3.pdf): operation timed out"
                )
                .context("GET /blobs/01M5N1WSR91G8CJKNYXKJESK7X/report-q3.pdf");
                let _ = shared.events.send(SyncEvent::Failure {
                    error: UserError::classify("download report-q3.pdf", &raw),
                    retry: Some(Box::new(ClientCommand::DevicesRefresh)),
                });
                let _ = shared
                    .events
                    .send(SyncEvent::Notice("Forwarded to Studio".into()));
            }
            "composer" => {
                for (chat_id, last_mine) in targets {
                    let raw = anyhow::anyhow!("dns error: failed to lookup address information")
                        .context("POST /chats/01M5N1WSR91G8CJKNYXKJESK7X/messages");
                    let _ = shared.events.send(SyncEvent::MessageSendFailed {
                        chat_id,
                        message_id: last_mine.unwrap_or_default(),
                        error: UserError::classify("send your message", &raw),
                    });
                }
            }
            "delivery" => {
                for (chat_id, _) in targets {
                    let _ = shared.events.send(SyncEvent::PushRefused {
                        chat_id,
                        refused: true,
                        why: PushRefusal::code("not_available"),
                    });
                }
            }
            // Rig ([feedback blocked-state-mac-2026-10-26]): the relay refused the push
            // because of a block — `blocked-by` (Sam blocked me) / `blocks-member` (I
            // blocked Sam, a DM) — in every chat, so the open one shows the sentence.
            "blocked-by" | "blocks-member" => {
                for (chat_id, _) in targets {
                    let _ = shared.events.send(SyncEvent::PushRefused {
                        chat_id,
                        refused: true,
                        why: PushRefusal {
                            code: "not_available".into(),
                            reason: Some(spec.replace('-', "_")),
                            by_handle: Some("sam".into()),
                            by_name: Some("Sam Rivera".into()),
                        },
                    });
                }
            }
            "machine" => {
                {
                    let mut m = shared.machine.lock().unwrap();
                    m.state = MachineState::Error;
                    m.detail =
                        Some("GET /machine: HTTP 503 from the relay (upstream unreachable)".into());
                }
                emit_machine(&shared);
            }
            "secrets" => {
                let _ = shared.events.send(SyncEvent::Failure {
                    error: UserError::from_status("save the secret", 502, Some("Bad Gateway")),
                    retry: None,
                });
            }
            "people" => {
                let raw = anyhow::anyhow!("error sending request for url (https://relay.arbos.life/users): operation timed out")
                    .context("GET /users");
                let _ = shared
                    .events
                    .send(SyncEvent::PeopleError(UserError::classify("search", &raw)));
            }
            _ => {}
        }
    });
}

/// How many agents `ARBOS_FIXTURE_WORKING=<n>` wants working (0 when unset).
fn fixture_working_count() -> usize {
    std::env::var("ARBOS_FIXTURE_WORKING")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0)
}

/// `ARBOS_FIXTURE_FOREIGN_AGENT=1` (screenshot rig, #1592): the Launch week group holds
/// someone else's agent — "TestAgent", run by @sami, in the roster but not in my project
/// list — with one bubble and, under `ARBOS_FIXTURE_WORKING`, the working row, so the
/// bubble disc, the presence row, the sidebar badge and the `@testagent` chip can be shot
/// for an agent only the roster knows (Jacob's Arbonites screenshot).
fn fixture_foreign_agent() -> bool {
    std::env::var("ARBOS_FIXTURE_FOREIGN_AGENT").is_ok_and(|v| v.trim() == "1")
}

/// `ARBOS_FIXTURE_LONG_ROWS=<n>` (screenshot / perf rig): pad the Launch week
/// group with this many synthetic rows — only the message list grows; the chat
/// header pill stays in the header row ([feedback
/// local-relay-unarbos-20261024065516774-20261024065609], iPhone twin).
fn fixture_long_rows() -> Option<usize> {
    std::env::var("ARBOS_FIXTURE_LONG_ROWS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
}

pub const PROJECT_TESTAGENT: &str = "01J9PROJTESTAGENT000000000";

fn testagent_member() -> AgentMember {
    AgentMember {
        project_id: PROJECT_TESTAGENT.into(),
        name: "TestAgent".into(),
        icon: "T".into(),
        color: "#1084fe".into(),
        handle: "testagent".into(),
        owner_id: "gh_459".into(),
        owner_handle: "sami".into(),
        reply_policy: Default::default(),
        avatar: None,
        avatar_color: None,
        avatar_color_index: Some(3),
        avatar_leaf: Some(3),
        avatar_leaf_color: Some(7),
        image_url: None,
        avatar_version: 0,
        description: None,
        model: None,
        owner: None,
    }
}

/// Extra agents for the Launch week group beyond the seeded Arbos, so the
/// group has `ARBOS_FIXTURE_WORKING` agents to show working.
fn fixture_working_extra_agents() -> Vec<(&'static str, &'static str, &'static str, u8, u8)> {
    const EXTRA: [(&str, &str, &str, u8, u8); 4] = [
        ("01J9PROJLIUM00000000000000", "Lium", "lium", 2, 3),
        ("01J9PROJHOME00000000000000", "Home", "home", 5, 6),
        ("01J9PROJDOUG00000000000000", "Doug", "doug", 7, 1),
        ("01J9PROJNOVA00000000000000", "Nova", "nova", 1, 8),
    ];
    EXTRA
        .into_iter()
        .take(fixture_working_count().saturating_sub(1))
        .collect()
}

/// `ARBOS_FIXTURE_WORKING=<n>` (screenshot rig, [feedback
/// typing-indicator-left-2026-10-23]): the first n agents of every fixture
/// people chat open a streaming row with no words yet — the "<name> is
/// working" state — and keep it open, so the presence row over the composer
/// can be shot with one or several agents working. The agent chats (Affine,
/// Arbos, …) get the same: their one agent streams.
fn emit_working_hook(shared: &Arc<Shared>) {
    let n = fixture_working_count();
    if n == 0 {
        return;
    }
    let spaces: Vec<Project> = shared.projects.lock().unwrap().clone();
    for space in spaces {
        let chats: Vec<Id> = shared
            .chats
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.project_id == space.id)
            .map(|c| c.id.clone())
            .collect();
        for chat_id in chats {
            if space.is_people() {
                for agent in space.agents.iter().take(n) {
                    let id = new_id();
                    let asker = space.person().cloned().unwrap_or_else(sam);
                    with_doc(shared, &chat_id, |doc| {
                        doc.core.transact(|d| {
                            begin_assistant_row(
                                d,
                                &id,
                                &format!("agent:{}", agent.project_id),
                                &[
                                    ("senderName", json!(agent.name)),
                                    ("senderIcon", json!(agent.icon)),
                                    ("senderHandle", json!(agent.handle)),
                                    ("askedByHandle", json!(asker.handle)),
                                    ("ownerHandle", json!(agent.owner_handle)),
                                ],
                                StatusKind::Working,
                                "Working…",
                            )
                        })
                    });
                }
            } else if !space.agents.is_empty() || space.kind == SpaceKind::Project {
                let id = new_id();
                with_doc(shared, &chat_id, |doc| {
                    doc.core.transact(|d| {
                        begin_assistant_row(d, &id, "agent", &[], StatusKind::Working, "Working…")
                    })
                });
            }
        }
    }
}

/// `ARBOS_FIXTURE_JOBS=<n>` (screenshot rig, duplex §2.4): the first n of three
/// background jobs are open in every agent chat — each with its hidden `job` row
/// (what agentd writes) and its entry in the relay's `activity.jobs[]` (what
/// `JobsRefresh` answers): 1 running with a step, 2 running with no step yet,
/// 3 queued behind a lock — the three drawer states. Elapsed grows live; the
/// drawer's Stop removes a job, the Stop button removes them all.
fn fixture_jobs_count() -> usize {
    std::env::var("ARBOS_FIXTURE_JOBS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0)
        .min(3)
}

const FIXTURE_JOBS: [(&str, &str, JobState, &str, u64); 3] = [
    (
        "01J9JOBSURF000000000000000",
        "Searching for surf schools",
        JobState::Running,
        "Reading tripadvisor.com",
        41,
    ),
    (
        "01J9JOBTESTS00000000000000",
        "Fixing the flaky tests",
        JobState::Running,
        "",
        125,
    ),
    (
        "01J9JOBRENAME0000000000000",
        "Renaming the config module",
        JobState::Queued,
        "Waiting for Fixing the flaky tests",
        12,
    ),
];

fn emit_jobs_hook(shared: &Arc<Shared>) {
    let n = fixture_jobs_count();
    if n == 0 {
        return;
    }
    let spaces: Vec<Project> = shared.projects.lock().unwrap().clone();
    let now = now_ms();
    for space in spaces.iter().filter(|s| !s.is_people()) {
        let chats: Vec<Id> = shared
            .chats
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.project_id == space.id)
            .map(|c| c.id.clone())
            .collect();
        for chat_id in chats {
            // The ask the jobs answer: the newest user line of the chat.
            let ask = with_doc(shared, &chat_id, |doc| {
                doc.read_all()
                    .iter()
                    .rev()
                    .find(|m| m.role == Role::User)
                    .map(|m| m.id.clone())
            })
            .flatten();
            for (id, title, state, step, elapsed) in FIXTURE_JOBS.iter().take(n) {
                let job_id = format!("{id}{}", &chat_id[chat_id.len().saturating_sub(4)..]);
                let row_id = new_id();
                let activity = if step.is_empty() { *title } else { step };
                let shown = if n > 1 {
                    format!("{activity} · {n} tasks")
                } else {
                    activity.to_string()
                };
                let fields = [
                    (
                        "job",
                        json!({ "id": job_id, "title": title, "kind": "research" }),
                    ),
                    ("worker", json!(true)),
                    ("hidden", json!(true)),
                    ("inReplyTo", json!(ask.clone().unwrap_or_default())),
                ];
                with_doc(shared, &chat_id, |doc| {
                    doc.core.transact(|d| {
                        begin_assistant_row(
                            d,
                            &row_id,
                            "agent",
                            &fields,
                            StatusKind::Working,
                            "Working…",
                        );
                        d.set_activity(&row_id, Some(&shown));
                    })
                });
                shared.jobs.lock().unwrap().push(FixtureJob {
                    job: Job {
                        id: job_id,
                        title: (*title).to_string(),
                        state: *state,
                        step: (*step).to_string(),
                        elapsed_s: *elapsed,
                        ask_message_id: ask.clone(),
                        chat_id: Some(chat_id.clone()),
                    },
                    started_at: now.saturating_sub(elapsed * 1000),
                    row_id,
                });
            }
        }
    }
}

/// `JobsRefresh`: the relay's `activity.jobs[]` for one project — the seeded
/// jobs of its chats with their elapsed clocks moved on.
fn emit_project_jobs(shared: &Arc<Shared>, project_id: &str) {
    let chats: Vec<Id> = shared
        .chats
        .lock()
        .unwrap()
        .iter()
        .filter(|c| c.project_id == project_id)
        .map(|c| c.id.clone())
        .collect();
    let now = now_ms();
    let jobs: Vec<Job> = shared
        .jobs
        .lock()
        .unwrap()
        .iter()
        .filter(|f| f.job.chat_id.as_ref().is_some_and(|c| chats.contains(c)))
        .map(|f| Job {
            elapsed_s: now.saturating_sub(f.started_at) / 1000,
            ..f.job.clone()
        })
        .collect();
    let _ = shared.events.send(SyncEvent::ProjectJobs {
        project_id: project_id.to_string(),
        jobs,
    });
}

/// A job ends (Stop in the drawer, or the Stop button for every job of the
/// chat): its hidden row closes as agentd closes it and the list forgets it.
/// The scripted exchange of each fixture job, as offsets (s) from its start: the brief,
/// the ack, tool steps with their lengths, the result. `RolloutRefresh` answers the
/// entries that have "happened" by now, the step under way as `running`, so a rollout
/// opened on the rig streams. `ARBOS_FIXTURE_ROLLOUT_DONE=1` ends the first job's document
/// (result delivered, `ended_at`) while the chip still lists it — the finished-state shot.
/// One scripted step: (offset_s, kind, words, duration_ms).
type ScriptStep = (u64, &'static str, &'static str, u64);

fn fixture_rollout_script(title: &str) -> (&'static str, Vec<ScriptStep>, &'static str) {
    // (brief, [(offset_s, kind, words, duration_ms)], result)
    match title {
        "Searching for surf schools" => (
            "Find the 3 best-reviewed surf schools near Santa Teresa, compare group and private lesson prices, and say which is the best value. Return each school with its page URL and the facts you compared.",
            vec![
                (3, "text", "On it — pulling ratings and current prices for the top schools around Santa Teresa.", 0),
                (6, "tool", "Searched the web for “surf schools Santa Teresa reviews 2026”", 2100),
                (9, "tool", "Read tripadvisor.com — Santa Teresa surf schools", 4800),
                (15, "tool", "Read bluemysticsurf.com/prices", 3000),
                (19, "tool", "Reading delsoulsurf.com", 2600),
                (23, "tool", "Read monkeyadventures.cr", 3400),
                (28, "text", "Three schools have enough reviews to compare; checking the lesson pages for this month's prices.", 0),
                (33, "tool", "Read bluemysticsurf.com/lessons", 2200),
                (40, "tool", "Comparing prices across the three", 1800),
            ],
            "Blue Mystic is the best value for single lessons ($60–70 group, $90 private, 4.9 from 444 Google reviews); Del Soul is the pick if you take above five (5.0 from 593, $65–75 group); Monkey Adventures runs $75 group / $115 private. Full table with page URLs attached.",
        ),
        "Fixing the flaky tests" => (
            "The relay's websocket tests fail one run in five on CI. Find the race, fix it on a branch, and run the suite three times.",
            vec![
                (4, "text", "Reading the failing runs first.", 0),
                (8, "tool", "Read .github/workflows/ci.yml", 1200),
                (12, "tool", "Ran cargo test -p arbos-relay ws:: (3×)", 48_000),
                (70, "tool", "Read crates/arbos-relay/src/ws.rs", 2100),
                (80, "text", "The ping timer races the close frame; adding the join before the shutdown.", 0),
                (90, "tool", "Edited crates/arbos-relay/src/ws.rs", 900),
                (100, "tool", "Running cargo test -p arbos-relay (1 of 3)", 45_000),
            ],
            "Fixed: the ping timer raced the close frame. Three green runs on the branch fix/ws-close-race.",
        ),
        _ => (
            "Rename the config module to settings across the crate and keep the public re-exports.",
            vec![],
            "",
        ),
    }
}

fn emit_job_rollout(shared: &Arc<Shared>, project_id: &str, job_id: &str, after: Option<u64>) {
    let Some(f) = shared
        .jobs
        .lock()
        .unwrap()
        .iter()
        .find(|f| f.job.id == job_id)
        .cloned()
    else {
        return;
    };
    let parent = shared
        .projects
        .lock()
        .unwrap()
        .iter()
        .find(|p| p.id == project_id)
        .map(|p| p.name.clone())
        .unwrap_or_default();
    let now = now_ms();
    let done = std::env::var("ARBOS_FIXTURE_ROLLOUT_DONE").is_ok_and(|v| v == "1")
        && f.job.title == "Searching for surf schools";
    let (brief, script, result) = fixture_rollout_script(&f.job.title);
    let start = f.started_at;
    let mut entries = vec![RolloutEntry {
        seq: 1,
        at: start,
        kind: RolloutKind::Prompt,
        text: Some(brief.to_string()),
        label: None,
        name: None,
        status: None,
        duration_ms: None,
    }];
    let mut seq = 2;
    let mut tool_calls = 0;
    let mut running_step: Option<String> = None;
    let horizon = if done { u64::MAX } else { now };
    for (offset, kind, words, dur) in &script {
        let at = start + offset * 1000;
        if at > horizon {
            break;
        }
        let is_tool = *kind == "tool";
        let finished = done || at + dur <= now;
        if is_tool {
            tool_calls += 1;
            if !finished {
                running_step = Some(words.to_string());
            }
        }
        entries.push(RolloutEntry {
            seq,
            at,
            kind: if is_tool {
                RolloutKind::Tool
            } else {
                RolloutKind::Text
            },
            text: (!is_tool).then(|| words.to_string()),
            label: is_tool.then(|| words.to_string()),
            name: is_tool.then(|| "tool".to_string()),
            status: is_tool.then(|| if finished { "done" } else { "running" }.to_string()),
            duration_ms: (is_tool && finished).then_some(*dur),
        });
        seq += 1;
    }
    let ended_at = done.then(|| start + 372_000);
    if done {
        entries.push(RolloutEntry {
            seq,
            at: start + 372_000,
            kind: RolloutKind::Result,
            text: Some(result.to_string()),
            label: None,
            name: None,
            status: Some("done".to_string()),
            duration_ms: Some(372_000),
        });
        seq += 1;
    }
    if let Some(a) = after {
        entries.retain(|e| e.seq > a || e.is_live());
    }
    let step = running_step.unwrap_or_else(|| f.job.step.clone());
    let rollout = JobRollout {
        id: f.job.id.clone(),
        title: f.job.title.clone(),
        kind: "research".into(),
        state: f.job.state,
        step,
        elapsed_s: if done {
            372
        } else {
            now.saturating_sub(start) / 1000
        },
        started_at: Some(start),
        ended_at,
        chat_id: f.job.chat_id.clone(),
        ask_message_id: f.job.ask_message_id.clone(),
        parent: RolloutParent {
            project_id: project_id.to_string(),
            name: parent,
        },
        brief: brief.to_string(),
        entries,
        tool_calls,
        next_seq: seq,
        unavailable: false,
    };
    let _ = shared.events.send(SyncEvent::JobRollout {
        project_id: project_id.to_string(),
        job_id: job_id.to_string(),
        rollout,
    });
}

fn fixture_stop_jobs(shared: &Arc<Shared>, chat_id: &str, only: Option<&str>) {
    let ended: Vec<FixtureJob> = {
        let mut jobs = shared.jobs.lock().unwrap();
        let (gone, kept): (Vec<FixtureJob>, Vec<FixtureJob>) = jobs.drain(..).partition(|f| {
            f.job.chat_id.as_deref() == Some(chat_id) && only.is_none_or(|id| f.job.id == id)
        });
        *jobs = kept;
        gone
    };
    for f in ended {
        with_doc(shared, chat_id, |doc| {
            doc.core.transact(|d| {
                d.set_activity(&f.row_id, None);
                let _ = d.add_status_part(&f.row_id, "stopped", "Stopped");
                d.finish_message(&f.row_id, CoreStatus::Cancelled);
            })
        });
    }
}

/// `ARBOS_FIXTURE_TYPING=1|<handle>[,<handle>…]` keeps "<name> is typing…"
/// up in every fixture people chat (`1` = the chat's other person, else those
/// handles) for the screenshot rig ([feedback typing-indicator-left-2026-10-23]);
/// the app drops a typing mark after 6 s, so it is re-sent every 3 s.
/// `ARBOS_FIXTURE_TYPING_MS=<ms>` stops it after that long.
fn emit_typing_hook(shared: &Arc<Shared>) {
    let Ok(spec) = std::env::var("ARBOS_FIXTURE_TYPING") else {
        return;
    };
    let spec = spec.trim().to_string();
    if spec.is_empty() || spec == "0" {
        return;
    }
    let chats: Vec<(Id, String)> = shared
        .projects
        .lock()
        .unwrap()
        .iter()
        .filter(|p| p.is_people())
        .flat_map(|p| -> Vec<(Id, String)> {
            if spec == "1" {
                p.person()
                    .map(|person| vec![(p.id.clone(), person.handle.clone())])
                    .unwrap_or_default()
            } else {
                spec.split(',')
                    .map(str::trim)
                    .filter(|h| !h.is_empty())
                    .map(|h| (p.id.clone(), h.to_string()))
                    .collect()
            }
        })
        .collect();
    let stop_after = std::env::var("ARBOS_FIXTURE_TYPING_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok());
    let events = shared.events.clone();
    tokio::spawn(async move {
        let started = Instant::now();
        loop {
            let active = stop_after.is_none_or(|ms| started.elapsed() < Duration::from_millis(ms));
            for (chat_id, handle) in &chats {
                let _ = events.send(SyncEvent::Typing {
                    chat_id: chat_id.clone(),
                    handle: handle.clone(),
                    active,
                });
            }
            if !active {
                return;
            }
            sleep(Duration::from_secs(3)).await;
        }
    });
}

/// The relay's cap (Jacob 2026-10-28: five, no tiers).
const FIXTURE_PIN_CAP: usize = 5;

/// `ARBOS_RIG_PINS=<name or id>[,<name or id>…]` pins those sidebar rows at launch, top
/// first (screenshot rigs, [feedback sidebar-pinning-mac-2026-10-28]). A name is matched
/// against projects (agents, DMs, groups); a `server:<id>` key is passed through.
fn emit_pins_hook(shared: &Arc<Shared>) {
    let Ok(spec) = std::env::var("ARBOS_RIG_PINS") else {
        return;
    };
    let projects = shared.projects.lock().unwrap();
    let pins: Vec<String> = spec
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|name| {
            if name.contains(':') {
                return Some(name.to_owned());
            }
            projects
                .iter()
                .find(|p| p.id == name || p.name.eq_ignore_ascii_case(name))
                .map(|p| {
                    if p.is_people() {
                        format!("chat:{}", p.id)
                    } else {
                        format!("agent:{}", p.id)
                    }
                })
        })
        .take(FIXTURE_PIN_CAP)
        .collect();
    drop(projects);
    *shared.pins.lock().unwrap() = pins.clone();
    let _ = shared.events.send(SyncEvent::Pins {
        pins,
        cap: FIXTURE_PIN_CAP,
    });
}

/// `ARBOS_FIXTURE_OPEN=<project name or id>[/<chat id>]` opens that chat at
/// launch (screenshot rigs; the iPhone has `ARBOS_OPEN_PROJECT` / `_CHAT`).
fn emit_open_request(shared: &Arc<Shared>) {
    let Ok(spec) = std::env::var("ARBOS_FIXTURE_OPEN") else {
        return;
    };
    let (project, chat) = match spec.split_once('/') {
        Some((p, c)) => (p.trim(), Some(c.trim())),
        None => (spec.trim(), None),
    };
    let project_id = shared
        .projects
        .lock()
        .unwrap()
        .iter()
        .find(|p| p.id == project || p.name.eq_ignore_ascii_case(project))
        .map(|p| p.id.clone());
    let Some(project_id) = project_id else {
        return;
    };
    let chat_id = {
        let chats = shared.chats.lock().unwrap();
        chat.map(str::to_owned)
            .or_else(|| {
                chats
                    .iter()
                    .find(|c| c.project_id == project_id && c.is_pinned)
                    .or_else(|| chats.iter().find(|c| c.project_id == project_id))
                    .map(|c| c.id.clone())
            })
            .unwrap_or_default()
    };
    let _ = shared.events.send(SyncEvent::RestoreSelection {
        project_id,
        chat_id,
    });
}

fn forward_doc_change(shared: &Arc<Shared>, changed: DocChanged) {
    let docs = shared.docs.lock().unwrap();
    let Some(doc) = docs.get(&changed.chat_id) else {
        return;
    };
    if changed.touched.list_changed {
        let _ = shared.events.send(SyncEvent::ChatSnapshot {
            chat_id: changed.chat_id.clone(),
            messages: doc.read_all(),
        });
        return;
    }
    for ix in changed.touched.messages {
        if let Some(message) = doc.read_message(ix) {
            let _ = shared.events.send(SyncEvent::MessageUpsert {
                chat_id: changed.chat_id.clone(),
                message: Box::new(message),
            });
        }
    }
}

fn handle_command(shared: &Arc<Shared>, cmd: ClientCommand) {
    if handle_people_command(shared, &cmd) {
        return;
    }
    match cmd {
        // Handled above; listed so a new variant still fails to compile here.
        ClientCommand::SearchPeople { .. }
        | ClientCommand::StartDm { .. }
        | ClientCommand::CreatePeopleGroup { .. }
        | ClientCommand::SetTyping { .. }
        | ClientCommand::RemoveAgent { .. }
        | ClientCommand::AddMembers { .. }
        | ClientCommand::RemoveMember { .. }
        | ClientCommand::SetMemberRole { .. }
        | ClientCommand::LeaveChat { .. }
        | ClientCommand::BlockUser { .. }
        | ClientCommand::UnblockUser { .. }
        | ClientCommand::InviteToProject { .. }
        | ClientCommand::RemoveProjectMember { .. }
        | ClientCommand::SetMessageGrant { .. }
        | ClientCommand::RemoveMessageGrant { .. }
        | ClientCommand::FetchAvatar { .. }
        | ClientCommand::DeleteProject { .. }
        | ClientCommand::ConnectAgent { .. }
        | ClientCommand::OutsideAgentStatus { .. }
        | ClientCommand::OutsideAgentRotate { .. }
        | ClientCommand::OutsideAgentRevoke { .. }
        | ClientCommand::OutsideAgentScope { .. }
        | ClientCommand::OutsideAgentWakeSet { .. }
        | ClientCommand::OutsideAgentWakeTest { .. }
        | ClientCommand::DeletePeopleChat { .. }
        | ClientCommand::RenameGroup { .. }
        | ClientCommand::SetGroupImage { .. }
        | ClientCommand::SetGroupLeaf { .. }
        | ClientCommand::SetProjectImage { .. }
        | ClientCommand::SetGroupImageKey { .. }
        | ClientCommand::SetServerImage { .. }
        | ClientCommand::SetServerLeaf { .. }
        | ClientCommand::GenerateImage { .. }
        | ClientCommand::CloseChat { .. } => {}
        // D88 servers live on the relay only; the fixture has none.
        ClientCommand::ServersRefresh
        | ClientCommand::ServerCreate { .. }
        | ClientCommand::ServerJoin { .. }
        | ClientCommand::ServerFetch { .. }
        | ClientCommand::ServerRename { .. }
        | ClientCommand::ServerDelete { .. }
        | ClientCommand::ServerLeave { .. }
        | ClientCommand::ServerSetRole { .. }
        | ClientCommand::ServerRemoveMember { .. }
        | ClientCommand::ChannelCreate { .. }
        | ClientCommand::ChannelPatch { .. }
        | ClientCommand::ChannelDelete { .. }
        | ClientCommand::ServerAddMembers { .. }
        | ClientCommand::ServerInviteCreate { .. }
        | ClientCommand::ServerInviteRevoke { .. }
        | ClientCommand::ChannelArchive { .. }
        | ClientCommand::ChannelOpen { .. }
        | ClientCommand::ChannelLoadOlder { .. }
        | ClientCommand::ChannelSend { .. }
        | ClientCommand::ChannelMarkRead { .. } => {}
        // The relay keeps the pin list and answers with it; the fixture echoes
        // the list it was given, capped like the relay.
        ClientCommand::SetPins { pins } => {
            let mut pins = pins;
            pins.truncate(FIXTURE_PIN_CAP);
            *shared.pins.lock().unwrap() = pins.clone();
            let _ = shared.events.send(SyncEvent::Pins {
                pins,
                cap: FIXTURE_PIN_CAP,
            });
        }
        // The relay keeps archive state; the fixture does the same so a chat
        // brought back from the "+" dropdown stays back when the list is re-sent.
        ClientCommand::ArchiveChat { chat_id, archived } => {
            if let Some(c) = shared
                .chats
                .lock()
                .unwrap()
                .iter_mut()
                .find(|c| c.id == chat_id)
            {
                c.archived_at = archived.then(now_ms);
            }
        }
        ClientCommand::OpenChat { chat_id } => {
            *shared.open_chat.lock().unwrap() = Some(chat_id.clone());
            mark_read(shared, &chat_id);
        }
        // Calls v1 against a local `livekit-server --dev` (devkey/secret):
        // the fixture mints the join token itself.
        ClientCommand::StartCall { chat_id, kind } => {
            let call = fixture_call(shared, &chat_id, kind);
            let _ = shared.events.send(SyncEvent::CallReady {
                call: call.clone(),
                created: true,
                reachable: Vec::new(),
            });
            // Sam picks up after a moment so the tile grid has someone in it.
            let shared = shared.clone();
            tokio::spawn(async move {
                sleep(Duration::from_millis(2500)).await;
                let mut joined = call.clone();
                joined.token = None;
                joined.participants.push(sender_of(&sam()));
                let _ = shared.events.send(SyncEvent::CallInfo(joined));
            });
        }
        ClientCommand::JoinCall { call_id } => {
            let chat_id = call_id
                .strip_prefix("call_fixture_")
                .unwrap_or(DM_SAM)
                .to_string();
            let _ = shared.events.send(SyncEvent::CallReady {
                call: fixture_call(shared, &chat_id, CallKind::Video),
                created: false,
                reachable: Vec::new(),
            });
        }
        ClientCommand::LeaveCall { call_id } => {
            let chat_id = call_id
                .strip_prefix("call_fixture_")
                .unwrap_or(DM_SAM)
                .to_string();
            let _ = shared.events.send(SyncEvent::CallEnded(CallEnded {
                call_id,
                chat_id,
                project_id: "people".into(),
                duration_s: 72,
                reason: "last_left".into(),
                writer_device: Some("fixture-device".into()),
                writer_is_me: true,
            }));
        }
        ClientCommand::FetchCall { call_id } => {
            let chat_id = call_id
                .strip_prefix("call_fixture_")
                .unwrap_or(DM_SAM)
                .to_string();
            let mut call = fixture_call(shared, &chat_id, CallKind::Voice);
            call.token = None;
            let _ = shared.events.send(SyncEvent::CallInfo(call));
        }
        ClientCommand::SystemLine {
            chat_id,
            text,
            call_id,
        } => {
            with_doc(shared, &chat_id, |doc| {
                doc.core.transact(|d| {
                    let _ = d.append_system_line(&new_id(), &text, DEVICE, call_id.as_deref());
                });
            });
        }
        ClientCommand::Greeting { chat_id, .. } => {
            let _ = shared.events.send(SyncEvent::GreetingWritten(chat_id));
        }
        ClientCommand::PollNow => {}
        ClientCommand::Send {
            chat_id,
            message_id,
            text,
            attachments,
            reply_to,
            reply_ref,
            mentions,
        } => {
            let attachments: Vec<Attachment> = attachments
                .iter()
                .map(|path| Attachment {
                    name: path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("attachment")
                        .to_string(),
                    mime: "application/octet-stream".into(),
                    blob_ref: None,
                    bytes: None,
                    kind: None,
                    title: None,
                    chart_part: None,
                    url: None,
                    source: None,
                })
                .collect();
            send_user(
                shared,
                chat_id,
                message_id,
                text,
                attachments,
                reply_to,
                None,
                reply_ref,
                mentions,
            );
        }
        ClientCommand::SendMedia {
            chat_id,
            message_id,
            pick,
            reply_to,
        } => {
            // The fixture has no relay: a sticker keeps its bytes under a `local:` key the
            // optimistic echo already decoded; a GIF keeps its provider URL (fixture_png).
            let attachment = match pick {
                MediaPick::Sticker { pack, item, .. } => Attachment {
                    name: format!("{}.png", item.id),
                    source: Some(Box::new(AttachmentSource {
                        provider: pack.name.clone(),
                        id: format!("{}/{}", pack.id, item.id),
                        page_url: pack.page_url.clone(),
                    })),
                    mime: item.mime,
                    blob_ref: Some(format!("local:{message_id}")),
                    bytes: None,
                    kind: Some(AttachmentKind::Sticker),
                    title: None,
                    chart_part: None,
                    url: None,
                },
                MediaPick::Gif(gif) => {
                    let gif = *gif;
                    Attachment {
                        name: format!("{}.gif", gif.id),
                        mime: "image/gif".into(),
                        blob_ref: None,
                        bytes: None,
                        kind: Some(AttachmentKind::Image),
                        title: None,
                        chart_part: None,
                        url: Some(gif.url),
                        source: Some(Box::new(AttachmentSource {
                            provider: "KLIPY".into(),
                            id: gif.id,
                            page_url: gif.page_url,
                        })),
                    }
                }
            };
            send_user(
                shared,
                chat_id,
                message_id,
                String::new(),
                vec![attachment],
                reply_to,
                None,
                None,
                Vec::new(),
            );
        }
        ClientCommand::GifSearch { query, pos } => {
            // Rig: a few fixture tiles so the GIFs tab has a grid to screenshot.
            let items = (0..9)
                .map(|i| crate::model::GifItem {
                    id: format!("fixture-gif-{i}"),
                    title: format!(
                        "{} {}",
                        if query.is_empty() {
                            "trending"
                        } else {
                            query.as_str()
                        },
                        i + 1
                    ),
                    width: 220,
                    height: if i % 3 == 1 { 160 } else { 220 },
                    url: format!("https://static.klipy.com/fixture/{i}/full.gif"),
                    preview: crate::model::GifPreview {
                        url: format!("https://static.klipy.com/fixture/{i}/tiny.gif"),
                        width: 220,
                        height: if i % 3 == 1 { 160 } else { 220 },
                    },
                    page_url: "https://klipy.com".into(),
                })
                .collect();
            let _ = shared.events.send(SyncEvent::GifResults {
                query,
                pos,
                page: crate::model::GifPage::Items { items, next: None },
            });
        }
        ClientCommand::StickerPacks => {
            let _ = shared.events.send(SyncEvent::StickerPacks {
                packs: Some(Vec::new()),
            });
        }
        ClientCommand::GifShare { .. } => {}
        ClientCommand::Forward {
            chat_id,
            message_id,
            attachment,
            text,
            ..
        } => {
            // No blob store to copy between: the row lands in the destination
            // chat with the same key, so its picture is already cached.
            send_user(
                shared,
                chat_id,
                message_id,
                text,
                vec![attachment],
                None,
                None,
                None,
                Vec::new(),
            );
        }
        ClientCommand::OpenAttachment {
            key,
            name,
            quick_look,
            ..
        } => {
            let bytes = if key.ends_with(".png") || name.ends_with(".png") {
                fixture_png(&key)
            } else {
                format!("Fixture attachment {name} ({key}).\n").into_bytes()
            };
            let dir = std::env::temp_dir().join("arbos-media");
            let _ = std::fs::create_dir_all(&dir);
            let path = dir.join(name.replace('/', "_"));
            let mut opened_path = None;
            match std::fs::write(&path, &bytes) {
                Ok(()) => {
                    let open_external = quick_look || !cfg!(target_os = "macos");
                    if open_external {
                        crate::ui::attachments::open_with_system(&path, quick_look);
                    }
                    opened_path = Some(path);
                }
                Err(err) => {
                    let _ = shared.events.send(SyncEvent::Failure {
                        error: UserError::from_text(format!("open {name}"), err.to_string()),
                        retry: None,
                    });
                }
            }
            let _ = shared.events.send(SyncEvent::AttachmentOpened {
                key,
                path: opened_path,
            });
        }
        ClientCommand::ForwardMessage {
            chat_id,
            message_id,
            text,
            attachments,
            charts,
            forwarded_from,
            ..
        } => {
            // No blob store to copy between: the row lands with the same keys,
            // so its pictures are already cached. A chart travels as its
            // fallback picture here (the fixture doc writer has no chart parts).
            let mut attachments = attachments;
            attachments.extend(charts.into_iter().filter_map(|c| c.fallback));
            send_user(
                shared,
                chat_id,
                message_id,
                text,
                attachments,
                None,
                Some(forwarded_from),
                None,
                Vec::new(),
            );
        }
        ClientCommand::Steer {
            chat_id,
            message_id,
            text,
            reply_to,
        } => {
            // Fixture has no live pi turn; land the bubble like a normal send.
            send_user(
                shared,
                chat_id,
                message_id,
                text,
                Vec::new(),
                reply_to,
                None,
                None,
                Vec::new(),
            );
        }
        ClientCommand::SendVoice {
            chat_id,
            message_id,
            text,
            wav,
        } => {
            // The fixture has no blob store; the memo stays in memory under a seed key.
            let key = format!("blob/voice/{}", new_id().to_lowercase());
            shared.voice_blobs.lock().unwrap().insert(key.clone(), wav);
            let attachments = vec![Attachment {
                name: "voice-memo.wav".into(),
                mime: "audio/wav".into(),
                blob_ref: Some(key),
                bytes: None,
                kind: Some(AttachmentKind::Audio),
                title: None,
                chart_part: None,
                url: None,
                source: None,
            }];
            send_user(
                shared,
                chat_id,
                message_id,
                text,
                attachments,
                None,
                None,
                None,
                Vec::new(),
            );
        }
        ClientCommand::Transcribe {
            request_id, wav, ..
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                sleep(Duration::from_millis(600)).await;
                let secs = crate::audio::decode_wav(&wav)
                    .map(|p| p.duration_secs())
                    .unwrap_or(0.0);
                let _ = shared.events.send(SyncEvent::Transcript {
                    request_id,
                    text: format!("{FIXTURE_TRANSCRIPT} ({secs:.1} s clip)"),
                    error: None,
                    mock: true,
                });
            });
        }
        ClientCommand::Speak {
            request_id, text, ..
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                sleep(Duration::from_millis(400)).await;
                let _ = shared.events.send(SyncEvent::Speech {
                    request_id,
                    bytes: Arc::new(crate::audio::tone_speech(&text)),
                    mime: "audio/wav".into(),
                    error: None,
                    mock: true,
                });
            });
        }
        ClientCommand::Answer {
            chat_id,
            message_id,
            text,
            question_id,
            value,
        } => {
            let project_id = project_of(shared, &chat_id);
            with_doc(shared, &chat_id, |doc| {
                doc.core.transact(|d| {
                    let value = value.clone().unwrap_or(Value::Null);
                    match &question_id {
                        Some(qid) => {
                            d.answer_question_part(qid, &value);
                            d.append_user_answer(&message_id, &text, qid, &value, DEVICE);
                        }
                        None => {
                            let _ = d.append_user_message(
                                &message_id,
                                &text,
                                DEVICE,
                                &UserMessageOpts::default(),
                            );
                            d.push_command(
                                "answer",
                                json!({ "messageId": message_id, "text": text, "questionId": null, "value": value }),
                                DEVICE,
                            );
                        }
                    }
                });
            });
            spawn_turn(shared, project_id, chat_id, text, true);
        }
        ClientCommand::Stop { chat_id } => {
            if let Some(flag) = shared.running.lock().unwrap().get(&chat_id) {
                flag.store(true, Ordering::SeqCst);
            }
            with_doc(shared, &chat_id, |doc| {
                doc.core.transact(|d| {
                    d.push_command("stop", json!({}), DEVICE);
                });
            });
            fixture_stop_jobs(shared, &chat_id, None);
        }
        ClientCommand::StopJob {
            chat_id,
            job_id,
            message_id,
        } => {
            with_doc(shared, &chat_id, |doc| {
                doc.core.transact(|d| {
                    d.push_command(
                        "stop",
                        json!({ "jobId": job_id, "messageId": message_id }),
                        DEVICE,
                    );
                });
            });
            fixture_stop_jobs(shared, &chat_id, Some(&job_id));
        }
        ClientCommand::JobsRefresh { project_id } => emit_project_jobs(shared, &project_id),
        ClientCommand::RolloutRefresh {
            project_id,
            job_id,
            after,
        } => emit_job_rollout(shared, &project_id, &job_id, after),
        ClientCommand::React {
            chat_id,
            message_id,
            emoji,
        } => {
            let me = format!("user:{ME_ID}");
            let docs = shared.docs.lock().unwrap();
            if let Some(doc) = docs.get(&chat_id) {
                let base = arbos_core::doc::normalize_reaction_emoji(&emoji);
                let already = doc
                    .index_of(&message_id)
                    .and_then(|ix| doc.read_message(ix))
                    .map(|m| {
                        m.reactions.iter().any(|(e, bucket)| {
                            arbos_core::doc::normalize_reaction_emoji(e) == base
                                && bucket.get(&me).copied().unwrap_or(false)
                        })
                    })
                    .unwrap_or(false);
                let on = !already;
                let _ = doc
                    .core
                    .transact(|d| d.set_reaction(&message_id, &emoji, &me, on));
            }
        }
        ClientCommand::DeleteMessage {
            chat_id,
            message_id,
        } => {
            let docs = shared.docs.lock().unwrap();
            if let Some(doc) = docs.get(&chat_id) {
                let _ = doc.core.transact(|d| d.delete_message(&message_id));
            }
        }
        ClientCommand::EditMessage {
            chat_id,
            message_id,
            text,
        } => {
            let docs = shared.docs.lock().unwrap();
            if let Some(doc) = docs.get(&chat_id) {
                let _ = doc
                    .core
                    .transact(|d| d.edit_message_text(&message_id, &text));
            }
        }
        ClientCommand::RemoveLinkPreview {
            chat_id,
            message_id,
        } => {
            let docs = shared.docs.lock().unwrap();
            if let Some(doc) = docs.get(&chat_id) {
                doc.core.transact(|d| {
                    d.set_message_fields(&message_id, &[("preview", serde_json::json!("off"))]);
                    d.remove_parts_of_kind(&message_id, "link_preview");
                });
            }
        }
        ClientCommand::Approve {
            chat_id,
            approval_id,
            grant,
        } => {
            if let Some(class) = grant {
                with_doc(shared, &chat_id, |doc| {
                    doc.core.transact(|d| {
                        d.set_approval_grant(&approval_id, &class);
                    });
                });
            }
            decide_approval(shared, chat_id, approval_id, ApprovalState::Approved, None)
        }
        ClientCommand::Reject {
            chat_id,
            approval_id,
            reason,
        } => decide_approval(
            shared,
            chat_id,
            approval_id,
            ApprovalState::Rejected,
            reason,
        ),
        ClientCommand::Retry {
            chat_id,
            message_id,
        } => {
            let project_id = project_of(shared, &chat_id);
            let prompt = {
                let docs = shared.docs.lock().unwrap();
                docs.get(&chat_id).and_then(|doc| {
                    let ix = doc.index_of(&message_id)?;
                    let text = (0..ix)
                        .rev()
                        .filter_map(|i| doc.read_message(i))
                        .find(|m| m.role == Role::User)
                        .map(|m| m.plain_text())?;
                    doc.core.transact(|d| {
                        set_status(d, &message_id, MessageStatus::Cancelled);
                        d.push_command("retry", json!({ "messageId": message_id }), DEVICE);
                    });
                    Some(text)
                })
            };
            if let Some(text) = prompt {
                spawn_turn(shared, project_id, chat_id, text, true);
            }
        }
        ClientCommand::Retitle { chat_id, title, .. } => {
            let changed = {
                let mut chats = shared.chats.lock().unwrap();
                match chats.iter_mut().find(|c| c.id == chat_id) {
                    Some(c) => {
                        c.title = title;
                        true
                    }
                    None => false,
                }
            };
            if changed {
                emit_chats(shared);
            }
        }
        ClientCommand::NewChat {
            project_id,
            chat_id,
        } => {
            add_doc(shared, ChatDoc::new(&chat_id, &project_id));
            shared.chats.lock().unwrap().push(Chat {
                id: chat_id.clone(),
                project_id,
                title: "New chat".into(),
                is_pinned: false,
                is_system: false,
                kind: ChatKind::Chat,
                participants: Vec::new(),
                unread: false,
                unread_count: 0,
                mention_count: 0,
                last_message_at: now_ms(),
                archived_at: None,
            });
            emit_chats(shared);
            let _ = shared.events.send(SyncEvent::ChatSnapshot {
                chat_id,
                messages: Vec::new(),
            });
        }
        ClientCommand::ForkChat {
            project_id,
            source_chat_id,
            chat_id,
            title,
        } => {
            // The box's job in production: the new room starts as a copy of the
            // source transcript under the new chat id.
            let copy = shared
                .docs
                .lock()
                .unwrap()
                .get(&source_chat_id)
                .map(|src| src.core.snapshot());
            let doc = match copy {
                Some(snap) => ChatDoc::forked_from_snapshot(&chat_id, &project_id, &snap),
                None => ChatDoc::new(&chat_id, &project_id),
            };
            let messages = doc.read_all();
            add_doc(shared, doc);
            shared.chats.lock().unwrap().push(Chat {
                id: chat_id.clone(),
                project_id,
                title,
                is_pinned: false,
                is_system: false,
                kind: ChatKind::Chat,
                participants: Vec::new(),
                unread: false,
                unread_count: 0,
                mention_count: 0,
                last_message_at: now_ms(),
                archived_at: None,
            });
            emit_chats(shared);
            let _ = shared
                .events
                .send(SyncEvent::ChatSnapshot { chat_id, messages });
        }
        ClientCommand::MarkRead { chat_id } => mark_read(shared, &chat_id),
        ClientCommand::MarkMentionsRead { chat_id, .. } => mark_mentions_read(shared, &chat_id),
        ClientCommand::FetchVoices => {
            let _ = shared.events.send(SyncEvent::Voices {
                default: Some("ara".into()),
                voices: fixture_voices(),
            });
        }
        ClientCommand::FetchVoiceSample { url } => {
            // The demo has no samples to fetch; a short synthesised tone stands in.
            let dir = crate::sync::credentials::voice_sample_cache_dir();
            let _ = std::fs::create_dir_all(&dir);
            let path = dir.join("fixture-sample.wav");
            if !path.is_file() {
                let _ = std::fs::write(&path, fixture_sample_wav());
            }
            let _ = shared.events.send(SyncEvent::VoiceSample { url, path });
        }
        ClientCommand::SetVoice { project_id, voice } => {
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == project_id) {
                    p.voice = voice;
                }
            }
            emit_projects(shared);
        }
        ClientCommand::SetVision {
            project_id,
            enabled,
        } => {
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == project_id) {
                    p.vision_enabled = enabled;
                }
            }
            emit_projects(shared);
        }
        ClientCommand::SetModel {
            project_id,
            model,
            effort,
        } => {
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == project_id) {
                    p.model = model;
                    p.effort = effort;
                }
            }
            emit_projects(shared);
        }
        ClientCommand::MachineStart => {
            let shared = shared.clone();
            tokio::spawn(async move {
                wake_machine(&shared).await;
            });
        }
        ClientCommand::MachineStop => {
            set_machine(shared, MachineState::Stopped, Some("Stopped by you"), None);
        }
        ClientCommand::FocusChat { .. } => {}
        ClientCommand::SetAvatarLeaf {
            project_id,
            leaf,
            color,
        } => {
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == project_id) {
                    p.avatar_leaf = leaf;
                    p.avatar_leaf_color = color;
                    p.image_url = None;
                }
            }
            emit_projects(shared);
        }
        ClientCommand::SetMyAvatar { path } => {
            // The chosen file itself (mastered by `emit_avatar`), so a rig shot shows the real
            // picture on the hero and the rows after Set.
            let url = if path.is_file() {
                crate::ui::avatar_picker::file_url(&path)
            } else {
                "fixture://avatar/me-picture".to_string()
            };
            emit_avatar(shared, &url);
            let _ = shared
                .events
                .send(SyncEvent::Account(Some(crate::model::AccountView {
                    id: ME_ID.into(),
                    login: ME_HANDLE.into(),
                    name: "Jacob Steeves".into(),
                    avatar_url: url,
                    device_name: "Fixture".into(),
                    balance_usd: 84.5,
                    spent_today_usd: 1.6,
                    daily_cap_usd: Some(20.0),
                })));
        }
        ClientCommand::SetMyAvatarKey { key } => {
            // The Generate tab's preview (seeded by `GenerateImage`) becomes my picture.
            let url = format!("fixture://avatar/me-{key}");
            emit_avatar(shared, &url);
            let _ = shared
                .events
                .send(SyncEvent::Account(Some(crate::model::AccountView {
                    id: ME_ID.into(),
                    login: ME_HANDLE.into(),
                    name: "Jacob Steeves".into(),
                    avatar_url: url,
                    device_name: "Fixture".into(),
                    balance_usd: 84.5,
                    spent_today_usd: 1.6,
                    daily_cap_usd: Some(20.0),
                })));
        }
        ClientCommand::RemoveMyAvatar => {
            let _ = shared
                .events
                .send(SyncEvent::Account(Some(crate::model::AccountView {
                    id: ME_ID.into(),
                    login: ME_HANDLE.into(),
                    name: "Jacob Steeves".into(),
                    avatar_url: String::new(),
                    device_name: "Fixture".into(),
                    balance_usd: 84.5,
                    spent_today_usd: 1.6,
                    daily_cap_usd: Some(20.0),
                })));
        }
        ClientCommand::UpdateProfile { name, handle } => {
            // The fixture keeps it: Settings shows the new name and handle.
            let _ = shared
                .events
                .send(SyncEvent::Account(Some(crate::model::AccountView {
                    id: ME_ID.into(),
                    login: handle,
                    name,
                    avatar_url: String::new(),
                    device_name: "Fixture".into(),
                    balance_usd: 84.5,
                    spent_today_usd: 1.6,
                    daily_cap_usd: Some(20.0),
                })));
        }
        ClientCommand::SetDescription {
            project_id,
            description,
        } => {
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == project_id) {
                    p.description = Some(description).filter(|d| !d.trim().is_empty());
                }
            }
            emit_projects(shared);
        }
        ClientCommand::SetProjectName { project_id, name } => {
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == project_id) {
                    p.name = name.clone();
                }
                // Keep people-group @mention handles in sync with the new name.
                let handle = Project::mention_slug(&name);
                for space in projects.iter_mut().filter(|p| p.is_people()) {
                    if let Some(a) = space.agents.iter_mut().find(|a| a.project_id == project_id) {
                        a.name = name.clone();
                        a.handle = handle.clone();
                    }
                }
            }
            emit_projects(shared);
        }
        ClientCommand::ArchiveSpace {
            space_id,
            server: _,
            archived,
        } => {
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == space_id) {
                    p.archived_at = archived.then(now_ms);
                }
            }
            emit_projects(shared);
        }
        ClientCommand::SetMuted { space_id, muted } => {
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == space_id) {
                    p.muted = muted;
                }
            }
            emit_projects(shared);
        }
        ClientCommand::SetReadReceipts { .. } => {}
        ClientCommand::MachineRestart => {
            set_machine(shared, MachineState::Stopped, Some("Restarting…"), None);
            let shared = shared.clone();
            tokio::spawn(async move {
                wake_machine(&shared).await;
            });
        }
        ClientCommand::Pair { .. } | ClientCommand::SignIn { .. } => {
            let _ = shared.events.send(SyncEvent::Paired {
                device_id: DEVICE.into(),
            });
        }
        ClientCommand::SignOut => {
            let _ = shared.events.send(SyncEvent::Account(None));
            let _ = shared.events.send(SyncEvent::NeedsPairing {
                relay_url: "fixture".into(),
            });
        }
        ClientCommand::CreateProject {
            name,
            avatar_color_index,
            leaf,
            picture,
            source_project_id: _,
            description,
            color,
        } => {
            let id = new_id();
            // No blob store in the demo: the Create sheet's picture becomes a seeded one.
            let image_url = picture.as_ref().map(|p| draft_picture_url("agent", p));
            let icon = name
                .chars()
                .next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_else(|| "Ω".into());
            let accent = color.unwrap_or_else(|| "#8b7cf6".into());
            shared.projects.lock().unwrap().push(Project {
                id: id.clone(),
                name: name.clone(),
                icon,
                color: accent,
                state: BoxState::Awake,
                state_detail: None,
                wake_progress: None,
                model: MODELS[0].into(),
                effort: Effort::High,
                spend_today_usd: 0.0,
                spend_cap_usd: 25.0,
                kind: SpaceKind::Project,
                people: Vec::new(),
                agents: Vec::new(),
                pending: false,
                shared: false,
                owner_handle: None,
                avatar: None,
                avatar_color: None,
                description,
                owner_person: None,
                machine_info: None,
                outside: None,
                muted: false,
                silenced: false,
                silenced_until: None,
                archived_at: None,
                primary_chat: None,
                voice: None,
                vision_enabled: true,
                avatar_color_index: avatar_color_index
                    .map(|i| crate::ui::avatars::index_for(&id, Some(i))),
                avatar_color_override: avatar_color_index,
                avatar_leaf: leaf.map(|l| l.leaf),
                avatar_leaf_color: leaf.map(|l| l.color),
                image_url: image_url.clone(),
                avatar_version: 0,
                message_grants: Vec::new(),
                blocked_senders: Vec::new(),
                my_role: None,
            });
            if let Some(url) = &image_url {
                emit_avatar(shared, url);
            }
            let chat_id = new_id();
            shared.chats.lock().unwrap().push(Chat {
                id: chat_id.clone(),
                project_id: id.clone(),
                title: name,
                is_pinned: true,
                is_system: false,
                kind: ChatKind::Chat,
                participants: Vec::new(),
                unread: false,
                unread_count: 0,
                mention_count: 0,
                last_message_at: now_ms(),
                archived_at: None,
            });
            add_doc(shared, ChatDoc::new(&chat_id, &id));
            emit_projects(shared);
            emit_chats(shared);
            let _ = shared.events.send(SyncEvent::ChatSnapshot {
                chat_id,
                messages: Vec::new(),
            });
        }
        ClientCommand::FetchBlob {
            project_id,
            key,
            save,
        } => {
            let memo = if key == VOICE_SEED_BLOB {
                Some(Arc::new(crate::audio::TEST_CLIP_WAV.to_vec()))
            } else {
                shared.voice_blobs.lock().unwrap().get(&key).cloned()
            };
            if let Some(bytes) = memo {
                let _ = shared.events.send(SyncEvent::Blob {
                    project_id,
                    key,
                    mime: "audio/wav".into(),
                    bytes,
                    saved_to: None,
                });
                return;
            }
            let body = format!(
                "# {key}\n\nFixture blob for project {project_id}.\n\nThe relay source downloads the real bytes from GET /blob/{{project}}/{{key}}.\n"
            );
            let saved_to = if save {
                let path = super::credentials::downloads_dir().join(key.replace('/', "_"));
                std::fs::write(&path, &body).ok().map(|_| path)
            } else {
                None
            };
            let _ = shared.events.send(SyncEvent::Blob {
                project_id,
                key,
                mime: "text/markdown".into(),
                bytes: Arc::new(body.into_bytes()),
                saved_to,
            });
        }
        ClientCommand::FetchThumbnail { key, .. } => {
            let _ = shared.events.send(SyncEvent::Thumbnail {
                bytes: Arc::new(fixture_png(&key)),
                key,
            });
        }
        ClientCommand::FetchCopyBlobs { request, keys, .. } => {
            // The fixture's pictures are PNGs; a key that reads as a file gets a small text body.
            let blobs = keys
                .into_iter()
                .map(|key| {
                    let is_picture = key.ends_with(".png")
                        || key.ends_with(".jpg")
                        || key.ends_with(".jpeg")
                        || key.contains("Screenshot")
                        || key.starts_with("fixture-g")
                        || key.starts_with("img-");
                    let (mime, bytes) = if is_picture {
                        ("image/png".to_owned(), fixture_png(&key))
                    } else {
                        (
                            "text/plain".to_owned(),
                            format!("Fixture attachment {key}.\n").into_bytes(),
                        )
                    };
                    Some(super::CopyBlob {
                        key,
                        mime,
                        bytes: Arc::new(bytes),
                    })
                })
                .collect();
            let _ = shared.events.send(SyncEvent::CopyBlobs { request, blobs });
        }
        ClientCommand::FetchLinkIcon { host } => {
            // The real icons of the fixture replies' sites (`assets/link-icons`), so the
            // shots show the feature; any other host has none → the placeholder.
            let png = fixture_link_icon(&host).map(|b| Arc::new(b.to_vec()));
            let _ = shared.events.send(SyncEvent::LinkIcon { host, png });
        }
        ClientCommand::DownloadAttachment {
            project_id,
            key,
            name,
        } => {
            let bytes = if key.ends_with(".png") || name.ends_with(".png") {
                fixture_png(&key)
            } else {
                format!(
                    "Fixture attachment {name} ({key}) for project {project_id}.\nThe relay source downloads the real bytes from GET /blob/{{project}}/{{key}}.\n"
                )
                .into_bytes()
            };
            let path = super::relay::unique_download_path(&name.replace('/', "_"));
            match std::fs::write(&path, &bytes) {
                Ok(()) => {
                    let _ = shared.events.send(SyncEvent::AttachmentSaved { path });
                }
                Err(err) => {
                    let _ = shared.events.send(SyncEvent::Failure {
                        error: UserError::from_text(format!("save {name}"), err.to_string()),
                        retry: None,
                    });
                }
            }
        }
        ClientCommand::RefreshConnectors => emit_connectors(shared),
        ClientCommand::ConnectorConnect { provider, token } => {
            // Mock relay: the "OAuth dance" completes by itself after a moment.
            let shared = shared.clone();
            tokio::spawn(async move {
                sleep(Duration::from_millis(700)).await;
                let label = {
                    let mut view = shared.connectors.lock().unwrap();
                    let label = view
                        .providers
                        .iter()
                        .find(|p| p.provider == provider)
                        .map(|p| p.label.clone())
                        .unwrap_or_else(|| provider.clone());
                    let label = format!(
                        "{label} · {}",
                        if token.is_some() { "token" } else { "mock" }
                    );
                    view.items.retain(|c| c.provider != provider);
                    view.items.push(Connector {
                        id: new_id(),
                        provider: provider.clone(),
                        label: label.clone(),
                        scopes: Vec::new(),
                        status: ConnectorStatus::Connected,
                        last_error: None,
                        mock: true,
                    });
                    label
                };
                emit_connectors(&shared);
                let _ = shared
                    .events
                    .send(SyncEvent::Notice(format!("{label} connected (fixture).")));
            });
        }
        ClientCommand::ConnectorDisconnect { id } => {
            shared
                .connectors
                .lock()
                .unwrap()
                .items
                .retain(|c| c.id != id);
            emit_connectors(shared);
        }
        ClientCommand::SecretsRefresh => emit_secrets(shared),
        ClientCommand::ShellGrantsRefresh { project_id } => emit_shell_grants(shared, &project_id),
        ClientCommand::ShellGrantRevoke {
            project_id,
            grant_id,
        } => {
            if let Some(list) = shared.shell_grants.lock().unwrap().get_mut(&project_id) {
                list.retain(|g| g.id != grant_id);
            }
            emit_shell_grants(shared, &project_id);
        }
        ClientCommand::DevicesRefresh => emit_devices(shared),
        ClientCommand::DeviceRevoke { id } => {
            // The relay refuses to revoke the caller's own device through this
            // path (Sign out does that); the fixture mirrors it.
            crate::ui::devices::remove_other(&mut shared.devices.lock().unwrap(), &id);
            emit_devices(shared);
        }
        ClientCommand::ProjectSecretsRefresh { project_id } => {
            let view = shared.secrets.lock().unwrap().clone();
            let items = view
                .secrets
                .iter()
                .filter_map(|s| {
                    let a = s.agents.iter().find(|a| a.project_id == project_id)?;
                    Some(crate::model::ProjectSecretAccess {
                        access_id: a.access_id.clone(),
                        enabled: a.enabled,
                        note: a.note.clone(),
                        hosts: a.hosts.clone(),
                        last_used_at: a.last_used_at,
                        disabled_at: a.disabled_at,
                        secret: Some(Secret {
                            agents: Vec::new(),
                            ..s.clone()
                        }),
                        vault: None,
                    })
                })
                .collect::<Vec<_>>();
            let mut items = items;
            for v in &view.vaults {
                if let Some(a) = v.agents.iter().find(|a| a.project_id == project_id) {
                    items.push(crate::model::ProjectSecretAccess {
                        access_id: a.access_id.clone(),
                        enabled: a.enabled,
                        note: a.note.clone(),
                        hosts: a.hosts.clone(),
                        last_used_at: a.last_used_at,
                        disabled_at: a.disabled_at,
                        secret: None,
                        vault: Some(crate::model::ConnectedVault {
                            agents: Vec::new(),
                            ..v.clone()
                        }),
                    });
                }
            }
            let requests = view
                .requests
                .iter()
                .filter(|r| r.project_id == project_id)
                .cloned()
                .collect();
            let activity = view
                .activity
                .iter()
                .filter(|a| a.project_id.as_deref() == Some(project_id.as_str()))
                .cloned()
                .collect();
            let _ = shared.events.send(SyncEvent::ProjectSecrets {
                project_id,
                view: crate::model::ProjectSecrets {
                    paused: view.paused,
                    owner: true,
                    items,
                    requests,
                    activity,
                },
            });
        }
        ClientCommand::SecretHandoffSave {
            chat_id,
            approval_id,
            project_id,
            name,
            purpose,
            ..
        } => {
            let env = crate::model::secret_env_name(&name);
            {
                let mut view = shared.secrets.lock().unwrap();
                view.secrets.retain(|s| s.env != env);
                let project_name = shared
                    .projects
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|p| p.id == project_id)
                    .map(|p| p.name.clone());
                view.secrets.push(Secret {
                    id: new_id(),
                    name: name.clone(),
                    env: env.clone(),
                    kind: "key".into(),
                    note: purpose.clone(),
                    hosts: Vec::new(),
                    ask_every_time: false,
                    inject_env: false,
                    expires_at: None,
                    version: 1,
                    created_at: now_ms() as i64,
                    last_used_at: None,
                    username: None,
                    vault_name: "Arbos".into(),
                    agents: vec![fixture_agent(&project_id, "", true, &purpose)],
                    directors: Vec::new(),
                    others_ask: "refuse".into(),
                    source: Some(crate::model::SecretSource {
                        kind: "chat".into(),
                        project_id: Some(project_id.clone()),
                        project_name,
                        chat_id: Some(chat_id.clone()),
                    }),
                });
                view.secrets.sort_by(|a, b| a.env.cmp(&b.env));
            }
            emit_secrets(shared);
            let _ = shared.events.send(SyncEvent::SecretHandoff {
                approval_id: approval_id.clone(),
                result: Ok(env),
            });
            decide_approval(shared, chat_id, approval_id, ApprovalState::Approved, None)
        }
        ClientCommand::SecretAdd { item } => {
            {
                let mut view = shared.secrets.lock().unwrap();
                let env = crate::model::secret_env_name(&item.name);
                view.secrets.retain(|s| s.env != env);
                let agents = item
                    .agents
                    .iter()
                    .map(|pid| fixture_agent(pid, "", true, &item.note))
                    .collect();
                view.secrets.push(Secret {
                    id: new_id(),
                    name: item.name.clone(),
                    env,
                    kind: item.kind.clone(),
                    note: item.note.clone(),
                    hosts: item.hosts.clone(),
                    ask_every_time: false,
                    inject_env: item.inject_env,
                    expires_at: None,
                    version: 1,
                    created_at: now_ms() as i64,
                    last_used_at: None,
                    username: item
                        .fields
                        .iter()
                        .find(|(k, _)| k == "username")
                        .map(|(_, v)| v.clone()),
                    vault_name: "Arbos".into(),
                    agents,
                    directors: Vec::new(),
                    others_ask: "refuse".into(),
                    source: None,
                });
                view.secrets.sort_by(|a, b| a.env.cmp(&b.env));
            }
            emit_secrets(shared);
            let _ = shared.events.send(SyncEvent::Notice(format!(
                "Secrets: saved {} (fixture; values are not kept).",
                item.name
            )));
        }
        ClientCommand::SecretPatch { id, body } => {
            {
                let mut view = shared.secrets.lock().unwrap();
                if let Some(s) = view.secrets.iter_mut().find(|s| s.id == id) {
                    if let Some(a) = body.get("ask_every_time").and_then(|v| v.as_bool()) {
                        s.ask_every_time = a;
                        if a {
                            s.inject_env = false;
                        }
                    }
                    if let Some(i) = body.get("inject_env").and_then(|v| v.as_bool()) {
                        s.inject_env = i;
                        if i {
                            s.ask_every_time = false;
                        }
                    }
                    if let Some(n) = body.get("note").and_then(|v| v.as_str()) {
                        s.note = n.to_string();
                    }
                }
            }
            emit_secrets(shared);
        }
        ClientCommand::SecretReplaceValue { id, .. } => {
            {
                let mut view = shared.secrets.lock().unwrap();
                if let Some(s) = view.secrets.iter_mut().find(|s| s.id == id) {
                    s.version += 1;
                }
            }
            emit_secrets(shared);
        }
        ClientCommand::SecretDelete { id } => {
            shared
                .secrets
                .lock()
                .unwrap()
                .secrets
                .retain(|s| s.id != id);
            emit_secrets(shared);
        }
        ClientCommand::SecretAccess {
            id,
            project_id,
            enabled,
            note,
        } => {
            {
                let mut view = shared.secrets.lock().unwrap();
                if let Some(s) = view.secrets.iter_mut().find(|s| s.id == id) {
                    match s.agents.iter_mut().find(|a| a.project_id == project_id) {
                        Some(a) => {
                            a.enabled = enabled;
                            a.disabled_at = (!enabled).then(|| now_ms() as i64);
                            if let Some(n) = note {
                                a.note = n;
                            }
                        }
                        None => s.agents.push(fixture_agent(
                            &project_id,
                            "",
                            enabled,
                            note.as_deref().unwrap_or(""),
                        )),
                    }
                }
            }
            emit_secrets(shared);
        }
        ClientCommand::SecretPerson {
            id,
            user_id,
            enabled,
            note,
        } => {
            {
                let mut view = shared.secrets.lock().unwrap();
                if let Some(s) = view.secrets.iter_mut().find(|s| s.id == id) {
                    if let Some(d) = s.directors.iter_mut().find(|d| d.user_id == user_id) {
                        d.enabled = enabled;
                        d.disabled_at = (!enabled).then(|| now_ms() as i64);
                        if let Some(n) = note {
                            d.note = n;
                        }
                    }
                }
            }
            emit_secrets(shared);
        }
        ClientCommand::SecretPeopleAdd { id, handles } => {
            {
                let mut view = shared.secrets.lock().unwrap();
                if let Some(s) = view.secrets.iter_mut().find(|s| s.id == id) {
                    for h in handles {
                        if s.directors.iter().any(|d| d.handle == h) {
                            continue;
                        }
                        if let Some(p) = directory().into_iter().find(|p| p.handle == h) {
                            s.directors.push(fixture_person(&p, true, "", None));
                        }
                    }
                }
            }
            emit_secrets(shared);
        }
        ClientCommand::SecretPersonRemove { id, user_id } => {
            {
                let mut view = shared.secrets.lock().unwrap();
                if let Some(s) = view.secrets.iter_mut().find(|s| s.id == id) {
                    s.directors.retain(|d| d.user_id != user_id);
                }
            }
            emit_secrets(shared);
        }
        ClientCommand::SecretRevokeAll { id } => {
            {
                let mut view = shared.secrets.lock().unwrap();
                if let Some(s) = view.secrets.iter_mut().find(|s| s.id == id) {
                    for a in &mut s.agents {
                        a.enabled = false;
                        a.disabled_at = Some(now_ms() as i64);
                    }
                }
            }
            emit_secrets(shared);
        }
        ClientCommand::SecretsPause { paused } => {
            shared.secrets.lock().unwrap().paused = paused;
            emit_secrets(shared);
        }
        ClientCommand::SecretDecide {
            request_id,
            decision,
        } => {
            {
                let mut view = shared.secrets.lock().unwrap();
                if let Some(r) = view.requests.iter_mut().find(|r| r.id == request_id) {
                    r.status = match decision.as_str() {
                        "deny" => "denied",
                        "once" => "allowed_once",
                        "chat" => "allowed_chat",
                        _ => "allowed_always",
                    }
                    .into();
                    r.decided_at = Some(now_ms() as i64);
                }
            }
            emit_secrets(shared);
        }
        ClientCommand::VaultConnect { .. }
        | ClientCommand::VaultAccess { .. }
        | ClientCommand::VaultItemEnv { .. }
        | ClientCommand::VaultReplaceToken { .. }
        | ClientCommand::VaultDisconnect { .. } => {
            let _ = shared.events.send(SyncEvent::Notice(
                "Connected vaults need a relay (fixture).".into(),
            ));
        }
        ClientCommand::RoutinesRefresh { project_id } => emit_routines(shared, &project_id),
        ClientCommand::RoutineCreate {
            project_id,
            name,
            prompt,
            chat_id,
            trigger,
        } => {
            let next_run_at = match &trigger {
                RoutineTrigger::Cron { .. } => Some(now_ms() as i64 + 3_600_000),
                RoutineTrigger::Once { at } => Some(*at),
                RoutineTrigger::Webhook { .. } | RoutineTrigger::Event { .. } => None,
            };
            let fire_url = match &trigger {
                RoutineTrigger::Webhook { .. } => {
                    Some("http://127.0.0.1:7700/routines/<id>/fire?secret=…".into())
                }
                RoutineTrigger::Cron { .. }
                | RoutineTrigger::Once { .. }
                | RoutineTrigger::Event { .. } => None,
            };
            shared.routines.lock().unwrap().push(Routine {
                id: new_id(),
                project_id: project_id.clone(),
                chat_id,
                name,
                prompt,
                trigger,
                enabled: true,
                created_at: now_ms() as i64,
                last_run_at: None,
                next_run_at,
                fire_url,
            });
            emit_routines(shared, &project_id);
        }
        ClientCommand::RoutineSetEnabled {
            project_id,
            routine_id,
            enabled,
        } => {
            {
                let mut routines = shared.routines.lock().unwrap();
                if let Some(r) = routines.iter_mut().find(|r| r.id == routine_id) {
                    r.enabled = enabled;
                    r.next_run_at = match (&r.trigger, enabled) {
                        (RoutineTrigger::Cron { .. }, true) => Some(now_ms() as i64 + 3_600_000),
                        (RoutineTrigger::Once { at }, true) => Some(*at),
                        (RoutineTrigger::Cron { .. }, false)
                        | (RoutineTrigger::Once { .. }, false)
                        | (RoutineTrigger::Webhook { .. }, _)
                        | (RoutineTrigger::Event { .. }, _) => None,
                    };
                }
            }
            emit_routines(shared, &project_id);
        }
        ClientCommand::RoutineRunNow {
            project_id,
            routine_id,
        } => run_routine_now(shared, &project_id, &routine_id),
        ClientCommand::RoutineRuns { routine_id, .. } => emit_routine_runs(shared, &routine_id),
        ClientCommand::RoutineDelete {
            project_id,
            routine_id,
        } => {
            shared
                .routines
                .lock()
                .unwrap()
                .retain(|r| r.id != routine_id);
            shared.routine_runs.lock().unwrap().remove(&routine_id);
            emit_routines(shared, &project_id);
        }
        ClientCommand::SkillsRefresh => emit_skills(shared),
        ClientCommand::SkillSave { mut skill } => {
            {
                let mut skills = shared.skills.lock().unwrap();
                let now = now_ms() as i64;
                skill.updated_at = now;
                if skill.slug.is_empty() {
                    skill.slug = arbos_core::skill::slugify(&skill.name);
                }
                match skills
                    .iter_mut()
                    .find(|s| s.id == skill.id && !skill.id.is_empty())
                {
                    Some(existing) => {
                        skill.version = existing.version + 1;
                        skill.created_at = existing.created_at;
                        *existing = skill;
                    }
                    None => {
                        skill.id = new_id();
                        skill.version = 1;
                        skill.created_at = now;
                        skills.push(skill);
                    }
                }
            }
            emit_skills(shared);
        }
        ClientCommand::SkillDelete { id } => {
            shared.skills.lock().unwrap().retain(|s| s.id != id);
            emit_skills(shared);
        }
        ClientCommand::RefreshChannels => emit_channels(shared),
        ClientCommand::ChannelPairNew {
            project_id,
            chat_id,
        } => {
            // Mock relay: mint a code now; "the user" pairs a Telegram chat with it after a moment.
            let code = format!("{:06}", now_ms() % 1_000_000);
            {
                let mut view = shared.channels.lock().unwrap();
                view.pairing = Some(ChannelPairing {
                    code: code.clone(),
                    project_id: project_id.clone(),
                    chat_id: chat_id.clone().unwrap_or_else(|| CHAT_COMMS_MAIN.into()),
                    expires_at: now_ms() + 10 * 60 * 1000,
                });
            }
            emit_channels(shared);
            let shared = shared.clone();
            tokio::spawn(async move {
                sleep(Duration::from_secs(6)).await;
                {
                    let mut view = shared.channels.lock().unwrap();
                    if view.pairing.as_ref().map(|p| p.code.as_str()) != Some(code.as_str()) {
                        return;
                    }
                    view.pairing = None;
                    view.bindings.insert(
                        0,
                        ChannelBinding {
                            id: new_id(),
                            platform: "telegram".into(),
                            external_chat_id: "8812".into(),
                            external_user_id: "8812".into(),
                            project_id: project_id.clone(),
                            chat_id: chat_id.unwrap_or_else(|| CHAT_COMMS_MAIN.into()),
                            label: "Telegram · DM · Jacob".into(),
                            bound_at: now_ms(),
                        },
                    );
                }
                emit_channels(&shared);
                let _ = shared.events.send(SyncEvent::Notice(
                    "Telegram · DM · Jacob paired with /pair (fixture).".into(),
                ));
            });
        }
        ClientCommand::ChannelUnbind { id } => {
            shared
                .channels
                .lock()
                .unwrap()
                .bindings
                .retain(|b| b.id != id);
            emit_channels(shared);
        }
        ClientCommand::LocalRefresh => emit_local(shared),
        ClientCommand::MachineDebugRefresh => emit_box_report(shared),
        ClientCommand::LocalSetAllowReads { device_id, allow } => {
            if let Some(d) = shared
                .local
                .lock()
                .unwrap()
                .devices
                .iter_mut()
                .find(|d| d.device_id == device_id)
            {
                d.allow_reads = allow;
            }
            emit_local(shared);
        }
        ClientCommand::LocalForget { device_id } => {
            shared
                .local
                .lock()
                .unwrap()
                .devices
                .retain(|d| d.device_id != device_id);
            emit_local(shared);
        }
    }
}

fn emit_channels(shared: &Shared) {
    let view = shared.channels.lock().unwrap().clone();
    let _ = shared.events.send(SyncEvent::Channels(view));
}

/// Settings → Channels demo: Beeper configured through Harness Comms, Slack in mock mode, the
/// others missing credentials; one WhatsApp chat already bound to Comms.
fn seed_channels() -> ChannelsView {
    let platform =
        |platform: &str, label: &str, state: ChannelPlatformState, detail: &str| ChannelPlatform {
            platform: platform.into(),
            label: label.into(),
            state,
            detail: detail.into(),
            inbound: String::new(),
        };
    ChannelsView {
        mock: false,
        platforms: vec![
            platform(
                "slack",
                "Slack",
                ChannelPlatformState::Mock,
                "ARBOS_CHANNELS_MOCK=1 · inbound via POST /channels/mock/inbound",
            ),
            platform(
                "telegram",
                "Telegram",
                ChannelPlatformState::Missing,
                "missing ARBOS_TELEGRAM_BOT_TOKEN",
            ),
            platform(
                "whatsapp",
                "WhatsApp (Cloud API)",
                ChannelPlatformState::Configured,
                "Cloud API webhook + messages",
            ),
            platform(
                "beeper",
                "Beeper (Harness Comms)",
                ChannelPlatformState::Configured,
                "via Harness Comms · https://comms.example.trycloudflare.com",
            ),
        ],
        bindings: vec![ChannelBinding {
            id: new_id(),
            platform: "whatsapp".into(),
            external_chat_id: "15555550123".into(),
            external_user_id: "15555550123".into(),
            project_id: PROJECT_COMMS.into(),
            chat_id: CHAT_COMMS_MAIN.into(),
            label: "WhatsApp · Jacob".into(),
            bound_at: now_ms().saturating_sub(3 * 86_400_000),
        }],
        pairing: None,
        loaded: true,
    }
}

fn emit_local(shared: &Shared) {
    let view = shared.local.lock().unwrap().clone();
    let _ = shared.events.send(SyncEvent::LocalDevices(view));
}

/// Settings → This Mac demo: one helper online, reads still ask.
fn seed_local() -> LocalView {
    LocalView {
        devices: vec![LocalDevice {
            device_id: "01JFIXTURELOCALMACBOOK0001".into(),
            name: "Jacob's MacBook Pro".into(),
            hostname: "jacobs-mbp".into(),
            os: "macos".into(),
            home: "/Users/jacob".into(),
            connected: true,
            last_seen: Some(now_ms() as i64 - 12_000),
            allow_reads: false,
        }],
        relay_url: "http://127.0.0.1:7700".into(),
        loaded: true,
    }
}

/// The user row (attachments first, then the text), its `send` command, and the scripted turn.
#[allow(clippy::too_many_arguments)]
fn send_user(
    shared: &Arc<Shared>,
    chat_id: Id,
    message_id: Id,
    text: String,
    attachments: Vec<Attachment>,
    reply_to: Option<String>,
    forwarded_from: Option<String>,
    reply_ref: Option<ReplyRef>,
    tokens: Vec<String>,
) {
    let project_id = project_of(shared, &chat_id);
    if let Some(space) = people_space_of(shared, &chat_id) {
        let mentions = mentions_in(&space, &text, &tokens);
        // Rig (#1596): `ARBOS_FIXTURE_TRACE_SENDS=1` prints what the row carries.
        if std::env::var_os("ARBOS_FIXTURE_TRACE_SENDS").is_some() {
            eprintln!("fixture send: text={text:?} chips={tokens:?} mentions={mentions:?}");
        }
        let me = me();
        let opts = UserMessageOpts {
            attachments: user_attachments(&attachments),
            mentions: mentions.clone(),
            reply_to,
            sender: Some(format!("user:{}", me.id)),
            sender_handle: Some(me.handle.clone()),
            sender_name: me.name.clone(),
            sender_avatar: Some(me.avatar_url.clone()),
            answers_question_id: None,
            tz: None,
        };
        with_doc(shared, &chat_id, |doc| {
            doc.core.transact(|d| {
                let _ = d.append_user_message(&message_id, &text, DEVICE, &opts);
                if let Some(by) = forwarded_from.as_deref() {
                    d.set_message_fields(&message_id, &[("forwardedFrom", json!(by))]);
                }
                if let Some(r) = reply_ref.as_ref() {
                    d.set_reply_ref(&message_id, r);
                }
                d.push_command("send", json!({ "messageId": message_id }), DEVICE);
            });
        });
        people_after_send(shared, space, chat_id, text, mentions);
        return;
    }
    let asleep = machine_down(shared);
    let opts = UserMessageOpts {
        attachments: user_attachments(&attachments),
        reply_to,
        ..UserMessageOpts::default()
    };
    with_doc(shared, &chat_id, |doc| {
        doc.core.transact(|d| {
            let _ = d.append_user_message(&message_id, &text, DEVICE, &opts);
            if let Some(by) = forwarded_from.as_deref() {
                d.set_message_fields(&message_id, &[("forwardedFrom", json!(by))]);
            }
            if let Some(r) = reply_ref.as_ref() {
                d.set_reply_ref(&message_id, r);
            }
            if asleep {
                set_status(d, &message_id, MessageStatus::Queued);
            }
            d.push_command(
                "send",
                json!({ "messageId": message_id, "text": text }),
                DEVICE,
            );
        });
    });
    maybe_retitle(shared, &chat_id, &text);
    spawn_turn(shared, project_id, chat_id, text, false);
}

fn emit_connectors(shared: &Shared) {
    let view = shared.connectors.lock().unwrap().clone();
    let _ = shared.events.send(SyncEvent::Connectors(view));
}

fn emit_secrets(shared: &Shared) {
    let view = shared.secrets.lock().unwrap().clone();
    let _ = shared.events.send(SyncEvent::Secrets(view));
}

fn emit_devices(shared: &Shared) {
    let list = shared.devices.lock().unwrap().clone();
    let _ = shared.events.send(SyncEvent::Devices(Ok(list)));
}

/// Three devices: this Mac (current), an iPhone seen an hour ago, and a
/// read-only Grok Bot reader (`kind: "reader"`) from last week.
pub fn seed_devices() -> Vec<DeviceView> {
    let now = now_ms() as i64;
    vec![
        DeviceView {
            id: "dev-fx-mac".into(),
            name: "Jacob's MacBook Pro".into(),
            kind: "github".into(),
            created_at: now - 12 * 24 * 3_600_000,
            last_seen_at: Some(now - 20_000),
            revoked_at: None,
            current: true,
        },
        DeviceView {
            id: "dev-fx-iphone".into(),
            name: "iPhone".into(),
            kind: "github".into(),
            created_at: now - 9 * 24 * 3_600_000,
            last_seen_at: Some(now - 2 * 3_600_000),
            revoked_at: None,
            current: false,
        },
        DeviceView {
            id: "dev-fx-grokbot".into(),
            name: "GrokBot-Arbos".into(),
            kind: "reader".into(),
            created_at: now - 6 * 24 * 3_600_000,
            last_seen_at: Some(now - 3 * 24 * 3_600_000),
            revoked_at: None,
            current: false,
        },
    ]
}

fn fixture_agent(project_id: &str, name: &str, enabled: bool, note: &str) -> SecretAgent {
    let name = if name.is_empty() {
        match project_id {
            PROJECT_COMMS => "Comms",
            PROJECT_ARBOS => "Arbos",
            PROJECT_AFFINE => "Affine",
            other => other,
        }
        .to_string()
    } else {
        name.to_string()
    };
    SecretAgent {
        project_id: project_id.into(),
        name,
        access_id: new_id(),
        enabled,
        note: note.into(),
        hosts: Vec::new(),
        scope: SecretScope {
            kind: "all".into(),
            chat_id: None,
        },
        last_used_at: enabled.then(|| now_ms() as i64 - 120_000),
        disabled_at: (!enabled).then(|| now_ms() as i64 - 6 * 86_400_000),
    }
}

/// One People row for the fixture (spec §7.7).
fn fixture_person(
    p: &Person,
    enabled: bool,
    note: &str,
    last_asked_ago_ms: Option<i64>,
) -> SecretDirector {
    SecretDirector {
        user_id: p.id.clone(),
        handle: p.handle.clone(),
        name: p.name.clone(),
        avatar_url: Some(p.avatar_url.clone()),
        note: note.into(),
        created_at: now_ms() as i64 - 9 * 86_400_000,
        enabled,
        disabled_at: (!enabled).then(|| now_ms() as i64 - 2 * 86_400_000),
        scope: SecretScope::default(),
        last_asked_at: last_asked_ago_ms.map(|ago| now_ms() as i64 - ago),
    }
}

/// Settings → Secrets demo (D83): a Stripe key for Research and Comms, a Mailgun key for Comms,
/// a GitHub login for Website, one pending request from Comms, recent activity. No value exists
/// anywhere in the fixture. STRIPE_KEY has two people on its list (Sam on, Samir off).
fn seed_secrets() -> SecretsView {
    let now = now_ms() as i64;
    let secret = |name: &str,
                  kind: &str,
                  note: &str,
                  hosts: &[&str],
                  agents: Vec<SecretAgent>,
                  days_ago: i64| Secret {
        id: new_id(),
        name: name.into(),
        env: crate::model::secret_env_name(name),
        kind: kind.into(),
        note: note.into(),
        hosts: hosts.iter().map(|h| h.to_string()).collect(),
        ask_every_time: false,
        inject_env: false,
        expires_at: None,
        version: 1,
        created_at: now - days_ago * 86_400_000,
        last_used_at: (days_ago > 3).then(|| now - 120_000),
        username: (kind == "login").then(|| "unarbos".to_string()),
        vault_name: "Arbos".into(),
        agents,
        directors: Vec::new(),
        others_ask: "refuse".into(),
        source: None,
    };
    let mut stripe = secret(
        "STRIPE_KEY",
        "key",
        "Stripe live key — refunds and reports. Never for payouts.",
        &["api.stripe.com"],
        vec![
            fixture_agent(PROJECT_AFFINE, "Affine", true, "pull refund reports"),
            fixture_agent(
                PROJECT_COMMS,
                "Comms",
                true,
                "monthly revenue line in the newsletter",
            ),
            fixture_agent(PROJECT_ARBOS, "Arbos", false, ""),
        ],
        12,
    );
    // The iPhone fixture's twin: the Stripe key is a real value in the agents' environment,
    // and Sam may ask for it (Samir's row is off).
    stripe.inject_env = true;
    // Secret hand-off rig: the key was given to Arbos in chat (list "given in chat", page
    // "Added · In chat with Arbos").
    if std::env::var("ARBOS_FIXTURE_SECRET_HANDOFF").is_ok() {
        stripe.inject_env = false;
        stripe.hosts = Vec::new();
        stripe.note = "For the refunds report. Read-only until you say otherwise.".into();
        stripe.last_used_at = None;
        stripe.created_at = now - 120_000;
        stripe.agents = vec![fixture_agent(
            PROJECT_ARBOS,
            "Arbos",
            true,
            "the refunds report",
        )];
        stripe.source = Some(crate::model::SecretSource {
            kind: "chat".into(),
            project_id: Some(PROJECT_ARBOS.into()),
            project_name: Some("Arbos".into()),
            chat_id: Some(CHAT_ARBOS_MAIN.into()),
        });
    }
    let dir = directory();
    stripe.directors = vec![
        fixture_person(&dir[0], true, "reports only", Some(120_000)),
        fixture_person(&dir[1], false, "", None),
    ];
    let secrets = vec![
        stripe,
        secret(
            "MAILGUN_API_KEY",
            "key",
            "Newsletter sends.",
            &["api.mailgun.net"],
            vec![fixture_agent(
                PROJECT_COMMS,
                "Comms",
                true,
                "send the newsletter",
            )],
            5,
        ),
        secret(
            "GitHub login",
            "login",
            "The unarbos account, for Actions and reviews.",
            &["github.com", "*.github.com"],
            vec![fixture_agent(
                PROJECT_ARBOS,
                "Arbos",
                true,
                "review failing Actions runs",
            )],
            1,
        ),
    ];
    let requests = vec![SecretRequest {
        id: new_id(),
        project_id: PROJECT_COMMS.into(),
        project_name: Some("Comms".into()),
        chat_id: CHAT_COMMS_MAIN.into(),
        name: "GITHUB_LOGIN".into(),
        purpose: "Sign in to post the release notes".into(),
        use_kind: "browser_fill".into(),
        host: Some("github.com".into()),
        status: "pending".into(),
        created_at: now - 180_000,
        decided_at: None,
        asked_by_handle: None,
        asked_by_name: None,
    }];
    let act = |action: &str,
               name: &str,
               project: Option<(&str, &str)>,
               host: Option<&str>,
               by: &str,
               mins_ago: i64| SecretActivity {
        id: mins_ago,
        at: now - mins_ago * 60_000,
        action: action.into(),
        name: name.into(),
        target_kind: "secret".into(),
        project_id: project.map(|(id, _)| id.to_string()),
        project_name: project.map(|(_, n)| n.to_string()),
        host: host.map(str::to_string),
        by: by.into(),
        detail: serde_json::json!({}),
    };
    let mut activity = vec![
        act(
            "used",
            "STRIPE_KEY",
            Some((PROJECT_AFFINE, "Affine")),
            Some("api.stripe.com"),
            "agent:research",
            2,
        ),
        act(
            "access_on",
            "MAILGUN_API_KEY",
            Some((PROJECT_COMMS, "Comms")),
            None,
            "user:me",
            1_440,
        ),
        act(
            "refused",
            "STRIPE_KEY",
            Some((PROJECT_ARBOS, "Arbos")),
            Some("stripe.evil.example"),
            "agent:website",
            4_320,
        ),
    ];
    // Sam asked Affine for the Stripe key (spec §7.7): the row names who asked.
    activity[0].detail = serde_json::json!({ "asked_by": SAM_ID, "asked_by_handle": "sam" });
    // One connected 1Password vault (Phase 2), names only — the iPhone fixture's twin. Seeded only
    // with `ARBOS_FIXTURE_VAULT=1` (screenshot rig): the shipped default hides the connect flow
    // (`features.vault_connect` off) and no user has a vault, so the page shows secrets only.
    let fixture_vault = std::env::var("ARBOS_FIXTURE_VAULT").is_ok_and(|v| v == "1");
    let mut vaults = vec![crate::model::ConnectedVault {
        id: "fx-vault-1p".into(),
        provider: "onepassword".into(),
        name: "Arbos — Research".into(),
        items_count: 12,
        token_expires_at: None,
        status: "ok".into(),
        agents: vec![
            fixture_agent(
                PROJECT_AFFINE,
                "Affine",
                true,
                "read the Stripe and Postgres items",
            ),
            fixture_agent(PROJECT_COMMS, "Comms", true, "read the Stripe item"),
        ],
        limits: serde_json::json!({ "reads_per_hour": 1000, "reads_used_hour": 3 }),
        items: vec![
            serde_json::json!({ "ref": "op://Arbos - Research/Stripe", "title": "Stripe", "category": "API_CREDENTIAL", "fields": ["credential", "username"], "urls": ["https://dashboard.stripe.com"], "env": "STRIPE", "inject_env": true }),
            serde_json::json!({ "ref": "op://Arbos - Research/Postgres", "title": "Postgres", "category": "DATABASE", "fields": ["password", "hostname", "port", "database"], "urls": [], "env": "POSTGRES", "inject_env": false }),
        ],
        env_items: 1,
    }];
    if !fixture_vault {
        vaults.clear();
    }
    SecretsView {
        paused: false,
        secrets,
        vaults,
        providers: if fixture_vault {
            vec!["onepassword".into()]
        } else {
            Vec::new()
        },
        vault_connect: fixture_vault,
        requests,
        activity,
        loaded: true,
    }
}

/// Settings → Connectors demo: GitHub and Harness Comms connected, Google waiting for sign-in,
/// Slack not connected; everything flagged mock.
fn seed_connectors() -> ConnectorsView {
    let act = |name: &str, kind: &str| ConnectorAction {
        name: name.into(),
        kind: kind.into(),
    };
    let providers = vec![
        ConnectorProvider {
            provider: "google".into(),
            label: "Google (Gmail, Calendar)".into(),
            connect: "oauth".into(),
            oauth_configured: false,
            actions: vec![
                act("list_threads", "read"),
                act("search", "read"),
                act("read_message", "read"),
                act("create_draft", "write"),
                act("send", "send"),
                act("list_events", "read"),
                act("create_event", "send"),
            ],
        },
        ConnectorProvider {
            provider: "slack".into(),
            label: "Slack".into(),
            connect: "oauth_or_token".into(),
            oauth_configured: false,
            actions: vec![
                act("list_channels", "read"),
                act("read_channel", "read"),
                act("search", "read"),
                act("post_message", "send"),
            ],
        },
        ConnectorProvider {
            provider: "github".into(),
            label: "GitHub".into(),
            connect: "oauth_or_token".into(),
            oauth_configured: false,
            actions: vec![
                act("list_repos", "read"),
                act("list_issues", "read"),
                act("read_issue", "read"),
                act("list_prs", "read"),
                act("create_issue", "write"),
                act("comment", "send"),
            ],
        },
        ConnectorProvider {
            provider: "harness_comms".into(),
            label: "Harness Comms (Beeper + Gmail)".into(),
            connect: "token".into(),
            oauth_configured: true,
            actions: vec![
                act("status", "read"),
                act("search", "read"),
                act("read", "read"),
                act("search_conversations", "read"),
                act("read_chat", "read"),
                act("draft", "write"),
                act("send", "send"),
            ],
        },
    ];
    let items = vec![
        Connector {
            id: "01J9CONNGITHUB00000000000".into(),
            provider: "github".into(),
            label: "GitHub · unarbos".into(),
            scopes: vec!["repo".into(), "read:user".into()],
            status: ConnectorStatus::Connected,
            last_error: None,
            mock: true,
        },
        Connector {
            id: "01J9CONNGOOGLE00000000000".into(),
            provider: "google".into(),
            label: "Google (Gmail, Calendar)".into(),
            scopes: Vec::new(),
            status: ConnectorStatus::NeedsAuth,
            last_error: Some("consent not finished".into()),
            mock: true,
        },
        Connector {
            id: "01J9CONNHARNESS0000000000".into(),
            provider: "harness_comms".into(),
            label: "Harness Comms · arbos-comms".into(),
            scopes: vec![
                "read".into(),
                "search".into(),
                "draft".into(),
                "request_send".into(),
            ],
            status: ConnectorStatus::Connected,
            last_error: None,
            mock: true,
        },
    ];
    ConnectorsView {
        mock: true,
        providers,
        items,
        loaded: true,
    }
}

/// The Computer tab's report, scripted from the fixture's machine: an awake box with the
/// fixture's spec, one workspace per agent project, the streaming one mid-turn.
fn emit_box_report(shared: &Shared) {
    let machine = shared.machine.lock().unwrap().clone();
    let streaming: Vec<Id> = shared
        .running
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, on)| on.load(Ordering::SeqCst))
        .map(|(chat, _)| chat.clone())
        .collect();
    let chats = shared.chats.lock().unwrap().clone();
    // `ARBOS_FIXTURE_WORKING=<n>`: the first n agents are mid-turn here too.
    let working_n = fixture_working_count();
    let state = match machine.shown_state() {
        MachineState::Awake => "awake",
        MachineState::Waking => "waking",
        MachineState::Stopped => "stopped",
        MachineState::Error => "error",
        MachineState::Asleep | MachineState::Unknown => "asleep",
    };
    let reachable = machine.shown_state() == MachineState::Awake;
    let workspaces: Vec<BoxWorkspace> = shared
        .projects
        .lock()
        .unwrap()
        .iter()
        .filter(|p| !p.is_people())
        .enumerate()
        .map(|(i, p)| {
            let running = i < working_n
                || streaming
                    .iter()
                    .any(|c| chats.iter().any(|ch| &ch.id == c && ch.project_id == p.id));
            // Settings › Machine's per-agent breakdown: a share per workspace, the busy ones
            // biggest ([feedback machine-page-mac-2026-10-26]).
            let (cpu, mem, disk) = match (running, i) {
                (true, 0) => (61.0, 1_900, 2_300),
                (true, _) => (9.0, 1_100, 900),
                (false, 1) => (0.0, 300, 1_600),
                (false, _) => (0.0, 200 + 50 * i as u64, 100 + 150 * i as u64),
            };
            BoxWorkspace {
                project_id: p.id.clone(),
                name: p.name.clone(),
                healthy: true,
                running,
                elapsed_ms: running.then_some(42_000),
                tool_calls: running.then_some(7),
                jobs_running: Some(if running && i == 0 { 1 } else { 0 }),
                cpu_pct: Some(cpu),
                mem_mb: Some(mem),
                pids: Some(if running { 40 } else { 12 }),
                disk_mb: Some(disk),
            }
        })
        .collect();
    // `ARBOS_FIXTURE_MACHINE=near-limit`: disk 92 % and memory 88 % used, CPU 71 % — the
    // warning state of the Machine page on the rig.
    let near_limit = std::env::var("ARBOS_FIXTURE_MACHINE").is_ok_and(|v| v == "near-limit");
    let (disk_avail, mem_avail, cpu_pct, rx, tx) = if near_limit {
        (819, 983, 71.0, 4_800_000.0, 300_000.0)
    } else {
        (6_900, 5_300, 23.0, 1_200_000.0, 80_000.0)
    };
    let report = BoxReport {
        fetched_at: now_ms(),
        state: state.into(),
        awake_for_s: reachable.then_some(5 * 3600 + 12 * 60),
        image: machine
            .image
            .clone()
            .or_else(|| Some("arbos-agent-0.1.114".into())),
        reachable,
        unreachable_reason: (!reachable).then(|| "box has no url yet (asleep or starting)".into()),
        uptime_s: reachable.then_some(5 * 3600 + 12 * 60),
        load_1m: reachable.then_some(0.42),
        load_is_host: true,
        mem_total_mb: reachable.then_some(8 * 1024),
        mem_available_mb: reachable.then_some(mem_avail),
        disk_total_mb: reachable.then_some(10 * 1024),
        disk_available_mb: reachable.then_some(disk_avail),
        cpu_cores: reachable.then_some(4.0),
        cpu_pct: reachable.then_some(cpu_pct),
        inodes_total: reachable.then_some(131_072),
        inodes_avail: reachable.then_some(if near_limit { 1_200 } else { 93_000 }),
        net_rx_bps: reachable.then_some(rx),
        net_tx_bps: reachable.then_some(tx),
        net_rx_today: reachable.then_some(if near_limit {
            6_100_000_000
        } else {
            2_000_000_000
        }),
        net_tx_today: reachable.then_some(if near_limit { 300_000_000 } else { 100_000_000 }),
        workspaces: if reachable { workspaces } else { Vec::new() },
    };
    let _ = shared.events.send(SyncEvent::BoxReport(report));
}

fn emit_routines(shared: &Shared, project_id: &str) {
    let routines: Vec<Routine> = shared
        .routines
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.project_id == project_id)
        .cloned()
        .collect();
    let _ = shared.events.send(SyncEvent::Routines {
        project_id: project_id.to_string(),
        routines,
    });
}

fn emit_routine_runs(shared: &Shared, routine_id: &str) {
    let runs = shared
        .routine_runs
        .lock()
        .unwrap()
        .get(routine_id)
        .cloned()
        .unwrap_or_default();
    let _ = shared.events.send(SyncEvent::RoutineRuns {
        routine_id: routine_id.to_string(),
        runs,
    });
}

/// What the box does on a `routine_run` push, scripted: the prompt lands in the chat as a user
/// message from `routine:<id>`, a reply streams, and the run row flips to `done`.
fn run_routine_now(shared: &Arc<Shared>, project_id: &str, routine_id: &str) {
    let Some(routine) = shared
        .routines
        .lock()
        .unwrap()
        .iter()
        .find(|r| r.id == routine_id)
        .cloned()
    else {
        return;
    };
    let run_id = new_id();
    let started = now_ms() as i64;
    {
        let mut runs = shared.routine_runs.lock().unwrap();
        runs.entry(routine_id.to_string()).or_default().insert(
            0,
            RoutineRun {
                id: run_id.clone(),
                routine_id: routine_id.to_string(),
                started_at: started,
                finished_at: None,
                status: RoutineRunStatus::Running,
                message_id: None,
                note: Some("cause: manual".into()),
            },
        );
        let mut routines = shared.routines.lock().unwrap();
        if let Some(r) = routines.iter_mut().find(|r| r.id == routine_id) {
            r.last_run_at = Some(started);
        }
    }
    emit_routine_runs(shared, routine_id);
    emit_routines(shared, project_id);

    let message_id = new_id();
    let chat_id = routine.chat_id.clone();
    if !shared.docs.lock().unwrap().contains_key(&chat_id) {
        return;
    }
    with_doc(shared, &chat_id, |doc| {
        doc.core.transact(|d| {
            let _ = d.begin_message(
                &message_id,
                "user",
                &format!("routine:{routine_id}"),
                DEVICE,
                CoreStatus::Done,
                &[("routineName", json!(routine.name))],
            );
            push_part(d, &message_id, &text_part(&routine.prompt));
        });
    });
    spawn_turn(
        shared,
        project_id.to_string(),
        chat_id.clone(),
        routine.prompt.clone(),
        true,
    );
    let shared = shared.clone();
    let routine_id = routine_id.to_string();
    tokio::spawn(async move {
        // Wait for the scripted reply to settle, then close the run with its message id.
        for _ in 0..40 {
            sleep(Duration::from_millis(250)).await;
            let settled = shared.running.lock().unwrap().get(&chat_id).is_none();
            if settled {
                break;
            }
        }
        let reply_id = with_doc(&shared, &chat_id, |doc| {
            doc.read_all()
                .into_iter()
                .rev()
                .find(|m| m.role == Role::Assistant)
                .map(|m| m.id)
        })
        .flatten();
        {
            let mut runs = shared.routine_runs.lock().unwrap();
            if let Some(run) = runs
                .get_mut(&routine_id)
                .and_then(|v| v.iter_mut().find(|r| r.id == run_id))
            {
                run.status = RoutineRunStatus::Done;
                run.finished_at = Some(now_ms() as i64);
                run.message_id = reply_id;
                run.note = Some("Here is how I would approach it.".into());
            }
        }
        emit_routine_runs(&shared, &routine_id);
    });
}

fn emit_skills(shared: &Shared) {
    let skills = shared.skills.lock().unwrap().clone();
    let _ = shared.events.send(SyncEvent::Skills(skills));
}

fn add_doc(shared: &Arc<Shared>, doc: ChatDoc) {
    let chat_id = doc.chat_id.clone();
    let tx = shared.doc_tx.clone();
    let id_for_cb = chat_id.clone();
    let sub = doc.core.doc.subscribe_root(Arc::new(move |event| {
        let touched = ChatDoc::touched(&event);
        if touched.list_changed || !touched.messages.is_empty() {
            let _ = tx.send(DocChanged {
                chat_id: id_for_cb.clone(),
                touched,
            });
        }
    }));
    shared.subscriptions.lock().unwrap().push(sub);
    shared.docs.lock().unwrap().insert(chat_id, doc);
}

fn project_of(shared: &Shared, chat_id: &str) -> Id {
    shared
        .chats
        .lock()
        .unwrap()
        .iter()
        .find(|c| c.id == chat_id)
        .map(|c| c.project_id.clone())
        .unwrap_or_else(|| PROJECT_ARBOS.to_string())
}

fn project_state(shared: &Shared, project_id: &str) -> BoxState {
    shared
        .projects
        .lock()
        .unwrap()
        .iter()
        .find(|p| p.id == project_id)
        .map(|p| p.state)
        .unwrap_or(BoxState::Awake)
}

fn project_model(shared: &Shared, project_id: &str) -> (String, Effort) {
    shared
        .projects
        .lock()
        .unwrap()
        .iter()
        .find(|p| p.id == project_id)
        .map(|p| (p.model.clone(), p.effort))
        .unwrap_or((MODELS[0].to_string(), Effort::High))
}

fn set_project_state(
    shared: &Shared,
    project_id: &str,
    state: BoxState,
    detail: Option<&str>,
    progress: Option<f32>,
) {
    {
        let mut projects = shared.projects.lock().unwrap();
        if let Some(p) = projects.iter_mut().find(|p| p.id == project_id) {
            p.state = state;
            p.state_detail = detail.map(str::to_string);
            p.wake_progress = progress;
        }
    }
    let _ = shared.events.send(SyncEvent::ProjectStatus {
        project_id: project_id.to_string(),
        state,
        detail: detail.map(str::to_string),
        progress,
    });
}

fn emit_machine(shared: &Shared) {
    let m = shared.machine.lock().unwrap().clone();
    let _ = shared.events.send(SyncEvent::Machine(m));
}

fn machine_down(shared: &Shared) -> bool {
    let m = shared.machine.lock().unwrap();
    !m.always_on && m.state.is_down()
}

fn set_machine(shared: &Shared, state: MachineState, detail: Option<&str>, progress: Option<f32>) {
    {
        let mut m = shared.machine.lock().unwrap();
        if m.state != state {
            m.since = Some(now_ms());
        }
        m.state = state;
        m.detail = detail.map(str::to_string);
        m.progress = progress;
    }
    emit_machine(shared);
}

/// Start the user's machine: a short progress ramp, then every queued
/// message in every workspace is released.
async fn wake_machine(shared: &Arc<Shared>) {
    if !machine_down(shared) {
        return;
    }
    let was_stopped = shared.machine.lock().unwrap().state == MachineState::Stopped;
    let steps = if was_stopped { 18 } else { 14 };
    for i in 0..=steps {
        set_machine(
            shared,
            MachineState::Waking,
            Some("Starting your machine…"),
            Some(i as f32 / steps as f32),
        );
        sleep(Duration::from_millis(180)).await;
    }
    set_machine(shared, MachineState::Awake, None, None);
    let docs = shared.docs.lock().unwrap();
    for doc in docs.values() {
        release_queued(doc);
    }
}

fn emit_projects(shared: &Shared) {
    let projects = shared.projects.lock().unwrap().clone();
    let _ = shared.events.send(SyncEvent::Projects(projects));
}

/// `ARBOS_FIXTURE_BLOCKED=<handle>[,<handle>…]` (screenshot rig, [feedback
/// blocked-state-mac-2026-10-26]): those people are on my block list from launch, as the
/// relay's `GET /blocks` would list them — the Blocked chip / tag, the Unblock confirm and the
/// DM's "You blocked X" bar can be shot without a second account.
fn emit_blocked_hook(shared: &Arc<Shared>) {
    let Ok(spec) = std::env::var("ARBOS_FIXTURE_BLOCKED") else {
        return;
    };
    let handles: Vec<String> = spec
        .split(',')
        .map(|h| h.trim().trim_start_matches('@').to_lowercase())
        .filter(|h| !h.is_empty())
        .collect();
    if handles.is_empty() {
        return;
    }
    let people: Vec<Person> = shared
        .projects
        .lock()
        .unwrap()
        .iter()
        .flat_map(|p| p.people.iter().cloned())
        .filter(|p| handles.contains(&p.handle.to_lowercase()))
        .fold(Vec::new(), |mut acc: Vec<Person>, p| {
            if !acc.iter().any(|q| q.handle == p.handle) {
                acc.push(p);
            }
            acc
        });
    *shared.blocked.lock().unwrap() = people.clone();
    // The relay's `blockedSenders` hint on every people group that holds them.
    {
        let ids: Vec<String> = people.iter().map(|p| format!("user:{}", p.id)).collect();
        let mut projects = shared.projects.lock().unwrap();
        for p in projects.iter_mut().filter(|p| p.is_people_group()) {
            p.blocked_senders = ids
                .iter()
                .filter(|id| p.people.iter().any(|m| format!("user:{}", m.id) == **id))
                .cloned()
                .collect();
        }
    }
    emit_projects(shared);
    emit_blocked(shared);
}

fn emit_blocked(shared: &Shared) {
    let blocked = shared.blocked.lock().unwrap().clone();
    let _ = shared.events.send(SyncEvent::Blocked(blocked));
}

fn emit_project_docs(shared: &Shared) {
    let t = now_ms() as i64;
    let thread =
        |id: &str, name: &str, purpose: &str, summary: &str, status: &str, ago_min: i64| {
            ThreadInfo {
                id: id.into(),
                name: name.into(),
                purpose: purpose.into(),
                summary: summary.into(),
                status: status.into(),
                harness: "prime".into(),
                last_turn_at: Some(t - ago_min * 60_000),
            }
        };
    let arbos = ProjectDocView {
        threads: vec![
            thread("01J9THREADRAIL", "rail-layout", "Match the rail tiles to the mock", "Tiles 72 px, gap 10 px; cargo check clean.", "idle", 40),
            thread("01J9THREADDOCS", "docs-lookup", "Check GPUI list() alignment", "ListAlignment::Bottom confirmed; notes in docs/research/gpui.md.", "retired", 38),
        ],
        status: json!({ "box": "awake", "runningTurns": 0, "spendTodayUsd": 3.87 }),
        settings: json!({ "agentModel": MODELS[0], "agentEffort": "high" }),
        memory_t1: "- The UI is a messenger: rail left, chat center, nothing else at rest.\n- Jacob prefers conservative defaults chosen without asking.".into(),
        model_note: String::new(),
    };
    let _ = shared.events.send(SyncEvent::ProjectDoc {
        project_id: PROJECT_ARBOS.into(),
        view: arbos,
    });
}

fn emit_chats(shared: &Shared) {
    let chats = shared.chats.lock().unwrap().clone();
    let mut by_project: HashMap<Id, Vec<Chat>> = HashMap::new();
    for chat in chats {
        by_project
            .entry(chat.project_id.clone())
            .or_default()
            .push(chat);
    }
    for (project_id, chats) in by_project {
        let _ = shared.events.send(SyncEvent::Chats { project_id, chats });
    }
}

fn mark_read(shared: &Shared, chat_id: &str) {
    let changed = {
        let mut chats = shared.chats.lock().unwrap();
        match chats.iter_mut().find(|c| c.id == chat_id && c.unread) {
            Some(c) => {
                c.unread = false;
                c.unread_count = 0;
                true
            }
            None => false,
        }
    };
    if changed {
        emit_chats(shared);
    }
}

/// Seeing a mention clears one `@` and nothing else — in particular not the
/// chat's unread messages, which is the whole reason the relay route is
/// separate from `/read`.
fn mark_mentions_read(shared: &Shared, chat_id: &str) {
    let changed = {
        let mut chats = shared.chats.lock().unwrap();
        match chats
            .iter_mut()
            .find(|c| c.id == chat_id && c.mention_count > 0)
        {
            Some(c) => {
                c.mention_count -= 1;
                true
            }
            None => false,
        }
    };
    if changed {
        emit_chats(shared);
    }
}

fn maybe_retitle(shared: &Shared, chat_id: &str, text: &str) {
    let changed = {
        let mut chats = shared.chats.lock().unwrap();
        match chats
            .iter_mut()
            .find(|c| c.id == chat_id && !c.is_pinned && c.title == "New chat")
        {
            Some(c) => {
                c.title = title_from(text);
                c.last_message_at = now_ms();
                true
            }
            None => false,
        }
    };
    if changed {
        emit_chats(shared);
    }
}

fn title_from(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    let mut out: String = line.chars().take(34).collect();
    if line.chars().count() > 34 {
        out.push('…');
    }
    if out.is_empty() {
        "New chat".into()
    } else {
        out
    }
}

async fn wake_project(shared: &Arc<Shared>, project_id: &str) {
    let state = project_state(shared, project_id);
    if !state.is_asleep() && state != BoxState::Error {
        return;
    }
    // Workspaces open fast once the machine is up: a brief "waking" dot, then awake.
    set_project_state(shared, project_id, BoxState::Waking, None, None);
    sleep(Duration::from_millis(600)).await;
    set_project_state(shared, project_id, BoxState::Awake, None, None);
    let chat_ids: Vec<Id> = shared
        .chats
        .lock()
        .unwrap()
        .iter()
        .filter(|c| c.project_id == project_id)
        .map(|c| c.id.clone())
        .collect();
    let docs = shared.docs.lock().unwrap();
    for chat_id in chat_ids {
        if let Some(doc) = docs.get(&chat_id) {
            release_queued(doc);
        }
    }
}

fn spawn_turn(shared: &Arc<Shared>, project_id: Id, chat_id: Id, text: String, force_reply: bool) {
    let stop = Arc::new(AtomicBool::new(false));
    shared
        .running
        .lock()
        .unwrap()
        .insert(chat_id.clone(), stop.clone());
    let shared = shared.clone();
    tokio::spawn(async move {
        run_turn(&shared, &project_id, &chat_id, &text, force_reply, stop).await;
        shared.running.lock().unwrap().remove(&chat_id);
    });
}

enum Script {
    Reply(&'static [&'static str]),
    Act {
        tools: &'static [(&'static str, &'static str, &'static str, u64)],
        diff: Option<(&'static str, u32, u32)>,
        summary: &'static [&'static str],
    },
    Browse {
        title: &'static str,
        url: &'static str,
        summary: &'static [&'static str],
    },
    /// `ask_user`: lead sentence, question, optional typed widget (design §10).
    Ask(&'static str, &'static str, Option<Widget>),
    Fail(&'static str, &'static str),
    /// Grok Bot reply flow (agent-flow contract): an ack bubble, a working
    /// line whose `activity` swaps, a milestone bubble, then the result.
    Flow,
    /// S18: a draft that waits for approval before anything is sent.
    Draft {
        lead: &'static str,
        channel: &'static str,
        to: &'static str,
        subject: &'static str,
        body: &'static str,
    },
}

const FLOW_ACK: &str = "Cloning it now.";
/// (activity, how long it shows) — one line only; no milestone boilerplate for a quick clone.
const FLOW_ACTIVITIES: &[(&str, u64)] = &[("Cloning RaoFoundation/subtensor", 2200)];
const FLOW_RESULT: &[&str] = &["Got `subtensor/` on main. What do you want to do with it?"];

/// `ARBOS_FIXTURE_REPLY=long`: the shape of Jacob's Oct 23 screenshot — a
/// heading, bold labels and bullets over twenty-odd lines.
const LONG_REVIEW: &str = "Arbos, from where I sat:\n\n**What worked**\n- Authority held: three guest agents deferred to the owner's call every time.\n- The handoff lines read cleanly; nobody asked who was speaking.\n- Replies landed inside two seconds on every box.\n\n**What did not**\n- The group lost the thread twice when two agents answered at once.\n- One attachment never rendered on the phone.\n- The working line stayed up after a stopped turn.\n\n**Next**\n1. Serialize agent replies in groups.\n2. Re-check the attachment pipeline on iPhone.\n3. Clear the working line on stop.\n\nTell me which of these you want first.";

const REPLY_DEFAULT: &[&str] = &[
    "Here is how I would approach it.",
    "- Keep the rail narrow and the tabs flat, so the chat column gets the width.\n- Put the composer at the bottom once a chat has a message; before that, center it so a new tab feels like a blank page.\n- Treat every tool call as a card that folds, so long runs stay readable.",
    "If you want, I can turn this into a short checklist and start on the first item.",
];

/// "rules" / "long": a several-paragraph answer with a list and a code span,
/// the shape of Jacob's bubble in #1389 (selection / copy rigs; the iPhone
/// fixture answers the same words with the same text).
const REPLY_RULES: &[&str] = &[
    "I can't see anything in my records that points to an app bug, so I wouldn't blame the app without more evidence.",
    "The likeliest cause is a prompt that never told the agents to wait for everyone and to keep quiet between turns:",
    "- post only the number\n- call `I won` only for the lowest\n- wait for every player before the next round",
    "The app side plays a part too: several agents answer the same message at once, and some of my filings went out before you could stop them.",
    "Do you want me to post a clear rule summary to the group before the next round?",
];

const REPLY_SIMPLE: &[&str] = &[
    "Agreed. Two sections only: the rail on the left and the chat in the center.",
    "Nothing else at rest. Details open only when you ask for them.",
];

/// The reply to "Find me a book store in lima" (agent Bubble, 2026-10-25 21:31 UTC), one
/// string per bubble, byte for byte: conclusion, GFM table of linked stores, details
/// ([feedback markdown-tables-mac-2026-10-25]; the iPhone fixture's `limaReply`).
const LIMA_REPLY: &[&str] = &[
    "Go to Librería El Virrey in Miraflores. Their site lists Sunday hours as 11am–7pm, so it should still be open this afternoon.",
    "| Store | Area | Sunday | What they sell |\n| --- | --- | --- | --- |\n| [Librería El Virrey](https://www.elvirrey.com/) | Miraflores, Calle Bolognesi 510 | 11am–7pm | Spanish literature, essays, history, art; some English at a higher price |\n| [Communitas](https://www.communitas.pe/) | San Isidro, Av. 2 de Mayo 1690 | 10:30am–7:30pm | Humanities, literature, law, history, science, in Spanish |\n| [Vallejo Librería Café](https://www.tangol.com/blog/en/the-3-best-bookstores-in-lima/n-15210) | San Isidro, Av. Camino Real 1119 | not listed | Books plus coffee or tea |\n| [Assouline Lima](https://www.assouline.com/pages/store/lima) | Miraflores, street not listed | not listed | Luxury and design books |",
    "El Virrey’s phone is (511) 713 0505. Their own page says the shop dates to late 1973; a travel write-up said 1972. Communitas is the other store with Sunday hours on its site, but it is in San Isidro, not Miraflores, and the page does not clearly say it is a walk-in shop, so check before you go. Vallejo is a bookstore-café; I only have a travel blog for it, and hours were not listed. Assouline is a luxury boutique, not a general bookstore. La Casa de Kanu in Barranco was called a favorite on an expat guide, but I have no street address or hours.\n\nHours were not checked on a second source, and I did not confirm anyone is open right now. No used-book shop and no central Lima store made this list with an address I could verify.\n\nWant me to check whether El Virrey is open right now, or look for an English-language shop?",
];

/// The reply to "Read the latest from the New York post" (agent Bubble, 2026-10-25, Jacob's
/// screenshot): seven linked headlines as bullets, then one line of prose.
/// Jacob's 2026-10-27 bubble (Mac v0.6.296): the URL wrapped as `https:/` | `/treatment-…`
/// and would not open ([feedback link-click-mac-2026-10-27]).
const LINK_REPLY: &str = "https://treatment-perfume-marble-tournaments.trycloudflare.com/jacob/competitors.html\n\nI checked it from outside and it loads. No zip or file card needed.";

const NYPOST_REPLY: &str = "- [Syringe attack: lawyer claims mistaken identity](https://nypost.com/2026/10/25/us-news/syringe-attack-lawyer-claims-mistaken-identity/)\n- [Arrest story: a second Jewish victim minutes earlier](https://nypost.com/2026/10/25/us-news/arrest-story-second-jewish-victim-minutes-earlier/)\n- [Ex-NYPD cop charged in a George Washington Bridge killing, free on $1M bail](https://nypost.com/2026/10/25/us-news/ex-nypd-cop-charged-in-gw-bridge-killing-free-on-1m-bail/)\n- [Thomas Markle home search, child pornography, Philippine official’s comment](https://nypost.com/2026/10/25/world-news/thomas-markle-home-search-philippine-official-comment/)\n- [Musk says SpaceX cut Ukraine’s Starlink access](https://nypost.com/2026/10/25/business/musk-says-spacex-cut-ukraines-starlink-access/)\n- [Giants beat the Texans in overtime in J. J. McCarthy’s first start](https://nypost.com/2026/10/25/sports/giants-beat-texans-in-overtime-mccarthy-first-start/)\n- [McCarthy’s wife after that win](https://nypost.com/2026/10/25/sports/mccarthys-wife-after-that-win/)\n\nNo times or bylines were on the page text I read. Want one of these opened?";

/// The chat-formatting sample sheet, my own bubble: one line per format, labelled
/// ([feedback chat-markdown-mac-2026-10-27]). `ARBOS_FIXTURE_REPLY=markdown`.
pub const MARKDOWN_SAMPLE_USER: &str = "> quote: a line that starts with \"> \"\n> second quoted line\n>> nested quote inside it\n\nbold: **bold text** · italic: *italic* and _italic_ · strike: ~~gone~~\ninline code: `cargo test -p arbos-core` · spoiler (click): ||the butler did it||\nlink: [Arbos docs](https://docs.arbos.life/formatting) · bare: https://arbos.life/x_y · mention: @sam · emoji: 🎉 👍🏽\n\n# Heading 1\n## Heading 2\n### Heading 3\n\n- bullet one\n- bullet **two**\n* star bullet\n\n1. first\n2. second\n\n```python\ndef hello(name):\n    return f\"hi {name}\"  # fenced, copyable\n```\n\nescapes: \\*not italic\\* \\> not a quote \\`not code\\`";

/// The agent's answer on the sample sheet: the same grammar in a grey bubble.
pub const MARKDOWN_SAMPLE_REPLY: &str = "> quote: what you sent, quoted back\n> with a second line\n\nEvery format renders the same on both sides: **bold**, *italic*, ~~strike~~, `code`, ||spoiler||, [a named link](https://docs.arbos.life/formatting) and https://arbos.life/x_y.\n\n## Heading 2 in a reply\n\n- list item with `code`\n- and a [link](https://arbos.life)\n\n1. numbered\n2. list\n\n```rust\nlet quoted = parse_blocks(\"> hello\");\n```\n\nMath stays math: $O(n^2)$, and prices stay prices: $5 and $10.";

/// The must-not-mangle lines: plain text the grammar must leave exactly as typed.
pub const MARKDOWN_SAMPLE_PLAIN: &str = "plain, must not change:\nsnake_case_word __init__ some_var_name\n2*3*4 = 24 and 5 * 3 = 15\nhttps://example.com/some_path_with_underscores\njacob_s@example.com\n>5 means more than five · a > b · lone > here\n<b>html</b> stays <i>text</i>\n#hashtag #1 fan C++ C# 100%\n** not bold ** · a*b · trailing* · *leading";

/// The reply to "write bubble sort in python" from Jacob's 2026-10-26 10:50 Mac screenshot,
/// byte for byte: a Python code card, then prose whose `$O(n^2)$` showed its dollars and
/// caret verbatim ([feedback math-render-mac-2026-10-26]). `ARBOS_FIXTURE_REPLY=math`.
pub const BUBBLE_SORT_REPLY: &str = "Here is a standard implementation of bubble sort in Python:\n\n```python\ndef bubble_sort(arr):\n    n = len(arr)\n    for i in range(n):\n        swapped = False\n        for j in range(0, n - i - 1):\n            if arr[j] > arr[j + 1]:\n                arr[j], arr[j + 1] = arr[j + 1], arr[j]\n                swapped = True\n        # If no two elements swapped, list is already sorted\n        if not swapped:\n            break\n    return arr\n\n# Example usage:\nitems = [64, 34, 25, 12, 22, 11, 90]\nprint(bubble_sort(items))  # Output: [11, 12, 22, 25, 34, 64, 90]\n```\n\nIt runs in $O(n^2)$ time in the worst and average cases, and $O(n)$ time when the array is already sorted thanks to the `swapped` flag. Space complexity is $O(1)$.";

const REPLY_CODE: &[&str] = &[
    "The transcript is a bottom-aligned list with one row per markdown block:",
    "```rust\nlet list = ListState::new(0, ListAlignment::Bottom, px(320.0));\nlist(list.clone(), cx.processor(Self::render_row))\n```",
    "Rows are keyed by `{msg}#{part}.{block}` and re-spliced only when their bytes change, so a streaming reply never re-renders the settled prefix.",
];

const ACT_TOOLS: &[(&str, &str, &str, u64)] = &[
    (
        "read_file",
        "crates/arbos-mac/src/ui/rail.rs",
        "90 px rail, 72 px tiles, 11 px labels",
        420,
    ),
    (
        "grep",
        "TILE_W crates/",
        "3 matches in rail.rs, theme.rs",
        180,
    ),
    (
        "edit_file",
        "crates/arbos-mac/src/ui/rail.rs",
        "Set tile gap to 10 px",
        640,
    ),
    (
        "bash",
        "cargo check -p arbos-mac",
        "Finished `dev` profile in 4.1 s",
        1900,
    ),
];

const ACT_SUMMARY: &[&str] = &[
    "Done. The tiles were 68 px wide with an 8 px gap; the mock uses 72 px and 10 px.",
    "I changed the two constants in `rail.rs`, re-ran `cargo check`, and it is clean. The diff is above if you want to look.",
];

const BROWSE_SUMMARY: &[&str] = &[
    "Yes. `ListAlignment::Bottom` scrolls from the bottom up, like a chat log, and `ListSizingBehavior::Auto` lets the list fill the column.",
    "I saved the relevant page as `docs/research/gpui.md` in the sandbox.",
];

const ASK_LEAD: &str = "I can do that. One thing first:";
const ASK_QUESTION: &str =
    "Which date should I plan toward: the 1 December private beta, or the January public launch?";

const REPLY_ANSWERED: &[&str] =
    &["Noted. I will go with that and come back when there is something to look at."];
const DRAFT_LEAD: &str = "Here is the draft. Nothing goes out until you approve the card below.";
const DRAFT_TO: &str = "priya@northlight.vc";
const DRAFT_SUBJECT: &str = "Arbos — October investor update";
const DRAFT_BODY: &str = "Hi Priya,\n\nQuick October update from Arbos:\n\n- The Mac beta is feature-complete; 14 external testers are on it this week.\n- Box boot is down to 2.1 s warm and 28 s cold.\n- We closed the design partner agreement with Affine.\n\nHappy to walk you through any of it on Thursday.\n\nBest,\nJacob";

fn pick_script(text: &str, force_reply: bool) -> Script {
    let t = text.to_lowercase();
    if force_reply {
        return Script::Reply(REPLY_ANSWERED);
    }
    if t.contains("clone") || t.contains("repo") || t.contains("summar") {
        return Script::Flow;
    }
    if t.contains("draft") || t.contains("email") || t.contains("send ") {
        return Script::Draft {
            lead: DRAFT_LEAD,
            channel: "email",
            to: DRAFT_TO,
            subject: DRAFT_SUBJECT,
            body: DRAFT_BODY,
        };
    }
    if t.contains("fail") || t.contains("error") || t.contains("timeout") {
        return Script::Fail("sandbox_timeout", "Sandbox did not answer in 60 s");
    }
    if t.contains("which") || t.contains("choose") || t.contains("pick one") {
        return Script::Ask(
            ASK_LEAD,
            "Which build should the beta ship from?",
            Some(widget_choice()),
        );
    }
    if t.contains("select") || t.contains("several") || t.contains("platforms") {
        return Script::Ask(
            ASK_LEAD,
            "Which platforms are in the first wave?",
            Some(widget_multi()),
        );
    }
    if t.contains("when") || t.contains("date") {
        return Script::Ask(ASK_LEAD, "When does the beta open?", Some(widget_date()));
    }
    if t.contains("form") || t.contains("details") {
        return Script::Ask(ASK_LEAD, "Release details", Some(widget_form()));
    }
    if t.contains("name") || t.contains("call it") {
        return Script::Ask(
            ASK_LEAD,
            "What should this release be called?",
            Some(widget_text()),
        );
    }
    if t.contains("ask") || t.contains("launch") || t.contains("deadline") {
        return Script::Ask(ASK_LEAD, ASK_QUESTION, None);
    }
    if t.contains("browse") || t.contains("docs") || t.contains("open ") || t.contains("website") {
        return Script::Browse {
            title: "GPUI docs — ListAlignment",
            url: "https://docs.rs/gpui/latest/gpui/enum.ListAlignment.html",
            summary: BROWSE_SUMMARY,
        };
    }
    if t.contains("fix")
        || t.contains("check")
        || t.contains("run ")
        || t.contains("build")
        || t.contains("test")
        || t.contains("padding")
    {
        return Script::Act {
            tools: ACT_TOOLS,
            diff: Some(("crates/arbos-mac/src/ui/rail.rs", 6, 2)),
            summary: ACT_SUMMARY,
        };
    }
    if t.contains("rules") || t.contains("long") {
        return Script::Reply(REPLY_RULES);
    }
    if t.contains("simple") {
        return Script::Reply(REPLY_SIMPLE);
    }
    if t.contains("list") || t.contains("code") || t.contains("transcript") {
        return Script::Reply(REPLY_CODE);
    }
    Script::Reply(REPLY_DEFAULT)
}

async fn run_turn(
    shared: &Arc<Shared>,
    project_id: &str,
    chat_id: &str,
    text: &str,
    force_reply: bool,
    stop: Arc<AtomicBool>,
) {
    sleep(Duration::from_millis(220)).await;

    if machine_down(shared) {
        wake_machine(shared).await;
    }
    if project_state(shared, project_id).is_asleep() {
        wake_project(shared, project_id).await;
    }

    let (model, effort) = project_model(shared, project_id);
    let id = new_id();
    // The `deciding` status row; it becomes the reply, the first tool card, or the error.
    let lead = with_doc(shared, chat_id, |doc| {
        doc.core.transact(|d| {
            begin_assistant_row(
                d,
                &id,
                "agent",
                &[("model", json!(model)), ("effort", json!(effort))],
                StatusKind::Deciding,
                "Arbos is deciding…",
            )
        })
    })
    .unwrap_or_default();

    sleep(Duration::from_millis(700)).await;
    if stop.load(Ordering::SeqCst) {
        finish_cancelled(shared, chat_id, &id);
        return;
    }

    let script = pick_script(text, force_reply);
    let turn = match &script {
        Script::Reply(_) | Script::Ask(_, _, _) | Script::Fail(_, _) => TurnKind::Reply,
        Script::Act { .. } | Script::Draft { .. } | Script::Flow => TurnKind::Act,
        Script::Browse { .. } => TurnKind::Spawn,
    };
    with_doc(shared, chat_id, |doc| {
        doc.core.set_message_fields(&id, &[("turn", json!(turn))]);
    });

    let finished = match script {
        Script::Flow => {
            // 1. The running row becomes the ack bubble at once.
            with_doc(shared, chat_id, |doc| {
                doc.core.transact(|d| {
                    replace_part(d, &id, &lead, &text_part(FLOW_ACK));
                    set_status(d, &id, MessageStatus::Done);
                });
            });
            // 2. A new running row with no prose: only the working line shows,
            //    its text swapping through the activities.
            let (work_id, work_part) =
                insert_running_row(shared, chat_id, project_id, TurnKind::Act);
            let mut cancelled = false;
            for (activity, ms) in FLOW_ACTIVITIES {
                if stop.load(Ordering::SeqCst) {
                    cancelled = true;
                    break;
                }
                with_doc(shared, chat_id, |doc| {
                    doc.core.set_activity(&work_id, Some(activity));
                });
                sleep(Duration::from_millis(*ms)).await;
            }
            if cancelled {
                finish_cancelled(shared, chat_id, &work_id);
                return;
            }
            // 3. Same row becomes the short conversational result (no milestone bubble).
            with_doc(shared, chat_id, |doc| {
                doc.core.transact(|d| {
                    d.set_activity(&work_id, None);
                    d.set_message_fields(&work_id, &[("turn", json!(TurnKind::Reply))]);
                    replace_part(d, &work_id, &work_part, &empty_text());
                });
            });
            let ok = stream_blocks(shared, chat_id, &work_id, &work_part, FLOW_RESULT, &stop).await;
            if ok {
                with_doc(shared, chat_id, |doc| {
                    set_status(&doc.core, &work_id, MessageStatus::Done);
                });
                mark_unread_if_hidden(shared, chat_id);
            } else {
                finish_cancelled(shared, chat_id, &work_id);
            }
            return;
        }
        Script::Reply(blocks) => {
            with_doc(shared, chat_id, |doc| {
                replace_part(&doc.core, &id, &lead, &empty_text());
            });
            stream_blocks(shared, chat_id, &id, &lead, blocks, &stop).await
        }
        Script::Act {
            tools,
            diff,
            summary,
        } => {
            let mut first = true;
            let mut ok = true;
            for (name, input, out, ms) in tools {
                if stop.load(Ordering::SeqCst) {
                    ok = false;
                    break;
                }
                let call = ToolCall {
                    id: new_id(),
                    name: name.to_string(),
                    input: json!(input),
                };
                let part = Part::Tool {
                    id: new_id(),
                    call: call.clone(),
                    output_summary: String::new(),
                    output_ref: None,
                    is_error: false,
                    status: Some(ToolStatus::Running),
                    duration_ms: None,
                    subagent_name: None,
                };
                let pid = with_doc(shared, chat_id, |doc| {
                    if first {
                        replace_part(&doc.core, &id, &lead, &part);
                        lead.clone()
                    } else {
                        push_part(&doc.core, &id, &part)
                    }
                })
                .unwrap_or_default();
                first = false;
                sleep(Duration::from_millis(*ms)).await;
                with_doc(shared, chat_id, |doc| {
                    doc.core.set_part_fields(
                        &id,
                        &pid,
                        &[
                            ("outputSummary", json!(out)),
                            (
                                "outputRef",
                                json!(format!("blob/{}/{}", project_id, call.id)),
                            ),
                            ("status", json!("ok")),
                            ("durationMs", json!(ms)),
                        ],
                    );
                });
                if *name == "edit_file" {
                    if let Some((path, added, removed)) = diff {
                        with_doc(shared, chat_id, |doc| {
                            push_part(
                                &doc.core,
                                &id,
                                &Part::Diff {
                                    id: new_id(),
                                    path: path.to_string(),
                                    added,
                                    removed,
                                    blob_ref: Some(format!("blob/{project_id}/diff-rail")),
                                },
                            );
                        });
                    }
                }
            }
            if ok {
                let pid = with_doc(shared, chat_id, |doc| {
                    push_part(&doc.core, &id, &empty_text())
                })
                .unwrap_or_default();
                stream_blocks(shared, chat_id, &id, &pid, summary, &stop).await
            } else {
                false
            }
        }
        Script::Browse {
            title,
            url,
            summary,
        } => {
            with_doc(shared, chat_id, |doc| {
                doc.core.transact(|d| {
                    replace_part(
                        d,
                        &id,
                        &lead,
                        &Part::Status {
                            id: new_id(),
                            code: StatusKind::Working,
                            message: "Working in sandbox · starting browser · 00:00".into(),
                        },
                    );
                    push_part(
                        d,
                        &id,
                        &Part::Tool {
                            id: new_id(),
                            call: ToolCall {
                                id: new_id(),
                                name: "spawn_subagent".into(),
                                input: json!("docs-lookup · prime"),
                            },
                            output_summary: "Subagent started".into(),
                            output_ref: None,
                            is_error: false,
                            status: Some(ToolStatus::Ok),
                            duration_ms: Some(300),
                            subagent_name: Some("docs-lookup".into()),
                        },
                    );
                });
            });
            sleep(Duration::from_millis(900)).await;
            let browser = with_doc(shared, chat_id, |doc| {
                push_part(
                    &doc.core,
                    &id,
                    &Part::Browser {
                        id: new_id(),
                        title: title.to_string(),
                        url: url.to_string(),
                        last_action: "navigate".into(),
                        live: true,
                    },
                )
            })
            .unwrap_or_default();
            let actions = ["scroll", "click \"Variants\"", "read page", "save notes"];
            let mut cancelled = false;
            for (i, action) in actions.iter().enumerate() {
                if stop.load(Ordering::SeqCst) {
                    cancelled = true;
                    break;
                }
                sleep(Duration::from_millis(1000)).await;
                let secs = i + 2;
                with_doc(shared, chat_id, |doc| {
                    doc.core.transact(|d| {
                        d.set_part_fields(
                            &id,
                            &lead,
                            &[(
                                "message",
                                json!(format!("Working in sandbox · {} · 00:{:02}", action, secs)),
                            )],
                        );
                        d.set_part_fields(&id, &browser, &[("lastAction", json!(action))]);
                    });
                });
            }
            if cancelled {
                false
            } else {
                with_doc(shared, chat_id, |doc| {
                    doc.core.transact(|d| {
                        d.set_part_fields(&id, &browser, &[("live", json!(false))]);
                        d.set_part_fields(
                            &id,
                            &lead,
                            &[("message", json!("Worked in sandbox · 4 actions · 00:06"))],
                        );
                        push_part(
                            d,
                            &id,
                            &Part::File {
                                id: new_id(),
                                path: "docs/research/gpui.md".into(),
                                lines: 212,
                                blob_ref: Some(format!("blob/{project_id}/gpui-notes")),
                            },
                        );
                    });
                });
                let pid = with_doc(shared, chat_id, |doc| {
                    push_part(&doc.core, &id, &empty_text())
                })
                .unwrap_or_default();
                stream_blocks(shared, chat_id, &id, &pid, summary, &stop).await
            }
        }
        Script::Ask(lead_text, question, widget) => {
            with_doc(shared, chat_id, |doc| {
                replace_part(&doc.core, &id, &lead, &empty_text());
            });
            let done = stream_blocks(shared, chat_id, &id, &lead, &[lead_text], &stop).await;
            if done {
                let widget = widget.and_then(|w| serde_json::to_value(w).ok());
                with_doc(shared, chat_id, |doc| {
                    doc.core.transact(|d| {
                        d.add_question_part(&id, &new_id(), question, widget.as_ref());
                        // Legacy status row, as agentd still writes it next to the question part.
                        d.add_status_part(&id, "ask_user", question);
                    });
                });
            }
            done
        }
        Script::Draft {
            lead: lead_text,
            channel,
            to,
            subject,
            body,
        } => {
            with_doc(shared, chat_id, |doc| {
                replace_part(&doc.core, &id, &lead, &empty_text());
            });
            let done = stream_blocks(shared, chat_id, &id, &lead, &[lead_text], &stop).await;
            if done {
                let request = draft_request(channel, to, subject, body);
                with_doc(shared, chat_id, |doc| {
                    doc.core.transact(|d| {
                        d.add_approval_part(
                            &id,
                            &new_id(),
                            &request,
                            now_ms() as i64 + APPROVAL_TTL_MS,
                        );
                        d.add_status_part(&id, "waiting_approval", &request.title);
                        d.set_message_fields(&id, &[("waitingApproval", json!(true))]);
                    });
                });
            }
            done
        }
        Script::Fail(code, message) => {
            sleep(Duration::from_millis(900)).await;
            with_doc(shared, chat_id, |doc| {
                doc.core.transact(|d| {
                    replace_part(
                        d,
                        &id,
                        &lead,
                        &Part::Error {
                            id: new_id(),
                            code: code.to_string(),
                            message: message.to_string(),
                        },
                    );
                    set_status(d, &id, MessageStatus::Error);
                });
            });
            mark_unread_if_hidden(shared, chat_id);
            return;
        }
    };

    if finished {
        with_doc(shared, chat_id, |doc| {
            set_status(&doc.core, &id, MessageStatus::Done);
        });
        mark_unread_if_hidden(shared, chat_id);
        group_replies(shared, chat_id, text, &stop).await;
    } else {
        finish_cancelled(shared, chat_id, &id);
    }
}

fn opt(id: &str, label: &str) -> ChoiceOption {
    ChoiceOption {
        id: id.into(),
        label: label.into(),
    }
}

fn widget_choice() -> Widget {
    Widget::Choice {
        options: vec![
            opt("main", "main"),
            opt("release-1.2", "release-1.2"),
            opt("nightly", "nightly"),
        ],
    }
}

fn widget_multi() -> Widget {
    Widget::MultiSelect {
        options: vec![
            opt("macos", "macOS"),
            opt("ios", "iOS"),
            opt("web", "Web"),
            opt("windows", "Windows"),
        ],
    }
}

fn widget_text() -> Widget {
    Widget::Text {
        placeholder: Some("e.g. Juniper".into()),
        multiline: false,
    }
}

fn widget_date() -> Widget {
    Widget::Date {
        min: Some("2026-10-15".into()),
        max: Some("2027-03-31".into()),
    }
}

fn widget_form() -> Widget {
    let field = |id: &str, label: &str, kind: FieldKind, required: bool| FormField {
        id: id.into(),
        label: label.into(),
        kind,
        options: Vec::new(),
        required,
    };
    Widget::Form {
        fields: vec![
            field("version", "Version", FieldKind::Text, true),
            FormField {
                options: vec![opt("beta", "Beta"), opt("stable", "Stable")],
                ..field("channel", "Channel", FieldKind::Choice, true)
            },
            field("seats", "Seats", FieldKind::Number, false),
            field("freeze", "Code freeze", FieldKind::Date, false),
            field("notify", "Email testers", FieldKind::Bool, false),
        ],
    }
}

/// A `question` part as agentd writes it; `answer` makes it an answered card.
fn question_part(
    question: &str,
    widget: Option<Widget>,
    answer: Option<serde_json::Value>,
) -> Part {
    let answered = answer.is_some();
    Part::Question {
        id: new_id(),
        question_id: new_id(),
        question: question.into(),
        widget,
        answer,
        state: if answered {
            QuestionState::Answered
        } else {
            QuestionState::Open
        },
        answered_at: answered.then(now_ms),
    }
}

const GROUP_REPLIES: &[&str] = &[
    "Heard. On my side that is done by end of day; I will post the link here.",
    "Agreed — I can take that. One question for the group: do we announce before or after the build is tagged?",
];

/// F6 demo: in a group chat the other participants answer after the host, one by one, as
/// `project` messages (what the relay's `group_message` fan-out produces in real use).
async fn group_replies(shared: &Arc<Shared>, chat_id: &str, _text: &str, stop: &AtomicBool) {
    let participants: Vec<Id> = {
        let chats = shared.chats.lock().unwrap();
        match chats.iter().find(|c| c.id == chat_id) {
            Some(c) if c.is_group() => c.participants.clone(),
            _ => return,
        }
    };
    let host = project_of(shared, chat_id);
    let others: Vec<Project> = {
        let projects = shared.projects.lock().unwrap();
        participants
            .iter()
            .filter(|id| **id != host)
            .filter_map(|id| projects.iter().find(|p| &p.id == id).cloned())
            .collect()
    };
    for (i, p) in others.iter().enumerate() {
        sleep(Duration::from_millis(1400)).await;
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let line = GROUP_REPLIES[i % GROUP_REPLIES.len()];
        with_doc(shared, chat_id, |doc| {
            doc.core
                .transact(|d| write_project_line(d, p, line, now_ms()));
        });
        mark_unread_if_hidden(shared, chat_id);
    }
}

/// A fresh assistant row in `streaming`, no prose yet: only the working line shows.
/// Returns `(message id, status part id)`.
fn insert_running_row(
    shared: &Arc<Shared>,
    chat_id: &str,
    project_id: &str,
    turn: TurnKind,
) -> (Id, String) {
    let (model, effort) = project_model(shared, project_id);
    let id = new_id();
    let part = with_doc(shared, chat_id, |doc| {
        doc.core.transact(|d| {
            begin_assistant_row(
                d,
                &id,
                "agent",
                &[
                    ("turn", json!(turn)),
                    ("model", json!(model)),
                    ("effort", json!(effort)),
                ],
                StatusKind::Working,
                "",
            )
        })
    })
    .unwrap_or_default();
    (id, part)
}

fn empty_text() -> Part {
    Part::Text {
        id: new_id(),
        text: String::new(),
    }
}

/// The draft-and-approve request for one outgoing message (`arbos_core::approval`).
fn draft_request(channel: &str, to: &str, subject: &str, body: &str) -> ApprovalRequest {
    ApprovalRequest {
        title: format!("Send {channel} to {to}: {subject}"),
        summary: body
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(160)
            .collect(),
        risk: Risk::Send,
        payload: json!({ "kind": "draft_message", "channel": channel, "to": to, "subject": subject, "body": body }),
    }
}

/// One seeded `approval` part in the shape `arbos_core::approval` owns.
fn approval_card(
    approval_id: &str,
    channel: &str,
    to: &str,
    subject: &str,
    body: &str,
    state: ApprovalState,
    decided_at: Option<u64>,
) -> Part {
    let now = now_ms();
    let request = draft_request(channel, to, subject, body);
    Part::Approval {
        id: new_id(),
        approval_id: approval_id.to_string(),
        title: request.title,
        summary: request.summary,
        risk: ApprovalRisk::Send,
        payload: request.payload,
        state,
        expires_at: Some(decided_at.unwrap_or(now) + APPROVAL_TTL_MS as u64),
        decided_at,
        decided_by: decided_at.map(|_| DEVICE.to_string()),
        reason: None,
        grant: None,
    }
}

/// S18 in the fixture: flip the card, mirror the command, then let the "agent" report
/// the outcome as a follow-up turn (what the box's `[Arbos]` notice turn does).
fn decide_approval(
    shared: &Arc<Shared>,
    chat_id: Id,
    approval_id: String,
    state: ApprovalState,
    reason: Option<String>,
) {
    let (kind, core_state) = match state {
        ApprovalState::Approved => ("approve", CoreApprovalState::Approved),
        ApprovalState::Rejected => ("reject", CoreApprovalState::Rejected),
        ApprovalState::Pending | ApprovalState::Expired => return,
    };
    let reason = reason.filter(|r| !r.trim().is_empty());
    let found = with_doc(shared, &chat_id, |doc| {
        let mut payload = json!({ "approvalId": approval_id });
        if let Some(r) = reason.as_deref() {
            payload["reason"] = json!(r);
        }
        let hit = doc.read_all().into_iter().rev().find_map(|m| {
            m.parts.iter().find_map(|p| match p {
                Part::Approval {
                    approval_id: a,
                    state: ApprovalState::Pending,
                    title,
                    payload,
                    ..
                } if *a == approval_id => Some((m.id.clone(), title.clone(), payload.clone())),
                _ => None,
            })
        });
        doc.core.transact(|d| {
            d.push_command(kind, payload, DEVICE);
            if let Some((msg_id, _, _)) = &hit {
                d.set_approval_state(&approval_id, core_state, Some(DEVICE), reason.as_deref());
                d.set_message_fields(msg_id, &[("waitingApproval", json!(false))]);
            }
        });
        hit
    })
    .flatten();
    let Some((_, title, payload)) = found else {
        return;
    };
    let to = payload
        .get("to")
        .and_then(|v| v.as_str())
        .unwrap_or("them")
        .to_string();
    let channel = payload
        .get("channel")
        .and_then(|v| v.as_str())
        .unwrap_or("message")
        .to_string();
    let blocks: Vec<String> = match state {
        ApprovalState::Approved => vec![format!(
            "Approved. I recorded the {channel} to {to} in this box's outbox; no {channel} connector is configured yet, so it was not delivered externally."
        )],
        ApprovalState::Rejected => vec![format!(
            "Understood, nothing was sent{}. Tell me what to change in \"{title}\" and I will redraft it.",
            reason
                .as_deref()
                .map(|r| format!(" ({r})"))
                .unwrap_or_default()
        )],
        ApprovalState::Pending | ApprovalState::Expired => return,
    };
    let project_id = project_of(shared, &chat_id);
    let shared = shared.clone();
    tokio::spawn(async move {
        run_outcome_turn(&shared, &project_id, &chat_id, &approval_id, blocks).await;
    });
}

async fn run_outcome_turn(
    shared: &Arc<Shared>,
    project_id: &str,
    chat_id: &str,
    approval_id: &str,
    blocks: Vec<String>,
) {
    sleep(Duration::from_millis(400)).await;
    let (model, effort) = project_model(shared, project_id);
    let id = new_id();
    let text = with_doc(shared, chat_id, |doc| {
        doc.core.transact(|d| {
            let _ = d.begin_message(
                &id,
                "assistant",
                "agent",
                DEVICE,
                CoreStatus::Streaming,
                &[
                    ("turn", json!(TurnKind::Reply)),
                    ("decision", json!("reply")),
                    ("model", json!(model)),
                    ("effort", json!(effort)),
                ],
            );
            let text = push_part(d, &id, &empty_text());
            d.add_status_part(
                &id,
                "working",
                &format!("Following up on approval {approval_id}"),
            );
            text
        })
    })
    .unwrap_or_default();
    let refs: Vec<&str> = blocks.iter().map(String::as_str).collect();
    let stop = AtomicBool::new(false);
    stream_blocks(shared, chat_id, &id, &text, &refs, &stop).await;
    with_doc(shared, chat_id, |doc| {
        set_status(&doc.core, &id, MessageStatus::Done);
    });
    mark_unread_if_hidden(shared, chat_id);
}

fn finish_cancelled(shared: &Arc<Shared>, chat_id: &str, msg_id: &str) {
    with_doc(shared, chat_id, |doc| {
        doc.core.transact(|d| {
            d.add_status_part(msg_id, "note", "Stopped by you");
            set_status(d, msg_id, MessageStatus::Cancelled);
        });
    });
}

fn mark_unread_if_hidden(shared: &Arc<Shared>, chat_id: &str) {
    let open = shared.open_chat.lock().unwrap().clone();
    if open.as_deref() == Some(chat_id) {
        return;
    }
    {
        let mut chats = shared.chats.lock().unwrap();
        if let Some(c) = chats.iter_mut().find(|c| c.id == chat_id) {
            c.unread = true;
            c.unread_count = c.unread_count.max(1);
            c.last_message_at = now_ms();
        }
    }
    emit_chats(shared);
}

/// Stream markdown blocks into one text part in 120 ms frames of a few words.
/// Returns false when stopped.
async fn stream_blocks(
    shared: &Arc<Shared>,
    chat_id: &str,
    msg_id: &str,
    part_id: &str,
    blocks: &[&str],
    stop: &AtomicBool,
) -> bool {
    let mut full = String::new();
    for (i, block) in blocks.iter().enumerate() {
        if i > 0 {
            full.push_str("\n\n");
        }
        full.push_str(block);
    }
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut words = 0;
    for piece in full.split_inclusive(' ') {
        current.push_str(piece);
        words += 1;
        if words >= 3 {
            chunks.push(std::mem::take(&mut current));
            words = 0;
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    for chunk in chunks {
        if stop.load(Ordering::SeqCst) {
            return false;
        }
        with_doc(shared, chat_id, |doc| {
            doc.core
                .transact(|d| d.append_text(msg_id, part_id, &chunk));
        });
        sleep(Duration::from_millis(STREAM_FRAME_MS)).await;
    }
    true
}

/// The signed-in fixture user as a notice names them.
const ME_NAME: &str = "Jacob Steeves";

/// "Jacob Steeves added A, B and C" — empty when nobody was new.
fn added_line_text(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => format!("{ME_NAME} added {one}"),
        [a, b] => format!("{ME_NAME} added {a} and {b}"),
        [init @ .., last] => format!("{ME_NAME} added {} and {last}", init.join(", ")),
    }
}

/// What the relay writes on a membership change (#1349): one centred system line in
/// the group's chat, as device `relay`.
fn membership_notice(shared: &Arc<Shared>, chat_id: &str, text: &str) {
    if text.is_empty() {
        return;
    }
    with_doc(shared, chat_id, |doc| {
        doc.core.transact(|d| {
            let _ = d.append_system_line(&new_id(), text, "relay", None);
        });
    });
}

fn emit_shell_grants(shared: &Arc<Shared>, project_id: &str) {
    // The relay answers someone else's agent (Sam's Studio, Sami's TestAgent) with 403 `not_owner`.
    if [SHARED_STUDIO, PROJECT_TESTAGENT].contains(&project_id) {
        let err = super::relay::RelayError::from_body(
            reqwest::StatusCode::FORBIDDEN,
            r#"{"error":{"type":"not_owner","message":"not your project"}}"#,
        );
        if let Some(event) = super::relay::shell_grants_failed(project_id, &err.into()) {
            let _ = shared.events.send(event);
        }
        return;
    }
    let items = shared
        .shell_grants
        .lock()
        .unwrap()
        .get(project_id)
        .cloned()
        .unwrap_or_default();
    let _ = shared.events.send(SyncEvent::ShellGrants {
        project_id: project_id.to_string(),
        items,
    });
}

/// #1544: the first agent has one standing grant (so the Settings row and its switch show).
fn seed_shell_grants() -> HashMap<Id, Vec<crate::model::ShellGrant>> {
    let mut m = HashMap::new();
    m.insert(
        PROJECT_ARBOS.to_string(),
        vec![crate::model::ShellGrant {
            id: "grant-scratch".into(),
            rule_class: "file-delete".into(),
            path_class: "scratch".into(),
            chat_id: None,
            granted_by: "dev-mac".into(),
            created_at: now_ms() - 3 * 60 * 60 * 1000,
            label: "File deletes in /tmp".into(),
            detail: "scratch only: /tmp and /work/tmp".into(),
        }],
    );
    m
}

fn with_doc<R>(shared: &Arc<Shared>, chat_id: &str, f: impl FnOnce(&ChatDoc) -> R) -> Option<R> {
    let docs = shared.docs.lock().unwrap();
    docs.get(chat_id).map(f)
}

// ---------------------------------------------------------------------------
// Doc writes. Every chat-doc write goes through the `arbos_core::doc::ChatDoc`
// writers inside `transact` (one commit per step), the same rows the box and the
// relay client write. Rows and parts are addressed by id, never by list index.
// ---------------------------------------------------------------------------

fn core_status(status: MessageStatus) -> CoreStatus {
    match status {
        MessageStatus::Queued => CoreStatus::Queued,
        MessageStatus::Streaming => CoreStatus::Streaming,
        MessageStatus::Done => CoreStatus::Done,
        MessageStatus::Error => CoreStatus::Error,
        MessageStatus::Cancelled => CoreStatus::Cancelled,
    }
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
        Role::Project => "project",
    }
}

fn is_terminal(status: MessageStatus) -> bool {
    match status {
        MessageStatus::Done | MessageStatus::Error | MessageStatus::Cancelled => true,
        MessageStatus::Queued | MessageStatus::Streaming => false,
    }
}

fn field_refs(fields: &[(String, Value)]) -> Vec<(&str, Value)> {
    fields
        .iter()
        .map(|(k, v)| (k.as_str(), v.clone()))
        .collect()
}

/// Top-level fields of `msg` the row writers do not set themselves, in wire
/// (camelCase) form. Skips the client-only placeholders core never stores
/// (`chatId`, `pending`, `hiddenParts`) and `null` / `false` / empty values. A
/// finished row without `completedAt` gets its `createdAt`, so a seeded row never
/// reads as finished "now".
fn message_extra(msg: &Message) -> Vec<(String, Value)> {
    let Ok(Value::Object(fields)) = serde_json::to_value(msg) else {
        return Vec::new();
    };
    let mut out: Vec<(String, Value)> = fields
        .into_iter()
        .filter(|(k, v)| {
            let placeholder = matches!(
                k.as_str(),
                "id" | "role"
                    | "sender"
                    | "status"
                    | "parts"
                    | "hiddenParts"
                    | "reactions"
                    | "chatId"
                    | "pending"
            );
            let empty = v.is_null()
                || *v == Value::Bool(false)
                || v.as_array().is_some_and(|a| a.is_empty());
            !placeholder && !empty
        })
        .collect();
    if is_terminal(msg.status) && msg.completed_at.is_none() {
        out.push(("completedAt".into(), json!(msg.created_at)));
    }
    out
}

/// Append `parts` to `msg_id`; `hidden` marks them drawer-only. Returns their ids.
fn write_parts(core: &CoreDoc, msg_id: &str, parts: &[Part], hidden: bool) -> Vec<String> {
    parts
        .iter()
        .filter_map(|part| {
            let mut v = serde_json::to_value(part).unwrap_or(json!({}));
            if let (true, Some(obj)) = (hidden, v.as_object_mut()) {
                obj.insert("hidden".into(), Value::Bool(true));
            }
            core.add_part_from_json(msg_id, &v)
        })
        .collect()
}

/// One part, appended to `msg_id`. Returns its id (empty when the row is gone).
fn push_part(core: &CoreDoc, msg_id: &str, part: &Part) -> String {
    write_parts(core, msg_id, std::slice::from_ref(part), false)
        .pop()
        .unwrap_or_default()
}

/// Overwrite part `part_id` in place with `part`'s fields (the id stays), the way a
/// `deciding` status row becomes the reply, a tool card, or an error.
fn replace_part(core: &CoreDoc, msg_id: &str, part_id: &str, part: &Part) {
    let Ok(Value::Object(fields)) = serde_json::to_value(part) else {
        return;
    };
    let fields: Vec<(&str, Value)> = fields
        .iter()
        .filter(|(k, _)| k.as_str() != "id")
        .map(|(k, v)| (k.as_str(), v.clone()))
        .collect();
    core.set_part_fields(msg_id, part_id, &fields);
}

/// `status`, plus `completedAt` when it is terminal.
fn set_status(core: &CoreDoc, msg_id: &str, status: MessageStatus) {
    if is_terminal(status) {
        core.finish_message(msg_id, core_status(status));
    } else {
        core.set_message_fields(msg_id, &[("status", json!(core_status(status).as_str()))]);
    }
}

fn user_attachments(attachments: &[Attachment]) -> Vec<UserAttachment> {
    attachments
        .iter()
        .map(|a| UserAttachment {
            name: a.name.clone(),
            mime: a.mime.clone(),
            kind: a
                .kind
                .and_then(|k| serde_json::to_value(k).ok())
                .and_then(|v| v.as_str().map(str::to_owned)),
            blob_ref: a.blob_ref.clone(),
            url: a.url.clone(),
            source: a
                .source
                .as_ref()
                .map(|s| arbos_core::doc::AttachmentSource {
                    provider: s.provider.clone(),
                    id: s.id.clone(),
                    page_url: s.page_url.clone(),
                }),
        })
        .collect()
}

/// Size and caption the client user writer does not carry: rewrite those
/// attachment parts (they lead the row, in order) with the full object.
fn fill_attachment_details(core: &CoreDoc, msg_id: &str, attachments: &[Attachment]) {
    for (pix, a) in attachments.iter().enumerate() {
        if a.bytes.is_none() && a.title.is_none() {
            continue;
        }
        let (Some(part_id), Ok(full)) = (core.part_id_at(msg_id, pix), serde_json::to_value(a))
        else {
            continue;
        };
        core.set_part_fields(msg_id, &part_id, &[("attachment", full)]);
    }
}

/// A user row in the shape the clients write: attachments, then one text part.
fn user_row_shape(msg: &Message) -> Option<(Vec<Attachment>, &str)> {
    if msg.role != Role::User {
        return None;
    }
    let (last, rest) = msg.parts.split_last()?;
    let Part::Text { text, .. } = last else {
        return None;
    };
    let attachments = rest
        .iter()
        .map(|p| match p {
            Part::Attachment { attachment, .. } => Some(attachment.clone()),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    Some((attachments, text.as_str()))
}

/// Write a whole `Message` (seed rows, people-chat lines). User rows go through the
/// client user writer; everything else is `begin_message` plus its parts. Call
/// inside `transact`.
fn write_message(core: &CoreDoc, msg: &Message) {
    let extra = message_extra(msg);
    if let Some((attachments, text)) = user_row_shape(msg) {
        let opts = UserMessageOpts {
            attachments: user_attachments(&attachments),
            mentions: msg.mentions.clone(),
            reply_to: msg.reply_to.clone(),
            sender: Some(msg.sender.clone()),
            sender_handle: msg.sender_handle.clone(),
            sender_name: msg.sender_name.clone(),
            sender_avatar: msg.sender_avatar.clone(),
            answers_question_id: None,
            tz: None,
        };
        let _ = core.append_user_message(&msg.id, text, DEVICE, &opts);
        fill_attachment_details(core, &msg.id, &attachments);
        let mut rest: Vec<(String, Value)> = extra
            .into_iter()
            .filter(|(k, _)| {
                !matches!(
                    k.as_str(),
                    "mentions" | "replyTo" | "senderHandle" | "senderName" | "senderAvatar"
                )
            })
            .collect();
        if msg.status != MessageStatus::Done {
            rest.push(("status".into(), json!(core_status(msg.status).as_str())));
        }
        core.set_message_fields(&msg.id, &field_refs(&rest));
    } else {
        let _ = core.begin_message(
            &msg.id,
            role_str(msg.role),
            &msg.sender,
            DEVICE,
            core_status(msg.status),
            &field_refs(&extra),
        );
        write_parts(core, &msg.id, &msg.parts, false);
        write_parts(core, &msg.id, &msg.hidden_parts, true);
    }
    for (emoji, senders) in &msg.reactions {
        for (sender, on) in senders {
            if *on {
                let _ = core.set_reaction(&msg.id, emoji, sender, true);
            }
        }
    }
}

/// [`write_message`] as one commit.
fn insert_message_via_core(doc: &ChatDoc, msg: &Message) {
    doc.core.transact(|d| write_message(d, msg));
}

/// Another Project's finished line in a group (`role: project`, F6).
fn write_project_line(core: &CoreDoc, project: &Project, text: &str, t: u64) {
    core.append_project_message(
        &new_id(),
        text,
        DEVICE,
        &format!("project:{}", project.id),
        &[
            ("senderName", json!(project.name)),
            ("senderIcon", json!(project.icon)),
            ("createdAt", json!(t)),
            ("completedAt", json!(t)),
        ],
        false,
    );
}

/// Every `queued` row becomes `done` (the box picked them up after a wake).
fn release_queued(doc: &ChatDoc) {
    let queued: Vec<Id> = doc
        .read_all()
        .into_iter()
        .filter(|m| m.status == MessageStatus::Queued)
        .map(|m| m.id)
        .collect();
    if queued.is_empty() {
        return;
    }
    doc.core.transact(|d| {
        for id in &queued {
            set_status(d, id, MessageStatus::Done);
        }
    });
}

/// A fresh assistant row in `streaming` with `fields` (turn, model, …) and one
/// `status` part. Returns the status part's id.
fn begin_assistant_row(
    core: &CoreDoc,
    msg_id: &str,
    sender: &str,
    fields: &[(&str, Value)],
    status: StatusKind,
    status_text: &str,
) -> String {
    let _ = core.begin_message(
        msg_id,
        "assistant",
        sender,
        DEVICE,
        CoreStatus::Streaming,
        fields,
    );
    let code = serde_json::to_value(status)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    core.add_status_part(msg_id, &code, status_text)
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Seed data: the mock conversation plus one chat per rendering state.
// ---------------------------------------------------------------------------

fn seed_projects() -> Vec<Project> {
    vec![
        Project {
            id: PROJECT_AFFINE.into(),
            name: "Affine".into(),
            icon: "△".into(),
            color: "#3fa7d6".into(),
            state: BoxState::HibernatedLight,
            state_detail: None,
            wake_progress: None,
            model: MODELS[1].into(),
            effort: Effort::Medium,
            spend_today_usd: 0.42,
            spend_cap_usd: 25.0,
            kind: SpaceKind::Project,
            people: Vec::new(),
            agents: Vec::new(),
            pending: false,
            shared: false,
            owner_handle: None,
            avatar: None,
            avatar_color: Some("teal".into()),
            description: None,
            owner_person: None,
            machine_info: None,
            outside: None,
            muted: false,
            silenced: false,
            silenced_until: None,
            archived_at: None,
            primary_chat: None,
            voice: None,
            vision_enabled: true,
            avatar_color_index: None,
            avatar_color_override: None,
            avatar_leaf: None,
            avatar_leaf_color: None,
            image_url: None,
            avatar_version: 0,
            message_grants: Vec::new(),
            blocked_senders: Vec::new(),
            my_role: None,
        },
        Project {
            id: PROJECT_COMMS.into(),
            name: "Comms".into(),
            icon: "Ψ".into(),
            color: "#e0a458".into(),
            state: BoxState::StoppedByUser,
            state_detail: None,
            wake_progress: None,
            model: MODELS[0].into(),
            effort: Effort::Low,
            spend_today_usd: 1.10,
            spend_cap_usd: 25.0,
            kind: SpaceKind::Project,
            people: Vec::new(),
            agents: Vec::new(),
            pending: false,
            shared: false,
            owner_handle: None,
            avatar: None,
            avatar_color: Some("violet".into()),
            description: None,
            owner_person: None,
            machine_info: None,
            outside: None,
            muted: false,
            silenced: false,
            silenced_until: None,
            archived_at: None,
            primary_chat: None,
            voice: None,
            vision_enabled: true,
            avatar_color_index: None,
            avatar_color_override: None,
            avatar_leaf: None,
            avatar_leaf_color: None,
            image_url: None,
            avatar_version: 0,
            message_grants: Vec::new(),
            blocked_senders: Vec::new(),
            my_role: None,
        },
        Project {
            id: PROJECT_ARBOS.into(),
            name: "Arbos".into(),
            icon: "Ω".into(),
            color: "#8b7cf6".into(),
            state: BoxState::Awake,
            state_detail: None,
            wake_progress: None,
            model: MODELS[0].into(),
            effort: Effort::High,
            spend_today_usd: 3.87,
            spend_cap_usd: 25.0,
            kind: SpaceKind::Project,
            people: Vec::new(),
            agents: Vec::new(),
            pending: false,
            shared: false,
            owner_handle: None,
            avatar: None,
            avatar_color: Some("amber".into()),
            description: None,
            owner_person: None,
            machine_info: None,
            outside: None,
            muted: false,
            silenced: false,
            silenced_until: None,
            archived_at: None,
            primary_chat: None,
            voice: None,
            vision_enabled: true,
            avatar_color_index: None,
            avatar_color_override: None,
            avatar_leaf: None,
            avatar_leaf_color: None,
            image_url: None,
            avatar_version: 0,
            message_grants: Vec::new(),
            blocked_senders: Vec::new(),
            my_role: None,
        },
    ]
}

fn seed_skills() -> Vec<Skill> {
    let t = now_ms() as i64;
    let skill = |slug: &str,
                 name: &str,
                 description: &str,
                 tags: &[&str],
                 when_to_use: &str,
                 body: &str,
                 origin: Option<&str>,
                 shared: bool,
                 ago_h: i64| Skill {
        id: new_id(),
        slug: slug.into(),
        name: name.into(),
        description: description.into(),
        tags: tags.iter().map(|s| s.to_string()).collect(),
        when_to_use: when_to_use.into(),
        body: body.into(),
        origin_project_id: origin.map(str::to_string),
        shared,
        version: 1,
        created_at: t - ago_h * 3_600_000,
        updated_at: t - ago_h * 3_600_000,
    };
    vec![
        skill(
            "gpui-list-alignment",
            "GPUI bottom-aligned lists",
            "How to build a chat-style list that scrolls from the bottom in GPUI.",
            &["gpui", "rust", "ui"],
            "Any transcript or log view that must stick to its newest row.",
            "## Steps\n\n1. `ListState::new(0, ListAlignment::Bottom, px(320.0))`.\n2. Key rows by `{msg}#{part}.{block}` so streaming appends re-splice only the changed tail.\n3. Use `ListSizingBehavior::Auto` so the list fills the column.\n\n## Pitfalls\n\n- Never rebuild the whole list on a text append; use `splice`.\n",
            Some(PROJECT_ARBOS),
            true,
            30,
        ),
        skill(
            "weekly-update",
            "Weekly team update",
            "Template and checklist for the Monday update mail.",
            &["comms", "writing"],
            "When asked to draft, send, or summarise the weekly update.",
            "## Shape\n\n- Three bullets: shipped, in flight, blocked.\n- One line per project, link to the thread.\n- Under 200 words; no adjectives.\n\n## Send\n\nRead `updates/<date>.md`, fill the template, ask before sending to more than ten people.\n",
            Some(PROJECT_COMMS),
            false,
            52,
        ),
        skill(
            "release-checklist",
            "Mac beta release checklist",
            "Everything to check before tagging a Mac beta build.",
            &["release", "mac", "process"],
            "Before any tag, TestFlight upload, or launch-plan question.",
            "## Before tagging\n\n1. `cargo fmt --all --check` and `cargo clippy --workspace -- -D warnings` are clean.\n2. Fixture and relay sources both start and pair.\n3. Changelog entry written.\n\n## After\n\n- Notarise, upload, post the build link in the launch chat.\n",
            None,
            true,
            6,
        ),
    ]
}

/// The demo's call voices (`GET /voices` shape).
fn fixture_voices() -> Vec<Voice> {
    let v = |id: &str, name: &str, description: &str| Voice {
        id: id.into(),
        name: name.into(),
        description: Some(description.into()),
        sample_url: format!("fixture://voices/{id}.wav"),
    };
    vec![
        v(
            "ara",
            "Ara",
            "Warm and clear, a touch of gravel. The current default.",
        ),
        v("rex", "Rex", "Low, steady, unhurried."),
        v("sol", "Sol", "Bright and quick; good for short answers."),
        v("ivy", "Ivy", "Soft, close to the mic."),
        v("ode", "Ode", "Even and neutral, reads long text well."),
    ]
}

/// A 0.6 s two-note wav (48 kHz mono, 16-bit) so Play has something to play.
fn fixture_sample_wav() -> Vec<u8> {
    let sr = 48_000u32;
    let n = (sr as f32 * 0.6) as usize;
    let mut pcm: Vec<i16> = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / sr as f32;
        let f = if t < 0.3 { 440.0 } else { 554.37 };
        let env = (1.0 - (t % 0.3) / 0.3).max(0.0);
        pcm.push(((t * f * std::f32::consts::TAU).sin() * 0.3 * env * i16::MAX as f32) as i16);
    }
    let data_len = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&sr.to_le_bytes());
    out.extend_from_slice(&(sr * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

fn seed_chats() -> Vec<Chat> {
    let t = now_ms();
    let chat = |id: &str, project: &str, title: &str, pinned: bool, ago_min: u64| Chat {
        id: id.into(),
        project_id: project.into(),
        title: title.into(),
        is_pinned: pinned,
        is_system: false,
        kind: ChatKind::Chat,
        participants: Vec::new(),
        unread: false,
        unread_count: 0,
        mention_count: 0,
        last_message_at: t.saturating_sub(ago_min * 60_000),
        archived_at: None,
    };
    vec![
        chat(CHAT_ARBOS_MAIN, PROJECT_ARBOS, "Arbos", true, 3),
        chat(
            CHAT_ARBOS_RESEARCH,
            PROJECT_ARBOS,
            "Interface research",
            false,
            40,
        ),
        // Two archived chats, so the tab strip's "+" dropdown has previous
        // chats to bring back.
        Chat {
            archived_at: Some(t.saturating_sub(80 * 60_000)),
            ..chat(
                CHAT_ARBOS_LAUNCH,
                PROJECT_ARBOS,
                "Launch planning",
                false,
                95,
            )
        },
        chat(CHAT_ARBOS_WIDGETS, PROJECT_ARBOS, "Beta release", false, 12),
        Chat {
            archived_at: Some(t.saturating_sub(10 * 60_000)),
            ..chat(
                CHAT_ARBOS_DRAFTS,
                PROJECT_ARBOS,
                "Investor update",
                false,
                12,
            )
        },
        chat(CHAT_ARBOS_MEDIA, PROJECT_ARBOS, "Deliverables", false, 18),
        chat(CHAT_ARBOS_VOICE, PROJECT_ARBOS, "Voice memo", false, 6),
        Chat {
            kind: ChatKind::Group,
            participants: vec![
                PROJECT_ARBOS.into(),
                PROJECT_AFFINE.into(),
                PROJECT_COMMS.into(),
            ],
            ..chat(CHAT_ARBOS_GROUP, PROJECT_ARBOS, "Launch sync", false, 12)
        },
        chat(CHAT_AFFINE_MAIN, PROJECT_AFFINE, "Affine", true, 60 * 26),
        chat(CHAT_COMMS_MAIN, PROJECT_COMMS, "Comms", true, 60 * 5),
    ]
}

pub const ROUTINE_STANDUP: &str = "01J9ROUTINE0STANDUP0000000";
pub const ROUTINE_PRS: &str = "01J9ROUTINE0PRS00000000000";
pub const ROUTINE_DEPLOY: &str = "01J9ROUTINE0DEPLOY00000000";

fn seed_routines() -> Vec<Routine> {
    let t = now_ms() as i64;
    let hour = 3_600_000;
    vec![
        Routine {
            id: ROUTINE_STANDUP.into(),
            project_id: PROJECT_ARBOS.into(),
            chat_id: CHAT_ARBOS_LAUNCH.into(),
            name: "Morning stand-up".into(),
            prompt: "Summarise what changed in the launch plan since yesterday and list today's three most important items.".into(),
            trigger: RoutineTrigger::Cron {
                expr: "0 9 * * 1-5".into(),
                tz: "Europe/Berlin".into(),
            },
            enabled: true,
            created_at: t - 9 * 24 * hour,
            last_run_at: Some(t - 5 * hour),
            next_run_at: Some(t + 19 * hour),
            fire_url: None,
        },
        Routine {
            id: ROUTINE_PRS.into(),
            project_id: PROJECT_ARBOS.into(),
            chat_id: CHAT_ARBOS_RESEARCH.into(),
            name: "Open PR digest".into(),
            prompt: "Check the open pull requests and tell me which ones are waiting on me.".into(),
            trigger: RoutineTrigger::Cron {
                expr: "0 * * * *".into(),
                tz: "UTC".into(),
            },
            enabled: false,
            created_at: t - 3 * 24 * hour,
            last_run_at: Some(t - 26 * hour),
            next_run_at: None,
            fire_url: None,
        },
        Routine {
            id: ROUTINE_DEPLOY.into(),
            project_id: PROJECT_ARBOS.into(),
            chat_id: CHAT_ARBOS_MAIN.into(),
            name: "Deploy finished".into(),
            prompt: "A deploy webhook fired. Read the trigger context and tell me whether anything needs attention.".into(),
            trigger: RoutineTrigger::Webhook {
                secret: "fixture".into(),
            },
            enabled: true,
            created_at: t - 24 * hour,
            last_run_at: Some(t - 40 * 60_000),
            next_run_at: None,
            fire_url: Some(format!(
                "http://127.0.0.1:7700/routines/{ROUTINE_DEPLOY}/fire?secret=…"
            )),
        },
    ]
}

fn seed_routine_runs() -> HashMap<Id, Vec<RoutineRun>> {
    let t = now_ms() as i64;
    let hour = 3_600_000;
    let run = |routine: &str, ago_ms: i64, dur_ms: i64, status: RoutineRunStatus, note: &str| {
        RoutineRun {
            id: new_id(),
            routine_id: routine.into(),
            started_at: t - ago_ms,
            finished_at: matches!(status, RoutineRunStatus::Done | RoutineRunStatus::Failed)
                .then_some(t - ago_ms + dur_ms),
            status,
            message_id: Some(new_id()),
            note: Some(note.into()),
        }
    };
    let mut out = HashMap::new();
    out.insert(
        ROUTINE_STANDUP.to_string(),
        vec![
            run(ROUTINE_STANDUP, 5 * hour, 41_000, RoutineRunStatus::Done, "Two items moved: the beta invite copy is done, the pricing page slipped to Thursday."),
            run(ROUTINE_STANDUP, 29 * hour, 38_000, RoutineRunStatus::Done, "No changes since yesterday; today: invite copy, pricing page, press list."),
            run(ROUTINE_STANDUP, 53 * hour, 7_000, RoutineRunStatus::Failed, "the box did not finish this run within 30 min"),
            run(ROUTINE_STANDUP, 77 * hour, 44_000, RoutineRunStatus::Done, "Launch date confirmed for 1 December; three items listed."),
        ],
    );
    out.insert(
        ROUTINE_PRS.to_string(),
        vec![run(
            ROUTINE_PRS,
            26 * hour,
            52_000,
            RoutineRunStatus::Done,
            "Three PRs open; #412 and #415 wait on your review.",
        )],
    );
    out.insert(
        ROUTINE_DEPLOY.to_string(),
        vec![
            run(
                ROUTINE_DEPLOY,
                40 * 60_000,
                12_000,
                RoutineRunStatus::Done,
                "api 1.4.2 deployed cleanly; error rate unchanged.",
            ),
            run(
                ROUTINE_DEPLOY,
                6 * hour,
                15_000,
                RoutineRunStatus::Done,
                "web 2.0.9 deployed; one new warning in the logs, not urgent.",
            ),
        ],
    );
    out
}

struct Seeder<'a> {
    doc: &'a ChatDoc,
    t: u64,
}

impl Seeder<'_> {
    fn user(&mut self, text: &str) {
        self.user_row(text, Vec::new(), None);
    }

    /// Attachments come before the text, as the clients write them.
    fn user_with_attachments(&mut self, text: &str, attachments: Vec<Attachment>) {
        self.user_row(text, attachments, None);
    }

    /// A reply bubble: `reply_to` is the id of the message it quotes.
    fn user_replying_to(&mut self, text: &str, reply_to: &str) {
        self.user_row(text, Vec::new(), Some(reply_to.to_owned()));
    }

    fn user_row(&mut self, text: &str, attachments: Vec<Attachment>, reply_to: Option<String>) {
        self.t += 45_000;
        let id = new_id();
        let opts = UserMessageOpts {
            attachments: user_attachments(&attachments),
            reply_to,
            ..UserMessageOpts::default()
        };
        self.doc.core.transact(|d| {
            let _ = d.append_user_message(&id, text, DEVICE, &opts);
            fill_attachment_details(d, &id, &attachments);
            d.set_message_fields(
                &id,
                &[("createdAt", json!(self.t)), ("completedAt", json!(self.t))],
            );
        });
    }

    /// Returns the new row's id so a later user row can reply to it.
    fn assistant(&mut self, turn: TurnKind, status: MessageStatus, parts: Vec<Part>) -> Id {
        self.t += 20_000;
        let id = new_id();
        self.doc.core.transact(|d| {
            let _ = d.begin_message(
                &id,
                "assistant",
                "agent",
                DEVICE,
                core_status(status),
                &[
                    ("turn", json!(turn)),
                    ("model", json!(MODELS[0])),
                    ("effort", json!(Effort::High)),
                    ("createdAt", json!(self.t)),
                    ("completedAt", json!(self.t + 9_000)),
                ],
            );
            write_parts(d, &id, &parts, false);
        });
        id
    }

    fn project(&mut self, project: &Project, text: &str) {
        self.t += 12_000;
        self.doc
            .core
            .transact(|d| write_project_line(d, project, text, self.t));
    }

    /// Agent-to-agent messaging, sender side: the hand-off row agentd writes after
    /// `send_message` (what the box writes, byte for byte in shape).
    fn agent_message_sent(&mut self, to_name: &str, text: &str) -> Id {
        self.t += 6_000;
        let id = new_id();
        self.doc.core.transact(|d| {
            let _ = d.begin_message(
                &id,
                "assistant",
                "agent",
                DEVICE,
                core_status(MessageStatus::Done),
                &[
                    ("turn", json!({ "kind": "spawn" })),
                    ("subagentName", json!(to_name)),
                    ("handoffMessages", json!(2)),
                    ("agentMessage", json!({ "id": new_id(), "toName": to_name })),
                    ("createdAt", json!(self.t)),
                    ("completedAt", json!(self.t)),
                ],
            );
            let _ = d.add_hidden_part(
                &id,
                "agent_message_sent",
                text,
                &json!({ "toName": to_name }),
            );
        });
        id
    }

    /// Agent-to-agent messaging, receiving side: another agent's words in this chat.
    fn agent_message_received(&mut self, from_id: &str, from_name: &str, text: &str) {
        self.t += 30_000;
        self.doc.core.transact(|d| {
            d.append_project_message(
                &new_id(),
                text,
                DEVICE,
                &format!("project:{from_id}"),
                &[
                    ("senderName", json!(from_name)),
                    ("senderHandle", json!(Project::mention_slug(from_name))),
                    (
                        "agentMessage",
                        json!({ "id": new_id(), "fromProjectId": from_id }),
                    ),
                    ("createdAt", json!(self.t)),
                    ("completedAt", json!(self.t)),
                ],
                false,
            );
        });
    }

    /// An answer to the user message two rows back: `replyTo` set (the quote header) when
    /// `quoted`, else the plain bubble (#1521 before/after).
    fn assistant_replying_to(&mut self, quoted: bool, text: &str) -> Id {
        let target = self
            .doc
            .read_all()
            .into_iter()
            .filter(|m| m.role == Role::User)
            .rev()
            .nth(1)
            .map(|m| m.id);
        let id = self.assistant(TurnKind::Reply, MessageStatus::Done, vec![text_part(text)]);
        if let Some(target) = target {
            self.doc.core.transact(|d| {
                d.set_message_fields(&id, &[("inReplyTo", json!(target))]);
                if quoted {
                    d.set_message_fields(&id, &[("replyTo", json!(target))]);
                }
            });
        }
        id
    }

    /// The reply that went back to that agent ("Replied to ◉ X").
    fn assistant_replying_to_agent(&mut self, to_name: &str, text: &str) -> Id {
        let id = self.assistant(TurnKind::Reply, MessageStatus::Done, vec![text_part(text)]);
        self.doc.core.transact(|d| {
            d.set_message_fields(&id, &[("agentReplyTo", json!(to_name))]);
        });
        id
    }
}

/// `ARBOS_FIXTURE_AGENT_MESSAGE=1`: seed the agent-to-agent messaging scenario.
fn agent_message_fixture() -> bool {
    std::env::var("ARBOS_FIXTURE_AGENT_MESSAGE").is_ok_and(|v| v == "1")
}

fn text(s: &str) -> Part {
    Part::Text {
        id: new_id(),
        text: s.into(),
    }
}

fn attachment(name: &str, mime: &str, kind: AttachmentKind, title: &str, bytes: u64) -> Part {
    Part::Attachment {
        id: new_id(),
        attachment: Attachment {
            name: name.into(),
            mime: mime.into(),
            blob_ref: Some(format!("seed-{name}")),
            bytes: Some(bytes),
            kind: Some(kind),
            title: Some(title.into()),
            chart_part: None,
            url: None,
            source: None,
        },
    }
}

/// A 320×200 PNG (diagonal gradient tinted from the key) for fixture thumbnails and downloads.
/// The 64×64 site icons the fixture's replies link to (what the relay's
/// `/link-icon` would answer), by host.
fn fixture_link_icon(host: &str) -> Option<&'static [u8]> {
    Some(match host {
        "www.elvirrey.com" => include_bytes!("../../assets/link-icons/www.elvirrey.com.png"),
        "www.communitas.pe" => include_bytes!("../../assets/link-icons/www.communitas.pe.png"),
        "www.tangol.com" => include_bytes!("../../assets/link-icons/www.tangol.com.png"),
        "www.assouline.com" => include_bytes!("../../assets/link-icons/www.assouline.com.png"),
        "nypost.com" => include_bytes!("../../assets/link-icons/nypost.com.png"),
        _ => return None,
    })
}

fn fixture_png(key: &str) -> Vec<u8> {
    if key.contains("Screenshot") {
        return fixture_page_png();
    }
    let seed = key.bytes().fold(0x811c_9dc5u32, |a, b| {
        (a ^ b as u32).wrapping_mul(0x0100_0193)
    });
    let (w, h) = (320u32, 200u32);
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        let fx = x as f32 / w as f32;
        let fy = y as f32 / h as f32;
        let r = 60.0 + (seed & 0x3F) as f32 + 110.0 * fx;
        let g = 70.0 + ((seed >> 8) & 0x3F) as f32 + 90.0 * fy;
        let b = 120.0 + ((seed >> 16) & 0x3F) as f32 + 60.0 * (1.0 - fx);
        let edge = x < 6 || y < 6 || x + 6 >= w || y + 6 >= h;
        if edge {
            image::Rgb([245, 245, 245])
        } else {
            image::Rgb([r as u8, g as u8, b as u8])
        }
    });
    let mut out = std::io::Cursor::new(Vec::new());
    let _ = img.write_to(&mut out, image::ImageFormat::Png);
    out.into_inner()
}

/// A 640×400 "page screenshot": white, empty top 22 %, a dark title bar, a
/// grey line, a blue button, a short grey line — the shape of Jacob's shot.
fn fixture_page_png() -> Vec<u8> {
    let (w, h) = (640u32, 400u32);
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        let inside = |x0: u32, y0: u32, x1: u32, y1: u32| x >= x0 && x < x1 && y >= y0 && y < y1;
        if inside(40, 88, 300, 116) {
            image::Rgb([24, 24, 24])
        } else if inside(40, 134, 330, 146) {
            image::Rgb([96, 96, 96])
        } else if inside(40, 176, 180, 216) {
            image::Rgb([26, 115, 232])
        } else if inside(40, 240, 190, 252) {
            image::Rgb([96, 96, 96])
        } else {
            image::Rgb([255, 255, 255])
        }
    });
    let mut out = std::io::Cursor::new(Vec::new());
    let _ = img.write_to(&mut out, image::ImageFormat::Png);
    out.into_inner()
}

fn tool(name: &str, input: &str, out: &str, ms: u64) -> Part {
    Part::Tool {
        id: new_id(),
        call: ToolCall {
            id: new_id(),
            name: name.into(),
            input: json!(input),
        },
        output_summary: out.into(),
        output_ref: Some(format!("blob/seed/{}", new_id())),
        is_error: false,
        status: Some(ToolStatus::Ok),
        duration_ms: Some(ms),
        subagent_name: None,
    }
}

/// The agent chats' seeded transcripts (one doc per demo chat).
fn seed_chat_docs() -> Vec<ChatDoc> {
    let mut out = Vec::new();
    let base = now_ms().saturating_sub(3 * 60 * 60_000);

    let main = ChatDoc::new(CHAT_ARBOS_MAIN, PROJECT_ARBOS);
    {
        let mut s = Seeder {
            doc: &main,
            t: base,
        };
        s.user("Let's refine the agent workspace.");
        s.assistant(
            TurnKind::Reply,
            MessageStatus::Done,
            vec![text(
                "Each agent has its own space, with separate tabs for the conversations happening in parallel.",
            )],
        );
        s.user("Keep it simple.");
        let last = s.assistant(
            TurnKind::Reply,
            MessageStatus::Done,
            vec![text(
                "A compact agent rail, clear chat tabs, and everything else within reach.",
            )],
        );
        // `ARBOS_FIXTURE_REPLY=1` (screenshot rig): one reply bubble quoting that answer.
        // `ARBOS_FIXTURE_REPLY=long`: a multi-paragraph markdown review as the last
        // message, so `ARBOS_REPLY_TO=1` arms the composer's reply preview on it.
        // `ARBOS_FIXTURE_REPLY=code`: the `REPLY_CODE` reply (a ```rust card) as
        // the last message, for the code card's Copy pill shots.
        // `ARBOS_FIXTURE_REPLY=table`: the `LIMA_REPLY` turn (a linked GFM table
        // between two prose bubbles), for the table renderer's shots.
        // `ARBOS_FIXTURE_REPLY=math`: the `BUBBLE_SORT_REPLY` turn (code card + inline math).
        match std::env::var("ARBOS_FIXTURE_REPLY").as_deref() {
            Ok("1") => {
                s.user_replying_to("Which tabs do you mean here?", &last);
            }
            Ok("long") => {
                s.user("How did the review go?");
                s.assistant(
                    TurnKind::Reply,
                    MessageStatus::Done,
                    vec![text(LONG_REVIEW)],
                );
            }
            Ok("code") => {
                s.user("How is the transcript list built?");
                s.assistant(
                    TurnKind::Reply,
                    MessageStatus::Done,
                    vec![text(&REPLY_CODE.join("\n\n"))],
                );
            }
            // `ARBOS_FIXTURE_REPLY=table`: Jacob's Lima bookstore turn — the ask, then
            // the three bubbles agentd posted (conclusion, linked GFM table, details).
            Ok("table") => {
                s.user("Find me a book store in lima");
                for bubble in LIMA_REPLY {
                    s.assistant(TurnKind::Reply, MessageStatus::Done, vec![text(bubble)]);
                }
            }
            // `ARBOS_FIXTURE_REPLY=links`: Jacob's New York Post turn — a bullet list of
            // linked headlines ([feedback link-icons-mac-2026-10-25]).
            Ok("links") => {
                s.user("Read the latest from the New York post");
                s.assistant(
                    TurnKind::Reply,
                    MessageStatus::Done,
                    vec![text(NYPOST_REPLY)],
                );
            }
            // `ARBOS_FIXTURE_REPLY=link`: Jacob's competitors-page turn — a bare URL
            // long enough to wrap, then a line of prose ([feedback link-click-mac-2026-10-27]).
            Ok("link") => {
                s.user("Where is the competitors page?");
                s.assistant(TurnKind::Reply, MessageStatus::Done, vec![text(LINK_REPLY)]);
            }
            // `ARBOS_FIXTURE_REPLY=markdown`: the chat-formatting sample sheet — every
            // format in my own bubble, the agent's answer with the same, then the
            // must-not-mangle lines ([feedback chat-markdown-mac-2026-10-27]).
            Ok("markdown") => {
                s.user(MARKDOWN_SAMPLE_USER);
                s.assistant(
                    TurnKind::Reply,
                    MessageStatus::Done,
                    vec![text(MARKDOWN_SAMPLE_REPLY)],
                );
                s.user(MARKDOWN_SAMPLE_PLAIN);
            }
            // `ARBOS_FIXTURE_REPLY=quote`: Jacob's report — "> hello" typed in the
            // composer, and the agent quoting it back ([feedback chat-markdown-mac-2026-10-27]).
            Ok("quote") => {
                s.user("> hello\nthat is what the quote looks like now");
                s.assistant(
                    TurnKind::Reply,
                    MessageStatus::Done,
                    vec![text(
                        "> hello\n\nYes — a `> ` line is a quote: inset, muted, with the bar. **Bold**, *italic*, ~~strike~~ and ||spoilers|| work too.",
                    )],
                );
            }
            // `ARBOS_FIXTURE_REPLY=math`: Jacob's bubble-sort turn — a Python code card,
            // then prose with inline `$O(n^2)$` math ([feedback math-render-mac-2026-10-26]).
            Ok("math") => {
                s.user("write bubble sort in python");
                s.assistant(
                    TurnKind::Reply,
                    MessageStatus::Done,
                    vec![text(BUBBLE_SORT_REPLY)],
                );
            }
            _ => {}
        }
        // Secret hand-off rig (`ARBOS_FIXTURE_SECRET_HANDOFF=offer|saved|ghost|detect`): the
        // request card open / saved, the "save this" ghost row, the offer card for a pasted key.
        let handoff_card = |approval_id: &str,
                            state: ApprovalState,
                            payload: Value,
                            title: &str,
                            summary: &str| Part::Approval {
            id: new_id(),
            approval_id: approval_id.into(),
            title: title.into(),
            summary: summary.into(),
            risk: ApprovalRisk::Credential,
            payload,
            state,
            expires_at: Some(base + 600_000),
            decided_at: (state != ApprovalState::Pending).then_some(base + 90_000),
            decided_by: (state != ApprovalState::Pending)
                .then(|| "user:01J9USER0JACOB000000000000".to_owned()),
            reason: None,
            grant: None,
        };
        match std::env::var("ARBOS_FIXTURE_SECRET_HANDOFF").as_deref() {
            Ok(mode @ ("offer" | "saved")) => {
                s.user("Can I give you a secret? It's the Stripe key for the refunds report.");
                let state = if mode == "saved" {
                    ApprovalState::Approved
                } else {
                    ApprovalState::Pending
                };
                s.assistant(
                    TurnKind::Act,
                    MessageStatus::Done,
                    vec![
                        text("Yes. Use the box below. It goes straight to your vault, never into this chat."),
                        handoff_card(
                            "01J9APPROVAL0SECRETOFFER00",
                            state,
                            json!({ "kind": "secret_offer", "name": "STRIPE_KEY", "purpose": "the refunds report",
                                "agent": "Arbos", "projectId": PROJECT_ARBOS, "chatId": CHAT_ARBOS_MAIN, "answers": [] }),
                            "Give Arbos a secret",
                            "the refunds report",
                        ),
                    ],
                );
                if mode == "saved" {
                    s.assistant(
                        TurnKind::Reply,
                        MessageStatus::Done,
                        vec![text("Got it. I'll use STRIPE_KEY for the refunds report and keep the value out of this chat.")],
                    );
                }
            }
            Ok("ghost") => {
                s.user("Let's do the refunds report next.");
                s.assistant(
                    TurnKind::Reply,
                    MessageStatus::Done,
                    vec![text("Sure — I need the Stripe key for that.")],
                );
                s.user("save this");
                let id = main
                    .core
                    .messages()
                    .last()
                    .map(|m| m.id.clone())
                    .unwrap_or_default();
                main.core.redact_message_as_secret(
                    &id,
                    &arbos_core::doc::SecretRedaction {
                        secret_id: "01J9SECRET0STRIPE000000000".into(),
                        name: "STRIPE_KEY".into(),
                        env: "STRIPE_KEY".into(),
                        saved_by: "user:01J9USER0JACOB000000000000".into(),
                        agent: Some(PROJECT_ARBOS.into()),
                    },
                );
                s.assistant(
                    TurnKind::Reply,
                    MessageStatus::Done,
                    vec![text("Saved as STRIPE_KEY and removed from this chat — also from my memory and working context. I can use it; the value stays in your vault. Running the refunds report now.")],
                );
            }
            Ok("detect") => {
                s.user("Let's do the refunds report next.");
                s.assistant(
                    TurnKind::Reply,
                    MessageStatus::Done,
                    vec![text("Sure — I need the Stripe key for that.")],
                );
                s.user(concat!(
                    "sk_",
                    "test_FIXTURE51HxQ9mT3vK2pLnR8wZ4eYc7bD1fG6hJ0kM9nP5qS2tU8vW3xA1"
                ));
                s.assistant(
                    TurnKind::Act,
                    MessageStatus::Done,
                    vec![handoff_card(
                        "01J9APPROVAL0SECRETDETECT0",
                        ApprovalState::Pending,
                        json!({ "kind": "secret_detected", "optional": true, "name": "STRIPE_KEY", "agent": "Arbos",
                            "chatId": CHAT_ARBOS_MAIN, "messageId": "", "answers": [
                                { "id": "save", "label": "Save as STRIPE_KEY and remove" },
                                { "id": "deny", "label": "Leave it here" } ] }),
                        "That looks like a key. Save it to your vault as STRIPE_KEY and remove it from this chat?",
                        "Nothing is saved until you choose.",
                    )],
                );
            }
            _ => {}
        }
        // `ARBOS_FIXTURE_AGENT_MESSAGE=1` (screenshot rig): the sender side of an
        // agent-to-agent message — "send Comms a hello" → the hand-off row
        // ("Messaged ◉ Comms" → "2 messages with ◉ Comms") → "Sent to Comms." → the
        // relayed reply. The Comms chat gets the receiving side (below).
        if agent_message_fixture() {
            s.user("send Comms a hello");
            s.agent_message_sent(
                "Comms",
                "Hello, Comms — Jacob asked me to pass along a hello.",
            );
            s.assistant(
                TurnKind::Reply,
                MessageStatus::Done,
                vec![text("Sent to Comms.")],
            );
            s.assistant(
                TurnKind::SubagentResult,
                MessageStatus::Done,
                vec![text("Comms says hello back — and it's ready whenever you want the weekly update sent.")],
            );
        }
    }
    out.push(main);

    let research = ChatDoc::new(CHAT_ARBOS_RESEARCH, PROJECT_ARBOS);
    {
        let mut s = Seeder {
            doc: &research,
            t: base + 600_000,
        };
        s.user("Check the rail tile sizes against the mock and fix the padding.");
        s.assistant(
            TurnKind::Act,
            MessageStatus::Done,
            vec![
                tool("read_file", "crates/arbos-mac/src/ui/rail.rs", "90 px rail, 68 px tiles, 8 px gap", 410),
                tool("grep", "TILE_W crates/", "3 matches in rail.rs, theme.rs", 160),
                tool("edit_file", "crates/arbos-mac/src/ui/rail.rs", "Tiles 72 px, gap 10 px", 620),
                Part::Diff {
                    id: new_id(),
                    path: "crates/arbos-mac/src/ui/rail.rs".into(),
                    added: 6,
                    removed: 2,
                    blob_ref: Some("blob/seed/diff-rail".into()),
                },
                tool("bash", "cargo check -p arbos-mac", "Finished `dev` profile in 4.1 s", 4100),
                text("Tiles were 68 px with an 8 px gap. I set them to 72 px and 10 px to match the mock, and `cargo check` is clean."),
            ],
        );
        s.user("Open the GPUI docs and see if list() supports bottom alignment.");
        s.assistant(
            TurnKind::Spawn,
            MessageStatus::Done,
            vec![
                Part::Status {
                    id: new_id(),
                    code: StatusKind::Working,
                    message: "Worked in sandbox · 4 actions · 00:41".into(),
                },
                Part::Tool {
                    id: new_id(),
                    call: ToolCall {
                        id: new_id(),
                        name: "spawn_subagent".into(),
                        input: json!("docs-lookup · prime"),
                    },
                    output_summary: "Subagent finished".into(),
                    output_ref: None,
                    is_error: false,
                    status: Some(ToolStatus::Ok),
                    duration_ms: Some(41_000),
                    subagent_name: Some("docs-lookup".into()),
                },
                Part::Browser {
                    id: new_id(),
                    title: "GPUI docs — ListAlignment".into(),
                    url: "https://docs.rs/gpui/latest/gpui/enum.ListAlignment.html".into(),
                    last_action: "read page".into(),
                    live: false,
                },
                Part::File {
                    id: new_id(),
                    path: "docs/research/gpui.md".into(),
                    lines: 212,
                    blob_ref: Some("blob/seed/gpui-notes".into()),
                },
                text("Yes. `ListAlignment::Bottom` scrolls from the bottom up, like a chat log. Notes are in `docs/research/gpui.md`."),
            ],
        );
    }
    out.push(research);

    let launch = ChatDoc::new(CHAT_ARBOS_LAUNCH, PROJECT_ARBOS);
    {
        let mut s = Seeder {
            doc: &launch,
            t: base + 1_200_000,
        };
        s.user("Draft a launch plan for the Mac beta.");
        s.assistant(
            TurnKind::Reply,
            MessageStatus::Done,
            vec![
                text(ASK_LEAD),
                Part::Status {
                    id: new_id(),
                    code: StatusKind::AskUser,
                    message: ASK_QUESTION.into(),
                },
            ],
        );
    }
    out.push(launch);

    // Every widget type once (design §10): one answered choice, then five open cards.
    let widgets = ChatDoc::new(CHAT_ARBOS_WIDGETS, PROJECT_ARBOS);
    {
        let mut s = Seeder {
            doc: &widgets,
            t: base + 2_400_000,
        };
        s.user("Let's get the beta out this month.");
        s.assistant(
            TurnKind::Reply,
            MessageStatus::Done,
            vec![
                text("Happy to. One thing first:"),
                question_part(
                    "Dark or light theme for the first screenshots?",
                    Some(Widget::Choice {
                        options: vec![opt("dark", "Dark"), opt("light", "Light")],
                    }),
                    Some(json!("dark")),
                ),
            ],
        );
        s.user("Answer: Dark");
        s.assistant(
            TurnKind::Reply,
            MessageStatus::Done,
            vec![
                text("Dark it is. A few more things so I can set everything up:"),
                question_part(
                    "Which build should the beta ship from?",
                    Some(widget_choice()),
                    None,
                ),
            ],
        );
        s.assistant(
            TurnKind::Reply,
            MessageStatus::Done,
            vec![question_part(
                "Which platforms are in the first wave?",
                Some(widget_multi()),
                None,
            )],
        );
        s.assistant(
            TurnKind::Reply,
            MessageStatus::Done,
            vec![question_part(
                "What should this release be called?",
                Some(widget_text()),
                None,
            )],
        );
        s.assistant(
            TurnKind::Reply,
            MessageStatus::Done,
            vec![question_part(
                "When does the beta open?",
                Some(widget_date()),
                None,
            )],
        );
        s.assistant(
            TurnKind::Reply,
            MessageStatus::Done,
            vec![question_part("Release details", Some(widget_form()), None)],
        );
    }
    out.push(widgets);
    // S18: one approved card (already sent) and one pending card waiting on the user.
    let drafts = ChatDoc::new(CHAT_ARBOS_DRAFTS, PROJECT_ARBOS);
    {
        let mut s = Seeder {
            doc: &drafts,
            t: base + 2_000_000,
        };
        s.user("Email Sam to confirm Tuesday's demo at 10:00.");
        let sam_body = "Hi Sam,\n\nConfirming the Arbos demo on Tuesday at 10:00 (your office, 45 min). I'll bring the Mac build and a sandbox you can poke at.\n\nSee you then,\nJacob";
        let decided = s.t + 60_000;
        s.assistant(
            TurnKind::Act,
            MessageStatus::Done,
            vec![
                text("Here is the confirmation. Approve the card to send it."),
                approval_card(
                    "01J9APPROVAL0SAM0000000000",
                    "email",
                    "sam@affine.pro",
                    "Tuesday demo — confirmed for 10:00",
                    sam_body,
                    ApprovalState::Approved,
                    Some(decided),
                ),
            ],
        );
        s.assistant(
            TurnKind::Reply,
            MessageStatus::Done,
            vec![text(
                "Sent. The email to sam@affine.pro went out at 10:14; it is in this box's outbox and the audit trail.",
            )],
        );
        s.user("Now draft the October investor update for Priya.");
        let pending = s.assistant(
            TurnKind::Act,
            MessageStatus::Done,
            vec![
                text(DRAFT_LEAD),
                approval_card(
                    "01J9APPROVAL0PRIYA00000000",
                    "email",
                    DRAFT_TO,
                    DRAFT_SUBJECT,
                    DRAFT_BODY,
                    ApprovalState::Pending,
                    None,
                ),
                Part::Status {
                    id: new_id(),
                    code: StatusKind::WaitingApproval,
                    message: format!("Send email to {DRAFT_TO}: {DRAFT_SUBJECT}"),
                },
            ],
        );
        drafts
            .core
            .set_message_fields(&pending, &[("waitingApproval", json!(true))]);
        // #1544 rig (`ARBOS_FIXTURE_SHELL_CARD=1`): a destructive-shell card with its four
        // standing rows, as agentd posts it for `rm -rf` outside scratch.
        if std::env::var("ARBOS_FIXTURE_SHELL_CARD").as_deref() == Ok("1") {
            s.user("clone arbos into /tmp and list the top level");
            let command = "cd /work/app && rm -rf node_modules && git clone --depth 1 https://github.com/unarbos/arbos arbos-src";
            let shell = s.assistant(
                TurnKind::Act,
                MessageStatus::Done,
                vec![
                    text("On it."),
                    Part::Approval {
                        id: new_id(),
                        approval_id: "01J9APPROVAL0SHELL00000000".into(),
                        title: "Run a destructive command (file-delete)".into(),
                        summary: command.into(),
                        risk: ApprovalRisk::Destructive,
                        payload: json!({
                            "kind": "tool_call", "tool": "bash", "class": "file-delete", "path_class": "workspace",
                            "why": "deletes files outside /work/tmp",
                            "input": { "command": command },
                            "grant_options": [
                                { "path_class": "workspace", "label": "its workspace", "detail": "the agent's own /work folder, including scratch" },
                                { "path_class": "anywhere", "label": "anywhere", "detail": "any folder of its box" }
                            ]
                        }),
                        state: ApprovalState::Pending,
                        expires_at: Some(s.t + APPROVAL_TTL_MS as u64),
                        decided_at: None,
                        decided_by: None,
                        reason: None,
                        grant: None,
                    },
                ],
            );
            drafts
                .core
                .set_message_fields(&shell, &[("waitingApproval", json!(true))]);
        }
    }
    out.push(drafts);
    let media = ChatDoc::new(CHAT_ARBOS_MEDIA, PROJECT_ARBOS);
    {
        let mut s = Seeder {
            doc: &media,
            t: base + 2_400_000,
        };
        s.user("Put together the Q3 review pack: deck, budget sheet, one-pager, PDF export, a cover image, the teaser clip and the voice-over.");
        s.assistant(
            TurnKind::Act,
            MessageStatus::Done,
            vec![
                text("Here is the Q3 review pack. Each file is ready to download."),
                attachment("q3-cover.png", "image/png", AttachmentKind::Image, "Q3 cover image", 418_000),
                attachment("q3-review.pptx", "application/vnd.openxmlformats-officedocument.presentationml.presentation", AttachmentKind::Slides, "Q3 review deck · 12 slides", 2_310_000),
                attachment("q3-budget.xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", AttachmentKind::Sheet, "Q3 budget vs. actuals", 61_000),
                attachment("q3-one-pager.docx", "application/vnd.openxmlformats-officedocument.wordprocessingml.document", AttachmentKind::Doc, "One-pager for the board", 48_500),
                attachment("q3-review.pdf", "application/pdf", AttachmentKind::Pdf, "Deck, exported as PDF", 1_540_000),
                attachment("q3-teaser.mp4", "video/mp4", AttachmentKind::Video, "Teaser clip · 0:24", 9_800_000),
                attachment("q3-voiceover.mp3", "audio/mpeg", AttachmentKind::Audio, "Voice-over · 1:12", 1_120_000),
                attachment("q3-sources.zip", "application/zip", AttachmentKind::File, "Source data and scripts", 3_300_000),
                text("The deck follows last quarter's structure; the budget sheet has a variance column and a chart on the second tab."),
            ],
        );
        // Jacob's case (chat-preview-cards-2026-10-23): the agent screenshots the
        // small page it made — a white page with its own empty top margin.
        s.user("Make me a small page with a button that counts clicks.");
        s.assistant(
            TurnKind::Reply,
            MessageStatus::Done,
            vec![
                text("Here it is — a small page with a counter button."),
                attachment(
                    "Screenshot 2026-10-23 at 7.55.41.png",
                    "image/png",
                    AttachmentKind::Image,
                    "Screenshot 2026-10-23 at 7.55.41.png",
                    212_000,
                ),
            ],
        );
    }
    out.push(media);

    // Voice memo (F11): a spoken prompt keeps its audio as an attachment part.
    let voice = ChatDoc::new(CHAT_ARBOS_VOICE, PROJECT_ARBOS);
    {
        let mut s = Seeder {
            doc: &voice,
            t: base + 2_700_000,
        };
        s.user_with_attachments(
            FIXTURE_TRANSCRIPT,
            vec![Attachment {
                name: "voice-memo.wav".into(),
                mime: "audio/wav".into(),
                blob_ref: Some(VOICE_SEED_BLOB.into()),
                bytes: None,
                kind: Some(AttachmentKind::Audio),
                title: None,
                chart_part: None,
                url: None,
                source: None,
            }],
        );
        s.assistant(
            TurnKind::Reply,
            MessageStatus::Done,
            vec![text(
                "Three things today: the beta invite copy is due at noon, the pricing page review is at three, and two pull requests wait on you. Want me to start with the invite copy?",
            )],
        );
    }
    out.push(voice);

    let group = ChatDoc::new(CHAT_ARBOS_GROUP, PROJECT_ARBOS);
    {
        let projects = seed_projects();
        let affine = projects.iter().find(|p| p.id == PROJECT_AFFINE).unwrap();
        let comms = projects.iter().find(|p| p.id == PROJECT_COMMS).unwrap();
        let mut s = Seeder {
            doc: &group,
            t: base + 2_400_000,
        };
        s.user("Launch sync: Affine, Comms — where do you each stand for the Mac beta?");
        s.assistant(
            TurnKind::Reply,
            MessageStatus::Done,
            vec![text(
                "Arbos is ready on its side: the shell, tabs and the composer are stable and the relay runs. Affine and Comms will answer here.",
            )],
        );
        s.project(
            affine,
            "The canvas export is in review and lands tomorrow. Nothing from Arbos is blocking me.",
        );
        s.project(
            comms,
            "Announcement draft is written. I need the final build number from Arbos before I schedule it.",
        );
        // `ARBOS_FIXTURE_REPLY_TO=1` (screenshot rig, #1521): a question asked two messages
        // back, then the agent's answer carrying `replyTo` = that question — the bubble shows
        // the quote header. `=0` writes the same rows without `replyTo` (the before-shot).
        if let Ok(v) = std::env::var("ARBOS_FIXTURE_REPLY_TO") {
            s.user("Arbos, what is the final build number for the beta?");
            s.user("And Affine, is the export behind a flag?");
            s.project(affine, "Yes — `canvas_export`, on by default in the beta.");
            s.assistant_replying_to(
                v == "1",
                "The beta ships as build 0.6.184; Comms, that is the number for the announcement.",
            );
        }
    }
    out.push(group);

    let affine = ChatDoc::new(CHAT_AFFINE_MAIN, PROJECT_AFFINE);
    {
        let mut s = Seeder {
            doc: &affine,
            t: base.saturating_sub(23 * 60 * 60_000),
        };
        s.user("Summarise yesterday's design review.");
        s.assistant(
            TurnKind::Reply,
            MessageStatus::Done,
            vec![text(
                "Three decisions: keep the canvas infinite, move export to the share sheet, and drop the legacy sidebar. Two open questions are in the thread.",
            )],
        );
    }
    out.push(affine);

    let comms = ChatDoc::new(CHAT_COMMS_MAIN, PROJECT_COMMS);
    {
        let mut s = Seeder {
            doc: &comms,
            t: base.saturating_sub(2 * 60 * 60_000),
        };
        s.user("Send the weekly update to the team.");
        s.assistant(
            TurnKind::Act,
            MessageStatus::Error,
            vec![
                tool("read_file", "updates/2026-10-13.md", "1.2 KB", 300),
                Part::Error {
                    id: new_id(),
                    code: "sandbox_timeout".into(),
                    message: "Sandbox did not answer in 60 s".into(),
                },
            ],
        );
        // `ARBOS_FIXTURE_AGENT_MESSAGE=1`: the receiving side — Arbos's row (member
        // bubble labelled "Arbos"), Comms's reply, "Replied to ◉ Arbos".
        if agent_message_fixture() {
            s.t = base.saturating_sub(10 * 60_000);
            s.agent_message_received(
                PROJECT_ARBOS,
                "Arbos",
                "Hello, Comms — Jacob asked me to pass along a hello.",
            );
            s.assistant_replying_to_agent(
                "Arbos",
                "Hello back, Jacob — and hi Arbos! Ready whenever you want the weekly update sent.",
            );
        }
    }
    out.push(comms);
    out
}

// ---------------------------------------------------------------------------
// People chats (D72): two users (me @jacob, @sam) and one shared agent.
// ---------------------------------------------------------------------------

pub const ME_ID: &str = "gh_1";
pub const ME_HANDLE: &str = "jacob";
const SAM_ID: &str = "gh_456";
pub const DM_SAM: &str = "01J9DMSAM00000000000000000";
/// A project @sam shares with me (I am a guest).
pub const SHARED_STUDIO: &str = "01J9STUDIO0000000000000000";
const SHARED_STUDIO_CHAT: &str = "01J9STUDIOCHAT000000000000";
/// The seeded people group (redesign mockups): one id for the space and its chat.
pub const GROUP_LAUNCH: &str = "01J9GROUPLAUNCH00000000000";

fn person(id: &str, handle: &str, name: Option<&str>, status: &str) -> Person {
    Person {
        id: id.into(),
        handle: handle.into(),
        name: name.map(str::to_string),
        avatar_url: format!("fixture://avatar/{handle}"),
        status: status.into(),
        role: "member".into(),
        // Sam is "online" in the demo; the rest were seen a while ago.
        last_seen_at: Some(if handle == "sam" {
            now_ms() as i64
        } else {
            now_ms() as i64 - 3 * 3_600_000
        }),
        online: handle == "sam",
        avatar_color_index: None,
    }
}

fn me() -> Person {
    person(ME_ID, ME_HANDLE, Some("Jacob Steeves"), "member")
}

fn sam() -> Person {
    person(SAM_ID, "sam", Some("Sam Rivera"), "member")
}

/// `ARBOS_FIXTURE_EMOJI_NAMES=1` ([feedback emoji-name-handle-offset-mac-2026-10-28]):
/// a DM with someone whose display name is one emoji and a group that also has a
/// mixed name, so every name + handle stack can be shot beside text names.
fn emoji_names_seeded() -> bool {
    std::env::var("ARBOS_FIXTURE_EMOJI_NAMES").is_ok_and(|v| v == "1")
}

/// `ARBOS_FIXTURE_CHIPS=1` ([feedback mention-chip-highlight-2026-10-28]): the
/// group ends with someone else's and my own bubble, each with an agent and a
/// person chip (mine has one inside a code span).
fn mention_chips_seeded() -> bool {
    std::env::var("ARBOS_FIXTURE_CHIPS").is_ok_and(|v| v == "1")
}

/// The emoji-only name (a mage with a skin tone and a ZWJ: one grapheme, four scalars).
pub const MAGE_NAME: &str = "🧙🏽\u{200d}♂\u{fe0f}";
pub const MAGE_HANDLE: &str = "dawizzaa";
pub const DM_MAGE: &str = "01J9DMMAGE0000000000000000";
pub const GROUP_EMOJI: &str = "01J9GROUPEMOJI000000000000";

fn mage() -> Person {
    person("gh_461", MAGE_HANDLE, Some(MAGE_NAME), "member")
}

fn cato() -> Person {
    person("gh_462", "cato", Some("Cato 🌳"), "member")
}

fn emoji_names_group() -> Project {
    let members = vec![
        mage(),
        cato(),
        person("gh_458", "sara", Some("Sara Lind"), "member"),
    ];
    let mut space = people_space(GROUP_EMOJI, members[0].clone(), Vec::new());
    space.kind = SpaceKind::Group;
    space.name = "Emoji names".into();
    space.icon = "E".into();
    space.people = members;
    space.owner_handle = Some(ME_HANDLE.into());
    space.my_role = Some("owner".into());
    space
}

/// The directory the new-chat sheet searches.
fn directory() -> Vec<Person> {
    vec![
        sam(),
        person("gh_457", "samir", Some("Samir Haddad"), "member"),
        person("gh_458", "sara", Some("Sara Lind"), "member"),
        person("gh_459", "octocat", None, "invited"),
        person("gh_460", "jules", Some("Jules Park"), "member"),
    ]
}

fn people_space(id: &str, other: Person, agents: Vec<AgentMember>) -> Project {
    Project {
        id: id.into(),
        name: other.handle.clone(),
        icon: other.initial(),
        color: "#8b7cf6".into(),
        state: BoxState::Awake,
        state_detail: None,
        wake_progress: None,
        model: MODELS[0].into(),
        effort: Effort::Low,
        spend_today_usd: 0.0,
        spend_cap_usd: 0.0,
        kind: SpaceKind::Dm,
        pending: other.status == "invited",
        people: vec![other],
        agents,
        shared: false,
        owner_handle: None,
        avatar: None,
        avatar_color: None,
        description: None,
        owner_person: None,
        machine_info: None,
        outside: None,
        muted: false,
        silenced: false,
        silenced_until: None,
        archived_at: None,
        primary_chat: None,
        voice: None,
        vision_enabled: true,
        avatar_color_index: None,
        avatar_color_override: None,
        avatar_leaf: None,
        avatar_leaf_color: None,
        image_url: None,
        avatar_version: 0,
        message_grants: Vec::new(),
        blocked_senders: Vec::new(),
        my_role: None,
    }
}

/// The seeded people group: Sam, Samir and Sara with my Arbos agent in it.
fn launch_week() -> Project {
    let members = vec![
        sam(),
        person("gh_457", "samir", Some("Samir Haddad"), "member"),
        person("gh_458", "sara", Some("Sara Lind"), "member"),
    ];
    let agent = AgentMember {
        project_id: PROJECT_ARBOS.into(),
        name: "Arbos".into(),
        icon: "✳".into(),
        color: "#1084fe".into(),
        handle: "arbos".into(),
        owner_id: ME_ID.into(),
        owner_handle: ME_HANDLE.into(),
        reply_policy: Default::default(),
        avatar: None,
        avatar_color: None,
        avatar_color_index: None,
        avatar_leaf: None,
        avatar_leaf_color: None,
        image_url: None,
        avatar_version: 0,
        description: None,
        model: None,
        owner: None,
    };
    let mut agents = vec![agent];
    // Someone else's agent first, so `ARBOS_FIXTURE_WORKING=1` shows it working (#1592).
    if fixture_foreign_agent() {
        agents.insert(0, testagent_member());
    }
    // `ARBOS_FIXTURE_WORKING=<n>` (screenshot rig): the group holds n agents so
    // the presence row can show several of them working at once.
    for (project_id, name, handle, leaf, color) in fixture_working_extra_agents() {
        agents.push(AgentMember {
            project_id: project_id.into(),
            name: name.into(),
            icon: "✳".into(),
            color: "#1084fe".into(),
            handle: handle.into(),
            owner_id: ME_ID.into(),
            owner_handle: ME_HANDLE.into(),
            reply_policy: Default::default(),
            avatar: None,
            avatar_color: None,
            avatar_color_index: Some(color),
            avatar_leaf: Some(leaf),
            avatar_leaf_color: Some(color),
            image_url: None,
            avatar_version: 0,
            description: None,
            model: None,
            owner: None,
        });
    }
    let mut space = people_space(GROUP_LAUNCH, members[0].clone(), agents);
    space.kind = SpaceKind::Group;
    space.name = "Launch week".into();
    space.icon = "L".into();
    space.people = members;
    space.owner_handle = Some(ME_HANDLE.into());
    space.my_role = Some("owner".into());
    // `ARBOS_FIXTURE_GROUP_ROLE=member|admin` (screenshot rig, [feedback
    // group-settings-mac-2026-10-27]): I am that in "Launch week" and Sam owns it, so the
    // pane can be shot as a plain member (no crayon, no name field) without a second account.
    if let Ok(role) = std::env::var("ARBOS_FIXTURE_GROUP_ROLE") {
        let role = role.trim().to_lowercase();
        if role == "member" || role == "admin" {
            if let Some(sam) = space.people.first_mut() {
                sam.role = "owner".into();
                space.owner_handle = Some(sam.handle.clone());
            }
            space.my_role = Some(role);
        }
    }
    space
}

fn seed_people_projects() -> Vec<Project> {
    let mut owner = sam();
    owner.role = "owner".into();
    let mut out = vec![
        people_space(DM_SAM, sam(), Vec::new()),
        launch_week(),
        Project {
            id: SHARED_STUDIO.into(),
            name: "Studio".into(),
            icon: "◆".into(),
            color: "#d6843f".into(),
            state: BoxState::Awake,
            state_detail: None,
            wake_progress: None,
            model: MODELS[0].into(),
            effort: Effort::Medium,
            spend_today_usd: 0.0,
            spend_cap_usd: 25.0,
            kind: SpaceKind::Project,
            people: vec![owner],
            agents: Vec::new(),
            pending: false,
            shared: true,
            owner_handle: Some("sam".into()),
            avatar: None,
            avatar_color: Some("cyan".into()),
            description: None,
            owner_person: None,
            machine_info: None,
            outside: None,
            muted: false,
            silenced: false,
            silenced_until: None,
            archived_at: None,
            primary_chat: None,
            voice: None,
            vision_enabled: true,
            avatar_color_index: None,
            avatar_color_override: None,
            avatar_leaf: None,
            avatar_leaf_color: None,
            image_url: None,
            avatar_version: 0,
            message_grants: Vec::new(),
            blocked_senders: Vec::new(),
            my_role: None,
        },
    ];
    if emoji_names_seeded() {
        out.push(people_space(DM_MAGE, mage(), Vec::new()));
        out.push(emoji_names_group());
    }
    out
}

fn people_chat(id: &str, title: &str, minutes_ago: u64) -> Chat {
    Chat {
        id: id.into(),
        project_id: id.into(),
        title: title.into(),
        is_pinned: true,
        is_system: false,
        kind: ChatKind::Chat,
        participants: Vec::new(),
        unread: false,
        unread_count: 0,
        mention_count: 0,
        last_message_at: now_ms().saturating_sub(minutes_ago * 60_000),
        archived_at: None,
    }
}

fn seed_people_chats() -> Vec<Chat> {
    let mut out = seed_people_chats_base();
    if emoji_names_seeded() {
        out.push(people_chat(DM_MAGE, MAGE_HANDLE, 8));
        out.push(people_chat(GROUP_EMOJI, "Emoji names", 30));
    }
    out
}

fn seed_people_chats_base() -> Vec<Chat> {
    vec![
        Chat {
            id: DM_SAM.into(),
            project_id: DM_SAM.into(),
            title: "sam".into(),
            is_pinned: true,
            is_system: false,
            kind: ChatKind::Chat,
            participants: Vec::new(),
            unread: true,
            // The control for the unread-mention mark: unread, and named by
            // nobody. Sitting next to Launch week it is what makes a shot of
            // the sidebar prove something — the `@` is beside one count and
            // not the other, so the mark is visibly about being mentioned
            // rather than about being unread. Keep the two rows adjacent and
            // keep this one's `mention_count` at zero.
            unread_count: 3,
            mention_count: 0,
            last_message_at: now_ms().saturating_sub(12 * 60_000),
            archived_at: None,
        },
        Chat {
            id: GROUP_LAUNCH.into(),
            project_id: GROUP_LAUNCH.into(),
            title: "Launch week".into(),
            is_pinned: true,
            is_system: false,
            kind: ChatKind::Chat,
            participants: Vec::new(),
            unread: true,
            unread_count: 6,
            // Two of those six name me directly; the `@here` in the same
            // group does not count.
            mention_count: 2,
            last_message_at: now_ms().saturating_sub(25 * 60_000),
            archived_at: None,
        },
        Chat {
            id: SHARED_STUDIO_CHAT.into(),
            project_id: SHARED_STUDIO.into(),
            title: "Studio".into(),
            is_pinned: true,
            is_system: false,
            kind: ChatKind::Chat,
            participants: Vec::new(),
            unread: false,
            unread_count: 0,
            mention_count: 0,
            last_message_at: now_ms().saturating_sub(50 * 60_000),
            archived_at: None,
        },
    ]
}

fn human_message(chat_id: &str, who: &Person, text: &str, at: u64) -> Message {
    Message {
        id: new_id(),
        chat_id: chat_id.into(),
        role: Role::User,
        sender: format!("user:{}", who.id),
        status: MessageStatus::Done,
        turn: None,
        decision: None,
        subagent_id: None,
        subagent_name: None,
        handoff_messages: None,
        notice: None,
        worker: false,
        job: None,
        hidden: false,
        agent_reply_to: None,
        reactions: Default::default(),
        in_reply_to: None,
        routine_name: None,
        sender_name: who.name.clone(),
        sender_icon: None,
        sender_handle: Some(who.handle.clone()),
        sender_avatar: Some(who.avatar_url.clone()),
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
        created_at: at,
        completed_at: Some(at),
        call_id: None,
        activity: None,
        pending: false,
        parts: vec![text_part(text)],
        hidden_parts: Vec::new(),
        greeting: false,
        preview: None,
    }
}

fn text_part(text: &str) -> Part {
    Part::Text {
        id: new_id(),
        text: text.into(),
    }
}

/// `ARBOS_FIXTURE_LONG_ROWS`: a long group history shaped like the iPhone
/// rig's "Testing" group — four people and one agent talking, a reply every
/// tenth row, a markdown answer every seventh, a relay membership line every
/// fortieth, an image or file every twenty-fifth, a date break every three
/// hundredth. Deterministic ids so two launches render the same rows. Ends
/// `before`; rows sit 40 s apart. Only lengthens the message list.
fn long_group_transcript(rows: usize, before: u64) -> Vec<Message> {
    let people: [Person; 4] = [
        person("fx-u3", "mira-k", Some("Mira Kaplan"), "member"),
        sam(),
        me(),
        person("gh_32490803", "unconst", Some("Unconst"), "member"),
    ];
    let lines = [
        "Can we move the launch call to 10 am? The press embargo lifts at noon.",
        "Done — calendar updated. I also pinged the design channel about the hero asset.",
        "The signup form drops the handle field on iOS 27 when autofill fires. Repro in 2 of 5 tries.",
        "Looks related to the keyboard inset change. Checking the TestFlight build from last night.",
        "Shipping notes are in the shared doc. Shout if the pricing table reads wrong.",
        "Also: the relay restarted twice this morning, both clean 1012 closes. Nothing lost.",
        "https://arbos.life/i/launch is live behind the flag. Please smoke it on a phone.",
        "Agreed. Let's keep the group to launch chatter until Friday.",
    ];
    let markdown = [
        "Three things to check before the embargo lifts:\n\n- the **hero** video loops without a black frame\n- `/download/mac` resolves to the new build\n- the status page shows green for the relay\n\nI'll take the first two.",
        "## Launch day timeline\n\n1. 09:30 final smoke on TestFlight\n2. 10:00 call\n3. 12:00 embargo lifts, post goes out\n4. 13:00 watch the relay dashboard\n\nCode to flip the flag: `arbosctl admin flags set launch on`.",
        "| Step | Owner | State |\n| --- | --- | --- |\n| Hero asset | Mira | done |\n| Pricing copy | Sam | review |\n| Relay deploy | Jacob | queued |",
    ];
    let mut out = Vec::with_capacity(rows);
    let start = before.saturating_sub(rows as u64 * 40_000);
    for i in 0..rows {
        let id = format!("long-{i}");
        let mut at = start + i as u64 * 40_000;
        if i > 0 && i % 300 == 0 {
            at = at.saturating_add(2 * 3_600_000);
        }
        if i % 40 == 39 {
            let who = &people[i % people.len()];
            let name = who.name.as_deref().unwrap_or(who.handle.as_str());
            let mut msg = human_message(GROUP_LAUNCH, who, &format!("Jacob added {name}"), at);
            msg.id = id;
            msg.role = Role::System;
            msg.sender = "system".into();
            msg.sender_name = None;
            msg.sender_handle = None;
            msg.sender_avatar = None;
            out.push(msg);
            continue;
        }
        if i % 7 == 6 {
            let text = markdown[(i / 7) % markdown.len()];
            let mut msg = human_message(GROUP_LAUNCH, &me(), text, at);
            msg.id = id;
            msg.role = Role::Assistant;
            msg.sender = format!("agent:{PROJECT_ARBOS}");
            msg.sender_name = Some("Arbos".into());
            msg.sender_handle = Some("arbos".into());
            msg.sender_avatar = None;
            msg.turn = Some(TurnKind::Reply);
            msg.decision = Some("reply".into());
            msg.completed_at = Some(at.saturating_add(3_000));
            out.push(msg);
            continue;
        }
        let who = &people[(i / 3) % people.len()];
        if i % 25 == 24 {
            let image = i % 50 == 24;
            let part = if image {
                attachment(
                    &format!("IMG_{}.jpg", 1000 + i),
                    "image/jpeg",
                    AttachmentKind::Image,
                    "Launch photo",
                    212_000,
                )
            } else {
                attachment(
                    &format!("launch-notes-{i}.pdf"),
                    "application/pdf",
                    AttachmentKind::Pdf,
                    "Launch notes",
                    1_840_000,
                )
            };
            let mut msg = human_message(GROUP_LAUNCH, who, "", at);
            msg.id = id;
            msg.parts = vec![part];
            out.push(msg);
            continue;
        }
        let mut msg = human_message(GROUP_LAUNCH, who, lines[i % lines.len()], at);
        msg.id = id;
        if i % 10 == 5 && i >= 5 {
            msg.reply_to = Some(format!("long-{}", i - 5));
        }
        out.push(msg);
    }
    out
}

fn seed_people_chat_docs() -> Vec<ChatDoc> {
    let mut out = Vec::new();
    let base = now_ms().saturating_sub(40 * 60_000);
    let doc = ChatDoc::new(DM_SAM, "people");
    insert_message_via_core(
        &doc,
        &human_message(
            DM_SAM,
            &sam(),
            "Hey — are you around? I want to go over the beta invite copy before it ships.",
            base,
        ),
    );
    // Sam reacted to my line (one chip); a seed for the reaction rows.
    let mut mine = human_message(
        DM_SAM,
        &me(),
        "Here now. Paste what you have and I'll read it.",
        base + 90_000,
    );
    mine.reactions
        .entry("👍".into())
        .or_default()
        .insert(format!("user:{}", sam().id), true);
    insert_message_via_core(&doc, &mine);
    insert_message_via_core(&doc, &human_message(
        DM_SAM,
        &sam(),
        "Sending in a sec. Also: can we add your Arbos agent here so it can pull the numbers while we talk?",
        base + 150_000,
    ));
    if std::env::var("ARBOS_FIXTURE_LINKS").is_ok_and(|v| v == "1") {
        // Sam's Links tab ([feedback link-icons-mac-tile-2026-10-27] shots): a brand with a
        // mono mark, one with a colour, X, and a site with no bundled mark.
        for (i, text) in [
            "The repo: https://github.com/unarbos/arbos",
            "Launch playlist https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M?si=abc",
            "Their post https://x.com/arbos/status/1982341234567890123",
            "Background reading https://nypost.com/2026/10/25/business/musk-says-spacex-cut-ukraines-starlink-access/",
        ]
        .into_iter()
        .enumerate()
        {
            insert_message_via_core(
                &doc,
                &human_message(DM_SAM, &sam(), text, base + 160_000 + i as u64 * 5_000),
            );
        }
    }
    // The Launch week group's rows are built here so the DM can quote them
    // (`ARBOS_FIXTURE_REPLY_DM=1`, #1835: Reply in DM carries the original).
    let samir = person("gh_457", "samir", Some("Samir Haddad"), "member");
    let sara = person("gh_458", "sara", Some("Sara Lind"), "member");
    let deck = human_message(
        GROUP_LAUNCH,
        &samir,
        "Deck v3 is in the shared folder. Sam, can you check the pricing slide before the run-through?",
        base + 300_000,
    );
    let ask = human_message(
        GROUP_LAUNCH,
        &sam(),
        "On it. @arbos pull the latest signup numbers into slide 4 while I read.",
        base + 330_000,
    );
    // Two direct `@jacob` lines and one `@here`, so the fixture shows the
    // unread-mention mark on the row, the count on the transcript's jump
    // capsule, and — through the `@here` — that a collective lights neither.
    let tagged_early = human_message(
        GROUP_LAUNCH,
        &samir,
        "@jacob can you take the embargo list? I am out until Monday.",
        base + 336_000,
    );
    let collective = human_message(
        GROUP_LAUNCH,
        &sara,
        "Reminder @here: the run-through is at four.",
        base + 342_000,
    );
    let tagged_late = human_message(
        GROUP_LAUNCH,
        &sam(),
        "One more for you @jacob — the DMG link in the post is still the old one.",
        base + 348_000,
    );
    if std::env::var("ARBOS_FIXTURE_REPLY_DM").is_ok_and(|v| v == "1") {
        // Sam answers Samir's group line privately, here; I answer Sam's group ask.
        let mut theirs = human_message(DM_SAM, &sam(), "Works", base + 400_000);
        theirs.reply_ref = Some(ReplyRef {
            chat_id: GROUP_LAUNCH.into(),
            project_id: "people".into(),
            message_id: deck.id.clone(),
            chat_title: "Launch week".into(),
            sender: deck.sender.clone(),
            sender_name: "Samir Haddad".into(),
            sender_handle: "samir".into(),
            text: ReplyRef::snapshot_text(&deck.plain_text()),
        });
        insert_message_via_core(&doc, &theirs);
        let mut mine = human_message(
            DM_SAM,
            &me(),
            "Take your time — the numbers are in.",
            base + 430_000,
        );
        mine.reply_ref = Some(ReplyRef {
            chat_id: GROUP_LAUNCH.into(),
            project_id: "people".into(),
            message_id: ask.id.clone(),
            chat_title: "Launch week".into(),
            sender: ask.sender.clone(),
            sender_name: "Sam Rivera".into(),
            sender_handle: "sam".into(),
            text: ReplyRef::snapshot_text(&ask.plain_text()),
        });
        insert_message_via_core(&doc, &mine);
        // …and one from a chat I am not in: the quote without the link.
        let mut outside = human_message(
            DM_SAM,
            &sam(),
            "Same thing happened in ours.",
            base + 460_000,
        );
        outside.reply_ref = Some(ReplyRef {
            chat_id: "01J9NOTMYCHAT000000000000".into(),
            project_id: "people".into(),
            message_id: "01J9NOTMYMSG0000000000000".into(),
            chat_title: "Design crit".into(),
            sender: "user:gh_900".into(),
            sender_name: "Mira Kaplan".into(),
            sender_handle: "mira-k".into(),
            text: "The pricing slide still shows last quarter's tiers.".into(),
        });
        insert_message_via_core(&doc, &outside);
    }
    out.push(doc);

    // Sam's shared project: Sam wrote first, the agent answered with attribution.
    let studio = ChatDoc::new(SHARED_STUDIO_CHAT, SHARED_STUDIO);
    insert_message_via_core(
        &studio,
        &human_message(
            SHARED_STUDIO_CHAT,
            &sam(),
            "@jacob is in now. Studio: let's use this for the launch page work.",
            base + 200_000,
        ),
    );
    let mut reply = human_message(SHARED_STUDIO_CHAT, &sam(), "", base + 230_000);
    reply.role = Role::Assistant;
    reply.sender = format!("agent:{SHARED_STUDIO}");
    reply.sender_name = Some("Studio".into());
    reply.sender_icon = Some("◆".into());
    reply.sender_handle = Some("studio".into());
    reply.sender_avatar = None;
    reply.asked_by_handle = Some("sam".into());
    reply.owner_handle = Some("sam".into());
    reply.parts = vec![text_part(
        "@sam asked: let's use this for the launch page work.\n\nWelcome @jacob. The launch page draft lives in `site/launch/`; I will keep both of you posted here.",
    )];
    insert_message_via_core(&studio, &reply);
    out.push(studio);

    // The Launch week group: three people and my agent, as in the redesign mockups.
    let group = ChatDoc::new(GROUP_LAUNCH, GROUP_LAUNCH);
    // `ARBOS_FIXTURE_LONG_ROWS=<n>`: older synthetic history first — the list
    // only; header geometry is unchanged.
    if let Some(rows) = fixture_long_rows() {
        let before = base.saturating_sub(4 * 3_600_000);
        for msg in long_group_transcript(rows, before) {
            insert_message_via_core(&group, &msg);
        }
    }
    insert_message_via_core(&group, &deck);
    insert_message_via_core(&group, &ask);
    insert_message_via_core(&group, &tagged_early);
    insert_message_via_core(&group, &collective);
    insert_message_via_core(&group, &tagged_late);
    // Scout's photo (Jacob, 2026-10-27, [feedback forward-attachments-mac-2026-10-27]): an
    // agent's picture-only message in a group right before its words — it must carry the
    // same disc and name as the bubble under it.
    let mut agent_picture = human_message(GROUP_LAUNCH, &sam(), "", base + 358_000);
    agent_picture.role = Role::Assistant;
    agent_picture.sender = format!("agent:{PROJECT_ARBOS}");
    agent_picture.sender_name = Some("Arbos".into());
    agent_picture.sender_handle = Some("arbos".into());
    agent_picture.sender_avatar = None;
    agent_picture.asked_by_handle = Some("sam".into());
    agent_picture.owner_handle = Some(ME_HANDLE.into());
    agent_picture.parts = vec![attachment(
        "slide-04-signups.png",
        "image/png",
        AttachmentKind::Image,
        "Slide 4 · signups",
        212_000,
    )];
    insert_message_via_core(&group, &agent_picture);
    let mut agent = human_message(GROUP_LAUNCH, &sam(), "", base + 360_000);
    agent.role = Role::Assistant;
    agent.sender = format!("agent:{PROJECT_ARBOS}");
    agent.sender_name = Some("Arbos".into());
    agent.sender_handle = Some("arbos".into());
    agent.sender_avatar = None;
    agent.asked_by_handle = Some("sam".into());
    agent.owner_handle = Some(ME_HANDLE.into());
    // The relay threads a group reply to the asker's message (#1447), so the
    // bubble opens with a quote header — the shape of Jacob's Oct 23 "Doug"
    // screenshot (bubble-quote-width-2026-10-23).
    agent.in_reply_to = Some(ask.id.clone());
    agent.parts = vec![text_part(
        "Done. Slide 4 now reads 1,284 signups this week (+18 %). The chart is in `deck/v3/slide-04.png`.",
    )];
    insert_message_via_core(&group, &agent);
    // #1592 rig: a question to someone else's agent and its first bubble — the shape of
    // Jacob's Arbonites screenshot ("@testagent how many messages …" / "On it.").
    if fixture_foreign_agent() {
        let ask = human_message(
            GROUP_LAUNCH,
            &samir,
            "@testagent how many messages are there in this chat?",
            base + 362_000,
        );
        insert_message_via_core(&group, &ask);
        let mut reply = human_message(GROUP_LAUNCH, &samir, "", base + 364_000);
        reply.role = Role::Assistant;
        reply.sender = format!("agent:{PROJECT_TESTAGENT}");
        reply.sender_name = Some("TestAgent".into());
        reply.sender_handle = Some("testagent".into());
        reply.sender_avatar = None;
        reply.asked_by_handle = Some("samir".into());
        reply.owner_handle = Some("sami".into());
        reply.in_reply_to = Some(ask.id.clone());
        reply.parts = vec![text_part("On it.")];
        insert_message_via_core(&group, &reply);
    }
    // Two images and a file, so the group pane's Library and the attachment
    // composer have something to show (redesign mocks d / e).
    let mut shared_files = human_message(GROUP_LAUNCH, &samir, "", base + 380_000);
    shared_files.parts = vec![
        attachment(
            "deck-v3-slide-04.png",
            "image/png",
            AttachmentKind::Image,
            "Slide 4 · signups",
            212_000,
        ),
        attachment(
            "launch-dashboard.png",
            "image/png",
            AttachmentKind::Image,
            "Launch dashboard",
            348_000,
        ),
        attachment(
            "launch-day-team.jpg",
            "image/jpeg",
            AttachmentKind::Image,
            "Launch day, the team",
            1_204_000,
        ),
        attachment(
            "signup-funnel.png",
            "image/png",
            AttachmentKind::Image,
            "Signup funnel",
            156_000,
        ),
        attachment(
            "press-kit-v3.pdf",
            "application/pdf",
            AttachmentKind::Pdf,
            "Press kit v3",
            1_840_000,
        ),
        // A clip, so the media viewer's video state (poster + Play) has an item.
        attachment(
            "launch-teaser.mp4",
            "video/mp4",
            AttachmentKind::Video,
            "Launch teaser",
            6_420_000,
        ),
        text_part("Deck, dashboard and the press kit for the run-through."),
    ];
    insert_message_via_core(&group, &shared_files);
    // Two shared links, as in the iPhone fixture: the group Library's Links
    // section ([feedback group-library-links-2026-10-23]) and the link-open
    // confirm have something to show.
    insert_message_via_core(
        &group,
        &human_message(
            GROUP_LAUNCH,
            &samir,
            "Launch checklist is here: https://notion.so/arbos/launch-week — and the signup form: https://arbos.life/i/launch",
            base + 390_000,
        ),
    );
    insert_message_via_core(
        &group,
        &human_message(
            GROUP_LAUNCH,
            &sara,
            "Thanks both. I will do the run-through at 3 and post notes here.",
            base + 400_000,
        ),
    );
    // A reply the model provider refused (feedback lium-empty-reply-2026-10-21): the box
    // writes an empty text part plus the `model_refused` line; the transcript shows the line
    // in the agent's bubble instead of a bare "answering @…" header.
    insert_message_via_core(
        &group,
        &human_message(
            GROUP_LAUNCH,
            &sara,
            "@arbos can you also pull the churn numbers for slide 6?",
            base + 420_000,
        ),
    );
    let mut refused = human_message(GROUP_LAUNCH, &sara, "", base + 425_000);
    refused.role = Role::Assistant;
    refused.status = MessageStatus::Error;
    refused.sender = format!("agent:{PROJECT_ARBOS}");
    refused.sender_name = Some("Arbos".into());
    refused.sender_handle = Some("arbos".into());
    refused.sender_avatar = None;
    refused.asked_by_handle = Some("sara".into());
    refused.owner_handle = Some(ME_HANDLE.into());
    refused.parts = vec![
        text_part(""),
        Part::Error {
            id: new_id(),
            code: "model_refused".into(),
            message: "I couldn't answer that: the model provider declined the request. Ask me again in a moment.".into(),
        },
    ];
    insert_message_via_core(&group, &refused);
    // The same failure from an older box: no error part, no `senderName` — the transcript
    // resolves the name from the agent id ("Arbos didn't answer. Ask again.").
    insert_message_via_core(
        &group,
        &human_message(
            GROUP_LAUNCH,
            &samir,
            "@arbos and the press list for the embargo?",
            base + 430_000,
        ),
    );
    let mut silent = human_message(GROUP_LAUNCH, &samir, "", base + 433_000);
    silent.role = Role::Assistant;
    silent.status = MessageStatus::Error;
    silent.sender = format!("agent:{PROJECT_ARBOS}");
    silent.sender_name = None;
    silent.sender_handle = Some("arbos".into());
    silent.sender_avatar = None;
    silent.asked_by_handle = Some("samir".into());
    silent.owner_handle = Some(ME_HANDLE.into());
    silent.parts = vec![text_part("")];
    insert_message_via_core(&group, &silent);
    // Jacob's case (chat-preview-cards-2026-10-23): the agent screenshots the
    // small page it made — a white page with its own empty top margin.
    insert_message_via_core(
        &group,
        &human_message(
            GROUP_LAUNCH,
            &sam(),
            "@arbos make me a small page with a button that counts clicks.",
            base + 440_000,
        ),
    );
    let mut page = human_message(GROUP_LAUNCH, &sam(), "", base + 445_000);
    page.role = Role::Assistant;
    page.sender = format!("agent:{PROJECT_ARBOS}");
    page.sender_name = Some("Arbos".into());
    page.sender_handle = Some("arbos".into());
    page.sender_avatar = None;
    page.asked_by_handle = Some("sam".into());
    page.owner_handle = Some(ME_HANDLE.into());
    page.parts = vec![
        text_part("Here it is — a small page with a counter button."),
        attachment(
            "Screenshot 2026-10-23 at 7.55.41.png",
            "image/png",
            AttachmentKind::Image,
            "Screenshot 2026-10-23 at 7.55.41.png",
            212_000,
        ),
    ];
    insert_message_via_core(&group, &page);
    // Jacob's v0.6.184 shots (bubble-overflow-hotfix-2026-10-23): a one-line
    // message longer than the bubble cap — his own and someone else's — so the
    // rig shows whether the text wraps inside the bubble or runs out of it.
    insert_message_via_core(
        &group,
        &human_message(
            GROUP_LAUNCH,
            &sara,
            "Expected: holding my own message shows an Edit option in the menu alongside copy, reply and delete. Forward is missing here too.",
            base + 450_000,
        ),
    );
    insert_message_via_core(
        &group,
        &human_message(
            GROUP_LAUNCH,
            &me(),
            "When you think about it, most people are going to be blocked by how many tokens they can access pretty soon, so the launch page should say what a seat includes.",
            base + 452_000,
        ),
    );
    insert_message_via_core(
        &group,
        &human_message(
            GROUP_LAUNCH,
            &me(),
            "Most people cant do what I am doing right now",
            base + 455_000,
        ),
    );
    if mention_chips_seeded() {
        insert_message_via_core(
            &group,
            &human_message(
                GROUP_LAUNCH,
                &sara,
                "@arbos and @sam, the embargo lifts at 9.",
                base + 456_000,
            ),
        );
        insert_message_via_core(
            &group,
            &human_message(
                GROUP_LAUNCH,
                &me(),
                "Thanks @sara. @arbos, ask `@sam` for the list.",
                base + 457_000,
            ),
        );
    }
    out.push(group);
    if emoji_names_seeded() {
        let dm = ChatDoc::new(DM_MAGE, "people");
        insert_message_via_core(
            &dm,
            &human_message(
                DM_MAGE,
                &mage(),
                "Or even better, a wizard hat for everyone at launch.",
                base + 500_000,
            ),
        );
        out.push(dm);
        let group = ChatDoc::new(GROUP_EMOJI, GROUP_EMOJI);
        insert_message_via_core(
            &group,
            &human_message(GROUP_EMOJI, &mage(), "hello from the mage", base + 510_000),
        );
        insert_message_via_core(
            &group,
            &human_message(
                GROUP_EMOJI,
                &cato(),
                "Trees for everyone, then.",
                base + 520_000,
            ),
        );
        out.push(group);
    }
    out
}

/// True when `lower` has `@token` as a whole mention (not a longer `@token…` handle).
fn text_has_at_token(lower: &str, token: &str) -> bool {
    let t = format!("@{token}");
    lower.match_indices(&t).any(|(i, _)| {
        let after = lower[i + t.len()..].chars().next();
        after.is_none_or(|c| !(c.is_alphanumeric() || c == '-' || c == '_'))
    })
}

/// `@slug` / `@handle` tokens in the text, resolved to member ids.
/// Group owners may also use `@here` (every human) and `@agents` (every agent).
fn mentions_in(space: &Project, text: &str, tokens: &[String]) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut out = crate::sync::expand_collectives(space, &lower, tokens, ME_ID);
    let claimed = crate::sync::claimed_handles(space, tokens);
    for a in &space.agents {
        let handle = a.handle.to_lowercase();
        let slug = Project::mention_slug(&a.name);
        if claimed.contains(&handle) || claimed.contains(&slug) {
            continue;
        }
        if (!handle.is_empty() && text_has_at_token(&lower, &handle))
            || (!slug.is_empty() && text_has_at_token(&lower, &slug))
        {
            let id = format!("agent:{}", a.project_id);
            if !out.iter().any(|x| x == &id) {
                out.push(id);
            }
        }
    }
    for p in &space.people {
        let handle = p.handle.to_lowercase();
        if claimed.contains(&handle) {
            continue;
        }
        if lower.contains(&format!("@{handle}")) {
            let id = format!("user:{}", p.id);
            if !out.iter().any(|x| x == &id) {
                out.push(id);
            }
        }
    }
    out
}

fn people_space_of(shared: &Shared, chat_id: &str) -> Option<Project> {
    let projects = shared.projects.lock().unwrap();
    projects
        .iter()
        .find(|p| p.is_people() && p.id == chat_id)
        .cloned()
}

/// A tiny two-tone PNG avatar so the image path is exercised (`image` crate).
fn avatar_png(seed: &str) -> Vec<u8> {
    let hash = seed
        .bytes()
        .fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    let (r, g, b) = (
        120 + (hash % 100) as u8,
        90 + ((hash >> 8) % 120) as u8,
        140 + ((hash >> 16) % 100) as u8,
    );
    let size = 64u32;
    let mut img = image::RgbaImage::new(size, size);
    let c = (size as f32 - 1.0) / 2.0;
    for (x, y, px) in img.enumerate_pixels_mut() {
        let dx = x as f32 - c;
        let dy = y as f32 - c;
        let d = (dx * dx + dy * dy).sqrt();
        if d > c {
            *px = image::Rgba([0, 0, 0, 0]);
        } else if (dx + dy) > 0.0 {
            *px = image::Rgba([r, g, b, 255]);
        } else {
            *px = image::Rgba([r / 2 + 40, g / 2 + 40, b / 2 + 40, 255]);
        }
    }
    let mut out = std::io::Cursor::new(Vec::new());
    let _ = img.write_to(&mut out, image::ImageFormat::Png);
    out.into_inner()
}

/// The Create sheet's pending picture as a seeded fixture URL (the same seed the sheet
/// previewed for a generated one, so the new entity shows the picture it was created with).
fn draft_picture_url(kind: &str, picture: &DraftPicture) -> String {
    match picture {
        DraftPicture::File(path) => format!(
            "fixture://avatar/{kind}-{}",
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("picture")
        ),
        DraftPicture::Generated { key } => format!("fixture://avatar/draft-{key}"),
    }
}

fn emit_avatar(shared: &Shared, url: &str) {
    // A picked file (`file://…`, the picker's Upload preview) is mastered like the relay client
    // does, so the fixture rig shows the real picture at upload quality; everything else is
    // the seeded placeholder.
    if let Some(path) = url.strip_prefix("file://") {
        if let Some(bytes) = std::fs::read(path)
            .ok()
            .and_then(|b| super::avatar_master::master(&b).ok())
            .map(|m| m.bytes)
        {
            let _ = shared.events.send(SyncEvent::Avatar {
                url: url.to_string(),
                bytes: Arc::new(bytes),
            });
        }
        return;
    }
    let seed = url.rsplit('/').next().unwrap_or(url).to_string();
    let _ = shared.events.send(SyncEvent::Avatar {
        url: url.to_string(),
        bytes: Arc::new(avatar_png(&seed)),
    });
}

fn emit_people_seed(shared: &Shared) {
    for p in directory() {
        emit_avatar(shared, &p.avatar_url);
    }
    emit_avatar(shared, &me().avatar_url);
}

/// The fixture's directory has no such GitHub user: the relay's own words, as a refusal.
fn no_such_user(verb: &str, handle: &str) -> UserError {
    UserError::of(
        verb,
        ErrorClass::Refused {
            message: Some(format!("No GitHub user named @{handle}.")),
        },
        "fixture",
    )
}

fn handle_people_command(shared: &Arc<Shared>, cmd: &ClientCommand) -> bool {
    match cmd {
        ClientCommand::SearchPeople { query } => {
            let q = query.trim().trim_start_matches('@').to_lowercase();
            let results: Vec<Person> = if q.is_empty() {
                directory()
            } else {
                directory()
                    .into_iter()
                    .filter(|p| {
                        p.handle.starts_with(&q)
                            || p.name
                                .as_deref()
                                .map(|n| n.to_lowercase().contains(&q))
                                .unwrap_or(false)
                    })
                    .take(10)
                    .collect()
            };
            let shared = shared.clone();
            let query = query.clone();
            tokio::spawn(async move {
                sleep(Duration::from_millis(180)).await;
                let _ = shared
                    .events
                    .send(SyncEvent::PeopleSearch { query, results });
            });
            true
        }
        ClientCommand::StartDm { handle } => {
            let handle = handle.trim().trim_start_matches('@').to_lowercase();
            if handle == ME_HANDLE {
                let _ = shared.events.send(SyncEvent::PeopleError(UserError::of(
                    "start the chat",
                    ErrorClass::Refused {
                        message: Some("That is you.".into()),
                    },
                    "fixture",
                )));
                return true;
            }
            let existing = shared
                .projects
                .lock()
                .unwrap()
                .iter()
                .find(|p| p.is_people() && p.person().map(|x| x.handle == handle).unwrap_or(false))
                .map(|p| p.id.clone());
            if existing.is_some() {
                emit_projects(shared);
                return true;
            }
            let Some(other) = directory().into_iter().find(|p| p.handle == handle) else {
                let _ = shared.events.send(SyncEvent::PeopleError(no_such_user(
                    "start the chat",
                    &handle,
                )));
                return true;
            };
            let id = new_id();
            let pending = other.status == "invited";
            shared
                .projects
                .lock()
                .unwrap()
                .push(people_space(&id, other.clone(), Vec::new()));
            shared.chats.lock().unwrap().push(Chat {
                id: id.clone(),
                project_id: id.clone(),
                title: other.handle.clone(),
                is_pinned: true,
                is_system: false,
                kind: ChatKind::Chat,
                participants: Vec::new(),
                unread: false,
                unread_count: 0,
                mention_count: 0,
                last_message_at: now_ms(),
                archived_at: None,
            });
            add_doc(shared, ChatDoc::new(&id, "people"));
            emit_projects(shared);
            emit_chats(shared);
            if pending {
                let _ = shared.events.send(SyncEvent::Notice(format!(
                    "@{} has not signed in yet; the chat opens for them when they do.",
                    other.handle
                )));
            }
            true
        }
        ClientCommand::CreatePeopleGroup {
            handles,
            agent_ids,
            title: draft_title,
            leaf,
            picture,
        } => {
            let mut members: Vec<Person> = Vec::new();
            for raw in handles {
                let handle = raw.trim().trim_start_matches('@').to_lowercase();
                if handle.is_empty() || handle == ME_HANDLE {
                    continue;
                }
                if members.iter().any(|p| p.handle == handle) {
                    continue;
                }
                let Some(other) = directory().into_iter().find(|p| p.handle == handle) else {
                    let _ = shared.events.send(SyncEvent::PeopleError(no_such_user(
                        "create the group",
                        &handle,
                    )));
                    return true;
                };
                members.push(other);
            }
            let agents: Vec<AgentMember> = {
                let projects = shared.projects.lock().unwrap();
                agent_ids
                    .iter()
                    .filter_map(|pid| {
                        projects
                            .iter()
                            .find(|p| p.id == *pid && !p.is_people())
                            .map(|p| AgentMember {
                                project_id: p.id.clone(),
                                name: p.name.clone(),
                                icon: p.icon.clone(),
                                color: p.color.clone(),
                                handle: Project::mention_slug(&p.name),
                                owner_id: ME_ID.into(),
                                owner_handle: ME_HANDLE.into(),
                                reply_policy: ReplyPolicy::Mention,
                                avatar: p.avatar.clone(),
                                avatar_color: p.avatar_color.clone(),
                                avatar_color_index: p.avatar_color_index,
                                avatar_leaf: p.avatar_leaf,
                                avatar_leaf_color: p.avatar_leaf_color,
                                image_url: None,
                                avatar_version: 0,
                                description: None,
                                model: None,
                                owner: None,
                            })
                    })
                    .collect()
            };
            // People, agents, or both — never nobody (the relay's rule).
            if members.is_empty() && agents.is_empty() {
                let _ = shared.events.send(SyncEvent::PeopleError(UserError::of(
                    "create the group",
                    ErrorClass::Refused {
                        message: Some("Pick at least one person or agent".into()),
                    },
                    "fixture",
                )));
                return true;
            }
            let id = new_id();
            // The typed name, else the people's handles, else the agents'
            // (a group of agents only).
            let title = draft_title
                .clone()
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| {
                    if members.is_empty() {
                        agents
                            .iter()
                            .map(|a| a.handle.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    } else {
                        members
                            .iter()
                            .map(|p| p.handle.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                });
            let seed = members.first().cloned().unwrap_or_else(|| Person {
                handle: title.clone(),
                ..Person::default()
            });
            let mut space = people_space(&id, seed, agents);
            space.kind = SpaceKind::Group;
            space.name = title.clone();
            space.avatar_leaf = leaf.as_ref().map(|l| l.leaf);
            space.avatar_leaf_color = leaf.as_ref().map(|l| l.color);
            if let Some(url) = picture.as_ref().map(|p| draft_picture_url("group", p)) {
                emit_avatar(shared, &url);
                space.image_url = Some(url);
            }
            space.people = members;
            space.pending = space.people.iter().any(|p| p.status == "invited");
            space.owner_handle = Some(ME_HANDLE.into());
            space.my_role = Some("owner".into());
            shared.projects.lock().unwrap().push(space);
            shared.chats.lock().unwrap().push(Chat {
                id: id.clone(),
                project_id: id.clone(),
                title,
                is_pinned: true,
                is_system: false,
                kind: ChatKind::Chat,
                participants: Vec::new(),
                unread: false,
                unread_count: 0,
                mention_count: 0,
                last_message_at: now_ms(),
                archived_at: None,
            });
            add_doc(shared, ChatDoc::new(&id, "people"));
            emit_projects(shared);
            emit_chats(shared);
            // Like the relay: the id arrives after the list that carries it.
            let _ = shared
                .events
                .send(SyncEvent::PeopleGroupCreated { chat_id: id });
            true
        }
        ClientCommand::SetTyping { .. } => true,
        ClientCommand::RemoveAgent {
            chat_id,
            project_id,
        } => {
            let removed = {
                let mut projects = shared.projects.lock().unwrap();
                projects
                    .iter_mut()
                    .find(|p| p.id == *chat_id)
                    .and_then(|space| {
                        let gone = space
                            .agents
                            .iter()
                            .find(|a| a.project_id == *project_id)
                            .map(|a| a.name.clone());
                        space.agents.retain(|a| a.project_id != *project_id);
                        gone
                    })
            };
            if let Some(name) = removed {
                membership_notice(shared, chat_id, &format!("{} removed {name}", ME_NAME));
            }
            emit_projects(shared);
            true
        }
        ClientCommand::AddMembers {
            chat_id,
            handles,
            agent_ids,
        } => {
            let mut projects = shared.projects.lock().unwrap();
            let agents: Vec<AgentMember> = agent_ids
                .iter()
                .filter_map(|pid| {
                    projects
                        .iter()
                        .find(|p| p.id == *pid && !p.is_people())
                        .map(|p| AgentMember {
                            project_id: p.id.clone(),
                            name: p.name.clone(),
                            icon: p.icon.clone(),
                            color: p.color.clone(),
                            handle: Project::mention_slug(&p.name),
                            owner_id: ME_ID.into(),
                            owner_handle: ME_HANDLE.into(),
                            reply_policy: ReplyPolicy::Mention,
                            avatar: p.avatar.clone(),
                            avatar_color: p.avatar_color.clone(),
                            avatar_color_index: p.avatar_color_index,
                            avatar_leaf: p.avatar_leaf,
                            avatar_leaf_color: p.avatar_leaf_color,
                            image_url: None,
                            avatar_version: 0,
                            description: None,
                            model: None,
                            owner: None,
                        })
                })
                .collect();
            let mut names: Vec<String> = Vec::new();
            if let Some(space) = projects.iter_mut().find(|p| p.id == *chat_id) {
                for raw in handles {
                    let handle = raw.trim().trim_start_matches('@').to_lowercase();
                    let Some(other) = directory().into_iter().find(|p| p.handle == handle) else {
                        continue;
                    };
                    if !space.people.iter().any(|x| x.id == other.id) {
                        names.push(other.display_name());
                        space.people.push(other);
                    }
                }
                for agent in agents {
                    if !space.has_agent(&agent.project_id) {
                        names.push(agent.name.clone());
                        space.agents.push(agent);
                    }
                }
            }
            drop(projects);
            // Like the relay (#1349): one "<me> added A, B and C" line per batch.
            membership_notice(shared, chat_id, &added_line_text(&names));
            emit_projects(shared);
            true
        }
        ClientCommand::RemoveMember { chat_id, user_id } => {
            let removed = {
                let mut projects = shared.projects.lock().unwrap();
                projects
                    .iter_mut()
                    .find(|p| p.id == *chat_id)
                    .and_then(|p| {
                        let gone = p
                            .people
                            .iter()
                            .find(|x| x.id == *user_id)
                            .map(|x| x.display_name());
                        p.people.retain(|x| x.id != *user_id);
                        gone
                    })
            };
            if let Some(name) = removed {
                membership_notice(shared, chat_id, &format!("{} removed {name}", ME_NAME));
            }
            emit_projects(shared);
            true
        }
        ClientCommand::SetMemberRole {
            chat_id,
            user_id,
            role,
        } => {
            if let Some(m) = shared
                .projects
                .lock()
                .unwrap()
                .iter_mut()
                .find(|p| p.id == *chat_id)
                .and_then(|p| p.people.iter_mut().find(|x| x.id == *user_id))
                .filter(|m| m.role != "owner")
            {
                m.role = role.clone();
            }
            emit_projects(shared);
            true
        }
        ClientCommand::LeaveChat { chat_id } => {
            shared.projects.lock().unwrap().retain(|p| p.id != *chat_id);
            shared
                .chats
                .lock()
                .unwrap()
                .retain(|c| c.project_id != *chat_id);
            emit_projects(shared);
            emit_chats(shared);
            true
        }
        ClientCommand::BlockUser { handle } => {
            let handle = handle.trim().trim_start_matches('@').to_lowercase();
            let who = directory()
                .into_iter()
                .find(|p| p.handle == handle)
                .unwrap_or_else(|| person("", &handle, None, "member"));
            {
                let mut blocked = shared.blocked.lock().unwrap();
                if !blocked.iter().any(|p| p.handle == who.handle) {
                    blocked.push(who);
                }
            }
            let _ = shared
                .events
                .send(SyncEvent::Notice(format!("@{handle} blocked.")));
            emit_blocked(shared);
            true
        }
        ClientCommand::UnblockUser { user_id, handle } => {
            shared
                .blocked
                .lock()
                .unwrap()
                .retain(|p| p.id != *user_id && p.handle != *handle);
            let _ = shared
                .events
                .send(SyncEvent::Notice(format!("@{handle} unblocked.")));
            emit_blocked(shared);
            true
        }
        // "Who may message X" (agent-to-agent messaging): the grant list lives on the project.
        ClientCommand::SetMessageGrant {
            project_id,
            grantee,
            enabled,
        } => {
            let mut projects = shared.projects.lock().unwrap();
            if let Some(p) = projects.iter_mut().find(|p| p.id == *project_id) {
                if let Some(g) = p.message_grants.iter_mut().find(|g| g.grantee == *grantee) {
                    g.enabled = *enabled;
                } else {
                    let handle = grantee.trim_start_matches('@').to_lowercase();
                    let Some(other) = directory().into_iter().find(|p| p.handle == handle) else {
                        drop(projects);
                        let _ = shared
                            .events
                            .send(SyncEvent::PeopleError(no_such_user("give access", &handle)));
                        return true;
                    };
                    p.message_grants.push(crate::model::MessageGrant {
                        grantee: format!("user:{}", other.id),
                        kind: "person".into(),
                        id: other.id.clone(),
                        name: other.name.clone(),
                        handle: Some(other.handle.clone()),
                        avatar_url: other.avatar_url.clone(),
                        enabled: *enabled,
                        note: String::new(),
                        created_at: now_ms() as i64,
                        last_at: None,
                    });
                }
            }
            drop(projects);
            emit_projects(shared);
            true
        }
        ClientCommand::RemoveMessageGrant {
            project_id,
            grantee,
        } => {
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == *project_id) {
                    p.message_grants.retain(|g| g.grantee != *grantee);
                }
            }
            emit_projects(shared);
            true
        }
        ClientCommand::InviteToProject { project_id, handle } => {
            let handle = handle.trim().trim_start_matches('@').to_lowercase();
            let Some(other) = directory().into_iter().find(|p| p.handle == handle) else {
                let _ = shared
                    .events
                    .send(SyncEvent::PeopleError(no_such_user("invite them", &handle)));
                return true;
            };
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == *project_id) {
                    if !p.people.iter().any(|x| x.id == other.id) {
                        p.people.push(other.clone());
                    }
                }
            }
            emit_projects(shared);
            // The invitee shows up in the project's pinned chat and asks the agent.
            let pinned = shared
                .chats
                .lock()
                .unwrap()
                .iter()
                .find(|c| c.project_id == *project_id && c.is_pinned)
                .map(|c| c.id.clone());
            if let Some(chat_id) = pinned {
                let shared = shared.clone();
                let project_id = project_id.clone();
                tokio::spawn(async move {
                    sleep(Duration::from_millis(2200)).await;
                    let ask = "Thanks for the invite — what is the state of the beta invite copy?";
                    let msg = human_message(&chat_id, &other, ask, now_ms());
                    with_doc(&shared, &chat_id, |doc| insert_message_via_core(doc, &msg));
                    mark_unread_if_hidden(&shared, &chat_id);
                    let agent = {
                        let projects = shared.projects.lock().unwrap();
                        projects
                            .iter()
                            .find(|p| p.id == project_id)
                            .map(|p| AgentMember {
                                project_id: p.id.clone(),
                                name: p.name.clone(),
                                icon: p.icon.clone(),
                                color: p.color.clone(),
                                handle: Project::mention_slug(&p.name),
                                owner_id: ME_ID.into(),
                                owner_handle: ME_HANDLE.into(),
                                reply_policy: ReplyPolicy::EveryMessage,
                                avatar: p.avatar.clone(),
                                avatar_color: p.avatar_color.clone(),
                                avatar_color_index: p.avatar_color_index,
                                avatar_leaf: p.avatar_leaf,
                                avatar_leaf_color: p.avatar_leaf_color,
                                image_url: None,
                                avatar_version: 0,
                                description: None,
                                model: None,
                                owner: None,
                            })
                    };
                    if let Some(agent) = agent {
                        agent_reply(&shared, &chat_id, &agent, &other, ask).await;
                    }
                });
            }
            true
        }
        ClientCommand::RemoveProjectMember {
            project_id,
            user_id,
        } => {
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == *project_id) {
                    p.people.retain(|x| x.id != *user_id);
                }
            }
            emit_projects(shared);
            true
        }
        ClientCommand::FetchAvatar { url } => {
            emit_avatar(shared, url);
            true
        }
        ClientCommand::RenameGroup { chat_id, title } => {
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == *chat_id) {
                    p.name = title.clone();
                }
            }
            {
                let mut chats = shared.chats.lock().unwrap();
                if let Some(c) = chats.iter_mut().find(|c| c.id == *chat_id) {
                    c.title = title.clone();
                }
            }
            emit_projects(shared);
            emit_chats(shared);
            true
        }
        ClientCommand::SetGroupImage { chat_id, path } => {
            // The demo has no blob store: any picked file becomes a seeded picture.
            let url = format!(
                "fixture://avatar/group-{}",
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("picture")
            );
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == *chat_id) {
                    p.image_url = Some(url.clone());
                }
            }
            emit_avatar(shared, &url);
            emit_projects(shared);
            true
        }
        ClientCommand::SetGroupLeaf { chat_id, pick } => {
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == *chat_id) {
                    p.avatar_leaf = pick.map(|l| l.leaf);
                    p.avatar_leaf_color = pick.map(|l| l.color);
                    p.image_url = None;
                }
            }
            emit_projects(shared);
            true
        }
        ClientCommand::SetProjectImage { project_id, source } => {
            // No blob store in the demo: the picked file (or the "generated" key) becomes a
            // seeded picture, like a group's.
            let seed = match source {
                PictureSource::File(path) => path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("picture")
                    .to_owned(),
                PictureSource::Key(key) => key.clone(),
            };
            let url = format!("fixture://avatar/agent-{seed}");
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == *project_id) {
                    p.image_url = Some(url.clone());
                }
            }
            emit_avatar(shared, &url);
            emit_projects(shared);
            true
        }
        ClientCommand::GenerateImage { target, prompt } => {
            // The demo "generates" after a short wait: a seeded picture named after the prompt.
            let key = format!("gen-{}", prompt.trim().to_lowercase().replace(' ', "-"));
            // My own picture keeps the `me-` prefix `has_custom_avatar` reads.
            let url = match target {
                PictureTarget::Me(_) => format!("fixture://avatar/me-{key}"),
                _ => {
                    let owner = target.id().map(String::as_str).unwrap_or("draft");
                    format!("fixture://avatar/{owner}-{key}")
                }
            };
            let shared = shared.clone();
            let target = target.clone();
            // Rig (error-state shots): a prompt with "timeout" answers the relay's 504 sentence
            // after 2 s; one with "slow" paints for 25 s first (the progress state).
            let lower = prompt.to_lowercase();
            let fail = lower.contains("timeout");
            let wait_ms = if lower.contains("slow") {
                25_000
            } else if fail {
                2_000
            } else {
                1_400
            };
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;
                if fail {
                    let _ = shared.events.send(SyncEvent::ImageGenerated {
                        target,
                        image_key: None,
                        url: None,
                        error: Some(UserError::of(
                            "generate the image",
                            ErrorClass::Timeout,
                            "HTTP 504 avatar_timeout — gpt-image-1-mini: operation timed out after 40 s (attempt 2 of 2); relay budget 75 s",
                        )),
                    });
                    return;
                }
                emit_avatar(&shared, &url);
                let _ = shared.events.send(SyncEvent::ImageGenerated {
                    target,
                    image_key: Some(key),
                    url: Some(url),
                    error: None,
                });
            });
            true
        }
        ClientCommand::SetGroupImageKey { chat_id, key } => {
            let url = format!("fixture://avatar/{chat_id}-{key}");
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == *chat_id) {
                    p.image_url = Some(url.clone());
                }
            }
            emit_avatar(shared, &url);
            emit_projects(shared);
            true
        }
        ClientCommand::SetServerImage { .. } | ClientCommand::SetServerLeaf { .. } => true,
        _ => false,
    }
}

/// Which shared agent a human message addresses (API §4 routing, client side
/// of the fixture): explicit mention, `@slug`, reply-to, `@arbos` with one agent.
fn addressed_agent(space: &Project, text: &str, mentions: &[String]) -> Option<AgentMember> {
    let lower = text.to_lowercase();
    space
        .agents
        .iter()
        .find(|a| {
            mentions
                .iter()
                .any(|m| *m == format!("agent:{}", a.project_id))
                || lower.contains(&format!("@{}", a.handle))
                || a.reply_policy == ReplyPolicy::EveryMessage
        })
        .cloned()
        .or_else(|| {
            if space.agents.len() == 1 && lower.contains("@arbos") {
                space.agents.first().cloned()
            } else {
                None
            }
        })
}

/// After I send in a people chat: an addressed agent answers (quoting the
/// asker), otherwise Sam types for a moment and replies.
fn people_after_send(
    shared: &Arc<Shared>,
    space: Project,
    chat_id: Id,
    text: String,
    mentions: Vec<String>,
) {
    let shared = shared.clone();
    tokio::spawn(async move {
        sleep(Duration::from_millis(250)).await;
        if let Some(agent) = addressed_agent(&space, &text, &mentions) {
            agent_reply(&shared, &chat_id, &agent, &me(), &text).await;
            return;
        }
        let Some(other) = space.person().cloned() else {
            return;
        };
        if other.status == "invited" {
            return;
        }
        let _ = shared.events.send(SyncEvent::Typing {
            chat_id: chat_id.clone(),
            handle: other.handle.clone(),
            active: true,
        });
        sleep(Duration::from_millis(1800)).await;
        let _ = shared.events.send(SyncEvent::Typing {
            chat_id: chat_id.clone(),
            handle: other.handle.clone(),
            active: false,
        });
        let reply = sam_reply(&text, &space);
        let msg = human_message(&chat_id, &other, &reply, now_ms());
        with_doc(&shared, &chat_id, |doc| insert_message_via_core(doc, &msg));
        mark_unread_if_hidden(&shared, &chat_id);
        // Sam also asks the shared agent once it is in the chat.
        if let Some(agent) = space.agents.first().cloned() {
            if text.to_lowercase().contains("ask") {
                sleep(Duration::from_millis(900)).await;
                let ask = format!(
                    "@{} what did the beta invite copy change last week?",
                    agent.handle
                );
                let msg = human_message(&chat_id, &other, &ask, now_ms());
                with_doc(&shared, &chat_id, |doc| insert_message_via_core(doc, &msg));
                agent_reply(&shared, &chat_id, &agent, &other, &ask).await;
            }
        }
    });
}

fn sam_reply(text: &str, space: &Project) -> String {
    let lower = text.to_lowercase();
    if lower.contains("?") {
        "Yes — I think so. Let me check the doc and come back with the exact line.".into()
    } else if space.agents.is_empty() && lower.contains("agent") {
        "Please do, I'd rather ask it here than switch windows.".into()
    } else {
        "Got it. I'll fold that into the draft and ping you when it's up.".into()
    }
}

/// A shared agent's reply: prose with "@handle asked:" attribution, streamed.
async fn agent_reply(
    shared: &Arc<Shared>,
    chat_id: &str,
    agent: &AgentMember,
    asker: &Person,
    ask: &str,
) {
    let id = new_id();
    let reading = with_doc(shared, chat_id, |doc| {
        doc.core.transact(|d| {
            begin_assistant_row(
                d,
                &id,
                &format!("agent:{}", agent.project_id),
                &[
                    ("senderName", json!(agent.name)),
                    ("senderIcon", json!(agent.icon)),
                    ("senderHandle", json!(agent.handle)),
                    ("askedByHandle", json!(asker.handle)),
                    ("ownerHandle", json!(agent.owner_handle)),
                ],
                StatusKind::Deciding,
                &format!("{} is reading…", agent.name),
            )
        })
    })
    .unwrap_or_default();
    sleep(Duration::from_millis(600)).await;
    let text = with_doc(shared, chat_id, |doc| {
        doc.core.transact(|d| {
            d.set_part_fields(&id, &reading, &[("hidden", json!(true))]);
            push_part(d, &id, &text_part(""))
        })
    })
    .unwrap_or_default();
    let short = ask.trim().trim_end_matches('.');
    let quoted = format!(
        "@{} asked: {}",
        asker.handle,
        if short.chars().count() > 60 {
            format!("{}…", short.chars().take(60).collect::<String>())
        } else {
            short.to_string()
        }
    );
    let body = if ask.to_lowercase().contains("copy") || ask.to_lowercase().contains("invite") {
        "Last week's change to the beta invite copy was the second paragraph: the old line promised \"instant setup\"; the new one says \"your machine is ready in about a minute\". The CTA text did not change."
    } else if ask.to_lowercase().contains("number") || ask.to_lowercase().contains("metric") {
        "Pulled from the workspace: 1,284 invites sent, 412 accepted (32%), median first message 4 m 10 s after sign-in."
    } else {
        "On it. I checked the workspace and nothing blocks this on my side; say the word and I will draft it."
    };
    let stop = AtomicBool::new(false);
    stream_blocks(shared, chat_id, &id, &text, &[&quoted, body], &stop).await;
    with_doc(shared, chat_id, |doc| {
        set_status(&doc.core, &id, MessageStatus::Done);
    });
    mark_unread_if_hidden(shared, chat_id);
}

// ---------------------------------------------------------------------------
// Calls v1 fixtures
// ---------------------------------------------------------------------------

fn sender_of(p: &Person) -> Sender {
    Sender {
        kind: "user".into(),
        id: p.id.clone(),
        name: p.name.clone().unwrap_or_default(),
        handle: p.handle.clone(),
        avatar: None,
        avatar_color: None,
        avatar_color_index: None,
        avatar_leaf: None,
        avatar_leaf_color: None,
        avatar_url: p.avatar_url.clone(),
    }
}

/// `ARBOS_FIXTURE_LIVEKIT_URL` (default the dev server), identity
/// `ARBOS_FIXTURE_IDENTITY` (so two fixture instances can share a room).
fn fixture_call(shared: &Arc<Shared>, chat_id: &str, kind: CallKind) -> Call {
    let url =
        std::env::var("ARBOS_FIXTURE_LIVEKIT_URL").unwrap_or_else(|_| "ws://127.0.0.1:7880".into());
    let identity = std::env::var("ARBOS_FIXTURE_IDENTITY").unwrap_or_else(|_| format!("u_{ME_ID}"));
    let me = me();
    let room = format!("call_fixture_{chat_id}");
    let metadata = serde_json::to_string(&sender_of(&me)).unwrap_or_default();
    // An agent chat's call belongs to its project (the §11 badge names the
    // agent from it); people chats stay in the `people` space.
    let project_id = shared
        .chats
        .lock()
        .unwrap()
        .iter()
        .find(|c| c.id == chat_id)
        .map(|c| c.project_id.clone())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "people".into());
    Call {
        call_id: room.clone(),
        chat_id: chat_id.to_string(),
        project_id,
        kind,
        room: room.clone(),
        livekit_url: url,
        token: Some(dev_token(&room, &identity, &me.display_name(), &metadata)),
        token_expires_at: Some(now_ms() + 3_600_000),
        started_by: sender_of(&me),
        started_at: now_ms(),
        ended_at: None,
        reason: None,
        participants: vec![sender_of(&me)],
        agents: Vec::new(),
        speak_mode: None,
        invited_speakers: Vec::new(),
        can_speak: Some(true),
    }
}

/// LiveKit access token for `livekit-server --dev` (API key `devkey`, secret
/// `secret`): HS256 JWT with the video grant. Dev only.
fn dev_token(room: &str, identity: &str, name: &str, metadata: &str) -> String {
    use base64::Engine as _;
    use sha2::{Digest, Sha256};
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let now = now_ms() / 1000;
    let header = b64.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let claims = json!({
        "iss": "devkey",
        "sub": identity,
        "name": name,
        "metadata": metadata,
        "nbf": now - 10,
        "exp": now + 3600,
        "video": { "roomJoin": true, "room": room, "canPublish": true, "canSubscribe": true, "canPublishData": true },
    });
    let payload = b64.encode(claims.to_string());
    let signing = format!("{header}.{payload}");
    // HMAC-SHA256 by hand (one use; no extra crate).
    let key = b"secret";
    let mut k = [0u8; 64];
    k[..key.len()].copy_from_slice(key);
    let ipad: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    let opad: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    let inner = Sha256::digest([ipad.as_slice(), signing.as_bytes()].concat());
    let mac = Sha256::digest([opad.as_slice(), inner.as_slice()].concat());
    format!("{signing}.{}", b64.encode(mac))
}

#[cfg(test)]
mod seed_parity_tests {
    use super::*;

    fn part_sig(part: &Part) -> String {
        let kind = serde_json::to_value(part)
            .ok()
            .and_then(|v| v.get("kind").and_then(|k| k.as_str()).map(str::to_owned))
            .unwrap_or_else(|| "?".into());
        match part {
            Part::Question { state, .. } => format!("{kind}:{state:?}"),
            Part::Approval { state, .. } => format!("{kind}:{state:?}"),
            Part::Tool { status, .. } => format!("{kind}:{status:?}"),
            Part::Status { code, .. } => format!("{kind}:{code:?}"),
            Part::Attachment { attachment, .. } => format!(
                "{kind}:{}:{:?}:{:?}",
                attachment.title(),
                attachment.bytes,
                attachment.kind()
            ),
            _ => kind,
        }
    }

    /// `t0`: the doc's first `createdAt`, so seeded times compare without `now`.
    fn message_sig(m: &Message, t0: u64) -> String {
        let kinds: Vec<String> = m.parts.iter().map(part_sig).collect();
        let hidden: Vec<String> = m.hidden_parts.iter().map(part_sig).collect();
        let reactions: usize = m.reactions.values().map(|b| b.len()).sum();
        format!(
            "{:?} {:?} {} name={:?} h={:?} asked={:?} turn={:?} wa={} [{}] hidden=[{}] text={} r={} dt={} ct={:?}",
            m.role,
            m.status,
            m.sender,
            m.sender_name,
            m.sender_handle,
            m.asked_by_handle,
            m.turn,
            m.waiting_approval,
            kinds.join(","),
            hidden.join(","),
            m.plain_text().chars().count(),
            reactions,
            m.created_at.saturating_sub(t0),
            m.completed_at.map(|c| c.saturating_sub(m.created_at)),
        )
    }

    fn table() -> Vec<(String, Vec<String>)> {
        seed_chat_docs()
            .into_iter()
            .chain(seed_people_chat_docs())
            .map(|doc| {
                let messages = doc.read_all();
                let t0 = messages.first().map(|m| m.created_at).unwrap_or(0);
                let rows = messages.iter().map(|m| message_sig(m, t0)).collect();
                (doc.chat_id.clone(), rows)
            })
            .collect()
    }

    /// Recorded from the seed written by the Mac-side `insert_message` writer, before the
    /// fixture moved to the `arbos_core` writers: same chats, rows, part kinds, statuses.
    #[rustfmt::skip]
    const EXPECTED: &[(&str, &[&str])] = &[
        ("01J9CHAT0ARBOS000000000000", &[
            "User Done user name=None h=None asked=None turn=None wa=false [text] hidden=[] text=33 r=0 dt=0 ct=Some(0)",
            "Assistant Done agent name=None h=None asked=None turn=Some(Reply) wa=false [text] hidden=[] text=93 r=0 dt=20000 ct=Some(9000)",
            "User Done user name=None h=None asked=None turn=None wa=false [text] hidden=[] text=15 r=0 dt=65000 ct=Some(0)",
            "Assistant Done agent name=None h=None asked=None turn=Some(Reply) wa=false [text] hidden=[] text=72 r=0 dt=85000 ct=Some(9000)",
        ]),
        ("01J9CHAT0RESEARCH000000000", &[
            "User Done user name=None h=None asked=None turn=None wa=false [text] hidden=[] text=63 r=0 dt=0 ct=Some(0)",
            "Assistant Done agent name=None h=None asked=None turn=Some(Act) wa=false [tool:Some(Ok),tool:Some(Ok),tool:Some(Ok),diff,tool:Some(Ok),text] hidden=[] text=111 r=0 dt=20000 ct=Some(9000)",
            "User Done user name=None h=None asked=None turn=None wa=false [text] hidden=[] text=63 r=0 dt=65000 ct=Some(0)",
            "Assistant Done agent name=None h=None asked=None turn=Some(Spawn) wa=false [status:Working,tool:Some(Ok),browser,file,text] hidden=[] text=111 r=0 dt=85000 ct=Some(9000)",
        ]),
        ("01J9CHAT0LAUNCH00000000000", &[
            "User Done user name=None h=None asked=None turn=None wa=false [text] hidden=[] text=37 r=0 dt=0 ct=Some(0)",
            "Assistant Done agent name=None h=None asked=None turn=Some(Reply) wa=false [text,status:AskUser] hidden=[] text=31 r=0 dt=20000 ct=Some(9000)",
        ]),
        ("01J9CHAT0WIDGETS0000000000", &[
            "User Done user name=None h=None asked=None turn=None wa=false [text] hidden=[] text=34 r=0 dt=0 ct=Some(0)",
            "Assistant Done agent name=None h=None asked=None turn=Some(Reply) wa=false [text,question:Answered] hidden=[] text=26 r=0 dt=20000 ct=Some(9000)",
            "User Done user name=None h=None asked=None turn=None wa=false [text] hidden=[] text=12 r=0 dt=65000 ct=Some(0)",
            "Assistant Done agent name=None h=None asked=None turn=Some(Reply) wa=false [text,question:Open] hidden=[] text=57 r=0 dt=85000 ct=Some(9000)",
            "Assistant Done agent name=None h=None asked=None turn=Some(Reply) wa=false [question:Open] hidden=[] text=0 r=0 dt=105000 ct=Some(9000)",
            "Assistant Done agent name=None h=None asked=None turn=Some(Reply) wa=false [question:Open] hidden=[] text=0 r=0 dt=125000 ct=Some(9000)",
            "Assistant Done agent name=None h=None asked=None turn=Some(Reply) wa=false [question:Open] hidden=[] text=0 r=0 dt=145000 ct=Some(9000)",
            "Assistant Done agent name=None h=None asked=None turn=Some(Reply) wa=false [question:Open] hidden=[] text=0 r=0 dt=165000 ct=Some(9000)",
        ]),
        ("01J9CHAT0DRAFTS00000000000", &[
            "User Done user name=None h=None asked=None turn=None wa=false [text] hidden=[] text=45 r=0 dt=0 ct=Some(0)",
            "Assistant Done agent name=None h=None asked=None turn=Some(Act) wa=false [text,approval:Approved] hidden=[] text=54 r=0 dt=20000 ct=Some(9000)",
            "Assistant Done agent name=None h=None asked=None turn=Some(Reply) wa=false [text] hidden=[] text=100 r=0 dt=40000 ct=Some(9000)",
            "User Done user name=None h=None asked=None turn=None wa=false [text] hidden=[] text=48 r=0 dt=85000 ct=Some(0)",
            "Assistant Done agent name=None h=None asked=None turn=Some(Act) wa=true [text,approval:Pending,status:WaitingApproval] hidden=[] text=69 r=0 dt=105000 ct=Some(9000)",
        ]),
        ("01J9CHAT0MEDIA000000000000", &[
            "User Done user name=None h=None asked=None turn=None wa=false [text] hidden=[] text=126 r=0 dt=0 ct=Some(0)",
            "Assistant Done agent name=None h=None asked=None turn=Some(Act) wa=false [text,attachment:Q3 cover image:Some(418000):Image,attachment:Q3 review deck · 12 slides:Some(2310000):Slides,attachment:Q3 budget vs. actuals:Some(61000):Sheet,attachment:One-pager for the board:Some(48500):Doc,attachment:Deck, exported as PDF:Some(1540000):Pdf,attachment:Teaser clip · 0:24:Some(9800000):Video,attachment:Voice-over · 1:12:Some(1120000):Audio,attachment:Source data and scripts:Some(3300000):File,text] hidden=[] text=172 r=0 dt=20000 ct=Some(9000)",
            "User Done user name=None h=None asked=None turn=None wa=false [text] hidden=[] text=54 r=0 dt=65000 ct=Some(0)",
            "Assistant Done agent name=None h=None asked=None turn=Some(Reply) wa=false [text,attachment:Screenshot 2026-10-23 at 7.55.41.png:Some(212000):Image] hidden=[] text=48 r=0 dt=85000 ct=Some(9000)",
        ]),
        ("01J9CHAT0VOICE000000000000", &[
            "User Done user name=None h=None asked=None turn=None wa=false [attachment:voice-memo.wav:None:Audio,text] hidden=[] text=39 r=0 dt=0 ct=Some(0)",
            "Assistant Done agent name=None h=None asked=None turn=Some(Reply) wa=false [text] hidden=[] text=167 r=0 dt=20000 ct=Some(9000)",
        ]),
        ("01J9CHAT0GROUP000000000000", &[
            "User Done user name=None h=None asked=None turn=None wa=false [text] hidden=[] text=70 r=0 dt=0 ct=Some(0)",
            "Assistant Done agent name=None h=None asked=None turn=Some(Reply) wa=false [text] hidden=[] text=126 r=0 dt=20000 ct=Some(9000)",
            "Project Done project:01J9AFFINE0000000000000000 name=Some(\"Affine\") h=None asked=None turn=None wa=false [text] hidden=[] text=85 r=0 dt=32000 ct=Some(0)",
            "Project Done project:01J9COMMS00000000000000000 name=Some(\"Comms\") h=None asked=None turn=None wa=false [text] hidden=[] text=93 r=0 dt=44000 ct=Some(0)",
        ]),
        ("01J9CHAT0AFFINE00000000000", &[
            "User Done user name=None h=None asked=None turn=None wa=false [text] hidden=[] text=36 r=0 dt=0 ct=Some(0)",
            "Assistant Done agent name=None h=None asked=None turn=Some(Reply) wa=false [text] hidden=[] text=141 r=0 dt=20000 ct=Some(9000)",
        ]),
        ("01J9CHAT0COMMS000000000000", &[
            "User Done user name=None h=None asked=None turn=None wa=false [text] hidden=[] text=35 r=0 dt=0 ct=Some(0)",
            "Assistant Error agent name=None h=None asked=None turn=Some(Act) wa=false [tool:Some(Ok),error] hidden=[] text=0 r=0 dt=20000 ct=Some(9000)",
        ]),
        ("01J9DMSAM00000000000000000", &[
            "User Done user:gh_456 name=Some(\"Sam Rivera\") h=Some(\"sam\") asked=None turn=None wa=false [text] hidden=[] text=77 r=0 dt=0 ct=Some(0)",
            "User Done user:gh_1 name=Some(\"Jacob Steeves\") h=Some(\"jacob\") asked=None turn=None wa=false [text] hidden=[] text=47 r=1 dt=90000 ct=Some(0)",
            "User Done user:gh_456 name=Some(\"Sam Rivera\") h=Some(\"sam\") asked=None turn=None wa=false [text] hidden=[] text=98 r=0 dt=150000 ct=Some(0)",
        ]),
        ("01J9STUDIOCHAT000000000000", &[
            "User Done user:gh_456 name=Some(\"Sam Rivera\") h=Some(\"sam\") asked=None turn=None wa=false [text] hidden=[] text=66 r=0 dt=0 ct=Some(0)",
            "Assistant Done agent:01J9STUDIO0000000000000000 name=Some(\"Studio\") h=Some(\"studio\") asked=Some(\"sam\") turn=None wa=false [text] hidden=[] text=153 r=0 dt=30000 ct=Some(0)",
        ]),
        ("01J9GROUPLAUNCH00000000000", &[
            "User Done user:gh_457 name=Some(\"Samir Haddad\") h=Some(\"samir\") asked=None turn=None wa=false [text] hidden=[] text=93 r=0 dt=0 ct=Some(0)",
            "User Done user:gh_456 name=Some(\"Sam Rivera\") h=Some(\"sam\") asked=None turn=None wa=false [text] hidden=[] text=71 r=0 dt=30000 ct=Some(0)",
            "User Done user:gh_457 name=Some(\"Samir Haddad\") h=Some(\"samir\") asked=None turn=None wa=false [text] hidden=[] text=60 r=0 dt=36000 ct=Some(0)",
            "User Done user:gh_458 name=Some(\"Sara Lind\") h=Some(\"sara\") asked=None turn=None wa=false [text] hidden=[] text=43 r=0 dt=42000 ct=Some(0)",
            "User Done user:gh_456 name=Some(\"Sam Rivera\") h=Some(\"sam\") asked=None turn=None wa=false [text] hidden=[] text=72 r=0 dt=48000 ct=Some(0)",
            "Assistant Done agent:01J9ARBOS00000000000000000 name=Some(\"Arbos\") h=Some(\"arbos\") asked=Some(\"sam\") turn=None wa=false [attachment:Slide 4 · signups:Some(212000):Image] hidden=[] text=0 r=0 dt=58000 ct=Some(0)",
            "Assistant Done agent:01J9ARBOS00000000000000000 name=Some(\"Arbos\") h=Some(\"arbos\") asked=Some(\"sam\") turn=None wa=false [text] hidden=[] text=96 r=0 dt=60000 ct=Some(0)",
            "User Done user:gh_457 name=Some(\"Samir Haddad\") h=Some(\"samir\") asked=None turn=None wa=false [attachment:Slide 4 · signups:Some(212000):Image,attachment:Launch dashboard:Some(348000):Image,attachment:Launch day, the team:Some(1204000):Image,attachment:Signup funnel:Some(156000):Image,attachment:Press kit v3:Some(1840000):Pdf,attachment:Launch teaser:Some(6420000):Video,text] hidden=[] text=54 r=0 dt=80000 ct=Some(0)",
            "User Done user:gh_457 name=Some(\"Samir Haddad\") h=Some(\"samir\") asked=None turn=None wa=false [text] hidden=[] text=112 r=0 dt=90000 ct=Some(0)",
            "User Done user:gh_458 name=Some(\"Sara Lind\") h=Some(\"sara\") asked=None turn=None wa=false [text] hidden=[] text=64 r=0 dt=100000 ct=Some(0)",
            "User Done user:gh_458 name=Some(\"Sara Lind\") h=Some(\"sara\") asked=None turn=None wa=false [text] hidden=[] text=55 r=0 dt=120000 ct=Some(0)",
            "Assistant Error agent:01J9ARBOS00000000000000000 name=Some(\"Arbos\") h=Some(\"arbos\") asked=Some(\"sara\") turn=None wa=false [text,error] hidden=[] text=0 r=0 dt=125000 ct=Some(0)",
            "User Done user:gh_457 name=Some(\"Samir Haddad\") h=Some(\"samir\") asked=None turn=None wa=false [text] hidden=[] text=42 r=0 dt=130000 ct=Some(0)",
            "Assistant Error agent:01J9ARBOS00000000000000000 name=None h=Some(\"arbos\") asked=Some(\"samir\") turn=None wa=false [text] hidden=[] text=0 r=0 dt=133000 ct=Some(0)",
            "User Done user:gh_456 name=Some(\"Sam Rivera\") h=Some(\"sam\") asked=None turn=None wa=false [text] hidden=[] text=61 r=0 dt=140000 ct=Some(0)",
            "Assistant Done agent:01J9ARBOS00000000000000000 name=Some(\"Arbos\") h=Some(\"arbos\") asked=Some(\"sam\") turn=None wa=false [text,attachment:Screenshot 2026-10-23 at 7.55.41.png:Some(212000):Image] hidden=[] text=48 r=0 dt=145000 ct=Some(0)",
            "User Done user:gh_458 name=Some(\"Sara Lind\") h=Some(\"sara\") asked=None turn=None wa=false [text] hidden=[] text=128 r=0 dt=150000 ct=Some(0)",
            "User Done user:gh_1 name=Some(\"Jacob Steeves\") h=Some(\"jacob\") asked=None turn=None wa=false [text] hidden=[] text=160 r=0 dt=152000 ct=Some(0)",
            "User Done user:gh_1 name=Some(\"Jacob Steeves\") h=Some(\"jacob\") asked=None turn=None wa=false [text] hidden=[] text=45 r=0 dt=155000 ct=Some(0)",
        ]),
        
    ];

    #[test]
    fn seeded_docs_match_recorded_parity_table() {
        let got = table();
        assert_eq!(got.len(), EXPECTED.len(), "doc count");
        for ((chat, rows), (want_chat, want_rows)) in got.iter().zip(EXPECTED) {
            assert_eq!(chat, want_chat);
            let want: Vec<String> = want_rows.iter().map(|r| r.to_string()).collect();
            assert_eq!(rows, &want, "chat {chat}");
        }
    }

    #[test]
    fn scripted_turn_writes_by_id_read_back_as_mac_message() {
        let doc = ChatDoc::new("chat", PROJECT_ARBOS);
        let id = new_id();
        let lead = doc.core.transact(|d| {
            begin_assistant_row(
                d,
                &id,
                "agent",
                &[("model", json!(MODELS[0]))],
                StatusKind::Deciding,
                "Arbos is deciding…",
            )
        });
        let m = &doc.read_all()[0];
        assert_eq!(m.status, MessageStatus::Streaming);
        assert_eq!(
            m.parts.iter().map(part_sig).collect::<Vec<_>>(),
            ["status:Deciding"]
        );

        replace_part(&doc.core, &id, &lead, &empty_text());
        for chunk in ["Here is ", "the draft."] {
            doc.core.transact(|d| d.append_text(&id, &lead, chunk));
        }
        let tool = push_part(
            &doc.core,
            &id,
            &Part::Tool {
                id: new_id(),
                call: ToolCall {
                    id: new_id(),
                    name: "grep".into(),
                    input: json!("x"),
                },
                output_summary: String::new(),
                output_ref: None,
                is_error: false,
                status: Some(ToolStatus::Running),
                duration_ms: None,
                subagent_name: None,
            },
        );
        doc.core
            .set_part_fields(&id, &tool, &[("status", json!("ok"))]);
        let approval_id = new_id();
        doc.core.transact(|d| {
            d.add_approval_part(
                &id,
                &approval_id,
                &draft_request("email", "a@b.c", "Hi", "Body"),
                0,
            );
            d.set_approval_state(
                &approval_id,
                CoreApprovalState::Approved,
                Some(DEVICE),
                None,
            );
            set_status(d, &id, MessageStatus::Done);
        });

        let m = &doc.read_all()[0];
        assert_eq!(m.status, MessageStatus::Done);
        assert!(m.completed_at.is_some());
        assert_eq!(m.model.as_deref(), Some(MODELS[0]));
        assert_eq!(m.plain_text(), "Here is the draft.");
        assert_eq!(
            m.parts.iter().map(part_sig).collect::<Vec<_>>(),
            ["text", "tool:Some(Ok)", "approval:Approved"]
        );
    }

    #[test]
    fn seeded_rows_carry_no_mac_placeholders() {
        for doc in seed_chat_docs().into_iter().chain(seed_people_chat_docs()) {
            let raw = doc.core.to_json();
            for row in raw["messages"].as_array().into_iter().flatten() {
                let row = row.as_object().expect("row object");
                for key in ["chatId", "pending", "hiddenParts"] {
                    assert!(!row.contains_key(key), "{}: {key}", doc.chat_id);
                }
                assert_eq!(row["deviceId"], DEVICE);
            }
        }
    }
}

#[cfg(test)]
mod create_group_tests {
    //! "Create" in the New group sheet, end to end through the source: the
    //! command goes in, the next list carries the group, and the id event
    //! names it so the sheet can close on it (2026-10-22: Create was dead for
    //! a group of agents).

    use std::time::Duration;

    use tokio::time::timeout;

    use super::*;
    use crate::model::created_group;

    const WAIT: Duration = Duration::from_secs(10);

    fn start() -> (
        mpsc::UnboundedSender<ClientCommand>,
        mpsc::UnboundedReceiver<SyncEvent>,
    ) {
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        tokio::spawn(Box::new(FixtureSource::new()).run(ev_tx, cmd_rx));
        (cmd_tx, ev_rx)
    }

    async fn next_projects(rx: &mut mpsc::UnboundedReceiver<SyncEvent>) -> Vec<Project> {
        timeout(WAIT, async {
            loop {
                match rx.recv().await {
                    Some(SyncEvent::Projects(p)) => return p,
                    Some(_) => continue,
                    None => panic!("source ended"),
                }
            }
        })
        .await
        .expect("a Projects event")
    }

    /// Everything up to the id event: the latest list seen and the new id.
    async fn until_created(rx: &mut mpsc::UnboundedReceiver<SyncEvent>) -> (Vec<Project>, Id) {
        timeout(WAIT, async {
            let mut latest = Vec::new();
            loop {
                match rx.recv().await {
                    Some(SyncEvent::Projects(p)) => latest = p,
                    Some(SyncEvent::PeopleGroupCreated { chat_id }) => return (latest, chat_id),
                    Some(SyncEvent::PeopleError(e)) => panic!("create failed: {}", e.sentence()),
                    Some(_) => continue,
                    None => panic!("source ended"),
                }
            }
        })
        .await
        .expect("a PeopleGroupCreated event")
    }

    #[tokio::test]
    async fn people_and_an_agent_make_a_group_that_the_list_and_the_id_event_name() {
        let (cmd, mut rx) = start();
        let before = next_projects(&mut rx).await;
        cmd.send(ClientCommand::CreatePeopleGroup {
            handles: vec!["sam".into()],
            agent_ids: vec![PROJECT_ARBOS.into()],
            title: None,
            leaf: None,
            picture: None,
        })
        .unwrap();
        let (after, chat_id) = until_created(&mut rx).await;
        let group = after
            .iter()
            .find(|p| p.id == chat_id)
            .expect("the list that precedes the id event carries the group");
        assert!(group.is_people_group());
        assert_eq!(
            group
                .people
                .iter()
                .map(|p| p.handle.as_str())
                .collect::<Vec<_>>(),
            ["sam"]
        );
        assert!(group.agents.iter().any(|a| a.project_id == PROJECT_ARBOS));
        // The members match (older relays) finds the same group.
        assert_eq!(
            created_group(&before, &after, &["sam".to_string()]),
            Some(chat_id)
        );
    }

    #[tokio::test]
    async fn agents_alone_make_a_group_named_after_them() {
        let (cmd, mut rx) = start();
        let before = next_projects(&mut rx).await;
        cmd.send(ClientCommand::CreatePeopleGroup {
            handles: vec![],
            agent_ids: vec![PROJECT_ARBOS.into(), PROJECT_COMMS.into()],
            title: None,
            leaf: None,
            picture: None,
        })
        .unwrap();
        let (after, chat_id) = until_created(&mut rx).await;
        let group = after
            .iter()
            .find(|p| p.id == chat_id)
            .expect("the agents' group");
        assert!(group.is_people_group());
        assert!(group.people.is_empty());
        assert_eq!(group.agents.len(), 2);
        assert_eq!(group.name, "arbos, comms");
        // No handles to match on: only the id event can close the sheet.
        assert_eq!(created_group(&before, &after, &[]), None);
    }

    #[tokio::test]
    async fn nobody_is_refused() {
        let (cmd, mut rx) = start();
        let _ = next_projects(&mut rx).await;
        cmd.send(ClientCommand::CreatePeopleGroup {
            handles: vec![],
            agent_ids: vec![],
            title: None,
            leaf: None,
            picture: None,
        })
        .unwrap();
        let err = timeout(WAIT, async {
            loop {
                match rx.recv().await {
                    Some(SyncEvent::PeopleError(e)) => return e,
                    Some(SyncEvent::PeopleGroupCreated { .. }) => {
                        panic!("created a group of nobody")
                    }
                    Some(_) => continue,
                    None => panic!("source ended"),
                }
            }
        })
        .await
        .expect("a PeopleError");
        assert!(
            err.sentence().contains("person or agent"),
            "{}",
            err.sentence()
        );
    }
}
