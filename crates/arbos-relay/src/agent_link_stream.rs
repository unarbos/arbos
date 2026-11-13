//! The live stream an agent access link watches (PR 4 of 5; spec §7).
//!
//! `GET /agent-link/stream` is one Server-Sent Events response that stays open. The agent is
//! told about a new message as it lands instead of asking every few seconds, and it reconnects
//! from a cursor rather than re-reading the account.
//!
//! **Still one link.** The stream takes the same `arbl_…` bearer in the same
//! `Authorization` header as every route in [`crate::agent_link_read`]. That is the whole
//! reason this is SSE and not a WebSocket: a browser's `WebSocket` cannot set a header, so a
//! WebSocket stream would have needed the token in the query string — a second place to carry
//! the credential, into every proxy log, for the one feature whose rule is that the owner hands
//! over exactly one thing. `EventSource` has the same limitation, but an agent is not a browser:
//! it fetches, and `fetch` sets headers.
//!
//! **Two classes of event.** Durable events are rows in `link_events`, carry an `id:`, and are
//! replayed from a cursor on reconnect. Ephemeral events (typing, status) carry no `id` and are
//! never stored — writing a keystroke indicator to a table would be pure waste. This PR produces
//! the durable `message.received`; the rest of the spec's events wait for their own hooks
//! (`§7.4`), because reactions and edits live in the chat CRDT rather than behind an HTTP funnel
//! and hooking them is a different piece of work.
//!
//! **Free for every other account.** The announce path is the hottest path in the relay. It
//! reaches [`on_announce`] below, which returns after one read lock and one `is_empty` when no
//! account on this relay holds a live link — the normal case, forever. Only when the set is
//! non-empty does it look at who is in the chat.
//!
//! **One chokepoint, still.** Every durable row is put to [`crate::agent_link_read::may_see`]
//! at **send** time, not at append time. A chat the link lost access to between the message
//! landing and the stream waking is not sent, and neither is a replayed row from before a
//! restriction was tightened.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::stream::Stream;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::broadcast;

use arbos_core::ids::now_ms;

use crate::agent_link::{client_ip, not_found, user_agent, AgentLink, LinkPrincipal};
use crate::agent_link_read::{line_json, may_see, note_read, read_budget, ChatView};
use crate::db::Db;
use crate::matrix::backfill::load_doc;
use crate::notify::{ChatRef, Sender};
use crate::state::AppState;

/// Durable rows one account's backlog keeps, and how long any row lives. A backlog exists to
/// cover a reconnect, not to be an archive — the history routes are the archive. Both bounds are
/// applied on insert, and every row of an account is deleted when its last link dies, so the
/// table cannot grow without a live link to justify it.
const BACKLOG_ROWS: i64 = 1_000;
const BACKLOG_MS: i64 = 24 * 3_600_000;

/// Streams one link may hold open at once, and opens it may make in an hour. The concurrency cap
/// is in memory rather than in a table (spec §12, note on `link_streams`): it resets to zero on
/// restart, which is the safe direction — a crash that lost the count would otherwise lock the
/// agent out of its own stream with no way for the owner to clear it.
const STREAMS_PER_LINK: u32 = 2;
const OPENS_PER_HOUR: i64 = 30;
const HOUR_MS: i64 = 3_600_000;

/// How often an idle stream sends a keepalive comment, and how often the loop re-reads the link
/// row when nothing has happened. The second number is the worst-case delay between the owner
/// revoking a link and the stream closing.
const PING_EVERY: Duration = Duration::from_secs(20);
const RECHECK_EVERY: Duration = Duration::from_secs(30);

/// Durable rows one wake hands over before going back to sleep.
const BATCH: i64 = 200;

// ---- the hub ------------------------------------------------------------------------------------

/// Who is watching, and how to wake them.
///
/// One of these lives in [`AppState`]. It holds three things and no history: the set of accounts
/// with a live link (so the announce path can leave almost immediately), one broadcast channel
/// per watched account (so a new row wakes the open streams), and the open-stream count per link
/// (the two-at-once cap).
#[derive(Default)]
pub struct LinkHub {
    /// Accounts with at least one live link. The guard the announce path reads.
    users: RwLock<HashSet<String>>,
    /// user_id → wake-up for that account's open streams, created when the first stream
    /// connects and dropped when the last one leaves.
    ///
    /// The channel carries `()` and not the event itself. A woken stream reads `link_events`
    /// from its own cursor, so a reader that was mid-send while three rows landed still sends
    /// all three in order, and a reader slow enough to make `broadcast` drop a wake
    /// (`RecvError::Lagged`) loses nothing at all — the cursor, not the channel, is the record
    /// of what it has seen. This is ASP's `waiter(&pid)` shape (`outside.rs`), which is already
    /// proven in this relay.
    chans: Mutex<HashMap<String, broadcast::Sender<()>>>,
    /// link_id → streams open right now.
    open: Mutex<HashMap<String, u32>>,
}

impl LinkHub {
    /// The set as the database has it, read once at boot. Without this a relay restarted while a
    /// link was live would write no events until the owner next minted or revoked one.
    pub fn load(db: &Db) -> Self {
        let hub = Self::default();
        if let Ok(users) = db.users_with_live_links() {
            if let Ok(mut set) = hub.users.write() {
                set.extend(users);
            }
        }
        hub
    }

    /// Does any account on this relay hold a live link? The first question the announce path
    /// asks, and the one that makes this feature free for everybody else.
    fn idle(&self) -> bool {
        self.users.read().map(|s| s.is_empty()).unwrap_or(true)
    }

    fn watches(&self, user_id: &str) -> bool {
        self.users
            .read()
            .map(|s| s.contains(user_id))
            .unwrap_or(false)
    }

    /// This account has a link now. Called on mint, which is before the link is opened on
    /// purpose: events have to be in the backlog by the time the agent first connects, or the
    /// first thing it watches is a conversation that already moved on.
    pub fn note_live(&self, user_id: &str) {
        if let Ok(mut set) = self.users.write() {
            set.insert(user_id.to_owned());
        }
    }

    /// The account's last link is gone, so stop writing events for it and throw away the ones
    /// already written. Called on revoke and on sign-out-everywhere; a no-op while any link of
    /// the account is still live.
    pub fn forget_if_dead(&self, db: &Db, user_id: &str) {
        if db.live_agent_link_count(user_id).unwrap_or(1) > 0 {
            return;
        }
        if let Ok(mut set) = self.users.write() {
            set.remove(user_id);
        }
        // Wake the open streams before the channel goes: each re-reads its link row, finds it
        // revoked, and sends `bye`. Without the wake they would wait out the 30-second timer.
        self.wake(user_id);
        if let Ok(mut chans) = self.chans.lock() {
            chans.remove(user_id);
        }
        let _ = db.delete_link_events_of_user(user_id);
    }

    fn subscribe(&self, user_id: &str) -> broadcast::Receiver<()> {
        let mut chans = match self.chans.lock() {
            Ok(c) => c,
            // A poisoned lock here would mean a panic while holding it. Handing back a detached
            // receiver keeps the stream working on the 30-second re-check rather than failing
            // the request.
            Err(_) => return broadcast::channel(16).1,
        };
        match chans.get(user_id) {
            Some(tx) => tx.subscribe(),
            None => {
                let (tx, rx) = broadcast::channel(16);
                chans.insert(user_id.to_owned(), tx);
                rx
            }
        }
    }

    fn wake(&self, user_id: &str) {
        if let Ok(chans) = self.chans.lock() {
            if let Some(tx) = chans.get(user_id) {
                let _ = tx.send(());
            }
        }
    }

    /// Take one of this link's stream slots, or `None` when both are already held.
    fn take_slot(self: &Arc<Self>, link_id: &str) -> Option<u32> {
        let mut open = self.open.lock().ok()?;
        let n = open.entry(link_id.to_owned()).or_insert(0);
        if *n >= STREAMS_PER_LINK {
            return None;
        }
        *n += 1;
        Some(*n)
    }

    fn release_slot(&self, link_id: &str) {
        if let Ok(mut open) = self.open.lock() {
            if let Some(n) = open.get_mut(link_id) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    open.remove(link_id);
                }
            }
        }
    }
}

// ---- storage ------------------------------------------------------------------------------------

impl Db {
    /// Accounts with at least one live link, for [`LinkHub::load`].
    pub fn users_with_live_links(&self) -> anyhow::Result<Vec<String>> {
        let now = now_ms();
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT DISTINCT user_id FROM agent_links
                 WHERE revoked_at IS NULL AND expires_at > ?1",
            )?;
            let rows = st.query_map(params![now], |r| r.get(0))?;
            rows.collect()
        })
    }

    /// Append one durable stream event and trim the account's backlog in the same call, so the
    /// bound cannot be left to a sweep that might not run.
    pub fn append_link_event(
        &self,
        user_id: &str,
        chat_id: &str,
        payload: &Value,
    ) -> anyhow::Result<i64> {
        let now = now_ms();
        self.with(|c| {
            c.execute(
                "INSERT INTO link_events(user_id, chat_id, created_at, payload)
                 VALUES (?1,?2,?3,?4)",
                params![user_id, chat_id, now, payload.to_string()],
            )?;
            let id = c.last_insert_rowid();
            c.execute(
                "DELETE FROM link_events WHERE user_id = ?1 AND created_at < ?2",
                params![user_id, now - BACKLOG_MS],
            )?;
            c.execute(
                "DELETE FROM link_events WHERE user_id = ?1 AND id <= (
                     SELECT id FROM link_events WHERE user_id = ?1 ORDER BY id DESC
                     LIMIT 1 OFFSET ?2
                 )",
                params![user_id, BACKLOG_ROWS],
            )?;
            Ok(id)
        })
    }

    /// The account's durable events after `after`, oldest first.
    pub fn link_events_after(
        &self,
        user_id: &str,
        after: i64,
        limit: i64,
    ) -> anyhow::Result<Vec<(i64, String, Value)>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT id, chat_id, payload FROM link_events
                 WHERE user_id = ?1 AND id > ?2 ORDER BY id LIMIT ?3",
            )?;
            let rows = st.query_map(params![user_id, after, limit], |r| {
                let payload: String = r.get(2)?;
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    serde_json::from_str::<Value>(&payload).unwrap_or(Value::Null),
                ))
            })?;
            rows.collect()
        })
    }

    /// The oldest event id still held for an account, or 0 when the backlog is empty. A cursor
    /// below this has lost the events in between to [`BACKLOG_ROWS`] or [`BACKLOG_MS`], and no
    /// reader can work that out from its own cursor.
    pub fn link_events_oldest(&self, user_id: &str) -> anyhow::Result<i64> {
        self.with(|c| {
            c.query_row(
                "SELECT COALESCE(MIN(id), 0) FROM link_events WHERE user_id = ?1",
                params![user_id],
                |r| r.get(0),
            )
        })
    }

    /// The newest event id of an account, or 0. A stream opened without a cursor starts here:
    /// "from now on" is what a fresh connection means, and replaying a day of backlog to an
    /// agent that never asked for it would spend the owner's read budget on history it has.
    pub fn link_events_head(&self, user_id: &str) -> anyhow::Result<i64> {
        self.with(|c| {
            c.query_row(
                "SELECT COALESCE(MAX(id), 0) FROM link_events WHERE user_id = ?1",
                params![user_id],
                |r| r.get(0),
            )
        })
    }

    pub fn delete_link_events_of_user(&self, user_id: &str) -> anyhow::Result<()> {
        self.with(|c| {
            c.execute(
                "DELETE FROM link_events WHERE user_id = ?1",
                params![user_id],
            )
            .map(|_| ())
        })
    }
}

// ---- where the events come from ------------------------------------------------------------------

/// One `message.received` for every account in this chat that holds a live link.
///
/// Called from [`crate::notify::announce_with`], the single funnel every new message of every
/// kind goes through: a person's message off the room socket, an agent's finished reply, and the
/// reply a reply-scope link itself posts in PR 5. One hook, so there is no path that can deliver
/// a message to the apps and not to the stream.
///
/// What is stored is deliberately thin — the ids, and the announce's own text as a fallback. The
/// message is read out of the chat doc when the stream sends it ([`event_json`]), which is both
/// cheaper here (no document load on the announce path) and more correct there: a replayed row
/// carries the message as it is now, edits included, rather than a snapshot of a preview. The
/// announce body is only the sending client's push preview — iPhone cuts it to 140 characters —
/// so it is never what the agent is shown unless the row cannot be found at all.
pub fn on_announce(
    state: &AppState,
    chat: &ChatRef,
    sender: &Sender,
    message_id: &str,
    text: &str,
    reply_to: Option<&str>,
    mentions: &[String],
) {
    if state.link_hub.idle() {
        return;
    }
    let watchers: Vec<String> = crate::notify::recipients(state, chat)
        .into_iter()
        .filter(|u| state.link_hub.watches(u))
        .collect();
    if watchers.is_empty() {
        return;
    }
    let payload = json!({
        "type": "message.received",
        "message_id": message_id,
        "preview": text,
        "reply_to": reply_to,
        "mentions": mentions,
        "from": {
            "kind": sender.kind,
            "id": sender.id,
            "name": sender.name,
            "handle": sender.handle,
        },
        "at": now_ms(),
    });
    for user in watchers {
        match state.db.append_link_event(&user, &chat.chat_id, &payload) {
            Ok(_) => state.link_hub.wake(&user),
            Err(e) => tracing::warn!(
                error = %e, user = %user, chat = %chat.chat_id,
                "agent link: stream event not stored"
            ),
        }
    }
}

// ---- the stream ---------------------------------------------------------------------------------

#[derive(Deserialize, Default)]
pub struct StreamQuery {
    /// Start after this event id. `Last-Event-ID` wins over it, because that header is what the
    /// agent's own reconnect sends and the query string is what it typed by hand.
    #[serde(default)]
    pub since: Option<i64>,
    /// Narrow the stream to one conversation — the shape the owner's own example wants: watch
    /// `codingarbos` and nothing else.
    #[serde(default)]
    pub chat: Option<String>,
}

/// `GET /agent-link/stream?since=&chat=` — the one open connection.
pub async fn stream(
    State(state): State<AppState>,
    who: LinkPrincipal,
    Query(q): Query<StreamQuery>,
    headers: HeaderMap,
) -> Response {
    let link = who.link;
    // Opening a stream is cheap for the agent and not free for the relay, so it is counted
    // separately from the per-minute request budget: a reconnect loop with a bug in it should
    // run out of opens long before it runs out of requests.
    if !state
        .db
        .rate_check_and_note(
            &format!("link:{}", link.id),
            "linkstream",
            HOUR_MS,
            OPENS_PER_HOUR,
        )
        .unwrap_or(true)
    {
        return stream_limited(
            "this link has opened its hourly allowance of streams; reconnect less often",
        );
    }
    // A named chat is resolved before anything is opened, through the same chokepoint every
    // read route uses. A chat the link may not see is a 404, not a 403: a link pinned to one
    // conversation must not be able to test for the existence of the rest of the account.
    if let Some(id) = q.chat.as_deref() {
        if may_see(&state, &link, id).is_none() {
            return not_found();
        }
    }
    let Some(open) = Open::take(&state, &link, &headers) else {
        return stream_limited(
            "this link already has two streams open; close one or watch a narrower chat",
        );
    };
    // `Last-Event-ID` is what an `EventSource` or a well-behaved agent sends on reconnect, and it
    // is the agent's own record of what it has seen. `?since=` is a convenience for a first
    // connection, so the header wins.
    let cursor = match last_event_id(&headers).or(q.since) {
        Some(c) => c.max(0),
        None => state.db.link_events_head(&link.user_id).unwrap_or(0),
    };
    let rx = state.link_hub.subscribe(&link.user_id);
    let hello = Event::default().event("hello").data(
        json!({
            "protocol": "link/1",
            "user": { "id": link.user_id },
            "scope": link.scope.as_str(),
            "chat": q.chat,
            "since": cursor,
            "expires_at": link.expires_at,
            "keepalive_s": PING_EVERY.as_secs(),
        })
        .to_string(),
    );
    let mut pending = VecDeque::new();
    pending.push_back(hello);
    let watch = Watch {
        state,
        link,
        chat: q.chat,
        cursor,
        rx,
        pending,
        headers,
        told_limited: false,
        gap_told: false,
        done: false,
        _open: open,
    };
    Sse::new(frames(watch))
        .keep_alive(KeepAlive::new().interval(PING_EVERY).text("ping"))
        .into_response()
}

/// One open stream, and everything it needs to decide what to send next.
struct Watch {
    state: AppState,
    link: AgentLink,
    chat: Option<String>,
    cursor: i64,
    rx: broadcast::Receiver<()>,
    /// Frames built but not yet handed to the client. One wake can produce a batch, and an SSE
    /// stream yields one frame at a time.
    pending: VecDeque<Event>,
    headers: HeaderMap,
    /// Has the agent already been told it is out of message budget? Said once per dry spell,
    /// not once per wake.
    told_limited: bool,
    /// Has the agent already been told its cursor fell off the backlog? Said once per gap:
    /// cleared when rows are delivered, because the cursor can fall behind again during a dry
    /// spell that holds it.
    gap_told: bool,
    done: bool,
    /// Holds the stream slot and writes the closing audit row. Dropped when the stream ends —
    /// including when the agent simply disconnects, which is how most streams end and which no
    /// line of this loop ever reaches.
    _open: Open,
}

fn frames(watch: Watch) -> impl Stream<Item = Result<Event, std::convert::Infallible>> {
    futures::stream::unfold(Some(watch), |held| async move {
        let mut w = held?;
        loop {
            if let Some(ev) = w.pending.pop_front() {
                return Some((Ok(ev), Some(w)));
            }
            if w.done {
                return None;
            }
            // The link row, every single wake. There is no token cache here: a link revoked a
            // moment ago closes on the next thing that happens, or on the re-check timer.
            match w.state.db.agent_link(&w.link.id) {
                Ok(Some(fresh)) if fresh.live(now_ms()) => w.link = fresh,
                Ok(Some(dead)) => {
                    let reason = if dead.revoked_at.is_some() {
                        "revoked"
                    } else {
                        "expired"
                    };
                    w.pending.push_back(bye(reason));
                    w.done = true;
                    continue;
                }
                // The row is gone, or storage failed. Either way this stream has no authority to
                // keep reading the account.
                Ok(None) => {
                    w.pending.push_back(bye("revoked"));
                    w.done = true;
                    continue;
                }
                Err(e) => {
                    tracing::warn!(error = %e, link = %w.link.id, "agent link: stream re-check failed");
                    w.pending.push_back(bye("error"));
                    w.done = true;
                    continue;
                }
            }
            drain(&mut w);
            if !w.pending.is_empty() {
                continue;
            }
            // Nothing to send. Sleep until a row lands or the re-check falls due, whichever
            // comes first. A dropped wake (`Lagged`) is harmless: the cursor is the record of
            // what this stream has seen, so the next pass reads the same rows anyway.
            tokio::select! {
                _ = w.rx.recv() => {}
                _ = tokio::time::sleep(RECHECK_EVERY) => {}
            }
        }
    })
}

/// Has this stream's cursor fallen off the backlog, and if so, say it once.
///
/// The backlog is bounded by [`BACKLOG_ROWS`] and [`BACKLOG_MS`] and trimmed **per account** on
/// every insert, so events between a cursor and the oldest surviving row are gone. A reader
/// cannot detect that: `id > cursor` returns the surviving rows and looks exactly like a stream
/// with nothing missed. Without this, "send `Last-Event-ID` and you get everything after it" is
/// a promise the reader has no way to check.
///
/// It is checked on every pass rather than only at open, because the cursor can fall behind
/// while the stream is connected: the read budget holds the cursor for up to an hour, and the
/// cap is per account, so another link's traffic can prune past it in the meantime.
///
/// The event carries no `id:`, so it is not a cursor and is never replayed.
fn gap_check(w: &mut Watch) {
    if w.gap_told || w.cursor <= 0 {
        return;
    }
    let oldest = match w.state.db.link_events_oldest(&w.link.user_id) {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!(error = %e, link = %w.link.id, "agent link: backlog floor unreadable");
            return;
        }
    };
    // An empty backlog after a non-zero cursor means the account's rows are all gone, so loss
    // cannot be ruled out either. `oldest == cursor + 1` is the clean case: nothing was missed.
    let lost = if oldest == 0 {
        w.state.db.link_events_head(&w.link.user_id).unwrap_or(0) < w.cursor
    } else {
        oldest > w.cursor + 1
    };
    if !lost {
        return;
    }
    w.gap_told = true;
    tracing::info!(
        link = %w.link.id, cursor = w.cursor, oldest,
        "agent link: stream resumed past a pruned backlog"
    );
    w.pending.push_back(
        Event::default().event("gap").data(
            json!({
                "from": w.cursor,
                "oldest_available": oldest,
                "message": "events between your cursor and the backlog are gone; re-read the chats you care about from their history",
            })
            .to_string(),
        ),
    );
}

/// Turn every durable row this stream may send into a frame, and advance the cursor past the
/// ones it may not.
fn drain(w: &mut Watch) {
    gap_check(w);
    let rows = match w
        .state
        .db
        .link_events_after(&w.link.user_id, w.cursor, BATCH)
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, link = %w.link.id, "agent link: stream backlog unreadable");
            return;
        }
    };
    if rows.is_empty() {
        return;
    }
    // The chokepoint, asked now rather than when the row was written: a chat the link lost
    // between the message landing and this wake is not sent, and neither is a replayed row from
    // before the owner narrowed the link. Deciding per chat and not per row because one wake is
    // usually one conversation.
    let mut seen: HashMap<String, Option<ChatView>> = HashMap::new();
    let mut sendable: Vec<(i64, ChatView, Value)> = Vec::new();
    let mut highest = w.cursor;
    for (id, chat_id, payload) in rows {
        highest = highest.max(id);
        if w.chat.as_deref().is_some_and(|only| only != chat_id) {
            continue;
        }
        let view = seen
            .entry(chat_id.clone())
            .or_insert_with(|| may_see(&w.state, &w.link, &chat_id));
        if let Some(view) = view {
            sendable.push((id, view.clone(), payload));
        }
    }
    if sendable.is_empty() {
        w.cursor = highest;
        return;
    }
    // Messages handed over on the stream come out of the same hourly budget as messages handed
    // over by the history routes (spec §8.1), so an agent cannot read the account twice as fast
    // by watching it. Over the budget the cursor is **held** rather than advanced: the agent is
    // told once, the stream stays open, and the rows are still there when there is room again.
    // A 429 is not available mid-stream, and closing the connection would lose a reconnect's
    // worth of events for a limit that clears itself.
    if read_budget(&w.state, &w.link, sendable.len() as i64).is_err() {
        if !w.told_limited {
            w.told_limited = true;
            w.pending.push_back(
                Event::default().event("limited").data(
                    json!({
                        "retry_after": HOUR_MS / 1000,
                        "message": "the hourly message budget is spent; this stream will catch up from its cursor when it refills",
                    })
                    .to_string(),
                ),
            );
        }
        return;
    }
    w.told_limited = false;
    w.gap_told = false;
    let mut charged: HashMap<String, i64> = HashMap::new();
    for (id, view, payload) in sendable {
        w.pending.push_back(
            Event::default()
                .id(id.to_string())
                .event(payload["type"].as_str().unwrap_or("message.received"))
                .data(event_json(&w.state, &view, &payload).to_string()),
        );
        *charged.entry(view.id.clone()).or_insert(0) += 1;
    }
    // One audit row per chat per wake rather than one per message: the audit is what the budget
    // is summed from, and the relay has a single SQLite writer.
    for (chat, n) in charged {
        note_read(&w.state, &w.link, &w.headers, &chat, n);
    }
    w.cursor = highest;
}

/// The `data` of one durable event: the chat, and the message as the history routes render it.
///
/// Read out of the chat doc through [`crate::agent_link_read::line_json`], so the stream and
/// `GET /agent-link/chats/{id}/messages` cannot disagree about what a message says, who sent it
/// or where its attachments are. When the doc does not hold the row — the announce reaching the
/// relay a moment before the row is applied — the announce's preview is sent instead and
/// `preview: true` says so, rather than an empty message or a dropped event.
fn event_json(state: &AppState, view: &ChatView, payload: &Value) -> Value {
    let message_id = payload["message_id"].as_str().unwrap_or_default();
    let row = load_doc(state, &view.id, view.project.as_deref())
        .ok()
        .flatten()
        .and_then(|doc| line_json(state, view, &doc.projected_rows(), message_id));
    match row {
        Some(message) => json!({ "chat": view.json(), "message": message }),
        None => json!({
            "chat": view.json(),
            "message": {
                "id": message_id,
                "at": payload["at"],
                "sender": payload["from"],
                "text": payload["preview"],
                "attachments": [],
                "reply_to": payload["reply_to"],
            },
            "preview": true,
        }),
    }
}

fn bye(reason: &str) -> Event {
    Event::default()
        .event("bye")
        .data(json!({ "reason": reason }).to_string())
}

fn last_event_id(headers: &HeaderMap) -> Option<i64> {
    headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<i64>().ok())
}

fn stream_limited(message: &str) -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(header::RETRY_AFTER, "60")],
        Json(json!({"error": {"type": "rate_limited", "message": message}})),
    )
        .into_response()
}

/// A held stream slot, and the closing audit row.
///
/// Both halves are in `Drop` because a stream almost never ends by running off the end of its
/// loop — the agent goes away and the response future is dropped. Anything written at the bottom
/// of the handler would therefore be written almost never.
struct Open {
    hub: Arc<LinkHub>,
    db: Db,
    link_id: String,
    user_id: String,
    ip: Option<String>,
    agent: Option<String>,
    opened_at: i64,
}

impl Open {
    fn take(state: &AppState, link: &AgentLink, headers: &HeaderMap) -> Option<Self> {
        let n = state.link_hub.take_slot(&link.id)?;
        let ip = client_ip(headers);
        let agent = user_agent(headers);
        let _ = state.db.note_link_audit(
            &link.id,
            &link.user_id,
            "stream_open",
            None,
            None,
            ip.as_deref(),
            agent.as_deref(),
            &json!({ "open": n }),
        );
        Some(Self {
            hub: state.link_hub.clone(),
            db: state.db.clone(),
            link_id: link.id.clone(),
            user_id: link.user_id.clone(),
            ip,
            agent,
            opened_at: now_ms(),
        })
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        self.hub.release_slot(&self.link_id);
        let _ = self.db.note_link_audit(
            &self.link_id,
            &self.user_id,
            "stream_close",
            None,
            None,
            self.ip.as_deref(),
            self.agent.as_deref(),
            &json!({ "held_ms": now_ms() - self.opened_at }),
        );
    }
}
#[cfg(test)]
mod tests {
    //! What the stream must do, and the four things it must not: carry a chat the link was never
    //! granted, stay open after a revoke, cost anything to an account without a link, or let one
    //! link hold the relay's connections open without bound.

    use axum::body::Body;
    use futures::StreamExt;

    use arbos_core::doc::{ChatDoc, UserMessageOpts};
    use arbos_core::ids::ulid;
    use arbos_core::wire;

    use super::*;
    use crate::agent_link::Scope;
    use crate::db::Chat;
    use crate::people::tests_support::{dm, person, project_of};
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    /// One SSE frame, parsed.
    #[derive(Debug)]
    struct Frame {
        event: String,
        id: Option<String>,
        data: Value,
    }

    fn parse(raw: &str) -> Option<Frame> {
        let mut event = String::new();
        let mut id = None;
        let mut data = String::new();
        for line in raw.lines() {
            if let Some(v) = line.strip_prefix("event:") {
                event = v.trim().to_owned();
            } else if let Some(v) = line.strip_prefix("id:") {
                id = Some(v.trim().to_owned());
            } else if let Some(v) = line.strip_prefix("data:") {
                data.push_str(v.trim());
            }
        }
        if event.is_empty() {
            return None;
        }
        Some(Frame {
            event,
            id,
            data: serde_json::from_str(&data).unwrap_or(Value::Null),
        })
    }

    /// Read until `n` frames have arrived or the stream goes quiet. The body never ends on its
    /// own, so the only way out of a wait is the timeout — kept short, because every case here
    /// either has its frames ready or is asserting that none are coming.
    async fn frames_of(body: Body, n: usize) -> Vec<Frame> {
        let mut out = Vec::new();
        let mut buf = String::new();
        let mut stream = body.into_data_stream();
        while out.len() < n {
            let next = tokio::time::timeout(Duration::from_millis(1500), stream.next()).await;
            let Ok(Some(Ok(chunk))) = next else { break };
            buf.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(cut) = buf.find("\n\n") {
                let raw: String = buf.drain(..cut + 2).collect();
                if let Some(f) = parse(&raw) {
                    out.push(f);
                }
            }
        }
        out
    }

    fn link_for(state: &AppState, user: &str, scope: Scope, chats: Option<&[String]>) -> AgentLink {
        let id = ulid();
        state
            .db
            .insert_agent_link(
                &id,
                user,
                "Watcher",
                scope,
                &arbos_core::auth::hash(&ulid()),
                chats,
                "dev:test",
                7 * 24 * 3_600_000,
            )
            .unwrap();
        state
            .db
            .open_agent_link(&id, &arbos_core::auth::hash(&ulid()))
            .unwrap();
        state.link_hub.note_live(user);
        state.db.agent_link(&id).unwrap().expect("the link")
    }

    fn who(link: &AgentLink) -> LinkPrincipal {
        LinkPrincipal { link: link.clone() }
    }

    fn agent_chat(state: &AppState, project_id: &str, title: &str) -> String {
        let id = ulid();
        state
            .db
            .insert_chat(&Chat {
                id: id.clone(),
                project_id: project_id.to_owned(),
                title: title.to_owned(),
                is_pinned: false,
                position: 0,
                created_at: now_ms(),
                doc_epoch: 0,
                kind: "chat".to_owned(),
                participants: vec![],
                forked_from: None,
            })
            .unwrap();
        id
    }

    /// A real message in a chat's room, so the stream has a row to read back.
    fn say(state: &AppState, chat_id: &str, project: Option<&str>, message_id: &str, text: &str) {
        let room = wire::room_name(project.unwrap_or(crate::people::PEOPLE_PROJECT), chat_id);
        let doc = ChatDoc::new(Some(5), false);
        let before = doc.oplog_vv();
        doc.append_user_message(message_id, text, "dev:phone", &UserMessageOpts::default())
            .unwrap();
        state
            .db
            .append_row(
                &room,
                "dev:phone",
                &ulid(),
                &doc.export_updates_since(&before),
            )
            .unwrap();
    }

    fn sender_of(state: &AppState, user: &str) -> Sender {
        Sender::user(state, user)
    }

    async fn open(
        state: &AppState,
        link: &AgentLink,
        q: StreamQuery,
        headers: HeaderMap,
    ) -> Response {
        stream(State(state.clone()), who(link), Query(q), headers).await
    }

    /// The whole point, end to end: a message lands, and the agent watching the stream is handed
    /// it with the same body the history route would have given — one renderer, so the two
    /// surfaces cannot drift.
    #[tokio::test]
    async fn a_message_reaches_the_stream_as_the_history_route_would_render_it() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 7101);
        let project = project_of(&state, &alice, "codingarbos");
        let chat = agent_chat(&state, &project, "Launch week");
        let link = link_for(&state, &alice, Scope::Read, None);

        let r = open(&state, &link, StreamQuery::default(), HeaderMap::new()).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(
            r.headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("text/event-stream")
        );

        say(
            &state,
            &chat,
            Some(&project),
            "m-stream-1",
            "the build is red",
        );
        on_announce(
            &state,
            &ChatRef::project(&project, &chat),
            &sender_of(&state, &alice),
            "m-stream-1",
            "the build is red",
            None,
            &[],
        );

        let frames = frames_of(r.into_body(), 2).await;
        assert_eq!(frames[0].event, "hello", "the first frame is always hello");
        assert_eq!(frames[0].data["protocol"], "link/1");
        assert_eq!(frames[0].data["scope"], "read");

        let got = &frames[1];
        assert_eq!(got.event, "message.received");
        assert!(got.id.is_some(), "a durable event carries a replayable id");
        assert_eq!(got.data["chat"]["id"], chat.as_str());
        assert_eq!(got.data["message"]["id"], "m-stream-1");
        assert_eq!(got.data["message"]["text"], "the build is red");
        assert!(
            got.data["preview"].is_null(),
            "the row was in the doc, so the text is the message and not the push preview"
        );
    }

    /// The chokepoint, asked at send time. A link pinned to one conversation is handed that
    /// conversation and nothing else — and the row it may not see does not stall the stream
    /// behind it either.
    #[tokio::test]
    async fn a_pinned_link_is_never_handed_a_chat_it_was_not_granted() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 7201);
        let (bob, _) = person(&state, 7202);
        let project = project_of(&state, &alice, "codingarbos");
        let granted = agent_chat(&state, &project, "Launch week");
        let private = dm(&state, &alice, &bob);
        let link = link_for(
            &state,
            &alice,
            Scope::Read,
            Some(std::slice::from_ref(&granted)),
        );

        let r = open(&state, &link, StreamQuery::default(), HeaderMap::new()).await;

        // The one the link may not see lands first, so a stream that leaked would leak here.
        say(&state, &private, None, "m-private", "do not forward this");
        on_announce(
            &state,
            &ChatRef::people(&private),
            &sender_of(&state, &alice),
            "m-private",
            "do not forward this",
            None,
            &[],
        );
        say(&state, &granted, Some(&project), "m-granted", "ship it");
        on_announce(
            &state,
            &ChatRef::project(&project, &granted),
            &sender_of(&state, &alice),
            "m-granted",
            "ship it",
            None,
            &[],
        );

        let frames = frames_of(r.into_body(), 3).await;
        let bodies: Vec<String> = frames.iter().map(|f| f.data.to_string()).collect();
        assert!(
            !bodies.iter().any(|b| b.contains("do not forward this")),
            "the ungranted chat reached the stream: {bodies:?}"
        );
        let got: Vec<&Frame> = frames
            .iter()
            .filter(|f| f.event == "message.received")
            .collect();
        assert_eq!(got.len(), 1, "exactly the granted chat: {bodies:?}");
        assert_eq!(got[0].data["chat"]["id"], granted.as_str());
    }

    /// Another account's link gets nothing, even though the feed is keyed per user and the
    /// announce wrote a row for the account that owns the chat.
    #[tokio::test]
    async fn another_account_sees_none_of_it() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 7301);
        let (mallory, _) = person(&state, 7302);
        let project = project_of(&state, &alice, "codingarbos");
        let chat = agent_chat(&state, &project, "Launch week");
        let _alice_link = link_for(&state, &alice, Scope::Read, None);
        let theirs = link_for(&state, &mallory, Scope::Read, None);

        say(&state, &chat, Some(&project), "m-alice", "alice's business");
        on_announce(
            &state,
            &ChatRef::project(&project, &chat),
            &sender_of(&state, &alice),
            "m-alice",
            "alice's business",
            None,
            &[],
        );
        assert!(
            state.db.link_events_head(&alice).unwrap() > 0,
            "the owner's own feed did get the row"
        );
        assert_eq!(
            state.db.link_events_head(&mallory).unwrap(),
            0,
            "a row was written to another account's feed"
        );

        let r = open(&state, &theirs, StreamQuery::default(), HeaderMap::new()).await;
        let frames = frames_of(r.into_body(), 2).await;
        assert_eq!(frames.len(), 1, "only hello: {frames:?}");
        assert_eq!(frames[0].event, "hello");
    }

    /// A revoke closes the stream rather than leaving it readable until some signature expires.
    #[tokio::test]
    async fn a_revoke_closes_the_stream_with_a_reason() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 7401);
        let link = link_for(&state, &alice, Scope::Read, None);

        let r = open(&state, &link, StreamQuery::default(), HeaderMap::new()).await;
        let mut body = r.into_body().into_data_stream();
        let first = tokio::time::timeout(Duration::from_millis(1500), body.next())
            .await
            .expect("hello in time")
            .expect("a chunk")
            .expect("no error");
        assert!(String::from_utf8_lossy(&first).contains("event: hello"));

        assert!(state.db.revoke_agent_link(&link.id).unwrap());
        state.link_hub.forget_if_dead(&state.db, &alice);

        let mut rest = String::new();
        while let Ok(Some(Ok(chunk))) =
            tokio::time::timeout(Duration::from_millis(1500), body.next()).await
        {
            rest.push_str(&String::from_utf8_lossy(&chunk));
            if rest.contains("event: bye") {
                break;
            }
        }
        let bye = rest
            .split("\n\n")
            .filter_map(parse)
            .find(|f| f.event == "bye")
            .expect("a bye frame");
        assert_eq!(bye.data["reason"], "revoked");
    }

    /// `Last-Event-ID` is the agent's own record of what it has seen, so it wins over a `since`
    /// the agent typed, and replay walks forward in order.
    #[tokio::test]
    async fn the_reconnect_header_wins_over_since_and_replays_in_order() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 7501);
        let project = project_of(&state, &alice, "codingarbos");
        let chat = agent_chat(&state, &project, "Launch week");
        let link = link_for(&state, &alice, Scope::Read, None);

        let mut ids = Vec::new();
        for n in 0..3 {
            let mid = format!("m-replay-{n}");
            say(&state, &chat, Some(&project), &mid, &format!("line {n}"));
            on_announce(
                &state,
                &ChatRef::project(&project, &chat),
                &sender_of(&state, &alice),
                &mid,
                &format!("line {n}"),
                None,
                &[],
            );
            ids.push(state.db.link_events_head(&alice).unwrap());
        }

        let mut headers = HeaderMap::new();
        headers.insert("last-event-id", ids[0].to_string().parse().unwrap());
        // `since` points at the newest row, so a stream that honoured it would replay nothing.
        let q = StreamQuery {
            since: Some(ids[2]),
            chat: None,
        };
        let r = open(&state, &link, q, headers).await;
        let frames = frames_of(r.into_body(), 3).await;
        let sent: Vec<&Frame> = frames
            .iter()
            .filter(|f| f.event == "message.received")
            .collect();
        assert_eq!(
            sent.len(),
            2,
            "the header's cursor was not honoured: {frames:?}"
        );
        assert_eq!(sent[0].data["message"]["text"], "line 1");
        assert_eq!(sent[1].data["message"]["text"], "line 2");
        assert_eq!(sent[0].id.as_deref(), Some(ids[1].to_string().as_str()));
    }

    /// A cursor older than the backlog is told so, and still gets everything that survived.
    ///
    /// The sheet promises that a reconnect "gets everything after" the cursor, which the cap
    /// makes false; `id > cursor` returns the survivors and is indistinguishable from a stream
    /// that missed nothing. So the gap is announced rather than left for the reader to infer.
    ///
    /// Asserted as counts, which this test can do because it plants its own events: one `gap`
    /// frame naming the cursor and the floor, and exactly the two surviving messages. An
    /// assertion that merely looked for a missing message would pass against a wrong user id,
    /// which returns nothing at all.
    #[tokio::test]
    async fn a_cursor_that_fell_off_the_backlog_is_told_and_gets_what_survived() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 7651);
        let project = project_of(&state, &alice, "codingarbos");
        let chat = agent_chat(&state, &project, "Launch week");
        let link = link_for(&state, &alice, Scope::Read, None);

        let mut ids = Vec::new();
        for n in 0..4 {
            let mid = format!("m-pruned-{n}");
            say(&state, &chat, Some(&project), &mid, &format!("line {n}"));
            on_announce(
                &state,
                &ChatRef::project(&project, &chat),
                &sender_of(&state, &alice),
                &mid,
                &format!("line {n}"),
                None,
                &[],
            );
            ids.push(state.db.link_events_head(&alice).unwrap());
        }
        // The prune the agent cannot see: the two oldest events age out of the window.
        state
            .db
            .with(|c| {
                c.execute(
                    "DELETE FROM link_events WHERE user_id = ?1 AND id <= ?2",
                    params![&alice, ids[1]],
                )
                .map(|_| ())
            })
            .unwrap();

        let mut headers = HeaderMap::new();
        headers.insert("last-event-id", ids[0].to_string().parse().unwrap());
        let r = open(
            &state,
            &link,
            StreamQuery {
                since: None,
                chat: None,
            },
            headers,
        )
        .await;
        let frames = frames_of(r.into_body(), 4).await;

        let gaps: Vec<&Frame> = frames.iter().filter(|f| f.event == "gap").collect();
        assert_eq!(gaps.len(), 1, "one gap, said once: {frames:?}");
        assert_eq!(gaps[0].data["from"], ids[0]);
        assert_eq!(
            gaps[0].data["oldest_available"], ids[2],
            "the floor the agent must re-read from"
        );
        assert!(
            gaps[0].id.is_none(),
            "a gap is not a cursor and must not be replayed as one"
        );

        let sent: Vec<&Frame> = frames
            .iter()
            .filter(|f| f.event == "message.received")
            .collect();
        assert_eq!(sent.len(), 2, "the survivors, all of them: {frames:?}");
        assert_eq!(sent[0].data["message"]["text"], "line 2");
        assert_eq!(sent[1].data["message"]["text"], "line 3");
    }

    /// The companion to the test above, and the reason it is a pair: an ordinary reconnect that
    /// missed nothing must not be told it did. One cursor, two measurements, both asserted.
    #[tokio::test]
    async fn a_cursor_still_inside_the_backlog_is_not_told_of_a_gap() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 7652);
        let project = project_of(&state, &alice, "codingarbos");
        let chat = agent_chat(&state, &project, "Launch week");
        let link = link_for(&state, &alice, Scope::Read, None);

        let mut ids = Vec::new();
        for n in 0..2 {
            let mid = format!("m-intact-{n}");
            say(&state, &chat, Some(&project), &mid, &format!("line {n}"));
            on_announce(
                &state,
                &ChatRef::project(&project, &chat),
                &sender_of(&state, &alice),
                &mid,
                &format!("line {n}"),
                None,
                &[],
            );
            ids.push(state.db.link_events_head(&alice).unwrap());
        }

        let mut headers = HeaderMap::new();
        headers.insert("last-event-id", ids[0].to_string().parse().unwrap());
        let r = open(
            &state,
            &link,
            StreamQuery {
                since: None,
                chat: None,
            },
            headers,
        )
        .await;
        let frames = frames_of(r.into_body(), 2).await;

        assert_eq!(
            frames.iter().filter(|f| f.event == "gap").count(),
            0,
            "nothing was pruned: {frames:?}"
        );
        let sent: Vec<&Frame> = frames
            .iter()
            .filter(|f| f.event == "message.received")
            .collect();
        assert_eq!(sent.len(), 1, "the one event after the cursor: {frames:?}");
        assert_eq!(sent[0].data["message"]["text"], "line 1");
    }

    /// A fresh connection without a cursor means "from now on". Replaying a day of backlog
    /// nobody asked for would spend the owner's read budget on history the agent already has.
    #[tokio::test]
    async fn a_first_connection_starts_at_the_head_and_not_at_the_backlog() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 7601);
        let project = project_of(&state, &alice, "codingarbos");
        let chat = agent_chat(&state, &project, "Launch week");
        let link = link_for(&state, &alice, Scope::Read, None);

        say(&state, &chat, Some(&project), "m-old", "yesterday");
        on_announce(
            &state,
            &ChatRef::project(&project, &chat),
            &sender_of(&state, &alice),
            "m-old",
            "yesterday",
            None,
            &[],
        );

        let r = open(&state, &link, StreamQuery::default(), HeaderMap::new()).await;
        let frames = frames_of(r.into_body(), 2).await;
        assert_eq!(
            frames.len(),
            1,
            "the backlog was replayed unasked: {frames:?}"
        );
        assert_eq!(
            frames[0].data["since"],
            state.db.link_events_head(&alice).unwrap()
        );
    }

    /// Two at once, and the third is refused — in memory, so a restart clears it rather than
    /// locking the agent out of its own stream with no way for the owner to help.
    #[tokio::test]
    async fn one_link_holds_two_streams_and_no_more() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 7701);
        let link = link_for(&state, &alice, Scope::Read, None);

        let first = open(&state, &link, StreamQuery::default(), HeaderMap::new()).await;
        let second = open(&state, &link, StreamQuery::default(), HeaderMap::new()).await;
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(second.status(), StatusCode::OK);

        let third = open(&state, &link, StreamQuery::default(), HeaderMap::new()).await;
        assert_eq!(third.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(third.headers().contains_key(header::RETRY_AFTER));

        // Letting one go frees its slot.
        drop(first);
        let again = open(&state, &link, StreamQuery::default(), HeaderMap::new()).await;
        assert_eq!(again.status(), StatusCode::OK);
        drop(second);
        drop(again);
    }

    /// A chat the link may not see is not even admitted as a filter, and it is a 404 rather than
    /// a 403: a pinned link must not be able to test for the rest of the account.
    #[tokio::test]
    async fn narrowing_to_an_unreachable_chat_is_not_an_oracle() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 7801);
        let (bob, _) = person(&state, 7802);
        let project = project_of(&state, &alice, "codingarbos");
        let granted = agent_chat(&state, &project, "Launch week");
        let private = dm(&state, &alice, &bob);
        let link = link_for(
            &state,
            &alice,
            Scope::Read,
            Some(std::slice::from_ref(&granted)),
        );

        let q = StreamQuery {
            since: None,
            chat: Some(private.clone()),
        };
        let r = open(&state, &link, q, HeaderMap::new()).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);

        let q = StreamQuery {
            since: None,
            chat: Some(granted.clone()),
        };
        let r = open(&state, &link, q, HeaderMap::new()).await;
        assert_eq!(r.status(), StatusCode::OK);
    }

    /// The claim this feature has to be able to make: an account that never created a link pays
    /// nothing on the hottest path in the relay.
    #[tokio::test]
    async fn an_account_without_a_link_is_untouched_by_any_of_this() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 7901);
        let project = project_of(&state, &alice, "codingarbos");
        let chat = agent_chat(&state, &project, "Launch week");

        assert!(state.link_hub.idle(), "a fresh relay watches nobody");
        say(
            &state,
            &chat,
            Some(&project),
            "m-quiet",
            "nobody is watching",
        );
        on_announce(
            &state,
            &ChatRef::project(&project, &chat),
            &sender_of(&state, &alice),
            "m-quiet",
            "nobody is watching",
            None,
            &[],
        );
        assert_eq!(
            state.db.link_events_head(&alice).unwrap(),
            0,
            "an account with no link had a stream event written for it"
        );
    }

    /// The backlog is a reconnect buffer, not an archive: it is bounded by count on every
    /// insert, and it is deleted outright when the account's last link dies.
    #[tokio::test]
    async fn the_backlog_is_bounded_and_dies_with_the_last_link() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 8001);
        let link = link_for(&state, &alice, Scope::Read, None);

        for n in 0..(BACKLOG_ROWS + 5) {
            state
                .db
                .append_link_event(&alice, "c1", &json!({ "type": "message.received", "n": n }))
                .unwrap();
        }
        let count: i64 = state
            .db
            .with(|c| {
                c.query_row(
                    "SELECT COUNT(*) FROM link_events WHERE user_id = ?1",
                    params![&alice],
                    |r| r.get(0),
                )
            })
            .unwrap();
        assert_eq!(count, BACKLOG_ROWS, "the row bound was not applied");

        // A row older than the age bound goes on the next insert.
        state
            .db
            .with(|c| {
                c.execute(
                    "UPDATE link_events SET created_at = ?2 WHERE user_id = ?1",
                    params![&alice, now_ms() - BACKLOG_MS - 1],
                )
                .map(|_| ())
            })
            .unwrap();
        state
            .db
            .append_link_event(&alice, "c1", &json!({ "type": "message.received" }))
            .unwrap();
        let count: i64 = state
            .db
            .with(|c| {
                c.query_row(
                    "SELECT COUNT(*) FROM link_events WHERE user_id = ?1",
                    params![&alice],
                    |r| r.get(0),
                )
            })
            .unwrap();
        assert_eq!(count, 1, "the age bound was not applied");

        assert!(state.db.revoke_agent_link(&link.id).unwrap());
        state.link_hub.forget_if_dead(&state.db, &alice);
        assert_eq!(
            state.db.link_events_head(&alice).unwrap(),
            0,
            "the backlog outlived the last link that justified it"
        );
        assert!(state.link_hub.idle(), "the account is still being watched");
    }

    /// A second link keeps the account watched: revoking one of two must not throw away the
    /// other's backlog.
    #[tokio::test]
    async fn revoking_one_of_two_links_leaves_the_other_watching() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 8101);
        let one = link_for(&state, &alice, Scope::Read, None);
        let _two = link_for(&state, &alice, Scope::Reply, None);

        state
            .db
            .append_link_event(&alice, "c1", &json!({ "type": "message.received" }))
            .unwrap();
        assert!(state.db.revoke_agent_link(&one.id).unwrap());
        state.link_hub.forget_if_dead(&state.db, &alice);
        assert!(
            state.db.link_events_head(&alice).unwrap() > 0,
            "the surviving link's backlog was thrown away"
        );
        assert!(!state.link_hub.idle());
    }

    /// Stream opens are counted separately from requests, so a reconnect loop with a bug in it
    /// runs out of opens long before it runs out of the per-minute request budget.
    #[tokio::test]
    async fn stream_opens_are_capped_by_the_hour() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 8201);
        let link = link_for(&state, &alice, Scope::Read, None);

        for _ in 0..OPENS_PER_HOUR {
            let r = open(&state, &link, StreamQuery::default(), HeaderMap::new()).await;
            assert_eq!(r.status(), StatusCode::OK);
            drop(r);
        }
        let r = open(&state, &link, StreamQuery::default(), HeaderMap::new()).await;
        assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    /// Opening and closing a stream is in the owner's activity log, and the close is written
    /// even though no line of the loop runs when the agent simply disconnects.
    #[tokio::test]
    async fn the_owner_can_see_the_stream_open_and_close() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 8301);
        let link = link_for(&state, &alice, Scope::Read, None);

        let r = open(&state, &link, StreamQuery::default(), HeaderMap::new()).await;
        assert_eq!(r.status(), StatusCode::OK);
        drop(r);

        let actions: Vec<String> = state
            .db
            .link_audit_rows(&link.id, None, 50)
            .unwrap()
            .iter()
            .map(|r| r["action"].as_str().unwrap_or_default().to_owned())
            .collect();
        assert!(actions.contains(&"stream_open".to_owned()), "{actions:?}");
        assert!(actions.contains(&"stream_close".to_owned()), "{actions:?}");
    }

    /// Messages handed over on the stream come out of the same hourly budget as messages handed
    /// over by the history routes, and over the budget the cursor is held rather than advanced:
    /// the agent is told once and catches up later instead of losing the events.
    #[tokio::test]
    async fn the_stream_spends_the_same_message_budget_and_holds_its_cursor() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 8401);
        let project = project_of(&state, &alice, "codingarbos");
        let chat = agent_chat(&state, &project, "Launch week");
        let link = link_for(&state, &alice, Scope::Read, None);

        // Spend the hour's allowance through the audit the budget is summed from.
        state
            .db
            .note_link_audit(
                &link.id,
                &alice,
                "read",
                Some(&chat),
                None,
                None,
                None,
                &json!({ "messages": 100_000 }),
            )
            .unwrap();

        say(&state, &chat, Some(&project), "m-budget", "over the line");
        on_announce(
            &state,
            &ChatRef::project(&project, &chat),
            &sender_of(&state, &alice),
            "m-budget",
            "over the line",
            None,
            &[],
        );
        let head = state.db.link_events_head(&alice).unwrap();

        let r = open(&state, &link, StreamQuery::default(), HeaderMap::new()).await;
        // Start behind the row so the stream has something it wants to send.
        drop(r);
        let mut headers = HeaderMap::new();
        headers.insert("last-event-id", (head - 1).to_string().parse().unwrap());
        let r = open(&state, &link, StreamQuery::default(), headers).await;
        let frames = frames_of(r.into_body(), 3).await;
        let events: Vec<&str> = frames.iter().map(|f| f.event.as_str()).collect();
        assert_eq!(events, vec!["hello", "limited"], "{frames:?}");
        assert!(
            !frames.iter().any(|f| f.event == "message.received"),
            "a message was handed over with no budget left: {frames:?}"
        );
    }

    /// The wiring, not the module: a message announced the way every path in the relay announces
    /// one reaches the link's feed. One hook, so there is no way to deliver a message to the
    /// apps and not to the stream.
    #[tokio::test]
    async fn the_announce_funnel_itself_feeds_the_stream() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 8501);
        let (bob, _) = person(&state, 8502);
        let chat = dm(&state, &alice, &bob);
        let _link = link_for(&state, &alice, Scope::Read, None);

        say(&state, &chat, None, "m-funnel", "through the funnel");
        crate::notify::announce_with(
            &state,
            &ChatRef::people(&chat),
            &sender_of(&state, &bob),
            "m-funnel",
            "through the funnel",
            None,
            &[],
        )
        .await;

        let head = state.db.link_events_head(&alice).unwrap();
        assert!(head > 0, "announce_with did not reach the link feed");
        let rows = state.db.link_events_after(&alice, 0, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, chat);
        assert_eq!(rows[0].2["message_id"], "m-funnel");
    }

    /// The set the announce path reads is rebuilt at boot, or a relay restarted while a link was
    /// live would write no events until the owner next minted or revoked one.
    #[tokio::test]
    async fn the_watched_set_survives_a_restart() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 8601);
        let link = link_for(&state, &alice, Scope::Read, None);

        let rebuilt = LinkHub::load(&state.db);
        assert!(rebuilt.watches(&alice), "a live link was not loaded");

        assert!(state.db.revoke_agent_link(&link.id).unwrap());
        let rebuilt = LinkHub::load(&state.db);
        assert!(
            !rebuilt.watches(&alice),
            "a revoked link was loaded as if it were live"
        );
    }
}
