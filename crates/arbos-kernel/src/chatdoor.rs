//! Chat doors (K-04): a Discord or Slack channel as a way in and out of an
//! agent. `<place>/.arbos/doors.toml` names each door; the kernel polls
//! the channel, every new human message becomes a `user` inbox message
//! for the agent (channel `discord:<id>` / `slack:<id>`), and when that
//! turn ends the agent's last words go back to the channel.
//!
//! ```toml
//! [[door]]
//! kind = "discord"                       # or "slack"
//! token = "env:ARBOS_DISCORD_TOKEN"      # a secrets.toml NAME, env:VAR, file:/p, op://…
//! channels = ["123456789012345678"]
//! agent = "root"                         # default
//! every = "5s"                           # poll period, floor 3s
//! reply = true                           # post the turn's last words back
//! mention_only = false                   # only messages that @-mention the bot
//! ```
//!
//! Polling, not a gateway socket: one token, no intents dance, the same
//! shape for both services, and a kernel that sleeps between looks. The
//! first look only remembers where the channel is; history is not
//! replayed. Messages from bots (this one included) are skipped, so a
//! reply never wakes the agent again. Token values are protected in the
//! secrets store, so they are redacted wherever they might show up.

use anyhow::{Context, Result, bail};
use arbos_core::{EventKind, Layout, Place, load_transcript};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crate::hooks::KernelHooks;

pub const FILE: &str = "doors.toml";
const MIN_EVERY: Duration = Duration::from_secs(3);
const DEFAULT_EVERY: Duration = Duration::from_secs(5);
/// Longest reply posted back; a service refuses more anyway (Discord: 2000).
const REPLY_CAP: usize = 1900;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Door {
    pub kind: String,
    pub token: String,
    #[serde(default)]
    pub channels: Vec<String>,
    #[serde(default = "default_agent")]
    pub agent: String,
    #[serde(default)]
    pub every: Option<String>,
    #[serde(default = "default_true")]
    pub reply: bool,
    #[serde(default)]
    pub mention_only: bool,
    /// The service's API root; tests point it at a local stand-in.
    #[serde(default)]
    pub api_base: Option<String>,
}

fn default_agent() -> String {
    "root".into()
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Default, Deserialize)]
struct File {
    #[serde(default)]
    door: Vec<Door>,
}

impl Door {
    pub fn validate(&self) -> Result<()> {
        if !matches!(self.kind.as_str(), "discord" | "slack") {
            bail!("kind must be discord or slack, not {:?}", self.kind);
        }
        if self.token.trim().is_empty() {
            bail!(
                "{} door needs token (a secrets.toml name, env:VAR, file:/path, or op://…)",
                self.kind
            );
        }
        if self.channels.is_empty() {
            bail!("{} door needs channels: at least one channel id", self.kind);
        }
        if let Some(e) = &self.every
            && arbos_core::subscription::parse_duration_ms(e).is_none()
        {
            bail!("every: {e:?} is not a duration (5s, 1m)");
        }
        Ok(())
    }

    fn every(&self) -> Duration {
        self.every
            .as_deref()
            .and_then(arbos_core::subscription::parse_duration_ms)
            .map(Duration::from_millis)
            .unwrap_or(DEFAULT_EVERY)
            .max(MIN_EVERY)
    }

    fn api(&self) -> String {
        match &self.api_base {
            Some(b) => b.trim_end_matches('/').to_string(),
            None => match self.kind.as_str() {
                "discord" => "https://discord.com/api/v10".into(),
                _ => "https://slack.com/api".into(),
            },
        }
    }

    /// The `channel` a message from `ch` carries on the transcript.
    pub fn channel_tag(&self, ch: &str) -> String {
        format!("{}:{ch}", self.kind)
    }
}

/// Every door in the file. An absent file is no doors; a file that does
/// not parse is an error for `check` and a log line for the kernel.
pub fn load(place: &Place) -> Result<Vec<Door>> {
    let path = place.arbos().join(FILE);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(Vec::new());
    };
    let file: File = toml::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    for d in &file.door {
        d.validate()?;
    }
    Ok(file.door)
}

/// The token's value: a `secrets.toml` name first, then a source spelled
/// out. Protected in the store either way.
fn token_value(place: &Place, door: &Door) -> Result<String> {
    let source = match arbos_engine::secrets::Config::load(place.path()) {
        Ok(cfg) => cfg
            .secrets
            .get(door.token.trim())
            .cloned()
            .unwrap_or_else(|| door.token.trim().to_string()),
        Err(_) => door.token.trim().to_string(),
    };
    let value = arbos_engine::secrets::resolve(&source)?;
    arbos_engine::secrets::store().protect(
        &format!("{}_DOOR_TOKEN", door.kind.to_ascii_uppercase()),
        value.clone(),
    );
    Ok(value)
}

/// One message read from a channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incoming {
    pub id: String,
    pub author: String,
    pub text: String,
    pub from_bot: bool,
    pub mentions_me: bool,
    /// Slack: the `thread_ts` of a reply. Discord threads are channels of
    /// their own, so this stays empty there.
    pub thread: Option<String>,
}

/// What the kernel knows of a door once it runs: enough to post back.
#[derive(Clone)]
struct Live {
    door: Door,
    token: String,
    client: reqwest::Client,
}

fn registry() -> &'static Mutex<HashMap<String, Live>> {
    static R: OnceLock<Mutex<HashMap<String, Live>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Start a poller per door named in `doors.toml`. Called once at kernel start.
pub fn spawn_if_configured(hooks: Arc<KernelHooks>) {
    let doors = match load(&hooks.place) {
        Ok(d) => d,
        Err(e) => {
            crate::klog::warn("doors_config", None, format!("{e:#}"));
            return;
        }
    };
    for door in doors {
        let token = match token_value(&hooks.place, &door) {
            Ok(t) => t,
            Err(e) => {
                crate::klog::warn(
                    "door_token",
                    None,
                    format!("{} door: {e:#}; door not opened", door.kind),
                );
                continue;
            }
        };
        let live = Live {
            door: door.clone(),
            token,
            client: reqwest::Client::new(),
        };
        for ch in &door.channels {
            registry()
                .lock()
                .unwrap()
                .insert(door.channel_tag(ch), live.clone());
        }
        crate::klog::info(
            "door_open",
            Some(&door.agent),
            format!("{} channels {}", door.kind, door.channels.join(",")),
        );
        tokio::spawn(poll_loop(live, Arc::clone(&hooks)));
    }
}

async fn poll_loop(live: Live, hooks: Arc<KernelHooks>) {
    // channel id → last message id/ts seen. None until the first look,
    // which only sets it: history is not replayed.
    let mut cursors: HashMap<String, Option<String>> = HashMap::new();
    let mut me: Option<String> = None;
    let every = live.door.every();
    loop {
        if me.is_none() {
            me = whoami(&live).await;
        }
        for ch in &live.door.channels {
            let before = cursors.get(ch).cloned().flatten();
            match fetch(&live, ch, before.as_deref(), me.as_deref()).await {
                Ok(mut msgs) => {
                    msgs.sort_by_key(|m| order_key(&m.id));
                    let newest = msgs.last().map(|m| m.id.clone());
                    cursors.insert(
                        ch.clone(),
                        Some(
                            newest
                                .clone()
                                .or(before.clone())
                                .unwrap_or_else(|| start_cursor(&live.door.kind)),
                        ),
                    );
                    if before.is_none() {
                        continue;
                    }
                    for m in msgs {
                        if m.from_bot {
                            continue;
                        }
                        let text = m.text.trim();
                        if text.is_empty() {
                            continue;
                        }
                        // Agents that subscribed to this channel (`subscribe
                        // kind=chat`) hear every human message in it, whatever
                        // the door's own agent and mention rule.
                        crate::subs::fire_chat(
                            &hooks,
                            &live.door.channel_tag(ch),
                            m.thread.as_deref(),
                            &m.author,
                            text,
                        );
                        if live.door.mention_only && !m.mentions_me {
                            continue;
                        }
                        if let Err(e) = hooks.inbox_user(
                            &live.door.agent,
                            text,
                            Vec::new(),
                            &live.door.channel_tag(ch),
                            &m.author,
                        ) {
                            crate::klog::warn(
                                "door_inbox",
                                Some(&live.door.agent),
                                format!("{e:#}"),
                            );
                        }
                    }
                }
                Err(e) => {
                    crate::klog::warn(
                        "door_poll",
                        Some(&live.door.agent),
                        format!("{} {ch}: {e:#}", live.door.kind),
                    );
                }
            }
        }
        tokio::time::sleep(every).await;
    }
}

fn start_cursor(kind: &str) -> String {
    match kind {
        // Discord snowflake for "now": (ms since 2015-01-01) << 22.
        "discord" => {
            let ms = arbos_core::now_ms() as u64 - 1_420_070_400_000;
            ((ms) << 22).to_string()
        }
        _ => format!("{}.000000", arbos_core::now_ms() / 1000),
    }
}

/// Snowflakes and Slack `ts` both sort as numbers — exact ones: two
/// snowflakes a step apart are the same f64.
fn order_key(id: &str) -> (u128, u128) {
    let (whole, frac) = id.split_once('.').unwrap_or((id, ""));
    (
        whole.parse::<u128>().unwrap_or(0),
        frac.parse::<u128>().unwrap_or(0),
    )
}

async fn whoami(live: &Live) -> Option<String> {
    let v = match live.door.kind.as_str() {
        "discord" => live
            .client
            .get(format!("{}/users/@me", live.door.api()))
            .header("Authorization", format!("Bot {}", live.token))
            .send()
            .await
            .ok()?
            .json::<Value>()
            .await
            .ok()?,
        _ => live
            .client
            .post(format!("{}/auth.test", live.door.api()))
            .bearer_auth(&live.token)
            .send()
            .await
            .ok()?
            .json::<Value>()
            .await
            .ok()?,
    };
    v.get("id")
        .or_else(|| v.get("user_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

async fn fetch(
    live: &Live,
    ch: &str,
    after: Option<&str>,
    me: Option<&str>,
) -> Result<Vec<Incoming>> {
    match live.door.kind.as_str() {
        "discord" => {
            let mut url = format!("{}/channels/{ch}/messages?limit=50", live.door.api());
            if let Some(a) = after {
                url.push_str(&format!("&after={a}"));
            }
            let resp = live
                .client
                .get(&url)
                .header("Authorization", format!("Bot {}", live.token))
                .send()
                .await
                .context("discord: request")?;
            if !resp.status().is_success() {
                bail!("discord: HTTP {}", resp.status());
            }
            let v: Value = resp.json().await.context("discord: body")?;
            Ok(parse_discord(&v, me))
        }
        _ => {
            let mut url = format!(
                "{}/conversations.history?channel={ch}&limit=50",
                live.door.api()
            );
            if let Some(a) = after {
                url.push_str(&format!("&oldest={a}"));
            }
            let resp = live
                .client
                .get(&url)
                .bearer_auth(&live.token)
                .send()
                .await
                .context("slack: request")?;
            let v: Value = resp.json().await.context("slack: body")?;
            if v.get("ok").and_then(Value::as_bool) == Some(false) {
                bail!(
                    "slack: {}",
                    v.get("error").and_then(Value::as_str).unwrap_or("error")
                );
            }
            Ok(parse_slack(&v, me))
        }
    }
}

/// Discord `GET /channels/{id}/messages`: newest first; `author.bot`
/// marks bots; mentions list users.
pub fn parse_discord(v: &Value, me: Option<&str>) -> Vec<Incoming> {
    v.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let id = m.get("id")?.as_str()?.to_string();
                    let author = m.get("author").unwrap_or(&Value::Null);
                    let name = author
                        .get("global_name")
                        .and_then(Value::as_str)
                        .or_else(|| author.get("username").and_then(Value::as_str))
                        .unwrap_or("someone")
                        .to_string();
                    let from_bot = author.get("bot").and_then(Value::as_bool).unwrap_or(false)
                        || me
                            .is_some_and(|me| author.get("id").and_then(Value::as_str) == Some(me));
                    let mentions_me = me.is_some_and(|me| {
                        m.get("mentions")
                            .and_then(Value::as_array)
                            .is_some_and(|ms| {
                                ms.iter()
                                    .any(|u| u.get("id").and_then(Value::as_str) == Some(me))
                            })
                    });
                    Some(Incoming {
                        id,
                        author: name,
                        text: m
                            .get("content")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        from_bot,
                        mentions_me,
                        thread: None,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Slack `conversations.history`: `messages[]` with `ts`, `user`, `text`;
/// `bot_id` or a `subtype` marks what is not a person's message.
pub fn parse_slack(v: &Value, me: Option<&str>) -> Vec<Incoming> {
    v.get("messages")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let id = m.get("ts")?.as_str()?.to_string();
                    let user = m
                        .get("user")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let from_bot = m.get("bot_id").is_some()
                        || m.get("subtype").is_some()
                        || me.is_some_and(|me| user == me);
                    let text = m
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let mentions_me = me.is_some_and(|me| text.contains(&format!("<@{me}>")));
                    let thread = m
                        .get("thread_ts")
                        .and_then(Value::as_str)
                        .filter(|t| *t != id)
                        .map(str::to_string);
                    Some(Incoming {
                        id,
                        author: if user.is_empty() {
                            "someone".into()
                        } else {
                            user
                        },
                        text,
                        from_bot,
                        mentions_me,
                        thread,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A turn of `agent` ended: if the message that opened it came through a
/// door with `reply` on, post the turn's last words back to that channel.
/// Called from the turn-end path; the post runs on its own task.
pub fn reply_if_door_turn(hooks: &KernelHooks, agent: &str) {
    if registry().lock().unwrap().is_empty() {
        return;
    }
    let events =
        load_transcript(&Layout::new(&hooks.place, agent).transcript()).unwrap_or_default();
    let Some(start) = events.iter().rposition(arbos_core::Event::is_wake) else {
        return;
    };
    let channel = events[start..].iter().find_map(|e| match &e.kind {
        EventKind::User { channel, .. } if channel.contains(':') => Some(channel.clone()),
        _ => None,
    });
    let Some(channel) = channel else {
        return;
    };
    let live = registry().lock().unwrap().get(&channel).cloned();
    let Some(live) = live else {
        return;
    };
    if !live.door.reply {
        return;
    }
    let lo = events[start].seq;
    let (words, _ok) = crate::plan::turn_outcome(&events, lo);
    let words = arbos_engine::secrets::store().redact(words.trim());
    if words.is_empty() {
        return;
    }
    let text: String = if words.chars().count() > REPLY_CAP {
        let mut t: String = words.chars().take(REPLY_CAP).collect();
        t.push('…');
        t
    } else {
        words
    };
    let ch = channel
        .split_once(':')
        .map(|(_, c)| c.to_string())
        .unwrap_or_default();
    let agent = agent.to_string();
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(async move {
            if let Err(e) = post(&live, &ch, &text).await {
                crate::klog::warn(
                    "door_reply",
                    Some(&agent),
                    format!("{} {ch}: {e:#}", live.door.kind),
                );
            } else {
                crate::klog::info(
                    "door_reply",
                    Some(&agent),
                    format!("{} {ch}: {} chars", live.door.kind, text.len()),
                );
            }
        });
    }
}

async fn post(live: &Live, ch: &str, text: &str) -> Result<()> {
    match live.door.kind.as_str() {
        "discord" => {
            let resp = live
                .client
                .post(format!("{}/channels/{ch}/messages", live.door.api()))
                .header("Authorization", format!("Bot {}", live.token))
                .json(&serde_json::json!({ "content": text }))
                .send()
                .await
                .context("discord: post")?;
            if !resp.status().is_success() {
                bail!("discord: HTTP {}", resp.status());
            }
            Ok(())
        }
        _ => {
            let resp = live
                .client
                .post(format!("{}/chat.postMessage", live.door.api()))
                .bearer_auth(&live.token)
                .json(&serde_json::json!({ "channel": ch, "text": text }))
                .send()
                .await
                .context("slack: post")?;
            let v: Value = resp.json().await.context("slack: body")?;
            if v.get("ok").and_then(Value::as_bool) == Some(false) {
                bail!(
                    "slack: {}",
                    v.get("error").and_then(Value::as_str).unwrap_or("error")
                );
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_parses_and_refuses_what_the_poller_could_not_run() {
        let dir = std::env::temp_dir().join(format!(
            "arbos-doors-{}-{}",
            std::process::id(),
            arbos_core::now_ms()
        ));
        std::fs::create_dir_all(dir.join(".arbos")).unwrap();
        let place = Place::new(&dir);
        assert!(load(&place).unwrap().is_empty());
        std::fs::write(
            dir.join(".arbos/doors.toml"),
            "[[door]]\nkind = \"discord\"\ntoken = \"env:X\"\nchannels = [\"1\"]\n\n[[door]]\nkind = \"slack\"\ntoken = \"SLACK\"\nchannels = [\"C1\"]\nagent = \"ops\"\nevery = \"1s\"\nreply = false\nmention_only = true\n",
        )
        .unwrap();
        let doors = load(&place).unwrap();
        assert_eq!(doors.len(), 2);
        assert_eq!(doors[0].agent, "root");
        assert!(doors[0].reply);
        assert_eq!(doors[1].agent, "ops");
        assert!(!doors[1].reply && doors[1].mention_only);
        // The floor: 1s asks for 3s.
        assert_eq!(doors[1].every(), MIN_EVERY);
        assert_eq!(doors[0].channel_tag("1"), "discord:1");
        for bad in [
            "[[door]]\nkind = \"irc\"\ntoken = \"x\"\nchannels = [\"1\"]\n",
            "[[door]]\nkind = \"discord\"\ntoken = \"\"\nchannels = [\"1\"]\n",
            "[[door]]\nkind = \"discord\"\ntoken = \"x\"\n",
            "[[door]]\nkind = \"discord\"\ntoken = \"x\"\nchannels = [\"1\"]\nevery = \"soon\"\n",
        ] {
            std::fs::write(dir.join(".arbos/doors.toml"), bad).unwrap();
            assert!(load(&place).is_err(), "{bad}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn discord_and_slack_messages_read_the_same_way() {
        let d = serde_json::json!([
            {"id": "3", "content": "hi <@bot1>", "author": {"id": "u1", "username": "jacob", "bot": false}, "mentions": [{"id": "bot1"}]},
            {"id": "2", "content": "beep", "author": {"id": "bot1", "username": "Arbos", "bot": true}},
            {"id": "1", "content": "from me", "author": {"id": "bot1", "username": "Arbos"}}
        ]);
        let msgs = parse_discord(&d, Some("bot1"));
        assert_eq!(msgs.len(), 3);
        assert_eq!(
            (
                msgs[0].author.as_str(),
                msgs[0].from_bot,
                msgs[0].mentions_me
            ),
            ("jacob", false, true)
        );
        assert!(
            msgs[1].from_bot && msgs[2].from_bot,
            "the bot's own lines never wake it"
        );
        let s = serde_json::json!({"ok": true, "messages": [
            {"ts": "1700000000.000200", "user": "U1", "text": "hello <@B1>"},
            {"ts": "1700000000.000100", "bot_id": "B1", "user": "B1", "text": "reply"},
            {"ts": "1700000000.000050", "user": "U2", "subtype": "channel_join", "text": "joined"}
        ]});
        let msgs = parse_slack(&s, Some("B1"));
        assert_eq!(msgs.len(), 3);
        assert_eq!(
            (
                msgs[0].author.as_str(),
                msgs[0].from_bot,
                msgs[0].mentions_me
            ),
            ("U1", false, true)
        );
        assert!(msgs[1].from_bot && msgs[2].from_bot);
        assert!(order_key("1700000000.000200") > order_key("1700000000.000100"));
        assert!(order_key("9000000000000000002") > order_key("9000000000000000001"));
    }
}
