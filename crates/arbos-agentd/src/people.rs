//! People chats (D72) on the box side. This Project's agent can be a member of chats between
//! people; it joins each one as a peer in the room `people/<chat_id>` (the relay admits the
//! owner's box for it) and answers the humans there.
//!
//! Mention to reply (default): a human message is for this agent when its `mentions` carry
//! `agent:<project>`, or the text says `@<slug>` (the Project's name as a handle), or the text
//! opens with this agent's bare name/handle ("Bob?", "hey Bob …"), or it addresses agents
//! collectively ("can the agents…", "hey agents"), or it replies to one of this agent's messages,
//! or it says `@arbos` and this is the only agent in the chat; the owner can switch the agent to
//! "every message". In a people **group** (`kind: group`, or more than one agent), when nothing
//! addresses this agent, the decision model reads recent chat context and chooses whether this
//! agent should reply (or stay quiet) so the right agent continues the thread — or none do. Each
//! human message is labelled with the author's display name (falling back to bare handle) and role
//! (`owner` | `guest`) in the prompt. Guests may direct ordinary work; they may not use the
//! owner's secrets unless the owner put them on that secret's People list. Agents
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

/// How many prior messages the reply gate sees (oldest → newest).
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

    /// How to speak to / about this person: display name, else bare handle (no `@`).
    fn spoken_name_of(&self, user_id: &str) -> String {
        self.members
            .iter()
            .find(|m| m["member"].as_str() == Some(&format!("user:{user_id}")))
            .map(person_spoken_name)
            .unwrap_or_else(|| self.handle_of(user_id))
    }

    /// Human members by spoken name, for the responder decision's people roster.
    fn people_labels(&self) -> Vec<String> {
        self.members
            .iter()
            .filter(|m| m["member"].as_str().is_some_and(|s| s.starts_with("user:")))
            .map(person_spoken_name)
            .collect()
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

/// True when the message is aimed at people and names no agent (chip or `@handle`).
///
/// Person chips ("Call?" with a `user:` mention and no `@` in the prose) and a leading
/// `@handle` ("@ken …", "Yo @ken") are for humans — the reply gate must not wake an agent.
/// A mid-line `@person` alone ("Would you rent if @ken asked?") is a reference, not an
/// address: leave the hard gate open so the group decision model can still pick an agent.
fn addressed_only_to_people(msg: &MessageView) -> bool {
    let Some(mentions) = msg.rest.get("mentions").and_then(Value::as_array) else {
        return false;
    };
    let mut saw_person = false;
    for m in mentions {
        let Some(s) = m.as_str() else {
            continue;
        };
        if s.starts_with("agent:") {
            return false;
        }
        if s.starts_with("user:") {
            saw_person = true;
        }
    }
    if !saw_person {
        return false;
    }
    let text = msg.text();
    // Chip / bare-name address: person mention with no `@` token in the prose.
    if !text.contains('@') {
        return true;
    }
    // Leading `@handle` (optional yo/hey/hi…) is still a people ping.
    leading_address_token(&text).is_some_and(|t| t.starts_with('@'))
}

/// Leading token of the message (optional yo/hey/hi… greeting), lower-cased, punctuation stripped.
fn leading_address_token(text: &str) -> Option<String> {
    let mut lower = text.trim().to_lowercase();
    if lower.is_empty() {
        return None;
    }
    for g in ["yo ", "hey ", "hi ", "hello ", "ok ", "okay "] {
        if let Some(rest) = lower.strip_prefix(g) {
            lower = rest.to_owned();
            break;
        }
    }
    let first = lower
        .split_whitespace()
        .next()?
        .trim_matches(|c| matches!(c, '.' | ',' | '!' | '?' | ':' | ';'));
    if first.chars().count() < 2 {
        return None;
    }
    Some(first.to_owned())
}

/// True when the prose `@`-mentions an agent in this chat (not a sticky chip mention alone).
fn text_mentions_agent(msg: &MessageView, info: &PeopleChatInfo) -> bool {
    let text = msg.text().to_lowercase();
    let has = |token: &str| {
        let t = format!("@{token}");
        text.match_indices(&t).any(|(i, _)| {
            let after = text[i + t.len()..].chars().next();
            after.is_none_or(|c| !(c.is_alphanumeric() || c == '-' || c == '_'))
        })
    };
    if !info.slug.is_empty() && has(&info.slug) {
        return true;
    }
    for m in &info.members {
        let Some(member) = m["member"].as_str() else {
            continue;
        };
        if !member.starts_with("agent:") {
            continue;
        }
        let handle = m["handle"].as_str().unwrap_or("").to_lowercase();
        if !handle.is_empty() && has(&handle) {
            return true;
        }
        // Same slug rule as iOS `mentionSlug`: lower-case, spaces → "-".
        if let Some(name) = m["project"]["name"].as_str() {
            let slug = name.to_lowercase().replace(' ', "-");
            if !slug.is_empty() && has(&slug) {
                return true;
            }
        }
    }
    info.agents() == 1 && has("arbos")
}

/// True when the prose addresses a human at the start by handle or display/first name
/// ("Allan", "Allan?", "Allan. we are…", "Yo Allan") without needing `@`.
fn text_names_people(msg: &MessageView, info: &PeopleChatInfo) -> bool {
    let Some(token) = leading_address_token(&msg.text()) else {
        return false;
    };
    text_token_names_people(&token, info, true)
}

/// True when the leading token is a human's `@handle` (not merely their display name).
fn text_names_people_by_handle(msg: &MessageView, info: &PeopleChatInfo) -> bool {
    let Some(token) = leading_address_token(&msg.text()) else {
        return false;
    };
    text_token_names_people(&token, info, false)
}

fn text_token_names_people(token: &str, info: &PeopleChatInfo, allow_display_name: bool) -> bool {
    for m in &info.members {
        let Some(member) = m["member"].as_str() else {
            continue;
        };
        if !member.starts_with("user:") {
            continue;
        }
        let handle = m["handle"].as_str().unwrap_or("").to_lowercase();
        if !handle.is_empty() && handle == token {
            return true;
        }
        if !allow_display_name {
            continue;
        }
        if let Some(name) = m["name"].as_str() {
            let name = name.trim().to_lowercase();
            if name == token {
                return true;
            }
            if name
                .split_whitespace()
                .next()
                .is_some_and(|first| first == token)
            {
                return true;
            }
        }
    }
    false
}

/// True when the leading token matches any agent in the chat by handle or mention slug
/// (project name lower-cased, spaces → "-"). Used so a human's display name that collides
/// with an agent's `@slug` ("Arbos" when @unarbos is named Arbos and the agent is @arbos)
/// does not silence the agent.
fn text_names_any_agent_handle(msg: &MessageView, info: &PeopleChatInfo) -> bool {
    let Some(token) = leading_address_token(&msg.text()) else {
        return false;
    };
    for m in &info.members {
        let Some(member) = m["member"].as_str() else {
            continue;
        };
        if !member.starts_with("agent:") {
            continue;
        }
        let handle = m["handle"].as_str().unwrap_or("").to_lowercase();
        if !handle.is_empty() && handle == token {
            return true;
        }
        let name = m["project"]["name"]
            .as_str()
            .or_else(|| m["name"].as_str())
            .unwrap_or("")
            .trim()
            .to_lowercase();
        if name.is_empty() {
            continue;
        }
        if name == token || name.replace(' ', "-") == token {
            return true;
        }
        if name
            .split_whitespace()
            .next()
            .is_some_and(|first| first == token)
        {
            return true;
        }
    }
    // Sole-agent `@arbos` alias also applies to bare "Arbos" / "arbos".
    info.agents() == 1 && token == "arbos"
}

/// True when the prose addresses this agent at the start by handle, slug, or display/first name
/// ("Bob", "Bob?", "hey Bob what do you think?") without needing `@`. A leading person
/// `@handle` wins when the same token could match a human; a person's display name alone does
/// not beat this agent's handle/slug (so "Arbos what is your name" wakes @arbos even when a
/// human in the chat is also named Arbos).
fn text_names_this_agent(msg: &MessageView, info: &PeopleChatInfo, project_id: &str) -> bool {
    // Person @handle still wins ("catoneone …" when that is their handle).
    if text_names_people_by_handle(msg, info) {
        return false;
    }
    let Some(token) = leading_address_token(&msg.text()) else {
        return false;
    };
    let me = format!("agent:{project_id}");
    for m in &info.members {
        let Some(member) = m["member"].as_str() else {
            continue;
        };
        if member != me {
            continue;
        }
        let handle = m["handle"].as_str().unwrap_or("").to_lowercase();
        if !handle.is_empty() && handle == token {
            return true;
        }
        if !info.slug.is_empty() && info.slug.eq_ignore_ascii_case(&token) {
            return true;
        }
        let name = m["project"]["name"]
            .as_str()
            .or_else(|| m["name"].as_str())
            .unwrap_or("")
            .trim()
            .to_lowercase();
        if name.is_empty() {
            continue;
        }
        if name == token {
            return true;
        }
        // Same slug rule as iOS `mentionSlug`: lower-case, spaces → "-".
        if name.replace(' ', "-") == token {
            return true;
        }
        if name
            .split_whitespace()
            .next()
            .is_some_and(|first| first == token)
        {
            return true;
        }
    }
    // Sole-agent bare "arbos" — same alias as `@arbos` in `addressed`.
    info.agents() == 1 && token == "arbos"
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

/// Every agent project id in this people chat.
fn chat_agent_ids(info: &PeopleChatInfo) -> Vec<String> {
    let mut out = Vec::new();
    for m in &info.members {
        let Some(id) = m["member"].as_str().and_then(|s| s.strip_prefix("agent:")) else {
            continue;
        };
        if !out.iter().any(|x| x == id) {
            out.push(id.to_owned());
        }
    }
    out
}

/// Agents this message is shared with (more than one must reply): collective phrases
/// ("hey agents") or two-plus `agent:` mentions. Empty when only one agent is addressed
/// (or none — the decision model path stays single-claim).
pub fn people_shared_agent_ids(msg: &MessageView, info: &PeopleChatInfo) -> Vec<String> {
    if text_addresses_agents_collectively(msg) {
        return chat_agent_ids(info);
    }
    let mentioned = mentioned_agent_ids(msg);
    if mentioned.len() > 1 {
        return mentioned;
    }
    Vec::new()
}

/// "hey agents" / "can the agents…" / "all agents" — address every agent, no `@` required.
fn text_addresses_agents_collectively(msg: &MessageView) -> bool {
    let trimmed = msg.text().trim().to_lowercase();
    if trimmed.is_empty() {
        return false;
    }
    for g in [
        "hey agents",
        "hi agents",
        "hello agents",
        "yo agents",
        "ok agents",
        "okay agents",
        "all agents",
        "the agents",
        "can the agents",
        "can agents",
    ] {
        if trimmed == g {
            return true;
        }
        for sep in [" ", ",", "!", "?", ":", "—", "-"] {
            if trimmed.starts_with(&(g.to_owned() + sep)) {
                return true;
            }
        }
    }
    false
}

/// True when the message explicitly addresses one or more agents in this chat — by `mentions`
/// (`agent:<id>`), by `@slug` in the prose, or by an agent's bare name at the start — and this
/// agent is not among them. Such a message belongs to the named agent(s): this agent stays
/// silent, and the group decision model is not asked (it must never hand "@gigchadagent you
/// still borked?" to hjkb).
fn addressed_to_other_agents_only(
    msg: &MessageView,
    info: &PeopleChatInfo,
    project_id: &str,
) -> bool {
    let me = format!("agent:{project_id}");
    let mentioned = mentioned_agent_ids(msg);
    if !mentioned.is_empty() {
        return !mentioned.iter().any(|id| *id == project_id);
    }
    let text = msg.text().to_lowercase();
    let has = |token: &str| {
        let t = format!("@{token}");
        text.match_indices(&t).any(|(i, _)| {
            let after = text[i + t.len()..].chars().next();
            after.is_none_or(|c| !(c.is_alphanumeric() || c == '-' || c == '_'))
        })
    };
    let leading = leading_address_token(&msg.text());
    let mut names_other = false;
    for m in &info.members {
        let Some(member) = m["member"].as_str() else {
            continue;
        };
        if !member.starts_with("agent:") || member == me {
            continue;
        }
        let handle = m["handle"].as_str().unwrap_or("").to_lowercase();
        let name = m["project"]["name"]
            .as_str()
            .or_else(|| m["name"].as_str())
            .unwrap_or("")
            .trim()
            .to_lowercase();
        let slug = name.replace(' ', "-");
        if (!handle.is_empty() && has(&handle)) || (!slug.is_empty() && has(&slug)) {
            names_other = true;
        }
        if let Some(t) = &leading {
            if (!handle.is_empty() && handle == *t)
                || (!name.is_empty() && (name == *t || slug == *t))
            {
                names_other = true;
            }
        }
    }
    if !names_other {
        return false;
    }
    // Named another agent and not this one (by any of the same rules).
    !(text_mentions_this_agent(msg, info, project_id)
        || text_names_this_agent(msg, info, project_id))
}

/// True when the prose `@`-mentions this agent (its slug or handle).
fn text_mentions_this_agent(msg: &MessageView, info: &PeopleChatInfo, project_id: &str) -> bool {
    let text = msg.text().to_lowercase();
    let has = |token: &str| {
        let t = format!("@{token}");
        text.match_indices(&t).any(|(i, _)| {
            let after = text[i + t.len()..].chars().next();
            after.is_none_or(|c| !(c.is_alphanumeric() || c == '-' || c == '_'))
        })
    };
    if !info.slug.is_empty() && has(&info.slug) {
        return true;
    }
    let me = format!("agent:{project_id}");
    info.members.iter().any(|m| {
        if m["member"].as_str() != Some(me.as_str()) {
            return false;
        }
        let handle = m["handle"].as_str().unwrap_or("").to_lowercase();
        let name = m["project"]["name"]
            .as_str()
            .or_else(|| m["name"].as_str())
            .unwrap_or("")
            .trim()
            .to_lowercase();
        (!handle.is_empty() && has(&handle)) || (!name.is_empty() && has(&name.replace(' ', "-")))
    })
}

/// Leading person ping in the prose (or a people chip / leading `@handle`) and no agent `@`
/// in the text. Sticky chip / reply-to agent mentions must not wake an agent for "Allan?".
/// Mid-line `@person` alone is not a people ping — see `addressed_only_to_people`.
/// A leading token that is an agent's handle/slug is for the agent — even when a human's
/// display name collides ("Arbos" with @unarbos named Arbos and agent @arbos).
fn people_ping_not_for_agents(msg: &MessageView, info: &PeopleChatInfo) -> bool {
    if text_mentions_agent(msg, info) {
        return false;
    }
    if text_names_any_agent_handle(msg, info) {
        return false;
    }
    addressed_only_to_people(msg) || text_names_people(msg, info)
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
}

impl PeopleState {
    pub fn set_project_owner(&self, user_id: &str) {
        *self.project_owner.lock().expect("people") = user_id.to_owned();
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

    /// Test-only: seed a people-chat snapshot (e.g. rename + @handle refresh).
    #[cfg(test)]
    pub(crate) fn test_insert(&self, chat_id: &str, info: PeopleChatInfo) {
        self.chats
            .lock()
            .expect("people")
            .insert(chat_id.to_owned(), info);
    }
}

/// Is this human message for the agent `project_id` with the given slug and policy?
pub fn addressed(
    msg: &MessageView,
    project_id: &str,
    slug: &str,
    only_agent: bool,
    every_message: bool,
    replied_to_mine: bool,
) -> bool {
    if msg.role != "user" {
        return false;
    }
    let sender_is_human = msg
        .rest
        .get("sender")
        .and_then(Value::as_str)
        .is_none_or(|s| s.starts_with("user:") || s == "user");
    if !sender_is_human {
        return false;
    }
    if every_message || replied_to_mine {
        return true;
    }
    let me = format!("agent:{project_id}");
    if msg
        .rest
        .get("mentions")
        .and_then(Value::as_array)
        .is_some_and(|a| a.iter().any(|m| m.as_str() == Some(me.as_str())))
    {
        return true;
    }
    let text = msg.text().to_lowercase();
    let has = |token: &str| {
        let t = format!("@{token}");
        text.match_indices(&t).any(|(i, _)| {
            let after = text[i + t.len()..].chars().next();
            after.is_none_or(|c| !(c.is_alphanumeric() || c == '-' || c == '_'))
        })
    };
    (!slug.is_empty() && has(slug)) || (only_agent && has("arbos"))
}

/// Recent human and agent lines before `up_to` as `[{from, text}]`, oldest first, for the
/// responder decision.
fn recent_people_context(messages: &[MessageView], up_to: &str, max: usize) -> Vec<Value> {
    let mut out = Vec::new();
    for m in messages {
        if m.id == up_to {
            break;
        }
        if m.role != "user" && m.role != "assistant" {
            continue;
        }
        let text: String = m.text().chars().take(500).collect();
        if text.trim().is_empty() {
            continue;
        }
        let handle = m
            .rest
            .get("senderHandle")
            .and_then(Value::as_str)
            .unwrap_or(if m.role == "assistant" {
                "agent"
            } else {
                "user"
            });
        let from = if m.role == "assistant" {
            format!("@{handle} (agent)")
        } else {
            // Humans: display name when the client sent it; else bare handle (no @).
            message_spoken_name(m, handle)
        };
        out.push(json!({ "from": from, "text": text }));
    }
    if out.len() > max {
        out.drain(0..out.len() - max);
    }
    out
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
                        if !full {
                            // Added (or re-added) while running: the room's past is not ours.
                            self.sync.mark_late_join(&chat_id).await;
                        }
                        tracing::info!(chat = %chat_id, late = !full, "joined a people chat");
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
        h.doc.messages().iter().any(|m| {
            m.role == "assistant"
                && m.status != "cancelled"
                && m.rest.get("inReplyTo").and_then(Value::as_str) == Some(user_msg_id)
                && (m.rest.get("sender").and_then(Value::as_str) == Some(me.as_str())
                    || m.rest.get("deviceId").and_then(Value::as_str)
                        == Some(self.cfg.device_id.as_str()))
        })
    }

    /// Shared people `send` still waiting on another addressed agent: leave the command `taken`
    /// so their box can claim it. False when this is not a multi-agent address, or every named
    /// agent has already replied (or there is no message).
    pub async fn people_shared_send_open(
        &self,
        chat_id: &str,
        host: &SharedHost,
        user_msg_id: &str,
    ) -> bool {
        if user_msg_id.is_empty() || !self.people.is_people_chat(chat_id) {
            return false;
        }
        let Some(info) = self.people.info(chat_id) else {
            return false;
        };
        let h = host.lock().await;
        let Some(msg) = h.doc.message(user_msg_id) else {
            return false;
        };
        let shared = people_shared_agent_ids(&msg, &info);
        if shared.len() <= 1 {
            return false;
        }
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
        shared.iter().any(|id| !replied.contains(id))
    }

    /// Should this `send` command drive a turn here? People chats route by mention; a message
    /// not for this agent is left alone (its command stays pending for whoever it is for). In a
    /// group, an unaddressed message still asks the decision model — with recent chat context —
    /// whether this agent should reply.
    pub async fn people_wants(&self, chat_id: &str, host: &SharedHost, user_msg_id: &str) -> bool {
        let Some(info) = self.people.info(chat_id) else {
            return true;
        };
        let (wants, reason) = self.people_gate(chat_id, host, user_msg_id, &info).await;
        // The decision is part of the agent's record (`GET /projects/{id}/inspect`, kind
        // `gate`): "why did bob answer / stay quiet" is the first question in every routing bug.
        let _ = self.store.t3_append(
            chat_id,
            None,
            Some(chat_id),
            "gate",
            Some("system"),
            &format!(
                "gate {}: {reason}",
                if wants { "reply" } else { "skip" }
            ),
            Some(&json!({
                "message_id": user_msg_id, "reply": wants, "reason": reason,
                "me": self.cfg.project_id, "handle": self.identity.handle(),
                "reply_policy": info.settings["reply_policy"], "agents": info.agents(), "group": info.is_group(),
            })),
        );
        wants
    }

    /// The mention gate proper: the decision and the rule that made it.
    async fn people_gate(
        &self,
        chat_id: &str,
        host: &SharedHost,
        user_msg_id: &str,
        info: &PeopleChatInfo,
    ) -> (bool, &'static str) {
        let (msg, replied_to_mine, recent) = {
            let h = host.lock().await;
            let Some(msg) = h.doc.message(user_msg_id) else {
                return (false, "message not in the doc");
            };
            let me = format!("agent:{}", self.cfg.project_id);
            let replied_to_mine = msg
                .rest
                .get("replyTo")
                .and_then(Value::as_str)
                .and_then(|id| h.doc.message(id))
                .is_some_and(|m| {
                    m.rest.get("sender").and_then(Value::as_str) == Some(me.as_str())
                        || (m.role == "assistant"
                            && m.rest.get("deviceId").and_then(Value::as_str)
                                == Some(self.cfg.device_id.as_str()))
                });
            let recent = recent_people_context(&h.doc.messages(), user_msg_id, RESPOND_RECENT_MAX);
            (msg, replied_to_mine, recent)
        };
        let every = info.settings["reply_policy"].as_str() == Some("every_message");
        // Bare person name / people-only address in the prose ("Allan?", "Ken") wins over a
        // sticky agent chip or reply-to, unless the text @-mentions an agent or the policy is
        // every_message. The decision model must not wake bob for a ping meant for Allan.
        if !every && people_ping_not_for_agents(&msg, info) {
            return (false, "ping for a person, not for agents");
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
        if addressed(
            &msg,
            &self.cfg.project_id,
            &slug,
            info.agents() == 1,
            every,
            replied_to_mine,
        ) {
            return (
                true,
                if every {
                    "reply_policy every_message"
                } else if replied_to_mine {
                    "reply to my message"
                } else {
                    "addressed to me (mention, @handle or sole agent)"
                },
            );
        }
        // Bare agent name / collective address ("Bob?", "hey Bob …", "can the agents respond?")
        // — same as an @mention, including in DMs where the decision model does not run.
        if text_names_this_agent(&msg, info, &self.cfg.project_id) {
            return (true, "text names me");
        }
        if text_addresses_agents_collectively(&msg) {
            return (true, "text addresses the agents collectively");
        }
        // Addressed to another agent by mention, @slug or name: theirs, not ours — even when
        // that agent is silent, and even for a steer while this agent's own turn is running.
        if addressed_to_other_agents_only(&msg, info, &self.cfg.project_id) {
            return (false, "addressed to another agent only");
        }
        // A person chip or leading @handle with no agent address is for humans — stay quiet
        // even in a group (the decision model must not wake an agent to decline a call meant
        // for Ken). Mid-line @person references alone still reach the decision model.
        if addressed_only_to_people(&msg) {
            return (false, "addressed to people only");
        }
        if !info.is_group() {
            return (false, "unaddressed in a DM");
        }
        let chosen = self
            .people_decide_respond(chat_id, info, &msg, &recent)
            .await;
        (
            chosen,
            if chosen {
                "respond decision model chose me"
            } else {
                "respond decision model did not choose me"
            },
        )
    }

    /// Responder decision for an unaddressed human message in a people group (`responders.rs`):
    /// one state naming every agent in the chat (handle + description) with the recent
    /// transcript, one yes/no probability per agent; this agent replies when its own answer
    /// reaches the threshold. Every box in the chat asks the same question over the same state,
    /// so the agents agree on who continues the thread. Fail-open: the LLM router, then stay quiet
    /// (the previous default for an unaddressed message).
    async fn people_decide_respond(
        &self,
        chat_id: &str,
        info: &PeopleChatInfo,
        msg: &MessageView,
        recent: &[Value],
    ) -> bool {
        let text: String = msg.text().chars().take(4000).collect();
        if text.trim().is_empty() {
            return false;
        }
        let asker_handle = msg
            .rest
            .get("senderHandle")
            .and_then(Value::as_str)
            .unwrap_or("someone");
        let asker = message_spoken_name(msg, asker_handle);
        let me = self.cfg.project_id.as_str();
        let candidates: Vec<Candidate> = info
            .members
            .iter()
            .filter_map(|m| {
                let id = m["member"].as_str()?.strip_prefix("agent:")?;
                let handle = m["handle"].as_str().unwrap_or(id);
                let role = m["description"]
                    .as_str()
                    .filter(|d| !d.trim().is_empty())
                    .or_else(|| m["project"]["description"].as_str())
                    .unwrap_or("")
                    .to_owned();
                Some(Candidate {
                    id: id.to_owned(),
                    name: format!("@{handle}"),
                    role,
                    host: false,
                })
            })
            .collect();
        if !candidates.iter().any(|c| c.id == me) {
            return false;
        }
        let room = Room {
            kind: "people_group",
            chat_id: chat_id.to_owned(),
            title: info.kind.clone(),
            from: asker,
            text,
            recent: recent.to_vec(),
            candidates,
            people: info.people_labels(),
        };
        match self.decide_responders(&room).await {
            Ok(d) => {
                self.log_responder_decision(&room, &msg.id, Ok(&d), None);
                d.chose(me)
            }
            Err(errors) => {
                self.log_responder_decision(&room, &msg.id, Err(&errors), Some(("SKIP", &[])));
                false
            }
        }
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
                recent_people_context(&h.doc.messages(), user_msg_id, RESPOND_RECENT_MAX)
            } else {
                Vec::new()
            };
            (msg, recent)
        };
        let sender = msg
            .rest
            .get("sender")
            .and_then(Value::as_str)
            .unwrap_or("user")
            .to_owned();
        let user_id = sender
            .strip_prefix("user:")
            .unwrap_or(&info.owner_user)
            .to_owned();
        let handle = msg
            .rest
            .get("senderHandle")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| info.handle_of(&user_id));
        let spoken = message_spoken_name(&msg, &info.spoken_name_of(&user_id));
        let role = if user_id == info.owner_user {
            "owner"
        } else {
            "guest"
        };
        let asker = Asker {
            user_id,
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
                "; other agents present (they answer when @mentioned or when the reply gate picks them): {}",
                other_agents.join(", ")
            )
        };
        let visibility = if info.is_group() {
            " You can read the whole shared transcript — every human and every agent message in this chat, not only ones that address you."
        } else {
            ""
        };
        let owner_spoken = info.spoken_name_of(&info.owner_user);
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
        let mut prelude = format!(
            "[People chat with {}. You are @{my_slug} (the agent of {owner_spoken}'s Project \"{}\"){}.{} This message is from {spoken} ({role}); treat it as a request from another person in the conversation and address them by their display name ({spoken}), never as an @handle. Your reply is read by everyone in the chat.{secrets_rule}]\n",
            people.join(", "),
            self.cfg.project_id,
            others_clause,
            visibility,
        );
        if !recent.is_empty() {
            prelude.push_str("Recent messages in this shared chat:\n");
            for line in &recent {
                let from = line["from"].as_str().unwrap_or("?");
                let text = line["text"].as_str().unwrap_or("");
                prelude.push_str(&format!("- {from}: {text}\n"));
            }
        }
        prelude.push('\n');
        Some(prelude)
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
        let name = msg.rest.get("senderName").and_then(Value::as_str);
        let handle = msg.rest.get("senderHandle").and_then(Value::as_str);
        owner_address_note_text(name, handle, &self.name())
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
    clean(sender_name)
        .filter(|n| usable(n))
        .or_else(|| clean(sender_handle).filter(|h| usable(h)))
}

/// The per-turn identity note for the project owner: who the user is, and that the agent's
/// name is a name (not a place: "What do you want to work on in Doug?" — same feedback).
pub fn owner_address_note_text(
    sender_name: Option<&str>,
    sender_handle: Option<&str>,
    agent_name: &str,
) -> Option<String> {
    let name = user_address_name(sender_name, sender_handle, agent_name)?;
    let agent = agent_name.trim();
    let about_me = if agent.is_empty() || agent.eq_ignore_ascii_case("arbos") {
        String::new()
    } else {
        format!(
            " You are {agent}; \"{agent}\" is your own name, not a place, workspace or product — never say \"in {agent}\" or \"on {agent}\"."
        )
    };
    Some(format!(
        "[The user is {name}. Address them as {name}, never as an @handle and never as \"Arbos\" (Arbos is the app).{about_me}]\n\n"
    ))
}

#[cfg(test)]
mod address_tests {
    use super::*;

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

    #[test]
    fn product_name_as_display_name_falls_back_to_the_handle() {
        // Jacob's `unarbos` GitHub profile is literally named "Arbos".
        assert_eq!(
            user_address_name(Some("Arbos"), Some("unarbos"), "Doug").as_deref(),
            Some("unarbos")
        );
        assert_eq!(
            user_address_name(Some("arbos"), Some("@unarbos"), "Doug").as_deref(),
            Some("unarbos")
        );
        let note = owner_address_note_text(Some("Arbos"), Some("unarbos"), "Doug").unwrap();
        assert!(note.contains("The user is unarbos. Address them as unarbos"));
        assert!(!note.contains("Address them as Arbos"));
    }

    #[test]
    fn agent_name_as_display_name_falls_back_too() {
        assert_eq!(
            user_address_name(Some("Doug"), Some("jake"), "Doug").as_deref(),
            Some("jake")
        );
        // Neither usable: no note rather than a wrong one.
        assert_eq!(user_address_name(Some("Arbos"), Some("Doug"), "Doug"), None);
        assert_eq!(owner_address_note_text(Some("Arbos"), None, "Doug"), None);
        assert_eq!(user_address_name(None, None, "Doug"), None);
        assert_eq!(user_address_name(Some("  "), Some(""), "Doug"), None);
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
    pub fn people_reply_fields(&self, chat_id: &str) -> Option<ReplyAttribution> {
        let info = self.people.info(chat_id)?;
        let asker = self.people.asker(chat_id)?;
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
        let fields = vec![
            ("sender", json!(format!("agent:{}", self.cfg.project_id))),
            ("senderHandle", json!(handle)),
            ("senderName", json!(name)),
            ("senderAvatar", json!(avatar)),
            ("senderAvatarColor", json!(avatar_color)),
            ("ownerHandle", json!(info.owner_handle())),
            ("askedBy", json!(format!("user:{}", asker.user_id))),
            ("askedByHandle", json!(asker.handle)),
            ("inReplyTo", json!(asker.message_id)),
            // Platform reply feature: thread this bubble to the asker's message
            // so everyone sees which line is being answered (feedback
            // app-01M5MPKJ9K6HGHFH6BZWR9CME0).
            ("replyTo", json!(asker.message_id)),
        ];
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

#[cfg(test)]
mod tests {
    //! Mention routing: which human messages a shared agent answers.

    use serde_json::json;

    use super::*;

    fn msg(text: &str, extra: Value) -> MessageView {
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

    #[test]
    fn mention_routing() {
        // Resolved mention from the composer.
        assert!(addressed(
            &msg("hey", json!({ "mentions": ["agent:01HPROJ"] })),
            P,
            "research",
            false,
            false,
            false
        ));
        assert!(!addressed(
            &msg("hey", json!({ "mentions": ["agent:OTHER"] })),
            P,
            "research",
            false,
            false,
            false
        ));
        // Text mention by slug, with word boundary.
        assert!(addressed(
            &msg("@research what do you think?", json!({})),
            P,
            "research",
            false,
            false,
            false
        ));
        assert!(addressed(
            &msg("ping @Research!", json!({})),
            P,
            "research",
            false,
            false,
            false
        ));
        assert!(!addressed(
            &msg("@research-2 is another one", json!({})),
            P,
            "research",
            false,
            false,
            false
        ));
        assert!(!addressed(
            &msg("no mention here", json!({})),
            P,
            "research",
            false,
            false,
            false
        ));
        // @arbos only when this is the only agent in the chat.
        assert!(addressed(
            &msg("@arbos help", json!({})),
            P,
            "research",
            true,
            false,
            false
        ));
        assert!(!addressed(
            &msg("@arbos help", json!({})),
            P,
            "research",
            false,
            false,
            false
        ));
        // Reply-to one of my messages, or the every-message policy.
        assert!(addressed(
            &msg("and this?", json!({})),
            P,
            "research",
            false,
            false,
            true
        ));
        assert!(addressed(
            &msg("anything", json!({})),
            P,
            "research",
            false,
            true,
            false
        ));
        // Agents' and other roles' messages never drive a turn.
        let mut from_agent = msg("@research", json!({}));
        from_agent.rest["sender"] = json!("agent:OTHER");
        assert!(!addressed(&from_agent, P, "research", true, true, true));
        let mut assistant = msg("@research", json!({}));
        assistant.role = "assistant".into();
        assert!(!addressed(&assistant, P, "research", true, true, true));
        // Person-only mentions: chip (no @ in prose) or leading @handle — for humans.
        assert!(addressed_only_to_people(&msg(
            "Call?",
            json!({ "mentions": ["user:gh_ken"] })
        )));
        assert!(addressed_only_to_people(&msg(
            "@ken Call?",
            json!({ "mentions": ["user:gh_ken"] })
        )));
        assert!(addressed_only_to_people(&msg(
            "Yo @ken",
            json!({ "mentions": ["user:gh_ken"] })
        )));
        // Mid-line @person is a reference, not a people-only address.
        assert!(!addressed_only_to_people(&msg(
            "Would you rent a machine if @kenobijon asked ?",
            json!({ "mentions": ["user:kenobijon"] })
        )));
        assert!(!addressed_only_to_people(&msg(
            "can you see @jo?",
            json!({ "mentions": ["user:jo"] })
        )));
        assert!(!addressed_only_to_people(&msg(
            "Call?",
            json!({ "mentions": ["user:gh_ken", "agent:01HPROJ"] })
        )));
        assert!(!addressed_only_to_people(&msg("Call?", json!({}))));
        assert!(!addressed_only_to_people(&msg(
            "hey",
            json!({ "mentions": ["agent:01HPROJ"] })
        )));

        // Leading person name — even with a sticky agent mention — is for humans.
        let info = PeopleChatInfo {
            members: vec![
                json!({ "member": "user:allan", "handle": "catoneone", "name": "Allan" }),
                json!({ "member": "agent:01HPROJ", "handle": "bob" }),
            ],
            slug: "bob".into(),
            kind: "dm".into(),
            ..Default::default()
        };
        assert_eq!(leading_address_token("Allan?").as_deref(), Some("allan"));
        assert_eq!(
            leading_address_token("Allan. we are adding the notifications").as_deref(),
            Some("allan")
        );
        assert_eq!(leading_address_token("Yo Allan").as_deref(), Some("allan"));
        assert_eq!(
            leading_address_token("can you see @jo?").as_deref(),
            Some("can")
        );
        assert!(text_names_people(&msg("Allan?", json!({})), &info));
        assert!(people_ping_not_for_agents(
            &msg("Allan?", json!({ "mentions": ["agent:01HPROJ"] })),
            &info
        ));
        // Mid-line person @handle keeps a sticky agent (bob still answers about @jo).
        assert!(!people_ping_not_for_agents(
            &msg(
                "can you see @jo?",
                json!({ "mentions": ["user:jo", "agent:01HPROJ"] })
            ),
            &info
        ));
        // Mid-line @person with no agent address is also not a people ping — the group
        // decision model may still wake an agent (e.g. follow-up that names a human).
        assert!(!people_ping_not_for_agents(
            &msg(
                "Would you rent a machine if @kenobijon asked ?",
                json!({ "mentions": ["user:kenobijon"] })
            ),
            &info
        ));
        assert!(!people_ping_not_for_agents(
            &msg(
                "@bob what about Allan?",
                json!({ "mentions": ["agent:01HPROJ"] })
            ),
            &info
        ));
        assert!(!people_ping_not_for_agents(
            &msg("what's 2+2?", json!({ "mentions": ["agent:01HPROJ"] })),
            &info
        ));

        // Bare agent name — same as @bob (DM + group). Person @handle still wins on collision;
        // a person's display name alone does not beat the agent handle/slug.
        assert!(text_names_this_agent(&msg("Bob?", json!({})), &info, P));
        assert!(text_names_this_agent(
            &msg("hey Bob what do you think?", json!({})),
            &info,
            P
        ));
        assert!(!text_names_this_agent(&msg("Allan?", json!({})), &info, P));
        assert!(!text_names_this_agent(
            &msg("can you see @jo?", json!({})),
            &info,
            P
        ));
        assert!(!text_names_this_agent(
            &msg("Bob?", json!({})),
            &info,
            "OTHER"
        ));
        assert!(text_addresses_agents_collectively(&msg(
            "can the agents respond?",
            json!({})
        )));
        assert!(text_addresses_agents_collectively(&msg(
            "hey agents",
            json!({})
        )));
        assert!(!text_addresses_agents_collectively(&msg(
            "Allan?",
            json!({})
        )));

        // Display-name collision: human @unarbos named "Arbos", agent @arbos — bare "Arbos"
        // is a clear agent reference (and `@arbos` already works via the sole-agent alias).
        let arbos_chat = PeopleChatInfo {
            members: vec![
                json!({ "member": "user:unarbos", "handle": "unarbos", "name": "Arbos" }),
                json!({ "member": "user:const", "handle": "const", "name": "Const" }),
                json!({
                    "member": "agent:01HPROJ",
                    "handle": "arbos",
                    "project": { "name": "Arbos" }
                }),
            ],
            slug: "arbos".into(),
            kind: "group".into(),
            ..Default::default()
        };
        assert!(text_names_people(
            &msg("Arbos what is your name", json!({})),
            &arbos_chat
        ));
        assert!(!text_names_people_by_handle(
            &msg("Arbos what is your name", json!({})),
            &arbos_chat
        ));
        assert!(text_names_any_agent_handle(
            &msg("Arbos what is your name", json!({})),
            &arbos_chat
        ));
        assert!(!people_ping_not_for_agents(
            &msg("Arbos what is your name", json!({})),
            &arbos_chat
        ));
        assert!(!people_ping_not_for_agents(
            &msg("Arbos", json!({})),
            &arbos_chat
        ));
        assert!(!people_ping_not_for_agents(
            &msg("Hey Arbos", json!({})),
            &arbos_chat
        ));
        assert!(text_names_this_agent(
            &msg("Arbos what is your name", json!({})),
            &arbos_chat,
            P
        ));
        assert!(text_names_this_agent(
            &msg("Arbos", json!({})),
            &arbos_chat,
            P
        ));
        assert!(text_names_this_agent(
            &msg("Hey Arbos", json!({})),
            &arbos_chat,
            P
        ));
        // Person @handle still silences the agent.
        assert!(people_ping_not_for_agents(
            &msg("unarbos?", json!({})),
            &arbos_chat
        ));
        assert!(!text_names_this_agent(
            &msg("unarbos?", json!({})),
            &arbos_chat,
            P
        ));

        // Shared address: collective phrase → every agent; two @mentions → those two.
        let group = PeopleChatInfo {
            members: vec![
                json!({ "member": "agent:01HPROJ", "handle": "bob" }),
                json!({ "member": "agent:OTHER", "handle": "arbos" }),
                json!({ "member": "user:gh_2", "handle": "sam" }),
            ],
            slug: "bob".into(),
            kind: "group".into(),
            ..Default::default()
        };
        let shared = people_shared_agent_ids(&msg("Hey agents? Say hi", json!({})), &group);
        assert_eq!(shared.len(), 2);
        assert!(shared.contains(&"01HPROJ".to_string()));
        assert!(shared.contains(&"OTHER".to_string()));
        let two = people_shared_agent_ids(
            &msg(
                "hi",
                json!({ "mentions": ["agent:01HPROJ", "agent:OTHER"] }),
            ),
            &group,
        );
        assert_eq!(two.len(), 2);
        assert!(people_shared_agent_ids(
            &msg("hi", json!({ "mentions": ["agent:01HPROJ"] })),
            &group
        )
        .is_empty());
    }

    /// Arbonites, 2026-10-23 (#vault-approval-card): "@gigchadagent you still borked?" reached
    /// hjkb as a steer while its own turn ran, and hjkb answered. A message that names another
    /// agent — by resolved mention, by @slug, or by its bare name at the start — is that agent's
    /// alone: this agent stays silent and the decision model is not asked. Naming this agent too
    /// (both, or "hey agents") still counts as addressed.
    #[test]
    fn message_for_another_agent_is_not_ours() {
        let arbonites = PeopleChatInfo {
            members: vec![
                json!({ "member": "agent:01HPROJ", "handle": "hjkb", "project": { "name": "hjkb" } }),
                json!({ "member": "agent:GIG", "handle": "gigchadagent", "project": { "name": "GigChadAgent" } }),
                json!({ "member": "user:gh_2", "handle": "unarbos", "name": "Arbos" }),
            ],
            slug: "hjkb".into(),
            kind: "group".into(),
            ..Default::default()
        };
        // Resolved mention of the other agent (what the Mac composer sends).
        assert!(addressed_to_other_agents_only(
            &msg(
                "@gigchadagent you still borked?",
                json!({ "mentions": ["agent:GIG"] })
            ),
            &arbonites,
            P
        ));
        // Prose-only @slug of the other agent; bare name at the start; the display-name slug.
        assert!(addressed_to_other_agents_only(
            &msg("hey @gigchadagent use your engy key", json!({})),
            &arbonites,
            P
        ));
        assert!(addressed_to_other_agents_only(
            &msg("GigChadAgent, are you there?", json!({})),
            &arbonites,
            P
        ));
        assert!(addressed_to_other_agents_only(
            &msg("ping @GigChadAgent", json!({})),
            &arbonites,
            P
        ));
        // Both named, or this agent named: not "other only".
        assert!(!addressed_to_other_agents_only(
            &msg("hi", json!({ "mentions": ["agent:GIG", "agent:01HPROJ"] })),
            &arbonites,
            P
        ));
        assert!(!addressed_to_other_agents_only(
            &msg("@gigchadagent and @hjkb both try", json!({})),
            &arbonites,
            P
        ));
        assert!(!addressed_to_other_agents_only(
            &msg("@hjkb try to use that engy key now", json!({})),
            &arbonites,
            P
        ));
        // Nobody named (plain group chatter) — the decision model may still run.
        assert!(!addressed_to_other_agents_only(
            &msg("done yet?", json!({})),
            &arbonites,
            P
        ));
        // A mid-line reference to the other agent, with no address to anyone, is chatter too
        // ("ive borked @gigchadagent in this chat it seems" names it — theirs, not ours).
        assert!(addressed_to_other_agents_only(
            &msg(
                "ive borked @gigchadagent in this chat it seems",
                json!({ "mentions": ["agent:GIG"] })
            ),
            &arbonites,
            P
        ));
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
        // Context for "m12": lines m0…m11, capped to the last 5.
        let ctx = recent_people_context(&lines, "m12", 5);
        assert_eq!(ctx.len(), 5);
        assert_eq!(ctx[0]["text"], "line 7");
        assert_eq!(ctx[4]["text"], "line 11");
        assert_eq!(ctx[4]["from"], "@bob (agent)");
        assert_eq!(ctx[3]["from"], "sam");
        // Unknown id → everything, still capped.
        let all = recent_people_context(&lines, "missing", 3);
        assert_eq!(all.len(), 3);
        assert_eq!(all[2]["text"], "line 14");
    }
}

#[cfg(test)]
mod gate_record_tests {
    //! Every mention-gate decision leaves a `gate` T3 row with its reason, so
    //! `GET /projects/{id}/inspect` can answer "why did this agent stay quiet".

    use arbos_core::ids::ulid;

    use super::*;
    use crate::status::tests::test_agent;

    #[tokio::test]
    async fn gate_decision_is_recorded_with_its_reason() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATGATERECORD000001";
        agent.people.chats.lock().expect("people").insert(
            chat.to_owned(),
            PeopleChatInfo {
                members: vec![
                    json!({ "member": "user:gh_2", "handle": "sam" }),
                    json!({ "member": format!("agent:{}", agent.cfg.project_id), "handle": "doug" }),
                    json!({ "member": "agent:OTHER", "handle": "hjkb" }),
                ],
                settings: json!({ "reply_policy": "mention" }),
                owner_user: "gh_1".into(),
                slug: "doug".into(),
                kind: "group".into(),
            },
        );
        let host = agent.sync.open_room(chat).await.unwrap();
        let for_other = ulid();
        let for_me = ulid();
        {
            let h = host.lock().await;
            let opts = arbos_core::doc::UserMessageOpts {
                mentions: vec!["agent:OTHER".into()],
                ..Default::default()
            };
            h.doc
                .append_user_message(&for_other, "@hjkb you still borked?", "dev-mac", &opts)
                .unwrap();
            let opts = arbos_core::doc::UserMessageOpts {
                mentions: vec![format!("agent:{}", agent.cfg.project_id)],
                ..Default::default()
            };
            h.doc
                .append_user_message(&for_me, "@doug status?", "dev-mac", &opts)
                .unwrap();
        }
        assert!(!agent.people_wants(chat, &host, &for_other).await);
        assert!(agent.people_wants(chat, &host, &for_me).await);

        let rows = agent.store.t3_after(0, Some(chat), 50).unwrap();
        let gates: Vec<&(crate::store::T3Row, Option<Value>)> =
            rows.iter().filter(|(r, _)| r.kind == "gate").collect();
        assert_eq!(gates.len(), 2, "{rows:?}");
        let (skip, skip_data) = gates[0];
        assert!(skip
            .text
            .starts_with("gate skip: addressed to another agent only"));
        let skip_data = skip_data.as_ref().expect("gate row carries its facts");
        assert_eq!(skip_data["reply"], false);
        assert_eq!(skip_data["message_id"], for_other);
        assert_eq!(skip_data["handle"], "doug");
        let (reply, _) = gates[1];
        assert!(reply.text.starts_with("gate reply: addressed to me"));
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
}
