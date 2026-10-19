//! Local, offline source. Every chat is a real Loro doc (`doc.rs`); the
//! "agent" is a scripted tokio task that writes into the doc the way the box
//! would: a `deciding` status, tool cards flipping from running to done, text
//! appended in 120 ms frames, a final status. The UI cannot tell it apart from
//! the relay path.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use futures::future::BoxFuture;
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::sleep;

use crate::model::{
    AgentMember, Call, CallEnded, CallKind, Machine, MachineState, Person, ProjectDocView,
    ReplyPolicy, Sender, SpaceKind, ThreadInfo, Voice,
};

use super::doc::ChatDoc;
use super::{ClientCommand, SyncEvent, SyncSource};
use crate::model::{
    new_id, now_ms, ApprovalRisk, ApprovalState, Attachment, AttachmentKind, BoxState,
    ChannelBinding, ChannelPairing, ChannelPlatform, ChannelPlatformState, ChannelsView, Chat,
    ChatKind, ChoiceOption, Connector, ConnectorAction, ConnectorProvider, ConnectorStatus,
    ConnectorsView, Effort, FieldKind, FormField, Id, Link, LocalDevice, LocalView, Message,
    MessageStatus, Part, Project, QuestionState, Role, Routine, RoutineRun, RoutineRunStatus,
    RoutineTrigger, Secret, SecretActivity, SecretAgent, SecretRequest, SecretScope, SecretsView,
    Skill, StatusKind, ToolCall, ToolStatus, TurnKind, Widget,
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
    routines: Mutex<Vec<Routine>>,
    routine_runs: Mutex<HashMap<Id, Vec<RoutineRun>>>,
    local: Mutex<LocalView>,
    /// Voice memos sent in this session, by blob key (F11; no blob store offline).
    voice_blobs: Mutex<HashMap<String, Arc<Vec<u8>>>>,
    machine: Mutex<Machine>,
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
                routines: Mutex::new(seed_routines()),
                routine_runs: Mutex::new(seed_routine_runs()),
                local: Mutex::new(seed_local()),
                voice_blobs: Mutex::new(HashMap::new()),
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
                    always_on: std::env::var("ARBOS_FIXTURE_ALWAYS_ON")
                        .map(|v| v != "0")
                        .unwrap_or(true),
                    cpu: Some(4),
                    mem_gib: Some(8),
                    image: Some("arbos-agent-0.1.7".into()),
                }),
            });
            seed_docs(&shared);
            seed_people_docs(&shared);
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
            let _ = events.send(SyncEvent::Link(Link::Live));

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
        | ClientCommand::SetTyping { .. }
        | ClientCommand::AddAgent { .. }
        | ClientCommand::RemoveAgent { .. }
        | ClientCommand::RemoveMember { .. }
        | ClientCommand::LeaveChat { .. }
        | ClientCommand::BlockUser { .. }
        | ClientCommand::InviteToProject { .. }
        | ClientCommand::RemoveProjectMember { .. }
        | ClientCommand::FetchAvatar { .. }
        | ClientCommand::ArchiveChat { .. }
        | ClientCommand::DeleteProject { .. }
        | ClientCommand::DeletePeopleChat { .. }
        | ClientCommand::CloseChat { .. } => {}
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
            let docs = shared.docs.lock().unwrap();
            if let Some(doc) = docs.get(&chat_id) {
                let mut msg = system_message(&chat_id, &text);
                msg.call_id = call_id;
                let _ = doc.insert_message(&msg);
                doc.commit();
            }
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
        } => {
            let parts: Vec<Part> = attachments
                .iter()
                .map(|path| Part::Attachment {
                    id: new_id(),
                    attachment: Attachment {
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
                    },
                })
                .collect();
            send_user(shared, chat_id, message_id, text, parts, reply_to);
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
            let parts = vec![Part::Attachment {
                id: new_id(),
                attachment: Attachment {
                    name: "voice-memo.wav".into(),
                    mime: "audio/wav".into(),
                    blob_ref: Some(key),
                    bytes: None,
                    kind: Some(AttachmentKind::Audio),
                    title: None,
                },
            }];
            send_user(shared, chat_id, message_id, text, parts, None);
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
            {
                let docs = shared.docs.lock().unwrap();
                if let Some(doc) = docs.get(&chat_id) {
                    if let Some(qid) = &question_id {
                        let _ = doc.answer_question(qid, value.as_ref().unwrap_or(&json!(null)));
                    }
                    let msg = Message {
                        id: message_id.clone(),
                        chat_id: chat_id.clone(),
                        role: Role::User,
                        sender: "user".into(),
                        status: MessageStatus::Done,
                        turn: None,
                        decision: None,
                        subagent_id: None,
                        subagent_name: None,
                        handoff_messages: None,
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
                        asked_by_handle: None,
                        owner_handle: None,
                        usage: None,
                        return_contract: None,
                        waiting_user: false,
                        waiting_approval: false,
                        model: None,
                        effort: None,
                        created_at: now_ms(),
                        completed_at: None,
                        call_id: None,
                        activity: None,
                        pending: true,
                        parts: vec![Part::Text {
                            id: new_id(),
                            text: text.clone(),
                        }],
                        hidden_parts: Vec::new(),
                        greeting: false,
                    };
                    let _ = doc.insert_message(&msg);
                    let _ = doc.push_command(
                        "answer",
                        json!({ "messageId": message_id, "text": text, "questionId": question_id, "value": value }),
                        DEVICE,
                    );
                    doc.commit();
                }
            }
            spawn_turn(shared, project_id, chat_id, message_id, text, true);
        }
        ClientCommand::Stop { chat_id } => {
            if let Some(flag) = shared.running.lock().unwrap().get(&chat_id) {
                flag.store(true, Ordering::SeqCst);
            }
            let docs = shared.docs.lock().unwrap();
            if let Some(doc) = docs.get(&chat_id) {
                let _ = doc.push_command("stop", json!({}), DEVICE);
                doc.commit();
            }
        }
        ClientCommand::React {
            chat_id,
            message_id,
            emoji,
        } => {
            let docs = shared.docs.lock().unwrap();
            if let Some(doc) = docs.get(&chat_id) {
                let _ = doc.toggle_reaction(&message_id, &emoji, &format!("user:{ME_ID}"));
                doc.commit();
            }
        }
        ClientCommand::Approve {
            chat_id,
            approval_id,
        } => decide_approval(shared, chat_id, approval_id, ApprovalState::Approved, None),
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
                    let _ = doc.set_status(ix, MessageStatus::Cancelled);
                    let _ = doc.push_command("retry", json!({ "messageId": message_id }), DEVICE);
                    doc.commit();
                    Some(text)
                })
            };
            if let Some(text) = prompt {
                spawn_turn(shared, project_id, chat_id, new_id(), text, true);
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
                last_message_at: now_ms(),
                archived_at: None,
            });
            emit_chats(shared);
            let _ = shared.events.send(SyncEvent::ChatSnapshot {
                chat_id,
                messages: Vec::new(),
            });
        }
        ClientCommand::GroupCreate {
            project_id,
            chat_id,
            participants,
            title,
        } => {
            add_doc(shared, ChatDoc::new(&chat_id, &project_id));
            let mut all = vec![project_id.clone()];
            all.extend(participants.into_iter().filter(|p| *p != project_id));
            let names: Vec<String> = {
                let projects = shared.projects.lock().unwrap();
                all.iter()
                    .skip(1)
                    .filter_map(|id| projects.iter().find(|p| &p.id == id))
                    .map(|p| p.name.clone())
                    .collect()
            };
            let chat = Chat {
                id: chat_id.clone(),
                project_id: project_id.clone(),
                title: title.unwrap_or_else(|| format!("Group · {}", names.join(", "))),
                is_pinned: false,
                is_system: false,
                kind: ChatKind::Group,
                participants: all,
                unread: false,
                last_message_at: now_ms(),
                archived_at: None,
            };
            shared.chats.lock().unwrap().push(chat.clone());
            emit_chats(shared);
            let _ = shared.events.send(SyncEvent::ChatCreated {
                project_id,
                local_id: chat_id.clone(),
                chat,
            });
            let _ = shared.events.send(SyncEvent::ChatSnapshot {
                chat_id,
                messages: Vec::new(),
            });
        }
        ClientCommand::GroupAddParticipant {
            chat_id,
            participant,
            ..
        } => {
            {
                let mut chats = shared.chats.lock().unwrap();
                if let Some(c) = chats.iter_mut().find(|c| c.id == chat_id) {
                    if !c.participants.contains(&participant) {
                        c.participants.push(participant);
                    }
                }
            }
            emit_chats(shared);
        }
        ClientCommand::MarkRead { chat_id } => mark_read(shared, &chat_id),
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
        ClientCommand::SetVisionEnabled {
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
        ClientCommand::SetVoice { project_id, voice } => {
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == project_id) {
                    p.voice = voice;
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
        ClientCommand::SetAvatarColorIndex { project_id, index } => {
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == project_id) {
                    p.avatar_color_override = index;
                    p.avatar_color_index = Some(crate::ui::avatars::index_for(&project_id, index));
                }
            }
            emit_projects(shared);
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
                    p.name = name;
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
            source_project_id: _,
            description,
            color,
        } => {
            let id = new_id();
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
                muted: false,
                primary_chat: None,
                voice: None,
                vision_enabled: true,
                avatar_color_index: avatar_color_index
                    .map(|i| crate::ui::avatars::index_for(&id, Some(i))),
                avatar_color_override: avatar_color_index,
            });
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
                    let _ = shared
                        .events
                        .send(SyncEvent::Notice(format!("Could not save {name}: {err}")));
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

/// The user row (`extra_parts` first, then the text), its `send` command, and the scripted turn.
fn send_user(
    shared: &Arc<Shared>,
    chat_id: Id,
    message_id: Id,
    text: String,
    mut parts: Vec<Part>,
    reply_to: Option<String>,
) {
    let project_id = project_of(shared, &chat_id);
    if let Some(space) = people_space_of(shared, &chat_id) {
        let mentions = mentions_in(&space, &text);
        let mut msg = human_message(&chat_id, &me(), &text, now_ms());
        msg.id = message_id.clone();
        msg.mentions = mentions.clone();
        msg.reply_to = reply_to;
        msg.parts = parts;
        msg.parts.push(text_part(&text));
        with_doc(shared, &chat_id, |doc| {
            let _ = doc.insert_message(&msg);
            let _ = doc.push_command("send", json!({ "messageId": message_id }), DEVICE);
            doc.commit();
        });
        people_after_send(shared, space, chat_id, text, mentions);
        return;
    }
    {
        let docs = shared.docs.lock().unwrap();
        if let Some(doc) = docs.get(&chat_id) {
            let asleep = machine_down(shared);
            parts.push(Part::Text {
                id: new_id(),
                text: text.clone(),
            });
            let msg = Message {
                id: message_id.clone(),
                chat_id: chat_id.clone(),
                role: Role::User,
                sender: "user".into(),
                status: if asleep {
                    MessageStatus::Queued
                } else {
                    MessageStatus::Done
                },
                turn: None,
                decision: None,
                subagent_id: None,
                subagent_name: None,
                handoff_messages: None,
                reactions: Default::default(),
                in_reply_to: None,
                routine_name: None,
                sender_name: None,
                sender_icon: None,
                sender_handle: None,
                sender_avatar: None,
                sender_avatar_color: None,
                mentions: Vec::new(),
                reply_to,
                asked_by_handle: None,
                owner_handle: None,
                usage: None,
                return_contract: None,
                waiting_user: false,
                waiting_approval: false,
                model: None,
                effort: None,
                created_at: now_ms(),
                completed_at: None,
                call_id: None,
                activity: None,
                pending: true,
                parts,
                hidden_parts: Vec::new(),
                greeting: false,
            };
            let _ = doc.insert_message(&msg);
            let _ = doc.push_command(
                "send",
                json!({ "messageId": message_id, "text": text }),
                DEVICE,
            );
            doc.commit();
        }
    }
    maybe_retitle(shared, &chat_id, &text);
    spawn_turn(shared, project_id, chat_id, message_id, text, false);
}

fn emit_connectors(shared: &Shared) {
    let view = shared.connectors.lock().unwrap().clone();
    let _ = shared.events.send(SyncEvent::Connectors(view));
}

fn emit_secrets(shared: &Shared) {
    let view = shared.secrets.lock().unwrap().clone();
    let _ = shared.events.send(SyncEvent::Secrets(view));
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

/// Settings → Secrets demo (D83): a Stripe key for Research and Comms, a Mailgun key for Comms,
/// a GitHub login for Website, one pending request from Comms, recent activity. No value exists
/// anywhere in the fixture.
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
    // The iPhone fixture's twin: the Stripe key is a real value in the agents' environment.
    stripe.inject_env = true;
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
        project_id: project.map(|(id, _)| id.to_string()),
        project_name: project.map(|(_, n)| n.to_string()),
        host: host.map(str::to_string),
        by: by.into(),
        detail: serde_json::json!({}),
    };
    let activity = vec![
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
    // One connected 1Password vault (Phase 2), names only — the iPhone fixture's twin.
    let vaults = vec![crate::model::ConnectedVault {
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
    SecretsView {
        paused: false,
        secrets,
        vaults,
        providers: vec!["onepassword".into()],
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
    {
        let docs = shared.docs.lock().unwrap();
        if let Some(doc) = docs.get(&chat_id) {
            let _ = doc.insert_message(&Message {
                id: message_id.clone(),
                chat_id: chat_id.clone(),
                role: Role::User,
                sender: format!("routine:{routine_id}"),
                status: MessageStatus::Done,
                turn: None,
                decision: None,
                subagent_id: None,
                subagent_name: None,
                handoff_messages: None,
                reactions: Default::default(),
                in_reply_to: None,
                routine_name: Some(routine.name.clone()),
                sender_name: None,
                sender_icon: None,
                sender_handle: None,
                sender_avatar: None,
                sender_avatar_color: None,
                mentions: Vec::new(),
                reply_to: None,
                asked_by_handle: None,
                owner_handle: None,
                usage: None,
                return_contract: None,
                waiting_user: false,
                waiting_approval: false,
                model: None,
                effort: None,
                created_at: now_ms(),
                completed_at: None,
                call_id: None,
                activity: None,
                pending: false,
                parts: vec![Part::Text {
                    id: new_id(),
                    text: routine.prompt.clone(),
                }],
                hidden_parts: Vec::new(),
                greeting: false,
            });
            doc.commit();
        }
    }
    spawn_turn(
        shared,
        project_id.to_string(),
        chat_id.clone(),
        message_id.clone(),
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
    let sub = doc.doc.subscribe_root(Arc::new(move |event| {
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
        let mut changed = false;
        for ix in 0..doc.len() {
            if doc
                .read_message(ix)
                .map(|m| m.status == MessageStatus::Queued)
                .unwrap_or(false)
            {
                let _ = doc.set_status(ix, MessageStatus::Done);
                changed = true;
            }
        }
        if changed {
            doc.commit();
        }
    }
}

fn emit_projects(shared: &Shared) {
    let projects = shared.projects.lock().unwrap().clone();
    let _ = shared.events.send(SyncEvent::Projects(projects));
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
            for ix in 0..doc.len() {
                if doc
                    .read_message(ix)
                    .map(|m| m.status == MessageStatus::Queued)
                    .unwrap_or(false)
                {
                    let _ = doc.set_status(ix, MessageStatus::Done);
                }
            }
            doc.commit();
        }
    }
}

fn spawn_turn(
    shared: &Arc<Shared>,
    project_id: Id,
    chat_id: Id,
    user_message_id: Id,
    text: String,
    force_reply: bool,
) {
    let stop = Arc::new(AtomicBool::new(false));
    shared
        .running
        .lock()
        .unwrap()
        .insert(chat_id.clone(), stop.clone());
    let shared = shared.clone();
    tokio::spawn(async move {
        run_turn(
            &shared,
            &project_id,
            &chat_id,
            &user_message_id,
            &text,
            force_reply,
            stop,
        )
        .await;
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

const REPLY_DEFAULT: &[&str] = &[
    "Here is how I would approach it.",
    "- Keep the rail narrow and the tabs flat, so the chat column gets the width.\n- Put the composer at the bottom once a chat has a message; before that, center it so a new tab feels like a blank page.\n- Treat every tool call as a card that folds, so long runs stay readable.",
    "If you want, I can turn this into a short checklist and start on the first item.",
];

const REPLY_SIMPLE: &[&str] = &[
    "Agreed. Two sections only: the rail on the left and the chat in the center.",
    "Nothing else at rest. Details open only when you ask for them.",
];

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
    user_message_id: &str,
    text: &str,
    force_reply: bool,
    stop: Arc<AtomicBool>,
) {
    sleep(Duration::from_millis(220)).await;
    with_doc(shared, chat_id, |doc| {
        if let Some(ix) = doc.index_of(user_message_id) {
            let _ = doc.set_field(ix, "pending", json!(false));
            doc.commit();
        }
    });

    if machine_down(shared) {
        wake_machine(shared).await;
    }
    if project_state(shared, project_id).is_asleep() {
        wake_project(shared, project_id).await;
    }

    let (model, effort) = project_model(shared, project_id);
    let assistant_id = new_id();
    let ix = with_doc(shared, chat_id, |doc| {
        let msg = Message {
            id: assistant_id.clone(),
            chat_id: chat_id.to_string(),
            role: Role::Assistant,
            sender: "agent".into(),
            status: MessageStatus::Streaming,
            turn: None,
            decision: None,
            subagent_id: None,
            subagent_name: None,
            handoff_messages: None,
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
            asked_by_handle: None,
            owner_handle: None,
            usage: None,
            return_contract: None,
            waiting_user: false,
            waiting_approval: false,
            model: Some(model.clone()),
            effort: Some(effort),
            created_at: now_ms(),
            completed_at: None,
            call_id: None,
            activity: None,
            pending: false,
            parts: vec![Part::Status {
                id: new_id(),
                code: StatusKind::Deciding,
                message: "Arbos is deciding…".into(),
            }],
            hidden_parts: Vec::new(),
            greeting: false,
        };
        let ix = doc.insert_message(&msg).unwrap_or(0);
        doc.commit();
        ix
    })
    .unwrap_or(0);

    sleep(Duration::from_millis(700)).await;
    if stop.load(Ordering::SeqCst) {
        finish_cancelled(shared, chat_id, ix);
        return;
    }

    let script = pick_script(text, force_reply);
    let turn = match &script {
        Script::Reply(_) | Script::Ask(_, _, _) | Script::Fail(_, _) => TurnKind::Reply,
        Script::Act { .. } | Script::Draft { .. } | Script::Flow => TurnKind::Act,
        Script::Browse { .. } => TurnKind::Spawn,
    };
    with_doc(shared, chat_id, |doc| {
        let _ = doc.set_field(ix, "turn", serde_json::to_value(turn).unwrap_or_default());
        doc.commit();
    });

    let finished = match script {
        Script::Flow => {
            // 1. The running row becomes the ack bubble at once.
            with_doc(shared, chat_id, |doc| {
                let _ = doc.replace_part(ix, 0, &text_part(FLOW_ACK));
                let _ = doc.set_status(ix, MessageStatus::Done);
                doc.commit();
            });
            // 2. A new running row with no prose: only the working line shows,
            //    its text swapping through the activities.
            let work_ix = insert_running_row(shared, chat_id, project_id, TurnKind::Act);
            let mut cancelled = false;
            for (activity, ms) in FLOW_ACTIVITIES {
                if stop.load(Ordering::SeqCst) {
                    cancelled = true;
                    break;
                }
                with_doc(shared, chat_id, |doc| {
                    let _ = doc.set_field(work_ix, "activity", json!(activity));
                    doc.commit();
                });
                sleep(Duration::from_millis(*ms)).await;
            }
            if cancelled {
                finish_cancelled(shared, chat_id, work_ix);
                return;
            }
            // 3. Same row becomes the short conversational result (no milestone bubble).
            with_doc(shared, chat_id, |doc| {
                let _ = doc.set_field(work_ix, "activity", json!(null));
                let _ = doc.set_field(
                    work_ix,
                    "turn",
                    serde_json::to_value(TurnKind::Reply).unwrap_or_default(),
                );
                let _ = doc.replace_part(work_ix, 0, &empty_text());
                doc.commit();
            });
            let ok = stream_blocks(shared, chat_id, work_ix, 0, FLOW_RESULT, &stop).await;
            if ok {
                with_doc(shared, chat_id, |doc| {
                    let _ = doc.set_status(work_ix, MessageStatus::Done);
                    doc.commit();
                });
                mark_unread_if_hidden(shared, chat_id);
            } else {
                finish_cancelled(shared, chat_id, work_ix);
            }
            return;
        }
        Script::Reply(blocks) => {
            with_doc(shared, chat_id, |doc| {
                let _ = doc.replace_part(ix, 0, &empty_text());
                doc.commit();
            });
            stream_blocks(shared, chat_id, ix, 0, blocks, &stop).await
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
                let pix = with_doc(shared, chat_id, |doc| {
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
                    let pix = if first {
                        let _ = doc.replace_part(ix, 0, &part);
                        0
                    } else {
                        doc.push_part(ix, &part).unwrap_or(0)
                    };
                    doc.commit();
                    pix
                })
                .unwrap_or(0);
                first = false;
                sleep(Duration::from_millis(*ms)).await;
                with_doc(shared, chat_id, |doc| {
                    let _ = doc.set_part_field(ix, pix, "outputSummary", json!(out));
                    let _ = doc.set_part_field(
                        ix,
                        pix,
                        "outputRef",
                        json!(format!("blob/{}/{}", project_id, call.id)),
                    );
                    let _ = doc.set_part_field(ix, pix, "status", json!("ok"));
                    let _ = doc.set_part_field(ix, pix, "durationMs", json!(ms));
                    doc.commit();
                });
                if *name == "edit_file" {
                    if let Some((path, added, removed)) = diff {
                        with_doc(shared, chat_id, |doc| {
                            let _ = doc.push_part(
                                ix,
                                &Part::Diff {
                                    id: new_id(),
                                    path: path.to_string(),
                                    added,
                                    removed,
                                    blob_ref: Some(format!("blob/{project_id}/diff-rail")),
                                },
                            );
                            doc.commit();
                        });
                    }
                }
            }
            if ok {
                let pix = with_doc(shared, chat_id, |doc| {
                    let pix = doc.push_part(ix, &empty_text()).unwrap_or(0);
                    doc.commit();
                    pix
                })
                .unwrap_or(0);
                stream_blocks(shared, chat_id, ix, pix, summary, &stop).await
            } else {
                false
            }
        }
        Script::Browse {
            title,
            url,
            summary,
        } => {
            let status_pix = with_doc(shared, chat_id, |doc| {
                let _ = doc.replace_part(
                    ix,
                    0,
                    &Part::Status {
                        id: new_id(),
                        code: StatusKind::Working,
                        message: "Working in sandbox · starting browser · 00:00".into(),
                    },
                );
                let _ = doc.push_part(
                    ix,
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
                doc.commit();
                0
            })
            .unwrap_or(0);
            sleep(Duration::from_millis(900)).await;
            let browser_pix = with_doc(shared, chat_id, |doc| {
                let pix = doc
                    .push_part(
                        ix,
                        &Part::Browser {
                            id: new_id(),
                            title: title.to_string(),
                            url: url.to_string(),
                            last_action: "navigate".into(),
                            live: true,
                        },
                    )
                    .unwrap_or(0);
                doc.commit();
                pix
            })
            .unwrap_or(0);
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
                    let _ = doc.set_part_field(
                        ix,
                        status_pix,
                        "message",
                        json!(format!("Working in sandbox · {} · 00:{:02}", action, secs)),
                    );
                    let _ = doc.set_part_field(ix, browser_pix, "lastAction", json!(action));
                    doc.commit();
                });
            }
            if cancelled {
                false
            } else {
                with_doc(shared, chat_id, |doc| {
                    let _ = doc.set_part_field(ix, browser_pix, "live", json!(false));
                    let _ = doc.set_part_field(
                        ix,
                        status_pix,
                        "message",
                        json!("Worked in sandbox · 4 actions · 00:06"),
                    );
                    let _ = doc.push_part(
                        ix,
                        &Part::File {
                            id: new_id(),
                            path: "docs/research/gpui.md".into(),
                            lines: 212,
                            blob_ref: Some(format!("blob/{project_id}/gpui-notes")),
                        },
                    );
                    doc.commit();
                });
                let pix = with_doc(shared, chat_id, |doc| {
                    let pix = doc.push_part(ix, &empty_text()).unwrap_or(0);
                    doc.commit();
                    pix
                })
                .unwrap_or(0);
                stream_blocks(shared, chat_id, ix, pix, summary, &stop).await
            }
        }
        Script::Ask(lead, question, widget) => {
            with_doc(shared, chat_id, |doc| {
                let _ = doc.replace_part(ix, 0, &empty_text());
                doc.commit();
            });
            let done = stream_blocks(shared, chat_id, ix, 0, &[lead], &stop).await;
            if done {
                let widget = widget.clone();
                with_doc(shared, chat_id, |doc| {
                    let _ = doc.push_part(ix, &question_part(question, widget, None));
                    // Legacy status row, as agentd still writes it next to the question part.
                    let _ = doc.push_part(
                        ix,
                        &Part::Status {
                            id: new_id(),
                            code: StatusKind::AskUser,
                            message: question.to_string(),
                        },
                    );
                    doc.commit();
                });
            }
            done
        }
        Script::Draft {
            lead,
            channel,
            to,
            subject,
            body,
        } => {
            with_doc(shared, chat_id, |doc| {
                let _ = doc.replace_part(ix, 0, &empty_text());
                doc.commit();
            });
            let done = stream_blocks(shared, chat_id, ix, 0, &[lead], &stop).await;
            if done {
                let approval_id = new_id();
                with_doc(shared, chat_id, |doc| {
                    let _ = doc.push_part(
                        ix,
                        &approval_card(
                            &approval_id,
                            channel,
                            to,
                            subject,
                            body,
                            ApprovalState::Pending,
                            None,
                        ),
                    );
                    let _ = doc.push_part(
                        ix,
                        &Part::Status {
                            id: new_id(),
                            code: StatusKind::WaitingApproval,
                            message: format!("Send {channel} to {to}: {subject}"),
                        },
                    );
                    let _ = doc.set_field(ix, "waitingApproval", json!(true));
                    doc.commit();
                });
            }
            done
        }
        Script::Fail(code, message) => {
            sleep(Duration::from_millis(900)).await;
            with_doc(shared, chat_id, |doc| {
                let _ = doc.replace_part(
                    ix,
                    0,
                    &Part::Error {
                        id: new_id(),
                        code: code.to_string(),
                        message: message.to_string(),
                    },
                );
                let _ = doc.set_status(ix, MessageStatus::Error);
                doc.commit();
            });
            mark_unread_if_hidden(shared, chat_id);
            return;
        }
    };

    if finished {
        with_doc(shared, chat_id, |doc| {
            let _ = doc.set_status(ix, MessageStatus::Done);
            doc.commit();
        });
        mark_unread_if_hidden(shared, chat_id);
        group_replies(shared, chat_id, text, &stop).await;
    } else {
        finish_cancelled(shared, chat_id, ix);
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
            let _ = doc.insert_message(&project_message(chat_id, p, line, now_ms()));
            doc.commit();
        });
        mark_unread_if_hidden(shared, chat_id);
    }
}

/// A fresh assistant row in `streaming`, no prose yet: only the working line shows.
fn insert_running_row(
    shared: &Arc<Shared>,
    chat_id: &str,
    project_id: &str,
    turn: TurnKind,
) -> usize {
    let (model, effort) = project_model(shared, project_id);
    with_doc(shared, chat_id, |doc| {
        let msg = Message {
            id: new_id(),
            chat_id: chat_id.to_string(),
            role: Role::Assistant,
            sender: "agent".into(),
            status: MessageStatus::Streaming,
            turn: Some(turn),
            decision: None,
            subagent_id: None,
            subagent_name: None,
            handoff_messages: None,
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
            asked_by_handle: None,
            owner_handle: None,
            usage: None,
            return_contract: None,
            waiting_user: false,
            waiting_approval: false,
            model: Some(model),
            effort: Some(effort),
            created_at: now_ms(),
            completed_at: None,
            call_id: None,
            activity: None,
            pending: false,
            parts: vec![Part::Status {
                id: new_id(),
                code: StatusKind::Working,
                message: String::new(),
            }],
            hidden_parts: Vec::new(),
            greeting: false,
        };
        let ix = doc.insert_message(&msg).unwrap_or(0);
        doc.commit();
        ix
    })
    .unwrap_or(0)
}

fn empty_text() -> Part {
    Part::Text {
        id: new_id(),
        text: String::new(),
    }
}

/// One `approval` part in the shape `arbos_core::approval` owns.
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
    Part::Approval {
        id: new_id(),
        approval_id: approval_id.to_string(),
        title: format!("Send {channel} to {to}: {subject}"),
        summary: body
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(160)
            .collect(),
        risk: ApprovalRisk::Send,
        payload: json!({ "kind": "draft_message", "channel": channel, "to": to, "subject": subject, "body": body }),
        state,
        expires_at: Some(decided_at.unwrap_or(now) + 24 * 60 * 60_000),
        decided_at,
        decided_by: decided_at.map(|_| DEVICE.to_string()),
        reason: None,
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
    let kind = match state {
        ApprovalState::Approved => "approve",
        ApprovalState::Rejected => "reject",
        ApprovalState::Pending | ApprovalState::Expired => return,
    };
    let found = with_doc(shared, &chat_id, |doc| {
        let mut payload = json!({ "approvalId": approval_id });
        if let Some(r) = reason.as_deref().filter(|r| !r.trim().is_empty()) {
            payload["reason"] = json!(r);
        }
        let _ = doc.push_command(kind, payload, DEVICE);
        let hit = (0..doc.len()).rev().find_map(|ix| {
            let m = doc.read_message(ix)?;
            let pix = m.parts.iter().position(|p| {
                matches!(p, Part::Approval { approval_id: a, state: ApprovalState::Pending, .. } if *a == approval_id)
            })?;
            let title = match &m.parts[pix] {
                Part::Approval { title, payload, .. } => (title.clone(), payload.clone()),
                _ => return None,
            };
            Some((ix, pix, title))
        });
        if let Some((ix, pix, _)) = &hit {
            let _ = doc.set_part_field(*ix, *pix, "state", json!(state));
            let _ = doc.set_part_field(*ix, *pix, "decidedAt", json!(now_ms()));
            let _ = doc.set_part_field(*ix, *pix, "decidedBy", json!(DEVICE));
            if let Some(r) = reason.as_deref().filter(|r| !r.trim().is_empty()) {
                let _ = doc.set_part_field(*ix, *pix, "reason", json!(r));
            }
            let _ = doc.set_field(*ix, "waitingApproval", json!(false));
        }
        doc.commit();
        hit
    })
    .flatten();
    let Some((_, _, (title, payload))) = found else {
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
                .filter(|r| !r.trim().is_empty())
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
    let ix = with_doc(shared, chat_id, |doc| {
        let mut msg = Message {
            id: new_id(),
            chat_id: chat_id.to_string(),
            role: Role::Assistant,
            sender: "agent".into(),
            status: MessageStatus::Streaming,
            turn: Some(TurnKind::Reply),
            decision: Some("reply".into()),
            subagent_id: None,
            subagent_name: None,
            handoff_messages: None,
            reactions: Default::default(),
            in_reply_to: None,
            usage: None,
            return_contract: None,
            waiting_user: false,
            waiting_approval: false,
            routine_name: None,
            sender_name: None,
            sender_icon: None,
            sender_handle: None,
            sender_avatar: None,
            sender_avatar_color: None,
            mentions: Vec::new(),
            reply_to: None,
            asked_by_handle: None,
            owner_handle: None,
            model: Some(model),
            effort: Some(effort),
            created_at: now_ms(),
            completed_at: None,
            call_id: None,
            activity: None,
            pending: false,
            parts: vec![empty_text()],
            hidden_parts: Vec::new(),
            greeting: false,
        };
        msg.parts.push(Part::Status {
            id: new_id(),
            code: StatusKind::Working,
            message: format!("Following up on approval {approval_id}"),
        });
        let ix = doc.insert_message(&msg).unwrap_or(0);
        doc.commit();
        ix
    })
    .unwrap_or(0);
    let refs: Vec<&str> = blocks.iter().map(String::as_str).collect();
    let stop = AtomicBool::new(false);
    stream_blocks(shared, chat_id, ix, 0, &refs, &stop).await;
    with_doc(shared, chat_id, |doc| {
        let _ = doc.set_status(ix, MessageStatus::Done);
        doc.commit();
    });
    mark_unread_if_hidden(shared, chat_id);
}

fn finish_cancelled(shared: &Arc<Shared>, chat_id: &str, ix: usize) {
    with_doc(shared, chat_id, |doc| {
        let _ = doc.push_part(
            ix,
            &Part::Status {
                id: new_id(),
                code: StatusKind::Note,
                message: "Stopped by you".into(),
            },
        );
        let _ = doc.set_status(ix, MessageStatus::Cancelled);
        doc.commit();
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
    ix: usize,
    pix: usize,
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
            let _ = doc.append_text(ix, pix, &chunk);
            doc.commit();
        });
        sleep(Duration::from_millis(STREAM_FRAME_MS)).await;
    }
    true
}

fn with_doc<R>(shared: &Arc<Shared>, chat_id: &str, f: impl FnOnce(&ChatDoc) -> R) -> Option<R> {
    let docs = shared.docs.lock().unwrap();
    docs.get(chat_id).map(f)
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
            muted: false,
            primary_chat: None,
            voice: None,
            vision_enabled: true,
            avatar_color_index: None,
            avatar_color_override: None,
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
            muted: false,
            primary_chat: None,
            voice: None,
            vision_enabled: true,
            avatar_color_index: None,
            avatar_color_override: None,
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
            muted: false,
            primary_chat: None,
            voice: None,
            vision_enabled: true,
            avatar_color_index: None,
            avatar_color_override: None,
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
        chat(
            CHAT_ARBOS_LAUNCH,
            PROJECT_ARBOS,
            "Launch planning",
            false,
            95,
        ),
        chat(CHAT_ARBOS_WIDGETS, PROJECT_ARBOS, "Beta release", false, 12),
        chat(
            CHAT_ARBOS_DRAFTS,
            PROJECT_ARBOS,
            "Investor update",
            false,
            12,
        ),
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
        self.user_with_parts(text, Vec::new());
    }

    /// `extra` parts (attachments) come before the text, as the clients write them.
    fn user_with_parts(&mut self, text: &str, mut extra: Vec<Part>) {
        self.t += 45_000;
        extra.push(Part::Text {
            id: new_id(),
            text: text.into(),
        });
        let _ = self.doc.insert_message(&Message {
            id: new_id(),
            chat_id: self.doc.chat_id.clone(),
            role: Role::User,
            sender: "user".into(),
            status: MessageStatus::Done,
            turn: None,
            decision: None,
            subagent_id: None,
            subagent_name: None,
            handoff_messages: None,
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
            asked_by_handle: None,
            owner_handle: None,
            usage: None,
            return_contract: None,
            waiting_user: false,
            waiting_approval: false,
            model: None,
            effort: None,
            created_at: self.t,
            completed_at: Some(self.t),
            call_id: None,
            activity: None,
            pending: false,
            parts: extra,
            hidden_parts: Vec::new(),
            greeting: false,
        });
    }

    fn assistant(&mut self, turn: TurnKind, status: MessageStatus, parts: Vec<Part>) {
        self.t += 20_000;
        let _ = self.doc.insert_message(&Message {
            id: new_id(),
            chat_id: self.doc.chat_id.clone(),
            role: Role::Assistant,
            sender: "agent".into(),
            status,
            turn: Some(turn),
            decision: None,
            subagent_id: None,
            subagent_name: None,
            handoff_messages: None,
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
            asked_by_handle: None,
            owner_handle: None,
            usage: None,
            return_contract: None,
            waiting_user: false,
            waiting_approval: false,
            model: Some(MODELS[0].into()),
            effort: Some(Effort::High),
            created_at: self.t,
            completed_at: Some(self.t + 9_000),
            call_id: None,
            activity: None,
            pending: false,
            parts,
            hidden_parts: Vec::new(),
            greeting: false,
        });
    }
}

/// Another Project's finished message in a group (`role: project`, F6).
fn project_message(chat_id: &str, project: &Project, text: &str, t: u64) -> Message {
    Message {
        id: new_id(),
        chat_id: chat_id.to_string(),
        role: Role::Project,
        sender: format!("project:{}", project.id),
        status: MessageStatus::Done,
        turn: None,
        decision: None,
        subagent_id: None,
        subagent_name: None,
        handoff_messages: None,
        reactions: Default::default(),
        in_reply_to: None,
        routine_name: None,
        sender_name: Some(project.name.clone()),
        sender_icon: Some(project.icon.clone()),
        sender_handle: None,
        sender_avatar: None,
        sender_avatar_color: None,
        mentions: Vec::new(),
        reply_to: None,
        asked_by_handle: None,
        owner_handle: None,
        usage: None,
        return_contract: None,
        waiting_user: false,
        waiting_approval: false,
        model: None,
        effort: None,
        created_at: t,
        completed_at: Some(t),
        call_id: None,
        activity: None,
        pending: false,
        parts: vec![Part::Text {
            id: new_id(),
            text: text.into(),
        }],
        hidden_parts: Vec::new(),
        greeting: false,
    }
}

impl Seeder<'_> {
    fn project(&mut self, project: &Project, text: &str) {
        self.t += 12_000;
        let _ = self
            .doc
            .insert_message(&project_message(&self.doc.chat_id, project, text, self.t));
    }
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
        },
    }
}

/// A 320×200 PNG (diagonal gradient tinted from the key) for fixture thumbnails and downloads.
fn fixture_png(key: &str) -> Vec<u8> {
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

fn seed_docs(shared: &Arc<Shared>) {
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
        s.assistant(
            TurnKind::Reply,
            MessageStatus::Done,
            vec![text(
                "A compact agent rail, clear chat tabs, and everything else within reach.",
            )],
        );
        main.commit();
    }
    add_doc(shared, main);

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
        research.commit();
    }
    add_doc(shared, research);

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
        launch.commit();
    }
    add_doc(shared, launch);

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
        widgets.commit();
    }
    add_doc(shared, widgets);
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
        s.assistant(
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
        let _ = drafts.set_field(drafts.len() - 1, "waitingApproval", json!(true));
        drafts.commit();
    }
    add_doc(shared, drafts);
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
        media.commit();
    }
    add_doc(shared, media);

    // Voice memo (F11): a spoken prompt keeps its audio as an attachment part.
    let voice = ChatDoc::new(CHAT_ARBOS_VOICE, PROJECT_ARBOS);
    {
        let mut s = Seeder {
            doc: &voice,
            t: base + 2_700_000,
        };
        s.user_with_parts(
            FIXTURE_TRANSCRIPT,
            vec![Part::Attachment {
                id: new_id(),
                attachment: Attachment {
                    name: "voice-memo.wav".into(),
                    mime: "audio/wav".into(),
                    blob_ref: Some(VOICE_SEED_BLOB.into()),
                    bytes: None,
                    kind: Some(AttachmentKind::Audio),
                    title: None,
                },
            }],
        );
        s.assistant(
            TurnKind::Reply,
            MessageStatus::Done,
            vec![text(
                "Three things today: the beta invite copy is due at noon, the pricing page review is at three, and two pull requests wait on you. Want me to start with the invite copy?",
            )],
        );
        voice.commit();
    }
    add_doc(shared, voice);

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
        group.commit();
    }
    add_doc(shared, group);

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
        affine.commit();
    }
    add_doc(shared, affine);

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
        comms.commit();
    }
    add_doc(shared, comms);
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
        muted: false,
        primary_chat: None,
        voice: None,
        vision_enabled: true,
        avatar_color_index: None,
        avatar_color_override: None,
    }
}

fn seed_people_projects() -> Vec<Project> {
    let mut owner = sam();
    owner.role = "owner".into();
    vec![
        people_space(DM_SAM, sam(), Vec::new()),
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
            muted: false,
            primary_chat: None,
            voice: None,
            vision_enabled: true,
            avatar_color_index: None,
            avatar_color_override: None,
        },
    ]
}

fn seed_people_chats() -> Vec<Chat> {
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
            last_message_at: now_ms().saturating_sub(12 * 60_000),
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
    }
}

fn text_part(text: &str) -> Part {
    Part::Text {
        id: new_id(),
        text: text.into(),
    }
}

fn seed_people_docs(shared: &Arc<Shared>) {
    let base = now_ms().saturating_sub(40 * 60_000);
    let doc = ChatDoc::new(DM_SAM, "people");
    let _ = doc.insert_message(&human_message(
        DM_SAM,
        &sam(),
        "Hey — are you around? I want to go over the beta invite copy before it ships.",
        base,
    ));
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
    let _ = doc.insert_message(&mine);
    let _ = doc.insert_message(&human_message(
        DM_SAM,
        &sam(),
        "Sending in a sec. Also: can we add your Arbos agent here so it can pull the numbers while we talk?",
        base + 150_000,
    ));
    doc.commit();
    add_doc(shared, doc);

    // Sam's shared project: Sam wrote first, the agent answered with attribution.
    let studio = ChatDoc::new(SHARED_STUDIO_CHAT, SHARED_STUDIO);
    let _ = studio.insert_message(&human_message(
        SHARED_STUDIO_CHAT,
        &sam(),
        "@jacob is in now. Studio: let's use this for the launch page work.",
        base + 200_000,
    ));
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
    let _ = studio.insert_message(&reply);
    studio.commit();
    add_doc(shared, studio);
}

/// `@slug` / `@handle` tokens in the text, resolved to member ids.
fn mentions_in(space: &Project, text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut out = Vec::new();
    for a in &space.agents {
        if lower.contains(&format!("@{}", a.handle)) {
            out.push(format!("agent:{}", a.project_id));
        }
    }
    for p in &space.people {
        if lower.contains(&format!("@{}", p.handle.to_lowercase())) {
            out.push(format!("user:{}", p.id));
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

fn emit_avatar(shared: &Shared, url: &str) {
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
                let _ = shared
                    .events
                    .send(SyncEvent::PeopleError("That is you.".into()));
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
                let _ = shared.events.send(SyncEvent::PeopleError(format!(
                    "No GitHub user named @{handle}."
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
        ClientCommand::SetTyping { .. } => true,
        ClientCommand::AddAgent {
            chat_id,
            project_id,
        } => {
            let agent = {
                let projects = shared.projects.lock().unwrap();
                projects
                    .iter()
                    .find(|p| p.id == *project_id && !p.is_people())
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
                        description: None,
                        model: None,
                        owner: None,
                    })
            };
            if let Some(agent) = agent {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(space) = projects.iter_mut().find(|p| p.id == *chat_id) {
                    if !space
                        .agents
                        .iter()
                        .any(|a| a.project_id == agent.project_id)
                    {
                        space.agents.push(agent);
                    }
                }
            }
            emit_projects(shared);
            true
        }
        ClientCommand::RemoveAgent {
            chat_id,
            project_id,
        } => {
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(space) = projects.iter_mut().find(|p| p.id == *chat_id) {
                    space.agents.retain(|a| a.project_id != *project_id);
                }
            }
            emit_projects(shared);
            true
        }
        ClientCommand::RemoveMember { chat_id, .. } | ClientCommand::LeaveChat { chat_id } => {
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
            let _ = shared
                .events
                .send(SyncEvent::Notice(format!("@{handle} blocked.")));
            true
        }
        ClientCommand::InviteToProject { project_id, handle } => {
            let handle = handle.trim().trim_start_matches('@').to_lowercase();
            let Some(other) = directory().into_iter().find(|p| p.handle == handle) else {
                let _ = shared.events.send(SyncEvent::PeopleError(format!(
                    "No GitHub user named @{handle}."
                )));
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
                    with_doc(&shared, &chat_id, |doc| {
                        let _ = doc.insert_message(&msg);
                        doc.commit();
                    });
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
        with_doc(&shared, &chat_id, |doc| {
            let _ = doc.insert_message(&msg);
            doc.commit();
        });
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
                let _ = with_doc(&shared, &chat_id, |doc| {
                    let _ = doc.insert_message(&msg);
                    doc.commit();
                });
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
    let assistant_id = new_id();
    let ix = with_doc(shared, chat_id, |doc| {
        let msg = Message {
            id: assistant_id.clone(),
            chat_id: chat_id.to_string(),
            role: Role::Assistant,
            sender: format!("agent:{}", agent.project_id),
            status: MessageStatus::Streaming,
            turn: None,
            decision: None,
            subagent_id: None,
            subagent_name: None,
            handoff_messages: None,
            reactions: Default::default(),
            in_reply_to: None,
            routine_name: None,
            sender_name: Some(agent.name.clone()),
            sender_icon: Some(agent.icon.clone()),
            sender_handle: Some(agent.handle.clone()),
            sender_avatar: None,
            sender_avatar_color: None,
            mentions: Vec::new(),
            reply_to: None,
            asked_by_handle: Some(asker.handle.clone()),
            owner_handle: Some(agent.owner_handle.clone()),
            usage: None,
            return_contract: None,
            waiting_user: false,
            waiting_approval: false,
            model: None,
            effort: None,
            created_at: now_ms(),
            completed_at: None,
            call_id: None,
            activity: None,
            pending: false,
            parts: vec![Part::Status {
                id: new_id(),
                code: StatusKind::Deciding,
                message: format!("{} is reading…", agent.name),
            }],
            hidden_parts: Vec::new(),
            greeting: false,
        };
        let ix = doc.insert_message(&msg).unwrap_or(0);
        doc.commit();
        ix
    })
    .unwrap_or(0);
    sleep(Duration::from_millis(600)).await;
    let pix = with_doc(shared, chat_id, |doc| {
        let _ = doc.set_part_field(ix, 0, "hidden", json!(true));
        let pix = doc.push_part(ix, &text_part("")).unwrap_or(1);
        doc.commit();
        pix
    })
    .unwrap_or(1);
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
    stream_blocks(shared, chat_id, ix, pix, &[&quoted, body], &stop).await;
    with_doc(shared, chat_id, |doc| {
        let _ = doc.set_status(ix, MessageStatus::Done);
        let _ = doc.set_field(ix, "completedAt", json!(now_ms()));
        doc.commit();
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
    }
}

fn system_message(chat_id: &str, text: &str) -> Message {
    Message {
        id: new_id(),
        chat_id: chat_id.to_string(),
        role: Role::System,
        sender: "system".into(),
        sender_handle: None,
        sender_name: None,
        sender_avatar: None,
        sender_avatar_color: None,
        mentions: Vec::new(),
        status: MessageStatus::Done,
        turn: None,
        decision: None,
        subagent_id: None,
        subagent_name: None,
        handoff_messages: None,
        reactions: Default::default(),
        in_reply_to: None,
        routine_name: None,
        sender_icon: None,
        reply_to: None,
        asked_by_handle: None,
        owner_handle: None,
        usage: None,
        return_contract: None,
        waiting_user: false,
        waiting_approval: false,
        model: None,
        effort: None,
        created_at: now_ms(),
        completed_at: Some(now_ms()),
        call_id: None,
        activity: None,
        pending: false,
        parts: vec![Part::Text {
            id: new_id(),
            text: text.to_string(),
        }],
        hidden_parts: Vec::new(),
        greeting: false,
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
