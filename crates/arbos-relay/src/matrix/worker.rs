//! The outbox worker: takes queued [`Job`]s in order per chat and applies them to the homeserver.
//! Each job is idempotent — `SyncRoom` reconciles the room against what the relay's tables say
//! right now, `Message` is skipped when its event already exists — so a retry never doubles
//! anything. Failures back off and, after `db::MAX_ATTEMPTS`, park the job without blocking
//! the chat's later jobs.

use std::time::Duration;

use serde_json::{json, Value};

use arbos_core::ids::now_ms;

use super::client::{Client, MatrixError, MxResult};
use super::db::MAX_ATTEMPTS;
use super::{Job, Matrix};
use crate::people::PEOPLE_PROJECT;
use crate::state::AppState;

/// Creator and joined members of a room, from its full state.
///
/// Up to room version 11 `m.room.create` carries the creator in `content.creator`. Room v12
/// (MSC4291) removed that field and made the create event's **sender** the creator, so a lookup
/// of `content.creator` alone returns nothing for every room made since v12 became the default.
fn creator_and_joined(state: &[Value]) -> (Option<String>, Vec<String>) {
    let mut creator = None;
    let mut joined = Vec::new();
    for ev in state {
        match ev["type"].as_str() {
            Some("m.room.create") => {
                creator = ev["content"]["creator"]
                    .as_str()
                    .or_else(|| ev["sender"].as_str())
                    .map(str::to_owned);
            }
            Some("m.room.member") if ev["content"]["membership"] == "join" => {
                if let Some(who) = ev["state_key"].as_str() {
                    joined.push(who.to_owned());
                }
            }
            _ => {}
        }
    }
    (creator, joined)
}

/// One live member: its Matrix id, relay role and relay member string (`user:<id>` /
/// `agent:<project>`, original case).
#[derive(Debug, Clone)]
pub struct Member {
    pub mxid: String,
    pub role: String,
    pub member: String,
}

/// The chat as the relay sees it, in Matrix terms.
#[derive(Debug, Clone)]
pub struct RoomSpec {
    pub chat_id: String,
    pub project_id: Option<String>,
    /// `dm` | `group` | `chat` | `projects` (design §3.2).
    pub kind: String,
    pub name: Option<String>,
    pub is_direct: bool,
    /// The member who creates the room (infinite power in room v12).
    pub creator: String,
    /// Every live member, the creator included.
    pub members: Vec<Member>,
    pub created_at: i64,
}

impl RoomSpec {
    fn member_mxids(&self) -> impl Iterator<Item = &str> {
        self.members.iter().map(|m| m.mxid.as_str())
    }

    /// Owners who are not the creator get power level 100 (the creator has infinite power).
    fn promoted_owners(&self) -> serde_json::Map<String, Value> {
        self.members
            .iter()
            .filter(|m| m.role == "owner" && m.mxid != self.creator)
            .map(|m| (m.mxid.clone(), json!(100)))
            .collect()
    }
}

/// What the relay knows about a chat, or `None` when it is gone.
pub fn room_spec(state: &AppState, chat_id: &str, project_id: Option<&str>) -> Option<RoomSpec> {
    let cfg = &state.matrix.cfg;
    match project_id.filter(|p| *p != PEOPLE_PROJECT) {
        None => {
            let chat = state.db.people_chat(chat_id).ok().flatten()?;
            let members = state.db.members_of(chat_id).ok()?;
            let mut mx: Vec<Member> = members
                .iter()
                .filter_map(|m| {
                    cfg.member_mxid(&m.member).map(|mxid| Member {
                        mxid,
                        role: m.role.clone(),
                        member: m.member.clone(),
                    })
                })
                .collect();
            let humans = members.iter().filter(|m| m.user_id().is_some()).count();
            let creator = cfg.user_mxid(&chat.created_by);
            let creator = if mx.iter().any(|m| m.mxid == creator) {
                creator
            } else {
                mx.iter()
                    .find(|m| m.role == "owner")
                    .or_else(|| mx.iter().find(|m| !m.mxid.starts_with("@_")))
                    .map(|m| m.mxid.clone())?
            };
            mx.sort_by(|a, b| a.mxid.cmp(&b.mxid));
            let is_direct = chat.kind == "dm" && humans == 2;
            Some(RoomSpec {
                chat_id: chat_id.to_owned(),
                project_id: None,
                kind: chat.kind.clone(),
                name: if is_direct {
                    None
                } else {
                    chat.title
                        .clone()
                        .or_else(|| Some(group_name(state, &members)))
                },
                is_direct,
                creator,
                members: mx,
                created_at: chat.created_at,
            })
        }
        Some(pid) => {
            let project = state.db.project(pid).ok().flatten()?;
            let chat = state.db.chat(&project.id, chat_id).ok().flatten()?;
            let owner = cfg.user_mxid(&project.user_id);
            let user_member = |uid: &str, role: &str| Member {
                mxid: cfg.user_mxid(uid),
                role: role.to_owned(),
                member: format!("user:{uid}"),
            };
            let mut members = vec![user_member(&project.user_id, "owner")];
            for (uid, _, _) in state.db.project_members(&project.id).unwrap_or_default() {
                members.push(user_member(&uid, "member"));
            }
            let mut agents: Vec<String> = vec![project.id.clone()];
            if chat.kind == "group" {
                agents.extend(chat.participants.iter().cloned());
            }
            agents.sort();
            agents.dedup();
            for a in agents {
                members.push(Member {
                    mxid: cfg.agent_mxid(&a),
                    role: "agent".to_owned(),
                    member: format!("agent:{a}"),
                });
            }
            let name = match chat.kind.as_str() {
                "group" => Some(chat.title.clone()),
                "projects" => Some("Projects".to_owned()),
                _ => None,
            };
            let humans = members.iter().filter(|m| m.role != "agent").count();
            Some(RoomSpec {
                chat_id: chat_id.to_owned(),
                project_id: Some(project.id.clone()),
                kind: chat.kind.clone(),
                is_direct: chat.kind == "chat" && humans == 1,
                name,
                creator: owner,
                members,
                created_at: chat.created_at,
            })
        }
    }
}

/// Fallback group title, as `people::chat_json` builds it: member handles joined.
fn group_name(state: &AppState, members: &[crate::db_people::Membership]) -> String {
    let mut names: Vec<String> = members
        .iter()
        .filter_map(|m| m.user_id())
        .filter_map(|u| state.db.user_card(u).ok().flatten())
        .filter(|c| !c.handle.is_empty())
        .map(|c| format!("@{}", c.handle))
        .collect();
    names.sort();
    names.join(", ")
}

/// Display name for a relay member (`user:<id>` → GitHub login; `agent:<pid>` → project name).
fn display_name(state: &AppState, member: &str) -> Option<String> {
    if let Some(u) = member.strip_prefix("user:") {
        let card = state.db.user_card(u).ok().flatten()?;
        return (!card.handle.is_empty())
            .then_some(card.handle)
            .or(card.name);
    }
    let p = member.strip_prefix("agent:")?;
    state.db.project(p).ok().flatten().map(|p| p.name)
}

pub struct Worker {
    pub(super) state: AppState,
    pub(super) client: Client,
}

impl Worker {
    pub fn new(state: AppState) -> Self {
        let client = Client::new(state.http.clone(), state.matrix.cfg.clone());
        Self { state, client }
    }

    /// Runs until the relay stops. Wakes on `Matrix::wake` or every 5 s.
    pub async fn run(self) {
        let matrix: &Matrix = &self.state.matrix;
        let mut last_prune = now_ms();
        loop {
            if matrix.cfg.disabled() {
                tokio::time::sleep(Duration::from_secs(10)).await;
                continue;
            }
            let due = match self.state.db.matrix_outbox_due(32) {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!(error = %e, "matrix outbox: read failed");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            };
            if due.is_empty() {
                tokio::select! {
                    _ = matrix.wake.notified() => {}
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                }
                if now_ms() - last_prune > 3_600_000 {
                    let _ = self.state.db.matrix_outbox_prune(7 * 24 * 3_600_000);
                    last_prune = now_ms();
                }
                continue;
            }
            let mut homeserver_down = false;
            for row in due {
                match self.handle(&row.job).await {
                    Ok(()) => {
                        let _ = self.state.db.matrix_outbox_done(row.id);
                    }
                    Err(e) => {
                        // A request Synapse rejects for good (bad state, forbidden) is parked
                        // after a few tries; a homeserver outage retries for hours.
                        let attempts = if e.transient() {
                            row.attempts
                        } else {
                            row.attempts.max(MAX_ATTEMPTS - 3)
                        };
                        let parked = self
                            .state
                            .db
                            .matrix_outbox_failed(row.id, attempts, &e.to_string())
                            .unwrap_or(false);
                        if parked {
                            tracing::error!(id = row.id, job = ?row.job, error = %e, "matrix outbox: job parked");
                        } else {
                            tracing::warn!(id = row.id, attempt = row.attempts + 1, error = %e, "matrix outbox: job failed, will retry");
                        }
                        if matches!(e, MatrixError::Transport(_)) {
                            homeserver_down = true;
                            break;
                        }
                    }
                }
            }
            if homeserver_down {
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }

    pub async fn handle(&self, job: &Job) -> MxResult<()> {
        match job {
            Job::SyncRoom {
                chat_id,
                project_id,
            } => self
                .sync_room(chat_id, project_id.as_deref())
                .await
                .map(|_| ()),
            Job::Message {
                chat_id,
                project_id,
                message_id,
                sender,
                text,
                parts,
                ts,
            } => {
                self.message(
                    chat_id,
                    project_id.as_deref(),
                    message_id,
                    sender,
                    text,
                    parts.as_ref(),
                    *ts,
                )
                .await
            }
            Job::Read { chat_id, user_id } => self.read(chat_id, user_id).await,
            Job::Archive {
                chat_id,
                user_id,
                archived,
            } => self.archive(chat_id, user_id, *archived).await,
            Job::CloseRoom { chat_id } => self.close_room(chat_id).await,
            Job::Pushers { user_id, on } => {
                super::reads::set_pushers(&self.state, &self.client, user_id, *on).await
            }
            Job::SyncServer { server_id } => self.sync_server(server_id).await,
            Job::SyncChannel { channel_id } => self.sync_channel(channel_id).await,
            Job::ChannelNotice {
                channel_id,
                text,
                notice_id,
            } => self.channel_notice(channel_id, text, notice_id).await,
            Job::Resync {
                chat_id,
                project_id,
            } => {
                let r = super::backfill::resync_chat(&self.state, chat_id, project_id.as_deref());
                // Per-row warnings for the rows nobody announced are written by `resync_chat`;
                // this is the pass summary. Announced rows with a job still queued are not
                // "unannounced" (#2120) and do not make a line.
                if r.queued > 0 {
                    tracing::info!(
                        chat = %chat_id,
                        queued = r.queued,
                        announced = r.announced,
                        unannounced = r.unannounced,
                        solo_project = r.unannounced_expected,
                        pending_skipped = r.pending_skipped,
                        "matrix resync: mirrored rows the worker had not applied"
                    );
                }
                Ok(())
            }
        }
    }

    // ---- users ----------------------------------------------------------------------------------

    /// Register the user on first sight and keep its display name current.
    pub(super) async fn ensure_user(&self, m: &Member) -> MxResult<()> {
        let wanted = display_name(&self.state, &m.member);
        if let Some((_, have)) = self.state.db.matrix_user(&m.member).ok().flatten() {
            if have == wanted {
                return Ok(());
            }
        } else {
            self.client.register(&m.mxid).await?;
        }
        if let Some(name) = &wanted {
            self.client.set_displayname(&m.mxid, name).await?;
        }
        let _ = self
            .state
            .db
            .matrix_upsert_user(&m.member, &m.mxid, wanted.as_deref());
        Ok(())
    }

    // ---- rooms ----------------------------------------------------------------------------------

    /// Make the room match the relay. Returns the room id, or `None` when the chat is gone.
    async fn sync_room(&self, chat_id: &str, project_id: Option<&str>) -> MxResult<Option<String>> {
        let Some(spec) = room_spec(&self.state, chat_id, project_id) else {
            if self
                .state
                .db
                .matrix_room(chat_id)
                .ok()
                .flatten()
                .is_some_and(|r| r.closed_at.is_none())
            {
                self.close_room(chat_id).await?;
            }
            return Ok(None);
        };
        for m in &spec.members {
            self.ensure_user(m).await?;
        }
        let existing = self.state.db.matrix_room(chat_id).ok().flatten();
        let room_id = match existing {
            Some(r) => {
                self.reconcile_members(&r.room_id, &spec).await?;
                self.reconcile_name(&r.room_id, &spec).await?;
                r.room_id
            }
            None => self.create_room(&spec).await?,
        };
        if spec.is_direct {
            let humans: Vec<&str> = spec
                .member_mxids()
                .filter(|m| !m.starts_with("@_"))
                .collect();
            if humans.len() == 2 {
                let _ = self.client.set_direct(humans[0], humans[1], &room_id).await;
                let _ = self.client.set_direct(humans[1], humans[0], &room_id).await;
            }
        }
        Ok(Some(room_id))
    }

    fn chat_state(spec: &RoomSpec) -> Value {
        json!({
            "chat_id": spec.chat_id,
            "kind": spec.kind,
            "project_id": spec.project_id,
            "created_by": spec.creator,
            "created_at": spec.created_at,
            "epoch": 1,
        })
    }

    async fn create_room(&self, spec: &RoomSpec) -> MxResult<String> {
        let invite: Vec<&str> = spec.member_mxids().filter(|m| *m != spec.creator).collect();
        let users_pl = spec.promoted_owners();
        let mut body = json!({
            "preset": "private_chat",
            "visibility": "private",
            "is_direct": spec.is_direct,
            "invite": invite,
            "creation_content": { "life.arbos.chat_id": spec.chat_id },
            "initial_state": [
                { "type": "life.arbos.chat", "state_key": "", "content": Self::chat_state(spec) },
                { "type": "m.room.history_visibility", "state_key": "", "content": { "history_visibility": "shared" } },
            ],
            "power_level_content_override": { "users": users_pl, "invite": 0 },
        });
        if let Some(n) = &spec.name {
            body["name"] = json!(n);
        }
        let room_id = self.client.create_room(&spec.creator, &body).await?;
        self.state
            .db
            .matrix_insert_room(
                &spec.chat_id,
                &room_id,
                spec.project_id.as_deref(),
                &spec.kind,
            )
            .map_err(|e| MatrixError::Shape(format!("db: {e}")))?;
        for m in invite {
            self.client.join(&room_id, m).await?;
        }
        Ok(room_id)
    }

    async fn reconcile_members(&self, room_id: &str, spec: &RoomSpec) -> MxResult<()> {
        self.reconcile_room_members(
            room_id,
            &spec.creator,
            &spec.members,
            spec.promoted_owners(),
        )
        .await
    }

    /// Invite + join missing members, kick relay-minted members who left, and keep the
    /// `users` power-level map equal to `users_pl` (everyone else stays at the default).
    pub(super) async fn reconcile_room_members(
        &self,
        room_id: &str,
        creator: &str,
        members: &[Member],
        users_pl: serde_json::Map<String, Value>,
    ) -> MxResult<()> {
        let joined = self.client.joined_members(room_id, creator).await?;
        for m in members {
            if !joined.contains(&m.mxid) {
                self.client.invite(room_id, creator, &m.mxid).await?;
                self.client.join(room_id, &m.mxid).await?;
            }
        }
        let cfg = &self.state.matrix.cfg;
        for j in &joined {
            // Only relay-minted members are managed; appservice senders and future outside
            // agents are left alone.
            if cfg.member_of_mxid(j).is_some() && !members.iter().any(|m| m.mxid == *j) {
                self.client
                    .kick(room_id, creator, j, "left the chat")
                    .await?;
            }
        }
        if let Some(mut pl) = self
            .client
            .state(room_id, creator, "m.room.power_levels", "")
            .await?
        {
            let current = pl["users"].as_object().cloned().unwrap_or_default();
            if current != users_pl {
                pl["users"] = Value::Object(users_pl);
                self.client
                    .send_state(room_id, creator, "m.room.power_levels", "", &pl)
                    .await?;
            }
        }
        Ok(())
    }

    async fn reconcile_name(&self, room_id: &str, spec: &RoomSpec) -> MxResult<()> {
        self.reconcile_room_name(room_id, &spec.creator, spec.name.as_deref())
            .await?;
        let chat_state = self
            .client
            .state(room_id, &spec.creator, "life.arbos.chat", "")
            .await?;
        if chat_state.as_ref().and_then(|v| v["kind"].as_str()) != Some(spec.kind.as_str()) {
            self.client
                .send_state(
                    room_id,
                    &spec.creator,
                    "life.arbos.chat",
                    "",
                    &Self::chat_state(spec),
                )
                .await?;
        }
        Ok(())
    }

    /// `m.room.name` equal to `name` (unset when `None`).
    pub(super) async fn reconcile_room_name(
        &self,
        room_id: &str,
        creator: &str,
        name: Option<&str>,
    ) -> MxResult<()> {
        let have = self
            .client
            .state(room_id, creator, "m.room.name", "")
            .await?
            .and_then(|v| v["name"].as_str().map(str::to_owned));
        if name != have.as_deref() {
            let content = json!({ "name": name.unwrap_or_default() });
            self.client
                .send_state(room_id, creator, "m.room.name", "", &content)
                .await?;
        }
        Ok(())
    }

    /// The chat is gone: every relay-minted member leaves the room and the row is marked closed.
    ///
    /// The row is marked closed only once the homeserver has agreed. A close that cannot act —
    /// the state read failed, a leave was refused — returns an error so the job retries and is
    /// finally parked with a log line, because a close marked done while its members are still
    /// joined leaves a room nobody can see that still receives everything (#2433).
    async fn close_room(&self, chat_id: &str) -> MxResult<()> {
        let Some(room) = self.state.db.matrix_room(chat_id).ok().flatten() else {
            return Ok(());
        };
        if room.closed_at.is_some() {
            return Ok(());
        }
        let cfg = &self.state.matrix.cfg;
        let known: Vec<String> = self
            .known_members(&room)
            .into_iter()
            .filter(|m| cfg.member_of_mxid(m).is_some())
            .collect();
        if known.is_empty() {
            return Err(MatrixError::Shape(format!(
                "no member of {} is known to the relay, so its members cannot be removed",
                room.room_id
            )));
        }
        // Only a user that is, or was, in the room may read it, so the first candidate that
        // answers is the reader. A candidate that has left sees the room as it was when it
        // left, which is why the relay's own list is used alongside what the room says.
        let mut room_state = None;
        let mut refused = None;
        for who in &known {
            match self.client.room_state(&room.room_id, who).await {
                Ok(s) => {
                    room_state = Some(s);
                    break;
                }
                // The homeserver has no such room: there is nobody left to remove.
                Err(e) if e.errcode() == "M_NOT_FOUND" => return self.mark_closed(chat_id),
                Err(e) => refused = Some(e),
            }
        }
        let Some(room_state) = room_state else {
            return Err(refused.unwrap_or_else(|| {
                MatrixError::Shape(format!("{} could not be read by anyone", room.room_id))
            }));
        };
        let (creator, joined) = creator_and_joined(&room_state);
        let mut members: Vec<String> = joined
            .into_iter()
            .filter(|m| cfg.member_of_mxid(m).is_some())
            .collect();
        members.sort();
        for k in known {
            if !members.contains(&k) {
                members.push(k);
            }
        }
        // The creator can always send state (infinite power in v12, PL 100 before it); any other
        // member is better than nobody when the creator has already left.
        let actor = creator
            .filter(|c| members.contains(c))
            .or_else(|| members.first().cloned())
            .expect("members is not empty");
        let mut content = json!({ "chat_id": chat_id, "kind": room.kind, "closed_at": now_ms() });
        content["project_id"] = json!(room.project_id);
        if let Err(e) = self
            .client
            .send_state(&room.room_id, &actor, "life.arbos.chat", "", &content)
            .await
        {
            // The marker is for clients that still have the room open; failing to write it does
            // not stop the removal, which is what makes the close real.
            tracing::warn!(chat = %chat_id, room = %room.room_id, error = %e, "matrix close: closed marker not written");
        }
        for m in members.iter().filter(|m| **m != actor) {
            self.client.leave(&room.room_id, m).await?;
        }
        self.client.leave(&room.room_id, &actor).await?;
        self.mark_closed(chat_id)
    }

    /// Matrix ids that are, or were, in a room, best first.
    ///
    /// The homeserver will not say who is in a room except through a user that is or was in it:
    /// an appservice may only act as a user it has registered, and a room read from anyone else
    /// is `M_FORBIDDEN`. So a close has to bring its own list. Every source here outlives the
    /// delete that triggers the close — chats, channels and servers are soft-deleted and their
    /// membership rows stay — which the create event's `creator` field did not.
    fn known_members(&self, room: &super::db::MatrixRoom) -> Vec<String> {
        fn push(out: &mut Vec<String>, mxid: String) {
            if !out.contains(&mxid) {
                out.push(mxid);
            }
        }
        let cfg = &self.state.matrix.cfg;
        let db = &self.state.db;
        let mut out = Vec::new();
        // A project chat is only soft-closed and a sync-driven close races the delete, so the
        // chat's own spec is often still there, and it is the best list there is.
        if let Some(spec) = room_spec(&self.state, &room.chat_id, room.project_id.as_deref()) {
            push(&mut out, spec.creator);
            for m in spec.members {
                push(&mut out, m.mxid);
            }
        }
        for m in db.members_of(&room.chat_id).unwrap_or_default() {
            if let Some(mxid) = cfg.member_mxid(&m.member) {
                push(&mut out, mxid);
            }
        }
        // A space room *is* the server's room (`chat_id` is the server id); a channel room
        // reaches its server through the channel row.
        let server_id = match room.kind.as_str() {
            "space" => Some(room.chat_id.clone()),
            "channel" => db.channel_server_id(&room.chat_id).ok().flatten(),
            _ => None,
        };
        if let Some(server_id) = server_id {
            for m in db.server_members(&server_id).unwrap_or_default() {
                push(&mut out, cfg.user_mxid(&m.user_id));
            }
        }
        if let Some(project_id) = &room.project_id {
            if let Ok(Some(p)) = db.project(project_id) {
                push(&mut out, cfg.user_mxid(&p.user_id));
            }
            for (user_id, _, _) in db.project_members(project_id).unwrap_or_default() {
                push(&mut out, cfg.user_mxid(&user_id));
            }
            push(&mut out, cfg.agent_mxid(project_id));
        }
        // Last resort: whoever the relay has already written into the room.
        for sender in db.matrix_event_senders(&room.chat_id).unwrap_or_default() {
            if let Some(mxid) = cfg.member_mxid(&sender) {
                push(&mut out, mxid);
            }
        }
        out
    }

    fn mark_closed(&self, chat_id: &str) -> MxResult<()> {
        self.state
            .db
            .matrix_close_room(chat_id)
            .map_err(|e| MatrixError::Shape(format!("db: {e}")))
    }

    // ---- messages -------------------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    async fn message(
        &self,
        chat_id: &str,
        project_id: Option<&str>,
        message_id: &str,
        sender: &str,
        text: &str,
        parts: Option<&Value>,
        ts: Option<i64>,
    ) -> MxResult<()> {
        if self
            .state
            .db
            .matrix_event(chat_id, message_id)
            .ok()
            .flatten()
            .is_some()
        {
            return Ok(());
        }
        let room_id = match self.state.db.matrix_room(chat_id).ok().flatten() {
            Some(r) if r.closed_at.is_none() => r.room_id,
            _ => match self.sync_room(chat_id, project_id).await? {
                Some(r) => r,
                None => return Ok(()), // the chat is gone; nothing to mirror
            },
        };
        let cfg = &self.state.matrix.cfg;
        let spec = room_spec(&self.state, chat_id, project_id);
        let creator = spec.as_ref().map(|s| s.creator.clone());
        let (mxid, msgtype) = match cfg.member_mxid(sender) {
            Some(m) => (m, "m.text"),
            None => match &creator {
                Some(c) => (c.clone(), "m.notice"),
                None => return Ok(()),
            },
        };
        let mut content = json!({
            "msgtype": msgtype,
            "body": text,
            "life.arbos.id": message_id,
            "life.arbos.sender": sender,
        });
        if let Some(p) = parts {
            content["life.arbos.parts"] = p.clone();
        }
        let txn = format!("{message_id}-{}", short_hash(chat_id));
        let sent = match self
            .client
            .send(&room_id, &mxid, "m.room.message", &txn, &content, ts)
            .await
        {
            Ok(ev) => Ok(ev),
            // The author is no longer a member (left, removed, or a backfilled past member):
            // the creator carries the message with the real sender in content.
            Err(MatrixError::Api { errcode, .. })
                if errcode == "M_FORBIDDEN" && creator.as_deref() != Some(mxid.as_str()) =>
            {
                let Some(c) = &creator else {
                    return Ok(());
                };
                content["life.arbos.on_behalf"] = json!(true);
                self.client
                    .send(&room_id, c, "m.room.message", &txn, &content, ts)
                    .await
            }
            Err(e) => Err(e),
        }?;
        self.state
            .db
            .matrix_insert_event(
                chat_id,
                message_id,
                &sent,
                sender,
                ts.unwrap_or_else(now_ms),
            )
            .map_err(|e| MatrixError::Shape(format!("db: {e}")))?;
        Ok(())
    }

    /// The receipt goes where the relay's read mark is: on the mirrored event of the message
    /// at the mark while something is still unread, on the stream-newest event once caught up
    /// (a live read is always "everything up to now").
    async fn read(&self, chat_id: &str, user_id: &str) -> MxResult<()> {
        let Some(room) = self.state.db.matrix_room(chat_id).ok().flatten() else {
            return Ok(());
        };
        let newest = self.state.db.matrix_latest_event(chat_id).ok().flatten();
        let event = match self
            .state
            .db
            .matrix_read_mark(user_id, chat_id)
            .ok()
            .flatten()
        {
            Some((at_mark, false)) => at_mark,
            Some((_, true)) | None => newest,
        };
        let Some(event) = event else {
            return Ok(());
        };
        let who = self.state.matrix.cfg.user_mxid(user_id);
        match self
            .client
            .read_receipt(&room.room_id, &who, &event, false)
            .await
        {
            Ok(()) => Ok(()),
            // Not a member any more: nothing to mark.
            Err(MatrixError::Api { errcode, .. }) if errcode == "M_FORBIDDEN" => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn archive(&self, chat_id: &str, user_id: &str, archived: bool) -> MxResult<()> {
        let Some(room) = self.state.db.matrix_room(chat_id).ok().flatten() else {
            return Ok(());
        };
        let who = self.state.matrix.cfg.user_mxid(user_id);
        match self
            .client
            .set_tag(&room.room_id, &who, "m.lowpriority", archived)
            .await
        {
            Ok(()) => Ok(()),
            Err(MatrixError::Api { errcode, .. }) if errcode == "M_FORBIDDEN" => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// Eight hex chars of the chat id's hash: keeps transaction ids unique per (sender, room) even
/// when the same message id were mirrored into two rooms.
fn short_hash(s: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(&Sha256::digest(s.as_bytes())[..4])
}

/// Start the worker when `ARBOS_MATRIX` is not `off`.
pub fn spawn(state: AppState) {
    if !state.matrix.cfg.mode.writes() {
        return;
    }
    tracing::info!(
        mode = state.matrix.cfg.mode.as_str(),
        homeserver = %state.matrix.cfg.hs_url,
        "Matrix dual-write on"
    );
    tokio::spawn(Worker::new(state).run());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matrix::Config;
    use crate::people::tests_support::{dm, person, project_of};
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    #[test]
    fn room_spec_for_a_dm_and_a_project_chat() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1001);
        let (bob, _) = person(&state, 1002);
        let chat = dm(&state, &alice, &bob);
        let spec = room_spec(&state, &chat, None).expect("spec");
        assert_eq!(spec.kind, "dm");
        assert!(spec.is_direct);
        assert_eq!(spec.name, None);
        assert_eq!(spec.members.len(), 2);
        assert_eq!(spec.creator, state.matrix.cfg.user_mxid(&alice));
        assert_eq!(
            display_name(&state, &format!("user:{alice}")).as_deref(),
            Some("user1001")
        );

        let project = project_of(&state, &alice, "Research");
        let pchat = state.db.list_chats(&project).expect("chats")[0].id.clone();
        let spec = room_spec(&state, &pchat, Some(&project)).expect("spec");
        assert_eq!(spec.kind, "chat");
        assert!(spec.is_direct, "one human plus the agent");
        assert_eq!(spec.members.len(), 2);
        assert!(spec
            .members
            .iter()
            .any(|m| m.mxid.starts_with("@_a_") && m.role == "agent"));
        assert!(room_spec(&state, "nope", None).is_none());
    }

    /// A close against a real homeserver, which is the only place the room version is real.
    /// Skipped unless `ARBOS_MATRIX_TEST_HS` points at a throwaway Synapse whose
    /// `default_room_version` is 12 and whose `app_service_config_files` holds an appservice
    /// over `@.*` with `ARBOS_MATRIX_TEST_AS_TOKEN` as its `as_token`:
    ///
    /// ```text
    /// ARBOS_MATRIX_TEST_HS=http://127.0.0.1:8008 ARBOS_MATRIX_TEST_AS_TOKEN=… \
    ///   cargo test -p arbos-relay -- closes_a_room_the_homeserver_made --nocapture
    /// ```
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn closes_a_room_the_homeserver_made() {
        let (Ok(hs_url), Ok(as_token)) = (
            std::env::var("ARBOS_MATRIX_TEST_HS"),
            std::env::var("ARBOS_MATRIX_TEST_AS_TOKEN"),
        ) else {
            return;
        };
        let cfg = Config {
            mode: crate::matrix::Mode::Dual,
            hs_url,
            server_name: "arbos.life".into(),
            as_token_users: as_token.clone(),
            as_token_agents: as_token.clone(),
            as_token_bridges: as_token,
            ..Config::off()
        };
        let mut state = test_state(AuthMode::Github);
        state.matrix = Matrix::new(cfg.clone());
        let (alice, _) = person(&state, 2001);
        let (bob, _) = person(&state, 2002);
        let chat = dm(&state, &alice, &bob);
        let worker = Worker::new(state.clone());
        worker
            .handle(&Job::SyncRoom {
                chat_id: chat.clone(),
                project_id: None,
            })
            .await
            .expect("the room is made");
        let room = state.db.matrix_room(&chat).expect("row").expect("room");
        let client = Client::new(state.http.clone(), cfg);
        let create = client
            .state(
                &room.room_id,
                &state.matrix.cfg.user_mxid(&alice),
                "m.room.create",
                "",
            )
            .await
            .expect("create event")
            .expect("create content");
        assert_eq!(
            create["room_version"].as_str(),
            Some("12"),
            "this proves nothing unless the room really is v12: {create}"
        );
        assert!(
            create["creator"].is_null(),
            "v12 carries no creator in the content; that is the whole bug: {create}"
        );
        let joined = client
            .joined_members(&room.room_id, &state.matrix.cfg.user_mxid(&alice))
            .await
            .expect("members");
        assert_eq!(joined.len(), 2, "both people are in the room: {joined:?}");

        // A close that cannot reach the homeserver must fail loudly and leave the row open,
        // never mark the chat closed while its members are still joined.
        let mut blind = state.clone();
        blind.matrix = Matrix::new(Config {
            as_token_users: "not-a-token".into(),
            ..state.matrix.cfg.clone()
        });
        Worker::new(blind)
            .handle(&Job::CloseRoom {
                chat_id: chat.clone(),
            })
            .await
            .expect_err("a close that cannot read the room is an error");
        assert!(
            state
                .db
                .matrix_room(&chat)
                .expect("row")
                .expect("room")
                .closed_at
                .is_none(),
            "a failed close leaves the room open for the retry"
        );

        state.db.delete_people_chat(&chat).expect("chat deleted");
        worker
            .handle(&Job::CloseRoom {
                chat_id: chat.clone(),
            })
            .await
            .expect("the close succeeds");
        // A member that has left can still read the room, which is how the close is checked.
        let left = client
            .room_state(&room.room_id, &state.matrix.cfg.user_mxid(&alice))
            .await
            .expect("state after the close");
        let (_, still_joined) = creator_and_joined(&left);
        assert!(
            still_joined.is_empty(),
            "nobody is left in the room after a close: {still_joined:?}"
        );
        assert!(
            state
                .db
                .matrix_room(&chat)
                .expect("row")
                .expect("room")
                .closed_at
                .is_some(),
            "the row is closed once the homeserver agrees"
        );
    }

    #[test]
    fn short_hash_is_stable() {
        assert_eq!(short_hash("x").len(), 8);
        assert_eq!(short_hash("x"), short_hash("x"));
        assert_ne!(short_hash("x"), short_hash("y"));
    }
}
