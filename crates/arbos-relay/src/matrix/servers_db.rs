//! Storage for servers and channels (D88 C1). A **server** is a Space on the homeserver and a
//! row here; a **channel** is a room inside it. The relay keeps this registry for the API, the
//! ACL and invite links; the messages live only in the rooms (no Loro doc, design §4 stage C).

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use arbos_core::ids::now_ms;

use crate::db::Db;

pub const SERVERS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS servers (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, owner_user_id TEXT NOT NULL, image_key TEXT,
  invite_code TEXT NOT NULL UNIQUE, space_room_id TEXT, created_at INTEGER NOT NULL, deleted_at INTEGER,
  is_open INTEGER NOT NULL DEFAULT 0, avatar_leaf INTEGER, avatar_leaf_color INTEGER
);
CREATE TABLE IF NOT EXISTS server_members (
  server_id TEXT NOT NULL, user_id TEXT NOT NULL, role TEXT NOT NULL, added_by TEXT NOT NULL,
  joined_at INTEGER NOT NULL, left_at INTEGER, banned_at INTEGER,
  PRIMARY KEY (server_id, user_id)
);
CREATE INDEX IF NOT EXISTS server_members_user ON server_members(user_id);
CREATE TABLE IF NOT EXISTS channels (
  id TEXT PRIMARY KEY, server_id TEXT NOT NULL, name TEXT NOT NULL, topic TEXT NOT NULL DEFAULT '',
  restricted INTEGER NOT NULL DEFAULT 0, position INTEGER NOT NULL DEFAULT 0, room_id TEXT,
  created_at INTEGER NOT NULL, deleted_at INTEGER
);
CREATE INDEX IF NOT EXISTS channels_server ON channels(server_id, position);
CREATE TABLE IF NOT EXISTS server_invites (
  id TEXT PRIMARY KEY, server_id TEXT NOT NULL, code TEXT NOT NULL UNIQUE, created_by TEXT NOT NULL,
  created_at INTEGER NOT NULL, expires_at INTEGER, uses_left INTEGER, revoked_at INTEGER
);
CREATE INDEX IF NOT EXISTS server_invites_server ON server_invites(server_id, created_at);
"#;

pub fn migrate_servers(c: &Connection) {
    // Invite links are rows now (several per server, expiry, limited uses — Jacob 2026-10-22):
    // every server's single code becomes one unlimited row so existing links keep working.
    let _ = c.execute(
        "INSERT OR IGNORE INTO server_invites(id, server_id, code, created_by, created_at)
         SELECT 'legacy-' || id, id, invite_code, owner_user_id, created_at FROM servers WHERE deleted_at IS NULL",
        [],
    );
    // A channel's display name never carries '#': the glyph shows it (#1317).
    let _ = c.execute(
        "UPDATE channels SET name = replace(name, '#', '') WHERE name LIKE '%#%'",
        [],
    );
    let _ = c.execute(
        "ALTER TABLE servers ADD COLUMN is_open INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = c.execute("ALTER TABLE servers ADD COLUMN avatar_leaf INTEGER", []);
    let _ = c.execute(
        "ALTER TABLE servers ADD COLUMN avatar_leaf_color INTEGER",
        [],
    );
    let _ = c.execute_batch(
        "CREATE INDEX IF NOT EXISTS servers_open_name ON servers(is_open, name) WHERE deleted_at IS NULL;",
    );
}

#[derive(Debug, Clone, Serialize)]
pub struct Server {
    pub id: String,
    pub name: String,
    pub owner_user_id: String,
    pub image_key: Option<String>,
    pub invite_code: String,
    pub space_room_id: Option<String>,
    pub created_at: i64,
    /// Anyone on Arbos can find and join without an invite link.
    pub is_open: bool,
    /// D92 amendment: the server's leaf picture (0–8) and disc colour (0–9) from the Create
    /// screen; `None` = initials. An `image_key` picture wins.
    pub avatar_leaf: Option<i64>,
    pub avatar_leaf_color: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerMember {
    pub server_id: String,
    pub user_id: String,
    /// `owner` | `admin` | `member`.
    pub role: String,
    pub joined_at: i64,
    pub banned_at: Option<i64>,
}

impl ServerMember {
    pub fn can_admin(&self) -> bool {
        matches!(self.role.as_str(), "owner" | "admin")
    }
}

/// One invite link of a server. `expires_at` / `uses_left` are `None` for a link without a
/// limit; `revoked_at` ends it early. `servers.invite_code` mirrors the newest live unlimited
/// code for older clients.
#[derive(Debug, Clone, Serialize)]
pub struct ServerInvite {
    pub id: String,
    pub server_id: String,
    pub code: String,
    pub created_by: String,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub uses_left: Option<i64>,
    pub revoked_at: Option<i64>,
}

impl ServerInvite {
    pub fn usable(&self, now: i64) -> bool {
        self.revoked_at.is_none()
            && self.expires_at.is_none_or(|t| t > now)
            && self.uses_left.is_none_or(|n| n > 0)
    }
}

fn invite_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ServerInvite> {
    Ok(ServerInvite {
        id: r.get(0)?,
        server_id: r.get(1)?,
        code: r.get(2)?,
        created_by: r.get(3)?,
        created_at: r.get(4)?,
        expires_at: r.get(5)?,
        uses_left: r.get(6)?,
        revoked_at: r.get(7)?,
    })
}

const INVITE_COLS: &str =
    "id, server_id, code, created_by, created_at, expires_at, uses_left, revoked_at";

#[derive(Debug, Clone, Serialize)]
pub struct Channel {
    pub id: String,
    pub server_id: String,
    pub name: String,
    pub topic: String,
    /// Only owner/admins post (`#announcements`); shown with a lock.
    pub restricted: bool,
    pub position: i64,
    pub room_id: Option<String>,
    pub created_at: i64,
}

fn server_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Server> {
    Ok(Server {
        id: r.get(0)?,
        name: r.get(1)?,
        owner_user_id: r.get(2)?,
        image_key: r.get(3)?,
        invite_code: r.get(4)?,
        space_room_id: r.get(5)?,
        created_at: r.get(6)?,
        is_open: r.get::<_, i64>(7)? != 0,
        avatar_leaf: r.get(8)?,
        avatar_leaf_color: r.get(9)?,
    })
}

const SERVER_COLS: &str = "id, name, owner_user_id, image_key, invite_code, space_room_id, created_at, is_open, avatar_leaf, avatar_leaf_color";

fn channel_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Channel> {
    Ok(Channel {
        id: r.get(0)?,
        server_id: r.get(1)?,
        name: r.get(2)?,
        topic: r.get(3)?,
        restricted: r.get::<_, i64>(4)? != 0,
        position: r.get(5)?,
        room_id: r.get(6)?,
        created_at: r.get(7)?,
    })
}

const CHANNEL_COLS: &str = "id, server_id, name, topic, restricted, position, room_id, created_at";

fn member_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ServerMember> {
    Ok(ServerMember {
        server_id: r.get(0)?,
        user_id: r.get(1)?,
        role: r.get(2)?,
        joined_at: r.get(3)?,
        banned_at: r.get(4)?,
    })
}

impl Db {
    pub fn insert_server(&self, s: &Server) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO servers(id, name, owner_user_id, image_key, invite_code, space_room_id, created_at, is_open, avatar_leaf, avatar_leaf_color)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    s.id,
                    s.name,
                    s.owner_user_id,
                    s.image_key,
                    s.invite_code,
                    s.space_room_id,
                    s.created_at,
                    if s.is_open { 1 } else { 0 },
                    s.avatar_leaf,
                    s.avatar_leaf_color
                ],
            )?;
            c.execute(
                "INSERT INTO server_members(server_id, user_id, role, added_by, joined_at) VALUES (?1, ?2, 'owner', ?2, ?3)",
                params![s.id, s.owner_user_id, s.created_at],
            )?;
            Ok(())
        })
    }

    pub fn server(&self, id: &str) -> Result<Option<Server>> {
        self.with(|c| {
            c.query_row(
                &format!("SELECT {SERVER_COLS} FROM servers WHERE id = ?1 AND deleted_at IS NULL"),
                params![id],
                server_from_row,
            )
            .optional()
        })
    }

    pub fn servers_of_user(&self, user_id: &str) -> Result<Vec<(Server, ServerMember)>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {SERVER_COLS}, m.server_id, m.user_id, m.role, m.joined_at, m.banned_at
                 FROM servers s JOIN server_members m ON m.server_id = s.id
                 WHERE m.user_id = ?1 AND m.left_at IS NULL AND m.banned_at IS NULL AND s.deleted_at IS NULL
                 ORDER BY s.created_at"
            ))?;
            let rows = st.query_map(params![user_id], |r| {
                let s = server_from_row(r)?;
                let m = ServerMember {
                    server_id: r.get(10)?,
                    user_id: r.get(11)?,
                    role: r.get(12)?,
                    joined_at: r.get(13)?,
                    banned_at: r.get(14)?,
                };
                Ok((s, m))
            })?;
            rows.collect()
        })
    }

    /// Open servers whose name contains `q` (case-insensitive), newest first.
    pub fn search_open_servers(&self, q: &str, limit: i64) -> Result<Vec<(Server, i64)>> {
        let needle = q.trim().to_lowercase();
        if needle.is_empty() {
            return Ok(Vec::new());
        }
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {SERVER_COLS},
                        (SELECT COUNT(*) FROM server_members m
                         WHERE m.server_id = s.id AND m.left_at IS NULL AND m.banned_at IS NULL)
                 FROM servers s
                 WHERE s.is_open = 1 AND s.deleted_at IS NULL
                   AND lower(s.name) LIKE '%' || ?1 || '%'
                 ORDER BY s.created_at DESC
                 LIMIT ?2"
            ))?;
            let rows = st.query_map(params![needle, limit], |r| {
                let s = server_from_row(r)?;
                let count: i64 = r.get(10)?;
                Ok((s, count))
            })?;
            rows.collect()
        })
    }

    pub fn update_server(
        &self,
        id: &str,
        name: Option<&str>,
        image_key: Option<Option<&str>>,
        is_open: Option<bool>,
    ) -> Result<bool> {
        self.with(|c| {
            let mut n = 0;
            if let Some(name) = name {
                n += c.execute(
                    "UPDATE servers SET name = ?2 WHERE id = ?1",
                    params![id, name],
                )?;
            }
            if let Some(image) = image_key {
                n += c.execute(
                    "UPDATE servers SET image_key = ?2 WHERE id = ?1",
                    params![id, image],
                )?;
            }
            if let Some(open) = is_open {
                n += c.execute(
                    "UPDATE servers SET is_open = ?2 WHERE id = ?1",
                    params![id, if open { 1 } else { 0 }],
                )?;
            }
            Ok(n > 0)
        })
    }

    /// D92 amendment: the server's leaf picture (`None` = none).
    pub fn update_server_leaf(&self, id: &str, leaf: Option<(i64, i64)>) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE servers SET avatar_leaf = ?2, avatar_leaf_color = ?3 WHERE id = ?1",
                params![id, leaf.map(|l| l.0), leaf.map(|l| l.1)],
            )?;
            Ok(())
        })
    }

    pub fn set_server_space(&self, id: &str, room_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE servers SET space_room_id = ?2 WHERE id = ?1",
                params![id, room_id],
            )?;
            Ok(())
        })
    }

    // ---- invites ----------------------------------------------------------------------------

    pub fn insert_server_invite(&self, inv: &ServerInvite) -> Result<()> {
        self.with(|c| {
            c.execute(
                &format!("INSERT INTO server_invites({INVITE_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"),
                params![
                    inv.id,
                    inv.server_id,
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

    /// Live invites of a server (not revoked), newest first; expired / used-up ones included
    /// so the owner sees why a link stopped working.
    pub fn invites_of_server(&self, server_id: &str) -> Result<Vec<ServerInvite>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {INVITE_COLS} FROM server_invites WHERE server_id = ?1 AND revoked_at IS NULL ORDER BY created_at DESC"
            ))?;
            let rows = st.query_map(params![server_id], invite_from_row)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
    }

    pub fn invite_by_code(&self, code: &str) -> Result<Option<ServerInvite>> {
        self.with(|c| {
            c.query_row(
                &format!("SELECT {INVITE_COLS} FROM server_invites WHERE code = ?1"),
                params![code],
                invite_from_row,
            )
            .optional()
        })
    }

    pub fn revoke_server_invite(&self, server_id: &str, invite_id: &str) -> Result<bool> {
        self.with(|c| {
            Ok(c.execute(
                "UPDATE server_invites SET revoked_at = ?3 WHERE server_id = ?1 AND id = ?2 AND revoked_at IS NULL",
                params![server_id, invite_id, now_ms()],
            )? > 0)
        })
    }

    pub fn revoke_all_invites(&self, server_id: &str) -> Result<usize> {
        self.with(|c| {
            c.execute(
                "UPDATE server_invites SET revoked_at = ?2 WHERE server_id = ?1 AND revoked_at IS NULL",
                params![server_id, now_ms()],
            )
        })
    }

    /// One use of a limited link; `false` when it had none left.
    pub fn consume_invite_use(&self, invite_id: &str) -> Result<bool> {
        self.with(|c| {
            Ok(c.execute(
                "UPDATE server_invites SET uses_left = uses_left - 1 WHERE id = ?1 AND (uses_left IS NULL OR uses_left > 0)",
                params![invite_id],
            )? > 0)
        })
    }

    pub fn set_server_invite(&self, id: &str, code: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE servers SET invite_code = ?2 WHERE id = ?1",
                params![id, code],
            )?;
            Ok(())
        })
    }

    pub fn delete_server(&self, id: &str) -> Result<bool> {
        self.with(|c| {
            Ok(c.execute(
                "UPDATE servers SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                params![id, now_ms()],
            )? > 0)
        })
    }

    // ---- members --------------------------------------------------------------------------------

    pub fn server_member(&self, server_id: &str, user_id: &str) -> Result<Option<ServerMember>> {
        self.with(|c| {
            c.query_row(
                "SELECT server_id, user_id, role, joined_at, banned_at FROM server_members
                 WHERE server_id = ?1 AND user_id = ?2 AND left_at IS NULL",
                params![server_id, user_id],
                member_from_row,
            )
            .optional()
        })
    }

    pub fn server_members(&self, server_id: &str) -> Result<Vec<ServerMember>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT server_id, user_id, role, joined_at, banned_at FROM server_members
                 WHERE server_id = ?1 AND left_at IS NULL AND banned_at IS NULL
                 ORDER BY CASE role WHEN 'owner' THEN 0 WHEN 'admin' THEN 1 ELSE 2 END, joined_at",
            )?;
            let rows = st.query_map(params![server_id], member_from_row)?;
            rows.collect()
        })
    }

    /// Join (or re-join) as `member`; `false` when banned.
    pub fn add_server_member(
        &self,
        server_id: &str,
        user_id: &str,
        added_by: &str,
    ) -> Result<bool> {
        self.with(|c| {
            let banned: Option<i64> = c
                .query_row(
                    "SELECT banned_at FROM server_members WHERE server_id = ?1 AND user_id = ?2",
                    params![server_id, user_id],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            if banned.is_some() {
                return Ok(false);
            }
            c.execute(
                "INSERT INTO server_members(server_id, user_id, role, added_by, joined_at)
                 VALUES (?1, ?2, 'member', ?3, ?4)
                 ON CONFLICT(server_id, user_id) DO UPDATE SET left_at = NULL, joined_at = excluded.joined_at",
                params![server_id, user_id, added_by, now_ms()],
            )?;
            Ok(true)
        })
    }

    pub fn set_server_role(&self, server_id: &str, user_id: &str, role: &str) -> Result<bool> {
        self.with(|c| {
            Ok(c.execute(
                "UPDATE server_members SET role = ?3 WHERE server_id = ?1 AND user_id = ?2 AND left_at IS NULL",
                params![server_id, user_id, role],
            )? > 0)
        })
    }

    pub fn remove_server_member(&self, server_id: &str, user_id: &str, ban: bool) -> Result<bool> {
        self.with(|c| {
            let now = now_ms();
            Ok(c.execute(
                "UPDATE server_members SET left_at = ?3, banned_at = CASE WHEN ?4 THEN ?3 ELSE banned_at END
                 WHERE server_id = ?1 AND user_id = ?2 AND left_at IS NULL",
                params![server_id, user_id, now, ban],
            )? > 0)
        })
    }

    // ---- channels -------------------------------------------------------------------------------

    pub fn insert_channel(&self, ch: &Channel) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO channels(id, server_id, name, topic, restricted, position, room_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![ch.id, ch.server_id, ch.name, ch.topic, ch.restricted as i64, ch.position, ch.room_id, ch.created_at],
            )?;
            Ok(())
        })
    }

    pub fn channel(&self, id: &str) -> Result<Option<Channel>> {
        self.with(|c| {
            c.query_row(
                &format!(
                    "SELECT {CHANNEL_COLS} FROM channels WHERE id = ?1 AND deleted_at IS NULL"
                ),
                params![id],
                channel_from_row,
            )
            .optional()
        })
    }

    /// The channel behind a homeserver room id (the push gateway's way in).
    /// The server a channel belongs to, deleted channels included: a channel's room is closed
    /// after the channel row is marked deleted, and the close still needs its members.
    pub fn channel_server_id(&self, channel_id: &str) -> Result<Option<String>> {
        self.with(|c| {
            c.query_row(
                "SELECT server_id FROM channels WHERE id = ?1",
                params![channel_id],
                |r| r.get(0),
            )
            .optional()
        })
    }

    pub fn channel_by_room_id(&self, room_id: &str) -> Result<Option<Channel>> {
        self.with(|c| {
            c.query_row(
                &format!(
                    "SELECT {CHANNEL_COLS} FROM channels WHERE room_id = ?1 AND deleted_at IS NULL"
                ),
                params![room_id],
                channel_from_row,
            )
            .optional()
        })
    }

    pub fn channels_of_server(&self, server_id: &str) -> Result<Vec<Channel>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {CHANNEL_COLS} FROM channels WHERE server_id = ?1 AND deleted_at IS NULL ORDER BY position, created_at"
            ))?;
            let rows = st.query_map(params![server_id], channel_from_row)?;
            rows.collect()
        })
    }

    pub fn update_channel(
        &self,
        id: &str,
        name: Option<&str>,
        topic: Option<&str>,
        restricted: Option<bool>,
    ) -> Result<bool> {
        self.with(|c| {
            let mut n = 0;
            if let Some(v) = name {
                n += c.execute(
                    "UPDATE channels SET name = ?2 WHERE id = ?1",
                    params![id, v],
                )?;
            }
            if let Some(v) = topic {
                n += c.execute(
                    "UPDATE channels SET topic = ?2 WHERE id = ?1",
                    params![id, v],
                )?;
            }
            if let Some(v) = restricted {
                n += c.execute(
                    "UPDATE channels SET restricted = ?2 WHERE id = ?1",
                    params![id, v as i64],
                )?;
            }
            Ok(n > 0)
        })
    }

    pub fn set_channel_room(&self, id: &str, room_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE channels SET room_id = ?2 WHERE id = ?1",
                params![id, room_id],
            )?;
            Ok(())
        })
    }

    pub fn delete_channel(&self, id: &str) -> Result<bool> {
        self.with(|c| {
            Ok(c.execute(
                "UPDATE channels SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                params![id, now_ms()],
            )? > 0)
        })
    }

    pub fn next_channel_position(&self, server_id: &str) -> Result<i64> {
        self.with(|c| {
            c.query_row(
                "SELECT COALESCE(MAX(position), -1) + 1 FROM channels WHERE server_id = ?1 AND deleted_at IS NULL",
                params![server_id],
                |r| r.get(0),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbos_core::ids::ulid;

    fn db() -> Db {
        let dir = std::env::temp_dir().join(format!("arbos-servers-db-{}", ulid()));
        Db::open(&dir.join("relay.db")).expect("db")
    }

    #[test]
    fn servers_members_channels() {
        let d = db();
        let s = Server {
            id: "S1".into(),
            name: "Launch Crew".into(),
            owner_user_id: "gh_1".into(),
            image_key: None,
            invite_code: "abc".into(),
            space_room_id: None,
            created_at: 1,
            is_open: false,
            avatar_leaf: None,
            avatar_leaf_color: None,
        };
        d.insert_server(&s).unwrap();
        assert_eq!(d.server("S1").unwrap().unwrap().name, "Launch Crew");
        // Invite links are rows: unlimited, limited-use, expiring, revoked.
        let inv = ServerInvite {
            id: "I1".into(),
            server_id: "S1".into(),
            code: "abc".into(),
            created_by: "gh_1".into(),
            created_at: 1,
            expires_at: None,
            uses_left: Some(1),
            revoked_at: None,
        };
        d.insert_server_invite(&inv).unwrap();
        let got = d.invite_by_code("abc").unwrap().unwrap();
        assert_eq!(got.server_id, "S1");
        assert!(got.usable(5));
        assert!(d.consume_invite_use("I1").unwrap());
        assert!(!d.consume_invite_use("I1").unwrap(), "one use only");
        assert!(!d.invite_by_code("abc").unwrap().unwrap().usable(5));
        let expiring = ServerInvite {
            id: "I2".into(),
            code: "exp".into(),
            uses_left: None,
            expires_at: Some(10),
            ..inv.clone()
        };
        d.insert_server_invite(&expiring).unwrap();
        assert!(d.invite_by_code("exp").unwrap().unwrap().usable(9));
        assert!(!d.invite_by_code("exp").unwrap().unwrap().usable(11));
        assert_eq!(d.invites_of_server("S1").unwrap().len(), 2);
        assert!(d.revoke_server_invite("S1", "I2").unwrap());
        assert_eq!(d.invites_of_server("S1").unwrap().len(), 1);
        assert!(!d.invite_by_code("exp").unwrap().unwrap().usable(5));
        assert!(d.add_server_member("S1", "gh_2", "gh_1").unwrap());
        assert!(d.set_server_role("S1", "gh_2", "admin").unwrap());
        assert_eq!(d.servers_of_user("gh_2").unwrap()[0].1.role, "admin");
        assert!(d.remove_server_member("S1", "gh_2", true).unwrap());
        assert!(
            !d.add_server_member("S1", "gh_2", "gh_1").unwrap(),
            "banned stays out"
        );
        assert_eq!(d.server_members("S1").unwrap().len(), 1);
        for (i, name) in ["general", "announcements"].iter().enumerate() {
            d.insert_channel(&Channel {
                id: format!("C{i}"),
                server_id: "S1".into(),
                name: (*name).into(),
                topic: String::new(),
                restricted: i == 1,
                position: d.next_channel_position("S1").unwrap(),
                room_id: None,
                created_at: 1,
            })
            .unwrap();
        }
        let chans = d.channels_of_server("S1").unwrap();
        assert_eq!(chans.len(), 2);
        assert!(chans[1].restricted);
        assert!(d.update_channel("C0", Some("lobby"), None, None).unwrap());
        assert_eq!(d.channel("C0").unwrap().unwrap().name, "lobby");
        assert!(d.delete_channel("C1").unwrap());
        assert_eq!(d.channels_of_server("S1").unwrap().len(), 1);
        d.set_server_invite("S1", "xyz").unwrap();
        assert_eq!(d.server("S1").unwrap().unwrap().invite_code, "xyz");
        assert_eq!(d.revoke_all_invites("S1").unwrap(), 1);
        assert!(d.invites_of_server("S1").unwrap().is_empty());
        assert!(d.update_server("S1", None, None, Some(true)).unwrap());
        assert!(d.server("S1").unwrap().unwrap().is_open);
        let hits = d.search_open_servers("crew", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0.id, "S1");
        assert!(d.delete_server("S1").unwrap());
        assert!(d.server("S1").unwrap().is_none());
    }
}
