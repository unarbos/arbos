//! Servers and channels on the homeserver (D88 C1, design §3.2): a server is a **Space**
//! (`m.space` room) whose `m.space.child` events point at its channel rooms; a channel is a
//! room with `join_rule: restricted` allowing the Space's members, `m.space.parent` back, and
//! `life.arbos.chat {kind: channel}`. Every server member is joined to every channel; a
//! restricted channel (`#announcements`) sets `events_default: 50` so only owner/admins post,
//! while `events["m.reaction"] = 0` so every member can still like a post.

use serde_json::{json, Value};

use super::client::{MatrixError, MxResult};
use super::servers_db::{Channel, Server, ServerMember};
use super::worker::{Member, Worker};

/// Power levels per server role: owner is the creator (infinite in room v12); admins 50 can
/// invite, rename, post in restricted channels and manage children; members 0.
pub const ADMIN_PL: i64 = 50;

/// The rooms' view of a server: who creates, who is in, and the admin power map.
pub struct ServerSpec {
    pub server: Server,
    pub creator: String,
    pub members: Vec<Member>,
    pub users_pl: serde_json::Map<String, Value>,
}

impl Worker {
    fn server_spec(&self, server: Server, members: &[ServerMember]) -> ServerSpec {
        let cfg = &self.state.matrix.cfg;
        let creator = cfg.user_mxid(&server.owner_user_id);
        let mut mx = Vec::new();
        let mut users_pl = serde_json::Map::new();
        for m in members {
            let mxid = cfg.user_mxid(&m.user_id);
            if m.role == "admin" || (m.role == "owner" && mxid != creator) {
                users_pl.insert(mxid.clone(), json!(ADMIN_PL));
            }
            mx.push(Member {
                mxid,
                role: m.role.clone(),
                member: format!("user:{}", m.user_id),
            });
        }
        ServerSpec {
            server,
            creator,
            members: mx,
            users_pl,
        }
    }

    fn db_err(e: anyhow::Error) -> MatrixError {
        MatrixError::Shape(format!("db: {e}"))
    }

    fn via(&self) -> Vec<String> {
        vec![self.state.matrix.cfg.server_name.clone()]
    }

    /// The Space: created on first sight, then members, name and children reconciled.
    pub(super) async fn sync_server(&self, server_id: &str) -> MxResult<()> {
        let Some(server) = self.state.db.server(server_id).ok().flatten() else {
            return Ok(());
        };
        let members = self
            .state
            .db
            .server_members(server_id)
            .map_err(Self::db_err)?;
        let spec = self.server_spec(server, &members);
        for m in &spec.members {
            self.ensure_user(m).await?;
        }
        let space = match spec.server.space_room_id.clone() {
            Some(room) => {
                self.reconcile_room_members(
                    &room,
                    &spec.creator,
                    &spec.members,
                    spec.users_pl.clone(),
                )
                .await?;
                self.reconcile_room_name(&room, &spec.creator, Some(&spec.server.name))
                    .await?;
                room
            }
            None => {
                let invite: Vec<&str> = spec
                    .members
                    .iter()
                    .map(|m| m.mxid.as_str())
                    .filter(|m| *m != spec.creator)
                    .collect();
                let body = json!({
                    "preset": "private_chat",
                    "visibility": "private",
                    "name": spec.server.name,
                    "invite": invite,
                    "creation_content": { "type": "m.space", "life.arbos.chat_id": spec.server.id },
                    "initial_state": [
                        { "type": "life.arbos.chat", "state_key": "", "content": {
                            "chat_id": spec.server.id, "kind": "space", "created_by": spec.creator,
                            "created_at": spec.server.created_at, "epoch": 1 } },
                        { "type": "m.room.history_visibility", "state_key": "", "content": { "history_visibility": "shared" } },
                    ],
                    "power_level_content_override": { "users": spec.users_pl, "invite": ADMIN_PL, "state_default": ADMIN_PL },
                });
                let room = self.client.create_room(&spec.creator, &body).await?;
                self.state
                    .db
                    .set_server_space(&spec.server.id, &room)
                    .map_err(Self::db_err)?;
                let _ = self
                    .state
                    .db
                    .matrix_insert_room(&spec.server.id, &room, None, "space");
                for m in invite {
                    self.client.join(&room, m).await?;
                }
                room
            }
        };
        // Children: one m.space.child per channel room, ordered by position.
        for ch in self
            .state
            .db
            .channels_of_server(server_id)
            .map_err(Self::db_err)?
        {
            if let Some(room) = &ch.room_id {
                self.ensure_child(&space, &spec.creator, room, ch.position)
                    .await?;
            } else {
                // The channel row exists but its room does not yet: that job follows.
                self.state.matrix.touch_channel(&self.state, &ch.id);
            }
        }
        Ok(())
    }

    async fn ensure_child(
        &self,
        space: &str,
        creator: &str,
        child: &str,
        position: i64,
    ) -> MxResult<()> {
        let want = json!({ "via": self.via(), "order": format!("{position:04}") });
        let have = self
            .client
            .state(space, creator, "m.space.child", child)
            .await?;
        if have
            .as_ref()
            .map(|v| v["via"].is_array() && v["order"] == want["order"])
            != Some(true)
        {
            self.client
                .send_state(space, creator, "m.space.child", child, &want)
                .await?;
        }
        Ok(())
    }

    /// One channel room: created with the restricted join rule and the posting rule, then
    /// members, name, posting rule and parent reconciled. The Space must exist first.
    pub(super) async fn sync_channel(&self, channel_id: &str) -> MxResult<()> {
        let Some(channel) = self.state.db.channel(channel_id).ok().flatten() else {
            return Ok(());
        };
        let Some(server) = self.state.db.server(&channel.server_id).ok().flatten() else {
            return Ok(());
        };
        let Some(space) = server.space_room_id.clone() else {
            // Space first (its job re-queues this channel when done).
            self.state.matrix.touch_server(&self.state, &server.id);
            return Ok(());
        };
        let members = self
            .state
            .db
            .server_members(&server.id)
            .map_err(Self::db_err)?;
        let spec = self.server_spec(server, &members);
        for m in &spec.members {
            self.ensure_user(m).await?;
        }
        let events_default = if channel.restricted { ADMIN_PL } else { 0 };
        let room = match channel.room_id.clone() {
            Some(room) => {
                self.reconcile_room_members(
                    &room,
                    &spec.creator,
                    &spec.members,
                    spec.users_pl.clone(),
                )
                .await?;
                self.reconcile_room_name(&room, &spec.creator, Some(&channel.name))
                    .await?;
                self.reconcile_posting(&room, &spec.creator, events_default)
                    .await?;
                room
            }
            None => {
                let room = self
                    .create_channel_room(&spec, &channel, &space, events_default)
                    .await?;
                self.state
                    .db
                    .set_channel_room(&channel.id, &room)
                    .map_err(Self::db_err)?;
                let _ = self
                    .state
                    .db
                    .matrix_insert_room(&channel.id, &room, None, "channel");
                room
            }
        };
        self.ensure_child(&space, &spec.creator, &room, channel.position)
            .await?;
        Ok(())
    }

    /// Membership notice in a channel (#1349): "Jacob added Doug" as an `m.notice` from the
    /// server owner with `life.arbos.sender: system` — the apps' row mapper makes that a
    /// centred system line, never a message from the owner. A channel without a room yet
    /// gets its sync first and the notice is dropped (nothing was there to read it).
    pub(super) async fn channel_notice(
        &self,
        channel_id: &str,
        text: &str,
        notice_id: &str,
    ) -> MxResult<()> {
        let Some(channel) = self.state.db.channel(channel_id).ok().flatten() else {
            return Ok(());
        };
        let Some(server) = self.state.db.server(&channel.server_id).ok().flatten() else {
            return Ok(());
        };
        let Some(room) = channel.room_id.clone() else {
            self.state.matrix.touch_channel(&self.state, channel_id);
            return Ok(());
        };
        let creator = self.state.matrix.cfg.user_mxid(&server.owner_user_id);
        let content = json!({
            "msgtype": "m.notice",
            "body": text,
            "life.arbos.id": notice_id,
            "life.arbos.sender": "system",
        });
        self.client
            .send(&room, &creator, "m.room.message", notice_id, &content, None)
            .await?;
        Ok(())
    }

    async fn create_channel_room(
        &self,
        spec: &ServerSpec,
        channel: &Channel,
        space: &str,
        events_default: i64,
    ) -> MxResult<String> {
        let invite: Vec<&str> = spec
            .members
            .iter()
            .map(|m| m.mxid.as_str())
            .filter(|m| *m != spec.creator)
            .collect();
        let body = json!({
            "preset": "private_chat",
            "visibility": "private",
            "name": channel.name,
            "topic": channel.topic,
            "invite": invite,
            "creation_content": { "life.arbos.chat_id": channel.id },
            "initial_state": [
                { "type": "life.arbos.chat", "state_key": "", "content": {
                    "chat_id": channel.id, "kind": "channel", "server_id": channel.server_id,
                    "created_by": spec.creator, "created_at": channel.created_at, "epoch": 1 } },
                { "type": "m.room.join_rules", "state_key": "", "content": {
                    "join_rule": "restricted",
                    "allow": [{ "type": "m.room_membership", "room_id": space }] } },
                { "type": "m.space.parent", "state_key": space, "content": { "via": self.via(), "canonical": true } },
                { "type": "m.room.history_visibility", "state_key": "", "content": { "history_visibility": "shared" } },
            ],
            "power_level_content_override": {
                "users": spec.users_pl, "invite": ADMIN_PL, "state_default": ADMIN_PL,
                "events_default": events_default,
                // Announcement channels (`events_default: 50`): members cannot post, but they
                // can still like (m.reaction). Unrestricted channels keep the same override.
                "events": { "m.reaction": 0 },
            },
        });
        let room = self.client.create_room(&spec.creator, &body).await?;
        for m in invite {
            self.client.join(&room, m).await?;
        }
        Ok(room)
    }

    /// `events_default` follows the channel's posting rule; `m.reaction` stays at 0 so
    /// members can like announcement posts without being able to reply.
    async fn reconcile_posting(
        &self,
        room: &str,
        creator: &str,
        events_default: i64,
    ) -> MxResult<()> {
        if let Some(mut pl) = self
            .client
            .state(room, creator, "m.room.power_levels", "")
            .await?
        {
            let reaction_pl = pl["events"]["m.reaction"].as_i64();
            let need_default = pl["events_default"].as_i64().unwrap_or(0) != events_default;
            let need_reaction = reaction_pl != Some(0);
            if need_default || need_reaction {
                if need_default {
                    pl["events_default"] = json!(events_default);
                }
                if need_reaction {
                    if !pl["events"].is_object() {
                        pl["events"] = json!({});
                    }
                    pl["events"]["m.reaction"] = json!(0);
                }
                self.client
                    .send_state(room, creator, "m.room.power_levels", "", &pl)
                    .await?;
            }
        }
        Ok(())
    }
}
