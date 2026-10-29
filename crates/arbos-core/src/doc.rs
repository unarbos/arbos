//! The chat doc: one Loro document per chat (crdt-sync §6.2). Text bodies
//! are LoroText and are only appended while streaming; the `commands` list
//! is the client→box outbox. The relay never sees this module: it stores
//! opaque bytes.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use loro::event::DiffEvent;
pub use loro::VersionVector;
use loro::{
    Container, ExportMode, Index, LoroDoc, LoroList, LoroMap, LoroText, LoroValue, ValueOrContainer,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::approval::{ApprovalRequest, ApprovalState};
use crate::ids::{now_ms, ulid};
use crate::widget::QuestionState;

pub const SCHEMA_VERSION: i64 = 1;

/// Error from a client chat-doc writer (UniFFI-friendly owned message).
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("{0}")]
    Message(String),
}

impl CoreError {
    pub fn msg(s: impl Into<String>) -> Self {
        Self::Message(s.into())
    }
}

pub type CoreResult<T> = Result<T, CoreError>;

fn loro_err<T, E: std::fmt::Display>(r: Result<T, E>) -> CoreResult<T> {
    r.map_err(|e| CoreError::msg(e.to_string()))
}

/// What a root-level diff touched, mapped to message indexes.
#[derive(Debug, Default)]
pub struct Touched {
    /// The `messages` list itself changed (insert/delete): re-read everything.
    pub list_changed: bool,
    pub messages: BTreeSet<usize>,
    /// Anything outside `messages` (project doc maps, `commands`, `meta`, …).
    /// Mirrors Swift `ChatDoc.Touched.other` so UniFFI clients lose nothing.
    pub other: bool,
}

/// Map a root diff to message indexes. Paths look like
/// `[(messages, Key("messages")), (msg, Seq(i)), (parts, Key("parts")), ...]`.
pub fn touched(event: &DiffEvent<'_>) -> Touched {
    let mut out = Touched::default();
    for diff in &event.events {
        let mut saw_messages = false;
        let mut found = None;
        for (_, index) in diff.path.iter() {
            match index {
                Index::Key(k) if !saw_messages => {
                    if k.as_str() == "messages" {
                        saw_messages = true;
                    } else {
                        break;
                    }
                }
                Index::Seq(i) if saw_messages => {
                    found = Some(*i);
                    break;
                }
                Index::Key(_) | Index::Seq(_) | Index::Node(_) => {}
            }
        }
        if !saw_messages {
            // A diff whose target is the root `messages` list itself (empty path
            // to that container) is a list change; anything else is `other`.
            match diff.target {
                loro::ContainerID::Root { name, .. } if name.as_str() == "messages" => {
                    out.list_changed = true;
                }
                _ => out.other = true,
            }
            continue;
        }
        match found {
            Some(i) => {
                out.messages.insert(i);
            }
            None => out.list_changed = true,
        }
    }
    out
}

/// Reader projection: move `parts[*]` with `hidden: true` into `hiddenParts`
/// so the transcript never sees them while a details drawer still can.
/// Writers never create `hiddenParts`.
pub fn project_hidden_parts(json: &mut Value) {
    let Some(obj) = json.as_object_mut() else {
        return;
    };
    let Some(Value::Array(parts)) = obj.get_mut("parts") else {
        return;
    };
    let mut hidden = Vec::new();
    parts.retain(|p| {
        let is_hidden = p.get("hidden").and_then(Value::as_bool).unwrap_or(false);
        if is_hidden {
            hidden.push(p.clone());
        }
        !is_hidden
    });
    if !hidden.is_empty() {
        obj.insert("hiddenParts".into(), Value::Array(hidden));
    }
}

/// Separator in flat reaction keys: `"emoji|sender"` under `reactions`.
pub const REACTION_KEY_SEP: char = '|';

/// Fitzpatrick skin-tone modifiers (U+1F3FB … U+1F3FF). Stripped so
/// `👍🏽` and `👍🏿` count as the same reaction on a message.
const SKIN_TONE_LO: u32 = 0x1F3FB;
const SKIN_TONE_HI: u32 = 0x1F3FF;

/// Drop skin-tone modifiers from an emoji. Other combining marks (ZWJ,
/// variation selectors, hair/gender) stay. Empty input stays empty; if
/// stripping would leave nothing, the original string is kept.
pub fn normalize_reaction_emoji(emoji: &str) -> String {
    if emoji.is_empty() {
        return String::new();
    }
    let stripped: String = emoji
        .chars()
        .filter(|c| !(SKIN_TONE_LO..=SKIN_TONE_HI).contains(&(*c as u32)))
        .collect();
    if stripped.is_empty() {
        emoji.to_string()
    } else {
        stripped
    }
}

/// Build the CRDT key for one sender's hold of an emoji.
/// The emoji side is skin-tone-normalized so tone variants share one key.
pub fn reaction_key(emoji: &str, sender: &str) -> String {
    let base = normalize_reaction_emoji(emoji);
    format!("{base}{REACTION_KEY_SEP}{sender}")
}

/// Remove flat keys and legacy buckets that are skin-tone variants of `base`
/// for `sender`, so a message keeps one entry per base emoji per person.
fn clear_skin_tone_reaction_variants(reactions: &LoroMap, base: &str, sender: &str) {
    let raw = serde_json::to_value(reactions.get_deep_value()).unwrap_or(Value::Null);
    let Some(obj) = raw.as_object() else {
        return;
    };
    for key in obj.keys() {
        if let Some((emoji, sid)) = key.split_once(REACTION_KEY_SEP) {
            if sid == sender && emoji != base && normalize_reaction_emoji(emoji) == base {
                let _ = reactions.delete(key.as_str());
            }
            continue;
        }
        if key.contains(REACTION_KEY_SEP) {
            continue;
        }
        if key != base && normalize_reaction_emoji(key) == base {
            if let Some(ValueOrContainer::Container(Container::Map(bucket))) =
                reactions.get(key.as_str())
            {
                let _ = bucket.delete(sender);
            }
        }
    }
}

/// Concatenate text parts of a projected (or raw) message row JSON.
pub fn message_text(row: &Value) -> String {
    row.get("parts")
        .and_then(|p| p.as_array())
        .map(|parts| {
            parts
                .iter()
                .filter(|p| p.get("kind").and_then(|k| k.as_str()) == Some("text"))
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

/// Reader projection: expand flat `reactions` keys (`"emoji|sender" → true`)
/// into `emoji → { sender → true }` for UI / serde. Also folds legacy nested
/// emoji-bucket maps so both shapes read the same. Skin-tone variants of the
/// same base emoji merge under one bucket. Call before deserializing a
/// message row.
pub fn project_reactions(json: &mut Value) {
    let Some(obj) = json.as_object_mut() else {
        return;
    };
    let Some(Value::Object(raw)) = obj.get("reactions").cloned() else {
        return;
    };
    let mut nested = serde_json::Map::new();
    for (key, value) in raw {
        if let Value::Object(bucket) = value {
            // Legacy: emoji → { sender → true }.
            if key.contains(REACTION_KEY_SEP) {
                continue;
            }
            let base = normalize_reaction_emoji(&key);
            if base.is_empty() {
                continue;
            }
            let entry = nested
                .entry(base)
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            if let Value::Object(dst) = entry {
                for (sender, flag) in bucket {
                    if flag.as_bool() == Some(true) {
                        dst.insert(sender, Value::Bool(true));
                    }
                }
            }
            continue;
        }
        if value.as_bool() != Some(true) {
            continue;
        }
        let Some((emoji, sender)) = key.split_once(REACTION_KEY_SEP) else {
            continue;
        };
        if emoji.is_empty() || sender.is_empty() {
            continue;
        }
        let base = normalize_reaction_emoji(emoji);
        if base.is_empty() {
            continue;
        }
        let entry = nested
            .entry(base)
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        if let Value::Object(dst) = entry {
            dst.insert(sender.to_string(), Value::Bool(true));
        }
    }
    obj.insert("reactions".into(), Value::Object(nested));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageStatus {
    Queued,
    Streaming,
    Done,
    Error,
    Cancelled,
}

impl MessageStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            MessageStatus::Queued => "queued",
            MessageStatus::Streaming => "streaming",
            MessageStatus::Done => "done",
            MessageStatus::Error => "error",
            MessageStatus::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    Pending,
    Taken,
    Done,
    Failed,
}

impl CommandStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            CommandStatus::Pending => "pending",
            CommandStatus::Taken => "taken",
            CommandStatus::Done => "done",
            CommandStatus::Failed => "failed",
        }
    }
}

/// Plain view of one command row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandView {
    pub id: String,
    pub kind: String,
    pub payload: Value,
    #[serde(rename = "issuedBy")]
    pub issued_by: String,
    #[serde(rename = "issuedAt")]
    pub issued_at: i64,
    pub status: String,
}

/// Plain view of one message (deep JSON of the map).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageView {
    pub id: String,
    pub role: String,
    pub status: String,
    pub parts: Vec<Value>,
    #[serde(flatten)]
    pub rest: Value,
}

impl MessageView {
    /// Concatenated text of all `text` parts.
    pub fn text(&self) -> String {
        self.parts
            .iter()
            .filter(|p| p.get("kind").and_then(Value::as_str) == Some("text"))
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("")
    }

    /// Words or a visible attachment: what a reader of the chat can see on this row. A
    /// hand-off row, a background job's host row or a stream that never got text has
    /// neither and is a blank, sender-less line to anyone reading the transcript as text.
    pub fn has_visible_content(&self) -> bool {
        !self.text().trim().is_empty()
            || self.parts.iter().any(|p| {
                p.get("kind").and_then(Value::as_str) == Some("attachment")
                    && p.get("hidden").and_then(Value::as_bool) != Some(true)
            })
    }
}

/// A chat doc with handles cached for the streaming hot path.
pub struct ChatDoc {
    pub doc: LoroDoc,
    live_text: RefCell<HashMap<(String, String), LoroText>>,
    /// Nesting depth for [`Self::transact`]. Only the outermost call commits.
    txn_depth: Cell<u32>,
    /// Set when a value was removed from this doc's state (a secret redaction or scrub, made
    /// here or imported): the holder persists at once. Cleared by [`Self::take_history_dirty`].
    history_dirty: Cell<bool>,
    /// Sticky twin of `history_dirty`: once a value was removed, every [`Self::snapshot`] this
    /// doc exports is shallow (state at the frontier, no older ops), so no persisted copy —
    /// box, relay checkpoint, device store — carries the old text again.
    shallow_mode: Cell<bool>,
    /// Set from inside Loro's diff callback when an import (or a local write) stamps a
    /// message `redactedAt`; folded into the two cells above by [`Self::sync_redaction_flag`].
    redaction_seen: Arc<AtomicBool>,
    _redaction_sub: loro::Subscription,
}

/// Does a diff touch a message's `redactedAt` (a secret redaction arriving)?
fn diff_marks_redaction(event: &DiffEvent<'_>) -> bool {
    event.events.iter().any(|d| match &d.diff {
        loro::event::Diff::Map(m) => m.updated.keys().any(|k| &**k == "redactedAt"),
        _ => false,
    })
}

/// Optional fields for a client user-message write (crdt-sync §6.2).
///
/// Omit empty lists and `None` values — the writer does not store null
/// placeholders, `chatId`, `pending`, or `hiddenParts`.
#[derive(Debug, Clone, Default)]
pub struct UserMessageOpts {
    pub attachments: Vec<UserAttachment>,
    pub mentions: Vec<String>,
    pub reply_to: Option<String>,
    pub sender: Option<String>,
    pub sender_handle: Option<String>,
    pub sender_name: Option<String>,
    pub sender_avatar: Option<String>,
    pub answers_question_id: Option<String>,
    /// The sender's IANA time zone (`"Europe/London"`), written as `tz` so the
    /// agent can answer for the user's moment ("4:28 PM where you are") —
    /// Grok parity trait 1 (agentd reads `tz` / `timeZone` / `timezone`).
    /// Clients pass [`device_time_zone`]; fixtures and tests leave it unset.
    pub tz: Option<String>,
}

/// Reply-to across chats ("Reply in DM", crdt-sync §6.2 `replyRef`): where the
/// quoted message lives plus a snapshot of it, so a reader who is not in the
/// source chat still sees the quote (and nobody has to open another doc to
/// draw the header). `replyTo` stays the in-chat reply link.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplyRef {
    /// The chat the quoted message is in.
    pub chat_id: String,
    /// `"people"` for DMs and people groups, else the project id.
    pub project_id: String,
    pub message_id: String,
    /// Where the quote came from, as the header says it: "in Arbonites".
    #[serde(default)]
    pub chat_title: String,
    /// `user:<id>` / `agent:<project>` / `agent` of the quoted row.
    #[serde(default)]
    pub sender: String,
    #[serde(default)]
    pub sender_name: String,
    #[serde(default)]
    pub sender_handle: String,
    /// The quoted prose, one line, cut to [`REPLY_REF_TEXT_CHARS`].
    #[serde(default)]
    pub text: String,
}

/// Row field that carries a [`ReplyRef`].
pub const REPLY_REF_FIELD: &str = "replyRef";
/// The snapshot keeps this much of the quoted message.
pub const REPLY_REF_TEXT_CHARS: usize = 240;

impl ReplyRef {
    /// Whitespace-collapsed first `REPLY_REF_TEXT_CHARS` characters of `text`,
    /// cut on a word boundary with an ellipsis.
    pub fn snapshot_text(text: &str) -> String {
        let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if flat.chars().count() <= REPLY_REF_TEXT_CHARS {
            return flat;
        }
        let mut cut: String = flat.chars().take(REPLY_REF_TEXT_CHARS).collect();
        if let Some(ix) = cut.rfind(' ') {
            if ix > REPLY_REF_TEXT_CHARS / 2 {
                cut.truncate(ix);
            }
        }
        format!("{}…", cut.trim_end_matches([',', ';', ':', '.']))
    }

    /// The `replyRef` of a row, when it carries a well-formed one.
    pub fn from_row(row: &Value) -> Option<Self> {
        let r: Self = serde_json::from_value(row.get(REPLY_REF_FIELD)?.clone()).ok()?;
        (!r.chat_id.is_empty() && !r.message_id.is_empty()).then_some(r)
    }

    /// Who the quote is from, for "Replying to <who> in <chat>".
    pub fn who(&self) -> String {
        if !self.sender_name.trim().is_empty() {
            return self.sender_name.trim().to_owned();
        }
        if !self.sender_handle.trim().is_empty() {
            return format!("@{}", self.sender_handle.trim());
        }
        "a message".to_owned()
    }
}

/// The device's IANA time zone, if the platform names one (macOS and iOS via
/// CoreFoundation, Linux via /etc/localtime).
pub fn device_time_zone() -> Option<String> {
    iana_time_zone::get_timezone()
        .ok()
        .map(|z| z.trim().to_string())
        .filter(|z| !z.is_empty())
}

/// Where a GIF / sticker came from (a later picker fills this in). Old messages
/// and pasted images have none. Written as an optional `source` map on the
/// attachment: `{ provider, id, pageUrl }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentSource {
    pub provider: String,
    pub id: String,
    pub page_url: String,
}

/// One attachment part on a client user message (`name` / `mime` / optional `kind` / `ref`
/// / optional `url` / optional `source`).
///
/// `kind` is a free string the clients agree on: `image`, `audio`, `video`, `file`,
/// and `sticker` (a picker sticker: drawn at a fixed size with no bubble).
#[derive(Debug, Clone, Default)]
pub struct UserAttachment {
    pub name: String,
    pub mime: String,
    pub kind: Option<String>,
    pub blob_ref: Option<String>,
    /// A picture that lives with its provider, not in the chat slot (a GIF picked from
    /// KLIPY: the provider's terms allow no stored copy). Clients load it through the
    /// relay's pass-through (`GET /gif/media?u=`). Never set together with `blob_ref`.
    pub url: Option<String>,
    /// Attribution for a GIF / sticker picker result; omitted when absent.
    pub source: Option<AttachmentSource>,
}

fn lv_to_json(v: &LoroValue) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

fn json_to_lv(v: &Value) -> LoroValue {
    serde_json::from_value(v.clone()).unwrap_or(LoroValue::Null)
}

fn number_value(n: &serde_json::Number) -> LoroValue {
    if let Some(i) = n.as_i64() {
        LoroValue::I64(i)
    } else if let Some(u) = n.as_u64() {
        LoroValue::I64(u as i64)
    } else {
        LoroValue::Double(n.as_f64().unwrap_or(0.0))
    }
}

/// Write a JSON value under `key`. Objects become `LoroMap`, arrays `LoroList`.
/// (Command payloads use this; no LoroText special case for part text fields.)
fn insert_json_map(map: &LoroMap, key: &str, value: Value) -> CoreResult<()> {
    match value {
        Value::Null => {
            loro_err(map.insert(key, LoroValue::Null))?;
        }
        Value::Bool(b) => {
            loro_err(map.insert(key, b))?;
        }
        Value::Number(n) => {
            loro_err(map.insert(key, number_value(&n)))?;
        }
        Value::String(s) => {
            loro_err(map.insert(key, s))?;
        }
        Value::Array(items) => {
            let list = loro_err(map.insert_container(key, LoroList::new()))?;
            for (i, item) in items.into_iter().enumerate() {
                insert_json_list(&list, i, item)?;
            }
        }
        Value::Object(fields) => {
            let child = loro_err(map.insert_container(key, LoroMap::new()))?;
            for (k, v) in fields {
                insert_json_map(&child, &k, v)?;
            }
        }
    }
    Ok(())
}

/// Write one top-level part field. A `text` / `reasoning` string becomes a `LoroText`
/// (returned so callers can cache it for streaming); everything else goes through
/// [`insert_json_map`].
fn insert_part_field(map: &LoroMap, key: &str, value: Value) -> CoreResult<Option<LoroText>> {
    match value {
        Value::String(s) if key == "text" || key == "reasoning" => {
            let t = loro_err(map.insert_container(key, LoroText::new()))?;
            loro_err(t.push_str(&s))?;
            Ok(Some(t))
        }
        other => {
            insert_json_map(map, key, other)?;
            Ok(None)
        }
    }
}

fn insert_json_list(list: &LoroList, pos: usize, value: Value) -> CoreResult<()> {
    match value {
        Value::Null => {
            loro_err(list.insert(pos, LoroValue::Null))?;
        }
        Value::Bool(b) => {
            loro_err(list.insert(pos, b))?;
        }
        Value::Number(n) => {
            loro_err(list.insert(pos, number_value(&n)))?;
        }
        Value::String(s) => {
            loro_err(list.insert(pos, s))?;
        }
        Value::Array(items) => {
            let child = loro_err(list.insert_container(pos, LoroList::new()))?;
            for (i, item) in items.into_iter().enumerate() {
                insert_json_list(&child, i, item)?;
            }
        }
        Value::Object(fields) => {
            let child = loro_err(list.insert_container(pos, LoroMap::new()))?;
            for (k, v) in fields {
                insert_json_map(&child, &k, v)?;
            }
        }
    }
    Ok(())
}

impl ChatDoc {
    /// A new empty chat doc (crdt-sync R7).
    ///
    /// `peer`: `Some(id)` sets that Loro peer (box / `arbosctl` mint a fresh id
    /// per boot). `None` keeps Loro's random peer — today's Mac / iPhone
    /// behaviour. Never derive a peer from a device id.
    pub fn new(peer: Option<u64>, record_timestamps: bool) -> Self {
        let doc = LoroDoc::new();
        if let Some(peer) = peer {
            doc.set_peer_id(peer).expect("peer id");
        }
        doc.set_record_timestamp(record_timestamps);
        let redaction_seen = Arc::new(AtomicBool::new(false));
        let seen = redaction_seen.clone();
        let sub = doc.subscribe_root(Arc::new(move |e: DiffEvent<'_>| {
            if diff_marks_redaction(&e) {
                seen.store(true, Ordering::Relaxed);
            }
        }));
        Self {
            doc,
            live_text: RefCell::new(HashMap::new()),
            txn_depth: Cell::new(0),
            history_dirty: Cell::new(false),
            shallow_mode: Cell::new(false),
            redaction_seen,
            _redaction_sub: sub,
        }
    }

    /// Fold what the diff callback saw into the flags (called by every reader of them).
    fn sync_redaction_flag(&self) {
        if self.redaction_seen.swap(false, Ordering::Relaxed) {
            self.history_dirty.set(true);
            self.shallow_mode.set(true);
        }
    }

    /// Run `f`, then commit once when this is the outermost `transact`.
    /// Nested calls only decrement depth; they do not commit.
    /// New client writers do not commit themselves; call them inside `transact`.
    /// Depth is restored by a drop guard so a panic inside `f` cannot leave it stuck.
    pub fn transact<R>(&self, f: impl FnOnce(&Self) -> R) -> R {
        struct DepthGuard<'a>(&'a Cell<u32>);
        impl Drop for DepthGuard<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get().saturating_sub(1));
            }
        }
        self.txn_depth.set(self.txn_depth.get() + 1);
        let guard = DepthGuard(&self.txn_depth);
        let out = f(self);
        drop(guard);
        self.commit_now();
        out
    }

    /// Commit pending ops only when no `transact` is open.
    fn commit_now(&self) {
        if self.txn_depth.get() == 0 {
            self.doc.commit();
        }
    }

    /// Write `meta` on a brand-new doc (box side).
    pub fn init_meta(&self, chat_id: &str, project_id: &str, epoch: i64) {
        let meta = self.doc.get_map("meta");
        meta.insert("chatId", chat_id).ok();
        meta.insert("projectId", project_id).ok();
        meta.insert("schemaVersion", SCHEMA_VERSION).ok();
        meta.insert("epoch", epoch).ok();
        self.commit_now();
    }

    pub fn from_snapshot(
        peer: Option<u64>,
        record_timestamps: bool,
        snapshot: &[u8],
    ) -> anyhow::Result<Self> {
        let d = Self::new(peer, record_timestamps);
        d.doc.import(snapshot)?;
        Ok(d)
    }

    pub fn import(&mut self, bytes: &[u8]) -> anyhow::Result<loro::ImportStatus> {
        Ok(self.doc.import(bytes)?)
    }

    pub fn oplog_vv(&self) -> VersionVector {
        self.doc.oplog_vv()
    }

    /// Updates since `from`, after committing pending edits.
    ///
    /// While a [`Self::transact`] is open this returns empty and does not call
    /// Loro `export` (export is a commit barrier and would flush the batch).
    pub fn export_updates_since(&self, from: &VersionVector) -> Vec<u8> {
        if self.txn_depth.get() > 0 {
            return Vec::new();
        }
        self.doc.commit();
        self.doc
            .export(ExportMode::updates(from))
            .unwrap_or_default()
    }

    /// Full snapshot after committing pending edits — or, once a secret was removed from this
    /// doc ([`Self::redact_message_as_secret`], [`Self::scrub_values`], an imported redaction
    /// noted through [`Self::note_redactions_in`]), the [`Self::shallow_snapshot`] instead.
    ///
    /// While a [`Self::transact`] is open this returns empty and does not call
    /// Loro `export` (export is a commit barrier and would flush the batch).
    pub fn snapshot(&self) -> Vec<u8> {
        if self.txn_depth.get() > 0 {
            return Vec::new();
        }
        self.sync_redaction_flag();
        if self.shallow_mode.get() {
            return self.shallow_snapshot();
        }
        self.doc.commit();
        self.doc.export(ExportMode::Snapshot).unwrap_or_default()
    }

    /// Does every snapshot this doc exports drop history (a value was removed)?
    pub fn is_shallow_mode(&self) -> bool {
        self.sync_redaction_flag();
        self.shallow_mode.get()
    }

    /// Ops this doc holds that `theirs` (a peer's version vector, e.g. the relay's in
    /// `ROWS_DONE`) does not: the history-heal payload. Empty when `theirs` already covers this
    /// doc — a Loro export of nothing is still a few header bytes, so callers must not judge by
    /// length alone.
    pub fn updates_beyond(&self, theirs: &VersionVector) -> Vec<u8> {
        if self.txn_depth.get() > 0 {
            return Vec::new();
        }
        self.doc.commit();
        if theirs.includes_vv(&self.doc.oplog_vv()) {
            return Vec::new();
        }
        self.export_updates_since(theirs)
    }

    /// True when every op in `update` is already covered by `vv` (e.g. the room's stored doc).
    /// Uses Loro's import-blob metadata so the relay can ACK a resent push without storing or
    /// gating it. Undecodable bytes are not covered.
    pub fn update_covered_by(update: &[u8], vv: &VersionVector) -> bool {
        match LoroDoc::decode_import_blob_meta(update, true) {
            Ok(meta) => vv.includes_vv(&meta.partial_end_vv),
            Err(_) => false,
        }
    }

    /// True when every op in `vv` is already in this doc (checkpoint containment check, crdt-sync §7.3).
    pub fn contains_vv(&self, vv: &VersionVector) -> bool {
        self.doc.oplog_vv().includes_vv(vv)
    }

    /// §7.3 containment on raw STATE frontier bytes (`decode_vv` then [`Self::contains_vv`]).
    /// False when the frontier does not decode.
    pub fn contains_frontier(&self, frontier: &[u8]) -> bool {
        match decode_vv(frontier) {
            Some(vv) => self.contains_vv(&vv),
            None => false,
        }
    }

    pub fn messages_list(&self) -> LoroList {
        self.doc.get_list("messages")
    }

    pub fn commands_list(&self) -> LoroList {
        self.doc.get_list("commands")
    }

    pub fn to_json(&self) -> Value {
        lv_to_json(&self.doc.get_deep_value())
    }

    /// Message rows as JSON, with [`project_hidden_parts`] and
    /// [`project_reactions`] already applied. Used by the FFI
    /// `read_rows_json`, fixture writer, and fixture tests so those
    /// projections cannot drift apart.
    pub fn projected_rows(&self) -> Vec<Value> {
        let list = self.messages_list();
        (0..list.len())
            .filter_map(|i| Self::projected_row_in(&list, i))
            .collect()
    }

    /// One projected message row by list index: the only row that is read and
    /// converted. Reading a long transcript one row at a time stays linear.
    pub fn projected_row(&self, index: usize) -> Option<Value> {
        Self::projected_row_in(&self.messages_list(), index)
    }

    fn projected_row_in(list: &LoroList, index: usize) -> Option<Value> {
        let Some(ValueOrContainer::Container(Container::Map(m))) = list.get(index) else {
            return None;
        };
        let mut json = serde_json::to_value(m.get_deep_value()).unwrap_or(Value::Null);
        project_hidden_parts(&mut json);
        project_reactions(&mut json);
        Some(json)
    }

    fn find_map(list: &LoroList, id: &str) -> Option<LoroMap> {
        for i in 0..list.len() {
            if let Some(ValueOrContainer::Container(Container::Map(m))) = list.get(i) {
                if let Some(ValueOrContainer::Value(LoroValue::String(s))) = m.get("id") {
                    if s.as_str() == id {
                        return Some(m);
                    }
                }
            }
        }
        None
    }

    pub fn find_message(&self, id: &str) -> Option<LoroMap> {
        Self::find_map(&self.messages_list(), id)
    }

    // -------------------------------------------------------------------------
    // Client writers (crdt-sync §6.2). They never commit — use [`Self::transact`].
    // They write no null placeholders, `chatId`, `pending`, `hiddenParts`, or `call_id`.
    // -------------------------------------------------------------------------

    /// Client write: a finished user row (text + optional attachments / mentions /
    /// reply / author / answer link). Does **not** push a `send` command and does
    /// not commit — pair with [`Self::push_command`] inside [`Self::transact`].
    pub fn append_user_message(
        &self,
        msg_id: &str,
        text: &str,
        device_id: &str,
        opts: &UserMessageOpts,
    ) -> CoreResult<()> {
        let msgs = self.messages_list();
        let m = loro_err(msgs.push_container(LoroMap::new()))?;
        let now = now_ms();
        loro_err(m.insert("id", msg_id))?;
        loro_err(m.insert("role", "user"))?;
        loro_err(m.insert("sender", opts.sender.as_deref().unwrap_or("user")))?;
        if let Some(h) = opts.sender_handle.as_deref() {
            loro_err(m.insert("senderHandle", h))?;
        }
        if let Some(n) = opts.sender_name.as_deref() {
            loro_err(m.insert("senderName", n))?;
        }
        if let Some(a) = opts.sender_avatar.as_deref() {
            loro_err(m.insert("senderAvatar", a))?;
        }
        if !opts.mentions.is_empty() {
            let ml = loro_err(m.insert_container("mentions", LoroList::new()))?;
            for mention in &opts.mentions {
                loro_err(ml.push(mention.as_str()))?;
            }
        }
        if let Some(r) = opts.reply_to.as_deref() {
            loro_err(m.insert("replyTo", r))?;
        }
        if let Some(q) = opts.answers_question_id.as_deref() {
            loro_err(m.insert("answersQuestionId", q))?;
        }
        if let Some(tz) = opts.tz.as_deref().map(str::trim).filter(|z| !z.is_empty()) {
            loro_err(m.insert("tz", tz))?;
        }
        loro_err(m.insert("status", MessageStatus::Done.as_str()))?;
        loro_err(m.insert("deviceId", device_id))?;
        loro_err(m.insert("createdAt", now))?;
        loro_err(m.insert_container("reactions", LoroMap::new()))?;
        let parts = loro_err(m.insert_container("parts", LoroList::new()))?;
        for att in &opts.attachments {
            let part = loro_err(parts.push_container(LoroMap::new()))?;
            loro_err(part.insert("id", ulid()))?;
            loro_err(part.insert("kind", "attachment"))?;
            let a = loro_err(part.insert_container("attachment", LoroMap::new()))?;
            loro_err(a.insert("name", att.name.as_str()))?;
            loro_err(a.insert("mime", att.mime.as_str()))?;
            if let Some(k) = att.kind.as_deref() {
                loro_err(a.insert("kind", k))?;
            }
            if let Some(r) = att.blob_ref.as_deref() {
                loro_err(a.insert("ref", r))?;
            }
            if let Some(u) = att.url.as_deref() {
                loro_err(a.insert("url", u))?;
            }
            if let Some(src) = &att.source {
                let s = loro_err(a.insert_container("source", LoroMap::new()))?;
                loro_err(s.insert("provider", src.provider.as_str()))?;
                loro_err(s.insert("id", src.id.as_str()))?;
                loro_err(s.insert("pageUrl", src.page_url.as_str()))?;
            }
        }
        let part = loro_err(parts.push_container(LoroMap::new()))?;
        loro_err(part.insert("id", ulid()))?;
        loro_err(part.insert("kind", "text"))?;
        let t = loro_err(part.insert_container("text", LoroText::new()))?;
        loro_err(t.insert(0, text))?;
        Ok(())
    }

    /// Client write: a system line (`role: system`, one text part). Optional
    /// `callId` only — never `call_id`. Does not commit.
    pub fn append_system_line(
        &self,
        msg_id: &str,
        text: &str,
        device_id: &str,
        call_id: Option<&str>,
    ) -> CoreResult<()> {
        let msgs = self.messages_list();
        let m = loro_err(msgs.push_container(LoroMap::new()))?;
        let now = now_ms();
        loro_err(m.insert("id", msg_id))?;
        loro_err(m.insert("role", "system"))?;
        loro_err(m.insert("sender", "system"))?;
        loro_err(m.insert("status", MessageStatus::Done.as_str()))?;
        loro_err(m.insert("deviceId", device_id))?;
        loro_err(m.insert("createdAt", now))?;
        loro_err(m.insert("completedAt", now))?;
        if let Some(c) = call_id {
            loro_err(m.insert("callId", c))?;
        }
        loro_err(m.insert_container("reactions", LoroMap::new()))?;
        let parts = loro_err(m.insert_container("parts", LoroList::new()))?;
        let part = loro_err(parts.push_container(LoroMap::new()))?;
        loro_err(part.insert("id", ulid()))?;
        loro_err(part.insert("kind", "text"))?;
        let t = loro_err(part.insert_container("text", LoroText::new()))?;
        loro_err(t.insert(0, text))?;
        Ok(())
    }

    /// Client write (D82): the agent's canned first line in a brand-new project
    /// chat. `role: assistant`, `sender: agent:<projectId>`, `greeting: true`.
    /// Does not commit.
    pub fn append_greeting(
        &self,
        msg_id: &str,
        text: &str,
        device_id: &str,
        project_id: &str,
        agent_name: &str,
    ) -> CoreResult<()> {
        let msgs = self.messages_list();
        let m = loro_err(msgs.push_container(LoroMap::new()))?;
        let now = now_ms();
        loro_err(m.insert("id", msg_id))?;
        loro_err(m.insert("role", "assistant"))?;
        let sender = format!("agent:{project_id}");
        loro_err(m.insert("sender", sender.as_str()))?;
        loro_err(m.insert("senderName", agent_name))?;
        loro_err(m.insert("greeting", true))?;
        loro_err(m.insert("status", MessageStatus::Done.as_str()))?;
        loro_err(m.insert("deviceId", device_id))?;
        loro_err(m.insert("createdAt", now))?;
        loro_err(m.insert("completedAt", now))?;
        loro_err(m.insert_container("reactions", LoroMap::new()))?;
        let parts = loro_err(m.insert_container("parts", LoroList::new()))?;
        let part = loro_err(parts.push_container(LoroMap::new()))?;
        loro_err(part.insert("id", ulid()))?;
        loro_err(part.insert("kind", "text"))?;
        let t = loro_err(part.insert_container("text", LoroText::new()))?;
        loro_err(t.insert(0, text))?;
        Ok(())
    }

    /// True when any message has `greeting: true` (D82 first-line marker).
    pub fn has_greeting(&self) -> bool {
        let msgs = self.messages_list();
        for i in 0..msgs.len() {
            let Some(ValueOrContainer::Container(Container::Map(m))) = msgs.get(i) else {
                continue;
            };
            if matches!(
                m.get("greeting"),
                Some(ValueOrContainer::Value(LoroValue::Bool(true)))
            ) {
                return true;
            }
        }
        false
    }

    /// Client write: set whether `sender` holds `emoji` on a message.
    ///
    /// CRDT shape: flat keys under `reactions` — `"emoji|sender" → true` — so
    /// concurrent first reacts on the same emoji merge as map-key inserts
    /// instead of racing on nested emoji-bucket creation. The emoji side is
    /// skin-tone-normalized (`👍🏽` ≡ `👍`). Readers call [`project_reactions`]
    /// to view `emoji → { sender → true }`. Does not commit. Turning a
    /// reaction off is a no-op when the reactions map is missing (does not
    /// create containers).
    pub fn set_reaction(
        &self,
        msg_id: &str,
        emoji: &str,
        sender: &str,
        on: bool,
    ) -> CoreResult<()> {
        if emoji.is_empty() || sender.is_empty() {
            return Ok(());
        }
        let base = normalize_reaction_emoji(emoji);
        if base.is_empty() {
            return Ok(());
        }
        let Some(map) = self.find_message(msg_id) else {
            return Ok(());
        };
        let reactions = match map.get("reactions") {
            Some(ValueOrContainer::Container(Container::Map(m))) => m,
            _ if on => loro_err(map.insert_container("reactions", LoroMap::new()))?,
            _ => return Ok(()),
        };
        // Drop any older skin-tone variant keys / legacy buckets for this
        // sender so only the normalized base remains.
        clear_skin_tone_reaction_variants(&reactions, &base, sender);
        let key = reaction_key(&base, sender);
        if on {
            loro_err(reactions.insert(key.as_str(), true))?;
        } else {
            // Flat key (current shape). Also clear a legacy nested bucket entry
            // if one is still present from older writers.
            let _ = reactions.delete(key.as_str());
            if let Some(ValueOrContainer::Container(Container::Map(bucket))) =
                reactions.get(base.as_str())
            {
                let _ = bucket.delete(sender);
            }
        }
        Ok(())
    }

    /// Client write: remove one message from the shared `messages` list (delete for
    /// everyone). Does not commit. No-op when the id is missing. Peers see the
    /// same list delete through normal room sync; callers must not announce.
    pub fn delete_message(&self, msg_id: &str) -> CoreResult<()> {
        if msg_id.is_empty() {
            return Ok(());
        }
        let msgs = self.messages_list();
        for i in 0..msgs.len() {
            let Some(ValueOrContainer::Container(Container::Map(m))) = msgs.get(i) else {
                continue;
            };
            let Some(ValueOrContainer::Value(LoroValue::String(s))) = m.get("id") else {
                continue;
            };
            if s.as_str() == msg_id {
                loro_err(msgs.delete(i, 1))?;
                return Ok(());
            }
        }
        Ok(())
    }

    /// Client write: replace the body of a message's last `text` part and set
    /// `editedAt`. Does not commit. No-op when the id is missing or there is no
    /// text part (attachment-only rows). User rows keep one text part after any
    /// attachments; that is the part this updates.
    pub fn edit_message_text(&self, msg_id: &str, text: &str) -> CoreResult<()> {
        if msg_id.is_empty() {
            return Ok(());
        }
        let Some(map) = self.find_message(msg_id) else {
            return Ok(());
        };
        let Some(parts) = self.parts_of(msg_id) else {
            return Ok(());
        };
        let mut text_part: Option<(String, LoroMap)> = None;
        for i in 0..parts.len() {
            let Some(ValueOrContainer::Container(Container::Map(p))) = parts.get(i) else {
                continue;
            };
            let kind = match p.get("kind") {
                Some(ValueOrContainer::Value(LoroValue::String(s))) => s.to_string(),
                _ => continue,
            };
            if kind != "text" {
                continue;
            }
            let id = match p.get("id") {
                Some(ValueOrContainer::Value(LoroValue::String(s))) => s.to_string(),
                _ => continue,
            };
            text_part = Some((id, p));
        }
        let Some((part_id, part)) = text_part else {
            return Ok(());
        };
        let t = loro_err(part.insert_container("text", LoroText::new()))?;
        loro_err(t.insert(0, text))?;
        self.live_text
            .borrow_mut()
            .insert((msg_id.to_owned(), part_id), t);
        loro_err(map.insert("editedAt", now_ms()))?;
        Ok(())
    }

    /// Client write: append a `commands` row. Does not commit. Returns the command id.
    /// Object payloads become a nested `LoroMap` (arrays → `LoroList`).
    pub fn push_command(&self, kind: &str, payload: Value, device_id: &str) -> String {
        self.push_command_at(kind, payload, device_id, now_ms())
    }

    /// `push_command` with an explicit `issuedAt` (replaying history, tests of catch-up order).
    pub fn push_command_at(
        &self,
        kind: &str,
        payload: Value,
        device_id: &str,
        issued_at: i64,
    ) -> String {
        let cmds = self.commands_list();
        let c = cmds.push_container(LoroMap::new()).expect("push command");
        let id = ulid();
        c.insert("id", id.as_str()).ok();
        c.insert("kind", kind).ok();
        insert_json_map(&c, "payload", payload).expect("payload");
        c.insert("issuedBy", device_id).ok();
        c.insert("issuedAt", issued_at).ok();
        c.insert("status", CommandStatus::Pending.as_str()).ok();
        id
    }

    /// Client write on the project doc: mirror the model picker into `settings`
    /// (crdt-sync §6.3). Does not commit.
    pub fn set_model_settings(&self, model: &str, effort: &str) -> CoreResult<()> {
        let settings = self.doc.get_map("settings");
        loro_err(settings.insert("agentModel", model))?;
        loro_err(settings.insert("agentEffort", effort))?;
        insert_json_map(
            &settings,
            "models",
            serde_json::json!({
                "talking_model": { "id": model, "effort": effort }
            }),
        )?;
        Ok(())
    }

    /// Box / CLI write: a user-role message from another sender (`routine:<id>`,
    /// `channel:<platform>:<id>`, `project:<id>`) with extra scalar fields
    /// (`routineName`, `senderName`, …), plus its `send` command.
    /// Honors an open [`Self::transact`] (only the outermost commits).
    /// Returns the command id.
    pub fn append_user_message_from(
        &self,
        msg_id: &str,
        text: &str,
        device_id: &str,
        sender: &str,
        extra: &[(&str, Value)],
    ) -> String {
        self.append_done_message("user", msg_id, text, device_id, sender, extra);
        let cmd_id = self.push_command(
            "send",
            serde_json::json!({ "messageId": msg_id }),
            device_id,
        );
        self.commit_now();
        cmd_id
    }

    /// Box write (F6 groups, S15): a finished message from another Project (`role: "project"`,
    /// `sender: "project:<id>"`, `senderName`, `senderIcon`, `traceId`). When `with_send` is set a
    /// `send` command follows, so the message also drives a turn (a participant addressed by name).
    /// Honors an open [`Self::transact`] (only the outermost commits).
    #[allow(clippy::too_many_arguments)]
    pub fn append_project_message(
        &self,
        msg_id: &str,
        text: &str,
        device_id: &str,
        sender: &str,
        extra: &[(&str, Value)],
        with_send: bool,
    ) -> Option<String> {
        self.append_done_message("project", msg_id, text, device_id, sender, extra);
        let cmd = with_send.then(|| {
            self.push_command(
                "send",
                serde_json::json!({ "messageId": msg_id }),
                device_id,
            )
        });
        self.commit_now();
        cmd
    }

    fn append_done_message(
        &self,
        role: &str,
        msg_id: &str,
        text: &str,
        device_id: &str,
        sender: &str,
        extra: &[(&str, Value)],
    ) {
        let msgs = self.messages_list();
        let m = msgs.push_container(LoroMap::new()).expect("push message");
        let now = now_ms();
        m.insert("id", msg_id).ok();
        m.insert("role", role).ok();
        m.insert("sender", sender).ok();
        m.insert("createdAt", now).ok();
        m.insert("completedAt", now).ok();
        m.insert("deviceId", device_id).ok();
        m.insert("status", MessageStatus::Done.as_str()).ok();
        m.insert_container("reactions", LoroMap::new())
            .expect("reactions");
        for (k, v) in extra {
            m.insert(k, json_to_lv(v)).ok();
        }
        let parts = m.insert_container("parts", LoroList::new()).expect("parts");
        let part = parts.push_container(LoroMap::new()).expect("part");
        part.insert("id", ulid()).ok();
        part.insert("kind", "text").ok();
        let t = part
            .insert_container("text", LoroText::new())
            .expect("text");
        t.insert(0, text).ok();
    }

    /// Box / fixture write: a message row with empty `parts` and an empty `reactions`
    /// map. `completedAt` is set only for a terminal `status`. `extra` fields are
    /// written last (before `parts`), so they can override defaults like `createdAt`.
    /// Honors an open [`Self::transact`] (only the outermost commits).
    pub fn begin_message(
        &self,
        msg_id: &str,
        role: &str,
        sender: &str,
        device_id: &str,
        status: MessageStatus,
        extra: &[(&str, Value)],
    ) -> CoreResult<()> {
        let msgs = self.messages_list();
        let m = loro_err(msgs.push_container(LoroMap::new()))?;
        let now = now_ms();
        loro_err(m.insert("id", msg_id))?;
        loro_err(m.insert("role", role))?;
        loro_err(m.insert("sender", sender))?;
        loro_err(m.insert("createdAt", now))?;
        let terminal = match status {
            MessageStatus::Done | MessageStatus::Error | MessageStatus::Cancelled => true,
            MessageStatus::Queued | MessageStatus::Streaming => false,
        };
        if terminal {
            loro_err(m.insert("completedAt", now))?;
        }
        loro_err(m.insert("deviceId", device_id))?;
        loro_err(m.insert("status", status.as_str()))?;
        loro_err(m.insert_container("reactions", LoroMap::new()))?;
        for (k, v) in extra {
            loro_err(m.insert(k, json_to_lv(v)))?;
        }
        loro_err(m.insert_container("parts", LoroList::new()))?;
        self.commit_now();
        Ok(())
    }

    /// Box write (Calls Phase 2): one finished line of a voice call — what a human said
    /// (`role: "user"`) or what the agent said (`role: "assistant"`) — with no `send` command, so
    /// it never drives a turn. `extra` carries `voice: true`, `callId`, speaker attribution.
    /// Honors an open [`Self::transact`] (only the outermost commits).
    pub fn append_voice_line(
        &self,
        role: &str,
        msg_id: &str,
        text: &str,
        device_id: &str,
        sender: &str,
        extra: &[(&str, Value)],
    ) {
        self.append_done_message(role, msg_id, text, device_id, sender, extra);
        self.commit_now();
    }

    /// Box write: a one-line `system` message with a `note` status part (`code: "note"`).
    /// Neither Mac nor iPhone renders this shape in the transcript today — use
    /// [`Self::append_system_line`] for client-visible system lines. Kept for box
    /// paths (routines / vault / voice) that already write it.
    /// Honors an open [`Self::transact`] (only the outermost commits).
    pub fn append_system_note(&self, msg_id: &str, sender: &str, text: &str, device_id: &str) {
        let msgs = self.messages_list();
        let m = msgs.push_container(LoroMap::new()).expect("push message");
        m.insert("id", msg_id).ok();
        m.insert("role", "system").ok();
        m.insert("sender", sender).ok();
        m.insert("createdAt", now_ms()).ok();
        m.insert("deviceId", device_id).ok();
        m.insert("status", MessageStatus::Done.as_str()).ok();
        m.insert_container("reactions", LoroMap::new())
            .expect("reactions");
        let parts = m.insert_container("parts", LoroList::new()).expect("parts");
        let part = parts.push_container(LoroMap::new()).expect("part");
        part.insert("id", ulid()).ok();
        part.insert("kind", "status").ok();
        part.insert("code", "note").ok();
        part.insert("message", text).ok();
        self.commit_now();
    }

    /// Client write: the user's answer to a `question` part as a user message (rendered text)
    /// plus an `answer` command carrying the structured value.
    /// Honors an open [`Self::transact`] (only the outermost commits).
    pub fn append_user_answer(
        &self,
        msg_id: &str,
        text: &str,
        question_id: &str,
        value: &Value,
        device_id: &str,
    ) -> String {
        let msgs = self.messages_list();
        let m = msgs.push_container(LoroMap::new()).expect("push message");
        m.insert("id", msg_id).ok();
        m.insert("role", "user").ok();
        m.insert("sender", "user").ok();
        m.insert("createdAt", now_ms()).ok();
        m.insert("deviceId", device_id).ok();
        m.insert("status", MessageStatus::Done.as_str()).ok();
        m.insert("answersQuestionId", question_id).ok();
        m.insert_container("reactions", LoroMap::new())
            .expect("reactions");
        let parts = m.insert_container("parts", LoroList::new()).expect("parts");
        let part = parts.push_container(LoroMap::new()).expect("part");
        part.insert("id", ulid()).ok();
        part.insert("kind", "text").ok();
        let t = part
            .insert_container("text", LoroText::new())
            .expect("text");
        t.insert(0, text).ok();
        let cmd_id = self.push_command(
            "answer",
            serde_json::json!({ "messageId": msg_id, "questionId": question_id, "value": value, "text": text }),
            device_id,
        );
        self.commit_now();
        cmd_id
    }

    /// Box / CLI write: append a command row and commit (unless inside [`Self::transact`]).
    /// Honors an open `transact` (only the outermost commits). Returns the command id.
    pub fn append_command(&self, kind: &str, payload: Value, device_id: &str) -> String {
        let id = self.push_command(kind, payload, device_id);
        self.commit_now();
        id
    }

    pub fn commands(&self) -> Vec<CommandView> {
        let v = lv_to_json(&self.commands_list().get_deep_value());
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|c| serde_json::from_value(c.clone()).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn pending_commands(&self) -> Vec<CommandView> {
        self.commands()
            .into_iter()
            .filter(|c| c.status == "pending")
            .collect()
    }

    /// Box side: mark a command `taken` before acting, `done`/`failed` after (mark-before-execute).
    pub fn set_command_status(
        &self,
        cmd_id: &str,
        status: CommandStatus,
        resolution: Option<Value>,
    ) {
        if let Some(c) = Self::find_map(&self.commands_list(), cmd_id) {
            c.insert("status", status.as_str()).ok();
            if let Some(r) = resolution {
                c.insert("resolution", json_to_lv(&r)).ok();
            }
            self.commit_now();
        }
    }

    /// Box write: a new assistant message with one empty streaming text part. Returns `(msg_id, part_id)`.
    pub fn start_assistant_message(
        &self,
        msg_id: &str,
        in_reply_to: &str,
        device_id: &str,
    ) -> String {
        let msgs = self.messages_list();
        let m = msgs.push_container(LoroMap::new()).expect("push message");
        m.insert("id", msg_id).ok();
        m.insert("role", "assistant").ok();
        m.insert("sender", "agent").ok();
        m.insert("createdAt", now_ms()).ok();
        m.insert("deviceId", device_id).ok();
        m.insert("inReplyTo", in_reply_to).ok();
        m.insert("status", MessageStatus::Streaming.as_str()).ok();
        m.insert_container("reactions", LoroMap::new())
            .expect("reactions");
        let parts = m.insert_container("parts", LoroList::new()).expect("parts");
        let part_id = self.add_text_part_inner(&parts, msg_id, "text");
        self.commit_now();
        part_id
    }

    fn add_text_part_inner(&self, parts: &LoroList, msg_id: &str, kind: &str) -> String {
        let part = parts.push_container(LoroMap::new()).expect("part");
        let part_id = ulid();
        part.insert("id", part_id.as_str()).ok();
        part.insert("kind", kind).ok();
        let field = if kind == "reasoning" {
            "reasoning"
        } else {
            "text"
        };
        let t = part.insert_container(field, LoroText::new()).expect("text");
        self.live_text
            .borrow_mut()
            .insert((msg_id.to_owned(), part_id.clone()), t);
        part_id
    }

    /// Add a `text` or `reasoning` part to an existing message.
    pub fn add_text_part(&self, msg_id: &str, kind: &str) -> Option<String> {
        let m = self.find_message(msg_id)?;
        let parts = match m.get("parts") {
            Some(ValueOrContainer::Container(Container::List(l))) => l,
            _ => return None,
        };
        let id = self.add_text_part_inner(&parts, msg_id, kind);
        self.commit_now();
        Some(id)
    }

    /// Append a delta to a live text part. Caller batches commits on the 120 ms tick.
    pub fn append_text(&self, msg_id: &str, part_id: &str, delta: &str) {
        let key = (msg_id.to_owned(), part_id.to_owned());
        let mut live = self.live_text.borrow_mut();
        if !live.contains_key(&key) {
            if let Some(t) = self.lookup_text(msg_id, part_id) {
                live.insert(key.clone(), t);
            }
        }
        if let Some(t) = live.get(&key) {
            let len = t.len_unicode();
            t.insert(len, delta).ok();
        }
    }

    fn lookup_text(&self, msg_id: &str, part_id: &str) -> Option<LoroText> {
        let m = self.find_message(msg_id)?;
        let parts = match m.get("parts")? {
            ValueOrContainer::Container(Container::List(l)) => l,
            _ => return None,
        };
        let p = Self::find_map(&parts, part_id)?;
        for field in ["text", "reasoning"] {
            if let Some(ValueOrContainer::Container(Container::Text(t))) = p.get(field) {
                return Some(t);
            }
        }
        None
    }

    fn parts_of(&self, msg_id: &str) -> Option<LoroList> {
        match self.find_message(msg_id)?.get("parts")? {
            ValueOrContainer::Container(Container::List(l)) => Some(l),
            _ => None,
        }
    }

    /// Append a part from a JSON object (`kind` + fields). Uses the object's `id` when it
    /// is a string, else mints one. Top-level `text` / `reasoning` strings become
    /// `LoroText` (so [`Self::append_text`] can stream into them); a `text` / `reasoning`
    /// kind without its body field gets an empty one. Other values follow the
    /// command-payload shape (objects → `LoroMap`, arrays → `LoroList`).
    /// Returns the part id; `None` when the message or its `parts` list is missing,
    /// or `part` is not an object. Honors an open [`Self::transact`].
    pub fn add_part_from_json(&self, msg_id: &str, part: &Value) -> Option<String> {
        let fields = part.as_object()?;
        let parts = self.parts_of(msg_id)?;
        let map = parts.push_container(LoroMap::new()).ok()?;
        let part_id = match fields.get("id") {
            Some(Value::String(s)) => s.clone(),
            _ => ulid(),
        };
        map.insert("id", part_id.as_str()).ok()?;
        let kind = fields.get("kind").and_then(Value::as_str);
        let body_field = match kind {
            Some("text") => Some("text"),
            Some("reasoning") => Some("reasoning"),
            _ => None,
        };
        let mut body = None;
        for (k, v) in fields {
            if k == "id" {
                continue;
            }
            let t = insert_part_field(&map, k, v.clone()).ok()?;
            if body_field == Some(k.as_str()) {
                body = t;
            }
        }
        if let Some(field) = body_field {
            let t = match body {
                Some(t) => t,
                None => map.insert_container(field, LoroText::new()).ok()?,
            };
            self.live_text
                .borrow_mut()
                .insert((msg_id.to_owned(), part_id.clone()), t);
        }
        self.commit_now();
        Some(part_id)
    }

    /// Set fields on an existing part by id. Same value rules as
    /// [`Self::add_part_from_json`]: a `text` / `reasoning` string replaces the body with a
    /// fresh `LoroText`. Returns false when the message or part is missing.
    /// Honors an open [`Self::transact`].
    pub fn set_part_fields(&self, msg_id: &str, part_id: &str, fields: &[(&str, Value)]) -> bool {
        let Some(parts) = self.parts_of(msg_id) else {
            return false;
        };
        let Some(map) = Self::find_map(&parts, part_id) else {
            return false;
        };
        let key = (msg_id.to_owned(), part_id.to_owned());
        for (k, v) in fields {
            match insert_part_field(&map, k, v.clone()) {
                Ok(Some(t)) => {
                    self.live_text.borrow_mut().insert(key.clone(), t);
                }
                Ok(None) => {
                    if *k == "text" || *k == "reasoning" {
                        self.live_text.borrow_mut().remove(&key);
                    }
                }
                Err(_) => return false,
            }
        }
        self.commit_now();
        true
    }

    /// Remove every part of `kind` from message `msg_id` (the relay dropping a link preview
    /// the sender removed). Returns how many went. Commits like the other part writers.
    pub fn remove_parts_of_kind(&self, msg_id: &str, kind: &str) -> usize {
        let Some(parts) = self.parts_of(msg_id) else {
            return 0;
        };
        let mut removed = 0;
        let mut i = 0;
        while i < parts.len() {
            let is_kind = match parts.get(i) {
                Some(ValueOrContainer::Container(Container::Map(p))) => matches!(
                    p.get("kind"),
                    Some(ValueOrContainer::Value(LoroValue::String(s))) if s.as_str() == kind
                ),
                _ => false,
            };
            if is_kind && parts.delete(i, 1).is_ok() {
                removed += 1;
            } else {
                i += 1;
            }
        }
        if removed > 0 {
            self.commit_now();
        }
        removed
    }

    /// Id of the part at index `pix` in message `msg_id`.
    pub fn part_id_at(&self, msg_id: &str, pix: usize) -> Option<String> {
        let parts = self.parts_of(msg_id)?;
        let Some(ValueOrContainer::Container(Container::Map(p))) = parts.get(pix) else {
            return None;
        };
        match p.get("id")? {
            ValueOrContainer::Value(LoroValue::String(s)) => Some(s.to_string()),
            _ => None,
        }
    }

    /// Add a tool part: ≤160-char summary inline, the rest by `outputRef` (crdt-sync §6.2).
    #[allow(clippy::too_many_arguments)]
    pub fn add_tool_part(
        &self,
        msg_id: &str,
        call_id: &str,
        name: &str,
        input: &Value,
        output_summary: &str,
        output_ref: Option<&str>,
        output_bytes: Option<i64>,
        is_error: bool,
    ) -> Option<String> {
        let m = self.find_message(msg_id)?;
        let parts = match m.get("parts")? {
            ValueOrContainer::Container(Container::List(l)) => l,
            _ => return None,
        };
        let part = parts.push_container(LoroMap::new()).ok()?;
        let part_id = ulid();
        part.insert("id", part_id.as_str()).ok();
        part.insert("kind", "tool").ok();
        part.insert(
            "call",
            json_to_lv(&serde_json::json!({"id": call_id, "name": name, "input": input})),
        )
        .ok();
        let summary: String = output_summary
            .chars()
            .take(crate::TOOL_OUTPUT_SUMMARY_MAX)
            .collect();
        part.insert("outputSummary", summary.as_str()).ok();
        if let Some(r) = output_ref {
            part.insert("outputRef", r).ok();
        }
        if let Some(b) = output_bytes {
            part.insert("outputBytes", b).ok();
        }
        part.insert("isError", is_error).ok();
        // Tool details belong to the Details drawer, not the transcript (demo review item 1).
        part.insert("hidden", true).ok();
        self.commit_now();
        Some(part_id)
    }

    /// Add a part the transcript must not render (`hidden: true`): the raw subagent output
    /// (`kind = "subagent_result"`) or any other drawer-only text. Unknown kinds render empty anyway.
    pub fn add_hidden_part(
        &self,
        msg_id: &str,
        kind: &str,
        text: &str,
        extra: &Value,
    ) -> Option<String> {
        let m = self.find_message(msg_id)?;
        let parts = match m.get("parts")? {
            ValueOrContainer::Container(Container::List(l)) => l,
            _ => return None,
        };
        let part = parts.push_container(LoroMap::new()).ok()?;
        let part_id = ulid();
        part.insert("id", part_id.as_str()).ok();
        part.insert("kind", kind).ok();
        part.insert("text", text).ok();
        part.insert("hidden", true).ok();
        if let Some(o) = extra.as_object() {
            for (k, v) in o {
                part.insert(k, json_to_lv(v)).ok();
            }
        }
        self.commit_now();
        Some(part_id)
    }

    /// Add an `attachment` part (F10): a file the agent produced, blob-backed. `attachment` is
    /// `{ name, mime, ref, bytes, kind, title }`; the bytes live at `GET /blob/{project}/{ref}`.
    pub fn add_attachment_part(&self, msg_id: &str, attachment: &Value) -> Option<String> {
        let m = self.find_message(msg_id)?;
        let parts = match m.get("parts")? {
            ValueOrContainer::Container(Container::List(l)) => l,
            _ => return None,
        };
        let part = parts.push_container(LoroMap::new()).ok()?;
        let part_id = ulid();
        part.insert("id", part_id.as_str()).ok();
        part.insert("kind", "attachment").ok();
        part.insert("attachment", json_to_lv(attachment)).ok();
        self.commit_now();
        Some(part_id)
    }

    /// Add a `status` part. Codes: `working`, `done`, `failed`, `stopped`, `waiting_user`,
    /// `waiting_approval` — one working indicator, nothing that narrates tools. Tool detail lives
    /// in `hidden` tool parts (Details drawer) and T3.
    pub fn add_status_part(&self, msg_id: &str, kind: &str, text: &str) -> Option<String> {
        let m = self.find_message(msg_id)?;
        let parts = match m.get("parts")? {
            ValueOrContainer::Container(Container::List(l)) => l,
            _ => return None,
        };
        let part = parts.push_container(LoroMap::new()).ok()?;
        let part_id = ulid();
        part.insert("id", part_id.as_str()).ok();
        part.insert("kind", "status").ok();
        part.insert("code", kind).ok();
        part.insert("message", text).ok();
        self.commit_now();
        Some(part_id)
    }

    /// Add an `approval` part (design §10, feature `approve`): the card stays in the transcript
    /// with its final `state`. `payload` is rendered by the card (draft fields, command).
    pub fn add_approval_part(
        &self,
        msg_id: &str,
        approval_id: &str,
        req: &ApprovalRequest,
        expires_at: i64,
    ) -> Option<String> {
        let m = self.find_message(msg_id)?;
        let parts = match m.get("parts")? {
            ValueOrContainer::Container(Container::List(l)) => l,
            _ => return None,
        };
        let part = parts.push_container(LoroMap::new()).ok()?;
        let part_id = ulid();
        part.insert("id", part_id.as_str()).ok();
        part.insert("kind", "approval").ok();
        part.insert("approvalId", approval_id).ok();
        part.insert("title", req.title.as_str()).ok();
        part.insert("summary", req.summary.as_str()).ok();
        part.insert("risk", req.risk.as_str()).ok();
        part.insert("payload", json_to_lv(&req.payload)).ok();
        part.insert("state", ApprovalState::Pending.as_str()).ok();
        part.insert("requestedAt", now_ms()).ok();
        part.insert("expiresAt", expires_at).ok();
        self.commit_now();
        Some(part_id)
    }

    fn approval_part(&self, approval_id: &str) -> Option<(String, LoroMap)> {
        let msgs = self.messages_list();
        for i in 0..msgs.len() {
            let Some(ValueOrContainer::Container(Container::Map(m))) = msgs.get(i) else {
                continue;
            };
            let Some(ValueOrContainer::Container(Container::List(parts))) = m.get("parts") else {
                continue;
            };
            for j in 0..parts.len() {
                let Some(ValueOrContainer::Container(Container::Map(p))) = parts.get(j) else {
                    continue;
                };
                let is_match = matches!(
                    p.get("approvalId"),
                    Some(ValueOrContainer::Value(LoroValue::String(s))) if s.as_str() == approval_id
                );
                if is_match {
                    let msg_id = match m.get("id") {
                        Some(ValueOrContainer::Value(LoroValue::String(s))) => s.to_string(),
                        _ => String::new(),
                    };
                    return Some((msg_id, p));
                }
            }
        }
        None
    }

    /// Final state of an approval card. Returns the id of the message holding it.
    pub fn set_approval_state(
        &self,
        approval_id: &str,
        state: ApprovalState,
        decided_by: Option<&str>,
        reason: Option<&str>,
    ) -> Option<String> {
        let (msg_id, p) = self.approval_part(approval_id)?;
        p.insert("state", state.as_str()).ok();
        p.insert("decidedAt", now_ms()).ok();
        if let Some(d) = decided_by {
            p.insert("decidedBy", d).ok();
        }
        if let Some(r) = reason {
            p.insert("reason", r).ok();
        }
        self.commit_now();
        Some(msg_id)
    }

    /// The standing choice made on a destructive-shell card (#1544): `grant` = the path class
    /// the owner allowed from now on, so an answered card shows which row was picked.
    pub fn set_approval_grant(&self, approval_id: &str, path_class: &str) -> Option<String> {
        let (msg_id, p) = self.approval_part(approval_id)?;
        p.insert("grant", path_class).ok();
        self.commit_now();
        Some(msg_id)
    }

    /// Plain JSON of one approval part, if present.
    pub fn approval(&self, approval_id: &str) -> Option<Value> {
        self.approval_part(approval_id)
            .map(|(_, p)| lv_to_json(&p.get_deep_value()))
    }

    /// Every approval part across the chat, in document order.
    pub fn approvals(&self) -> Vec<Value> {
        self.messages()
            .into_iter()
            .flat_map(|m| m.parts.into_iter())
            .filter(|p| p.get("kind").and_then(Value::as_str) == Some("approval"))
            .collect()
    }

    /// Client write: `approve {approvalId}` / `reject {approvalId, reason?}`.
    pub fn append_approval_command(
        &self,
        approve: bool,
        approval_id: &str,
        reason: Option<&str>,
        device_id: &str,
    ) -> String {
        let mut payload = serde_json::json!({ "approvalId": approval_id });
        if let Some(r) = reason {
            payload["reason"] = Value::from(r);
        }
        self.append_command(
            if approve { "approve" } else { "reject" },
            payload,
            device_id,
        )
    }

    /// Add a `question` part (design §10 widgets): `{ kind: "question", questionId, question,
    /// widget?, state: "open" }`. `widget` is `None` for a plain text question (S8 composer).
    pub fn add_question_part(
        &self,
        msg_id: &str,
        question_id: &str,
        question: &str,
        widget: Option<&Value>,
    ) -> Option<String> {
        let m = self.find_message(msg_id)?;
        let parts = match m.get("parts")? {
            ValueOrContainer::Container(Container::List(l)) => l,
            _ => return None,
        };
        let part = parts.push_container(LoroMap::new()).ok()?;
        let part_id = ulid();
        part.insert("id", part_id.as_str()).ok();
        part.insert("kind", "question").ok();
        part.insert("questionId", question_id).ok();
        part.insert("question", question).ok();
        if let Some(w) = widget {
            part.insert("widget", json_to_lv(w)).ok();
        }
        part.insert("state", QuestionState::Open.as_str()).ok();
        self.commit_now();
        Some(part_id)
    }

    /// Mark the `question` part with `questionId` answered and record the value. Returns false
    /// when no such part exists in this doc.
    pub fn answer_question_part(&self, question_id: &str, value: &Value) -> bool {
        let msgs = self.messages_list();
        for i in 0..msgs.len() {
            let Some(ValueOrContainer::Container(Container::Map(m))) = msgs.get(i) else {
                continue;
            };
            let Some(ValueOrContainer::Container(Container::List(parts))) = m.get("parts") else {
                continue;
            };
            for p in 0..parts.len() {
                let Some(ValueOrContainer::Container(Container::Map(part))) = parts.get(p) else {
                    continue;
                };
                let is_match = matches!(part.get("questionId"), Some(ValueOrContainer::Value(LoroValue::String(s))) if s.as_str() == question_id);
                if !is_match {
                    continue;
                }
                part.insert("answer", json_to_lv(value)).ok();
                part.insert("state", QuestionState::Answered.as_str()).ok();
                part.insert("answeredAt", now_ms()).ok();
                m.insert("waitingUser", false).ok();
                self.commit_now();
                return true;
            }
        }
        false
    }

    /// The newest `question` part still `open`, as `(questionId, question, widget)`.
    pub fn open_question(&self) -> Option<(String, String, Option<Value>)> {
        let msgs = self.messages();
        for m in msgs.iter().rev() {
            for p in m.parts.iter().rev() {
                if p.get("kind").and_then(Value::as_str) != Some("question") {
                    continue;
                }
                if p.get("state").and_then(Value::as_str) != Some("open") {
                    continue;
                }
                let qid = p.get("questionId").and_then(Value::as_str)?.to_owned();
                let q = p
                    .get("question")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let w = p.get("widget").filter(|w| !w.is_null()).cloned();
                return Some((qid, q, w));
            }
        }
        None
    }

    pub fn add_error_part(&self, msg_id: &str, code: &str, text: &str) -> Option<String> {
        let m = self.find_message(msg_id)?;
        let parts = match m.get("parts")? {
            ValueOrContainer::Container(Container::List(l)) => l,
            _ => return None,
        };
        let part = parts.push_container(LoroMap::new()).ok()?;
        let part_id = ulid();
        part.insert("id", part_id.as_str()).ok();
        part.insert("kind", "error").ok();
        part.insert("code", code).ok();
        part.insert("message", text).ok();
        self.commit_now();
        Some(part_id)
    }

    /// Set scalar fields on a message (status, decision, subagentId, model, usage, completedAt, …).
    pub fn set_message_fields(&self, msg_id: &str, fields: &[(&str, Value)]) {
        if let Some(m) = self.find_message(msg_id) {
            for (k, v) in fields {
                m.insert(k, json_to_lv(v)).ok();
            }
            self.commit_now();
        }
    }

    /// Cross-chat reply: write `replyRef` on a row (a plain value map, so
    /// every reader sees the whole snapshot at once).
    pub fn set_reply_ref(&self, msg_id: &str, reply_ref: &ReplyRef) {
        if let Ok(v) = serde_json::to_value(reply_ref) {
            self.set_message_fields(msg_id, &[(REPLY_REF_FIELD, v)]);
        }
    }

    pub fn finish_message(&self, msg_id: &str, status: MessageStatus) {
        self.set_message_fields(
            msg_id,
            &[
                ("status", Value::from(status.as_str())),
                ("completedAt", Value::from(now_ms())),
            ],
        );
        self.live_text.borrow_mut().retain(|(m, _), _| m != msg_id);
    }

    /// What the agent is doing right now, in plain words, on the streaming assistant row
    /// (agent-flow contract §2). `None` clears it: clients fall back to "<agent> is working".
    pub fn set_activity(&self, msg_id: &str, activity: Option<&str>) {
        let v = activity
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(Value::from)
            .unwrap_or(Value::Null);
        self.set_message_fields(msg_id, &[("activity", v)]);
    }

    pub fn messages(&self) -> Vec<MessageView> {
        let v = lv_to_json(&self.messages_list().get_deep_value());
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|m| serde_json::from_value(m.clone()).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn message(&self, id: &str) -> Option<MessageView> {
        self.messages().into_iter().find(|m| m.id == id)
    }

    pub fn meta(&self) -> Value {
        lv_to_json(&self.doc.get_map("meta").get_deep_value())
    }

    /// Replace every occurrence of `values` in message text (and a few string fields) with
    /// `[REDACTED:vault]`. Used when the owner scrubs a leaked secret from context. Returns
    /// how many replacements were made. Values shorter than 6 characters are ignored.
    pub fn scrub_values(&self, values: &[String]) -> usize {
        let needles: Vec<&str> = values
            .iter()
            .map(String::as_str)
            .filter(|v| v.len() >= 6)
            .collect();
        if needles.is_empty() {
            return 0;
        }
        let mut hits = 0;
        let msgs = self.messages_list();
        for i in 0..msgs.len() {
            let Some(ValueOrContainer::Container(Container::Map(m))) = msgs.get(i) else {
                continue;
            };
            let Some(ValueOrContainer::Container(Container::List(parts))) = m.get("parts") else {
                continue;
            };
            for j in 0..parts.len() {
                let Some(ValueOrContainer::Container(Container::Map(p))) = parts.get(j) else {
                    continue;
                };
                for field in [
                    "text",
                    "reasoning",
                    "outputSummary",
                    "message",
                    "summary",
                    "title",
                ] {
                    match p.get(field) {
                        Some(ValueOrContainer::Container(Container::Text(t))) => {
                            hits += scrub_loro_text(&t, &needles);
                        }
                        Some(ValueOrContainer::Value(LoroValue::String(s))) => {
                            let original = s.to_string();
                            let scrubbed = scrub_plain(&original, &needles);
                            if scrubbed != original {
                                hits += 1;
                                let _ = p.insert(field, scrubbed.as_str());
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        if hits > 0 {
            self.history_dirty.set(true);
            self.shallow_mode.set(true);
            self.commit_now();
        }
        hits
    }
}

/// The part kind a message becomes when its value was moved into the vault: the "Secret
/// saved as NAME" ghost row. Carries names only — `name`, `env`, `secretId`, `savedBy`
/// (`user:<id>`), `agent` (project id that saved it) — never a value field.
pub const SECRET_REDACTED_KIND: &str = "secret_redacted";

/// What a message's parts are replaced with by [`ChatDoc::redact_message_as_secret`].
#[derive(Debug, Clone, Default)]
pub struct SecretRedaction {
    pub secret_id: String,
    pub name: String,
    pub env: String,
    /// `user:<id>` of the person whose message held the value.
    pub saved_by: String,
    /// Project id of the agent that saved it, when an agent did.
    pub agent: Option<String>,
}

impl ChatDoc {
    /// Relay write: replace **every** part of a message with one `secret_redacted` part and
    /// mark the row `redactedAt`. The old text is gone from the live state; the oplog still
    /// holds it until the holder exports a [`Self::shallow_snapshot`] (the relay does so at
    /// once; clients and the box do on receipt). Returns false when the id is unknown.
    pub fn redact_message_as_secret(&self, msg_id: &str, r: &SecretRedaction) -> bool {
        let Some(map) = self.find_message(msg_id) else {
            return false;
        };
        let Some(parts) = self.parts_of(msg_id) else {
            return false;
        };
        let n = parts.len();
        if n > 0 && parts.delete(0, n).is_err() {
            return false;
        }
        let Ok(part) = parts.push_container(LoroMap::new()) else {
            return false;
        };
        let _ = part.insert("id", ulid().as_str());
        let _ = part.insert("kind", SECRET_REDACTED_KIND);
        let _ = part.insert("name", r.name.as_str());
        let _ = part.insert("env", r.env.as_str());
        let _ = part.insert("secretId", r.secret_id.as_str());
        let _ = part.insert("savedBy", r.saved_by.as_str());
        if let Some(a) = &r.agent {
            let _ = part.insert("agent", a.as_str());
        }
        let _ = map.insert("redactedAt", now_ms());
        // Anything cached for streaming on this message is stale now.
        self.live_text.borrow_mut().retain(|(m, _), _| m != msg_id);
        self.history_dirty.set(true);
        self.shallow_mode.set(true);
        self.commit_now();
        true
    }

    /// Importers call this with the message indexes an import touched: a `secret_redacted`
    /// part among them means the oplog now holds text the state no longer shows.
    pub fn note_redactions_in(&self, touched: &[usize]) {
        if self.history_dirty.get() {
            return;
        }
        let msgs = self.messages_list();
        for &ix in touched {
            let Some(ValueOrContainer::Container(Container::Map(m))) = msgs.get(ix) else {
                continue;
            };
            if m.get("redactedAt").is_some() {
                self.history_dirty.set(true);
                self.shallow_mode.set(true);
                return;
            }
        }
    }

    /// Was a value removed since the last call? The holder persists a shallow snapshot then.
    pub fn take_history_dirty(&self) -> bool {
        self.sync_redaction_flag();
        self.history_dirty.replace(false)
    }

    pub fn mark_history_dirty(&self) {
        self.history_dirty.set(true);
        self.shallow_mode.set(true);
    }

    /// Does any message carry a `secret_redacted` part? Holders use this to decide whether
    /// their persisted snapshot must drop history ([`Self::shallow_snapshot`]).
    pub fn has_secret_redaction(&self) -> bool {
        self.messages().iter().any(|m| {
            m.parts
                .iter()
                .any(|p| p.get("kind").and_then(Value::as_str) == Some(SECRET_REDACTED_KIND))
        })
    }

    /// Snapshot **without** history before the current frontier: the state as it is now plus
    /// nothing older. After a redaction this is the only export that no longer contains the
    /// removed text (a plain [`Self::snapshot`] keeps every past op). Peers import it like any
    /// snapshot; updates made after this frontier merge as usual. Empty while a transact is open.
    pub fn shallow_snapshot(&self) -> Vec<u8> {
        if self.txn_depth.get() > 0 {
            return Vec::new();
        }
        self.doc.commit();
        let f = self.doc.oplog_frontiers();
        self.doc
            .export(ExportMode::shallow_snapshot(&f))
            .unwrap_or_default()
    }
}

const SCRUB_REDACTED: &str = "[REDACTED:vault]";

fn scrub_plain(text: &str, needles: &[&str]) -> String {
    let mut out = text.to_owned();
    for v in needles {
        if out.contains(v) {
            out = out.replace(v, SCRUB_REDACTED);
        }
    }
    out
}

fn scrub_loro_text(t: &LoroText, needles: &[&str]) -> usize {
    let original = t.to_string();
    let scrubbed = scrub_plain(&original, needles);
    if scrubbed == original {
        return 0;
    }
    let hits = needles
        .iter()
        .map(|v| original.matches(v).count())
        .sum::<usize>();
    let len = t.len_unicode();
    let _ = t.delete(0, len);
    let _ = t.insert(0, &scrubbed);
    hits.max(1)
}

/// Encode a version vector for `STATE`'s checkpoint frontier payload.
pub fn encode_vv(vv: &VersionVector) -> Vec<u8> {
    vv.encode()
}

pub fn decode_vv(bytes: &[u8]) -> Option<VersionVector> {
    VersionVector::decode(bytes).ok()
}

/// True when every op in `update` is already covered by `vv`. See [`ChatDoc::update_covered_by`].
pub fn update_covered_by(update: &[u8], vv: &VersionVector) -> bool {
    ChatDoc::update_covered_by(update, vv)
}

#[cfg(test)]
mod secret_redaction_tests {
    use super::*;

    const KEY: &str = concat!(
        "sk_",
        "live_51HxQ9mT3vK2pLnR8wZ4eYc7bD1fG6hJ0kM9nP5qS2tU8vW3xA1"
    );

    fn holds(bytes: &[u8]) -> bool {
        let needle = KEY.as_bytes();
        bytes.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn redaction_replaces_every_part_and_the_shallow_snapshot_forgets_the_value() {
        let doc = ChatDoc::new(Some(7), false);
        doc.append_user_message(
            "m1",
            &format!("{KEY} save this"),
            "dev",
            &UserMessageOpts::default(),
        )
        .unwrap();
        doc.append_user_message(
            "m2",
            "and run the report",
            "dev",
            &UserMessageOpts::default(),
        )
        .unwrap();
        assert!(holds(&doc.snapshot()));
        let ok = doc.redact_message_as_secret(
            "m1",
            &SecretRedaction {
                secret_id: "s1".into(),
                name: "STRIPE_KEY".into(),
                env: "STRIPE_KEY".into(),
                saved_by: "user:u1".into(),
                agent: Some("p1".into()),
            },
        );
        assert!(ok);
        assert!(!doc.redact_message_as_secret("nope", &SecretRedaction::default()));
        let m = doc.message("m1").unwrap();
        assert_eq!(m.parts.len(), 1);
        assert_eq!(m.parts[0]["kind"], SECRET_REDACTED_KIND);
        assert_eq!(m.parts[0]["name"], "STRIPE_KEY");
        assert_eq!(m.parts[0]["savedBy"], "user:u1");
        assert!(m.parts[0].get("value").is_none());
        assert!(m.rest.get("redactedAt").is_some());
        assert_eq!(m.text(), "");
        assert_eq!(doc.message("m2").unwrap().text(), "and run the report");
        assert!(doc.has_secret_redaction());
        assert!(doc.take_history_dirty());
        assert!(!doc.take_history_dirty());

        // a plain export still carries the old op; the doc's own snapshot() is shallow from
        // now on, and a peer loading it sees the ghost row and can keep editing
        assert!(holds(&doc.doc.export(ExportMode::Snapshot).unwrap()));
        assert!(doc.is_shallow_mode());
        assert!(!holds(&doc.snapshot()));
        let shallow = doc.shallow_snapshot();
        assert!(!shallow.is_empty() && !holds(&shallow));
        let peer = ChatDoc::from_snapshot(Some(9), false, &shallow).unwrap();
        assert_eq!(
            peer.message("m1").unwrap().parts[0]["kind"],
            SECRET_REDACTED_KIND
        );
        peer.append_user_message("m3", "later", "dev2", &UserMessageOpts::default())
            .unwrap();
        assert_eq!(peer.messages().len(), 3);
    }

    #[test]
    fn an_importer_notices_a_redaction_in_touched_rows() {
        let a = ChatDoc::new(Some(1), false);
        a.append_user_message("m1", KEY, "dev", &UserMessageOpts::default())
            .unwrap();
        let mut b = ChatDoc::from_snapshot(Some(2), false, &a.snapshot()).unwrap();
        a.redact_message_as_secret(
            "m1",
            &SecretRedaction {
                name: "K".into(),
                ..Default::default()
            },
        );
        let upd = a.export_updates_since(&b.oplog_vv());
        b.import(&upd).unwrap();
        assert!(
            b.take_history_dirty(),
            "the import itself flagged the redaction"
        );
        assert!(b.is_shallow_mode());
        assert!(
            !holds(&b.snapshot()),
            "the importer's own snapshot is shallow too"
        );
        // a doc loaded from a snapshot with a ghost row in it exports shallow too: a ghost
        // anywhere means this room's history must never be re-exported in full
        let c = ChatDoc::from_snapshot(Some(3), false, &b.snapshot()).unwrap();
        assert!(c.is_shallow_mode());
        // the explicit note stays for holders that track touched rows themselves
        let d = ChatDoc::new(Some(4), false);
        d.append_user_message("x", "plain", "dev", &UserMessageOpts::default())
            .unwrap();
        assert!(!d.is_shallow_mode());
        d.note_redactions_in(&[0]);
        assert!(!d.take_history_dirty(), "no redactedAt on that row");
        d.mark_history_dirty();
        assert!(d.take_history_dirty() && d.is_shallow_mode());
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn remove_parts_of_kind_drops_only_that_kind_and_leaves_the_rest() {
        use super::*;
        let doc = ChatDoc::new(Some(1), false);
        doc.append_user_message("m", "look https://a.test/x", "dev", &Default::default())
            .unwrap();
        doc.add_part_from_json(
            "m",
            &serde_json::json!({ "kind": "link_preview", "title": "A", "url": "https://a.test/x" }),
        )
        .unwrap();
        doc.add_part_from_json(
            "m",
            &serde_json::json!({ "kind": "link_preview", "title": "B", "url": "https://a.test/y" }),
        )
        .unwrap();
        assert_eq!(doc.message("m").unwrap().parts.len(), 3);
        assert_eq!(doc.remove_parts_of_kind("m", "link_preview"), 2);
        let m = doc.message("m").unwrap();
        assert_eq!(m.parts.len(), 1);
        assert_eq!(m.parts[0]["kind"], "text");
        assert_eq!(m.text(), "look https://a.test/x");
        assert_eq!(doc.remove_parts_of_kind("m", "link_preview"), 0);
        assert_eq!(doc.remove_parts_of_kind("nope", "link_preview"), 0);
    }

    use super::*;

    #[test]
    fn update_covered_by_when_vv_includes_the_blob() {
        let doc = ChatDoc::new(Some(1), false);
        let base = doc.oplog_vv();
        doc.append_user_message("m1", "hi", "dev", &UserMessageOpts::default())
            .unwrap();
        let update = doc.export_updates_since(&base);
        let vv = doc.oplog_vv();
        assert!(
            ChatDoc::update_covered_by(&update, &vv),
            "the doc's own vv covers the update it just exported"
        );
    }

    #[test]
    fn update_covered_by_false_when_vv_lacks_ops() {
        let doc = ChatDoc::new(Some(2), false);
        let empty = doc.oplog_vv();
        doc.append_user_message("m1", "hi", "dev", &UserMessageOpts::default())
            .unwrap();
        let update = doc.export_updates_since(&empty);
        assert!(
            !ChatDoc::update_covered_by(&update, &empty),
            "an empty vv does not cover new ops"
        );
    }

    #[test]
    fn update_covered_by_false_when_undecodable() {
        let vv = ChatDoc::new(Some(3), false).oplog_vv();
        assert!(
            !ChatDoc::update_covered_by(b"not-a-loro-blob", &vv),
            "garbage bytes are not covered"
        );
    }

    /// Two replicas behind ONE device id (the busy-reply smoke, 2026-10-23): the second replica
    /// never receives the first one's rows (own-device rows are excluded on the way back), so the
    /// box's reply — whose ops depend on the user message — is parked by Loro as a pending import
    /// and the chat reads as empty until those ops arrive. One replica per device, always; the
    /// smoke now mints one device per client. This pins the mechanism so nobody "fixes" it in agentd.
    #[test]
    fn box_rows_stay_pending_until_the_user_message_ops_arrive() {
        let mut box_doc = ChatDoc::new(Some(1), true);
        box_doc.init_meta("chat1", "proj1", 1);
        let snapshot = box_doc.snapshot();
        let mut sender = ChatDoc::new(Some(2), false);
        sender.import(&snapshot).unwrap();
        let mut probe = ChatDoc::new(Some(3), false);
        probe.import(&snapshot).unwrap();

        let base = sender.oplog_vv();
        sender.transact(|d| {
            d.append_user_message("m1", "sleep 90", "dev:shared", &UserMessageOpts::default())
                .unwrap();
            d.push_command(
                "send",
                serde_json::json!({ "messageId": "m1" }),
                "dev:shared",
            );
        });
        let user_row = sender.export_updates_since(&base);
        box_doc.import(&user_row).unwrap();
        let before = box_doc.oplog_vv();
        let cmd = box_doc.pending_commands().remove(0);
        box_doc.set_command_status(&cmd.id, CommandStatus::Taken, None);
        let part = box_doc.start_assistant_message("m2", "m1", "box:1");
        box_doc.append_text("m2", &part, "On it.");
        box_doc.finish_message("m2", MessageStatus::Done);
        let box_rows = box_doc.export_updates_since(&before);

        // The probe shares the sender's device id and never gets `user_row`: the box rows import
        // without error but apply nothing.
        let status = probe.import(&box_rows).unwrap();
        assert!(
            status.pending.is_some(),
            "the box rows wait for the user message ops"
        );
        assert!(
            probe.messages().is_empty(),
            "nothing to show: {:?}",
            probe
                .messages()
                .iter()
                .map(|m| m.id.clone())
                .collect::<Vec<_>>()
        );
        // The moment the missing ops land, everything applies.
        probe.import(&user_row).unwrap();
        let ids: Vec<String> = probe.messages().into_iter().map(|m| m.id).collect();
        assert_eq!(ids, vec!["m1", "m2"]);
    }

    /// Doc write rules (crdt-sync §6.2): a user message and its `send` command land in one
    /// commit, streaming appends are RLE text appends, and a second peer converges.
    #[test]
    fn user_message_and_send_command_converge_across_peers() {
        let mut box_doc = ChatDoc::new(Some(1), true);
        box_doc.init_meta("chat1", "proj1", 1);
        let mut client = ChatDoc::new(Some(2), false);
        client.import(&box_doc.snapshot()).unwrap();

        let base = client.oplog_vv();
        client.transact(|d| {
            d.append_user_message("m1", "hello", "dev:mac", &UserMessageOpts::default())
                .unwrap();
            d.push_command("send", serde_json::json!({ "messageId": "m1" }), "dev:mac");
        });
        let upd = client.export_updates_since(&base);
        box_doc.import(&upd).unwrap();
        let pending = box_doc.pending_commands();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].kind, "send");

        // The peer exports everything the other side lacks (its acked version vector), so the
        // `taken` mark and the streamed reply arrive together.
        let before = client.oplog_vv();
        box_doc.set_command_status(&pending[0].id, CommandStatus::Taken, None);
        let part = box_doc.start_assistant_message("m2", "m1", "box:1");
        box_doc.append_text("m2", &part, "Hi ");
        box_doc.append_text("m2", &part, "there");
        box_doc.finish_message("m2", MessageStatus::Done);
        client
            .import(&box_doc.export_updates_since(&before))
            .unwrap();

        let msgs = client.messages();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1].text(), "Hi there");
        assert_eq!(msgs[1].status, "done");
        assert_eq!(client.commands()[0].status, "taken");
    }

    #[test]
    fn scrub_values_redacts_leaked_secret_from_text() {
        let doc = ChatDoc::new(Some(1), true);
        doc.init_meta("chat1", "proj1", 1);
        doc.append_user_message_from(
            "m1",
            "my key is sk-live-abcdefghijklmnopqrstuvwxyz012345",
            "dev:mac",
            "user",
            &[],
        );
        let part = doc.start_assistant_message("m2", "m1", "box:1");
        doc.append_text(
            "m2",
            &part,
            "got it: sk-live-abcdefghijklmnopqrstuvwxyz012345",
        );
        doc.finish_message("m2", MessageStatus::Done);

        let n = doc.scrub_values(&["sk-live-abcdefghijklmnopqrstuvwxyz012345".into()]);
        assert!(n >= 2);
        let msgs = doc.messages();
        assert!(msgs[0].text().contains("[REDACTED:vault]"));
        assert!(!msgs[0].text().contains("sk-live-"));
        assert!(msgs[1].text().contains("[REDACTED:vault]"));
        assert!(!msgs[1].text().contains("sk-live-"));
    }

    /// Grok parity trait 1 ([feedback hide-worker-rows-and-tz-2026-10-24]): a
    /// client writes the device's IANA zone as `tz` on its user rows so the
    /// agent can answer for the user's moment; nothing is written when the
    /// client has none (fixtures, tests), and a blank zone is not a zone.
    #[test]
    fn user_rows_carry_the_senders_time_zone_when_known() {
        let doc = ChatDoc::new(None, false);
        doc.transact(|d| {
            d.append_user_message(
                "m1",
                "what time is it there?",
                "dev:mac",
                &UserMessageOpts {
                    tz: Some("Europe/London".into()),
                    ..UserMessageOpts::default()
                },
            )
            .unwrap();
            d.append_user_message("m2", "and now?", "dev:fixture", &UserMessageOpts::default())
                .unwrap();
            d.append_user_message(
                "m3",
                "blank",
                "dev:odd",
                &UserMessageOpts {
                    tz: Some("  ".into()),
                    ..UserMessageOpts::default()
                },
            )
            .unwrap();
        });
        let rows = &doc.to_json()["messages"];
        assert_eq!(rows[0]["tz"], "Europe/London");
        assert!(rows[1].get("tz").is_none(), "no zone, no field");
        assert!(rows[2].get("tz").is_none(), "a blank zone is not written");
        // The helper answers something IANA-shaped or nothing — never a blank.
        if let Some(zone) = device_time_zone() {
            assert!(!zone.trim().is_empty() && !zone.contains(' '), "{zone}");
        }
    }

    /// Reply in DM (feedback reply-in-dm-context-2026-10-25): a row's `replyRef`
    /// round-trips as one value map — address + snapshot — so a reader who is
    /// not in the source chat still has the quote; a bare `replyTo` is untouched.
    #[test]
    fn reply_ref_round_trips_on_a_user_row() {
        let doc = ChatDoc::new(None, false);
        doc.transact(|d| {
            d.append_user_message("m1", "Works", "dev:ios", &UserMessageOpts::default())
                .unwrap();
            d.set_reply_ref(
                "m1",
                &ReplyRef {
                    chat_id: "01CHAT".into(),
                    project_id: "people".into(),
                    message_id: "01MSG".into(),
                    chat_title: "Arbonites".into(),
                    sender: "user:gh_1".into(),
                    sender_name: "Jacob".into(),
                    sender_handle: "unconst".into(),
                    text: ReplyRef::snapshot_text("Testing  reply\nin dm"),
                },
            );
        });
        let row = &doc.to_json()["messages"][0];
        let r = ReplyRef::from_row(row).expect("replyRef");
        assert_eq!(r.chat_id, "01CHAT");
        assert_eq!(r.message_id, "01MSG");
        assert_eq!(r.chat_title, "Arbonites");
        assert_eq!(r.text, "Testing reply in dm");
        assert_eq!(r.who(), "Jacob");
        assert!(
            row.get("replyTo").is_none(),
            "a cross-chat reply is not an in-chat reply"
        );
        // A malformed ref (no message id) reads as none, never as a broken header.
        assert!(
            ReplyRef::from_row(&serde_json::json!({ "replyRef": { "chatId": "x" } })).is_none()
        );
        // Long originals are cut on a word with an ellipsis.
        let long = ReplyRef::snapshot_text(&"word ".repeat(100));
        assert!(long.ends_with('…') && long.chars().count() <= REPLY_REF_TEXT_CHARS + 1);
    }

    #[test]
    fn client_writers_omit_placeholders_and_converge() {
        let a = ChatDoc::new(None, false);
        let mut b = ChatDoc::new(None, false);

        let base_a = a.oplog_vv();
        a.transact(|d| {
            d.append_user_message(
                "m1",
                "hi",
                "dev:a",
                &UserMessageOpts {
                    reply_to: Some("prev".into()),
                    sender: Some("user:ken".into()),
                    sender_handle: Some("ken".into()),
                    sender_name: Some("Ken".into()),
                    mentions: vec!["agent:p1".into()],
                    attachments: vec![UserAttachment {
                        name: "pic.png".into(),
                        mime: "image/png".into(),
                        kind: Some("image".into()),
                        blob_ref: Some("blob-1".into()),
                        url: None,
                        source: None,
                    }],
                    ..UserMessageOpts::default()
                },
            )
            .unwrap();
            d.push_command("send", serde_json::json!({ "messageId": "m1" }), "dev:a");
            d.append_system_line("m2", "Sam started a video call", "dev:a", Some("c1"))
                .unwrap();
            d.append_greeting("m3", "Hi, I'm Arbos.", "dev:a", "proj1", "Arbos")
                .unwrap();
            d.set_reaction("m1", "👍", "user:ken", true).unwrap();
            d.set_model_settings("gpt-test", "high").unwrap();
        });

        let row = &a.to_json()["messages"][0];
        assert!(row.get("chatId").is_none());
        assert!(row.get("pending").is_none());
        assert!(row.get("hiddenParts").is_none());
        assert!(row.get("waitingUser").is_none());
        assert!(row.get("reactions").is_some());
        assert_eq!(row["replyTo"], "prev");
        assert_eq!(row["senderHandle"], "ken");
        // Pasted / ordinary attachments omit `source`.
        assert!(row["parts"][0]["attachment"].get("source").is_none());
        let sys = &a.to_json()["messages"][1];
        assert_eq!(sys["callId"], "c1");
        assert!(sys.get("call_id").is_none());
        let greet = &a.to_json()["messages"][2];
        assert_eq!(greet["greeting"], true);
        assert_eq!(greet["sender"], "agent:proj1");
        assert!(a.has_greeting());
        assert_eq!(a.to_json()["settings"]["agentModel"], "gpt-test");
        assert_eq!(a.to_json()["settings"]["agentEffort"], "high");

        b.import(&a.export_updates_since(&base_a)).unwrap();
        assert_eq!(b.messages().len(), 3);
        assert_eq!(b.messages()[0].text(), "hi");
        assert_eq!(b.messages()[1].text(), "Sam started a video call");
        assert_eq!(b.messages()[2].text(), "Hi, I'm Arbos.");
        assert_eq!(b.pending_commands().len(), 1);
        let mut row = b.to_json()["messages"][0].clone();
        project_reactions(&mut row);
        assert_eq!(row["reactions"]["👍"]["user:ken"], true);
        // Storage is flat keys under reactions.
        assert_eq!(
            b.to_json()["messages"][0]["reactions"][reaction_key("👍", "user:ken").as_str()],
            true
        );
        assert_eq!(
            b.to_json()["settings"]["models"]["talking_model"]["id"],
            "gpt-test"
        );
    }

    #[test]
    fn none_peer_uses_loro_random_and_some_sets_explicit() {
        let random = ChatDoc::new(None, false);
        let explicit = ChatDoc::new(Some(42), false);
        assert_eq!(explicit.doc.peer_id(), 42);
        // Random peer is non-zero in practice; just ensure construction works.
        let _ = random.doc.peer_id();
    }

    #[test]
    fn none_peer_twice_yields_distinct_peer_ids() {
        let a = ChatDoc::new(None, false);
        let b = ChatDoc::new(None, false);
        assert_ne!(a.doc.peer_id(), b.doc.peer_id());
    }

    #[test]
    fn concurrent_reactions_and_messages_converge() {
        let mut a = ChatDoc::new(Some(10), false);
        let mut b = ChatDoc::new(Some(20), false);
        a.transact(|d| {
            d.append_user_message("shared", "base", "dev:a", &UserMessageOpts::default())
                .unwrap();
        });
        b.import(&a.snapshot()).unwrap();

        let base_a = a.oplog_vv();
        let base_b = b.oplog_vv();
        a.transact(|d| {
            d.set_reaction("shared", "👍", "user:a", true).unwrap();
            d.set_reaction("shared", "🎉", "user:a", true).unwrap();
            d.append_user_message(
                "from-a",
                "hello from a",
                "dev:a",
                &UserMessageOpts::default(),
            )
            .unwrap();
        });
        b.transact(|d| {
            d.set_reaction("shared", "❤️", "user:b", true).unwrap();
            d.set_reaction("shared", "👍", "user:b", true).unwrap();
            d.append_user_message(
                "from-b",
                "hello from b",
                "dev:b",
                &UserMessageOpts::default(),
            )
            .unwrap();
        });

        let upd_a = a.export_updates_since(&base_a);
        let upd_b = b.export_updates_since(&base_b);
        a.import(&upd_b).unwrap();
        b.import(&upd_a).unwrap();

        for peer in [&a, &b] {
            let json = peer.to_json();
            let mut shared = json["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["id"] == "shared")
                .cloned()
                .unwrap();
            project_reactions(&mut shared);
            assert_eq!(shared["reactions"]["👍"]["user:a"], true);
            assert_eq!(shared["reactions"]["👍"]["user:b"], true);
            assert_eq!(shared["reactions"]["🎉"]["user:a"], true);
            assert_eq!(shared["reactions"]["❤️"]["user:b"], true);
            let ids: Vec<_> = peer.messages().into_iter().map(|m| m.id).collect();
            assert!(ids.contains(&"from-a".into()));
            assert!(ids.contains(&"from-b".into()));
            assert!(ids.contains(&"shared".into()));
        }
    }

    #[test]
    fn concurrent_same_emoji_first_react_both_survive() {
        // Pins the flat "emoji|sender" shape: two peers reacting on=true for the
        // same emoji on a fresh reactions map both keep their sender after merge.
        let mut a = ChatDoc::new(Some(11), false);
        let mut b = ChatDoc::new(Some(22), false);
        a.transact(|d| {
            d.append_user_message("m", "hi", "dev:a", &UserMessageOpts::default())
                .unwrap();
        });
        b.import(&a.snapshot()).unwrap();
        let base_a = a.oplog_vv();
        let base_b = b.oplog_vv();
        a.transact(|d| d.set_reaction("m", "👍", "user:a", true).unwrap());
        b.transact(|d| d.set_reaction("m", "👍", "user:b", true).unwrap());
        a.import(&b.export_updates_since(&base_b)).unwrap();
        b.import(&a.export_updates_since(&base_a)).unwrap();
        for peer in [&a, &b] {
            let mut row = peer.to_json()["messages"][0].clone();
            project_reactions(&mut row);
            assert_eq!(row["reactions"]["👍"]["user:a"], true);
            assert_eq!(row["reactions"]["👍"]["user:b"], true);
        }
    }

    #[test]
    fn skin_tone_variants_collapse_to_one_reaction() {
        let doc = ChatDoc::new(None, false);
        doc.transact(|d| {
            d.append_user_message("m", "hi", "dev:a", &UserMessageOpts::default())
                .unwrap();
            d.set_reaction("m", "👍🏽", "user:a", true).unwrap();
            d.set_reaction("m", "👍🏿", "user:b", true).unwrap();
        });
        let mut row = doc.to_json()["messages"][0].clone();
        project_reactions(&mut row);
        assert_eq!(row["reactions"]["👍"]["user:a"], true);
        assert_eq!(row["reactions"]["👍"]["user:b"], true);
        assert!(row["reactions"].get("👍🏽").is_none());
        assert!(row["reactions"].get("👍🏿").is_none());
        // Same sender toggling a different tone flips the shared base off.
        doc.transact(|d| d.set_reaction("m", "👍🏻", "user:a", false).unwrap());
        let mut row = doc.to_json()["messages"][0].clone();
        project_reactions(&mut row);
        assert!(row["reactions"]
            .get("👍")
            .and_then(|b| b.get("user:a"))
            .is_none());
        assert_eq!(row["reactions"]["👍"]["user:b"], true);
    }

    #[test]
    fn set_reaction_off_is_noop_when_reactions_missing() {
        let doc = ChatDoc::new(None, false);
        // Legacy-shaped row with no reactions map.
        let msgs = doc.messages_list();
        let m = msgs.push_container(LoroMap::new()).expect("push");
        m.insert("id", "legacy").ok();
        m.insert("role", "user").ok();
        m.insert("sender", "user").ok();
        m.insert("status", "done").ok();
        m.insert("deviceId", "dev").ok();
        m.insert("createdAt", now_ms()).ok();
        let parts = m.insert_container("parts", LoroList::new()).expect("parts");
        let part = parts.push_container(LoroMap::new()).expect("part");
        part.insert("id", ulid()).ok();
        part.insert("kind", "text").ok();
        let t = part
            .insert_container("text", LoroText::new())
            .expect("text");
        t.insert(0, "hi").ok();
        doc.doc.commit();

        doc.set_reaction("legacy", "👍", "user:x", false).unwrap();
        let row = &doc.to_json()["messages"][0];
        assert!(row.get("reactions").is_none());
    }

    #[test]
    fn delete_message_removes_row_and_syncs() {
        let a = ChatDoc::new(None, false);
        let mut b = ChatDoc::new(None, false);
        a.transact(|d| {
            d.append_user_message("keep", "stay", "dev:a", &UserMessageOpts::default())
                .unwrap();
            d.append_user_message("gone", "bye", "dev:a", &UserMessageOpts::default())
                .unwrap();
        });
        b.import(&a.snapshot()).unwrap();
        assert_eq!(a.messages_list().len(), 2);
        assert_eq!(b.messages_list().len(), 2);

        let base = a.oplog_vv();
        a.transact(|d| d.delete_message("gone").unwrap());
        assert_eq!(a.messages_list().len(), 1);
        assert_eq!(a.messages()[0].id, "keep");
        assert!(a.message("gone").is_none());
        a.delete_message("gone").unwrap(); // idempotent
        a.delete_message("").unwrap();

        b.import(&a.export_updates_since(&base)).unwrap();
        assert_eq!(b.messages_list().len(), 1);
        assert_eq!(b.messages()[0].id, "keep");
    }

    #[test]
    fn edit_message_text_replaces_body_and_syncs() {
        let a = ChatDoc::new(None, false);
        let mut b = ChatDoc::new(None, false);
        a.transact(|d| {
            d.append_user_message("m1", "hello", "dev:a", &UserMessageOpts::default())
                .unwrap();
        });
        b.import(&a.snapshot()).unwrap();
        assert_eq!(message_text(&a.to_json()["messages"][0]), "hello");

        let base = a.oplog_vv();
        a.transact(|d| d.edit_message_text("m1", "hello edited").unwrap());
        let row = &a.to_json()["messages"][0];
        assert_eq!(message_text(row), "hello edited");
        assert!(row.get("editedAt").and_then(|v| v.as_i64()).unwrap_or(0) > 0);
        a.edit_message_text("missing", "x").unwrap(); // missing id is a no-op
        a.edit_message_text("", "x").unwrap();

        b.import(&a.export_updates_since(&base)).unwrap();
        assert_eq!(message_text(&b.to_json()["messages"][0]), "hello edited");
    }

    #[test]
    fn snapshot_and_export_skip_commit_while_transact_open() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let doc = ChatDoc::new(Some(3), false);
        let commits = Arc::new(AtomicUsize::new(0));
        let commits_cb = commits.clone();
        let _sub = doc.doc.subscribe_local_update(Box::new(move |_| {
            commits_cb.fetch_add(1, Ordering::SeqCst);
            true
        }));
        let before = doc.oplog_vv();
        doc.transact(|d| {
            d.append_user_message("m1", "hi", "dev", &UserMessageOpts::default())
                .unwrap();
            assert!(
                d.snapshot().is_empty(),
                "snapshot must not export (commit) inside transact"
            );
            assert!(
                d.export_updates_since(&before).is_empty(),
                "export_updates_since must not export (commit) inside transact"
            );
            assert_eq!(
                commits.load(Ordering::SeqCst),
                0,
                "snapshot/export must not commit inside transact"
            );
        });
        assert_eq!(commits.load(Ordering::SeqCst), 1);
        assert!(!doc.snapshot().is_empty());
    }

    #[test]
    fn nested_transact_commits_only_at_outermost() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let doc = ChatDoc::new(Some(7), false);
        let commits = Arc::new(AtomicUsize::new(0));
        let commits_cb = commits.clone();
        let _sub = doc.doc.subscribe_local_update(Box::new(move |_| {
            commits_cb.fetch_add(1, Ordering::SeqCst);
            true
        }));
        let before = doc.oplog_vv();
        doc.transact(|outer| {
            outer
                .append_user_message("m1", "hi", "dev", &UserMessageOpts::default())
                .unwrap();
            // Inner append_command used to commit immediately; with re-entrant
            // transact it must wait for the outer frame.
            outer.transact(|inner| {
                let _ =
                    inner.append_command("send", serde_json::json!({ "messageId": "m1" }), "dev");
            });
            assert_eq!(
                commits.load(Ordering::SeqCst),
                0,
                "no commit until outermost transact returns"
            );
        });
        assert_eq!(commits.load(Ordering::SeqCst), 1);
        let upd = doc.export_updates_since(&before);
        let mut peer = ChatDoc::new(Some(8), false);
        peer.import(&upd).unwrap();
        assert_eq!(peer.messages().len(), 1);
        assert_eq!(peer.pending_commands().len(), 1);
    }

    #[test]
    fn push_command_payload_is_nested_map_container() {
        let doc = ChatDoc::new(None, false);
        doc.transact(|d| {
            d.push_command(
                "send",
                serde_json::json!({ "messageId": "m1", "nested": { "ok": true } }),
                "dev",
            );
        });
        let cmds = doc.commands_list();
        let Some(ValueOrContainer::Container(Container::Map(c))) = cmds.get(0) else {
            panic!("expected command map");
        };
        match c.get("payload") {
            Some(ValueOrContainer::Container(Container::Map(payload))) => {
                match payload.get("nested") {
                    Some(ValueOrContainer::Container(Container::Map(_))) => {}
                    other => panic!("expected nested map container, got {other:?}"),
                }
            }
            other => panic!("expected payload map container, got {other:?}"),
        }
        assert_eq!(doc.commands()[0].payload["messageId"], "m1");
        assert_eq!(doc.commands()[0].payload["nested"]["ok"], true);
    }

    #[test]
    fn json_part_writers_stream_and_commit_once_inside_transact() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let doc = ChatDoc::new(Some(9), false);
        let commits = Arc::new(AtomicUsize::new(0));
        let commits_cb = commits.clone();
        let _sub = doc.doc.subscribe_local_update(Box::new(move |_| {
            commits_cb.fetch_add(1, Ordering::SeqCst);
            true
        }));
        let (text_id, tool_id) = doc.transact(|d| {
            d.begin_message(
                "m1",
                "assistant",
                "agent",
                "fixture",
                MessageStatus::Streaming,
                &[("model", Value::from("demo"))],
            )
            .unwrap();
            let text_id = d
                .add_part_from_json("m1", &serde_json::json!({ "kind": "text", "text": "Hi" }))
                .unwrap();
            let tool_id = d
                .add_part_from_json(
                    "m1",
                    &serde_json::json!({
                        "id": "tool-1",
                        "kind": "tool",
                        "call": { "id": "c1", "name": "grep", "input": { "q": "x" } },
                        "hidden": true,
                    }),
                )
                .unwrap();
            d.append_text("m1", &text_id, " there");
            assert!(d.set_part_fields("m1", &tool_id, &[("outputSummary", Value::from("ok"))]));
            let reasoning_id = d
                .add_part_from_json("m1", &serde_json::json!({ "kind": "reasoning" }))
                .unwrap();
            d.append_text("m1", &reasoning_id, "thinking");
            d.start_assistant_message("m2", "m1", "fixture");
            d.finish_message("m1", MessageStatus::Done);
            (text_id, tool_id)
        });
        assert_eq!(commits.load(Ordering::SeqCst), 1);
        assert_eq!(tool_id, "tool-1");
        assert_eq!(doc.part_id_at("m1", 0).as_deref(), Some(text_id.as_str()));
        assert_eq!(doc.part_id_at("m1", 1).as_deref(), Some("tool-1"));
        assert!(doc.part_id_at("m1", 3).is_none());
        assert!(doc.part_id_at("missing", 0).is_none());

        let raw = doc.to_json();
        let m1 = &raw["messages"][0];
        assert_eq!(m1["status"], "done");
        assert_eq!(m1["model"], "demo");
        assert!(m1.get("completedAt").is_some());
        assert!(m1["reactions"].as_object().unwrap().is_empty());
        assert_eq!(m1["parts"][0]["text"], "Hi there");
        assert_eq!(m1["parts"][1]["call"]["input"]["q"], "x");
        assert_eq!(m1["parts"][1]["outputSummary"], "ok");
        assert_eq!(m1["parts"][2]["reasoning"], "thinking");
        assert!(doc.lookup_text("m1", &text_id).is_some());
        assert_eq!(raw["messages"][1]["status"], "streaming");
    }

    #[test]
    fn begin_message_sets_completed_at_only_when_terminal() {
        let doc = ChatDoc::new(None, false);
        doc.begin_message("q", "user", "user", "dev", MessageStatus::Queued, &[])
            .unwrap();
        doc.begin_message("e", "assistant", "agent", "dev", MessageStatus::Error, &[])
            .unwrap();
        let raw = doc.to_json();
        assert!(raw["messages"][0].get("completedAt").is_none());
        assert_eq!(raw["messages"][0]["parts"], serde_json::json!([]));
        assert!(raw["messages"][1].get("completedAt").is_some());
    }

    #[test]
    fn set_part_fields_text_replaces_live_body() {
        let doc = ChatDoc::new(None, false);
        doc.begin_message(
            "m",
            "assistant",
            "agent",
            "dev",
            MessageStatus::Streaming,
            &[],
        )
        .unwrap();
        let pid = doc
            .add_part_from_json("m", &serde_json::json!({ "kind": "text", "text": "old" }))
            .unwrap();
        assert!(doc.set_part_fields("m", &pid, &[("text", Value::from("new"))]));
        doc.append_text("m", &pid, "!");
        assert_eq!(doc.message("m").unwrap().text(), "new!");
        assert!(!doc.set_part_fields("m", "nope", &[("x", Value::from(1))]));
    }

    #[test]
    fn project_hidden_parts_moves_hidden_into_hidden_parts() {
        let mut json = serde_json::json!({
            "id": "m1",
            "parts": [
                { "kind": "text", "text": "visible" },
                { "kind": "tool", "hidden": true, "call": { "id": "t1" } },
            ]
        });
        project_hidden_parts(&mut json);
        assert_eq!(json["parts"].as_array().unwrap().len(), 1);
        assert_eq!(json["parts"][0]["kind"], "text");
        assert_eq!(json["hiddenParts"].as_array().unwrap().len(), 1);
        assert_eq!(json["hiddenParts"][0]["kind"], "tool");
    }

    /// The two blank Arbonites rows of 2026-10-28 (an agent-to-agent hand-off row and a job's
    /// host row) have no words and no visible attachment; a reply, a picture, and a row whose
    /// only attachment is hidden are told apart.
    #[test]
    fn has_visible_content_tells_blank_rows_apart() {
        use super::*;
        use serde_json::json;
        let d = ChatDoc::new(None, false);
        d.start_assistant_message("handoff", "ask", "box:1");
        d.set_message_fields(
            "handoff",
            &[
                ("turn", json!({ "kind": "spawn" })),
                ("subagentName", json!("Bob")),
            ],
        );
        d.add_hidden_part("handoff", "agent_message_sent", "Hey Bob", &json!({}));
        d.finish_message("handoff", MessageStatus::Done);
        let part = d.start_assistant_message("reply", "ask", "box:1");
        d.append_text("reply", &part, "Bob's here.");
        d.finish_message("reply", MessageStatus::Done);
        d.transact(|d| {
            d.append_user_message(
                "photo",
                "",
                "dev",
                &UserMessageOpts {
                    attachments: vec![UserAttachment {
                        name: "a.jpg".into(),
                        mime: "image/jpeg".into(),
                        kind: Some("image".into()),
                        blob_ref: Some("k".into()),
                        url: None,
                        source: None,
                    }],
                    ..Default::default()
                },
            )
            .unwrap();
        });
        let by_id = |id: &str| d.message(id).unwrap();
        assert!(!by_id("handoff").has_visible_content());
        assert!(by_id("reply").has_visible_content());
        assert!(by_id("photo").has_visible_content());
        let hidden_card = MessageView {
            id: "h".into(),
            role: "assistant".into(),
            status: "done".into(),
            parts: vec![
                json!({ "kind": "text", "text": "  " }),
                json!({ "kind": "attachment", "hidden": true, "attachment": { "ref": "k" } }),
            ],
            rest: json!({}),
        };
        assert!(!hidden_card.has_visible_content());
    }

    /// An attachment with and without `source` round-trips through the doc
    /// (feedback app-01M5TGH18WV3QCK5KZT6XKG7FG): the picker attribution map
    /// is optional and omitted when absent.
    #[test]
    fn attachment_source_round_trips_through_the_doc() {
        let with = ChatDoc::new(None, false);
        with.transact(|d| {
            d.append_user_message(
                "m-src",
                "gif",
                "dev:a",
                &UserMessageOpts {
                    attachments: vec![UserAttachment {
                        name: "dance.gif".into(),
                        mime: "image/gif".into(),
                        kind: Some("image".into()),
                        blob_ref: Some("blob-gif".into()),
                        url: None,
                        source: Some(AttachmentSource {
                            provider: "giphy".into(),
                            id: "abc".into(),
                            page_url: "https://giphy.com/gifs/abc".into(),
                        }),
                    }],
                    ..UserMessageOpts::default()
                },
            )
            .unwrap();
        });
        let att = &with.to_json()["messages"][0]["parts"][0]["attachment"];
        assert_eq!(att["source"]["provider"], "giphy");
        assert_eq!(att["source"]["id"], "abc");
        assert_eq!(att["source"]["pageUrl"], "https://giphy.com/gifs/abc");

        let without = ChatDoc::new(None, false);
        without.transact(|d| {
            d.append_user_message(
                "m-plain",
                "pic",
                "dev:a",
                &UserMessageOpts {
                    attachments: vec![UserAttachment {
                        name: "shot.png".into(),
                        mime: "image/png".into(),
                        kind: Some("image".into()),
                        blob_ref: Some("blob-png".into()),
                        url: None,
                        source: None,
                    }],
                    ..UserMessageOpts::default()
                },
            )
            .unwrap();
        });
        let plain = &without.to_json()["messages"][0]["parts"][0]["attachment"];
        assert!(plain.get("source").is_none());
        assert_eq!(plain["name"], "shot.png");
    }
}
