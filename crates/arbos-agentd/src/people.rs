//! People chats (D72) on the box side. This Project's agent can be a member of chats between
//! people; it joins each one as a peer in the room `people/<chat_id>` (the relay admits the
//! owner's box for it) and answers the humans there.
//!
//! Who answers is the judge's call (`responders.rs`), one call per human message, the same on
//! every box in the room, with no keyword rule before it (Jacob, 2026-10-27: "the agents are
//! getting better but still buggy" woke three agents because the text started with "the
//! agents"). The judge sees the people (name + @handle), every agent (handle, role, how the
//! text @mentions it), the last 12 lines, the message and the line it replies to, and says
//! who it is for: one agent, any one agent, several, all, or nobody. The agent replies only on
//! a confident yes (`decision_model.threshold`, 0.60). A bare name, a reply to the agent's
//! bubble or a follow-up are signals in that state, never a verdict. A person's explicit
//! @mention (typed `@handle` or a chip) is the exception: it always wakes every agent it names,
//! whatever the judge says or when it fails (Jacob, 2026-10-28); a woken agent that was only
//! named may still say nothing. The owner's `reply_policy: every_message` bypasses the judge.
//! When the judge fails, an unmentioned message is silence. Silence posts nothing
//! (`silence.rs`). An agent's own bubble never wakes another agent (#2207).
//!
//! Each human message is labelled with the author's display name (falling back to bare handle)
//! and role (`owner` | `guest`) in the prompt. The author is the row's `sender: user:<id>`,
//! else the person whose device wrote it (the relay's box roster lists each member's device
//! ids), else the only human in the chat; a row nobody can be matched to is "a member" with
//! guest powers — the owner is never assumed (#1580). Guests may direct ordinary work; they may
//! not use the owner's secrets unless the owner put them on that secret's People list. Agents
//! address people by display name, never `@handle`. The owner's credits pay; audit lines carry
//! `asked_by`.

use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::Result;
use serde_json::{json, Value};

use arbos_core::doc::MessageView;
use arbos_core::ids::now_ms;

use crate::agent::Agent;
use crate::responders::{Candidate, Room};
use crate::sync::SharedHost;

/// The relay's project slot for people rooms.
pub const PEOPLE_PROJECT: &str = "people";

/// How many prior messages the judge sees (oldest → newest).
const RESPOND_RECENT_MAX: usize = 12;

#[derive(Debug, Clone, Default)]
pub struct PeopleChatInfo {
    pub members: Vec<Value>,
    /// `reply_policy`, `restrict_guests` for this agent in this chat.
    pub settings: Value,
    /// This agent's owner (the Project's user).
    pub owner_user: String,
    /// `@slug` this agent answers to here.
    pub slug: String,
    /// `dm` or `group` from the relay.
    pub kind: String,
    /// The owner's current name from the relay profile (#1895's owner block) at the time this
    /// snapshot was taken (every poll). One owner, found by `owner_user`, named by this — never
    /// by a row's `senderName` of the time (Hezbola, 2026-10-26: "the chat uses two names for
    /// the owner, Arbos and Jacob" — old rows said Arbos, the profile said Jacob).
    pub owner_address: Option<String>,
}

impl PeopleChatInfo {
    fn agents(&self) -> usize {
        self.members
            .iter()
            .filter(|m| {
                m["member"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("agent:"))
            })
            .count()
    }

    fn is_group(&self) -> bool {
        self.kind == "group" || self.agents() > 1
    }

    fn handle_of(&self, user_id: &str) -> String {
        self.members
            .iter()
            .find(|m| m["member"].as_str() == Some(&format!("user:{user_id}")))
            .and_then(|m| m["handle"].as_str())
            .unwrap_or(user_id)
            .to_owned()
    }

    fn owner_handle(&self) -> String {
        self.handle_of(&self.owner_user)
    }

    /// "Jacob's" — whose budget this agent spends, for the budget notice (`budget.rs`).
    pub fn owner_spoken_possessive(&self) -> String {
        let name = self.spoken_name_of(&self.owner_user);
        if name.ends_with('s') {
            format!("{name}'")
        } else {
            format!("{name}'s")
        }
    }

    /// How to speak to / about this person: display name, else bare handle (no `@`).
    fn spoken_name_of(&self, user_id: &str) -> String {
        if user_id == self.owner_user {
            if let Some(a) = self.owner_address.as_deref().map(str::trim) {
                if !a.is_empty() {
                    return a.to_owned();
                }
            }
        }
        self.members
            .iter()
            .find(|m| m["member"].as_str() == Some(&format!("user:{user_id}")))
            .map(person_spoken_name)
            .unwrap_or_else(|| self.handle_of(user_id))
    }

    /// `spoken_name_of` with the relay profile's address for this Project's owner winning over
    /// the member snapshot (the snapshot may be older than a PATCH /me).
    fn spoken_name_of_with_owner(&self, user_id: &str, owner_address: Option<&str>) -> String {
        if user_id == self.owner_user {
            if let Some(a) = owner_address.map(str::trim).filter(|a| !a.is_empty()) {
                return a.to_owned();
            }
        }
        self.spoken_name_of(user_id)
    }

    /// Human members as `Name (@handle)` for the judge's people roster: a first name or an
    /// @handle in the text that is a person's must never read as an agent's ("@A" is Scout,
    /// not @affine).
    fn people_labels(&self) -> Vec<String> {
        self.members
            .iter()
            .filter(|m| m["member"].as_str().is_some_and(|s| s.starts_with("user:")))
            .map(|m| {
                let name = person_spoken_name(m);
                match m["handle"]
                    .as_str()
                    .map(str::trim)
                    .filter(|h| !h.is_empty())
                {
                    Some(h) if !h.eq_ignore_ascii_case(&name) => format!("{name} (@{h})"),
                    _ => name,
                }
            })
            .collect()
    }

    /// Human member ids (`user:` stripped).
    fn human_ids(&self) -> Vec<String> {
        self.members
            .iter()
            .filter_map(|m| m["member"].as_str()?.strip_prefix("user:"))
            .map(str::to_owned)
            .collect()
    }

    /// The person whose device wrote a row, from the box-facing roster's `devices` lists.
    /// Row ids may carry the `dev:` prefix (Mac, arbosctl); the roster has bare ids.
    fn user_of_device(&self, device_id: &str) -> Option<String> {
        let bare = device_id.strip_prefix("dev:").unwrap_or(device_id);
        if bare.is_empty() {
            return None;
        }
        self.members.iter().find_map(|m| {
            let uid = m["member"].as_str()?.strip_prefix("user:")?;
            let devices = m["devices"].as_array()?;
            devices
                .iter()
                .any(|d| d.as_str().is_some_and(|d| d == bare || d == device_id))
                .then(|| uid.to_owned())
        })
    }

    /// Who wrote a human row. `sender: user:<id>` wins; else the row's `deviceId` through the
    /// roster; else the one human in the chat (a DM with only the owner and agents). `None` means
    /// the author is unknown — never assume the owner (#1580: a guest's note was read back as
    /// the owner's).
    pub(crate) fn author_of(&self, msg: &MessageView) -> Option<String> {
        let sender = msg.rest.get("sender").and_then(Value::as_str).unwrap_or("");
        if let Some(uid) = sender.strip_prefix("user:") {
            if !uid.is_empty() {
                return Some(uid.to_owned());
            }
        }
        if let Some(device) = msg.rest.get("deviceId").and_then(Value::as_str) {
            if let Some(uid) = self.user_of_device(device) {
                return Some(uid);
            }
        }
        if let Some(handle) = msg
            .rest
            .get("senderHandle")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|h| !h.is_empty())
        {
            let hit = self.members.iter().find_map(|m| {
                let uid = m["member"].as_str()?.strip_prefix("user:")?;
                m["handle"]
                    .as_str()
                    .is_some_and(|h| h.eq_ignore_ascii_case(handle))
                    .then(|| uid.to_owned())
            });
            if hit.is_some() {
                return hit;
            }
        }
        let humans = self.human_ids();
        match humans.as_slice() {
            [only] => Some(only.clone()),
            _ => None,
        }
    }
}

/// `user_id` of an asker the agent could not identify (see `arbos_core::UNKNOWN_ASKER`).
pub const UNKNOWN_ASKER: &str = arbos_core::UNKNOWN_ASKER;

/// Label for an author the row does not identify.
const UNKNOWN_SPOKEN: &str = "a member";

/// Handle for a human row's author: the row's `senderHandle`, else the roster's for `user_id`,
/// else `a member`.
fn author_handle(msg: &MessageView, info: &PeopleChatInfo, user_id: Option<&str>) -> String {
    msg.rest
        .get("senderHandle")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .map(str::to_owned)
        .or_else(|| user_id.map(|u| info.handle_of(u)))
        .unwrap_or_else(|| UNKNOWN_SPOKEN.to_owned())
}

/// Spoken name for a human row's author, resolved by identity, live: the owner (by user id)
/// is the profile's current name; another member is the roster's current display name / handle;
/// only an author the roster does not know keeps the row's `senderName` label, else the row's
/// handle, else `a member`. A row's `senderName` is the label one device sent at the time — it
/// must never make one person two names across a transcript.
fn author_spoken(msg: &MessageView, info: &PeopleChatInfo, user_id: Option<&str>) -> String {
    if let (Some(u), Some(owner)) = (user_id, info.owner_address.as_deref()) {
        if u == info.owner_user && !owner.trim().is_empty() {
            return owner.trim().to_owned();
        }
    }
    let from_roster = user_id.and_then(|u| {
        info.members
            .iter()
            .find(|m| m["member"].as_str() == Some(&format!("user:{u}")))
            .map(person_spoken_name)
    });
    match from_roster {
        Some(name) => name,
        None => message_spoken_name(msg, &author_handle(msg, info, user_id)),
    }
}

/// Prefer display `name`; else bare `handle`. Never `@handle`.
fn person_spoken_name(m: &Value) -> String {
    m["name"]
        .as_str()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .or_else(|| m["handle"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "?".to_owned())
}

/// Prefer `senderName` on the message; else the fallback (usually bare handle).
fn message_spoken_name(msg: &MessageView, fallback: &str) -> String {
    msg.rest
        .get("senderName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| fallback.to_owned())
}

/// Agent project ids named by `mentions` (`agent:<id>`), in order, deduped.
fn mentioned_agent_ids(msg: &MessageView) -> Vec<String> {
    let Some(mentions) = msg.rest.get("mentions").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for m in mentions {
        let Some(id) = m.as_str().and_then(|s| s.strip_prefix("agent:")) else {
            continue;
        };
        if !out.iter().any(|x| x == id) {
            out.push(id.to_owned());
        }
    }
    out
}

/// What a reply in a people chat carries: message fields for the clients.
/// `replyTo` is the platform quote/reply (clients show Grok's quote header on
/// the bubble). `inReplyTo` is turn linkage (retry, "already replied"). Both
/// point at the asker's message; the prose stays clean.
pub struct ReplyAttribution {
    pub fields: Vec<(&'static str, Value)>,
}

/// Who asked the question the current turn answers.
#[derive(Debug, Clone)]
pub struct Asker {
    pub user_id: String,
    pub handle: String,
    /// Display name when known; empty means speak the bare handle.
    pub name: String,
    pub role: &'static str,
    pub message_id: String,
}

impl Asker {
    /// How to speak about this person: display name, else bare handle (no `@`).
    pub fn spoken(&self) -> &str {
        let n = self.name.trim();
        if n.is_empty() {
            self.handle.as_str()
        } else {
            n
        }
    }
}

#[derive(Default)]
pub struct PeopleState {
    chats: Mutex<HashMap<String, PeopleChatInfo>>,
    /// chat → the asker of the turn now running or last run there.
    askers: Mutex<HashMap<String, Asker>>,
    /// The chat whose turn holds the turn lock right now (for audit attribution).
    current: Mutex<Option<String>>,
    /// This Project's owner (from the relay's project record), for guest labels in project chats.
    project_owner: Mutex<String>,
    /// How to address the owner, from the relay's profile at every poll (never from a row's
    /// `senderName`, which is whatever one device sent): `(address_name, label_name)` —
    /// `address_name` is the name the user set (PATCH /me), `label_name` the GitHub-seeded one.
    owner_profile: Mutex<(Option<String>, Option<String>)>,
    /// The whole owner block as last seen (for the prompt's "Your owner" section).
    owner_full: Mutex<crate::identity::OwnerProfile>,
    /// Judge verdicts by user message id: one call per message, whatever drives the command
    /// (send, steer, retry, a catch-up re-gate). Bounded; see `GATE_CACHE_MAX`.
    gate_cache: Mutex<GateCache>,
}

/// How many judged messages a box remembers (across its people chats).
const GATE_CACHE_MAX: usize = 512;

/// The judge's verdict on one message, as this agent's gate recorded it.
#[derive(Debug, Clone)]
pub struct GateVerdict {
    pub wants: bool,
    pub reason: String,
    /// Every agent the judge picked plus every agent a person @mentioned (the shared-send
    /// bookkeeping reads this).
    pub chosen: Vec<String>,
    /// Set when a person's explicit @mention woke this agent although the judge did not pick
    /// it (unsure, another addressee, or no judge at all).
    pub woken_by_mention: Option<MentionRole>,
}

impl GateVerdict {
    /// A person's explicit @mention (typed `@handle` or a chip) always wakes every agent it
    /// names; the judge can add agents but never veto a mentioned one (Jacob, Arbonites
    /// 2026-10-28: "@bob talk to @bubble" judged `for=@bob (0.48) < 0.50` woke nobody).
    fn honour_mentions(mut self, me: &str, my_role: MentionRole, mentioned: &[String]) -> Self {
        for id in mentioned {
            if !self.chosen.contains(id) {
                self.chosen.push(id.clone());
            }
        }
        if my_role != MentionRole::None && !self.wants {
            self.wants = true;
            self.reason = format!(
                "{}; @mention {} by a person → reply (a person's @mention always wakes the agent)",
                self.reason,
                my_role.as_str()
            );
            self.woken_by_mention = Some(my_role);
            if !self.chosen.iter().any(|c| c == me) {
                self.chosen.push(me.to_owned());
            }
        }
        self
    }
}

#[derive(Default)]
struct GateCache {
    by_message: HashMap<String, GateVerdict>,
    order: std::collections::VecDeque<String>,
}

impl GateCache {
    fn get(&self, message_id: &str) -> Option<GateVerdict> {
        self.by_message.get(message_id).cloned()
    }

    fn insert(&mut self, message_id: &str, v: GateVerdict) {
        if self.by_message.insert(message_id.to_owned(), v).is_none() {
            self.order.push_back(message_id.to_owned());
        }
        while self.order.len() > GATE_CACHE_MAX {
            if let Some(old) = self.order.pop_front() {
                self.by_message.remove(&old);
            }
        }
    }
}

impl PeopleState {
    pub fn set_project_owner(&self, user_id: &str) {
        *self.project_owner.lock().expect("people") = user_id.to_owned();
    }

    /// The relay's `owner` block of the box listing (`/box/hello`, `/box/projects`).
    pub fn set_owner_profile(&self, owner: &Value) {
        let pick = |k: &str| {
            owner
                .get(k)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        *self.owner_profile.lock().expect("people") = (pick("address_name"), pick("name"));
        *self.owner_full.lock().expect("people") = crate::identity::OwnerProfile::from_json(owner);
    }

    pub fn owner_profile(&self) -> crate::identity::OwnerProfile {
        self.owner_full.lock().expect("people").clone()
    }

    /// The name the owner is addressed by: the one they set, else the profile label, else none.
    /// A login/handle is never a name.
    pub fn owner_address_name(&self) -> Option<String> {
        let (address, label) = self.owner_profile.lock().expect("people").clone();
        address.or(label)
    }

    pub fn project_owner(&self) -> String {
        self.project_owner.lock().expect("people").clone()
    }

    pub fn info(&self, chat_id: &str) -> Option<PeopleChatInfo> {
        self.chats.lock().expect("people").get(chat_id).cloned()
    }

    pub fn is_people_chat(&self, chat_id: &str) -> bool {
        self.chats.lock().expect("people").contains_key(chat_id)
    }

    pub fn asker(&self, chat_id: &str) -> Option<Asker> {
        self.askers.lock().expect("people").get(chat_id).cloned()
    }

    /// `asked_by` for an audit line written during the current people-chat turn.
    pub fn current_asker(&self) -> Option<Asker> {
        let chat = self.current.lock().expect("people").clone()?;
        self.asker(&chat)
    }

    pub fn set_current(&self, chat_id: Option<&str>) {
        *self.current.lock().expect("people") = chat_id.map(str::to_owned);
    }

    /// The judge's verdict for a message this gate already decided, if any.
    pub fn gate_verdict(&self, message_id: &str) -> Option<GateVerdict> {
        self.gate_cache.lock().expect("people").get(message_id)
    }

    fn remember_verdict(&self, message_id: &str, v: GateVerdict) {
        self.gate_cache
            .lock()
            .expect("people")
            .insert(message_id, v);
    }

    /// Test-only: seed a people-chat snapshot (e.g. rename + @handle refresh).
    #[cfg(test)]
    pub(crate) fn test_insert(&self, chat_id: &str, info: PeopleChatInfo) {
        self.chats
            .lock()
            .expect("people")
            .insert(chat_id.to_owned(), info);
    }
}

/// Where an `@mention` of an agent sits in the sentence — the difference between being asked
/// and being talked about. An @mention is a wake/consideration signal, not an obligation
/// (Jacob, 2026-10-26: "@hezbola can you communicate directly with @doug-agent?" woke Doug,
/// who then posted "Addressed to Hezbola; staying silent").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MentionRole {
    /// Not mentioned in the prose.
    None,
    /// Vocative: leads the message (after an optional greeting), or stands alone / closes it
    /// ("…, @doug?"), or opens a clause ("…; @doug, your turn").
    Addressed,
    /// The grammatical object of a request made to someone else: after a preposition
    /// ("with @doug", "about @doug", "to @doug", "than @doug"), after "ask/tell/ping"
    /// following another addressee, or possessive ("@doug's renders").
    Object,
    /// Elsewhere mid-sentence, not clearly either ("I think @doug and you should sync").
    Referenced,
}

impl MentionRole {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Addressed => "addressed",
            Self::Object => "object",
            Self::Referenced => "referenced",
        }
    }
}

/// Words that make the next `@mention` the object of the sentence, not its addressee.
const OBJECT_PREPOSITIONS: &[&str] = &[
    "with",
    "about",
    "to",
    "from",
    "for",
    "of",
    "on",
    "at",
    "like",
    "than",
    "via",
    "through",
    "between",
    "regarding",
    "re",
    "re:",
    "against",
    "versus",
    "vs",
    "vs.",
    "towards",
    "toward",
    "into",
    "onto",
    "without",
    "ask",
    "tell",
    "ping",
    "cc",
    "cc:",
    "thank",
    "thanks",
    "unlike",
    "or",
];

/// Classify every occurrence of `@{handle}` in `text` and return the strongest role (one
/// vocative use makes the agent addressed even if it is also referred to later).
pub(crate) fn mention_role(text: &str, handle: &str) -> MentionRole {
    let handle = handle.trim().trim_start_matches('@').to_lowercase();
    if handle.is_empty() {
        return MentionRole::None;
    }
    let lower = text.to_lowercase();
    let needle = format!("@{handle}");
    let mut role = MentionRole::None;
    for (i, _) in lower.match_indices(&needle) {
        let end = i + needle.len();
        let after = lower[end..].chars().next();
        if after.is_some_and(|c| c.is_alphanumeric() || c == '-' || c == '_') {
            continue; // `@dougx` is another handle
        }
        let before = lower[..i].trim_end();
        // Possessive: "@doug's renders".
        if lower[end..].starts_with("'s") || lower[end..].starts_with("’s") {
            role = role.max(MentionRole::Object);
            continue;
        }
        // Vocative at the start (after a greeting / filler), after a clause break, or a lone
        // trailing "…, @doug?" / "… @doug".
        let lead = before
            .trim_end_matches([',', ':', ';', '—', '-', '!', '.'])
            .trim_end();
        let starts = lead.is_empty()
            || [
                "hey", "hi", "hello", "yo", "ok", "okay", "so", "and", "also", "btw", "please",
            ]
            .contains(
                &lead
                    .rsplit(|c: char| !c.is_alphanumeric())
                    .next()
                    .unwrap_or(""),
            ) && lead.split_whitespace().count() <= 2;
        let clause_break = before.ends_with([',', ';', ':', '—', '!', '?', '.']);
        let rest = lower[end..].trim_start();
        let trailing = rest.is_empty()
            || rest
                .trim_start_matches(['?', '!', '.', ',', ' '])
                .is_empty();
        let word_before = before
            .rsplit(|c: char| !(c.is_alphanumeric() || c == ':' || c == '.' || c == '\''))
            .next()
            .unwrap_or("");
        if starts || (clause_break && !OBJECT_PREPOSITIONS.contains(&word_before)) {
            return MentionRole::Addressed;
        }
        if OBJECT_PREPOSITIONS.contains(&word_before) {
            role = role.max(MentionRole::Object);
            continue;
        }
        if trailing && before.ends_with(',') {
            return MentionRole::Addressed;
        }
        role = role.max(MentionRole::Referenced);
    }
    role
}

impl PartialOrd for MentionRole {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MentionRole {
    /// Strength order for `max`: Addressed > Referenced > Object > None.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let rank = |r: &Self| match r {
            Self::None => 0,
            Self::Object => 1,
            Self::Referenced => 2,
            Self::Addressed => 3,
        };
        rank(self).cmp(&rank(other))
    }
}

/// The handles an agent answers to in the prose: its live/snapshot slug, roster handle, and
/// the slugged project name.
fn agent_handles(info: &PeopleChatInfo, project_id: &str, slug: &str) -> Vec<String> {
    let me = format!("agent:{project_id}");
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: &str| {
        let s = s.trim().to_lowercase();
        if !s.is_empty() && !out.contains(&s) {
            out.push(s);
        }
    };
    push(slug);
    push(&info.slug);
    for m in &info.members {
        if m["member"].as_str() != Some(me.as_str()) {
            continue;
        }
        push(m["handle"].as_str().unwrap_or(""));
        let name = m["project"]["name"]
            .as_str()
            .or_else(|| m["name"].as_str())
            .unwrap_or("");
        push(&name.trim().replace(' ', "-"));
    }
    out
}

/// This agent's mention role in `msg` over every handle it answers to (strongest wins). A typed
/// chip (`mentions: agent:<id>`) with no `@handle` in the prose counts as `Referenced`: the
/// composer resolved a name, but the sentence does not tell us how it was used.
pub(crate) fn my_mention_role(
    msg: &MessageView,
    info: &PeopleChatInfo,
    project_id: &str,
    slug: &str,
) -> MentionRole {
    let text = msg.text();
    let mut role = MentionRole::None;
    for h in agent_handles(info, project_id, slug) {
        role = role.max(mention_role(&text, &h));
    }
    if role == MentionRole::None && mentioned_agent_ids(msg).iter().any(|id| id == project_id) {
        role = MentionRole::Referenced;
    }
    role
}

/// Recent human and agent lines before `up_to` as `[{from, text}]`, oldest first, for the
/// responder decision and the group prelude. A human line is labelled with its author's display
/// name (roster or row), else their handle, else `a member` — never the owner by default.
#[cfg(test)]
pub(crate) fn recent_people_context(
    messages: &[MessageView],
    up_to: &str,
    max: usize,
    info: &PeopleChatInfo,
) -> Vec<Value> {
    recent_people_context_for(messages, up_to, max, info, "", &[])
}

/// `recent_people_context` with this agent's handles: a line that @mentions one of them, and
/// the message `up_to` replies to, are passed whole (`truncate::build_window`); every other
/// cut carries a marker naming the fetch call (`chat_messages`).
pub(crate) fn recent_people_context_for(
    messages: &[MessageView],
    up_to: &str,
    max: usize,
    info: &PeopleChatInfo,
    chat_id: &str,
    my_handles: &[String],
) -> Vec<Value> {
    let reply_to = messages
        .iter()
        .find(|m| m.id == up_to)
        .and_then(|m| m.rest.get("replyTo").and_then(Value::as_str))
        .map(str::to_owned);
    let mut lines = Vec::new();
    for m in messages {
        if m.id == up_to {
            break;
        }
        if m.role != "user" && m.role != "assistant" {
            continue;
        }
        let text = m.text();
        if text.trim().is_empty() {
            continue;
        }
        let from = if m.role == "assistant" {
            let handle = m
                .rest
                .get("senderHandle")
                .and_then(Value::as_str)
                .unwrap_or("agent");
            format!("@{handle} (agent)")
        } else {
            let author = info.author_of(m);
            author_spoken(m, info, author.as_deref())
        };
        let full = reply_to.as_deref() == Some(m.id.as_str())
            || my_handles
                .iter()
                .any(|h| mention_role(&text, h) != MentionRole::None);
        lines.push(crate::truncate::WindowLine {
            id: m.id.clone(),
            from,
            text,
            full,
        });
    }
    crate::truncate::build_window(chat_id, lines, max).0
}

impl Agent {
    /// After a rename (#1481): patch every cached people-chat row so `@newslug` still wakes this
    /// agent and the prelude / reply attribution speak the live name — without waiting for the
    /// next `/box/projects` listing.
    pub fn refresh_people_identity_after_rename(&self) {
        let me = format!("agent:{}", self.cfg.project_id);
        let handle = self.identity.handle();
        let name = self.name();
        let mut chats = self.people.chats.lock().expect("people");
        for info in chats.values_mut() {
            info.slug = handle.clone();
            for m in &mut info.members {
                if m["member"].as_str() != Some(me.as_str()) {
                    continue;
                }
                m["handle"] = json!(handle.clone());
                if let Some(obj) = m.as_object_mut() {
                    obj.insert("name".into(), json!(name.clone()));
                }
                if m.get("project").map(|p| p.is_object()).unwrap_or(false) {
                    m["project"]["name"] = json!(name.clone());
                }
            }
        }
    }

    /// The relay's list of people chats this agent is in (`people_chats` of `/box/projects`, or a
    /// single chat after an `people_agent_added` push): remember members and settings, open the
    /// rooms, and leave chats that are gone.
    pub async fn note_people_chats(self: &std::sync::Arc<Self>, list: &[Value], full: bool) {
        let me = format!("agent:{}", self.cfg.project_id);
        let mut keep = Vec::new();
        for c in list {
            let Some(chat_id) = c.get("id").and_then(Value::as_str).map(str::to_owned) else {
                continue;
            };
            let members = c
                .get("members")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mine = members
                .iter()
                .find(|m| m["member"].as_str() == Some(me.as_str()));
            // When this agent joined, from the relay's membership row: rows before that are
            // never work, also when the workspace itself booted after the add (TestAgent2).
            let joined_at = mine.and_then(|m| m["joined_at"].as_i64()).unwrap_or(0);
            let info = PeopleChatInfo {
                settings: mine
                    .and_then(|m| m.get("settings"))
                    .cloned()
                    .unwrap_or_else(|| json!({ "reply_policy": "mention" })),
                owner_user: mine
                    .and_then(|m| m["owner"].as_str())
                    .unwrap_or_default()
                    .to_owned(),
                slug: mine
                    .and_then(|m| m["handle"].as_str())
                    .unwrap_or_default()
                    .to_owned(),
                kind: c
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("dm")
                    .to_owned(),
                owner_address: self.people.owner_address_name(),
                members,
            };
            let fresh = self
                .people
                .chats
                .lock()
                .expect("people")
                .insert(chat_id.clone(), info)
                .is_none();
            keep.push(chat_id.clone());
            if self.sync.host(&chat_id).await.is_none() {
                let room = arbos_core::wire::room_name(PEOPLE_PROJECT, &chat_id);
                match self
                    .sync
                    .open_room_named(&chat_id, &room, PEOPLE_PROJECT)
                    .await
                {
                    Ok(_) => {
                        // The relay's membership time is the line: rows issued after it are
                        // work even when they landed before this box opened the room (a group
                        // created with its first message already in it, #2127/#2120). Only
                        // without that time does "now" stand in for it.
                        match people_join_marks(full, joined_at) {
                            JoinMarks::JoinedAt(at) => self.sync.mark_joined_at(&chat_id, at).await,
                            JoinMarks::LateJoinNow => self.sync.mark_late_join(&chat_id).await,
                            JoinMarks::None => {}
                        }
                        tracing::info!(chat = %chat_id, late = !full, joined_at, "joined a people chat");
                    }
                    Err(e) => tracing::warn!(chat = %chat_id, error = %e, "people room"),
                }
            }
            if fresh {
                self.append_audit(
                    "people_agent_joined",
                    json!({ "chat_id": chat_id, "members": self.people.info(&chat_id).map(|i| i.members.len()).unwrap_or(0) }),
                );
            }
        }
        if full {
            let gone: Vec<String> = self
                .people
                .chats
                .lock()
                .expect("people")
                .keys()
                .filter(|k| !keep.contains(k))
                .cloned()
                .collect();
            for chat_id in gone {
                self.people_agent_removed(&chat_id).await;
            }
        }
    }

    /// Fetch one people chat from the relay (after a `people_agent_added` push) and join it.
    /// Open one people chat's room on the relay's word (a `people_agent_added` or
    /// `people_chat_open` push): fetch its membership and join with the relay's joined_at as
    /// the line, so the row that triggered the push is work, not history.
    pub async fn join_people_chat(self: &std::sync::Arc<Self>, chat_id: &str) -> Result<()> {
        let r = self
            .http_client()
            .get(format!("{}/box/people/{chat_id}", self.cfg.relay_url))
            .bearer_auth(&self.cfg.box_token)
            .send()
            .await?;
        if !r.status().is_success() {
            anyhow::bail!("people chat {chat_id}: HTTP {}", r.status());
        }
        let v: Value = r.json().await?;
        self.note_people_chats(std::slice::from_ref(&v), false)
            .await;
        Ok(())
    }

    /// The owner removed this agent (or the chat was deleted): stop any turn there, leave the
    /// room, soft-invalidate what the agent remembered from the chat, keep the audit.
    pub async fn people_agent_removed(self: &std::sync::Arc<Self>, chat_id: &str) {
        let known = self
            .people
            .chats
            .lock()
            .expect("people")
            .remove(chat_id)
            .is_some();
        if !known {
            return;
        }
        let _ = self.handle_people_stop(chat_id).await;
        self.sync.close_room(chat_id).await;
        let invalidated = self
            .store
            .memory_invalidate_thread(chat_id, now_ms())
            .unwrap_or(0);
        self.people.askers.lock().expect("people").remove(chat_id);
        self.append_audit(
            "people_agent_removed",
            json!({ "chat_id": chat_id, "memory_invalidated": invalidated }),
        );
        tracing::info!(chat = %chat_id, invalidated, "left a people chat");
    }

    /// True when this agent already wrote an assistant reply for `user_msg_id` in this room.
    pub async fn people_already_replied(&self, host: &SharedHost, user_msg_id: &str) -> bool {
        if user_msg_id.is_empty() {
            return false;
        }
        let me = format!("agent:{}", self.cfg.project_id);
        let h = host.lock().await;
        // Sender only, never the device id: two agents of one box share `box:<sandbox>`, and
        // TestAgent's in-progress reply made TestAgent2 skip "@testagent2 I want you to send a
        // message to @testagent" as already answered (2026-10-24 00:48).
        h.doc.messages().iter().any(|m| {
            m.role == "assistant"
                && m.status != "cancelled"
                && m.rest.get("inReplyTo").and_then(Value::as_str) == Some(user_msg_id)
                && m.rest.get("sender").and_then(Value::as_str) == Some(me.as_str())
        })
    }

    /// Shared people `send` still waiting on another chosen agent: leave the command `taken` so
    /// their box can claim it. The judge's chosen set says who owes a reply; false when it
    /// picked one agent (or none), when every chosen agent has already replied, or when this
    /// box never judged the message (a restart in between).
    pub async fn people_shared_send_open(
        &self,
        chat_id: &str,
        host: &SharedHost,
        user_msg_id: &str,
    ) -> bool {
        if user_msg_id.is_empty() || !self.people.is_people_chat(chat_id) {
            return false;
        }
        let Some(verdict) = self.people.gate_verdict(user_msg_id) else {
            return false;
        };
        if verdict.chosen.len() <= 1 {
            return false;
        }
        let h = host.lock().await;
        let replied: std::collections::HashSet<String> = h
            .doc
            .messages()
            .iter()
            .filter(|m| {
                m.role == "assistant"
                    && m.status != "cancelled"
                    && m.rest.get("inReplyTo").and_then(Value::as_str) == Some(user_msg_id)
            })
            .filter_map(|m| {
                m.rest
                    .get("sender")
                    .and_then(Value::as_str)
                    .and_then(|s| s.strip_prefix("agent:"))
                    .map(str::to_owned)
            })
            .collect();
        verdict.chosen.iter().any(|id| !replied.contains(id))
    }

    /// Should this command drive a turn here? Every human message in a people chat goes to
    /// the judge (`responders.rs`): one call, the same on every box in the room, no rule before
    /// it. A message not for this agent is left alone (its command stays pending for whoever
    /// it is for). The verdict is cached per message, so a steer, a retry or a catch-up re-gate
    /// of the same message costs nothing.
    pub async fn people_wants(&self, chat_id: &str, host: &SharedHost, user_msg_id: &str) -> bool {
        let Some(info) = self.people.info(chat_id) else {
            return true;
        };
        if let Some(v) = self.people.gate_verdict(user_msg_id) {
            return v.wants;
        }
        let verdict = self.people_gate(chat_id, host, user_msg_id, &info).await;
        // The decision is part of the agent's record (`GET /projects/{id}/inspect`, kind
        // `gate`): "why did bob answer / stay quiet" is the first question in every routing bug.
        let _ = self.store.t3_append(
            chat_id,
            None,
            Some(chat_id),
            "gate",
            Some("system"),
            &format!(
                "gate {}: {}",
                if verdict.wants { "reply" } else { "skip" },
                verdict.reason
            ),
            Some(&json!({
                "message_id": user_msg_id, "reply": verdict.wants, "reason": verdict.reason,
                "chosen": verdict.chosen,
                "me": self.cfg.project_id, "handle": self.identity.handle(),
                "reply_policy": info.settings["reply_policy"], "agents": info.agents(), "group": info.is_group(),
            })),
        );
        if !user_msg_id.is_empty() {
            self.people.remember_verdict(user_msg_id, verdict.clone());
        }
        verdict.wants
    }

    /// The gate proper: the judge's verdict for this agent, with the reason that made it.
    async fn people_gate(
        &self,
        chat_id: &str,
        host: &SharedHost,
        user_msg_id: &str,
        info: &PeopleChatInfo,
    ) -> GateVerdict {
        let silent = |reason: String| GateVerdict {
            wants: false,
            reason,
            chosen: Vec::new(),
            woken_by_mention: None,
        };
        let mine = |reason: String| GateVerdict {
            wants: true,
            reason,
            chosen: vec![self.cfg.project_id.clone()],
            woken_by_mention: None,
        };
        let (msg, recent, reply_to) = {
            let h = host.lock().await;
            let Some(msg) = h.doc.message(user_msg_id) else {
                return silent("message not in the doc".into());
            };
            let all = h.doc.messages();
            let handles = agent_handles(info, &self.cfg.project_id, &self.identity.handle());
            let recent = recent_people_context_for(
                &all,
                user_msg_id,
                RESPOND_RECENT_MAX,
                info,
                chat_id,
                &handles,
            );
            // The quoted line, labelled like a recent line: the judge sees whom the message
            // answers and what it answers to.
            let reply_to = msg
                .rest
                .get("replyTo")
                .and_then(Value::as_str)
                .and_then(|id| h.doc.message(id))
                .map(|r| {
                    let from = if r.role == "assistant" {
                        format!(
                            "@{} (agent)",
                            r.rest
                                .get("senderHandle")
                                .and_then(Value::as_str)
                                .unwrap_or("agent")
                        )
                    } else {
                        author_spoken(&r, info, info.author_of(&r).as_deref())
                    };
                    json!({ "from": from, "text": crate::truncate::clip_marked(&r.text(), 500, &r.id, None) })
                });
            (msg, recent, reply_to)
        };
        // An agent's bubble never wakes another agent, whatever it says: "@lane-a" in lane-b's
        // prose (quoting the user, explaining what it could do) is not a send (#2207, long-task
        // h4: the judge chose lane-b alone, then lane-a answered the dispatched mention). The
        // only agent-to-agent path is the `send_message` tool (teammate send, 2026-10-23 rule).
        if msg.role == "assistant" {
            return silent(
                "another agent's row: agents reach each other only through send_message".into(),
            );
        }
        let sender_is_human = msg
            .rest
            .get("sender")
            .and_then(Value::as_str)
            .is_none_or(|s| s.starts_with("user:") || s == "user");
        if msg.role != "user" || !sender_is_human {
            return silent("not a human row".into());
        }
        // The owner's explicit setting, not a heuristic.
        if info.settings["reply_policy"].as_str() == Some("every_message") {
            return mine("reply_policy every_message".into());
        }
        let raw_text = msg.text();
        let text = crate::truncate::clip_marked(&raw_text, 4000, &msg.id, None);
        if text.trim().is_empty() {
            return silent("empty message".into());
        }
        // Prefer the live identity handle over a stale people-chat snapshot (#1481).
        let slug = {
            let live = self.identity.handle();
            if live.is_empty() {
                info.slug.clone()
            } else {
                live
            }
        };
        let my_role = my_mention_role(&msg, info, &self.cfg.project_id, &slug);
        let chips = mentioned_agent_ids(&msg);
        let asker = author_spoken(&msg, info, info.author_of(&msg).as_deref());
        let mut candidates: Vec<Candidate> = info
            .members
            .iter()
            .filter_map(|m| {
                let id = m["member"].as_str()?.strip_prefix("agent:")?;
                let handle = if id == self.cfg.project_id {
                    slug.as_str()
                } else {
                    m["handle"].as_str().unwrap_or(id)
                };
                let role = m["description"]
                    .as_str()
                    .filter(|d| !d.trim().is_empty())
                    .or_else(|| m["project"]["description"].as_str())
                    .unwrap_or("")
                    .to_owned();
                let name = m["project"]["name"]
                    .as_str()
                    .or_else(|| m["name"].as_str())
                    .unwrap_or("")
                    .trim()
                    .replace(' ', "-");
                let mut mentioned =
                    mention_role(&raw_text, handle).max(mention_role(&raw_text, &name));
                if mentioned == MentionRole::None && chips.iter().any(|c| c == id) {
                    mentioned = MentionRole::Referenced;
                }
                Some(Candidate {
                    id: id.to_owned(),
                    name: format!("@{handle}"),
                    role,
                    host: false,
                    mentioned_as: (mentioned != MentionRole::None).then(|| mentioned.as_str()),
                })
            })
            .collect();
        // Agent-id order on every box: the tie-break depends on it.
        candidates.sort_by(|a, b| a.id.cmp(&b.id));
        if !candidates.iter().any(|c| c.id == self.cfg.project_id) {
            return silent("not a member of this chat's roster".into());
        }
        let mentioned: Vec<String> = candidates
            .iter()
            .filter(|c| {
                c.mentioned_as.is_some()
                    || (c.id == self.cfg.project_id && my_role != MentionRole::None)
            })
            .map(|c| c.id.clone())
            .collect();
        let room = Room {
            kind: "people_group",
            chat_id: chat_id.to_owned(),
            title: info.kind.clone(),
            from: asker,
            text,
            recent,
            candidates,
            people: info.people_labels(),
            reply_to,
        };
        match self.decide_responders(&room).await {
            Ok(d) => {
                self.log_responder_decision(&room, &msg.id, Ok(&d), None);
                let wants = d.chose(&self.cfg.project_id);
                let verdict = GateVerdict {
                    wants,
                    reason: d.reason_for(&self.cfg.project_id, &room),
                    chosen: d.chosen.clone(),
                    woken_by_mention: None,
                }
                .honour_mentions(&self.cfg.project_id, my_role, &mentioned);
                self.report_gate_decision(chat_id, &msg.id, &verdict, Some(&d));
                verdict
            }
            Err(e) => {
                let error = e.to_string();
                // A budget refusal is not an outage: say so once in the chat and stop asking
                // the model for every message until the block lapses (`budget.rs`).
                if crate::budget::Refusal::classify(&error).is_some() {
                    self.note_budget_refusal(chat_id, &error, false).await;
                }
                // No second model. Unmentioned: silence (fail closed). Mentioned by a person:
                // woken, like every @mention (fail open).
                let verdict = GateVerdict {
                    wants: false,
                    reason: if my_role == MentionRole::None {
                        format!("judge failed: {error}; unmentioned → silent (fail closed)")
                    } else {
                        format!("judge failed: {error}")
                    },
                    chosen: Vec::new(),
                    woken_by_mention: None,
                }
                .honour_mentions(&self.cfg.project_id, my_role, &mentioned);
                let action = if verdict.wants { "REPLY" } else { "NONE" };
                self.log_responder_decision(
                    &room,
                    &msg.id,
                    Err(&error),
                    Some((action, &verdict.chosen)),
                );
                self.report_gate_decision(chat_id, &msg.id, &verdict, None);
                verdict
            }
        }
    }

    /// Tell the relay what this gate decided (`POST /box/people/{chat}/gate`), so room health
    /// (`GET /admin/room-health/{chat}`, `GET /chats/{id}/health`) can show every agent's
    /// decision and reason per message. Fire-and-forget; a relay without the route (404) is
    /// fine.
    fn report_gate_decision(
        &self,
        chat_id: &str,
        user_msg_id: &str,
        verdict: &GateVerdict,
        decision: Option<&crate::responders::ResponderDecision>,
    ) {
        let body = json!({
            "message_id": user_msg_id,
            "agent_id": self.cfg.project_id,
            "handle": self.identity.handle(),
            "decision": if verdict.wants { "reply" } else { "silent" },
            "reason": verdict.reason,
            "chosen": verdict.chosen,
            "confidence": decision.map(|d| d.confidence),
            "addressee": decision.map(|d| d.addressee.clone()),
            "model": decision.map(|d| d.model.clone()),
            "latency_ms": decision.map(|d| d.latency_ms),
            "cost_usd": decision.and_then(|d| d.cost_usd),
            "at": now_ms(),
        });
        let url = format!("{}/box/people/{chat_id}/gate", self.cfg.relay_url);
        let client = self.http_client().clone();
        let token = self.cfg.box_token.clone();
        let chat = chat_id.to_owned();
        tokio::spawn(async move {
            match client
                .post(url)
                .bearer_auth(token)
                .timeout(std::time::Duration::from_secs(5))
                .json(&body)
                .send()
                .await
            {
                Ok(r) if r.status().is_success() || r.status().as_u16() == 404 => {}
                Ok(r) => tracing::debug!(chat = %chat, status = %r.status(), "gate report refused"),
                Err(e) => tracing::debug!(chat = %chat, error = %e, "gate report failed"),
            }
        });
    }
    /// Prelude for the prompt and the asker for attribution, when `chat_id` is a people chat.
    /// In a group (several humans or several agents), the shared transcript is injected so this
    /// agent can read every human and agent line — not only the messages that addressed it
    /// (its own pi session only has the turns it ran).
    pub async fn people_turn_context(
        &self,
        chat_id: &str,
        host: &SharedHost,
        user_msg_id: &str,
    ) -> Option<String> {
        let info = self.people.info(chat_id)?;
        let (msg, recent) = {
            let h = host.lock().await;
            let msg = h.doc.message(user_msg_id)?;
            let recent = if info.is_group() {
                let handles = agent_handles(&info, &self.cfg.project_id, &self.identity.handle());
                recent_people_context_for(
                    &h.doc.messages(),
                    user_msg_id,
                    RESPOND_RECENT_MAX,
                    &info,
                    chat_id,
                    &handles,
                )
            } else {
                Vec::new()
            };
            (msg, recent)
        };
        // Who is asking: the row's `sender`, its device, its handle, or the only human in the
        // chat. An author the row does not identify is "a member" with guest powers — the owner
        // is never assumed (#1580).
        let author = info.author_of(&msg);
        let handle = author_handle(&msg, &info, author.as_deref());
        // The owner's spoken name comes from their relay profile (PATCH /me), not the row's
        // `senderName` (one device's label); guests keep the member card / row name.
        let owner_address = self.people.owner_address_name();
        let spoken = match (&author, &owner_address) {
            (Some(a), Some(n)) if *a == info.owner_user => n.clone(),
            _ => author_spoken(&msg, &info, author.as_deref()),
        };
        let role = match author.as_deref() {
            Some(uid) if uid == info.owner_user => "owner",
            Some(_) | None => "guest",
        };
        let asker = Asker {
            user_id: author.clone().unwrap_or_else(|| UNKNOWN_ASKER.to_owned()),
            handle: handle.clone(),
            name: spoken.clone(),
            role,
            message_id: user_msg_id.to_owned(),
        };
        self.people
            .askers
            .lock()
            .expect("people")
            .insert(chat_id.to_owned(), asker);
        self.people.set_current(Some(chat_id));
        let people: Vec<String> = info
            .members
            .iter()
            .filter(|m| m["member"].as_str().is_some_and(|s| s.starts_with("user:")))
            .map(|m| {
                let spoken = person_spoken_name(m);
                if m["member"].as_str() == Some(&format!("user:{}", info.owner_user)) {
                    format!("{spoken} (owner, your Project's user)")
                } else {
                    format!("{spoken} (guest)")
                }
            })
            .collect();
        let me_agent = format!("agent:{}", self.cfg.project_id);
        let other_agents: Vec<String> = info
            .members
            .iter()
            .filter(|m| {
                m["member"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("agent:") && s != me_agent)
            })
            .map(|m| {
                let h = m["handle"].as_str().unwrap_or("?");
                match m["description"]
                    .as_str()
                    .map(str::trim)
                    .filter(|d| !d.is_empty())
                {
                    Some(d) => {
                        let short: String = d.chars().take(160).collect();
                        format!("@{h} ({short})")
                    }
                    None => format!("@{h}"),
                }
            })
            .collect();
        let others_clause = if other_agents.is_empty() {
            String::new()
        } else {
            format!(
                "; other agents present (the reply gate decides who answers a person's message; writing @their-handle in your reply does not reach them — use send_message when you need them): {}",
                other_agents.join(", ")
            )
        };
        let visibility = if info.is_group() {
            " You can read the whole shared transcript — every human and every agent message in this chat, not only ones that address you."
        } else {
            ""
        };
        let owner_spoken =
            info.spoken_name_of_with_owner(&info.owner_user, owner_address.as_deref());
        let secrets_rule = if role == "guest" {
            " Do not use the owner's secrets (egress, browser fill, or real values in the environment) for this person unless they are on that secret's People list (secrets_list shows it); when a secret's others_ask is card, ask through secrets_request and wait for the owner's card instead; if a person's note sets a limit, honour it."
        } else {
            ""
        };
        // Live identity handle — not the join-time snapshot — so a mid-group rename does not
        // tell the model it is still @oldname (#1481 / #1431).
        let my_slug = {
            let live = self.identity.handle();
            if live.is_empty() {
                info.slug.clone()
            } else {
                live
            }
        };
        // An unidentified author is named as such: the model must not fill the gap with the
        // owner (nor with anyone else in the roster).
        let from_clause = match (&author, spoken.as_str()) {
            (Some(_), _) => format!(
                "This message is from {spoken} ({role}); treat it as a request from another person in the conversation and address them by their display name ({spoken}), never as an @handle"
            ),
            (None, UNKNOWN_SPOKEN) => format!(
                "This message is from a person in this chat whose account the row does not name — treat them as a guest, not as {owner_spoken}, and if asked who wrote it say the message does not say; address them plainly, never as an @handle"
            ),
            (None, _) => format!(
                "This message is from {spoken} (not matched to a member of this chat; treat them as a guest, not as {owner_spoken}); address them as {spoken}, never as an @handle"
            ),
        };
        // The judge already decided this message is for this agent (`people_gate`); the prompt
        // must not second-guess it. Proof run 2026-10-28 02:12 (image 0.1.170): the gate said
        // reply to "To any of the available agents: what is the capital of Portugal?" and to
        // "Hezbola-rg? 😂", and the talking model then routed an empty reply because the old
        // sentence told it to stay silent when "only mentioned". Silence stays available for a
        // genuine nothing-to-say, and a silence announcement is still never posted.
        let reply_clause = match self.people.gate_verdict(user_msg_id) {
            // Woken by the mention alone (the judge was unsure, picked someone else, or failed):
            // the agent reads the line itself; "thanks @you" or "@you has the notes" still
            // gets no post (eval g7).
            Some(GateVerdict {
                wants: true,
                woken_by_mention: Some(_),
                ..
            }) => format!(
                "{spoken} @mentioned you; a person's @mention always reaches you, even when the reply gate was unsure. Read it and do your part: if it asks you anything or asks you to do something, answer or act (\"talk to @another-agent\" means reach that agent with send_message); a bare name ping (\"{my_slug}?\", \"hey {my_slug}\") wants a one-line presence answer. If it only thanks you, talks about you or passes something along, route `reply` with `reply` empty and no words — never post that you are staying silent, that the question is for someone else, or that you have nothing to add"
            ),
            Some(v) if v.wants => format!(
                "The reply gate has already judged that this message is for you ({}); answer it plainly. A bare name ping (\"{my_slug}?\", \"hey {my_slug}\") wants a one-line presence answer, not silence. Only if you truly have nothing to say, route `reply` with `reply` empty and no words — never post that you are staying silent, that the question is for someone else, or that you have nothing to add",
                v.reason
            ),
            _ => "If this message is for someone else, or only mentions or refers to you without asking you anything, say nothing: route `reply` with `reply` empty and no words — never post that you are staying silent, that the question is for someone else, or that you have nothing to add".to_owned(),
        };
        let mut prelude = format!(
            "[People chat with {}. You are @{my_slug} (the agent of {owner_spoken}'s Project \"{}\"){}.{} {from_clause}. Your reply is read by everyone in the chat. {reply_clause}.{secrets_rule}]\n",
            people.join(", "),
            self.cfg.project_id,
            others_clause,
            visibility,
        );
        if !recent.is_empty() {
            prelude.push_str("Recent messages in this shared chat:\n");
            prelude.push_str(&crate::truncate::render_window(&recent));
            prelude.push_str(&crate::truncate::window_note(chat_id));
            prelude.push('\n');
        }
        prelude.push('\n');
        Some(prelude)
    }

    /// The recent lines of people chat `chat_id`, rendered for a prompt, when this agent is a
    /// member of it: what another agent's `send_message` from that chat was about (Arbonites
    /// 2026-10-28: Bubble's "Jacob asked me to talk to you" reached Bob with nothing of the group,
    /// so Bob asked "What's on Jacob's mind?"). `None` for chats this agent is not in — a
    /// sender's room is never shown to a non-member.
    pub(crate) async fn people_chat_window(&self, chat_id: &str) -> Option<String> {
        let info = self.people.info(chat_id)?;
        let host = self.sync.host(chat_id).await?;
        let handles = agent_handles(&info, &self.cfg.project_id, &self.identity.handle());
        let recent = {
            let h = host.lock().await;
            recent_people_context_for(
                &h.doc.messages(),
                "",
                RESPOND_RECENT_MAX,
                &info,
                chat_id,
                &handles,
            )
        };
        if recent.is_empty() {
            return None;
        }
        Some(format!(
            "{}{}",
            crate::truncate::render_window(&recent),
            crate::truncate::window_note(chat_id)
        ))
    }

    /// Projects together (D72 step 3): in this Project's own chats, a message written by a guest
    /// the owner invited is labelled so the agent knows who is talking. `None` for the owner's
    /// own messages and for people chats (which have their own prelude).
    pub async fn shared_project_guest_note(
        &self,
        chat_id: &str,
        host: &SharedHost,
        user_msg_id: &str,
    ) -> Option<String> {
        if self.people.is_people_chat(chat_id) {
            return None;
        }
        let msg = host.lock().await.doc.message(user_msg_id)?;
        let sender = msg.rest.get("sender").and_then(Value::as_str)?;
        let user_id = sender.strip_prefix("user:")?;
        let owner = self.people.project_owner();
        if owner.is_empty() || user_id == owner {
            return None;
        }
        let handle = msg
            .rest
            .get("senderHandle")
            .and_then(Value::as_str)
            .unwrap_or(user_id);
        let spoken = message_spoken_name(&msg, handle);
        self.people.askers.lock().expect("people").insert(
            chat_id.to_owned(),
            Asker {
                user_id: user_id.to_owned(),
                handle: handle.to_owned(),
                name: spoken.clone(),
                role: "guest",
                message_id: user_msg_id.to_owned(),
            },
        );
        self.people.set_current(Some(chat_id));
        Some(format!(
            "[This message is from {spoken}, a person the owner of this Project invited to work in it with you. Address them by their display name ({spoken}), never as an @handle. They may direct ordinary work here; do not use the owner's secrets for them unless the owner put them on that secret's People list (or, when its others_ask is card, the owner answered your secrets_request card), and honour any note on their row (for example a daily credit limit).]\n\n"
        ))
    }

    /// In this Project's own chats: tell the model the owner's display name so it addresses them
    /// by name, not `@handle`. Call only when people/guest preludes did not already run.
    /// `None` when the message carries neither a usable `senderName` nor a `senderHandle`.
    pub async fn owner_address_note(
        &self,
        chat_id: &str,
        host: &SharedHost,
        user_msg_id: &str,
    ) -> Option<String> {
        if self.people.is_people_chat(chat_id) {
            return None;
        }
        let msg = host.lock().await.doc.message(user_msg_id)?;
        // The address comes from the relay profile, live (P0 San José, Bubble "Hello, Const":
        // the row's `senderName` is whatever that device's client sent — one phone said Const,
        // another Arbos — so it only tells us a person wrote; the profile says what to call them).
        let profile = self.people.owner_address_name();
        let row_name = msg.rest.get("senderName").and_then(Value::as_str);
        let handle = msg.rest.get("senderHandle").and_then(Value::as_str);
        if profile.is_none() && row_name.is_none() && handle.is_none() {
            return None;
        }
        owner_address_note_text(profile.as_deref(), handle, &self.name())
    }
}

/// The name the agent should call the user. The display name wins unless it is empty or is
/// the product's name or the agent's own name (a GitHub profile called "Arbos" produced
/// "Hello, Arbos" — feedback 2026-10-22); then the bare handle, if any.
pub fn user_address_name(
    sender_name: Option<&str>,
    sender_handle: Option<&str>,
    agent_name: &str,
) -> Option<String> {
    let clean = |s: Option<&str>| {
        s.map(str::trim)
            .map(|s| s.trim_start_matches('@'))
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let usable =
        |n: &str| !n.eq_ignore_ascii_case("arbos") && !n.eq_ignore_ascii_case(agent_name.trim());
    // A login handle is never a name (Grok parity, traits 2/11: "Here's a list, unarbos"): with
    // no usable display name the agent uses no name at all.
    let _ = sender_handle;
    clean(sender_name).filter(|n| usable(n))
}

/// The per-turn identity note for the project owner: who the user is, and that the agent's
/// name is a name (not a place: "What do you want to work on in Doug?" — same feedback).
pub fn owner_address_note_text(
    sender_name: Option<&str>,
    sender_handle: Option<&str>,
    agent_name: &str,
) -> Option<String> {
    let agent = agent_name.trim();
    let about_me = if agent.is_empty() || agent.eq_ignore_ascii_case("arbos") {
        String::new()
    } else {
        format!(
            " You are {agent}; \"{agent}\" is your own name, not a place, workspace or product — never say \"in {agent}\" or \"on {agent}\"."
        )
    };
    let present = |v: Option<&str>| v.is_some_and(|h| !h.trim().trim_start_matches('@').is_empty());
    Some(match user_address_name(sender_name, sender_handle, agent_name) {
        Some(name) => format!(
            "[The user is {name}. Address them as {name}, never as an @handle and never as \"Arbos\" (Arbos is the app).{about_me}]\n\n"
        ),
        None if present(sender_handle) || present(sender_name) => format!(
            "[The user's display name is not set: do not address them by any name, handle or login — just talk to them.{about_me}]\n\n"
        ),
        None => return None,
    })
}

#[cfg(test)]
mod address_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn display_name_wins_when_it_is_a_real_name() {
        assert_eq!(
            user_address_name(Some("Jacob"), Some("unarbos"), "Doug").as_deref(),
            Some("Jacob")
        );
        let note = owner_address_note_text(Some(" Jacob "), Some("unarbos"), "Doug").unwrap();
        assert!(note.starts_with("[The user is Jacob. Address them as Jacob"));
        assert!(note.contains("never as \"Arbos\""));
        assert!(note.contains("You are Doug; \"Doug\" is your own name, not a place"));
        assert!(note.ends_with("]\n\n"));
    }

    /// Grok parity (traits 2/11, 2026-10-24): "Here's a 3-day surf trip list, unarbos:" — a login
    /// is never a name. Jacob's `unarbos` GitHub profile is literally named "Arbos".
    #[test]
    fn a_handle_is_never_used_as_the_name() {
        assert_eq!(
            user_address_name(Some("Arbos"), Some("unarbos"), "Doug"),
            None
        );
        assert_eq!(
            user_address_name(Some("arbos"), Some("@unarbos"), "Doug"),
            None
        );
        assert_eq!(user_address_name(None, Some("unarbos"), "Doug"), None);
        let note = owner_address_note_text(Some("Arbos"), Some("unarbos"), "Doug").unwrap();
        assert!(note
            .contains("display name is not set: do not address them by any name, handle or login"));
        assert!(!note.contains("unarbos"));
        assert!(!note.contains("Address them as Arbos"));
        assert!(note.contains("You are Doug"));
        let note = owner_address_note_text(None, Some("unarbos"), "Doug").unwrap();
        assert!(note.contains("do not address them by any name"));
    }

    #[test]
    fn agent_name_as_display_name_is_not_a_name_either() {
        assert_eq!(user_address_name(Some("Doug"), Some("jake"), "Doug"), None);
        assert_eq!(user_address_name(Some("Arbos"), Some("Doug"), "Doug"), None);
        // Nothing on the row at all: no note.
        assert_eq!(owner_address_note_text(None, None, "Doug"), None);
        assert_eq!(user_address_name(None, None, "Doug"), None);
        assert_eq!(user_address_name(Some("  "), Some(""), "Doug"), None);
        assert_eq!(owner_address_note_text(Some("  "), Some(""), "Doug"), None);
    }

    /// P0 Bubble "Hello, Const" (2026-10-26): the owner is addressed from the relay profile,
    /// never from the row's `senderName` (one device's label) and never from the handle.
    #[test]
    fn owner_is_addressed_from_the_profile_not_the_row() {
        let people = PeopleState::default();
        // No profile yet: a row with only a GitHub-seeded senderName gives no name.
        assert_eq!(people.owner_address_name(), None);
        people.set_owner_profile(&json!({ "id": "gh_32490803", "login": "const", "name": "Const", "address_name": null }));
        assert_eq!(
            people.owner_address_name().as_deref(),
            Some("Const"),
            "no user-set name: the profile label"
        );
        people.set_owner_profile(&json!({ "id": "gh_32490803", "login": "const", "name": "Jacob", "address_name": "Jacob" }));
        assert_eq!(people.owner_address_name().as_deref(), Some("Jacob"));
        // The note is built from the profile; the row said "Const".
        let note = owner_address_note_text(
            people.owner_address_name().as_deref(),
            Some("const"),
            "Bubble",
        )
        .unwrap();
        assert!(note.starts_with("[The user is Jacob. Address them as Jacob"));
        assert!(!note.contains("Const"));
        assert!(!note.contains("const."), "the login is never the address");
        // Blank fields are not names.
        people.set_owner_profile(&json!({ "id": "x", "name": "  ", "address_name": "" }));
        assert_eq!(people.owner_address_name(), None);
    }

    #[test]
    fn note_skips_the_self_line_when_the_agent_is_unnamed() {
        let note = owner_address_note_text(Some("Jacob"), None, "Arbos").unwrap();
        assert!(note.contains("The user is Jacob"));
        assert!(!note.contains("You are Arbos"));
        let note = owner_address_note_text(Some("Jacob"), None, "").unwrap();
        assert!(!note.contains("You are "));
    }
}

impl Agent {
    /// Fields on a reply in a people chat (called right after the assistant message starts).
    ///
    /// Always writes the agent's sender / handle / name / avatar so clients put the bubble
    /// on the avatar column (#1604). Asker fields (`askedBy*`, `inReplyTo`, `replyTo`) are
    /// added when this chat still has an asker — a late subagent-result turn may run after
    /// the asker map was cleared, and must still look like this agent.
    pub fn people_reply_fields(&self, chat_id: &str) -> Option<ReplyAttribution> {
        let info = self.people.info(chat_id)?;
        let asker = self.people.asker(chat_id);
        let mine = info
            .members
            .iter()
            .find(|m| m["member"].as_str() == Some(&format!("agent:{}", self.cfg.project_id)));
        let avatar = mine.and_then(|m| m["avatar"].as_str()).map(str::to_owned);
        let avatar_color = mine
            .and_then(|m| m["avatar_color"].as_str())
            .map(str::to_owned);
        // Live identity first (#1481): a mid-group rename must not attribute the reply under
        // the join-time name/handle. Fall back to the member row only when identity is empty.
        let name = {
            let live = self.name();
            if live.trim().is_empty() {
                mine.and_then(|m| m["project"]["name"].as_str().or_else(|| m["name"].as_str()))
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_default()
            } else {
                live
            }
        };
        let handle = {
            let live = self.identity.handle();
            if live.is_empty() {
                info.slug.clone()
            } else {
                live
            }
        };
        let mut fields = vec![
            ("sender", json!(format!("agent:{}", self.cfg.project_id))),
            ("senderHandle", json!(handle)),
            ("senderName", json!(name)),
            ("senderAvatar", json!(avatar)),
            ("senderAvatarColor", json!(avatar_color)),
            ("ownerHandle", json!(info.owner_handle())),
        ];
        // Asker fields when this chat still has an asker — a late subagent-result
        // turn may run after the asker map was cleared (#1604). An asker the row
        // did not identify gets no `askedBy` (there is no account to point at);
        // the handle label still says who the row claimed to be, if anything.
        if let Some(asker) = asker {
            if asker.user_id != UNKNOWN_ASKER {
                fields.push(("askedBy", json!(format!("user:{}", asker.user_id))));
            }
            fields.extend([
                ("askedByHandle", json!(asker.handle)),
                ("inReplyTo", json!(asker.message_id)),
                // Platform reply feature: thread this bubble to the asker's message
                // so everyone sees which line is being answered (feedback
                // app-01M5MPKJ9K6HGHFH6BZWR9CME0).
                ("replyTo", json!(asker.message_id)),
            ]);
        }
        Some(ReplyAttribution { fields })
    }

    /// The agent was removed while answering here: abort that turn.
    async fn handle_people_stop(self: &std::sync::Arc<Self>, chat_id: &str) -> Result<()> {
        let running = self
            .turn
            .lock()
            .await
            .as_ref()
            .is_some_and(|t| t.chat_id == chat_id);
        if running {
            self.stop_turn(chat_id).await?;
        }
        Ok(())
    }
}

/// What a freshly opened people room is told about its past (#2127/#2120). `full` = the room
/// came from the boot/registry listing; `joined_at` = the relay's membership time (0 = unknown).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum JoinMarks {
    /// Commands issued before the relay's membership time are never run; everything after it is.
    JoinedAt(i64),
    /// No membership time from the relay and the add came while running: "now" is the line.
    LateJoinNow,
    /// A listing at boot with no membership time: the room's whole past is replayed as usual.
    None,
}

pub(crate) fn people_join_marks(full: bool, joined_at: i64) -> JoinMarks {
    if joined_at > 0 {
        JoinMarks::JoinedAt(joined_at)
    } else if !full {
        JoinMarks::LateJoinNow
    } else {
        JoinMarks::None
    }
}

#[cfg(test)]
mod tests {
    //! Mention routing: which human messages a shared agent answers.

    use serde_json::json;

    use super::*;

    /// #2127/#2120: a push that opens a people room must not discard the row that caused it —
    /// the relay's joined_at is the line, "now" only when the relay gave none.
    #[test]
    fn a_push_join_keeps_rows_issued_after_the_membership_time() {
        // people_agent_added / people_chat_open push, relay knows when we joined
        assert_eq!(people_join_marks(false, 1_700), JoinMarks::JoinedAt(1_700));
        // boot/registry listing, relay knows
        assert_eq!(people_join_marks(true, 1_700), JoinMarks::JoinedAt(1_700));
        // push with no membership time: the old rule, now is the line
        assert_eq!(people_join_marks(false, 0), JoinMarks::LateJoinNow);
        // boot listing with no membership time: nothing to mark
        assert_eq!(people_join_marks(true, 0), JoinMarks::None);
    }

    pub(super) fn msg(text: &str, extra: Value) -> MessageView {
        let mut rest = json!({ "sender": "user:gh_2", "senderHandle": "sam" });
        if let (Some(r), Some(e)) = (rest.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                r.insert(k.clone(), v.clone());
            }
        }
        MessageView {
            id: "m1".into(),
            role: "user".into(),
            status: "done".into(),
            parts: vec![json!({ "kind": "text", "text": text })],
            rest,
        }
    }

    const P: &str = "01HPROJ";

    /// #1580: a guest's row with no `sender: user:<id>` / `senderHandle` (older app, arbosctl)
    /// was read back as the owner's. The author comes from the row, its device, its handle, or
    /// the only human in the chat — and is otherwise unknown, never the owner.
    #[test]
    fn unlabelled_rows_never_resolve_to_the_owner() {
        let group = PeopleChatInfo {
            members: vec![
                json!({ "member": "user:gh_1", "handle": "jacob", "name": "Jacob", "devices": ["DEVJ1", "DEVJ2"] }),
                json!({ "member": "user:gh_2", "handle": "sam", "name": "Sam Okafor", "devices": ["DEVSAM"] }),
                json!({ "member": "agent:01HPROJ", "handle": "bob" }),
            ],
            owner_user: "gh_1".into(),
            slug: "bob".into(),
            kind: "group".into(),
            ..Default::default()
        };
        let bare = |extra: Value| {
            let mut m = msg("we agreed the launch date is 12 November", json!({}));
            m.rest = json!({ "sender": "user" });
            if let (Some(r), Some(e)) = (m.rest.as_object_mut(), extra.as_object()) {
                for (k, v) in e {
                    r.insert(k.clone(), v.clone());
                }
            }
            m
        };

        // Explicit sender wins.
        let labelled = msg("hi", json!({}));
        assert_eq!(group.author_of(&labelled).as_deref(), Some("gh_2"));
        assert_eq!(author_handle(&labelled, &group, Some("gh_2")), "sam");
        assert_eq!(author_spoken(&labelled, &group, Some("gh_2")), "Sam Okafor");

        // The guest's device (with or without the `dev:` prefix) names the guest.
        let by_device = bare(json!({ "deviceId": "dev:DEVSAM" }));
        assert_eq!(group.author_of(&by_device).as_deref(), Some("gh_2"));
        assert_eq!(
            group
                .author_of(&bare(json!({ "deviceId": "DEVJ2" })))
                .as_deref(),
            Some("gh_1")
        );
        // A handle alone still finds the member.
        let by_handle = bare(json!({ "deviceId": "dev:anon", "senderHandle": "Sam" }));
        assert_eq!(group.author_of(&by_handle).as_deref(), Some("gh_2"));

        // Unknown device, no handle, two humans: nobody — and not the owner.
        let unknown = bare(json!({ "deviceId": "dev:anon" }));
        assert_eq!(group.author_of(&unknown), None);
        assert_eq!(author_handle(&unknown, &group, None), UNKNOWN_SPOKEN);
        assert_eq!(author_spoken(&unknown, &group, None), UNKNOWN_SPOKEN);
        // A handle the roster lacks is still kept as the label (never swapped for the owner).
        let stranger = bare(json!({ "deviceId": "dev:anon", "senderHandle": "priya" }));
        assert_eq!(group.author_of(&stranger), None);
        assert_eq!(author_handle(&stranger, &group, None), "priya");
        assert_eq!(author_spoken(&stranger, &group, None), "priya");
        let named = bare(json!({ "senderName": "Priya N." }));
        assert_eq!(author_spoken(&named, &group, None), "Priya N.");

        // The recent-context lines use the same resolution.
        let mut lines = vec![by_device.clone(), unknown.clone(), labelled.clone()];
        for (i, m) in lines.iter_mut().enumerate() {
            m.id = format!("m{i}");
        }
        let ctx = recent_people_context(&lines, "none", 10, &group);
        assert_eq!(ctx[0]["from"], "Sam Okafor");
        assert_eq!(ctx[1]["from"], UNKNOWN_SPOKEN);
        assert_eq!(ctx[2]["from"], "Sam Okafor");

        // A DM whose only human is the owner: an unlabelled row is theirs.
        let dm = PeopleChatInfo {
            members: vec![
                json!({ "member": "user:gh_1", "handle": "jacob", "name": "Jacob" }),
                json!({ "member": "agent:01HPROJ", "handle": "bob" }),
            ],
            owner_user: "gh_1".into(),
            slug: "bob".into(),
            kind: "dm".into(),
            ..Default::default()
        };
        assert_eq!(dm.author_of(&unknown).as_deref(), Some("gh_1"));
        // Two humans and no roster devices: still unknown.
        let no_devices = PeopleChatInfo {
            members: vec![
                json!({ "member": "user:gh_1", "handle": "jacob" }),
                json!({ "member": "user:gh_2", "handle": "sam" }),
            ],
            owner_user: "gh_1".into(),
            ..Default::default()
        };
        assert_eq!(no_devices.author_of(&unknown), None);
    }

    /// #1538: Jacob (@unarbos) named an agent "unarbos", and the composer resolved `@unarbos`
    /// to the person. The mention classifier still reads `@unarbos` as a vocative for the agent
    /// of that slug; the people roster the judge sees names the person as `Arbos (@unarbos)`,
    /// so the collision is in front of the model instead of settled by a rule.
    #[test]
    fn sender_own_handle_still_reads_as_a_mention_of_the_agent() {
        let arbonites = PeopleChatInfo {
            members: vec![
                json!({ "member": "agent:01HPROJ", "handle": "unarbos", "project": { "name": "unarbos" } }),
                json!({ "member": "user:gh_2", "handle": "unarbos", "name": "Arbos" }),
                json!({ "member": "user:gh_9", "handle": "A", "name": "Scout" }),
            ],
            slug: "unarbos".into(),
            kind: "group".into(),
            ..Default::default()
        };
        let m = msg(
            "@unarbos are you there?",
            json!({ "mentions": ["user:gh_2"] }),
        );
        assert_eq!(
            my_mention_role(&m, &arbonites, P, "unarbos"),
            MentionRole::Addressed
        );
        assert_eq!(
            arbonites.people_labels(),
            vec!["Arbos (@unarbos)".to_string(), "Scout (@A)".to_string()]
        );
        // A typed agent chip with no handle in the prose: referenced, for the judge to weigh.
        assert_eq!(
            my_mention_role(
                &msg("hey", json!({ "mentions": ["agent:01HPROJ"] })),
                &arbonites,
                P,
                "unarbos"
            ),
            MentionRole::Referenced
        );
    }

    /// Hezbola, 2026-10-26: "the chat uses two names for the owner, Arbos and Jacob". Old rows
    /// carried `senderName: "Arbos"` (the GitHub-seeded label of the time), newer ones "Jacob";
    /// the transcript lines must name one owner, by id, with the current profile name.
    #[test]
    fn one_owner_one_current_name_across_the_transcript() {
        let info = PeopleChatInfo {
            members: vec![
                json!({ "member": "user:gh_268993031", "handle": "unarbos", "name": "Jacob" }),
                json!({ "member": "user:gh_9", "handle": "ken", "name": "Ken" }),
                json!({ "member": "agent:01HEZ", "handle": "hezbola" }),
            ],
            owner_user: "gh_268993031".into(),
            slug: "hezbola".into(),
            kind: "group".into(),
            owner_address: Some("Jacob".into()),
            ..Default::default()
        };
        let row = |id: &str, extra: Value, text: &str| -> MessageView {
            let mut v = json!({ "id": id, "role": "user", "status": "done", "parts": [{ "kind": "text", "text": text }] });
            for (k, val) in extra.as_object().unwrap() {
                v[k] = val.clone();
            }
            serde_json::from_value(v).unwrap()
        };
        let old = row(
            "m1",
            json!({ "sender": "user:gh_268993031", "senderName": "Arbos", "senderHandle": "unarbos" }),
            "keep up the roleplay",
        );
        let newer = row(
            "m2",
            json!({ "sender": "user:gh_268993031", "senderName": "Jacob", "senderHandle": "unarbos" }),
            "progress update?",
        );
        let ken = row(
            "m3",
            json!({ "sender": "user:gh_9", "senderName": "Kenji", "senderHandle": "ken" }),
            "hi all",
        );
        let guest_unknown = row(
            "m4",
            json!({ "senderName": "Priya N.", "senderHandle": "priya" }),
            "hello",
        );
        let cur = row(
            "m5",
            json!({ "sender": "user:gh_268993031", "senderName": "Arbos" }),
            "you have two owners?",
        );
        let lines = vec![old, newer, ken, guest_unknown, cur];
        let ctx = recent_people_context(&lines, "m5", 10, &info);
        assert_eq!(
            ctx[0]["from"], "Jacob",
            "an old row's label does not become a second owner"
        );
        assert_eq!(ctx[1]["from"], "Jacob");
        assert_eq!(
            ctx[2]["from"], "Ken",
            "a roster member is named from the roster, not the row"
        );
        assert_eq!(
            ctx[3]["from"], "Priya N.",
            "someone the roster does not know keeps the row's label"
        );
        assert_eq!(info.spoken_name_of("gh_268993031"), "Jacob");
        // The profile name wins even when the roster snapshot is older than the rename.
        let stale_roster = PeopleChatInfo {
            members: vec![
                json!({ "member": "user:gh_268993031", "handle": "unarbos", "name": "Arbos" }),
            ],
            owner_address: Some("Jacob".into()),
            ..info.clone()
        };
        assert_eq!(stale_roster.spoken_name_of("gh_268993031"), "Jacob");
        assert_eq!(
            author_spoken(&lines[0], &stale_roster, Some("gh_268993031")),
            "Jacob"
        );
    }

    #[test]
    fn recent_context_stops_at_current_and_caps() {
        let mut lines = Vec::new();
        for i in 0..15 {
            let mut m = msg(&format!("line {i}"), json!({}));
            m.id = format!("m{i}");
            m.rest["senderHandle"] = json!(if i % 2 == 0 { "sam" } else { "bob" });
            if i % 2 == 1 {
                m.role = "assistant".into();
            }
            lines.push(m);
        }
        let info = PeopleChatInfo::default();
        // Context for "m12": lines m0…m11, capped to the last 5.
        // The 7 older lines become one marker that says how to page them.
        let ctx = recent_people_context(&lines, "m12", 5, &info);
        assert_eq!(ctx.len(), 6);
        assert!(ctx[0]["note"]
            .as_str()
            .unwrap()
            .starts_with("[7 earlier messages not shown"));
        assert_eq!(ctx[1]["text"], "line 7");
        assert_eq!(ctx[5]["text"], "line 11");
        assert_eq!(ctx[5]["from"], "@bob (agent)");
        assert_eq!(ctx[4]["from"], "sam");
        // Unknown id → everything, still capped.
        let all = recent_people_context(&lines, "missing", 3, &info);
        assert_eq!(all.len(), 4);
        assert_eq!(all[3]["text"], "line 14");
    }
}

#[cfg(test)]
mod attribution_tests {
    //! #1580 end to end on the agent: the prelude, the asker and the reply fields for a guest's
    //! row that carries no author fields.

    use arbos_core::ids::ulid;

    use super::*;
    use crate::status::tests::test_agent;

    fn group_info(agent: &Agent, with_devices: bool) -> PeopleChatInfo {
        let devices = |ids: &[&str]| {
            if with_devices {
                json!(ids)
            } else {
                Value::Null
            }
        };
        PeopleChatInfo {
            members: vec![
                json!({ "member": "user:gh_owner", "handle": "jacob", "name": "Jacob", "devices": devices(&["DEVJACOB"]) }),
                json!({ "member": "user:gh_peer", "handle": "lt-peer", "name": "Priya", "devices": devices(&["01M5ND06ZW"]) }),
                json!({ "member": format!("agent:{}", agent.cfg.project_id), "handle": "lane-a" }),
            ],
            settings: json!({ "reply_policy": "mention" }),
            owner_user: "gh_owner".into(),
            slug: "lane-a".into(),
            kind: "group".into(),
            owner_address: None,
        }
    }

    #[tokio::test]
    async fn guest_row_without_author_fields_is_not_the_owner() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATATTRIBUTION00001";
        agent.people.test_insert(chat, group_info(&agent, false));
        let host = agent.sync.open_room(chat).await.unwrap();
        let note = ulid();
        let ask = ulid();
        {
            let h = host.lock().await;
            // The peer's note, as an older client / arbosctl wrote it: `sender: user`, nothing else.
            h.doc
                .append_user_message(
                    &note,
                    "Note from me: the launch date is 12 November",
                    "dev:anon",
                    &arbos_core::doc::UserMessageOpts::default(),
                )
                .unwrap();
            // The owner's question, fully labelled.
            let opts = arbos_core::doc::UserMessageOpts {
                sender: Some("user:gh_owner".into()),
                sender_handle: Some("jacob".into()),
                sender_name: Some("Jacob".into()),
                mentions: vec![format!("agent:{}", agent.cfg.project_id)],
                ..Default::default()
            };
            h.doc
                .append_user_message(
                    &ask,
                    "@lane-a who posted the launch date?",
                    "dev:mac",
                    &opts,
                )
                .unwrap();
        }

        // The unlabelled row itself: unknown author, guest powers, no owner anywhere.
        let prelude = agent.people_turn_context(chat, &host, &note).await.unwrap();
        assert!(
            prelude.contains("whose account the row does not name"),
            "{prelude}"
        );
        assert!(prelude.contains("treat them as a guest, not as Jacob"));
        assert!(!prelude.contains("from Jacob (owner)"), "{prelude}");
        let asker = agent.people.asker(chat).unwrap();
        assert_eq!(asker.user_id, UNKNOWN_ASKER);
        assert_eq!(asker.role, "guest");
        assert_eq!(asker.spoken(), "a member");
        let fields = agent.people_reply_fields(chat).unwrap().fields;
        assert!(
            !fields.iter().any(|(k, _)| *k == "askedBy"),
            "no account to point at: {fields:?}"
        );
        assert!(fields
            .iter()
            .any(|(k, v)| *k == "askedByHandle" && v == "a member"));

        // The owner's question: the group transcript labels the note as a member's, not Jacob's.
        let prelude = agent.people_turn_context(chat, &host, &ask).await.unwrap();
        assert!(
            prelude.contains("This message is from Jacob (owner)"),
            "{prelude}"
        );
        assert!(
            prelude.contains("- a member (message id ")
                && prelude.contains("): Note from me: the launch date is 12 November"),
            "{prelude}"
        );
        assert!(!prelude.contains("- Jacob: Note from me"), "{prelude}");

        // With the relay's device roster the same row resolves to the peer.
        agent.people.test_insert(chat, group_info(&agent, true));
        let peer_note = ulid();
        {
            let h = host.lock().await;
            h.doc
                .append_user_message(
                    &peer_note,
                    "Priya owns the checklist",
                    "dev:01M5ND06ZW",
                    &arbos_core::doc::UserMessageOpts::default(),
                )
                .unwrap();
        }
        let prelude = agent
            .people_turn_context(chat, &host, &peer_note)
            .await
            .unwrap();
        assert!(
            prelude.contains("This message is from Priya (guest)"),
            "{prelude}"
        );
        let asker = agent.people.asker(chat).unwrap();
        assert_eq!(asker.user_id, "gh_peer");
        assert_eq!(asker.handle, "lt-peer");
        let fields = agent.people_reply_fields(chat).unwrap().fields;
        assert!(fields
            .iter()
            .any(|(k, v)| *k == "askedBy" && v == "user:gh_peer"));
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod gate_record_tests {
    //! Every gate decision leaves a `gate` T3 row with the judge's reason, so
    //! `GET /projects/{id}/inspect` can answer "why did this agent stay quiet"; the judge is
    //! asked once per message; when it fails, unmentioned messages stay silent and mentioned
    //! ones follow the one stated default.

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc as StdArc;

    use arbos_core::ids::ulid;

    use super::tests::msg;
    use super::*;
    use crate::status::tests::{test_agent, test_agent_with};

    /// A Decisions API that reads the message and answers like the judge would: `@doug …` is
    /// for @doug, `@hjkb …` for @hjkb, "hey agents" for all, anything else for nobody.
    fn mock_decisions(calls: StdArc<AtomicUsize>) -> axum::Router {
        axum::Router::new().route(
            "/proxy/decisions",
            axum::routing::post(move |body: axum::body::Bytes| {
                let calls = calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    let v: Value = serde_json::from_slice(&body).unwrap();
                    let text = v["state"]["message"]["text"].as_str().unwrap_or("").to_lowercase();
                    let names: Vec<String> = v["state"]["agents"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|a| a["name"].as_str().unwrap().to_owned())
                        .collect();
                    let mut probs = serde_json::Map::new();
                    for n in &names {
                        probs.insert(n.clone(), json!(0.0));
                    }
                    for opt in ["any one agent", "several agents", "all agents", "nobody"] {
                        probs.insert(opt.into(), json!(0.0));
                    }
                    let pick = if text.starts_with("hey agents") {
                        "all agents".to_owned()
                    } else if let Some(n) = names.iter().find(|n| text.contains(n.as_str())) {
                        n.clone()
                    } else {
                        "nobody".to_owned()
                    };
                    probs.insert(pick.clone(), json!(0.93));
                    let mut answers = serde_json::Map::new();
                    answers.insert(
                        "addressee".into(),
                        json!({ "type": "choice", "choice": pick, "probabilities": probs, "confidence": 0.93 }),
                    );
                    for (i, n) in names.iter().enumerate() {
                        let p = if text.contains(n.as_str()) || text.starts_with("hey agents") { 0.9 } else { 0.1 };
                        answers.insert(format!("reply_{i}"), json!({ "type": "noul", "noul": p }));
                    }
                    axum::Json(json!({
                        "model": "typesafe/jev-1.14-test",
                        "answers": answers,
                        "usage": { "input_tokens": 600, "output_tokens": 80, "cost": 0.00003 }
                    }))
                }
            }),
        )
    }

    fn seed_group(agent: &Agent, chat: &str) {
        agent.people.chats.lock().expect("people").insert(
            chat.to_owned(),
            PeopleChatInfo {
                members: vec![
                    json!({ "member": "user:gh_2", "handle": "sam", "name": "Sam" }),
                    json!({ "member": format!("agent:{}", agent.cfg.project_id), "handle": "doug" }),
                    json!({ "member": "agent:OTHER", "handle": "hjkb" }),
                ],
                settings: json!({ "reply_policy": "mention" }),
                owner_user: "gh_1".into(),
                slug: "doug".into(),
                kind: "group".into(),
                owner_address: None,
            },
        );
    }

    async fn append(host: &SharedHost, text: &str, mentions: Vec<String>) -> String {
        let id = ulid();
        let h = host.lock().await;
        let opts = arbos_core::doc::UserMessageOpts {
            mentions,
            ..Default::default()
        };
        h.doc
            .append_user_message(&id, text, "dev-mac", &opts)
            .unwrap();
        id
    }

    #[tokio::test]
    async fn gate_decision_is_recorded_with_the_judges_reason_and_judged_once() {
        let calls = StdArc::new(AtomicUsize::new(0));
        let (agent, root) = test_agent_with(mock_decisions(calls.clone())).await;
        let chat = "01TESTCHATGATERECORD000001";
        seed_group(&agent, chat);
        let host = agent.sync.open_room(chat).await.unwrap();
        let me = format!("agent:{}", agent.cfg.project_id);
        let for_other = append(&host, "@hjkb you still borked?", vec!["agent:OTHER".into()]).await;
        let for_me = append(&host, "@doug status?", vec![me.clone()]).await;
        let about = append(
            &host,
            "The agents are getting better but still buggy",
            vec![],
        )
        .await;
        let for_all = append(&host, "hey agents, who is here?", vec![]).await;
        assert!(!agent.people_wants(chat, &host, &for_other).await);
        assert!(agent.people_wants(chat, &host, &for_me).await);
        assert!(!agent.people_wants(chat, &host, &about).await);
        assert!(agent.people_wants(chat, &host, &for_all).await);
        // Asked again (a steer, a retry, a catch-up): the cached verdict, no second call.
        assert!(agent.people_wants(chat, &host, &for_me).await);
        assert!(!agent.people_wants(chat, &host, &about).await);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            4,
            "one judge call per message"
        );

        let rows = agent.store.t3_after(0, Some(chat), 50).unwrap();
        let gates: Vec<&(crate::store::T3Row, Option<Value>)> =
            rows.iter().filter(|(r, _)| r.kind == "gate").collect();
        assert_eq!(gates.len(), 4, "{rows:?}");
        let by_msg = |id: &str| {
            gates
                .iter()
                .find(|(_, d)| d.as_ref().and_then(|d| d["message_id"].as_str()) == Some(id))
                .copied()
                .expect("gate row for the message")
        };
        let (skip, skip_data) = by_msg(&for_other);
        assert_eq!(
            skip.text,
            "gate skip: judge: for=@hjkb (0.93); not me → silent"
        );
        let skip_data = skip_data.as_ref().expect("gate row carries its facts");
        assert_eq!(skip_data["reply"], false);
        assert_eq!(skip_data["handle"], "doug");
        assert_eq!(skip_data["chosen"], json!(["OTHER"]));
        let (reply, _) = by_msg(&for_me);
        assert_eq!(
            reply.text,
            "gate reply: judge: for=@doug (0.93) ≥ 0.60 → reply"
        );
        let (quiet, _) = by_msg(&about);
        assert_eq!(
            quiet.text,
            "gate skip: judge: for=nobody (0.93); me 0.10 → silent"
        );
        let (all, all_data) = by_msg(&for_all);
        assert_eq!(
            all.text,
            "gate reply: judge: for=all agents (0.93) ≥ 0.60 → everyone replies"
        );
        assert_eq!(
            all_data.as_ref().unwrap()["chosen"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        // The shared send stays open for @hjkb until it replies.
        assert!(agent.people_shared_send_open(chat, &host, &for_all).await);
        assert!(!agent.people_shared_send_open(chat, &host, &for_me).await);
        // The window honours the chat filter and the time cursor.
        assert!(agent
            .store
            .t3_after(0, Some("other-chat"), 50)
            .unwrap()
            .is_empty());
        let after = agent.store.t3_after(reply.at, Some(chat), 50).unwrap();
        assert!(after.iter().all(|(r, _)| r.at > reply.at));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// No judge (the mock relay has no `/proxy/decisions`): an unmentioned message is silence;
    /// every @mention by a person wakes the agent, vocative or not ("thanks @doug").
    #[tokio::test]
    async fn judge_failure_fails_closed_unless_mentioned() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATGATEFAILS0000001";
        seed_group(&agent, chat);
        let host = agent.sync.open_room(chat).await.unwrap();
        let me = format!("agent:{}", agent.cfg.project_id);
        let plain = append(&host, "the agents are getting better", vec![]).await;
        let vocative = append(&host, "@doug status", vec![me.clone()]).await;
        let thanks = append(&host, "thanks @doug", vec![me.clone()]).await;
        let asked = append(&host, "did you see this @doug?", vec![me.clone()]).await;
        assert!(!agent.people_wants(chat, &host, &plain).await);
        assert!(agent.people_wants(chat, &host, &vocative).await);
        assert!(agent.people_wants(chat, &host, &thanks).await);
        assert!(agent.people_wants(chat, &host, &asked).await);
        let reasons: HashMap<String, String> = agent
            .store
            .t3_after(0, Some(chat), 50)
            .unwrap()
            .into_iter()
            .filter(|(r, _)| r.kind == "gate")
            .filter_map(|(r, d)| Some((d?["message_id"].as_str()?.to_owned(), r.text)))
            .collect();
        assert!(
            reasons[&plain].contains("unmentioned → silent (fail closed)"),
            "{reasons:?}"
        );
        assert!(
            reasons[&vocative].contains("@mention addressed by a person → reply"),
            "{reasons:?}"
        );
        assert!(
            reasons[&thanks].contains("@mention object by a person → reply"),
            "{reasons:?}"
        );
        assert!(
            reasons[&asked].contains("by a person → reply"),
            "{reasons:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// After a `reply` verdict the turn prelude tells the model the gate already decided and
    /// to answer; without one it keeps the old "say nothing if not for you" sentence.
    #[tokio::test]
    async fn prelude_follows_the_judges_verdict() {
        let calls = StdArc::new(AtomicUsize::new(0));
        let (agent, root) = test_agent_with(mock_decisions(calls)).await;
        let chat = "01TESTCHATGATEPRELUDE00001";
        seed_group(&agent, chat);
        let host = agent.sync.open_room(chat).await.unwrap();
        let me = format!("agent:{}", agent.cfg.project_id);
        let asked = append(&host, "@doug what is the capital of Portugal?", vec![me]).await;
        assert!(agent.people_wants(chat, &host, &asked).await);
        let prelude = agent
            .people_turn_context(chat, &host, &asked)
            .await
            .unwrap();
        assert!(
            prelude.contains("The reply gate has already judged that this message is for you (judge: for=@doug (0.93) ≥ 0.60 → reply); answer it plainly."),
            "{prelude}"
        );
        assert!(prelude.contains("never post that you are staying silent"));
        // 0.1.173 proof: "Hezbola-rg? 😂" passed the gate (0.81) and the talking model still routed
        // an empty reply; a name ping is an ask for presence.
        assert!(
            prelude.contains("A bare name ping (\"doug?\", \"hey doug\") wants a one-line presence answer, not silence."),
            "{prelude}"
        );
        let unjudged = append(&host, "morning", vec![]).await;
        let prelude = agent
            .people_turn_context(chat, &host, &unjudged)
            .await
            .unwrap();
        assert!(
            prelude.contains("If this message is for someone else"),
            "{prelude}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The owner's `every_message` setting is not a heuristic: no judge call, every human
    /// line is answered.
    #[tokio::test]
    async fn every_message_policy_skips_the_judge() {
        let calls = StdArc::new(AtomicUsize::new(0));
        let (agent, root) = test_agent_with(mock_decisions(calls.clone())).await;
        let chat = "01TESTCHATGATEEVERY0000001";
        seed_group(&agent, chat);
        agent
            .people
            .chats
            .lock()
            .expect("people")
            .get_mut(chat)
            .unwrap()
            .settings = json!({ "reply_policy": "every_message" });
        let host = agent.sync.open_room(chat).await.unwrap();
        let plain = append(&host, "morning all", vec![]).await;
        assert!(agent.people_wants(chat, &host, &plain).await);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// #2207 (long-task h4, image 0.1.177): the judge chose lane-b alone for "@lane-b can you
    /// communicate directly with @lane-a?", then lane-b's reply said "I can message @lane-a
    /// directly …" and that bubble was dispatched as a `send` for lane-a, who answered. An
    /// agent's row never wakes another agent — not by @handle, not by chip, not by reply —
    /// whoever it names; agents reach each other only through `send_message`.
    #[tokio::test]
    async fn an_agents_bubble_never_wakes_another_agent() {
        let calls = StdArc::new(AtomicUsize::new(0));
        let (agent, root) = test_agent_with(mock_decisions(calls.clone())).await;
        let chat = "01TESTCHATAGENTROW00000001";
        seed_group(&agent, chat);
        let host = agent.sync.open_room(chat).await.unwrap();
        let me = format!("agent:{}", agent.cfg.project_id);
        let (vocative, chip, reply_to_mine) = {
            let h = host.lock().await;
            let mut ids = Vec::new();
            for (text, mentions) in [
                (
                    "@doug, Arbos asked me to pass this on: please confirm.",
                    vec![me.clone()],
                ),
                (
                    "I can message doug directly and relay the answer here.",
                    vec![me.clone()],
                ),
                ("Agreed with what @doug said above.", vec![]),
            ] {
                let id = ulid();
                h.doc.start_assistant_message(&id, "q", "box:other");
                h.doc.set_message_fields(
                    &id,
                    &[
                        ("sender", json!("agent:OTHER")),
                        ("senderHandle", json!("hjkb")),
                        ("mentions", json!(mentions)),
                    ],
                );
                let part = h.doc.add_text_part(&id, "text").unwrap();
                h.doc.append_text(&id, &part, text);
                h.doc
                    .finish_message(&id, arbos_core::doc::MessageStatus::Done);
                ids.push(id);
            }
            (ids[0].clone(), ids[1].clone(), ids[2].clone())
        };
        for id in [&vocative, &chip, &reply_to_mine] {
            assert!(!agent.people_wants(chat, &host, id).await, "{id}");
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "no judge call for an agent's row"
        );
        let reasons: Vec<String> = agent
            .store
            .t3_after(0, Some(chat), 50)
            .unwrap()
            .into_iter()
            .filter(|(r, _)| r.kind == "gate")
            .map(|(r, _)| r.text)
            .collect();
        assert_eq!(reasons.len(), 3);
        assert!(reasons.iter().all(|r| {
            r == "gate skip: another agent's row: agents reach each other only through send_message"
        }));
        // And nothing was pushed as a command for the named agent.
        assert!(host
            .lock()
            .await
            .doc
            .commands()
            .iter()
            .all(|c| c.payload.get("agentMention").is_none()));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A judge that is never sure: the first @mentioned agent at 0.48 (under every threshold),
    /// like the live call on "@bob talk to @bubble" (Arbonites 2026-10-28 21:48:14 UTC).
    fn unsure_decisions(calls: StdArc<AtomicUsize>) -> axum::Router {
        axum::Router::new().route(
            "/proxy/decisions",
            axum::routing::post(move |body: axum::body::Bytes| {
                let calls = calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    let v: Value = serde_json::from_slice(&body).unwrap();
                    let text = v["state"]["message"]["text"].as_str().unwrap_or("").to_lowercase();
                    let names: Vec<String> = v["state"]["agents"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|a| a["name"].as_str().unwrap().to_owned())
                        .collect();
                    let first = names
                        .iter()
                        .filter_map(|n| text.find(n.as_str()).map(|i| (i, n.clone())))
                        .min()
                        .map(|(_, n)| n)
                        .unwrap_or_else(|| "nobody".to_owned());
                    let mut probs = serde_json::Map::new();
                    for n in &names {
                        probs.insert(n.clone(), json!(if *n == first { 0.48 } else { 0.15 }));
                    }
                    for opt in ["any one agent", "several agents", "all agents"] {
                        probs.insert(opt.into(), json!(0.02));
                    }
                    probs.insert("nobody".into(), json!(if first == "nobody" { 0.48 } else { 0.2 }));
                    let mut answers = serde_json::Map::new();
                    answers.insert(
                        "addressee".into(),
                        json!({ "type": "choice", "choice": first, "probabilities": probs, "confidence": 0.48 }),
                    );
                    for i in 0..names.len() {
                        answers.insert(format!("reply_{i}"), json!({ "type": "noul", "noul": 0.3 }));
                    }
                    axum::Json(json!({
                        "model": "typesafe/jev-1.14-test",
                        "answers": answers,
                        "usage": { "input_tokens": 600, "output_tokens": 80, "cost": 0.00003 }
                    }))
                }
            }),
        )
    }

    /// Jacob, Arbonites 2026-10-28: "@bob talk to @bubble" woke nobody — the judge said
    /// `for=@bob (0.48) < 0.50` and both boxes stayed silent. A person's explicit @mentions
    /// wake every agent they name (addressed or object, typed or chip, judge unsure or down);
    /// the same words in an agent's bubble still wake no one (#2207).
    #[tokio::test]
    async fn a_persons_mentions_wake_every_mentioned_agent() {
        let calls = StdArc::new(AtomicUsize::new(0));
        let (agent, root) = test_agent_with(unsure_decisions(calls.clone())).await;
        let chat = "01TESTCHATMULTIMENTION0001";
        let me_id = agent.cfg.project_id.clone();
        let me = format!("agent:{me_id}");
        agent.people.chats.lock().expect("people").insert(
            chat.to_owned(),
            PeopleChatInfo {
                members: vec![
                    json!({ "member": "user:gh_2", "handle": "sam", "name": "Sam" }),
                    json!({ "member": me.clone(), "handle": "doug" }),
                    json!({ "member": "agent:OTHER", "handle": "hjkb" }),
                    json!({ "member": "agent:THIRD", "handle": "lane" }),
                    json!({ "member": "agent:QUIET", "handle": "quiet" }),
                ],
                settings: json!({ "reply_policy": "mention" }),
                owner_user: "gh_1".into(),
                slug: "doug".into(),
                kind: "group".into(),
                owner_address: None,
            },
        );
        let host = agent.sync.open_room(chat).await.unwrap();
        let chosen_of = |id: &str| {
            let mut c = agent.people.gate_verdict(id).expect("judged").chosen;
            c.sort();
            c
        };

        // Addressed first, another agent as the object: both wake.
        let me_first = append(
            &host,
            "@doug talk to @hjkb",
            vec![me.clone(), "agent:OTHER".into()],
        )
        .await;
        assert!(agent.people_wants(chat, &host, &me_first).await);
        assert_eq!(
            chosen_of(&me_first),
            vec![me_id.clone(), "OTHER".to_owned()]
        );

        // This agent is the object of a request to someone else: still woken.
        let me_object = append(
            &host,
            "@hjkb talk to @doug",
            vec!["agent:OTHER".into(), me.clone()],
        )
        .await;
        assert!(agent.people_wants(chat, &host, &me_object).await);
        assert_eq!(
            chosen_of(&me_object),
            vec![me_id.clone(), "OTHER".to_owned()]
        );

        // Three agents in one line, one by chip only; the unmentioned fourth is not added.
        let three = append(
            &host,
            "@hjkb and @lane, loop in doug please",
            vec!["agent:OTHER".into(), "agent:THIRD".into(), me.clone()],
        )
        .await;
        assert!(agent.people_wants(chat, &host, &three).await);
        assert_eq!(
            chosen_of(&three),
            vec![me_id.clone(), "OTHER".to_owned(), "THIRD".to_owned()]
        );

        // Not mentioned among several mentions: the unsure judge's silence stands.
        let others_only = append(
            &host,
            "@hjkb talk to @lane",
            vec!["agent:OTHER".into(), "agent:THIRD".into()],
        )
        .await;
        assert!(!agent.people_wants(chat, &host, &others_only).await);
        assert_eq!(
            chosen_of(&others_only),
            vec!["OTHER".to_owned(), "THIRD".to_owned()]
        );

        let reasons: HashMap<String, String> = agent
            .store
            .t3_after(0, Some(chat), 50)
            .unwrap()
            .into_iter()
            .filter(|(r, _)| r.kind == "gate")
            .filter_map(|(r, d)| Some((d?["message_id"].as_str()?.to_owned(), r.text)))
            .collect();
        assert!(
            reasons[&me_first].contains("< 0.60")
                && reasons[&me_first].ends_with("@mention addressed by a person → reply (a person's @mention always wakes the agent)"),
            "{reasons:?}"
        );
        assert!(
            reasons[&me_object].contains("not me")
                && reasons[&me_object].contains("@mention object by a person → reply"),
            "{reasons:?}"
        );

        // The prelude says why the agent woke, how to reach the other agent, and that a line
        // that only names it still gets no post.
        for id in [&me_first, &me_object] {
            let prelude = agent.people_turn_context(chat, &host, id).await.unwrap();
            assert!(
                prelude.contains("Sam @mentioned you; a person's @mention always reaches you"),
                "{prelude}"
            );
            assert!(
                prelude.contains("means reach that agent with send_message"),
                "{prelude}"
            );
            assert!(
                prelude.contains("never post that you are staying silent"),
                "{prelude}"
            );
            assert!(!prelude.contains("has already judged"), "{prelude}");
        }

        // #2207 stays: the same words in another agent's bubble wake nobody, no judge call.
        let before = calls.load(Ordering::SeqCst);
        let agent_row = {
            let h = host.lock().await;
            let id = ulid();
            h.doc.start_assistant_message(&id, "q", "box:other");
            h.doc.set_message_fields(
                &id,
                &[
                    ("sender", json!("agent:OTHER")),
                    ("senderHandle", json!("hjkb")),
                    ("mentions", json!([me.clone(), "agent:THIRD"])),
                ],
            );
            let part = h.doc.add_text_part(&id, "text").unwrap();
            h.doc.append_text(&id, &part, "@doug talk to @lane");
            h.doc
                .finish_message(&id, arbos_core::doc::MessageStatus::Done);
            id
        };
        assert!(!agent.people_wants(chat, &host, &agent_row).await);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            before,
            "no judge call for an agent's row"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Jacob, Arbonites 2026-10-26: "@hezbola can you communicate directly with @doug-agent?"
    /// woke Doug agent, who posted "Addressed to Hezbola; staying silent." An @mention is a
    /// consideration signal: Hezbola is addressed, Doug is the object of the request.
    #[test]
    fn mention_role_tells_addressed_from_object() {
        use MentionRole::*;
        let t = "@hezbola can you communicate directly with @doug-agent?";
        assert_eq!(mention_role(t, "hezbola"), Addressed);
        assert_eq!(mention_role(t, "doug-agent"), Object);
        // Jacob's first message of the pair: nobody else named in the prose, Doug still the object.
        assert_eq!(
            mention_role(
                "Can you communicate directly with @doug-agent?",
                "doug-agent"
            ),
            Object
        );
        assert_eq!(
            mention_role(
                "@doug-agent do you and @hezbola have access to the same box?",
                "doug-agent"
            ),
            Addressed
        );
        assert_eq!(
            mention_role(
                "@doug-agent do you and @hezbola have access to the same box?",
                "hezbola"
            ),
            Referenced
        );
        assert_eq!(
            mention_role("hey @doug-agent, status?", "doug-agent"),
            Addressed
        );
        assert_eq!(
            mention_role("ok @doug-agent go ahead", "doug-agent"),
            Addressed
        );
        assert_eq!(
            mention_role("what do you think, @doug-agent?", "doug-agent"),
            Addressed
        );
        assert_eq!(
            mention_role("Jacob, @doug-agent can you check?", "doug-agent"),
            Addressed
        );
        assert_eq!(
            mention_role("ask @doug-agent about the render", "doug-agent"),
            Object
        );
        assert_eq!(
            mention_role("send it to @doug-agent please", "doug-agent"),
            Object
        );
        assert_eq!(
            mention_role("@doug-agent's renders are on his box", "doug-agent"),
            Object
        );
        assert_eq!(mention_role("thanks @doug-agent", "doug-agent"), Object);
        assert_eq!(
            mention_role("I think @doug-agent and you should sync", "doug-agent"),
            Referenced
        );
        // Another handle that merely starts with the same letters is not this agent.
        assert_eq!(
            mention_role("ping @doug-agent2 instead", "doug-agent"),
            None
        );
        assert_eq!(mention_role("no mention here", "doug-agent"), None);
        // One vocative use wins over a later reference.
        assert_eq!(
            mention_role(
                "@doug-agent, Hezbola asked about @doug-agent's box",
                "doug-agent"
            ),
            Addressed
        );
    }

    #[test]
    fn object_mention_in_a_request_to_another_agent_is_not_addressed() {
        let group = PeopleChatInfo {
            members: vec![
                json!({ "member": "agent:HEZ", "handle": "hezbola", "project": { "name": "Hezbola" } }),
                json!({ "member": "agent:DOUG", "handle": "doug-agent", "project": { "name": "Doug agent" } }),
                json!({ "member": "user:gh_1", "handle": "unconst", "name": "Jacob" }),
            ],
            slug: "doug-agent".into(),
            kind: "group".into(),
            ..Default::default()
        };
        let m = msg(
            "@hezbola can you communicate directly with @doug-agent?",
            json!({ "mentions": ["agent:HEZ", "agent:DOUG"], "sender": "user:gh_1" }),
        );
        assert_eq!(
            my_mention_role(&m, &group, "DOUG", "doug-agent"),
            MentionRole::Object
        );
        assert_eq!(
            my_mention_role(&m, &group, "HEZ", "hezbola"),
            MentionRole::Addressed
        );
        // A chip with no @handle in the prose: referenced, the decision model judges.
        let chip_only = msg(
            "can you check with him?",
            json!({ "mentions": ["agent:DOUG"], "sender": "user:gh_1" }),
        );
        assert_eq!(
            my_mention_role(&chip_only, &group, "DOUG", "doug-agent"),
            MentionRole::Referenced
        );
        let bare = msg(
            "Hezbola, can you check with @doug-agent?",
            json!({ "sender": "user:gh_1" }),
        );
        assert_eq!(
            my_mention_role(&bare, &group, "DOUG", "doug-agent"),
            MentionRole::Object
        );
    }

    /// Jacob, 2026-10-26: Hezbola got a code post cut at `def __i` and did not know. An earlier
    /// message that @mentions the agent, and the one being replied to, come whole; other long
    /// ones carry the marker and the fetch call.
    #[test]
    fn recent_context_marks_cuts_and_passes_mentions_and_reply_targets_whole() {
        let info = PeopleChatInfo {
            members: vec![
                json!({ "member": "agent:HEZ", "handle": "hezbola" }),
                json!({ "member": "user:gh_1", "handle": "doug", "name": "Doug" }),
            ],
            slug: "hezbola".into(),
            kind: "group".into(),
            ..Default::default()
        };
        let code = format!(
            "def __init__(self):\n{}    return TAIL_OK\n",
            "    x = 1\n".repeat(350)
        );
        let mut m1 = msg(&code, json!({ "sender": "user:gh_1" }));
        m1.id = "m1".into();
        let mut m2 = msg(
            &format!("@hezbola here is the class:\n{code}"),
            json!({ "sender": "user:gh_1" }),
        );
        m2.id = "m2".into();
        let mut m3 = msg(&code, json!({ "sender": "user:gh_1" }));
        m3.id = "m3".into();
        let mut cur = msg(
            "does the tail look right?",
            json!({ "sender": "user:gh_1", "replyTo": "m3" }),
        );
        cur.id = "cur".into();
        let ctx = recent_people_context_for(
            &[m1, m2, m3, cur],
            "cur",
            12,
            &info,
            "01CHAT",
            &["hezbola".to_owned()],
        );
        assert_eq!(ctx.len(), 3);
        let t1 = ctx[0]["text"].as_str().unwrap();
        assert!(t1.contains("[truncated: showing 3000 of") && t1.contains("message id m1; full text: chat_messages {\"chat_id\": \"01CHAT\", \"message_id\": \"m1\"}"));
        assert!(!t1.contains("TAIL_OK"));
        assert!(
            ctx[1]["text"].as_str().unwrap().contains("TAIL_OK"),
            "the message that @mentions me is whole"
        );
        assert!(
            ctx[2]["text"].as_str().unwrap().contains("TAIL_OK"),
            "the message being replied to is whole"
        );
    }
}
