//! Map relay HTTP DTOs (`arbos_core::api`) onto Mac model types.
//!
//! Free-form JSON fields on the DTOs (`models`, `spend_today`, member
//! `settings`) stay as [`serde_json::Value`]; only the talking-model id /
//! effort, spend today, and reply policy are read from them.

use arbos_core::api::{
    AccountUserDto, CallDto, CallReachabilityDto, ChannelBindingDto, ChannelPairingDto,
    ChannelPlatformDto, ChannelsOverviewDto, ChatDto, ChatMemberDto, ConnectorDto,
    ConnectorsCatalogDto, ConnectorsListDto, CreateChatResponseDto, GetProjectResponseDto,
    LocalDeviceDto, LocalDevicesDto, MachineStatusDto, MeResponseDto, ModelListDto, PeopleChatDto,
    ProjectMemberDto, ProjectMembersDto, ProjectOwnerDto, ProjectSecretItemDto,
    ProjectSecretSummaryDto, ProjectSecretsDto, ProjectVaultSummaryDto, ProviderDto, RoutineDto,
    RoutineRunDto, SecretAccessDto, SecretActivityDto, SecretDirectorDto, SecretDto,
    SecretRequestDto, SecretsOverviewDto, SenderDto, SkillDto, TokenResponseDto, TranscriptionDto,
    UserCardDto, VaultDto, VoiceCatalogDto, VoiceDto,
};
use arbos_core::wire::BoxState as WireBox;
use serde_json::Value;
use std::collections::HashMap;

use crate::model::{
    AccountView, AgentMember, BoxState, Call, CallKind, ChannelBinding, ChannelPairing,
    ChannelPlatform, ChannelPlatformState, ChannelsView, Chat, ChatKind, ConnectedVault, Connector,
    ConnectorAction, ConnectorProvider, ConnectorStatus, ConnectorsView, Effort, Id, LocalDevice,
    LocalView, Machine, MachineState, MessageGrant, Person, Project, ProjectMachine,
    ProjectSecretAccess, ProjectSecrets, Reachable, ReachableDevice, ReplyPolicy, Routine,
    RoutineRun, RoutineRunStatus, RoutineTrigger, Secret, SecretActivity, SecretAgent,
    SecretDirector, SecretRequest, SecretScope, SecretsView, Sender, Skill, SpaceKind, Voice,
};
use crate::sync::credentials::{AccountUser, DeviceCredentials};

/// Wire box word → the string `box_state_from` already understands.
fn wire_box_word(state: WireBox) -> &'static str {
    match state {
        WireBox::Awake => "awake",
        WireBox::Waking => "waking",
        WireBox::Asleep => "asleep",
        WireBox::Error => "error",
    }
}

fn box_state_from(state: &str, box_state: &str) -> BoxState {
    match state {
        "awake" => BoxState::Awake,
        "waking" => BoxState::Waking,
        "hibernated_light" => BoxState::HibernatedLight,
        "hibernated_deep" => BoxState::HibernatedDeep,
        "stopped_by_user" => BoxState::StoppedByUser,
        "error" => BoxState::Error,
        _ => match box_state {
            "awake" => BoxState::Awake,
            "waking" => BoxState::Waking,
            "error" => BoxState::Error,
            _ => BoxState::HibernatedLight,
        },
    }
}

fn effort_from(v: &Value) -> Effort {
    match v.as_str().unwrap_or("") {
        "high" | "xhigh" | "max" => Effort::High,
        "medium" => Effort::Medium,
        _ => Effort::Low,
    }
}

fn chat_kind(s: &str) -> ChatKind {
    serde_json::from_value(Value::String(s.to_owned())).unwrap_or_default()
}

fn non_empty(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_owned())
}

fn color_index(effective: i64) -> u8 {
    (effective.rem_euclid(7)) as u8
}

/// D92 leaf / colour indices travel as small non-negative integers; anything else = automatic.
fn leaf_index(v: i64) -> Option<u8> {
    u8::try_from(v).ok()
}

impl From<&ProjectOwnerDto> for Person {
    fn from(o: &ProjectOwnerDto) -> Self {
        Person {
            id: o.id.clone(),
            handle: o.handle.clone(),
            name: non_empty(&o.name),
            avatar_url: o.avatar_url.clone().unwrap_or_default(),
            avatar_color_index: Some(color_index(o.avatar_color_index_effective)),
            status: "member".into(),
            role: String::new(),
            last_seen_at: o.last_seen_at,
            online: o.online,
        }
    }
}

impl From<&UserCardDto> for Person {
    fn from(c: &UserCardDto) -> Self {
        Person {
            id: c.id.clone(),
            handle: c.handle.clone(),
            name: c.name.clone().filter(|n| !n.is_empty()),
            avatar_url: c.avatar_url.clone().unwrap_or_default(),
            avatar_color_index: c.avatar_color_index_effective.map(color_index),
            status: if c.status.is_empty() {
                "member".into()
            } else {
                c.status.clone()
            },
            role: String::new(),
            last_seen_at: c.last_seen_at.flatten(),
            online: c.online.unwrap_or(false),
        }
    }
}

impl From<&AccountUserDto> for AccountUser {
    fn from(u: &AccountUserDto) -> Self {
        AccountUser {
            id: u.id.clone(),
            login: u.login.clone().flatten().unwrap_or_default(),
            avatar_url: u.avatar_url.clone().flatten().unwrap_or_default(),
            name: u.name.clone().flatten().unwrap_or_default(),
        }
    }
}

impl From<&VoiceDto> for Voice {
    fn from(v: &VoiceDto) -> Self {
        Voice {
            id: v.id.clone(),
            name: if v.name.is_empty() {
                v.id.clone()
            } else {
                v.name.clone()
            },
            description: v.description.clone(),
            sample_url: v.sample_url.clone(),
        }
    }
}

/// Model ids clients keep from `GET /proxy/v1/models` (filtered like the apps).
pub fn model_ids_from_dto(dto: &ModelListDto) -> Vec<String> {
    let allowed = ["anthropic/", "openai/", "google/", "x-ai/"];
    let mut ids: Vec<String> = dto
        .data
        .iter()
        .map(|m| m.id.clone())
        .filter(|id| !id.contains(':'))
        .filter(|id| allowed.iter().any(|p| id.starts_with(p)))
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// Voices catalogue → default id + model voices (relative sample paths resolved by the caller).
pub fn voices_from_dto(dto: &VoiceCatalogDto) -> (Option<String>, Vec<Voice>) {
    let default = (!dto.default_voice.is_empty()).then(|| dto.default_voice.clone());
    let voices: Vec<Voice> = dto
        .voices
        .iter()
        .map(Voice::from)
        .filter(|v| !v.id.is_empty())
        .collect();
    (default, voices)
}

/// Transcript text from a transcription reply (unknown keys ignored).
pub fn transcript_text(dto: &TranscriptionDto) -> String {
    dto.text.trim().to_owned()
}

/// Apply `/auth/token` or `/auth/refresh` onto credentials (keeps prior refresh/user when omitted).
pub fn credentials_from_token(
    dto: &TokenResponseDto,
    relay_url: &str,
    previous: Option<&DeviceCredentials>,
    now_ms: u64,
) -> DeviceCredentials {
    DeviceCredentials {
        relay_url: relay_url.to_owned(),
        device_id: if dto.device_id.is_empty() {
            previous
                .map(|c| c.device_id.clone())
                .unwrap_or_else(|| "device".into())
        } else {
            dto.device_id.clone()
        },
        access_token: dto.access_token.clone(),
        refresh_token: if dto.refresh_token.is_empty() {
            previous.and_then(|c| c.refresh_token.clone())
        } else {
            Some(dto.refresh_token.clone())
        },
        expires_at: if dto.expires_in > 0 {
            now_ms + (dto.expires_in as u64) * 1000
        } else {
            0
        },
        user: {
            let u: AccountUser = (&dto.user).into();
            if u.id.is_empty() {
                previous.and_then(|c| c.user.clone())
            } else {
                Some(u)
            }
        },
    }
}

impl From<&SenderDto> for Sender {
    fn from(s: &SenderDto) -> Self {
        Sender {
            kind: s.kind.clone(),
            id: s.id.clone(),
            name: s.name.clone(),
            handle: s.handle.clone(),
            avatar: s.avatar.clone(),
            avatar_color: s.avatar_color.clone(),
            avatar_color_index: Some(color_index(s.avatar_color_index_effective)),
            avatar_leaf: s.avatar_leaf_effective.and_then(leaf_index),
            avatar_leaf_color: s.avatar_leaf_color_effective.and_then(leaf_index),
            avatar_url: s.avatar_url.clone().unwrap_or_default(),
        }
    }
}

fn call_kind(s: &str) -> CallKind {
    match s {
        "video" => CallKind::Video,
        _ => CallKind::Voice,
    }
}

impl From<&CallDto> for Call {
    fn from(c: &CallDto) -> Self {
        Call {
            call_id: c.call_id.clone(),
            chat_id: c.chat_id.clone(),
            project_id: c.project_id.clone(),
            kind: call_kind(&c.kind),
            room: c.room.clone(),
            livekit_url: c.livekit_url.clone().unwrap_or_default(),
            token: c.token.clone().filter(|t| !t.is_empty()),
            token_expires_at: c.token_expires_at.filter(|t| *t > 0).map(|t| t as u64),
            started_by: Sender::from(&c.started_by),
            started_at: c.started_at.max(0) as u64,
            ended_at: c.ended_at.filter(|t| *t > 0).map(|t| t as u64),
            reason: c.reason.clone(),
            participants: c.participants.iter().map(Sender::from).collect(),
            agents: c.agents.iter().map(Sender::from).collect(),
            speak_mode: c.speak_mode.clone(),
            invited_speakers: c.invited_speakers.iter().map(Sender::from).collect(),
            can_speak: c.can_speak,
        }
    }
}

impl From<&CallReachabilityDto> for Reachable {
    fn from(r: &CallReachabilityDto) -> Self {
        Reachable {
            user_id: r.user_id.clone(),
            handle: r.handle.clone(),
            name: r.name.clone(),
            any_live: r.any_live,
            any_voip: r.any_voip,
            any_push: r.any_push,
            devices: r
                .devices
                .iter()
                .map(|d| ReachableDevice {
                    device_id: d.device_id.clone(),
                    name: d.name.clone(),
                    kind: d.kind.clone(),
                    platform: d.platform.clone().unwrap_or_default(),
                    app_version: d.app_version.clone().unwrap_or_default(),
                    polled_within_30s: d.polled_within_30s,
                    last_seen_at: d.last_seen_at.filter(|t| *t > 0).map(|t| t as u64),
                })
                .collect(),
        }
    }
}

impl From<&SecretAccessDto> for SecretAgent {
    fn from(a: &SecretAccessDto) -> Self {
        SecretAgent {
            project_id: a.project_id.clone(),
            name: a.name.clone(),
            access_id: a.access_id.clone(),
            enabled: a.enabled,
            note: a.note.clone(),
            hosts: a.hosts.clone(),
            scope: SecretScope {
                kind: a.scope.kind.clone(),
                chat_id: a.scope.chat_id.clone(),
            },
            last_used_at: a.last_used_at,
            disabled_at: a.disabled_at,
        }
    }
}

impl From<&SecretDirectorDto> for SecretDirector {
    fn from(d: &SecretDirectorDto) -> Self {
        SecretDirector {
            user_id: d.user_id.clone(),
            handle: d.handle.clone(),
            name: d.name.clone(),
            avatar_url: d.avatar_url.clone(),
            note: d.note.clone(),
            created_at: d.created_at,
            enabled: d.enabled.unwrap_or(true),
            disabled_at: d.disabled_at,
            scope: d
                .scope
                .as_ref()
                .map(|s| SecretScope {
                    kind: s.kind.clone(),
                    chat_id: s.chat_id.clone(),
                })
                .unwrap_or_default(),
            last_asked_at: d.last_asked_at,
        }
    }
}

impl From<&SecretDto> for Secret {
    fn from(d: &SecretDto) -> Self {
        Secret {
            id: d.id.clone(),
            name: d.name.clone(),
            env: d.env.clone(),
            kind: d.kind.clone(),
            note: d.note.clone(),
            hosts: d.hosts.clone(),
            ask_every_time: d.ask_every_time,
            inject_env: d.inject_env,
            expires_at: d.expires_at,
            version: d.version,
            created_at: d.created_at,
            last_used_at: d.last_used_at,
            username: d.username.clone(),
            vault_name: d.vault_name.clone(),
            agents: d.agents.iter().map(SecretAgent::from).collect(),
            directors: if d.people.is_empty() {
                d.directors.iter().map(SecretDirector::from).collect()
            } else {
                d.people.iter().map(SecretDirector::from).collect()
            },
            others_ask: d.others_ask.clone().unwrap_or_else(|| "refuse".into()),
            source: d.source.as_ref().map(|src| crate::model::SecretSource {
                kind: src.kind.clone(),
                project_id: src.project_id.clone(),
                project_name: src.project_name.clone(),
                chat_id: src.chat_id.clone(),
            }),
        }
    }
}

impl From<&SecretRequestDto> for SecretRequest {
    fn from(r: &SecretRequestDto) -> Self {
        SecretRequest {
            id: r.id.clone(),
            project_id: r.project_id.clone(),
            project_name: r.project_name.clone(),
            chat_id: r.chat_id.clone(),
            name: r.name.clone(),
            purpose: r.purpose.clone(),
            use_kind: r.use_kind.clone(),
            host: r.host.clone(),
            status: r.status.clone(),
            created_at: r.created_at,
            decided_at: r.decided_at,
            asked_by_handle: r.asked_by_handle.clone(),
            asked_by_name: r.asked_by_name.clone(),
        }
    }
}

impl From<&SecretActivityDto> for SecretActivity {
    fn from(a: &SecretActivityDto) -> Self {
        SecretActivity {
            id: a.id,
            at: a.at,
            action: a.action.clone(),
            name: a.name.clone(),
            target_kind: a.target_kind.clone(),
            project_id: a.project_id.clone(),
            project_name: a.project_name.clone(),
            host: a.host.clone(),
            by: a.by.clone(),
            detail: a.detail.clone(),
        }
    }
}

impl From<&VaultDto> for ConnectedVault {
    fn from(v: &VaultDto) -> Self {
        ConnectedVault {
            id: v.id.clone(),
            provider: v.provider.clone(),
            name: v.name.clone(),
            items_count: v.items_count,
            token_expires_at: v.token_expires_at,
            status: v.status.clone(),
            agents: v.agents.iter().map(SecretAgent::from).collect(),
            limits: serde_json::to_value(&v.limits).unwrap_or(Value::Null),
            items: v.items.as_array().cloned().unwrap_or_default(),
            env_items: v.env_items,
        }
    }
}

fn secret_from_summary(s: &ProjectSecretSummaryDto) -> Secret {
    Secret {
        id: s.id.clone(),
        name: s.name.clone(),
        env: s.env.clone(),
        kind: s.kind.clone(),
        note: s.note.clone(),
        hosts: s.hosts.clone(),
        ask_every_time: false,
        inject_env: s.inject_env,
        expires_at: None,
        version: 1,
        created_at: 0,
        last_used_at: s.last_used_at,
        username: None,
        vault_name: s.vault_name.clone(),
        agents: Vec::new(),
        directors: Vec::new(),
        others_ask: "refuse".into(),
        source: None,
    }
}

fn vault_from_summary(v: &ProjectVaultSummaryDto) -> ConnectedVault {
    ConnectedVault {
        id: v.id.clone(),
        provider: v.provider.clone(),
        name: v.name.clone(),
        items_count: v.items_count,
        token_expires_at: None,
        status: v.status.clone(),
        agents: Vec::new(),
        limits: Value::Null,
        items: Vec::new(),
        env_items: 0,
    }
}

impl From<&ProjectSecretItemDto> for ProjectSecretAccess {
    fn from(i: &ProjectSecretItemDto) -> Self {
        ProjectSecretAccess {
            access_id: i.access_id.clone(),
            enabled: i.enabled,
            note: i.note.clone(),
            hosts: i.hosts.clone(),
            last_used_at: i.last_used_at,
            disabled_at: i.disabled_at,
            secret: i.secret.as_ref().map(secret_from_summary),
            vault: i.vault.as_ref().map(vault_from_summary),
        }
    }
}

/// `GET /secrets` (+ requests list) → Settings secrets view.
pub fn secrets_view_from_dto(
    overview: &SecretsOverviewDto,
    requests: &[SecretRequestDto],
) -> SecretsView {
    SecretsView {
        paused: overview.paused,
        secrets: overview.secrets.iter().map(Secret::from).collect(),
        vaults: overview.vaults.iter().map(ConnectedVault::from).collect(),
        providers: overview.providers.clone(),
        vault_connect: overview.features.vault_connect.unwrap_or(false),
        requests: requests.iter().map(SecretRequest::from).collect(),
        activity: overview.activity.iter().map(SecretActivity::from).collect(),
        loaded: true,
    }
}

impl From<&ProjectSecretsDto> for ProjectSecrets {
    fn from(p: &ProjectSecretsDto) -> Self {
        ProjectSecrets {
            paused: p.paused,
            owner: p.owner,
            items: p.items.iter().map(ProjectSecretAccess::from).collect(),
            requests: p.requests.iter().map(SecretRequest::from).collect(),
            activity: p.activity.iter().map(SecretActivity::from).collect(),
        }
    }
}

/// `RoutineDto` → model; `trigger` stays free-form on the wire and becomes [`RoutineTrigger`].
pub fn routine_from_dto(d: &RoutineDto) -> Option<Routine> {
    let trigger: RoutineTrigger = serde_json::from_value(d.trigger.clone()).ok()?;
    Some(Routine {
        id: d.id.clone(),
        project_id: d.project_id.clone(),
        chat_id: d.chat_id.clone(),
        name: d.name.clone(),
        prompt: d.prompt.clone(),
        trigger,
        enabled: d.enabled,
        created_at: d.created_at,
        last_run_at: d.last_run_at,
        next_run_at: d.next_run_at,
        fire_url: d.fire_url.clone(),
    })
}

impl From<&RoutineRunDto> for RoutineRun {
    fn from(r: &RoutineRunDto) -> Self {
        let status = serde_json::from_value(Value::String(r.status.clone()))
            .unwrap_or(RoutineRunStatus::Other);
        RoutineRun {
            id: r.id.clone(),
            routine_id: r.routine_id.clone(),
            started_at: r.started_at,
            finished_at: r.finished_at,
            status,
            message_id: r.message_id.clone(),
            note: r.note.clone(),
        }
    }
}

/// `GET /me` → Settings account header.
pub fn account_view_from_me(dto: &MeResponseDto) -> AccountView {
    AccountView {
        id: dto.user.id.clone(),
        login: dto.user.login.clone().flatten().unwrap_or_default(),
        name: dto.user.name.clone().flatten().unwrap_or_default(),
        avatar_url: dto.user.avatar_url.clone().flatten().unwrap_or_default(),
        device_name: dto
            .device
            .as_ref()
            .map(|d| d.name.clone())
            .unwrap_or_default(),
        balance_usd: dto.credits.balance_usd,
        spent_today_usd: dto.credits.spent_today_usd,
        daily_cap_usd: dto.credits.daily_cap_usd,
    }
}

fn machine_state_from_word(word: &str) -> MachineState {
    match word {
        "awake" => MachineState::Awake,
        "waking" => MachineState::Waking,
        "error" => MachineState::Error,
        _ => MachineState::Asleep,
    }
}

/// Apply `GET /machine` onto the live machine snapshot (keeps waitlist/provider when absent).
pub fn apply_machine_status(m: &mut Machine, dto: &MachineStatusDto) {
    let word = match dto.state {
        WireBox::Awake => "awake",
        WireBox::Waking => "waking",
        WireBox::Asleep => "asleep",
        WireBox::Error => "error",
    };
    let state = machine_state_from_word(word);
    let since = (dto.since > 0).then_some(dto.since as u64);
    if m.state != state {
        m.since = since.or(Some(crate::model::now_ms()));
    } else if since.is_some() {
        m.since = since;
    }
    m.state = state;
    m.detail = None;
    m.progress = None;
    m.spec = Some(format!(
        "{} vCPU · {} GiB · {} GiB",
        dto.spec.cpu, dto.spec.mem_gib, dto.spec.disk_gib
    ));
    if !dto.spec.class.is_empty() {
        m.class = Some(dto.spec.class.clone());
    }
    m.cpu = Some(dto.spec.cpu as u32);
    m.mem_gib = Some(dto.spec.mem_gib as u32);
    if let Some(p) = dto.provider.as_deref().filter(|s| !s.is_empty()) {
        m.provider = Some(p.to_owned());
    }
    if let Some(img) = dto.image.current.as_deref().filter(|s| !s.is_empty()) {
        m.image = Some(img.to_owned());
    }
    m.spend_today_usd = dto.spend_today_usd;
    m.spend_cap_usd = dto.spend_cap_usd;
    m.always_on = dto.always_on;
}

fn person_from_project_member(m: &ProjectMemberDto) -> Person {
    let id = if m.user_id.is_empty() {
        m.id.clone()
    } else {
        m.user_id.clone()
    };
    Person {
        id: id.clone(),
        handle: m.handle.clone().unwrap_or(id),
        name: m.name.clone().filter(|n| !n.is_empty()),
        avatar_url: m.avatar_url.clone().unwrap_or_default(),
        avatar_color_index: Some(color_index(m.avatar_color_index_effective)),
        status: if m.status.is_empty() {
            "member".into()
        } else {
            m.status.clone()
        },
        role: if m.role.is_empty() {
            "member".into()
        } else {
            m.role.clone()
        },
        last_seen_at: m.last_seen_at,
        online: m.online,
    }
}

fn machine_from_dto(m: &arbos_core::api::MachineDto) -> ProjectMachine {
    ProjectMachine {
        state: wire_box_word(m.state).to_owned(),
        since: Some(m.since),
        class: m.class.clone(),
        snapshot: m.snapshot.clone(),
        image_outdated: m.image_outdated,
        always_on: m.always_on,
    }
}

/// `GET /projects/{id}` (+ optional members) → rail [`Project`].
pub(crate) fn project_from_dto(
    detail: &GetProjectResponseDto,
    members: Option<&ProjectMembersDto>,
) -> Project {
    let p = &detail.project;
    let talking = p
        .models
        .get("talking_model")
        .cloned()
        .unwrap_or(Value::Null);
    let people = members
        .map(|m| m.members.iter().map(person_from_project_member).collect())
        .unwrap_or_default();
    Project {
        id: p.id.clone(),
        name: if p.name.is_empty() {
            "Project".into()
        } else {
            p.name.clone()
        },
        icon: if p.icon.is_empty() {
            "Ω".into()
        } else {
            p.icon.clone()
        },
        color: "#8b7cf6".into(),
        state: box_state_from(&p.state, wire_box_word(detail.box_state)),
        state_detail: None,
        wake_progress: None,
        // Empty when the relay omits talking_model; refresh_registry fills from
        // the previous project, then the default id.
        model: talking
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .or_else(|| p.model.clone().filter(|s| !s.is_empty()))
            .unwrap_or_default(),
        effort: effort_from(talking.get("effort").unwrap_or(&Value::Null)),
        spend_today_usd: detail.spend_today.as_f64().unwrap_or(0.0),
        spend_cap_usd: p.spend_cap_usd,
        kind: SpaceKind::Project,
        agents: Vec::new(),
        pending: false,
        people,
        shared: p.shared.unwrap_or(false),
        owner_handle: p.owner_handle.clone().flatten().filter(|h| !h.is_empty()),
        archived_at: p.archived_at.flatten().filter(|t| *t > 0).map(|t| t as u64),
        avatar: non_empty(&p.avatar),
        avatar_color: non_empty(&p.avatar_color),
        description: non_empty(&p.description),
        owner_person: Some(Person::from(&p.owner)),
        machine_info: Some(machine_from_dto(&p.machine)),
        outside: None,
        muted: false,
        silenced: false,
        silenced_until: None,
        primary_chat: p.primary_chat_id.as_deref().and_then(non_empty),
        voice: p.voice.as_deref().and_then(non_empty),
        vision_enabled: p.vision_enabled,
        avatar_color_index: Some(color_index(p.avatar_color_index_effective)),
        avatar_color_override: p.avatar_color_index.map(color_index),
        avatar_leaf: p.avatar_leaf.and_then(leaf_index),
        avatar_leaf_color: p.avatar_leaf_color.and_then(leaf_index),
        image_url: None,
        avatar_version: p.avatar_version.unwrap_or(0),
        message_grants: p.message_grants.iter().map(message_grant_from).collect(),
        blocked_senders: Vec::new(),
        my_role: None,
    }
}

fn message_grant_from(g: &arbos_core::api::MessageGrantDto) -> MessageGrant {
    MessageGrant {
        grantee: g.grantee.clone(),
        kind: g.kind.clone(),
        id: g.id.clone(),
        name: g.name.clone().filter(|n| !n.is_empty()),
        handle: g.handle.clone().filter(|h| !h.is_empty()),
        avatar_url: g.avatar_url.clone().unwrap_or_default(),
        enabled: g.enabled,
        note: g.note.clone(),
        created_at: g.created_at,
        last_at: g.last_at,
    }
}

/// The relay's verdict for one chat: `GET /me/unread` is the user's read cursor and names
/// every chat with something unread (with a message count), so when it answered, a chat it
/// leaves out is read. Only when it did not answer (`None`: older relay) does the chat's own
/// `unread` flag count — then `known_count` (at least 1) stands in for the missing total.
pub(crate) fn relay_unread_for(
    unread_by_chat: Option<&HashMap<Id, u32>>,
    chat_id: &str,
    chat_flag: bool,
    known_count: u32,
) -> u32 {
    match unread_by_chat {
        Some(map) => map.get(chat_id).copied().unwrap_or(0),
        None => {
            if chat_flag {
                known_count.max(1)
            } else {
                0
            }
        }
    }
}

/// Merge the relay's unread message count with a local read mark.
///
/// The relay wins, except when this device marked the chat read at or after
/// the relay's last activity (our `PUT /read` has not landed yet).
pub(crate) fn merged_unread(
    relay_unread: u32,
    local_read_at: Option<i64>,
    last_activity_at: i64,
) -> u32 {
    if local_read_at.is_some_and(|at| at >= last_activity_at) {
        return 0;
    }
    relay_unread
}

/// Merge the relay's chat list with what this Mac already holds.
///
/// - Keeps a local tab whose `NewChat` POST is still in flight (`pending_local_ids`).
/// - Keeps a local `archived_at` / `title` when that field was edited after
///   `refresh_started_at` (the relay answer is older than the edit).
/// - Drops a chat the relay no longer lists when it has no pending local create.
pub(crate) fn merge_chats(
    relay: &[Chat],
    local: &[Chat],
    refresh_started_at: u64,
    local_changed_at: &std::collections::HashMap<String, u64>,
    pending_local_ids: &std::collections::HashSet<String>,
) -> Vec<Chat> {
    use std::collections::{HashMap, HashSet};

    let local_by_id: HashMap<&str, &Chat> = local.iter().map(|c| (c.id.as_str(), c)).collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<Chat> = Vec::with_capacity(relay.len() + pending_local_ids.len());

    for r in relay {
        let mut c = r.clone();
        if let Some(l) = local_by_id.get(c.id.as_str()) {
            c.last_message_at = c.last_message_at.max(l.last_message_at);
            let changed = local_changed_at.get(&c.id).copied().unwrap_or(0);
            if changed > refresh_started_at {
                c.archived_at = l.archived_at;
                c.title = l.title.clone();
            } else if c.title == "New chat" && l.title != "New chat" {
                c.title = l.title.clone();
            }
        }
        seen.insert(c.id.clone());
        out.push(c);
    }

    for l in local {
        if seen.contains(&l.id) {
            continue;
        }
        if pending_local_ids.contains(&l.id) {
            out.push(l.clone());
        }
    }
    out
}

/// After `NewChat` succeeds, queued sends keep their message ids and target the created chat.
pub(crate) fn remap_queued_send_targets(
    created_chat_id: &str,
    queued_message_ids: &[String],
) -> Vec<(String, String)> {
    queued_message_ids
        .iter()
        .map(|mid| (created_chat_id.to_owned(), mid.clone()))
        .collect()
}

/// `ChatDto` / create-chat nested chat → [`Chat`].
pub(crate) fn chat_from_dto(dto: &ChatDto) -> Chat {
    Chat {
        id: dto.id.clone(),
        project_id: dto.project_id.clone(),
        title: if dto.title.is_empty() {
            "New chat".into()
        } else {
            dto.title.clone()
        },
        is_pinned: dto.is_pinned,
        is_system: dto.is_system,
        kind: chat_kind(&dto.kind),
        participants: dto.participants.clone(),
        unread: false,
        unread_count: 0,
        last_message_at: dto.created_at.max(0) as u64,
        archived_at: dto
            .archived_at
            .flatten()
            .filter(|t| *t > 0)
            .map(|t| t as u64),
    }
}

/// Nested chat on `CreateChatResponseDto`.
pub(crate) fn chat_from_create(dto: &CreateChatResponseDto) -> Chat {
    chat_from_dto(&dto.chat)
}

fn agent_from_member(m: &ChatMemberDto, pid: &str, http_url: &str) -> AgentMember {
    let project = m.project.as_ref();
    let name = project
        .map(|p| p.name.as_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("Agent")
        .to_owned();
    let reply_policy = match m
        .settings
        .as_ref()
        .and_then(|s| s.get("reply_policy"))
        .and_then(Value::as_str)
    {
        Some("every_message") => ReplyPolicy::EveryMessage,
        _ => ReplyPolicy::Mention,
    };
    AgentMember {
        project_id: pid.to_owned(),
        handle: m
            .handle
            .clone()
            .unwrap_or_else(|| Project::mention_slug(&name)),
        icon: project
            .map(|p| p.icon.as_str())
            .filter(|i| !i.is_empty())
            .unwrap_or("Ω")
            .to_owned(),
        color: project
            .map(|p| p.color.as_str())
            .filter(|c| !c.is_empty())
            .unwrap_or("#8b7cf6")
            .to_owned(),
        name,
        owner_id: m.owner.clone().unwrap_or_default(),
        owner_handle: m.owner_handle.clone().flatten().unwrap_or_default(),
        description: m
            .description
            .clone()
            .filter(|d| !d.is_empty())
            .or_else(|| project.and_then(|p| non_empty(&p.description))),
        model: project.and_then(|p| p.model.as_deref()).and_then(non_empty),
        owner: project.map(|p| Person::from(&p.owner)),
        reply_policy,
        avatar: project.and_then(|p| non_empty(&p.avatar)),
        avatar_color: project
            .and_then(|p| non_empty(&p.avatar_color))
            .or_else(|| m.avatar_color.as_deref().and_then(non_empty)),
        avatar_color_index: project
            .map(|p| color_index(p.avatar_color_index_effective))
            .or(m.avatar_color_index_effective.map(color_index)),
        // D92: the pair in effect (the relay applies the owner's picks and the rule).
        avatar_leaf: project
            .and_then(|p| p.avatar_leaf_effective)
            .or(m.avatar_leaf_effective)
            .and_then(leaf_index),
        avatar_leaf_color: project
            .and_then(|p| p.avatar_leaf_color_effective)
            .or(m.avatar_leaf_color_effective)
            .and_then(leaf_index),
        // The public versioned picture route, not the owner-only blob slot (a viewer who
        // only shares the group got 403 there and drew the leaf — Jacob, 2026-10-25).
        image_url: super::agent_avatar::picture_url(
            http_url,
            pid,
            project.and_then(|p| p.image_key.as_deref()),
            project.and_then(|p| p.avatar_version).unwrap_or(0),
        ),
        avatar_version: project.and_then(|p| p.avatar_version).unwrap_or(0),
    }
}

/// `GET /chats` item → a rail space (kind dm/group) and its one chat.
pub(crate) fn people_space_from(
    dto: &PeopleChatDto,
    me_id: &str,
    me_handle: &str,
    http_url: &str,
    token: &str,
) -> (Project, Chat) {
    let id = dto.id.clone();
    let mut people: Vec<Person> = Vec::new();
    let mut agents: Vec<AgentMember> = Vec::new();
    let mut i_own = false;
    for m in &dto.members {
        if let Some(uid) = m.member.strip_prefix("user:") {
            if uid == me_id {
                i_own = m.role == "owner";
                continue;
            }
            people.push(Person {
                id: uid.to_owned(),
                handle: m.handle.clone().unwrap_or_else(|| uid.to_owned()),
                name: m.name.clone().flatten(),
                avatar_url: m.avatar_url.clone().flatten().unwrap_or_default(),
                status: m.status.clone().unwrap_or_else(|| "member".into()),
                role: if m.role.is_empty() {
                    "owner".into()
                } else {
                    m.role.clone()
                },
                last_seen_at: m.last_seen_at.flatten(),
                online: m.online.unwrap_or(false),
                avatar_color_index: m.avatar_color_index_effective.map(color_index),
            });
        } else if let Some(pid) = m.member.strip_prefix("agent:") {
            agents.push(agent_from_member(m, pid, http_url));
        }
    }
    let people_owner: Option<String> = people
        .iter()
        .find(|p| p.role == "owner")
        .map(|p| p.handle.clone());
    let kind = match dto.kind.as_str() {
        "group" => SpaceKind::Group,
        _ => SpaceKind::Dm,
    };
    let title = if dto.title.is_empty() {
        people
            .first()
            .map(|p| p.handle.clone())
            .unwrap_or_else(|| "chat".into())
    } else {
        dto.title.clone()
    };
    // Pending = every other person still has to sign in; a group of agents
    // only has nobody to wait for.
    let pending =
        dto.pending || (!people.is_empty() && people.iter().all(|p| p.status == "invited"));
    let icon = people
        .first()
        .map(|p| p.initial())
        .unwrap_or_else(|| "@".into());
    let space = Project {
        id: id.clone(),
        name: title.clone(),
        icon,
        color: "#8b7cf6".into(),
        state: BoxState::Awake,
        state_detail: None,
        wake_progress: None,
        model: String::new(),
        effort: Effort::Low,
        spend_today_usd: 0.0,
        spend_cap_usd: 0.0,
        kind,
        people,
        agents,
        pending,
        shared: false,
        owner_handle: if i_own {
            Some(me_handle.to_owned())
        } else {
            people_owner
        },
        avatar: None,
        avatar_color: None,
        description: dto
            .description
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_owned),
        owner_person: None,
        machine_info: None,
        outside: None,
        muted: dto.me.muted.unwrap_or(false),
        silenced: dto.me.silenced.unwrap_or(false),
        silenced_until: dto.me.silenced_until.filter(|&t| t > 0),
        archived_at: dto.archived_at.filter(|t| *t > 0).map(|t| t as u64),
        primary_chat: None,
        voice: None,
        vision_enabled: true,
        avatar_color_index: None,
        avatar_color_override: None,
        // D92 amendment: a group's leaf picture from the Create screen (the uploaded picture wins).
        avatar_leaf: dto.avatar_leaf.and_then(leaf_index),
        avatar_leaf_color: dto.avatar_leaf_color.and_then(leaf_index),
        image_url: dto
            .image_key
            .as_deref()
            .and_then(non_empty)
            .map(|k| format!("{http_url}/blob/people:{id}/{k}?token={token}")),
        avatar_version: 0,
        message_grants: Vec::new(),
        blocked_senders: dto.blocked_senders.clone(),
        my_role: dto.me.role.clone().flatten(),
    };
    let chat = Chat {
        id: id.clone(),
        project_id: id,
        title,
        is_pinned: true,
        is_system: false,
        kind: ChatKind::Chat,
        participants: Vec::new(),
        unread: dto.unread,
        unread_count: u32::from(dto.unread),
        last_message_at: dto.last_activity_at.max(0) as u64,
        archived_at: dto.archived_at.filter(|t| *t > 0).map(|t| t as u64),
    };
    (space, chat)
}

impl From<&ConnectorDto> for Connector {
    fn from(d: &ConnectorDto) -> Self {
        Self {
            id: d.id.clone(),
            provider: d.provider.clone(),
            label: d.label.clone(),
            scopes: d.scopes.clone(),
            status: serde_json::from_value(Value::String(d.status.clone()))
                .unwrap_or(ConnectorStatus::Other),
            last_error: d.last_error.clone(),
            mock: d.mock,
        }
    }
}

impl From<&ProviderDto> for ConnectorProvider {
    fn from(d: &ProviderDto) -> Self {
        Self {
            provider: d.provider.clone(),
            label: d.label.clone(),
            connect: d.connect.clone(),
            oauth_configured: d.oauth_configured,
            actions: d
                .actions
                .iter()
                .map(|a| ConnectorAction {
                    name: a.name.clone(),
                    kind: a.kind.clone(),
                })
                .collect(),
        }
    }
}

pub fn connectors_view_from_dto(
    catalog: &ConnectorsCatalogDto,
    list: &ConnectorsListDto,
) -> ConnectorsView {
    ConnectorsView {
        mock: list.mock,
        providers: catalog
            .providers
            .iter()
            .map(ConnectorProvider::from)
            .collect(),
        items: list.items.iter().map(Connector::from).collect(),
        loaded: true,
    }
}

impl From<&ChannelPlatformDto> for ChannelPlatform {
    fn from(d: &ChannelPlatformDto) -> Self {
        Self {
            platform: d.platform.clone(),
            label: d.label.clone(),
            state: serde_json::from_value(Value::String(d.state.clone()))
                .unwrap_or(ChannelPlatformState::Other),
            detail: d.detail.clone(),
            inbound: d.inbound.clone(),
        }
    }
}

impl From<&ChannelBindingDto> for ChannelBinding {
    fn from(d: &ChannelBindingDto) -> Self {
        Self {
            id: d.id.clone(),
            platform: d.platform.clone(),
            external_chat_id: d.external_chat_id.clone(),
            external_user_id: d.external_user_id.clone(),
            project_id: d.project_id.clone(),
            chat_id: d.chat_id.clone(),
            label: d.label.clone(),
            bound_at: u64::try_from(d.bound_at).unwrap_or(0),
        }
    }
}

impl From<&ChannelPairingDto> for ChannelPairing {
    fn from(d: &ChannelPairingDto) -> Self {
        Self {
            code: d.code.clone(),
            project_id: d.project_id.clone(),
            chat_id: d.chat_id.clone(),
            expires_at: u64::try_from(d.expires_at).unwrap_or(0),
        }
    }
}

impl From<&ChannelsOverviewDto> for ChannelsView {
    fn from(d: &ChannelsOverviewDto) -> Self {
        Self {
            mock: d.mock,
            platforms: d.platforms.iter().map(ChannelPlatform::from).collect(),
            bindings: d.bindings.iter().map(ChannelBinding::from).collect(),
            pairing: d.pairing.as_ref().map(ChannelPairing::from),
            loaded: true,
        }
    }
}

pub fn skill_from_dto(d: &SkillDto) -> Skill {
    Skill {
        id: d.id.clone(),
        slug: d.slug.clone(),
        name: d.name.clone(),
        description: d.description.clone(),
        tags: d.tags.clone(),
        when_to_use: d.when_to_use.clone(),
        body: d.body.clone(),
        origin_project_id: d.origin_project_id.clone(),
        shared: d.shared,
        version: d.version,
        created_at: d.created_at,
        updated_at: d.updated_at,
    }
}

impl From<&LocalDeviceDto> for LocalDevice {
    fn from(d: &LocalDeviceDto) -> Self {
        Self {
            device_id: d.device_id.clone(),
            name: d.name.clone(),
            hostname: d.hostname.clone(),
            os: d.os.clone(),
            home: d.home.clone(),
            connected: d.connected,
            last_seen: d.last_seen,
            allow_reads: d.allow_reads,
        }
    }
}

pub fn local_view_from_dto(d: &LocalDevicesDto, relay_url: &str) -> LocalView {
    LocalView {
        devices: d.items.iter().map(LocalDevice::from).collect(),
        relay_url: relay_url.to_owned(),
        loaded: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbos_core::api::{
        ChatMemberDto, GroupDto, MeResponseDto, ProjectDto, UnreadChatDto, UnreadDto,
    };
    use std::collections::{HashMap, HashSet};
    use std::fs;
    use std::path::PathBuf;

    fn fixture(name: &str) -> String {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../arbos-relay/tests/fixtures/api")
            .join(name);
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    }

    fn load_json<T: serde::de::DeserializeOwned>(name: &str) -> T {
        serde_json::from_str(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    #[test]
    fn person_from_user_card_fixtures() {
        let legacy: UserCardDto = load_json("user_card_legacy.json");
        let p = Person::from(&legacy);
        assert_eq!(p.id, "u_mac");
        assert_eq!(p.handle, "jacobs-mac");
        assert_eq!(p.name.as_deref(), Some("Jacob's Mac"));

        let presence: UserCardDto = load_json("user_card_presence.json");
        let p = Person::from(&presence);
        assert_eq!(p.handle, "user301");
        assert!(!p.online);
        assert_eq!(p.avatar_color_index, Some(2));

        let github: UserCardDto = load_json("user_card_github.json");
        let p = Person::from(&github);
        assert_eq!(p.status, "github");
        assert_eq!(p.handle, "ghostuser");
    }

    #[test]
    fn machine_and_account_from_fixtures() {
        let status: MachineStatusDto = load_json("machine_status.json");
        let mut m = Machine::default();
        apply_machine_status(&mut m, &status);
        assert_eq!(m.state, MachineState::Asleep);
        assert_eq!(m.cpu, Some(4));
        assert_eq!(m.mem_gib, Some(8));

        let me: MeResponseDto = load_json("me_with_machine.json");
        let view = account_view_from_me(&me);
        assert_eq!(view.id, "gh_301");
        assert_eq!(view.login, "user301");
        assert_eq!(view.device_name, "Test Mac");
        assert!((view.balance_usd - 20.0).abs() < f64::EPSILON);

        let unread: UnreadDto = load_json("unread.json");
        assert_eq!(unread.total, 2);
        assert_eq!(unread.chats[0].chat_id, "people_dm");
    }

    #[test]
    fn merged_unread_relay_and_local_read() {
        assert_eq!(merged_unread(2, None, 1_700_000_000_000), 2);
        assert_eq!(
            merged_unread(2, Some(1_700_000_000_100), 1_700_000_000_000),
            0
        );
        assert_eq!(merged_unread(0, None, 1_700_000_000_000), 0);
        assert_eq!(
            merged_unread(0, Some(1_700_000_000_100), 1_700_000_000_000),
            0
        );
        // Local read before the latest activity: relay still wins.
        assert_eq!(
            merged_unread(2, Some(1_699_999_999_000), 1_700_000_000_000),
            2
        );
    }

    /// #1438: `GET /me/unread` is the user's cursor. A chat it does not list is read even when
    /// the chat's own flag says otherwise (own message bumped the room head); the flag only
    /// counts when the endpoint did not answer.
    #[test]
    fn me_unread_is_authoritative_when_served() {
        let served: HashMap<Id, u32> = HashMap::from([("dm".to_string(), 2)]);
        assert_eq!(relay_unread_for(Some(&served), "dm", false, 0), 2);
        assert_eq!(relay_unread_for(Some(&served), "group", true, 1), 0);
        assert_eq!(relay_unread_for(None, "group", true, 0), 1);
        assert_eq!(relay_unread_for(None, "group", true, 4), 4);
        assert_eq!(relay_unread_for(None, "group", false, 0), 0);
    }

    fn sample_chat(id: &str, project_id: &str, title: &str, archived_at: Option<u64>) -> Chat {
        Chat {
            id: id.into(),
            project_id: project_id.into(),
            title: title.into(),
            is_pinned: false,
            is_system: false,
            kind: ChatKind::Chat,
            participants: Vec::new(),
            unread: false,
            unread_count: 0,
            last_message_at: 1_000,
            archived_at,
        }
    }

    #[test]
    fn merge_chats_keeps_pending_local_tab() {
        let relay = vec![sample_chat("r1", "p", "Hello", None)];
        let local = vec![
            sample_chat("r1", "p", "Hello", None),
            sample_chat("local_tab", "p", "New chat", None),
        ];
        let pending = HashSet::from(["local_tab".into()]);
        let merged = merge_chats(&relay, &local, 5_000, &HashMap::new(), &pending);
        assert!(merged.iter().any(|c| c.id == "local_tab"));
        assert!(merged.iter().any(|c| c.id == "r1"));
    }

    #[test]
    fn merge_chats_keeps_local_archive_after_refresh_start() {
        let relay = vec![sample_chat("c1", "p", "Hello", None)];
        let local = vec![sample_chat("c1", "p", "Hello", Some(9_000))];
        let changed = HashMap::from([("c1".into(), 8_000u64)]);
        let merged = merge_chats(&relay, &local, 5_000, &changed, &HashSet::new());
        assert_eq!(merged[0].archived_at, Some(9_000));
    }

    #[test]
    fn merge_chats_takes_relay_when_local_change_is_older() {
        let relay = vec![sample_chat("c1", "p", "Relay title", None)];
        let local = vec![sample_chat("c1", "p", "Old local", Some(9_000))];
        let changed = HashMap::from([("c1".into(), 3_000u64)]);
        let merged = merge_chats(&relay, &local, 5_000, &changed, &HashSet::new());
        assert_eq!(merged[0].title, "Relay title");
        assert!(merged[0].archived_at.is_none());
    }

    #[test]
    fn merge_chats_drops_relay_gone_without_pending() {
        let relay = vec![sample_chat("r1", "p", "Hello", None)];
        let local = vec![
            sample_chat("r1", "p", "Hello", None),
            sample_chat("gone", "p", "Bye", None),
        ];
        let merged = merge_chats(&relay, &local, 5_000, &HashMap::new(), &HashSet::new());
        assert!(merged.iter().all(|c| c.id != "gone"));
        assert_eq!(merged.len(), 1);
    }

    #[test]
    fn remap_queued_sends_keeps_message_ids() {
        let ids = vec!["msg_a".into(), "msg_b".into()];
        let mapped = remap_queued_send_targets("chat_real", &ids);
        assert_eq!(
            mapped,
            vec![
                ("chat_real".into(), "msg_a".into()),
                ("chat_real".into(), "msg_b".into()),
            ]
        );
    }

    #[test]
    fn unread_dto_maps_onto_agent_chats() {
        let mut unread: UnreadDto = load_json("unread.json");
        // Agent chats get their counts from the same GET /me/unread payload.
        unread.chats.push(UnreadChatDto {
            chat_id: "chat_fixture".into(),
            project_id: "proj_fixture".into(),
            unread: 3,
            mentions: 0,
        });
        let by_id: HashMap<&str, u32> = unread
            .chats
            .iter()
            .map(|c| (c.chat_id.as_str(), c.unread.max(0) as u32))
            .collect();
        assert_eq!(by_id.get("people_dm"), Some(&2));
        assert_eq!(by_id.get("chat_fixture"), Some(&3));
        assert_eq!(
            by_id.values().copied().sum::<u32>(),
            unread.total.max(0) as u32 + 3
        );

        let dto: ChatDto = load_json("chat_json_for.json");
        let mut chat = chat_from_dto(&dto);
        assert!(!chat.unread);
        assert_eq!(chat.unread_count, 0);
        let relay = by_id.get(chat.id.as_str()).copied().unwrap_or(0);
        let count = merged_unread(relay, None, chat.last_message_at as i64);
        chat.unread_count = count;
        chat.unread = count > 0;
        assert!(chat.unread);
        assert_eq!(chat.unread_count, 3);
        assert_eq!(chat.missed_count(), 3);
        let count = merged_unread(
            relay,
            Some(chat.last_message_at as i64 + 1),
            chat.last_message_at as i64,
        );
        chat.unread_count = count;
        chat.unread = count > 0;
        assert!(!chat.unread);
        assert_eq!(chat.unread_count, 0);
    }

    #[test]
    fn missed_count_sums_messages_not_chats() {
        let a = Chat {
            unread: true,
            unread_count: 5,
            ..sample_chat("a", "p", "A", None)
        };
        let b = Chat {
            unread: true,
            unread_count: 2,
            ..sample_chat("b", "p", "B", None)
        };
        let live = Chat {
            unread: true,
            unread_count: 0,
            ..sample_chat("c", "p", "C", None)
        };
        assert_eq!(
            [a, b, live].iter().map(Chat::missed_count).sum::<usize>(),
            8
        );
    }

    #[test]
    fn project_from_dto_fixture() {
        let project: ProjectDto = load_json("project_json.json");
        let members: ProjectMembersDto = load_json("project_members_json.json");
        let detail = GetProjectResponseDto {
            box_state: project.box_state,
            box_since: project.box_since,
            spend_today: serde_json::json!(project.spend_today_usd),
            skills_rev: String::new(),
            chats: Vec::new(),
            project,
        };
        let p = project_from_dto(&detail, Some(&members));
        assert_eq!(p.id, "proj_fixture");
        assert_eq!(p.name, "Fixture");
        assert_eq!(
            p.owner_person.as_ref().map(|o| o.handle.as_str()),
            Some("user101")
        );
        assert!((p.spend_cap_usd - 25.0).abs() < f64::EPSILON);
        assert_eq!(p.primary_chat.as_deref(), Some("chat_primary"));
        assert_eq!(p.people.len(), 1);
    }

    #[test]
    fn chat_from_dto_fixture() {
        let dto: ChatDto = load_json("chat_json_for.json");
        let chat = chat_from_dto(&dto);
        assert_eq!(chat.id, "chat_fixture");
        assert_eq!(chat.project_id, "proj_fixture");
        assert_eq!(chat.title, "Hello");
        assert!(chat.archived_at.is_none());
    }

    #[test]
    fn people_space_from_fixtures() {
        let with_me: PeopleChatDto = load_json("people_chat_json_with_me.json");
        let (space, _) = people_space_from(&with_me, "gh_101", "user101", "http://relay", "tok");
        assert_eq!(space.kind, SpaceKind::Dm);
        assert_eq!(space.name, "user102");
        assert_eq!(space.people.len(), 1);
        assert!(space.agents.is_empty());
        assert!(!space.silenced);
        assert!(space.silenced_until.is_none());

        let without_me: PeopleChatDto = load_json("people_chat_json_without_me.json");
        let (space, _) = people_space_from(&without_me, "", "", "http://relay", "tok");
        assert_eq!(space.kind, SpaceKind::Dm);
        assert_eq!(space.name, "user101, user102");
        assert_eq!(space.people.len(), 2);

        let mut with_agent = with_me;
        let agent: ChatMemberDto = load_json("member_json_agent.json");
        with_agent.members.push(agent);
        let (space, _) = people_space_from(&with_agent, "gh_101", "user101", "http://relay", "tok");
        assert_eq!(space.agents.len(), 1);
        assert_eq!(space.agents[0].name, "Fixture");
    }

    #[test]
    fn people_space_from_copies_me_silenced_until() {
        let mut dto: PeopleChatDto = load_json("people_chat_json_with_me.json");
        dto.me.silenced = Some(true);
        dto.me.silenced_until = Some(1_793_200_000_000);
        let (space, _) = people_space_from(&dto, "gh_101", "user101", "http://relay", "tok");
        assert!(space.silenced);
        assert_eq!(space.silenced_until, Some(1_793_200_000_000));
        assert!(space.is_silenced_now(1_793_100_000_000));
        assert!(!space.is_silenced_now(1_793_200_000_001));
    }

    /// Scout, 2026-10-29: an archived DM / group came back into the list on the next 15-s
    /// registry poll because its row was rebuilt with no archive mark.
    #[test]
    fn people_space_from_keeps_the_archive_on_the_row() {
        let mut dto: PeopleChatDto = load_json("people_chat_json_with_me.json");
        dto.archived_at = Some(1_793_288_000_000);
        let (space, chat) = people_space_from(&dto, "gh_101", "user101", "http://relay", "tok");
        assert_eq!(space.archived_at, Some(1_793_288_000_000));
        assert_eq!(chat.archived_at, Some(1_793_288_000_000));

        dto.archived_at = None;
        let (space, _) = people_space_from(&dto, "gh_101", "user101", "http://relay", "tok");
        assert!(space.archived_at.is_none());
        dto.archived_at = Some(0);
        let (space, _) = people_space_from(&dto, "gh_101", "user101", "http://relay", "tok");
        assert!(space.archived_at.is_none(), "0 = not archived");
    }

    #[test]
    fn group_dto_participants_fixture() {
        // The Mac no longer creates agent groups (#1329); the DTO still decodes.
        let dto: GroupDto = load_json("group_json.json");
        assert_eq!(dto.participants, vec!["proj_fixture", "proj_other"]);
        assert_eq!(dto.id, "group_fixture");
    }

    #[test]
    fn call_fixtures_into_model() {
        use arbos_core::api::{CallDto, CallStartedPayloadDto, StartCallResponseDto};

        let live: CallDto = load_json("call_live_with_token.json");
        let call = Call::from(&live);
        assert_eq!(call.call_id, "call_fixture");
        assert_eq!(call.token.as_deref(), Some("fixture.join.token"));
        assert_eq!(call.started_by.handle, "user201");
        assert_eq!(call.participants.len(), 1);
        assert!(call.agents.is_empty());

        let ended: CallDto = load_json("call_ended.json");
        let call = Call::from(&ended);
        assert!(call.ended_at.is_some());
        assert_eq!(call.reason.as_deref(), Some("last_left"));
        assert!(call.token.is_none());

        let start: StartCallResponseDto = load_json("start_call_response.json");
        let reachable: Vec<Reachable> = start
            .reachable
            .unwrap_or_default()
            .iter()
            .map(Reachable::from)
            .collect();
        assert_eq!(reachable.len(), 1);
        assert_eq!(reachable[0].user_id, "gh_202");
        assert!(reachable[0].any_voip);

        let started: CallStartedPayloadDto = load_json("call_started_payload.json");
        let s = Sender::from(&started.sender);
        assert_eq!(s.id, "gh_201");
        assert_eq!(s.handle, "user201");
    }

    #[test]
    fn routine_fixtures_into_model() {
        let schedule: RoutineDto = load_json("routine_schedule.json");
        let rt = routine_from_dto(&schedule).expect("schedule routine");
        assert_eq!(rt.id, "routine_schedule_fixture");
        assert_eq!(rt.name, "Weekday check-in");
        assert!(matches!(
            rt.trigger,
            RoutineTrigger::Cron { ref expr, ref tz }
                if expr == "0 9 * * 1-5" && tz == "UTC"
        ));
        assert!(rt.fire_url.is_none());

        let webhook: RoutineDto = load_json("routine_webhook.json");
        let rt = routine_from_dto(&webhook).expect("webhook routine");
        assert_eq!(rt.id, "routine_webhook_fixture");
        assert!(matches!(
            rt.trigger,
            RoutineTrigger::Webhook { ref secret }
                if secret == "fixture-webhook-secret-not-real"
        ));
        assert!(rt
            .fire_url
            .as_deref()
            .is_some_and(|u| u.contains("fixture-webhook-secret-not-real")));

        let run: RoutineRunDto = load_json("routine_run.json");
        let run = RoutineRun::from(&run);
        assert_eq!(run.id, "run_fixture");
        assert_eq!(run.routine_id, "routine_schedule_fixture");
        assert_eq!(run.status, RoutineRunStatus::Done);
        assert_eq!(run.message_id.as_deref(), Some("msg_fixture"));
    }

    #[test]
    fn secrets_fixtures_into_model() {
        use arbos_core::api::{
            DecideSecretResponseDto, ProjectSecretsDto, SecretActivityDto, SecretDto,
            SecretRequestDto, SecretsOverviewDto, SecretsPausedDto, VaultDto,
        };

        let secret: SecretDto = load_json("secret_json.json");
        let s = Secret::from(&secret);
        assert_eq!(s.id, "sec_fixture");
        assert_eq!(s.env, "FIXTURE_STRIPE_KEY");
        assert_eq!(s.agents.len(), 1);
        assert_eq!(s.directors.len(), 1);
        assert_eq!(s.directors[0].handle, "user502");
        assert!(s.directors[0].enabled);
        assert_eq!(s.directors[0].last_asked_at, Some(1_700_000_003_000));
        assert_eq!(s.others_ask, "refuse");
        // Secret hand-off: the fixture's secret came from Settings; one given in chat says so.
        assert_eq!(s.source.as_ref().map(|x| x.kind.as_str()), Some("settings"));
        assert!(s.source_label().is_none() && s.added_label().is_none());
        let mut given = secret.clone();
        given.source = Some(arbos_core::api::SecretSourceDto {
            kind: "chat".into(),
            project_id: Some("proj_secrets_fixture".into()),
            project_name: Some("Research".into()),
            chat_id: Some("chat_1".into()),
        });
        let g = Secret::from(&given);
        assert_eq!(g.source_label(), Some("given in chat"));
        assert_eq!(g.added_label().as_deref(), Some("In chat with Research"));
        // A relay from before #1442 sends only `directors[]` and no switch: still on, still refuse.
        let mut old = secret.clone();
        old.people.clear();
        old.others_ask = None;
        old.source = None;
        old.directors[0].enabled = None;
        let s_old = Secret::from(&old);
        assert_eq!(s_old.directors.len(), 1);
        assert!(s_old.directors[0].enabled);
        assert_eq!(s_old.others_ask, "refuse");

        let login: SecretDto = load_json("secret_login_json.json");
        let s = Secret::from(&login);
        assert_eq!(s.kind, "login");
        assert_eq!(s.username.as_deref(), Some("fixture.login"));

        let request: SecretRequestDto = load_json("secret_request_json.json");
        let r = SecretRequest::from(&request);
        assert_eq!(r.id, "req_secret_fixture");
        assert!(r.is_pending());

        let reason: SecretActivityDto = load_json("secret_activity_reason.json");
        let a = SecretActivity::from(&reason);
        assert_eq!(a.action, "refused");
        assert_eq!(a.target_kind, "secret");
        assert_eq!(a.detail["reason"], "host_not_allowed");

        let vault: VaultDto = load_json("vault_json.json");
        let v = ConnectedVault::from(&vault);
        assert_eq!(v.id, "vault_fixture");
        assert_eq!(v.items_count, 1);
        assert_eq!(v.agents.len(), 1);
        assert_eq!(v.items.len(), 1);

        let overview: SecretsOverviewDto = load_json("secrets_overview.json");
        let view = secrets_view_from_dto(&overview, &overview.requests);
        assert_eq!(view.secrets.len(), 2);
        assert_eq!(view.vaults.len(), 1);
        assert_eq!(view.activity.len(), 2);
        assert!(!view.vault_connect);

        let project: ProjectSecretsDto = load_json("project_secrets_json.json");
        let p = ProjectSecrets::from(&project);
        assert!(p.owner);
        assert_eq!(p.items.len(), 2);
        assert_eq!(
            p.items[0].secret.as_ref().map(|s| s.env.as_str()),
            Some("FIXTURE_STRIPE_KEY")
        );
        assert_eq!(
            p.items[1].vault.as_ref().map(|v| v.name.as_str()),
            Some("Fixture 1Password")
        );

        let paused: SecretsPausedDto = load_json("secrets_paused.json");
        assert!(paused.paused);
        assert_eq!(paused.paused_by.as_deref(), Some("user"));

        let decide: DecideSecretResponseDto = load_json("decide_secret_response.json");
        assert_eq!(decide.request.status, "allowed_always");
        assert_eq!(
            decide.access.as_ref().map(|a| a.access_id.as_str()),
            Some("acc_secret_fixture")
        );
    }

    #[test]
    fn pair_token_health_voices_fixtures() {
        use arbos_core::api::{HealthDto, PairResponseDto};

        let pair: PairResponseDto = load_json("pair_response.json");
        assert_eq!(pair.device_id, "dev_fixture");
        assert_eq!(pair.device_token, "adv_fixture_not_a_real_token");

        let token: TokenResponseDto = load_json("token_response.json");
        assert!(!token.access_token.is_empty());
        assert_eq!(token.device_id, "dev_fixture");
        let creds = credentials_from_token(&token, "http://relay.test", None, 1_700_000_000_000);
        assert_eq!(creds.device_id, "dev_fixture");
        assert_eq!(creds.access_token, token.access_token);
        assert_eq!(
            creds.user.as_ref().map(|u| u.login.as_str()),
            Some("user301")
        );

        let grace: TokenResponseDto = load_json("token_response_refresh_grace.json");
        assert_eq!(grace.access_token, token.access_token);
        assert_eq!(grace.refresh_token, token.refresh_token);

        let health: HealthDto = load_json("healthz.json");
        assert_eq!(health.auth_mode.as_deref(), Some("github"));
        assert!(health.ok);

        let voices: VoiceCatalogDto = load_json("voices.json");
        let (default, list) = voices_from_dto(&voices);
        assert_eq!(default.as_deref(), Some("marin"));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "marin");
        assert_eq!(list[0].name, "Marin");

        let tx: TranscriptionDto = load_json("transcription_mock.json");
        assert!(transcript_text(&tx).contains("mock transcript"));
    }

    fn parse_push_item(v: Value) -> Option<arbos_core::api::PushItemDto> {
        serde_json::from_value(v).ok()
    }

    #[test]
    fn push_item_fixtures() {
        use arbos_core::api::{PushEventDto, PushItemDto, PushRegisterResponseDto, ReleasesDto};

        let names = [
            "push_people_message.json",
            "push_people_chat_updated.json",
            "push_people_member_added.json",
            "push_people_member_removed.json",
            "push_people_chat_created.json",
            "push_people_chat_deleted.json",
            "push_people_agent_added.json",
            "push_people_agent_removed.json",
            "push_people_project_shared.json",
            "push_project_member_removed.json",
            "push_chat_archived.json",
            "push_people_invite_accepted.json",
            "push_feedback_capture.json",
            "push_server_member_removed.json",
            "push_server_member_joined.json",
            "push_server_updated.json",
            "push_server_deleted.json",
            "push_call_started.json",
            "push_call_ended.json",
            "push_call_missed.json",
            "push_machine_ready.json",
            "push_release_published.json",
            "push_secret_request.json",
            "push_secret_request_secret.json",
            "push_secrets_paused.json",
            "push_secret_expiring.json",
            "push_profile_updated.json",
            "push_pins_changed.json",
        ];
        for name in names {
            let item: PushItemDto = load_json(name);
            assert!(!item.id.is_empty(), "{name}");
            assert!(item.created_at > 0, "{name}");
        }

        let people: PushItemDto = load_json("push_people_message.json");
        match people.event {
            PushEventDto::PeopleMessage(p) => {
                assert_eq!(p.chat_id, "people_dm");
                assert_eq!(p.sender.handle, "user201");
            }
            other => panic!("expected people_message, got {other:?}"),
        }

        let started: PushItemDto = load_json("push_call_started.json");
        match started.event {
            PushEventDto::CallStarted(p) => assert_eq!(p.call.call_id, "call_fixture"),
            other => panic!("expected call_started, got {other:?}"),
        }

        let capture: PushItemDto = load_json("push_feedback_capture.json");
        match capture.event {
            PushEventDto::FeedbackCapture(p) => {
                assert_eq!(p.request_id, "cap_fixture");
                assert_eq!(p.expires_at, 1_700_003_600_000);
            }
            other => panic!("expected feedback_capture, got {other:?}"),
        }

        let ready: PushItemDto = load_json("push_machine_ready.json");
        match ready.event {
            PushEventDto::MachineReady(p) => {
                assert_eq!(p.message, "Your Arbos computer is ready.")
            }
            other => panic!("expected machine_ready, got {other:?}"),
        }

        let secret: PushItemDto = load_json("push_secret_request_secret.json");
        match secret.event {
            PushEventDto::SecretRequest(p) => {
                assert_eq!(p.request_id, "req_secret_fixture");
                assert_eq!(p.host.as_deref(), Some("api.stripe.com"));
                assert!(p.vault_id.is_none());
            }
            other => panic!("expected secret_request, got {other:?}"),
        }

        let unknown: Value = serde_json::from_str(&fixture("push_unknown_kind.json")).unwrap();
        assert!(parse_push_item(unknown).is_none());
        assert!(parse_push_item(serde_json::json!({
            "id": "bad",
            "created_at": 1,
            "kind": "not_a_real_push_kind",
            "payload": {"x": 1}
        }))
        .is_none());

        let releases: ReleasesDto = load_json("releases_current.json");
        assert_eq!(releases.releases.len(), 2);
        assert_eq!(
            releases
                .releases
                .iter()
                .find(|r| r.platform == "ios")
                .map(|r| r.build.as_str()),
            Some("456")
        );

        let reg: PushRegisterResponseDto = load_json("push_register_response.json");
        assert_eq!(reg.device_id, "dev_fixture");
        assert_eq!(reg.environment, "production");
        assert!(reg.push);
    }

    #[test]
    fn connectors_from_fixtures() {
        use arbos_core::api::ConnectConnectorReplyDto;

        let catalog: ConnectorsCatalogDto = load_json("connectors_catalog.json");
        let list: ConnectorsListDto = load_json("connectors_list.json");
        let view = connectors_view_from_dto(&catalog, &list);
        assert!(view.loaded);
        assert!(view.mock);
        let names: Vec<&str> = view.providers.iter().map(|p| p.provider.as_str()).collect();
        assert_eq!(names, ["google", "slack", "github", "harness_comms"]);
        let github = view.providers.iter().find(|p| p.provider == "github");
        let github = github.expect("github provider");
        assert!(github.has_oauth() && github.accepts_token());
        assert!(!github.actions.is_empty());
        let conn = view.connector_for("github").expect("github connector");
        assert_eq!(conn.id, "conn_fixture");
        assert_eq!(conn.status, ConnectorStatus::Connected);

        let provider: ProviderDto = load_json("provider_json.json");
        assert_eq!(ConnectorProvider::from(&provider).connect, "oauth_or_token");

        let connector = Connector::from(&load_json::<ConnectorDto>("connector_json.json"));
        assert_eq!(connector.scopes, ["read", "search"]);
        assert!(connector.last_error.is_none());

        let auth: ConnectConnectorReplyDto = load_json("connect_reply_auth.json");
        assert_eq!(auth.state.as_deref(), Some("oauth_state_fixture"));
        assert!(auth
            .auth_url
            .is_some_and(|u| u.contains("oauth_state_fixture")));
        assert!(auth.connector.is_none());

        let stored: ConnectConnectorReplyDto = load_json("connect_reply_connector.json");
        assert!(stored.auth_url.is_none());
        assert_eq!(
            stored.connector.map(|c| c.status).as_deref(),
            Some("connected")
        );
    }

    #[test]
    fn channels_from_fixtures() {
        let view = ChannelsView::from(&load_json::<ChannelsOverviewDto>("channels_overview.json"));
        assert!(view.loaded && view.mock);
        assert_eq!(view.platforms.len(), 4);
        assert!(view
            .platforms
            .iter()
            .all(|p| p.state != ChannelPlatformState::Other));
        assert_eq!(view.bindings[0].external_chat_id, "tg_chat_fixture");
        assert_eq!(
            view.pairing.as_ref().map(|p| p.code.as_str()),
            Some("PAIR42")
        );

        let binding =
            ChannelBinding::from(&load_json::<ChannelBindingDto>("channel_binding_json.json"));
        assert_eq!(binding.project_id, "proj_fixture");
        assert_eq!(binding.bound_at, 1_700_000_000_000);

        let pair = ChannelPairing::from(&load_json::<ChannelPairingDto>("channel_pair_json.json"));
        assert_eq!(pair.chat_id, "chat_primary");
        assert_eq!(pair.expires_at, 4_102_444_800_000);
    }

    #[test]
    fn skill_from_fixture() {
        let dto: SkillDto = load_json("skill_json.json");
        let skill = skill_from_dto(&dto);
        assert_eq!(skill.slug, "release-notes");
        assert_eq!(skill.version, 2);
        assert_eq!(skill.origin_project_id.as_deref(), Some("proj_fixture"));
        assert_eq!(skill.to_markdown(), dto.markdown);
    }

    #[test]
    fn local_devices_from_fixture() {
        let dto: LocalDevicesDto = load_json("local_devices.json");
        let view = local_view_from_dto(&dto, "http://relay.test");
        assert!(view.loaded);
        assert_eq!(view.relay_url, "http://relay.test");
        assert_eq!(view.devices[0].device_id, "ldev_fixture");
        assert_eq!(view.devices[0].os_label(), "macOS");
        assert_eq!(view.devices[0].last_seen, Some(1_700_000_000_000));
    }

    #[test]
    fn feedback_submit_reply_fixture() {
        use arbos_core::api::FeedbackSubmitReplyDto;

        let reply: FeedbackSubmitReplyDto = load_json("feedback_submit_reply.json");
        assert_eq!(reply.id, "fb_fixture");
        assert!(!reply.has_image);
        assert!(reply.duplicate.is_none() && reply.occurrences.is_none());
    }

    #[test]
    fn relay_error_from_api_error_fixtures() {
        use crate::sync::relay::RelayError;

        let converted = fixture("api_error_converted.json");
        let err = RelayError::from_body(reqwest::StatusCode::NOT_FOUND, &converted);
        assert_eq!(err.status, 404);
        assert_eq!(err.kind, "not_found");
        assert_eq!(err.message, "no such project");

        let body = r#"{"error":{"type":"token_expired","message":"access token expired"}}"#;
        let expired = RelayError::from_body(reqwest::StatusCode::UNAUTHORIZED, body);
        assert_eq!(expired.kind, "token_expired");
        assert_eq!(expired.message, "access token expired");
        assert!(expired.is_token_expired());

        let taken = RelayError::from_body(
            reqwest::StatusCode::CONFLICT,
            r#"{"error":{"type":"handle_taken","message":"that handle is taken"}}"#,
        );
        assert_eq!(taken.kind, "handle_taken");

        let plain =
            RelayError::from_body(reqwest::StatusCode::SERVICE_UNAVAILABLE, "relay restarting");
        assert!(plain.kind.is_empty());
        assert_eq!(plain.message, "relay restarting");
        assert!(!plain.is_token_expired());
    }
}
