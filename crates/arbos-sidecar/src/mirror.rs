//! The local mirror: plain files under the data dir (spec §3). One `messages.jsonl` per chat,
//! append-only and idempotent on the row id, `members.json`, `brief.md`, `files/`; `chats.json`
//! and `state.json` at the root. The daemon writes; `mcp` and the CLI read.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::{safe_segment, Paths};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct State {
    #[serde(default)]
    pub agent: Value,
    #[serde(default)]
    pub relay: String,
    /// The inbox cursor: the id of the last event applied to the mirror.
    #[serde(default)]
    pub cursor: i64,
    #[serde(default)]
    pub updated: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChatMeta {
    pub id: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub last_at: i64,
    /// Wake rows with an event id above this have not been answered yet.
    #[serde(default)]
    pub answered_through: i64,
    #[serde(default)]
    pub backfilled: bool,
}

/// A row that asks the agent to act, as `apply_event` reports it to the waker.
#[derive(Debug, Clone)]
pub struct WakeCandidate {
    pub chat: ChatMeta,
    pub event_id: i64,
    pub ts: i64,
    pub from: Value,
    pub from_agent: bool,
    pub reason: &'static str,
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Clone)]
pub struct Mirror {
    pub paths: Paths,
}

impl Mirror {
    pub fn new(paths: Paths) -> Self {
        Self { paths }
    }

    fn read_json<T: for<'de> Deserialize<'de> + Default>(&self, p: &PathBuf) -> T {
        fs::read_to_string(p)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn write_json<T: Serialize>(&self, p: &PathBuf, v: &T) -> Result<()> {
        if let Some(d) = p.parent() {
            fs::create_dir_all(d)?;
        }
        let tmp = p.with_extension("tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(v)?)?;
        fs::rename(&tmp, p)?;
        Ok(())
    }

    // ---- state and chats --------------------------------------------------------------------

    pub fn state(&self) -> State {
        self.read_json(&self.paths.state_file())
    }

    pub fn save_state(&self, s: &State) -> Result<()> {
        self.write_json(&self.paths.state_file(), s)
    }

    pub fn chats(&self) -> Vec<ChatMeta> {
        self.read_json(&self.paths.chats_file())
    }

    pub fn chat(&self, id: &str) -> Option<ChatMeta> {
        self.chats().into_iter().find(|c| c.id == id)
    }

    pub fn upsert_chat(&self, meta: &ChatMeta) -> Result<()> {
        let mut all = self.chats();
        match all.iter_mut().find(|c| c.id == meta.id) {
            Some(c) => {
                if !meta.kind.is_empty() {
                    c.kind = meta.kind.clone();
                }
                if !meta.title.is_empty() {
                    c.title = meta.title.clone();
                }
                c.last_at = c.last_at.max(meta.last_at);
                c.answered_through = c.answered_through.max(meta.answered_through);
                c.backfilled |= meta.backfilled;
            }
            None => all.push(meta.clone()),
        }
        all.sort_by_key(|a| std::cmp::Reverse(a.last_at));
        self.write_json(&self.paths.chats_file(), &all)
    }

    pub fn remove_chat(&self, id: &str) -> Result<()> {
        let all: Vec<ChatMeta> = self.chats().into_iter().filter(|c| c.id != id).collect();
        self.write_json(&self.paths.chats_file(), &all)
    }

    /// The agent posted in `chat`: every wake row up to the current head counts as answered.
    pub fn mark_answered(&self, chat: &str, through_event: i64) -> Result<()> {
        if let Some(mut c) = self.chat(chat) {
            c.answered_through = c.answered_through.max(through_event);
            self.upsert_chat(&c)?;
        }
        Ok(())
    }

    // ---- rows -----------------------------------------------------------------------------------

    fn messages_file(&self, chat: &str) -> PathBuf {
        self.paths.chat_dir(chat).join("messages.jsonl")
    }

    pub fn rows(&self, chat: &str) -> Vec<Value> {
        let Ok(text) = fs::read_to_string(self.messages_file(chat)) else {
            return Vec::new();
        };
        text.lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .collect()
    }

    /// Append rows not yet present (by `id`). Returns how many were new.
    pub fn append_rows(&self, chat: &str, rows: &[Value]) -> Result<usize> {
        let dir = self.paths.chat_dir(chat);
        fs::create_dir_all(&dir)?;
        let seen: HashSet<String> = self
            .rows(chat)
            .iter()
            .filter_map(|r| r["id"].as_str().map(str::to_owned))
            .collect();
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.messages_file(chat))?;
        let mut n = 0;
        for r in rows {
            let Some(id) = r["id"].as_str() else { continue };
            if seen.contains(id) {
                continue;
            }
            f.write_all(serde_json::to_string(r)?.as_bytes())?;
            f.write_all(b"\n")?;
            n += 1;
        }
        Ok(n)
    }

    /// History rows go in front of what is already mirrored, oldest first, without duplicates.
    pub fn prepend_history(&self, chat: &str, history: &[Value]) -> Result<usize> {
        let existing = self.rows(chat);
        let have: HashSet<String> = existing
            .iter()
            .filter_map(|r| r["id"].as_str().map(str::to_owned))
            .collect();
        let fresh: Vec<&Value> = history
            .iter()
            .filter(|r| r["id"].as_str().is_some_and(|id| !have.contains(id)))
            .collect();
        if fresh.is_empty() {
            return Ok(0);
        }
        let dir = self.paths.chat_dir(chat);
        fs::create_dir_all(&dir)?;
        let tmp = dir.join("messages.jsonl.tmp");
        {
            let mut f = fs::File::create(&tmp)?;
            for r in fresh.iter().copied().chain(existing.iter()) {
                f.write_all(serde_json::to_string(r)?.as_bytes())?;
                f.write_all(b"\n")?;
            }
        }
        fs::rename(&tmp, self.messages_file(chat))?;
        Ok(fresh.len())
    }

    /// `limit` rows ending before `before` (exclusive), oldest first, plus whether older rows exist.
    pub fn read(&self, chat: &str, limit: usize, before: Option<&str>) -> (Vec<Value>, bool) {
        let rows = self.rows(chat);
        let end = before
            .and_then(|b| rows.iter().position(|r| r["id"] == b))
            .unwrap_or(rows.len());
        let start = end.saturating_sub(limit.max(1));
        (rows[start..end].to_vec(), start > 0)
    }

    pub fn search(&self, query: &str, chat: Option<&str>, limit: usize) -> Vec<Value> {
        let q = query.to_lowercase();
        let chats: Vec<ChatMeta> = match chat {
            Some(c) => self.chat(c).into_iter().collect(),
            None => self.chats(),
        };
        let mut hits = Vec::new();
        for c in chats {
            for r in self.rows(&c.id).into_iter().rev() {
                let text = r["text"].as_str().unwrap_or("").to_lowercase();
                let name = r["from"]["name"].as_str().unwrap_or("").to_lowercase();
                let handle = r["from"]["handle"].as_str().unwrap_or("").to_lowercase();
                if text.contains(&q) || name.contains(&q) || handle.contains(&q) {
                    hits.push(json!({ "chat": c.id, "title": c.title, "row": r }));
                    if hits.len() >= limit {
                        return hits;
                    }
                }
            }
        }
        hits.sort_by(|a, b| b["row"]["ts"].as_i64().cmp(&a["row"]["ts"].as_i64()));
        hits
    }

    // ---- members, brief, files --------------------------------------------------------------------

    pub fn members(&self, chat: &str) -> Value {
        self.read_json(&self.paths.chat_dir(chat).join("members.json"))
    }

    pub fn write_members(&self, chat: &str, members: &Value) -> Result<()> {
        let v = json!({ "fetched": now_ms(), "members": members });
        self.write_json(&self.paths.chat_dir(chat).join("members.json"), &v)
    }

    pub fn brief(&self, chat: &str) -> String {
        fs::read_to_string(self.paths.chat_dir(chat).join("brief.md")).unwrap_or_default()
    }

    /// Deterministic brief: counts + the last five rows (spec §3; a model summary is a follow-up).
    pub fn refresh_brief(&self, chat: &str) -> Result<()> {
        let meta = self.chat(chat).unwrap_or_default();
        let rows = self.rows(chat);
        let members = self.members(chat);
        let n_members = members["members"].as_array().map(|a| a.len()).unwrap_or(0);
        let unanswered = rows
            .iter()
            .filter(|r| {
                r["wake"] == true && r["event_id"].as_i64().unwrap_or(0) > meta.answered_through
            })
            .count();
        let mut out = format!(
            "{} ({}) · {} members · {} rows · {} unanswered\n\nLast 5:\n",
            if meta.title.is_empty() {
                chat
            } else {
                &meta.title
            },
            meta.kind,
            n_members,
            rows.len(),
            unanswered
        );
        for r in rows
            .iter()
            .rev()
            .take(5)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            let who = r["from"]["name"]
                .as_str()
                .filter(|s| !s.is_empty())
                .or_else(|| r["from"]["handle"].as_str())
                .unwrap_or("?");
            let text: String = r["text"].as_str().unwrap_or("").chars().take(200).collect();
            out.push_str(&format!("- {who}: {text}\n"));
        }
        let older = rows.len().saturating_sub(5);
        if older > 0 {
            out.push_str(&format!(
                "\n({older} older rows — arbos_read_chat before={})\n",
                rows[rows.len() - 5]["id"].as_str().unwrap_or("")
            ));
        }
        fs::create_dir_all(self.paths.chat_dir(chat))?;
        fs::write(self.paths.chat_dir(chat).join("brief.md"), out)?;
        Ok(())
    }

    pub fn file_path(&self, chat: &str, blob: &str) -> PathBuf {
        let key = blob.rsplit('/').next().unwrap_or(blob);
        self.paths
            .chat_dir(chat)
            .join("files")
            .join(safe_segment(key))
    }

    pub fn save_file(&self, chat: &str, blob: &str, bytes: &[u8]) -> Result<PathBuf> {
        let p = self.file_path(chat, blob);
        fs::create_dir_all(p.parent().unwrap())?;
        fs::write(&p, bytes)?;
        Ok(p)
    }

    // ---- pending and wakes ------------------------------------------------------------------

    /// Wake rows the agent has not answered, per chat, freshest chats first. Rows older than
    /// `stale_ms` are left out (Buzz L4).
    pub fn pending(&self, stale_ms: i64) -> Vec<Value> {
        let now = now_ms();
        let mut out = Vec::new();
        for c in self.chats() {
            let rows: Vec<Value> = self
                .rows(&c.id)
                .into_iter()
                .filter(|r| {
                    r["wake"] == true
                        && r["event_id"].as_i64().unwrap_or(0) > c.answered_through
                        && now - r["ts"].as_i64().unwrap_or(now) <= stale_ms
                })
                .collect();
            if !rows.is_empty() {
                out.push(json!({ "chat": c.id, "title": c.title, "kind": c.kind, "rows": rows }));
            }
        }
        out
    }

    pub fn append_wake(&self, record: &Value) -> Result<()> {
        fs::create_dir_all(&self.paths.data_dir)?;
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.paths.wakes_file())?;
        f.write_all(serde_json::to_string(record)?.as_bytes())?;
        f.write_all(b"\n")?;
        Ok(())
    }

    pub fn last_wake(&self) -> Option<Value> {
        let text = fs::read_to_string(self.paths.wakes_file()).ok()?;
        text.lines()
            .rev()
            .find_map(|l| serde_json::from_str::<Value>(l).ok())
    }

    // ---- events ---------------------------------------------------------------------------------

    /// Apply one inbox event. Returns a wake candidate when the event asks the agent to act.
    /// Idempotent: a row already mirrored is not written twice and does not wake again.
    pub fn apply_event(&self, e: &Value) -> Result<Option<WakeCandidate>> {
        let ty = e["type"].as_str().unwrap_or("");
        let event_id = e["id"].as_i64().unwrap_or(0);
        let ts = e["ts"].as_i64().unwrap_or_else(now_ms);
        let chat_id = e["chat"]["id"].as_str().unwrap_or("").to_owned();
        if chat_id.is_empty() {
            return Ok(None);
        }
        let mut meta = ChatMeta {
            id: chat_id.clone(),
            kind: e["chat"]["kind"].as_str().unwrap_or("").to_owned(),
            title: e["chat"]["title"].as_str().unwrap_or("").to_owned(),
            last_at: ts,
            ..Default::default()
        };
        match ty {
            "message.received" => {
                let m = &e["message"];
                let wake = e["wake"] == true;
                let row = json!({
                    "id": m["id"],
                    "ts": ts,
                    "from": m["from"],
                    "text": m["text"],
                    "reply_to": m["reply_to"],
                    "mentions": m["mentions"],
                    "attachments": m["attachments"],
                    "wake": wake,
                    "event_id": event_id,
                });
                let new = self.append_rows(&chat_id, &[row])?;
                // Context rows the relay attached (context=<n>) are history we may lack.
                if let Some(recent) = e["context"]["recent"].as_array() {
                    let rows: Vec<Value> = recent
                        .iter()
                        .map(|r| {
                            let mut r = r.clone();
                            r["wake"] = json!(false);
                            r["event_id"] = json!(0);
                            r
                        })
                        .collect();
                    let _ = self.prepend_history(&chat_id, &rows);
                }
                if let Some(members) = e["context"].get("members") {
                    let _ = self.write_members(&chat_id, members);
                }
                let known = self.chat(&chat_id);
                if let Some(k) = &known {
                    meta.backfilled = k.backfilled;
                }
                self.upsert_chat(&meta)?;
                let _ = self.refresh_brief(&chat_id);
                if new == 0 || !wake {
                    return Ok(None);
                }
                let from_agent = m["from"]["kind"] == "agent";
                Ok(Some(WakeCandidate {
                    chat: self.chat(&chat_id).unwrap_or(meta),
                    event_id,
                    ts,
                    from: m["from"].clone(),
                    from_agent,
                    reason: "message",
                }))
            }
            "member.added" => {
                self.upsert_chat(&meta)?;
                Ok(Some(WakeCandidate {
                    chat: self.chat(&chat_id).unwrap_or(meta),
                    event_id,
                    ts,
                    from: Value::Null,
                    from_agent: false,
                    reason: "member.added",
                }))
            }
            "member.removed" => {
                self.remove_chat(&chat_id)?;
                Ok(None)
            }
            _ => Ok(None),
        }
    }
}

/// Convert a #2182 history row into the mirror row shape.
pub fn history_row(r: &Value) -> Value {
    json!({
        "id": r["id"],
        "ts": r["ts"],
        "from": r["from"],
        "text": r["text"],
        "reply_to": r["reply_to"],
        "mentions": r["mentions"],
        "attachments": r["attachments"],
        "wake": false,
        "event_id": 0,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn temp_mirror() -> (tempfile::TempDir, Mirror) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config_dir: dir.path().join("c"),
            data_dir: dir.path().join("d"),
        };
        paths.ensure().unwrap();
        (dir, Mirror::new(paths))
    }

    pub(crate) fn received(id: i64, chat: &str, msg: &str, text: &str, wake: bool) -> Value {
        json!({
            "id": id, "type": "message.received", "ts": now_ms(),
            "chat": { "id": chat, "kind": "group", "title": "Launch week" },
            "message": { "id": msg, "text": text,
                         "from": { "kind": "user", "id": "u1", "name": "Jacob", "handle": "unconst" },
                         "reply_to": null, "mentions": [] },
            "wake": wake,
        })
    }

    /// Buzz L1: the same event applied twice leaves one row and wakes once.
    #[test]
    fn duplicate_event_id_is_a_noop() {
        let (_d, m) = temp_mirror();
        let e = received(7, "c1", "m1", "@bot hi", true);
        assert!(m.apply_event(&e).unwrap().is_some());
        assert!(m.apply_event(&e).unwrap().is_none());
        assert_eq!(m.rows("c1").len(), 1);
        assert_eq!(m.chats().len(), 1);
    }

    #[test]
    fn pending_and_answered() {
        let (_d, m) = temp_mirror();
        m.apply_event(&received(1, "c1", "m1", "hello", false))
            .unwrap();
        m.apply_event(&received(2, "c1", "m2", "@bot one", true))
            .unwrap();
        m.apply_event(&received(3, "c1", "m3", "@bot two", true))
            .unwrap();
        let p = m.pending(i64::MAX);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0]["rows"].as_array().unwrap().len(), 2);
        m.mark_answered("c1", 3).unwrap();
        assert!(m.pending(i64::MAX).is_empty());
        // Stale rows are not pending.
        let mut old = received(4, "c1", "m4", "@bot old", true);
        old["ts"] = json!(now_ms() - 10 * 3600 * 1000);
        m.apply_event(&old).unwrap();
        assert!(m.pending(6 * 3600 * 1000).is_empty());
        assert_eq!(m.pending(i64::MAX).len(), 1);
    }

    #[test]
    fn history_goes_in_front_and_read_marks_truncation() {
        let (_d, m) = temp_mirror();
        m.apply_event(&received(5, "c1", "m5", "newest", false))
            .unwrap();
        let hist: Vec<Value> = (1..5)
            .map(|i| {
                history_row(
                    &json!({ "id": format!("m{i}"), "ts": i, "text": format!("row {i}"),
                    "from": { "kind": "user", "name": "Ken" }, "reply_to": null, "mentions": [] }),
                )
            })
            .collect();
        assert_eq!(m.prepend_history("c1", &hist).unwrap(), 4);
        assert_eq!(m.prepend_history("c1", &hist).unwrap(), 0);
        let rows = m.rows("c1");
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0]["id"], "m1");
        assert_eq!(rows[4]["id"], "m5");
        let (page, more) = m.read("c1", 2, None);
        assert_eq!(page.len(), 2);
        assert!(more);
        let (page, more) = m.read("c1", 10, Some("m3"));
        assert_eq!(page.len(), 2);
        assert!(!more);
        m.refresh_brief("c1").unwrap();
        assert!(m.brief("c1").contains("5 rows"));
        let hits = m.search("row 2", None, 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["row"]["id"], "m2");
    }

    #[test]
    fn member_events_add_and_remove_chats() {
        let (_d, m) = temp_mirror();
        let added = json!({ "id": 9, "type": "member.added", "ts": now_ms(),
            "chat": { "id": "g1", "kind": "group", "title": "Ops" } });
        let w = m.apply_event(&added).unwrap().unwrap();
        assert_eq!(w.reason, "member.added");
        assert_eq!(m.chats().len(), 1);
        let removed = json!({ "id": 10, "type": "member.removed", "ts": now_ms(),
            "chat": { "id": "g1", "kind": "group", "title": "Ops" } });
        assert!(m.apply_event(&removed).unwrap().is_none());
        assert!(m.chats().is_empty());
    }
}
