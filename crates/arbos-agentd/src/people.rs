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
//! (`owner` | `guest`) in the prompt. The author is the row's `sender: user:<id>`, else the
//! person whose device wrote it (the relay's box roster lists each member's device ids), else
//! the only human in the chat; a row nobody can be matched to is "a member" with guest powers
//! — the owner is never assumed (#1580). Guests may direct ordinary work; they may not use the
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
    /// The owner's current name from the relay profile (#1895's owner block) at the time this
    /// snapshot was taken (every poll). One owner, found by `owner_user`, named by this — never
    /// by a row's `senderName` of the time (Hezbola, 2026-10-26: "the chat uses two names for
    /// the owner, Arbos and Jacob" — old rows said Arbos, the profile said Jacob).
    pub owner_address: Option<String>,
}

impl PeopleChatInfo {
    /// Other agents of this chat whose `@handle` (or `@slug` of the name) appears in `text`.
    pub(crate) fn other_agents_mentioned(&self, text: &str, me: &str) -> Vec<String> {
        let lower = text.to_lowercase();
        let has = |token: &str| {
            let t = format!("@{token}");
            !token.is_empty()
                && lower.match_indices(&t).any(|(i, _)| {
                    let after = lower[i + t.len()..].chars().next();
                    after.is_none_or(|c| !(c.is_alphanumeric() || c == '-' || c == '_'))
                })
        };
        self.members
            .iter()
            .filter_map(|m| {
                let id = m["member"].as_str()?.strip_prefix("agent:")?;
                if format!("agent:{id}") == me {
                    return None;
                }
                let handle = m["handle"].as_str().unwrap_or("").to_lowercase();
                let name = m["project"]["name"]
                    .as_str()
                    .or_else(|| m["name"].as_str())
                    .unwrap_or("")
                    .trim()
                    .to_lowercase()
                    .replace(' ', "-");
                (has(&handle) || has(&name)).then(|| id.to_owned())
            })
            .collect()
    }

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

    /// Human members by spoken name, for the responder decision's people roster.
    fn people_labels(&self) -> Vec<String> {
        self.members
            .iter()
            .filter(|m| m["member"].as_str().is_some_and(|s| s.starts_with("user:")))
            .map(person_spoken_name)
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
    // Typed mentions (#1538): a `user:` chip for the SENDER is not a people ping — nobody
    // @-mentions themselves. It happens when an agent's slug equals the sender's own handle
    // (Jacob @unarbos named an agent "unarbos"): the composer resolved `@unarbos` to the
    // person, and the agent of that slug is who was meant. Other people's chips still are.
    let sender = msg.rest.get("sender").and_then(Value::as_str).unwrap_or("");
    let mut saw_person = false;
    for m in mentions {
        let Some(s) = m.as_str() else {
            continue;
        };
        if s.starts_with("agent:") {
            return false;
        }
        if s.starts_with("user:") && s != sender {
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
    let sender = msg.rest.get("sender").and_then(Value::as_str).unwrap_or("");
    text_token_names_people_except(&token, info, true, sender)
}

/// True when the leading token is a human's `@handle` (not merely their display name).
/// The sender's own handle does not count (#1538).
fn text_names_people_by_handle(msg: &MessageView, info: &PeopleChatInfo) -> bool {
    let Some(token) = leading_address_token(&msg.text()) else {
        return false;
    };
    let sender = msg.rest.get("sender").and_then(Value::as_str).unwrap_or("");
    text_token_names_people_except(&token, info, false, sender)
}

/// Does `token` name a human in the chat (by `@handle`, or by display / first name when
/// allowed), ignoring the member `except` (the sender: "@unarbos" written by @unarbos is not a
/// ping to themselves — #1538, an agent may carry that slug).
fn text_token_names_people_except(
    token: &str,
    info: &PeopleChatInfo,
    allow_display_name: bool,
    except: &str,
) -> bool {
    for m in &info.members {
        let Some(member) = m["member"].as_str() else {
            continue;
        };
        if !member.starts_with("user:") || member == except {
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
        // Agents named only as the object of a request to another agent ("@hezbola … with
        // @doug-agent") owe no reply; the `send` must not stay `taken` waiting for them.
        let text = msg.text();
        let kept: Vec<String> = mentioned
            .iter()
            .filter(|id| {
                let me = format!("agent:{id}");
                let Some(m) = info
                    .members
                    .iter()
                    .find(|m| m["member"].as_str() == Some(me.as_str()))
                else {
                    return true;
                };
                let handle = m["handle"].as_str().unwrap_or("");
                let name = m["project"]["name"]
                    .as_str()
                    .or_else(|| m["name"].as_str())
                    .unwrap_or("")
                    .trim()
                    .replace(' ', "-");
                let role = mention_role(&text, handle).max(mention_role(&text, &name));
                !(role == MentionRole::Object && another_agent_addressed(msg, info, id))
            })
            .cloned()
            .collect();
        return kept;
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
    if mention_is_object(&text, &info.slug.to_lowercase()) {
        return false;
    }
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
    /// How to address the owner, from the relay's profile at every poll (never from a row's
    /// `senderName`, which is whatever one device sent): `(address_name, label_name)` —
    /// `address_name` is the name the user set (PATCH /me), `label_name` the GitHub-seeded one.
    owner_profile: Mutex<(Option<String>, Option<String>)>,
    /// The whole owner block as last seen (for the prompt's "Your owner" section).
    owner_full: Mutex<crate::identity::OwnerProfile>,
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

    /// Test-only: seed a people-chat snapshot (e.g. rename + @handle refresh).
    #[cfg(test)]
    pub(crate) fn test_insert(&self, chat_id: &str, info: PeopleChatInfo) {
        self.chats
            .lock()
            .expect("people")
            .insert(chat_id.to_owned(), info);
    }
}

/// "can you send a message to @testagent?" names @testagent as the *object* of a request made
/// to someone else, not as the one asked. A mention right after "to" in a send / pass /
/// forward / relay / message phrase is such an object (2026-10-24 00:48: TestAgent answered
/// "Arbos, I am @testagent…" to a request meant for TestAgent2).
pub(crate) fn mention_is_object(text_lower: &str, slug_lower: &str) -> bool {
    if slug_lower.is_empty() {
        return false;
    }
    let needle = format!("to @{slug_lower}");
    text_lower.match_indices(&needle).any(|(i, _)| {
        let after = text_lower[i + needle.len()..].chars().next();
        let ends_token = after.is_none_or(|c| !(c.is_alphanumeric() || c == '-' || c == '_'));
        let before = &text_lower[..i];
        let verbs = [
            "send",
            "sending",
            "sent",
            "pass",
            "passing",
            "forward",
            "forwarding",
            "relay",
            "relaying",
            "message",
            "messaging",
            "msg",
            "dm",
            "a note",
            "a message",
            "word",
        ];
        ends_token
            && verbs.iter().any(|v| {
                before.rfind(v).is_some_and(|j| {
                    before.len() - j <= 60 && !before[j..].contains(['.', '?', '!'])
                })
            })
    })
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

/// True when some *other* agent of the chat is addressed by the prose (vocative @mention or a
/// leading bare name). Used to tell "@hezbola … with @doug" (Doug is the object of a request to
/// Hezbola) from "lunch with @doug?" (nobody else addressed; the decision model judges).
fn another_agent_addressed(msg: &MessageView, info: &PeopleChatInfo, project_id: &str) -> bool {
    let me = format!("agent:{project_id}");
    let text = msg.text();
    let leading = leading_address_token(&text);
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
        for h in [handle.as_str(), slug.as_str()] {
            if h.is_empty() {
                continue;
            }
            if mention_role(&text, h) == MentionRole::Addressed {
                return true;
            }
            if leading.as_deref() == Some(h) {
                return true;
            }
        }
        if let Some(t) = &leading {
            if !name.is_empty() && (name == *t || name.split_whitespace().next() == Some(t)) {
                return true;
            }
        }
    }
    false
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
    let text = msg.text().to_lowercase();
    if mention_is_object(&text, &slug.to_lowercase()) {
        return false;
    }
    if msg
        .rest
        .get("mentions")
        .and_then(Value::as_array)
        .is_some_and(|a| a.iter().any(|m| m.as_str() == Some(me.as_str())))
    {
        return true;
    }
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

/// An active exchange: this agent answered this person moments ago, and nobody else took the
/// thread since. Their next un-mentioned line ("Who is in this chat?", "How many messages are
/// there?") is for the agent — the decision model must not get a vote on it (2026-10-23 23:53,
/// Arbonites: jev scored the follow-ups 0.29 and 0.47 against a 0.50 bar; #1573).
pub const FOLLOW_UP_WINDOW_MS: i64 = 3 * 60 * 1000;
/// How many rows back the agent's last reply may sit for the thread to still be "active".
pub const FOLLOW_UP_MESSAGES: usize = 8;

/// Is `user_msg_id` a direct follow-up in an active exchange between its sender and `me`
/// (`agent:<project_id>`)? True when, within the last [`FOLLOW_UP_MESSAGES`] rows and
/// [`FOLLOW_UP_WINDOW_MS`], the newest agent reply is this agent's and it answered the same
/// person; false as soon as another agent replied or was @mentioned in between.
pub(crate) fn follow_up_in_active_exchange(
    messages: &[MessageView],
    user_msg_id: &str,
    me: &str,
) -> bool {
    let Some(i) = messages.iter().position(|m| m.id == user_msg_id) else {
        return false;
    };
    let msg = &messages[i];
    if msg.role != "user" {
        return false;
    }
    let sender = msg.rest.get("sender").and_then(Value::as_str).unwrap_or("");
    if sender.is_empty() || sender.starts_with("agent:") || sender.starts_with("project:") {
        return false;
    }
    let at = msg
        .rest
        .get("createdAt")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let sender_of = |id: &str| -> Option<String> {
        messages
            .iter()
            .find(|m| m.id == id)
            .and_then(|m| m.rest.get("sender").and_then(Value::as_str))
            .map(str::to_owned)
    };
    let start = i.saturating_sub(FOLLOW_UP_MESSAGES);
    for m in messages[start..i].iter().rev() {
        if m.role == "assistant" {
            let from = m.rest.get("sender").and_then(Value::as_str).unwrap_or("");
            if from != me {
                // Another agent has the thread.
                return false;
            }
            let reply_at = m.rest.get("createdAt").and_then(Value::as_i64).unwrap_or(0);
            if at - reply_at > FOLLOW_UP_WINDOW_MS {
                return false;
            }
            // My reply must have been to this same person (or to nobody in particular).
            return match m
                .rest
                .get("inReplyTo")
                .and_then(Value::as_str)
                .and_then(sender_of)
            {
                Some(asked_by) => asked_by == sender,
                None => true,
            };
        }
        if m.role == "user"
            && m.rest
                .get("mentions")
                .and_then(Value::as_array)
                .is_some_and(|a| {
                    a.iter().any(|v| {
                        v.as_str()
                            .is_some_and(|s| s.starts_with("agent:") && s != me)
                    })
                })
        {
            // Someone called another agent in between: the thread is theirs now.
            return false;
        }
    }
    false
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
                        if !full {
                            // Added (or re-added) while running: the room's past is not ours.
                            self.sync.mark_late_join(&chat_id).await;
                        }
                        self.sync.mark_joined_at(&chat_id, joined_at).await;
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
        let (msg, replied_to_mine, recent, follow_up) = {
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
                // Sender only: the device id `box:<sandbox>` is shared by every agent of the
                // box, so another agent's row would read as mine (TestAgent / TestAgent2).
                .is_some_and(|m| m.rest.get("sender").and_then(Value::as_str) == Some(me.as_str()));
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
            let follow_up = follow_up_in_active_exchange(&all, user_msg_id, &me);
            (msg, replied_to_mine, recent, follow_up)
        };
        let every = info.settings["reply_policy"].as_str() == Some("every_message");
        // Bare person name / people-only address in the prose ("Allan?", "Ken") wins over a
        // sticky agent chip or reply-to, unless the text @-mentions an agent or the policy is
        // every_message. The decision model must not wake bob for a ping meant for Allan.
        if !every && people_ping_not_for_agents(&msg, info) {
            return (false, "ping for a person, not for agents");
        }
        // Another agent's row that @mentions me (its box pushed the send; `dispatch_agent_mentions`).
        if msg.role == "assistant" {
            let from = msg.rest.get("sender").and_then(Value::as_str).unwrap_or("");
            let me = format!("agent:{}", self.cfg.project_id);
            if from.starts_with("agent:")
                && from != me
                && text_mentions_this_agent(&msg, info, &self.cfg.project_id)
            {
                return (true, "mentioned by another agent in the chat");
            }
            return (false, "another agent's row, not addressed to me");
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
            if every {
                return (true, "reply_policy every_message");
            }
            if replied_to_mine {
                return (true, "reply to my message");
            }
            // An @mention is a consideration signal, not an obligation: being talked about
            // ("@hezbola can you communicate directly with @doug-agent?") is not being asked.
            match my_mention_role(&msg, info, &self.cfg.project_id, &slug) {
                MentionRole::Addressed | MentionRole::None => {
                    return (true, "addressed to me (mention, @handle or sole agent)");
                }
                MentionRole::Object
                    if another_agent_addressed(&msg, info, &self.cfg.project_id) =>
                {
                    return (
                        false,
                        "mentioned as the object of a request to another agent",
                    );
                }
                MentionRole::Object | MentionRole::Referenced => {
                    // Mentioned, but the sentence does not ask me: the decision model judges
                    // with the mention role in view. Fail-open is the old behaviour (reply).
                    return match self
                        .people_decide_respond_opt(chat_id, info, &msg, &recent)
                        .await
                    {
                        Some(true) => (true, "mentioned and the respond decision model chose me"),
                        Some(false) => (false, "mentioned but not asked (respond decision model)"),
                        None => (
                            true,
                            "mentioned; decision unavailable, replying (fail-open)",
                        ),
                    };
                }
            }
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
        // @mentions set the order, never a filter: a direct follow-up from the person this
        // agent just answered is answered, in context, without asking the decision model.
        if follow_up {
            return (true, "follow-up in an active exchange with me");
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
        self.people_decide_respond_opt(chat_id, info, msg, recent)
            .await
            .unwrap_or(false)
    }

    /// `people_decide_respond` with the failure case kept apart: `None` when every provider
    /// failed, so the caller picks its own fail-open default.
    async fn people_decide_respond_opt(
        &self,
        chat_id: &str,
        info: &PeopleChatInfo,
        msg: &MessageView,
        recent: &[Value],
    ) -> Option<bool> {
        let raw_text = msg.text();
        let text = crate::truncate::clip_marked(&raw_text, 4000, &msg.id, None);
        if text.trim().is_empty() {
            return Some(false);
        }
        let chips = mentioned_agent_ids(msg);
        let asker = author_spoken(msg, info, info.author_of(msg).as_deref());
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
        if !candidates.iter().any(|c| c.id == me) {
            return Some(false);
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
                Some(d.chose(me))
            }
            Err(errors) => {
                self.log_responder_decision(&room, &msg.id, Err(&errors), Some(("SKIP", &[])));
                // A budget refusal is not an outage: say so once in the chat and stop asking
                // the model for every message until the block lapses (`budget.rs`).
                if let Some(e) = errors
                    .iter()
                    .find(|e| crate::budget::Refusal::classify(e).is_some())
                {
                    self.note_budget_refusal(chat_id, e, false).await;
                }
                None
            }
        }
    }

    /// My reply in a people chat @mentions another agent of the chat: push a `send` for the
    /// row so that agent's gate sees it (an agent's bubble is not a command; nothing woke
    /// TestAgent when TestAgent2 wrote "@testagent, Arbos asked me to pass this on", 2026-10-24
    /// 00:48). One hop only: a reply to a human's message dispatches; a reply to another
    /// agent's row does not, so two agents cannot ping-pong.
    pub async fn dispatch_agent_mentions(
        self: &std::sync::Arc<Self>,
        chat_id: &str,
        host: &SharedHost,
        user_msg_id: &str,
        reply_msg_id: &str,
        text: &str,
    ) {
        let Some(info) = self.people.info(chat_id) else {
            return;
        };
        let me = format!("agent:{}", self.cfg.project_id);
        let targets = info.other_agents_mentioned(text, &me);
        if targets.is_empty() {
            return;
        }
        let mut h = host.lock().await;
        let from_human = h.doc.message(user_msg_id).is_some_and(|m| {
            m.rest
                .get("sender")
                .and_then(Value::as_str)
                .is_none_or(|s| s.starts_with("user:") || s == "user")
        });
        if !from_human {
            tracing::info!(
                chat_id,
                ?targets,
                "agent mention not dispatched: my turn was itself a reply to an agent (one hop)"
            );
            return;
        }
        let cmd_id = h.doc.push_command(
            "send",
            json!({ "messageId": reply_msg_id, "agentMention": true, "from": me }),
            &self.cfg.device_id,
        );
        h.mark_seen(&cmd_id);
        tracing::info!(chat_id, ?targets, cmd = %cmd_id, "agent mention dispatched as a send for the named agents");
        let _ = self.store.t3_append(
            chat_id,
            None,
            Some(chat_id),
            "agent_mention",
            Some("system"),
            &format!("my reply @mentions {} agent(s); send pushed", targets.len()),
            Some(&json!({ "message_id": reply_msg_id, "targets": targets, "command_id": cmd_id })),
        );
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
                "; other agents present (they answer when @mentioned or when the reply gate picks them): {}",
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
        let mut prelude = format!(
            "[People chat with {}. You are @{my_slug} (the agent of {owner_spoken}'s Project \"{}\"){}.{} {from_clause}. Your reply is read by everyone in the chat. If this message is for someone else, or only mentions or refers to you without asking you anything, say nothing: route `reply` with `reply` empty and no words — never post that you are staying silent, that the question is for someone else, or that you have nothing to add.{secrets_rule}]\n",
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

#[cfg(test)]
mod tests {
    //! Mention routing: which human messages a shared agent answers.

    use serde_json::json;

    use super::*;

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

    /// #1538: Jacob (@unarbos) named an agent "unarbos". The composer resolved `@unarbos` to
    /// the person (`mentions=[user:…]`), which silenced every agent; typed resolution: a
    /// `user:` chip for the sender is not a people ping, and the sender's own handle in the
    /// prose does not count as naming a person — so the agent with that slug is addressed.
    /// An `agent:` chip wakes the agent regardless; another person's chip still wins.
    #[test]
    fn sender_own_handle_resolves_to_the_agent_of_that_slug() {
        let arbonites = PeopleChatInfo {
            members: vec![
                json!({ "member": "agent:01HPROJ", "handle": "unarbos", "project": { "name": "unarbos" } }),
                json!({ "member": "user:gh_2", "handle": "unarbos", "name": "Arbos" }),
                json!({ "member": "user:gh_9", "handle": "scout", "name": "Scout" }),
            ],
            slug: "unarbos".into(),
            kind: "group".into(),
            ..Default::default()
        };
        // The Mac resolved the slug to the sender themself.
        let m = msg(
            "@unarbos are you there?",
            json!({ "mentions": ["user:gh_2"] }),
        );
        assert!(
            !addressed_only_to_people(&m),
            "a chip for the sender is not a people ping"
        );
        assert!(!text_names_people_by_handle(&m, &arbonites));
        assert!(!people_ping_not_for_agents(&m, &arbonites));
        assert!(
            addressed(&m, P, "unarbos", false, false, false),
            "the agent of that slug is addressed"
        );
        // A typed agent chip wakes the agent, whoever shares the slug.
        assert!(addressed(
            &msg("hey", json!({ "mentions": ["agent:01HPROJ"] })),
            P,
            "unarbos",
            false,
            false,
            false
        ));
        // Someone else pinging the person by chip: still the person's, not ours.
        let other = msg(
            "@unarbos can you look?",
            json!({ "sender": "user:gh_9", "mentions": ["user:gh_2"] }),
        );
        assert!(addressed_only_to_people(&other));
        assert!(
            people_ping_not_for_agents(&other, &arbonites)
                || text_mentions_agent(&other, &arbonites)
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
    //! Every mention-gate decision leaves a `gate` T3 row with its reason, so
    //! `GET /projects/{id}/inspect` can answer "why did this agent stay quiet".

    use arbos_core::ids::ulid;

    use super::tests::msg;
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
                owner_address: None,
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
        // Two rows in one millisecond sort by ulid, not by write order: pick by message id.
        let by_msg = |id: &str| {
            gates
                .iter()
                .find(|(_, d)| d.as_ref().and_then(|d| d["message_id"].as_str()) == Some(id))
                .copied()
                .expect("gate row for the message")
        };
        let (skip, skip_data) = by_msg(&for_other);
        assert!(skip
            .text
            .starts_with("gate skip: addressed to another agent only"));
        let skip_data = skip_data.as_ref().expect("gate row carries its facts");
        assert_eq!(skip_data["reply"], false);
        assert_eq!(skip_data["message_id"], for_other);
        assert_eq!(skip_data["handle"], "doug");
        let (reply, _) = by_msg(&for_me);
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

    /// Jacob (2026-10-23 23:54, Arbonites): "@testagent who is in this chat" was answered, the
    /// un-mentioned "How many messages are there?" ten seconds later was not (jev 0.47 < 0.50).
    /// @mentions set the order, never a filter: a follow-up from the person the agent just
    /// answered is for the agent, decision model or not.
    #[test]
    fn follow_up_in_an_active_exchange_is_for_me() {
        const ME: &str = "agent:01HPROJ";
        let t0 = 1_792_799_600_000_i64;
        let row = |id: &str, role: &str, sender: &str, at: i64, extra: Value| {
            let mut rest = json!({ "sender": sender, "createdAt": at });
            if let (Some(r), Some(e)) = (rest.as_object_mut(), extra.as_object()) {
                for (k, v) in e {
                    r.insert(k.clone(), v.clone());
                }
            }
            MessageView {
                id: id.into(),
                role: role.into(),
                status: "done".into(),
                parts: vec![json!({ "kind": "text", "text": "…" })],
                rest,
            }
        };
        let thread = vec![
            row("q1", "user", "user:const", t0, json!({ "mentions": [ME] })),
            row(
                "a1",
                "assistant",
                ME,
                t0 + 4_000,
                json!({ "inReplyTo": "q1" }),
            ),
            row("q2", "user", "user:const", t0 + 14_000, json!({})),
        ];
        assert!(follow_up_in_active_exchange(&thread, "q2", ME));
        // Someone else's question is not my follow-up.
        let other = vec![
            thread[0].clone(),
            thread[1].clone(),
            row("q2", "user", "user:sam", t0 + 14_000, json!({})),
        ];
        assert!(!follow_up_in_active_exchange(&other, "q2", ME));
        // Another agent answered in between: the thread is theirs.
        let taken = vec![
            thread[0].clone(),
            thread[1].clone(),
            row("a2", "assistant", "agent:OTHER", t0 + 8_000, json!({})),
            row("q2", "user", "user:const", t0 + 14_000, json!({})),
        ];
        assert!(!follow_up_in_active_exchange(&taken, "q2", ME));
        // Another agent was called in between.
        let called = vec![
            thread[0].clone(),
            thread[1].clone(),
            row(
                "q1b",
                "user",
                "user:const",
                t0 + 8_000,
                json!({ "mentions": ["agent:OTHER"] }),
            ),
            row("q2", "user", "user:const", t0 + 14_000, json!({})),
        ];
        assert!(!follow_up_in_active_exchange(&called, "q2", ME));
        // Too late: the exchange is over.
        let late = vec![
            thread[0].clone(),
            thread[1].clone(),
            row(
                "q2",
                "user",
                "user:const",
                t0 + 4_000 + FOLLOW_UP_WINDOW_MS + 1,
                json!({}),
            ),
        ];
        assert!(!follow_up_in_active_exchange(&late, "q2", ME));
        // Never answered anyone: nothing to follow up on.
        let cold = vec![row("q2", "user", "user:const", t0, json!({}))];
        assert!(!follow_up_in_active_exchange(&cold, "q2", ME));
    }

    /// 2026-10-24 00:48: "can you send a message to @testagent ?" woke TestAgent ("Arbos, I am
    /// @testagent…") and "@testagent2 I want you to send a message to @testagent" woke both.
    /// A mention after "to" in a send/pass/forward phrase is the object, not the addressee.
    #[test]
    fn mention_after_send_to_is_the_object_not_the_addressee() {
        assert!(mention_is_object(
            "can you send a message to @testagent ?",
            "testagent"
        ));
        assert!(mention_is_object(
            "@testagent2 i want you to send a message to @testagent",
            "testagent"
        ));
        assert!(!mention_is_object(
            "@testagent2 i want you to send a message to @testagent",
            "testagent2"
        ));
        assert!(mention_is_object("pass this on to @doug please", "doug"));
        assert!(!mention_is_object("@doug are you there?", "doug"));
        assert!(!mention_is_object("talk to @doug, he knows", "doug"));
        assert!(!mention_is_object(
            "@testagent ok then ask testagent2 what his favorite sauce is",
            "testagent"
        ));
        // Through the gate: TestAgent is not addressed by the relay request even though the
        // composer resolved its mention; TestAgent2 is.
        let m = msg(
            "@testagent2 I want you to send a message to @testagent",
            json!({ "mentions": ["agent:TA2", "agent:TA"] }),
        );
        assert!(!addressed(&m, "TA", "testagent", false, false, false));
        assert!(addressed(&m, "TA2", "testagent2", false, false, false));
        let m = msg(
            "can you send a message to @testagent ?",
            json!({ "mentions": ["agent:TA"] }),
        );
        assert!(!addressed(&m, "TA", "testagent", false, false, false));
    }

    /// An agent's bubble that @mentions another agent of the chat is dispatched as a `send`
    /// by its box; the named agent's gate accepts it (one hop), nobody else's does.
    #[test]
    fn another_agents_mention_names_me() {
        let info = PeopleChatInfo {
            members: vec![
                json!({ "member": "user:const", "handle": "const" }),
                json!({ "member": "agent:TA", "handle": "testagent" }),
                json!({ "member": "agent:TA2", "handle": "testagent2" }),
            ],
            slug: "testagent".into(),
            kind: "group".into(),
            ..Default::default()
        };
        let mut m = msg(
            "@testagent, Arbos asked me to pass this on: please confirm.",
            json!({ "sender": "agent:TA2", "senderHandle": "testagent2" }),
        );
        m.role = "assistant".into();
        assert!(text_mentions_this_agent(&m, &info, "TA"));
        assert_eq!(
            info.other_agents_mentioned(&m.text(), "agent:TA2"),
            vec!["TA".to_owned()]
        );
        assert!(
            info.other_agents_mentioned("@testagent2 done", "agent:TA2")
                .is_empty(),
            "my own handle is not a target"
        );
        assert!(info
            .other_agents_mentioned("nothing to see", "agent:TA2")
            .is_empty());
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
        assert!(another_agent_addressed(&m, &group, "DOUG"));
        assert!(!another_agent_addressed(&m, &group, "HEZ"));
        // Only Hezbola is owed a reply; the shared `send` closes when Hezbola answers.
        assert_eq!(people_shared_agent_ids(&m, &group), vec!["HEZ".to_string()]);
        // A chip with no @handle in the prose: referenced, the decision model judges.
        let chip_only = msg(
            "can you check with him?",
            json!({ "mentions": ["agent:DOUG"], "sender": "user:gh_1" }),
        );
        assert_eq!(
            my_mention_role(&chip_only, &group, "DOUG", "doug-agent"),
            MentionRole::Referenced
        );
        // Bare leading name counts as addressing the other agent too.
        let bare = msg(
            "Hezbola, can you check with @doug-agent?",
            json!({ "sender": "user:gh_1" }),
        );
        assert!(another_agent_addressed(&bare, &group, "DOUG"));
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
