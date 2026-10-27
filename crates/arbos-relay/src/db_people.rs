//! Storage for people chats (D72): chats, memberships (the ACL), per-device read marks, blocks,
//! per-user notification queue, and the rate-limit event log. Same SQLite file; created from
//! `Db::open` through `PEOPLE_SCHEMA`.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use arbos_core::api::{PushEventDto, UserCardDto};
use arbos_core::ids::now_ms;

use crate::db::Db;

pub const PEOPLE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS people_chats (
  id TEXT PRIMARY KEY, kind TEXT NOT NULL, created_by TEXT NOT NULL, created_at INTEGER NOT NULL,
  deleted_at INTEGER, title TEXT, image_key TEXT, avatar_leaf INTEGER, avatar_leaf_color INTEGER,
  description TEXT
);
CREATE TABLE IF NOT EXISTS people_members (
  chat_id TEXT NOT NULL, member TEXT NOT NULL, role TEXT NOT NULL, added_by TEXT NOT NULL,
  joined_at INTEGER NOT NULL, left_at INTEGER, settings TEXT NOT NULL DEFAULT '{}',
  muted INTEGER NOT NULL DEFAULT 0, silenced INTEGER NOT NULL DEFAULT 0,
  silenced_until INTEGER, banned_at INTEGER,
  PRIMARY KEY (chat_id, member)
);
CREATE INDEX IF NOT EXISTS people_members_member ON people_members(member);
CREATE TABLE IF NOT EXISTS people_reads (
  chat_id TEXT NOT NULL, device_id TEXT NOT NULL, seq INTEGER NOT NULL DEFAULT 0,
  at INTEGER NOT NULL, hidden INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (chat_id, device_id)
);
CREATE TABLE IF NOT EXISTS blocks (
  blocker TEXT NOT NULL, blocked TEXT NOT NULL, created_at INTEGER NOT NULL,
  PRIMARY KEY (blocker, blocked)
);
CREATE TABLE IF NOT EXISTS user_pushes (
  id INTEGER PRIMARY KEY AUTOINCREMENT, user_id TEXT NOT NULL, device_id TEXT NOT NULL,
  kind TEXT NOT NULL, payload TEXT NOT NULL, chat_id TEXT, created_at INTEGER NOT NULL, delivered_at INTEGER
);
CREATE INDEX IF NOT EXISTS user_pushes_device ON user_pushes(device_id, delivered_at);
CREATE TABLE IF NOT EXISTS project_members (
  project_id TEXT NOT NULL, user_id TEXT NOT NULL, role TEXT NOT NULL DEFAULT 'member',
  added_by TEXT NOT NULL, joined_at INTEGER NOT NULL, left_at INTEGER,
  PRIMARY KEY (project_id, user_id)
);
CREATE INDEX IF NOT EXISTS project_members_user ON project_members(user_id);
CREATE TABLE IF NOT EXISTS invites (
  code TEXT PRIMARY KEY, github_login TEXT NOT NULL, inviter_user_id TEXT NOT NULL,
  target_kind TEXT NOT NULL, target_id TEXT NOT NULL, created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL,
  accepted_at INTEGER, accepted_user_id TEXT, revoked_at INTEGER
);
CREATE INDEX IF NOT EXISTS invites_login ON invites(github_login);
CREATE TABLE IF NOT EXISTS people_chat_invites (
  id TEXT PRIMARY KEY, chat_id TEXT NOT NULL, code TEXT NOT NULL UNIQUE, created_by TEXT NOT NULL,
  created_at INTEGER NOT NULL, expires_at INTEGER, uses_left INTEGER, revoked_at INTEGER
);
CREATE INDEX IF NOT EXISTS people_chat_invites_chat ON people_chat_invites(chat_id, created_at);
CREATE TABLE IF NOT EXISTS rate_events (
  user_id TEXT NOT NULL, kind TEXT NOT NULL, at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS rate_events_user ON rate_events(user_id, kind, at);
"#;

pub fn migrate_people(c: &Connection) {
    let _ = c.execute(
        "ALTER TABLE users ADD COLUMN status TEXT NOT NULL DEFAULT 'member'",
        [],
    );
    let _ = c.execute("ALTER TABLE people_chats ADD COLUMN title TEXT", []);
    let _ = c.execute("ALTER TABLE people_chats ADD COLUMN image_key TEXT", []);
    let _ = c.execute(
        "ALTER TABLE people_chats ADD COLUMN avatar_leaf INTEGER",
        [],
    );
    let _ = c.execute(
        "ALTER TABLE people_chats ADD COLUMN avatar_leaf_color INTEGER",
        [],
    );
    let _ = c.execute("ALTER TABLE people_chats ADD COLUMN description TEXT", []);
    // Group admin mute (can't post) and ban (can't rejoin).
    let _ = c.execute(
        "ALTER TABLE people_members ADD COLUMN silenced INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = c.execute(
        "ALTER TABLE people_members ADD COLUMN silenced_until INTEGER",
        [],
    );
    let _ = c.execute(
        "ALTER TABLE people_members ADD COLUMN banned_at INTEGER",
        [],
    );
}

#[derive(Debug, Clone, Serialize)]
pub struct Invite {
    pub code: String,
    pub github_login: String,
    pub inviter_user_id: String,
    /// `dm` (target = chat id) or `project` (target = project id).
    pub target_kind: String,
    pub target_id: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub accepted_at: Option<i64>,
    pub accepted_user_id: Option<String>,
    pub revoked_at: Option<i64>,
}

/// One invite link of a people group chat (same shape as server invites: optional expiry
/// and use count; `revoked_at` ends it early).
#[derive(Debug, Clone, Serialize)]
pub struct GroupInvite {
    pub id: String,
    pub chat_id: String,
    pub code: String,
    pub created_by: String,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub uses_left: Option<i64>,
    pub revoked_at: Option<i64>,
}

impl GroupInvite {
    pub fn usable(&self, now: i64) -> bool {
        self.revoked_at.is_none()
            && self.expires_at.is_none_or(|t| t > now)
            && self.uses_left.is_none_or(|n| n > 0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeopleChat {
    pub id: String,
    pub kind: String,
    pub created_by: String,
    pub created_at: i64,
    /// Custom group name; when unset the API builds one from member handles.
    #[serde(default)]
    pub title: Option<String>,
    /// Blob key under `people:{chat_id}` for an optional group picture.
    #[serde(default)]
    pub image_key: Option<String>,
    /// D92 amendment: the group's leaf picture (shape 0–8, disc colour 0–9) picked on the
    /// Create screen; `None` = today's member cluster. A `image_key` picture wins.
    #[serde(default)]
    pub avatar_leaf: Option<i64>,
    #[serde(default)]
    pub avatar_leaf_color: Option<i64>,
    /// Optional about text for a people group (≤200 chars); `None` or empty = unset.
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Membership {
    pub chat_id: String,
    /// `user:<user_id>` or `agent:<project_id>`.
    pub member: String,
    /// `owner` | `member` | `agent`.
    pub role: String,
    pub added_by: String,
    pub joined_at: i64,
    pub settings: Value,
    pub muted: bool,
    /// Admin muted this member from posting (not notification mute).
    pub silenced: bool,
    /// When the admin mute ends (ms). `None` while muted means until unmuted.
    #[serde(default)]
    pub silenced_until: Option<i64>,
}

impl Membership {
    pub fn user_id(&self) -> Option<&str> {
        self.member.strip_prefix("user:")
    }

    pub fn project_id(&self) -> Option<&str> {
        self.member.strip_prefix("agent:")
    }
}

/// Who is in a room right now: `(members, head_seq, last_row_at)` come from separate calls; this
/// is the chat plus its live memberships.
#[derive(Debug, Clone)]
pub struct ChatWithMembers {
    pub chat: PeopleChat,
    pub members: Vec<Membership>,
}

fn membership_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Membership> {
    Ok(Membership {
        chat_id: r.get(0)?,
        member: r.get(1)?,
        role: r.get(2)?,
        added_by: r.get(3)?,
        joined_at: r.get(4)?,
        settings: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or(Value::Null),
        muted: r.get::<_, i64>(6)? != 0,
        silenced: r.get::<_, i64>(7).unwrap_or(0) != 0,
        silenced_until: r.get::<_, Option<i64>>(8).unwrap_or(None),
    })
}

const MEMBER_COLS: &str =
    "chat_id, member, role, added_by, joined_at, settings, muted, silenced, silenced_until";

fn people_chat_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<PeopleChat> {
    Ok(PeopleChat {
        id: r.get(0)?,
        kind: r.get(1)?,
        created_by: r.get(2)?,
        created_at: r.get(3)?,
        title: r.get(4)?,
        image_key: r.get(5)?,
        avatar_leaf: r.get(6)?,
        avatar_leaf_color: r.get(7)?,
        description: r.get(8)?,
    })
}

impl Db {
    // ---- chats and members --------------------------------------------------------------------

    pub fn insert_people_chat(&self, chat: &PeopleChat) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO people_chats(id, kind, created_by, created_at, title, avatar_leaf, avatar_leaf_color, description) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![chat.id, chat.kind, chat.created_by, chat.created_at, chat.title, chat.avatar_leaf, chat.avatar_leaf_color, chat.description],
            )
            .map(|_| ())
        })
    }

    pub fn people_chat(&self, id: &str) -> Result<Option<PeopleChat>> {
        self.with(|c| {
            c.query_row(
                "SELECT id, kind, created_by, created_at, title, image_key, avatar_leaf, avatar_leaf_color, description FROM people_chats WHERE id = ?1 AND deleted_at IS NULL",
                params![id],
                people_chat_from_row,
            )
            .optional()
        })
    }

    /// Set a custom group title, picture key, and/or description. Empty strings clear the field.
    pub fn update_people_chat_profile(
        &self,
        chat_id: &str,
        title: Option<Option<&str>>,
        image_key: Option<Option<&str>>,
        leaf: Option<Option<(i64, i64)>>,
        description: Option<Option<&str>>,
    ) -> Result<bool> {
        self.with(|c| {
            if let Some(l) = leaf {
                c.execute(
                    "UPDATE people_chats SET avatar_leaf = ?2, avatar_leaf_color = ?3 WHERE id = ?1 AND deleted_at IS NULL",
                    params![chat_id, l.map(|l| l.0), l.map(|l| l.1)],
                )?;
            }
            if let Some(t) = title {
                c.execute(
                    "UPDATE people_chats SET title = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                    params![chat_id, t],
                )?;
            }
            if let Some(k) = image_key {
                c.execute(
                    "UPDATE people_chats SET image_key = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                    params![chat_id, k],
                )?;
            }
            if let Some(d) = description {
                c.execute(
                    "UPDATE people_chats SET description = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                    params![chat_id, d],
                )?;
            }
            Ok(true)
        })
    }

    pub fn add_member(
        &self,
        chat_id: &str,
        member: &str,
        role: &str,
        added_by: &str,
        settings: &Value,
    ) -> Result<()> {
        if self.member_is_banned(chat_id, member)? {
            anyhow::bail!("banned");
        }
        self.with(|c| {
            c.execute(
                "INSERT INTO people_members(chat_id, member, role, added_by, joined_at, settings)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(chat_id, member) DO UPDATE SET role = excluded.role, added_by = excluded.added_by,
                    joined_at = excluded.joined_at, left_at = NULL, silenced = 0, silenced_until = NULL,
                    settings = excluded.settings",
                params![chat_id, member, role, added_by, now_ms(), settings.to_string()],
            )
            .map(|_| ())
        })
    }

    /// True when this person was banned from the chat (they cannot rejoin).
    pub fn member_is_banned(&self, chat_id: &str, member: &str) -> Result<bool> {
        self.with(|c| {
            let banned: Option<i64> = c
                .query_row(
                    "SELECT banned_at FROM people_members WHERE chat_id = ?1 AND member = ?2",
                    params![chat_id, member],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            Ok(banned.is_some())
        })
    }

    /// Soft-leave a member. When `ban` is true they cannot be re-added.
    pub fn remove_member(&self, chat_id: &str, member: &str, ban: bool) -> Result<bool> {
        self.with(|c| {
            let now = now_ms();
            Ok(c.execute(
                "UPDATE people_members SET left_at = ?3,
                    banned_at = CASE WHEN ?4 THEN ?3 ELSE banned_at END,
                    silenced = 0, silenced_until = NULL
                 WHERE chat_id = ?1 AND member = ?2 AND left_at IS NULL",
                params![chat_id, member, now, ban as i64],
            )? > 0)
        })
    }

    /// Clear a ban so the person can be added again. Does not re-add them.
    pub fn unban_member(&self, chat_id: &str, member: &str) -> Result<bool> {
        self.with(|c| {
            c.execute(
                "UPDATE people_members SET banned_at = NULL
                 WHERE chat_id = ?1 AND member = ?2 AND banned_at IS NOT NULL",
                params![chat_id, member],
            )
            .map(|n| n > 0)
        })
    }

    /// Banned people for the group member list's Unban rows (not live members).
    pub fn banned_members_of(&self, chat_id: &str) -> Result<Vec<Membership>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {MEMBER_COLS} FROM people_members
                 WHERE chat_id = ?1 AND banned_at IS NOT NULL ORDER BY banned_at DESC"
            ))?;
            let rows = st.query_map(params![chat_id], membership_from_row)?;
            rows.collect()
        })
    }

    /// Mute (`silenced`) from posting. `until` is ms when the mute ends; `None` = until unmuted.
    pub fn set_member_silenced(
        &self,
        chat_id: &str,
        member: &str,
        silenced: bool,
        until: Option<i64>,
    ) -> Result<bool> {
        self.with(|c| {
            let until = if silenced { until } else { None };
            c.execute(
                "UPDATE people_members SET silenced = ?3, silenced_until = ?4
                 WHERE chat_id = ?1 AND member = ?2 AND left_at IS NULL",
                params![chat_id, member, silenced as i64, until],
            )
            .map(|n| n > 0)
        })
    }

    /// Owner sets a human member's role to `admin` or `member`.
    pub fn set_member_role(&self, chat_id: &str, member: &str, role: &str) -> Result<bool> {
        self.with(|c| {
            c.execute(
                "UPDATE people_members SET role = ?3
                 WHERE chat_id = ?1 AND member = ?2 AND left_at IS NULL AND role != 'owner'",
                params![chat_id, member, role],
            )
            .map(|n| n > 0)
        })
    }

    pub fn set_member_settings(
        &self,
        chat_id: &str,
        member: &str,
        settings: &Value,
    ) -> Result<bool> {
        self.with(|c| {
            c.execute(
                "UPDATE people_members SET settings = ?3 WHERE chat_id = ?1 AND member = ?2 AND left_at IS NULL",
                params![chat_id, member, settings.to_string()],
            )
            .map(|n| n > 0)
        })
    }

    pub fn set_member_muted(&self, chat_id: &str, member: &str, muted: bool) -> Result<bool> {
        self.with(|c| {
            c.execute(
                "UPDATE people_members SET muted = ?3 WHERE chat_id = ?1 AND member = ?2 AND left_at IS NULL",
                params![chat_id, member, muted as i64],
            )
            .map(|n| n > 0)
        })
    }

    pub fn members_of(&self, chat_id: &str) -> Result<Vec<Membership>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {MEMBER_COLS} FROM people_members WHERE chat_id = ?1 AND left_at IS NULL ORDER BY joined_at"
            ))?;
            let rows = st.query_map(params![chat_id], membership_from_row)?;
            rows.collect()
        })
    }

    pub fn membership(&self, chat_id: &str, member: &str) -> Result<Option<Membership>> {
        self.with(|c| {
            c.query_row(
                &format!("SELECT {MEMBER_COLS} FROM people_members WHERE chat_id = ?1 AND member = ?2 AND left_at IS NULL"),
                params![chat_id, member],
                membership_from_row,
            )
            .optional()
        })
    }

    /// Live chats a member (`user:<id>` or `agent:<project>`) is in.
    pub fn chats_of_member(&self, member: &str) -> Result<Vec<ChatWithMembers>> {
        let chats: Vec<PeopleChat> = self.with(|c| {
            let mut st = c.prepare(
                "SELECT p.id, p.kind, p.created_by, p.created_at, p.title, p.image_key, p.avatar_leaf, p.avatar_leaf_color, p.description FROM people_chats p
                 JOIN people_members m ON m.chat_id = p.id
                 WHERE m.member = ?1 AND m.left_at IS NULL AND p.deleted_at IS NULL
                 ORDER BY p.created_at DESC",
            )?;
            let rows = st.query_map(params![member], people_chat_from_row)?;
            rows.collect()
        })?;
        chats
            .into_iter()
            .map(|chat| {
                let members = self.members_of(&chat.id)?;
                Ok(ChatWithMembers { chat, members })
            })
            .collect()
    }

    /// The existing DM between two users, if any (both still members).
    pub fn dm_between(&self, a: &str, b: &str) -> Result<Option<PeopleChat>> {
        self.with(|c| {
            c.query_row(
                "SELECT p.id, p.kind, p.created_by, p.created_at, p.title, p.image_key, p.avatar_leaf, p.avatar_leaf_color, p.description FROM people_chats p
                 JOIN people_members ma ON ma.chat_id = p.id AND ma.member = ?1 AND ma.left_at IS NULL
                 JOIN people_members mb ON mb.chat_id = p.id AND mb.member = ?2 AND mb.left_at IS NULL
                 WHERE p.kind = 'dm' AND p.deleted_at IS NULL ORDER BY p.created_at LIMIT 1",
                params![format!("user:{a}"), format!("user:{b}")],
                people_chat_from_row,
            )
            .optional()
        })
    }

    pub fn set_people_chat_kind(&self, chat_id: &str, kind: &str) -> Result<bool> {
        self.with(|c| {
            c.execute(
                "UPDATE people_chats SET kind = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                params![chat_id, kind],
            )
            .map(|n| n > 0)
        })
    }

    pub fn delete_people_chat(&self, chat_id: &str) -> Result<bool> {
        self.with(|c| {
            c.execute(
                "UPDATE people_chats SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                params![chat_id, now_ms()],
            )
            .map(|n| n > 0)
        })
    }

    pub fn people_chats_created_since(&self, user_id: &str, since_ms: i64) -> Result<i64> {
        self.with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM people_chats WHERE created_by = ?1 AND created_at >= ?2",
                params![user_id, since_ms],
                |r| r.get(0),
            )
        })
    }

    /// Drop a room entirely: rows, checkpoint, sequence, blob index.
    pub fn delete_room(&self, room: &str) -> Result<()> {
        self.with(|c| {
            c.execute("DELETE FROM rows WHERE room = ?1", params![room])?;
            c.execute("DELETE FROM checkpoints WHERE room = ?1", params![room])?;
            c.execute("DELETE FROM room_seq WHERE room = ?1", params![room])?;
            Ok(())
        })
    }

    // ---- read marks and hide ------------------------------------------------------------------

    pub fn read_mark(&self, chat_id: &str, device_id: &str) -> Result<(i64, bool)> {
        self.with(|c| {
            c.query_row(
                "SELECT seq, hidden FROM people_reads WHERE chat_id = ?1 AND device_id = ?2",
                params![chat_id, device_id],
                |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0)),
            )
            .optional()
            .map(|o| o.unwrap_or((0, false)))
        })
    }

    pub fn set_read_mark(&self, chat_id: &str, device_id: &str, seq: i64) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO people_reads(chat_id, device_id, seq, at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(chat_id, device_id) DO UPDATE SET seq = MAX(people_reads.seq, excluded.seq), at = excluded.at, hidden = 0",
                params![chat_id, device_id, seq, now_ms()],
            )
            .map(|_| ())
        })
    }

    pub fn set_hidden(&self, chat_id: &str, device_id: &str, hidden: bool) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO people_reads(chat_id, device_id, seq, at, hidden) VALUES (?1, ?2, 0, ?3, ?4)
                 ON CONFLICT(chat_id, device_id) DO UPDATE SET hidden = excluded.hidden, at = excluded.at",
                params![chat_id, device_id, now_ms(), hidden as i64],
            )
            .map(|_| ())
        })
    }

    /// `(head_seq, last_row_at)` of a room; zeros when it has no rows.
    pub fn room_activity(&self, room: &str) -> Result<(i64, i64)> {
        self.with(|c| {
            let head: i64 = c
                .query_row(
                    "SELECT head FROM room_seq WHERE room = ?1",
                    params![room],
                    |r| r.get(0),
                )
                .optional()?
                .unwrap_or(0);
            let last: i64 = c
                .query_row(
                    "SELECT COALESCE(MAX(created_at), 0) FROM rows WHERE room = ?1",
                    params![room],
                    |r| r.get(0),
                )
                .optional()?
                .unwrap_or(0);
            Ok((head, last))
        })
    }

    // ---- blocks -------------------------------------------------------------------------------

    pub fn block(&self, blocker: &str, blocked: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT OR IGNORE INTO blocks(blocker, blocked, created_at) VALUES (?1, ?2, ?3)",
                params![blocker, blocked, now_ms()],
            )
            .map(|_| ())
        })
    }

    pub fn unblock(&self, blocker: &str, blocked: &str) -> Result<bool> {
        self.with(|c| {
            c.execute(
                "DELETE FROM blocks WHERE blocker = ?1 AND blocked = ?2",
                params![blocker, blocked],
            )
            .map(|n| n > 0)
        })
    }

    /// Does `blocker` block `blocked`?
    pub fn blocks(&self, blocker: &str, blocked: &str) -> Result<bool> {
        self.with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM blocks WHERE blocker = ?1 AND blocked = ?2",
                params![blocker, blocked],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
        })
    }

    /// Is either of the two blocking the other?
    pub fn blocked_either_way(&self, a: &str, b: &str) -> Result<bool> {
        self.with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM blocks WHERE (blocker = ?1 AND blocked = ?2) OR (blocker = ?2 AND blocked = ?1)",
                params![a, b],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
        })
    }

    /// Does anyone in this list block `user`?
    pub fn blocked_by_any(&self, user: &str, others: &[String]) -> Result<bool> {
        for o in others {
            if o != user {
                let hit: bool = self.with(|c| {
                    c.query_row(
                        "SELECT COUNT(*) FROM blocks WHERE blocker = ?1 AND blocked = ?2",
                        params![o, user],
                        |r| r.get::<_, i64>(0),
                    )
                    .map(|n| n > 0)
                })?;
                if hit {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    pub fn blocks_of(&self, blocker: &str) -> Result<Vec<(String, i64)>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT blocked, created_at FROM blocks WHERE blocker = ?1 ORDER BY created_at DESC",
            )?;
            let rows = st.query_map(params![blocker], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })
    }

    // ---- per-user pushes (fan-out to every live device) ---------------------------------------

    /// Queue a typed `/me/pushes` event for every live device of a user.
    pub fn push_user_event(
        &self,
        user_id: &str,
        event: &PushEventDto,
        coalesce_chat: Option<&str>,
    ) -> Result<()> {
        let (kind, payload) = event.kind_and_payload();
        self.push_user(user_id, &kind, &payload, coalesce_chat)
    }

    /// Queue a typed event for every live device except `except_device`.
    pub fn push_user_except_event(
        &self,
        user_id: &str,
        except_device: &str,
        event: &PushEventDto,
    ) -> Result<()> {
        let (kind, payload) = event.kind_and_payload();
        self.push_user_except(user_id, except_device, &kind, &payload)
    }

    /// Queue a notification for every non-revoked device of a user. `coalesce_chat` keeps at
    /// most one undelivered push of this kind per chat per device.
    pub fn push_user(
        &self,
        user_id: &str,
        kind: &str,
        payload: &Value,
        coalesce_chat: Option<&str>,
    ) -> Result<()> {
        let now = now_ms();
        let body = payload.to_string();
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT id FROM devices WHERE user_id = ?1 AND revoked_at IS NULL",
            )?;
            let devices: Vec<String> = st
                .query_map(params![user_id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for d in devices {
                if let Some(chat) = coalesce_chat {
                    let pending: i64 = c.query_row(
                        "SELECT COUNT(*) FROM user_pushes WHERE device_id = ?1 AND kind = ?2 AND chat_id = ?3 AND delivered_at IS NULL",
                        params![d, kind, chat],
                        |r| r.get(0),
                    )?;
                    if pending > 0 {
                        continue;
                    }
                }
                c.execute(
                    "INSERT INTO user_pushes(user_id, device_id, kind, payload, chat_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![user_id, d, kind, body, coalesce_chat, now],
                )?;
            }
            Ok(())
        })
    }

    /// Queue a notification for every live device of a user except `except_device` (the one
    /// that caused it already knows).
    pub fn push_user_except(
        &self,
        user_id: &str,
        except_device: &str,
        kind: &str,
        payload: &Value,
    ) -> Result<()> {
        let now = now_ms();
        let body = payload.to_string();
        self.with(|c| {
            c.execute(
                "INSERT INTO user_pushes(user_id, device_id, kind, payload, chat_id, created_at)
                 SELECT ?1, id, ?2, ?3, NULL, ?4 FROM devices WHERE user_id = ?1 AND revoked_at IS NULL AND id != ?5",
                params![user_id, kind, body, now, except_device],
            )
            .map(|_| ())
        })
    }

    /// Clear-on-read fan-out, queue form: for every live device of the user except
    /// `except_device` (the one that read), drop the undelivered `people_message` rows for
    /// `chat_id` — a device that has not drained yet must not post a banner for something
    /// already read — and any pending clear for the chat, then queue one `kind` row carrying
    /// `payload`. At most one clear per chat per device is pending, always with the newest
    /// badge. Returns the ids of the devices told.
    pub fn queue_chat_clear(
        &self,
        user_id: &str,
        except_device: &str,
        chat_id: &str,
        kind: &str,
        payload: &Value,
    ) -> Result<Vec<String>> {
        let now = now_ms();
        let body = payload.to_string();
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT id FROM devices WHERE user_id = ?1 AND revoked_at IS NULL AND id != ?2",
            )?;
            let devices: Vec<String> = st
                .query_map(params![user_id, except_device], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for d in &devices {
                c.execute(
                    "DELETE FROM user_pushes WHERE device_id = ?1 AND chat_id = ?2 AND delivered_at IS NULL AND kind IN ('people_message', ?3)",
                    params![d, chat_id, kind],
                )?;
                c.execute(
                    "INSERT INTO user_pushes(user_id, device_id, kind, payload, chat_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![user_id, d, kind, body, chat_id, now],
                )?;
            }
            Ok(devices)
        })
    }

    pub fn take_user_pushes(&self, device_id: &str) -> Result<Vec<(i64, String, Value, i64)>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT id, kind, payload, created_at FROM user_pushes WHERE device_id = ?1 AND delivered_at IS NULL ORDER BY id",
            )?;
            let items: Vec<(i64, String, Value, i64)> = st
                .query_map(params![device_id], |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        serde_json::from_str(&r.get::<_, String>(2)?).unwrap_or_default(),
                        r.get(3)?,
                    ))
                })?
                .collect::<rusqlite::Result<_>>()?;
            let now = now_ms();
            for (id, _, _, _) in &items {
                c.execute(
                    "UPDATE user_pushes SET delivered_at = ?2 WHERE id = ?1",
                    params![id, now],
                )?;
            }
            c.execute(
                "DELETE FROM user_pushes WHERE delivered_at IS NOT NULL AND delivered_at < ?1",
                params![now - 7 * 24 * 3_600_000],
            )?;
            Ok(items)
        })
    }

    // ---- invite links -------------------------------------------------------------------------

    pub fn insert_invite(&self, inv: &Invite) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO invites(code, github_login, inviter_user_id, target_kind, target_id, created_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![inv.code, inv.github_login, inv.inviter_user_id, inv.target_kind, inv.target_id, inv.created_at, inv.expires_at],
            )
            .map(|_| ())
        })
    }

    fn invite_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Invite> {
        Ok(Invite {
            code: r.get(0)?,
            github_login: r.get(1)?,
            inviter_user_id: r.get(2)?,
            target_kind: r.get(3)?,
            target_id: r.get(4)?,
            created_at: r.get(5)?,
            expires_at: r.get(6)?,
            accepted_at: r.get(7)?,
            accepted_user_id: r.get(8)?,
            revoked_at: r.get(9)?,
        })
    }

    const INVITE_COLS: &'static str = "code, github_login, inviter_user_id, target_kind, target_id, created_at, expires_at, accepted_at, accepted_user_id, revoked_at";

    pub fn invite(&self, code: &str) -> Result<Option<Invite>> {
        self.with(|c| {
            c.query_row(
                &format!("SELECT {} FROM invites WHERE code = ?1", Self::INVITE_COLS),
                params![code],
                Self::invite_from_row,
            )
            .optional()
        })
    }

    pub fn invites_by(&self, inviter: &str) -> Result<Vec<Invite>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM invites WHERE inviter_user_id = ?1 ORDER BY created_at DESC",
                Self::INVITE_COLS
            ))?;
            let rows = st.query_map(params![inviter], Self::invite_from_row)?;
            rows.collect()
        })
    }

    /// An open invite for this target and login, to hand back the same link twice.
    pub fn open_invite_for(
        &self,
        login: &str,
        target_kind: &str,
        target_id: &str,
    ) -> Result<Option<Invite>> {
        self.with(|c| {
            c.query_row(
                &format!(
                    "SELECT {} FROM invites WHERE lower(github_login) = lower(?1) AND target_kind = ?2 AND target_id = ?3
                     AND accepted_at IS NULL AND revoked_at IS NULL AND expires_at > ?4 ORDER BY created_at DESC LIMIT 1",
                    Self::INVITE_COLS
                ),
                params![login, target_kind, target_id, now_ms()],
                Self::invite_from_row,
            )
            .optional()
        })
    }

    /// Mark every open invite for this GitHub login accepted by `user_id`. Returns the inviters
    /// (deduplicated) so they can be told.
    pub fn accept_invites_for_login(&self, login: &str, user_id: &str) -> Result<Vec<Invite>> {
        let now = now_ms();
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM invites WHERE lower(github_login) = lower(?1) AND accepted_at IS NULL AND revoked_at IS NULL AND expires_at > ?2",
                Self::INVITE_COLS
            ))?;
            let open: Vec<Invite> = st
                .query_map(params![login, now], Self::invite_from_row)?
                .collect::<rusqlite::Result<_>>()?;
            for inv in &open {
                c.execute(
                    "UPDATE invites SET accepted_at = ?2, accepted_user_id = ?3 WHERE code = ?1",
                    params![inv.code, now, user_id],
                )?;
            }
            Ok(open)
        })
    }

    pub fn revoke_invite(&self, code: &str, inviter: &str) -> Result<bool> {
        self.with(|c| {
            c.execute(
                "UPDATE invites SET revoked_at = ?3 WHERE code = ?1 AND inviter_user_id = ?2 AND revoked_at IS NULL AND accepted_at IS NULL",
                params![code, inviter, now_ms()],
            )
            .map(|n| n > 0)
        })
    }

    // ---- group chat invite links --------------------------------------------------------------

    const GROUP_INVITE_COLS: &'static str =
        "id, chat_id, code, created_by, created_at, expires_at, uses_left, revoked_at";

    fn group_invite_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<GroupInvite> {
        Ok(GroupInvite {
            id: r.get(0)?,
            chat_id: r.get(1)?,
            code: r.get(2)?,
            created_by: r.get(3)?,
            created_at: r.get(4)?,
            expires_at: r.get(5)?,
            uses_left: r.get(6)?,
            revoked_at: r.get(7)?,
        })
    }

    pub fn insert_group_invite(&self, inv: &GroupInvite) -> Result<()> {
        self.with(|c| {
            c.execute(
                &format!(
                    "INSERT INTO people_chat_invites({}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    Self::GROUP_INVITE_COLS
                ),
                params![
                    inv.id,
                    inv.chat_id,
                    inv.code,
                    inv.created_by,
                    inv.created_at,
                    inv.expires_at,
                    inv.uses_left,
                    inv.revoked_at
                ],
            )?;
            Ok(())
        })
    }

    /// Live invites of a group (not revoked), newest first; expired / used-up ones included
    /// so members see why a link stopped working.
    pub fn invites_of_group(&self, chat_id: &str) -> Result<Vec<GroupInvite>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM people_chat_invites WHERE chat_id = ?1 AND revoked_at IS NULL ORDER BY created_at DESC",
                Self::GROUP_INVITE_COLS
            ))?;
            let rows = st.query_map(params![chat_id], Self::group_invite_from_row)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
    }

    pub fn group_invite_by_code(&self, code: &str) -> Result<Option<GroupInvite>> {
        self.with(|c| {
            c.query_row(
                &format!(
                    "SELECT {} FROM people_chat_invites WHERE code = ?1",
                    Self::GROUP_INVITE_COLS
                ),
                params![code],
                Self::group_invite_from_row,
            )
            .optional()
        })
    }

    pub fn revoke_group_invite(&self, chat_id: &str, invite_id: &str) -> Result<bool> {
        self.with(|c| {
            Ok(c.execute(
                "UPDATE people_chat_invites SET revoked_at = ?3 WHERE chat_id = ?1 AND id = ?2 AND revoked_at IS NULL",
                params![chat_id, invite_id, now_ms()],
            )? > 0)
        })
    }

    /// One use of a limited group link; `false` when it had none left.
    pub fn consume_group_invite_use(&self, invite_id: &str) -> Result<bool> {
        self.with(|c| {
            Ok(c.execute(
                "UPDATE people_chat_invites SET uses_left = uses_left - 1 WHERE id = ?1 AND (uses_left IS NULL OR uses_left > 0)",
                params![invite_id],
            )? > 0)
        })
    }

    // ---- rate limits (sliding window over logged events) --------------------------------------

    /// Log one event and return how many of this kind the user has in the window (incl. this one).
    pub fn rate_event(&self, user_id: &str, kind: &str, window_ms: i64) -> Result<i64> {
        let now = now_ms();
        self.with(|c| {
            c.execute(
                "INSERT INTO rate_events(user_id, kind, at) VALUES (?1, ?2, ?3)",
                params![user_id, kind, now],
            )?;
            c.execute(
                "DELETE FROM rate_events WHERE at < ?1",
                params![now - 24 * 3_600_000],
            )?;
            c.query_row(
                "SELECT COUNT(*) FROM rate_events WHERE user_id = ?1 AND kind = ?2 AND at >= ?3",
                params![user_id, kind, now - window_ms],
                |r| r.get(0),
            )
        })
    }

    /// Count events in the window first; insert one only when under `limit`. Returns `true` when
    /// the event was noted (allowed), `false` when the user is already at the limit (refused
    /// pushes must not count, or a reconnect storm never drains).
    pub fn rate_check_and_note(
        &self,
        user_id: &str,
        kind: &str,
        window_ms: i64,
        limit: i64,
    ) -> Result<bool> {
        let now = now_ms();
        self.with(|c| {
            let count: i64 = c.query_row(
                "SELECT COUNT(*) FROM rate_events WHERE user_id = ?1 AND kind = ?2 AND at >= ?3",
                params![user_id, kind, now - window_ms],
                |r| r.get(0),
            )?;
            if count >= limit {
                return Ok(false);
            }
            c.execute(
                "INSERT INTO rate_events(user_id, kind, at) VALUES (?1, ?2, ?3)",
                params![user_id, kind, now],
            )?;
            c.execute(
                "DELETE FROM rate_events WHERE at < ?1",
                params![now - 24 * 3_600_000],
            )?;
            Ok(true)
        })
    }

    /// How many events of this kind the user has in the window (no insert).
    #[cfg(test)]
    pub fn rate_count(&self, user_id: &str, kind: &str, window_ms: i64) -> Result<i64> {
        let now = now_ms();
        self.with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM rate_events WHERE user_id = ?1 AND kind = ?2 AND at >= ?3",
                params![user_id, kind, now - window_ms],
                |r| r.get(0),
            )
        })
    }

    /// Seconds until the oldest event of this kind leaves `window_ms` (at least 1). Used for
    /// `retryS` on a `rate_limited` room error frame.
    pub fn rate_retry_s(&self, user_id: &str, kind: &str, window_ms: i64) -> Result<i64> {
        let now = now_ms();
        self.with(|c| {
            let oldest: Option<i64> = c
                .query_row(
                    "SELECT MIN(at) FROM rate_events WHERE user_id = ?1 AND kind = ?2 AND at >= ?3",
                    params![user_id, kind, now - window_ms],
                    |r| r.get::<_, Option<i64>>(0),
                )
                .optional()?
                .flatten();
            Ok(match oldest {
                Some(at) => ((at + window_ms - now + 999) / 1000).max(1),
                None => 1,
            })
        })
    }

    // ---- projects together (step 3) -----------------------------------------------------------

    pub fn add_project_member(
        &self,
        project_id: &str,
        user_id: &str,
        added_by: &str,
    ) -> Result<bool> {
        self.with(|c| {
            let live: i64 = c.query_row(
                "SELECT COUNT(*) FROM project_members WHERE project_id = ?1 AND user_id = ?2 AND left_at IS NULL",
                params![project_id, user_id],
                |r| r.get(0),
            )?;
            if live > 0 {
                return Ok(false);
            }
            c.execute(
                "INSERT INTO project_members(project_id, user_id, role, added_by, joined_at) VALUES (?1, ?2, 'member', ?3, ?4)
                 ON CONFLICT(project_id, user_id) DO UPDATE SET added_by = excluded.added_by, joined_at = excluded.joined_at, left_at = NULL",
                params![project_id, user_id, added_by, now_ms()],
            )?;
            Ok(true)
        })
    }

    pub fn remove_project_member(&self, project_id: &str, user_id: &str) -> Result<bool> {
        self.with(|c| {
            c.execute(
                "UPDATE project_members SET left_at = ?3 WHERE project_id = ?1 AND user_id = ?2 AND left_at IS NULL",
                params![project_id, user_id, now_ms()],
            )
            .map(|n| n > 0)
        })
    }

    pub fn is_project_member(&self, project_id: &str, user_id: &str) -> Result<bool> {
        self.with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM project_members WHERE project_id = ?1 AND user_id = ?2 AND left_at IS NULL",
                params![project_id, user_id],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
        })
    }

    /// `(user_id, added_by, joined_at)` of the live guests of a project.
    pub fn project_members(&self, project_id: &str) -> Result<Vec<(String, String, i64)>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT user_id, added_by, joined_at FROM project_members WHERE project_id = ?1 AND left_at IS NULL ORDER BY joined_at",
            )?;
            let rows = st.query_map(params![project_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect()
        })
    }

    /// Project ids shared with a user (they are a live guest).
    pub fn shared_project_ids(&self, user_id: &str) -> Result<Vec<String>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT project_id FROM project_members WHERE user_id = ?1 AND left_at IS NULL ORDER BY joined_at",
            )?;
            let rows = st.query_map(params![user_id], |r| r.get(0))?;
            rows.collect()
        })
    }

    // ---- users: invited placeholders ----------------------------------------------------------

    /// The directory: every member or invited user with a handle, except `exclude`, by handle;
    /// `(rows, total)`.
    pub fn list_directory(
        &self,
        exclude: &str,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<UserCardDto>, i64)> {
        self.with(|c| {
            let total: i64 = c.query_row(
                "SELECT COUNT(*) FROM users WHERE login IS NOT NULL AND status IN ('member', 'invited') AND id != ?1",
                params![exclude],
                |r| r.get(0),
            )?;
            let mut st = c.prepare(
                "SELECT id, login, avatar_url, name, status, test FROM users
                 WHERE login IS NOT NULL AND status IN ('member', 'invited') AND id != ?1
                 ORDER BY lower(login) LIMIT ?2 OFFSET ?3",
            )?;
            let rows = st
                .query_map(params![exclude, limit, offset], |r| {
                    Ok(UserCardDto {
                        id: r.get::<_, String>(0)?,
                        handle: r.get::<_, String>(1)?,
                        avatar_url: r.get::<_, Option<String>>(2)?,
                        name: r.get::<_, Option<String>>(3)?,
                        status: r.get::<_, String>(4)?,
                        test: Some(r.get::<_, i64>(5)? != 0),
                        created_at: None,
                        last_seen_at: None,
                        online: None,
                        avatar_color_index_effective: None,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((rows, total))
        })
    }

    /// Prefix search on GitHub logins (members and invited placeholders).
    pub fn search_users(&self, prefix: &str, limit: i64) -> Result<Vec<UserCardDto>> {
        let like = format!(
            "{}%",
            prefix.replace('%', "").replace('_', "\\_").to_lowercase()
        );
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT id, login, avatar_url, name, status, test FROM users WHERE login IS NOT NULL AND status IN ('member', 'invited') AND lower(login) LIKE ?1 ESCAPE '\\' ORDER BY (status = 'member') DESC, lower(login) LIMIT ?2",
            )?;
            let rows = st.query_map(params![like, limit], |r| {
                Ok(UserCardDto {
                    id: r.get::<_, String>(0)?,
                    handle: r.get::<_, String>(1)?,
                    avatar_url: r.get::<_, Option<String>>(2)?,
                    name: r.get::<_, Option<String>>(3)?,
                    status: r.get::<_, String>(4)?,
                    test: Some(r.get::<_, i64>(5)? != 0),
                    created_at: None,
                    last_seen_at: None,
                    online: None,
                    avatar_color_index_effective: None,
                })
            })?;
            rows.collect()
        })
    }

    /// A placeholder for a GitHub account that has not signed in (`status = invited`). No grant
    /// until the first sign-in. Returns the user id.
    pub fn upsert_invited_user(
        &self,
        github_id: i64,
        login: &str,
        avatar_url: Option<&str>,
        name: Option<&str>,
    ) -> Result<String> {
        let id = format!("gh_{github_id}");
        self.with(|c| {
            c.execute(
                "INSERT INTO users(id, snapshot, spend_cap_usd, created_at, github_id, login, avatar_url, name, status)
                 VALUES (?1, NULL, 0.0, ?2, ?3, ?4, ?5, ?6, 'invited')
                 ON CONFLICT(id) DO UPDATE SET login = excluded.login,
                    avatar_url = COALESCE(excluded.avatar_url, users.avatar_url), name = COALESCE(excluded.name, users.name)",
                params![id, now_ms(), github_id, login, avatar_url, name],
            )
            .map(|_| ())
        })?;
        Ok(id)
    }

    /// `(id, login, avatar_url, name, status)` for a handle, case-insensitive.
    pub fn user_by_handle(&self, handle: &str) -> Result<Option<UserCardDto>> {
        self.with(|c| {
            c.query_row(
                "SELECT id, login, avatar_url, name, status FROM users WHERE lower(login) = lower(?1)",
                params![handle],
                |r| {
                    Ok(UserCardDto {
                        id: r.get::<_, String>(0)?,
                        handle: r.get::<_, String>(1)?,
                        avatar_url: r.get::<_, Option<String>>(2)?,
                        name: r.get::<_, Option<String>>(3)?,
                        status: r.get::<_, String>(4)?,
                        created_at: None,
                        test: None,
                        last_seen_at: None,
                        online: None,
                        avatar_color_index_effective: None,
                    })
                },
            )
            .optional()
        })
    }

    /// Public profile fields for a user id (for member lists).
    pub fn user_card(&self, user_id: &str) -> Result<Option<UserCardDto>> {
        self.with(|c| {
            let card = c
                .query_row(
                    "SELECT id, login, avatar_url, name, status, created_at FROM users WHERE id = ?1",
                    params![user_id],
                    |r| {
                        Ok(UserCardDto {
                            id: r.get::<_, String>(0)?,
                            handle: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                            avatar_url: r.get::<_, Option<String>>(2)?,
                            name: r.get::<_, Option<String>>(3)?,
                            status: r.get::<_, String>(4)?,
                            created_at: Some(r.get::<_, i64>(5)?),
                            test: None,
                            last_seen_at: None,
                            online: None,
                            avatar_color_index_effective: None,
                        })
                    },
                )
                .optional()?;
            let Some(mut card) = card else {
                return Ok(None);
            };
            // Legacy device-token accounts have no GitHub login: show them by the name of the
            // device they used most recently (stable, and what their owner recognises).
            if card.handle.is_empty() {
                let device_name: Option<String> = c
                    .query_row(
                        "SELECT name FROM devices WHERE user_id = ?1 AND revoked_at IS NULL
                         ORDER BY COALESCE(last_seen_at, created_at) DESC LIMIT 1",
                        params![user_id],
                        |r| r.get(0),
                    )
                    .optional()?
                    .filter(|n: &String| !n.trim().is_empty());
                let fallback_name = device_name
                    .unwrap_or_else(|| user_id.trim_start_matches("u_").to_owned());
                card.handle = legacy_handle(&fallback_name);
                if card.name.as_deref().is_none_or(str::is_empty) {
                    card.name = Some(fallback_name);
                }
            }
            Ok(Some(card))
        })
    }
}

/// A handle-shaped slug for an account without a login: `"Jacob's Mac"` → `jacobs-mac`.
pub fn legacy_handle(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            dash = false;
        } else if ch == '\'' || ch == '\u{2019}' {
            // apostrophes vanish: "jacobs", not "jacob-s"
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    let out = out.trim_end_matches('-').to_owned();
    if out.is_empty() {
        "someone".to_owned()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    //! A legacy device-token account (no GitHub login) is shown by its device's name.

    use super::*;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    #[test]
    fn legacy_user_card_falls_back_to_the_device_name() {
        assert_eq!(legacy_handle("Jacob's Mac"), "jacobs-mac");
        assert_eq!(legacy_handle("  "), "someone");
        let state = test_state(AuthMode::Github);
        state
            .db
            .with(|c| {
                c.execute(
                    "INSERT INTO users(id, snapshot, spend_cap_usd, created_at) VALUES ('u_mac', NULL, 0, 1)",
                    [],
                )
            })
            .expect("legacy user");
        state
            .db
            .insert_device("dev-mac", "Jacob's Mac", "tok", "u_mac", "paircode")
            .expect("device");
        let card = state.db.user_card("u_mac").unwrap().unwrap();
        assert_eq!(card.handle, "jacobs-mac");
        assert_eq!(card.name.as_deref(), Some("Jacob's Mac"));
        let sender = crate::notify::Sender::user(&state, "u_mac");
        assert_eq!(sender.title(), "@jacobs-mac");
    }
}
