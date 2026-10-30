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

    async fn close_room(&self, chat_id: &str) -> MxResult<()> {
        let Some(room) = self.state.db.matrix_room(chat_id).ok().flatten() else {
            return Ok(());
        };
        if room.closed_at.is_some() {
            return Ok(());
        }
        let cfg = &self.state.matrix.cfg;
        let creator = self
            .client
            .state(&room.room_id, &cfg.user_mxid("gh_0"), "m.room.create", "")
            .await
            .ok()
            .flatten()
            .and_then(|v| v["creator"].as_str().map(str::to_owned));
        // Room v12 omits `creator` from the content; fall back to the first joined relay member.
        let members = match &creator {
            Some(c) => self
                .client
                .joined_members(&room.room_id, c)
                .await
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let actor = creator.or_else(|| {
            members
                .iter()
                .find(|m| cfg.member_of_mxid(m).is_some())
                .cloned()
        });
        if let Some(actor) = actor {
            let mut content =
                json!({ "chat_id": chat_id, "kind": room.kind, "closed_at": now_ms() });
            content["project_id"] = json!(room.project_id);
            let _ = self
                .client
                .send_state(&room.room_id, &actor, "life.arbos.chat", "", &content)
                .await;
            for m in members
                .iter()
                .filter(|m| **m != actor && cfg.member_of_mxid(m).is_some())
            {
                let _ = self.client.leave(&room.room_id, m).await;
            }
            let _ = self.client.leave(&room.room_id, &actor).await;
        }
        self.state
            .db
            .matrix_close_room(chat_id)
            .map_err(|e| MatrixError::Shape(format!("db: {e}")))?;
        Ok(())
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
        // Read receipts (#1484): the relay sends this one on the person's behalf, from call
        // sites no client can reach, so with receipts off it must not be published at all.
        // Checked here rather than where the job is queued, so a job queued before the switch
        // moved is dropped too.
        if !crate::read_receipts::enabled_for(&self.state, user_id, chat_id) {
            return Ok(());
        }
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
        // Always private (`m.read.private`), for everyone, whatever their setting (Jacob,
        // 2026-10-30: "make all mirrored read receipts private"). Arbos draws no "Seen"
        // indicator of its own, so a public receipt served only other Matrix clients while
        // being permanent and unwithdrawable. One rule, so there is no branch here deciding
        // whose read activity is public.
        match self
            .client
            .read_receipt(&room.room_id, &who, &event, true)
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

    #[test]
    fn short_hash_is_stable() {
        assert_eq!(short_hash("x").len(), 8);
        assert_eq!(short_hash("x"), short_hash("x"));
        assert_ne!(short_hash("x"), short_hash("y"));
    }
}

#[cfg(test)]
mod read_receipt_tests {
    //! #1484, the mirrored half. Two rules, and they are separate things: a receipt the relay
    //! sends is always private (`m.read.private`), for everyone; and a person with read
    //! receipts off has none sent on their behalf at all.
    //!
    //! These drive the real `Client` against a stand-in homeserver and read back the path it
    //! was actually called on, because the receipt kind is in the path and a code reading is
    //! not evidence.

    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};

    use axum::extract::{Path as AxPath, State as AxState};
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::json;

    use super::*;
    use crate::matrix::{Config, Mode};
    use crate::people::tests_support::{dm, person};
    use crate::read_receipts::ChatSetting;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    type Calls = Arc<Mutex<Vec<(String, String, String)>>>;

    /// A homeserver that answers the receipt endpoint and records `(room, kind, event)`.
    async fn fake_homeserver() -> (SocketAddr, Calls) {
        let calls: Calls = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route(
                "/_matrix/client/v3/rooms/{room}/receipt/{kind}/{event}",
                post(
                    |AxState(calls): AxState<Calls>,
                     AxPath((room, kind, event)): AxPath<(String, String, String)>| async move {
                        calls.lock().unwrap().push((room, kind, event));
                        Json(json!({}))
                    },
                ),
            )
            .with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (addr, calls)
    }

    /// A chat already mirrored into a room, with one event to put the receipt on.
    fn mirrored_chat(state: &AppState, seed: i64) -> (String, String) {
        let (alice, _) = person(state, seed);
        let (bob, _) = person(state, seed + 1);
        let chat = dm(state, &alice, &bob);
        state
            .db
            .matrix_insert_room(&chat, "!room:arbos.life", None, "dm")
            .unwrap();
        state
            .db
            .matrix_insert_event(&chat, "msg-1", "$event-1", &format!("user:{bob}"), now_ms())
            .unwrap();
        (alice, chat)
    }

    fn worker_against(state: &AppState, addr: SocketAddr) -> Worker {
        let cfg = Config {
            mode: Mode::Dual,
            hs_url: format!("http://{addr}"),
            as_token_users: "as_test".into(),
            ..Config::off()
        };
        Worker {
            state: state.clone(),
            client: Client::new(state.http.clone(), cfg),
        }
    }

    /// The claim Jacob is going to repeat to people: a mirrored receipt goes in as private.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_mirrored_receipt_is_always_private() {
        let state = test_state(AuthMode::Github);
        let (alice, chat) = mirrored_chat(&state, 2001);
        let (addr, calls) = fake_homeserver().await;

        worker_against(&state, addr)
            .read(&chat, &alice)
            .await
            .expect("the receipt was accepted");

        let calls = calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1, "one receipt");
        assert_eq!(
            calls[0].1, "m.read.private",
            "a mirrored receipt must not be the public m.read; called {calls:?}",
        );
        assert_eq!(calls[0].0, "!room:arbos.life");
        assert_eq!(calls[0].2, "$event-1");
    }

    /// The other rule, and the one the switch is about: receipts off means the relay publishes
    /// nothing on that person's behalf, private or otherwise.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_receipt_at_all_is_published_for_a_person_with_receipts_off() {
        let state = test_state(AuthMode::Github);
        let (alice, chat) = mirrored_chat(&state, 2003);
        let (addr, calls) = fake_homeserver().await;
        let mut settings = json!({});
        crate::read_receipts::set_chat_override(&mut settings, ChatSetting::Off);
        state
            .db
            .set_member_settings(&chat, &format!("user:{alice}"), &settings)
            .unwrap();

        worker_against(&state, addr)
            .read(&chat, &alice)
            .await
            .expect("the job completes rather than failing");

        let calls = calls.lock().unwrap().clone();
        assert!(
            calls.is_empty(),
            "nothing may be published on her behalf; called {calls:?}",
        );
    }

    /// The account default reaches this path too, not only the per-chat override.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_account_default_also_stops_the_mirrored_receipt() {
        let state = test_state(AuthMode::Github);
        let (alice, chat) = mirrored_chat(&state, 2005);
        let (addr, calls) = fake_homeserver().await;
        state
            .db
            .set_user_read_receipts_default(&alice, false)
            .unwrap();

        worker_against(&state, addr)
            .read(&chat, &alice)
            .await
            .expect("the job completes");

        let calls = calls.lock().unwrap().clone();
        assert!(calls.is_empty(), "called {calls:?}");
    }
}
