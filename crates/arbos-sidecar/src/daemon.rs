//! `arbos-sidecar run`: the one process that holds the key. It long-polls the inbox, applies each
//! page to the mirror, hands wake candidates to the waker, and only then persists the cursor
//! (Buzz L1/L2: exactly-once by cursor, nothing dropped under backpressure). It also serves the
//! control socket the CLI and MCP server use for actions.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::config::{Config, Paths, EXIT_ALREADY_RUNNING, EXIT_KEY_REJECTED};
use crate::mirror::{history_row, now_ms, Mirror, State};
use crate::relay::{Relay, RelayError};
use crate::waker::{Engine, Plugin, Signal};

/// Backoff ladder after a failed relay call (Buzz L3: short ladder, reset after a stable minute).
const BACKOFF: [u64; 6] = [1, 2, 4, 8, 16, 30];
const STABLE: Duration = Duration::from_secs(60);
const HEARTBEAT: Duration = Duration::from_secs(60);
const PAGE: usize = 200;

/// Where events come from. The daemon uses the relay; tests use a scripted source.
pub trait Source: Send + Sync {
    fn next_page(
        &self,
        since: i64,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = crate::relay::RelayResult<crate::relay::InboxPage>>
                + Send
                + '_,
        >,
    >;
}

impl Source for Relay {
    fn next_page(
        &self,
        since: i64,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = crate::relay::RelayResult<crate::relay::InboxPage>>
                + Send
                + '_,
        >,
    > {
        Box::pin(self.inbox(since, 25, PAGE, 0))
    }
}

/// Apply one page: mirror first, then the waker, then the cursor. Returns the new cursor.
pub fn apply_page(
    mirror: &Mirror,
    state: &mut State,
    page: &crate::relay::InboxPage,
    wake_tx: &mpsc::UnboundedSender<Signal>,
) -> Result<usize> {
    let mut applied = 0;
    for e in &page.events {
        let id = e["id"].as_i64().unwrap_or(0);
        if id != 0 && id <= state.cursor {
            continue;
        }
        match mirror.apply_event(e) {
            Ok(Some(c)) => {
                let _ = wake_tx.send(Signal::Event(Box::new(c)));
            }
            Ok(None) => {}
            Err(err) => tracing::warn!(event = id, error = %err, "event not mirrored"),
        }
        applied += 1;
    }
    if page.next > state.cursor {
        state.cursor = page.next;
    }
    state.updated = now_ms();
    mirror.save_state(state)?;
    Ok(applied)
}

/// The fetch → apply → persist loop. Returns only on a rejected key or a closed waker.
pub async fn poll_loop<S: Source>(
    source: &S,
    mirror: &Mirror,
    mut state: State,
    wake_tx: mpsc::UnboundedSender<Signal>,
    mut stop: Option<tokio::sync::oneshot::Receiver<()>>,
) -> Result<State> {
    let mut step = 0usize;
    let mut connected_since: Option<Instant> = None;
    loop {
        if let Some(s) = stop.as_mut() {
            if s.try_recv().is_ok() {
                return Ok(state);
            }
        }
        let fetch = source.next_page(state.cursor);
        let page = if let Some(s) = stop.as_mut() {
            tokio::select! {
                p = fetch => p,
                _ = s => return Ok(state),
            }
        } else {
            fetch.await
        };
        match page {
            Ok(page) => {
                if connected_since.is_none() {
                    connected_since = Some(Instant::now());
                    tracing::info!(cursor = state.cursor, "connected to the relay");
                }
                if connected_since.is_some_and(|t| t.elapsed() >= STABLE) {
                    step = 0;
                }
                if !page.events.is_empty() {
                    let n = apply_page(mirror, &mut state, &page, &wake_tx)?;
                    tracing::debug!(applied = n, cursor = state.cursor, "page applied");
                }
            }
            Err(RelayError::KeyRejected) => {
                tracing::error!(
                    "the relay rejected the agent key — paste the new key with `arbos-sidecar init`"
                );
                bail!("key rejected");
            }
            Err(e) => {
                connected_since = None;
                let delay = BACKOFF[step.min(BACKOFF.len() - 1)];
                let jitter = (now_ms() % 500) as u64;
                tracing::warn!(error = %e, retry_in_s = delay, "relay call failed");
                step += 1;
                tokio::time::sleep(Duration::from_secs(delay) + Duration::from_millis(jitter))
                    .await;
            }
        }
    }
}

/// Backfill history and members for every chat in `/asp/me` that the mirror has not seen.
async fn backfill(relay: &Relay, mirror: &Mirror, cfg: &Config, me: &Value) {
    for c in me["chats"].as_array().into_iter().flatten() {
        let Some(id) = c["id"].as_str() else { continue };
        let meta = mirror.chat(id);
        let mut m = crate::mirror::ChatMeta {
            id: id.to_owned(),
            kind: c["kind"].as_str().unwrap_or("").to_owned(),
            title: c["title"].as_str().unwrap_or("").to_owned(),
            last_at: meta.as_ref().map(|m| m.last_at).unwrap_or(0),
            ..Default::default()
        };
        if meta.as_ref().is_some_and(|m| m.backfilled) {
            let _ = mirror.upsert_chat(&m);
            continue;
        }
        let mut before: Option<String> = None;
        let mut got = 0usize;
        loop {
            match relay.history(id, before.as_deref(), 100).await {
                Ok(v) => {
                    let rows: Vec<Value> = v["messages"]
                        .as_array()
                        .map(|a| a.iter().map(history_row).collect())
                        .unwrap_or_default();
                    got += rows.len();
                    let _ = mirror.prepend_history(id, &rows);
                    let more = v["has_more"] == true;
                    before = v["next_before"].as_str().map(str::to_owned);
                    if !more || before.is_none() || got >= cfg.mirror.backfill {
                        break;
                    }
                }
                Err(RelayError::Scope(s)) => {
                    tracing::info!(chat = id, "{s}");
                    break;
                }
                Err(e) => {
                    tracing::info!(chat = id, error = %e, "history not available yet");
                    break;
                }
            }
        }
        m.backfilled = true;
        let _ = mirror.upsert_chat(&m);
        match relay.members(id).await {
            Ok(v) => {
                let _ = mirror.write_members(id, &v["members"]);
            }
            Err(e) => tracing::debug!(chat = id, error = %e, "members not available yet"),
        }
        let _ = mirror.refresh_brief(id);
    }
}

// ---- control socket -------------------------------------------------------------------------------

/// Answer one control request. Actions carry no key; this process holds it.
pub async fn handle_control(
    relay: &Relay,
    mirror: &Mirror,
    cfg: &Config,
    signal: Option<&mpsc::UnboundedSender<Signal>>,
    req: &Value,
) -> Value {
    let op = req["op"].as_str().unwrap_or("");
    let s = |k: &str| req[k].as_str().unwrap_or("").to_owned();
    let result: crate::relay::RelayResult<Value> = match op {
        "ping" => Ok(json!({ "ok": true, "pid": std::process::id() })),
        "me" => relay.me().await,
        "post" | "reply" => {
            let chat = s("chat");
            let text = s("text");
            let mut ev = json!({ "type": "message.post", "chat": chat, "text": text });
            if op == "reply" {
                ev["reply_to"] = json!(s("message"));
            }
            match relay.event(ev).await {
                Ok(v) => {
                    // A reply answers the rows up to the one it answers; rows that arrived
                    // after it stay pending for the trailing nudge. A plain post answers
                    // everything mirrored so far.
                    let through = if op == "reply" {
                        mirror
                            .rows(&chat)
                            .iter()
                            .find(|r| r["id"] == s("message"))
                            .and_then(|r| r["event_id"].as_i64())
                            .unwrap_or_else(|| mirror.state().cursor)
                    } else {
                        mirror.state().cursor
                    };
                    let _ = mirror.mark_answered(&chat, through);
                    let _ = mirror.refresh_brief(&chat);
                    if let Some(tx) = signal {
                        let _ = tx.send(Signal::Done);
                    }
                    Ok(v)
                }
                Err(e) => Err(e),
            }
        }
        "react" => {
            let r = relay
                .event(json!({
                    "type": "message.react", "chat": s("chat"), "message": s("message"),
                    "emoji": s("emoji"), "on": req["on"].as_bool().unwrap_or(true),
                }))
                .await;
            if r.is_ok() {
                if let Some(tx) = signal {
                    let _ = tx.send(Signal::Done);
                }
            }
            r
        }
        "typing" => {
            relay
                .event(json!({ "type": "typing", "chat": s("chat"), "on": req["on"].as_bool().unwrap_or(true) }))
                .await
        }
        "status" => {
            relay
                .event(json!({ "type": "status", "chat": s("chat"), "text": s("text").chars().take(40).collect::<String>() }))
                .await
        }
        "members" => {
            let chat = s("chat");
            match relay.members(&chat).await {
                Ok(v) => {
                    let _ = mirror.write_members(&chat, &v["members"]);
                    Ok(mirror.members(&chat))
                }
                Err(e) => Err(e),
            }
        }
        "history" => {
            let chat = s("chat");
            let before = req["before"].as_str();
            match relay.history(&chat, before, 100).await {
                Ok(v) => {
                    let rows: Vec<Value> = v["messages"]
                        .as_array()
                        .map(|a| a.iter().map(history_row).collect())
                        .unwrap_or_default();
                    let n = mirror.prepend_history(&chat, &rows).unwrap_or(0);
                    Ok(json!({ "ok": true, "added": n, "has_more": v["has_more"] }))
                }
                Err(e) => Err(e),
            }
        }
        "file" => {
            let chat = s("chat");
            let blob = s("blob");
            match relay.file(&blob).await {
                Ok((bytes, mime)) => match mirror.save_file(&chat, &blob, &bytes) {
                    Ok(p) => Ok(json!({ "ok": true, "path": p, "mime": mime, "size": bytes.len() })),
                    Err(e) => Err(RelayError::Other(e)),
                },
                Err(e) => Err(e),
            }
        }
        "wake_test_relay" => relay.wake_test().await,
        "answered" => {
            let chat = s("chat");
            let head = mirror.state().cursor;
            let _ = mirror.mark_answered(&chat, head);
            if let Some(tx) = signal {
                let _ = tx.send(Signal::Done);
            }
            Ok(json!({ "ok": true }))
        }
        _ => Err(RelayError::Other(anyhow::anyhow!("unknown op {op:?}"))),
    };
    let _ = cfg;
    match result {
        Ok(mut v) => {
            if v.get("ok").is_none() {
                v["ok"] = json!(true);
            }
            v
        }
        Err(RelayError::Scope(msg)) => json!({ "ok": false, "error": "scope", "message": msg }),
        Err(RelayError::KeyRejected) => {
            json!({ "ok": false, "error": "key", "message": "the relay rejected the agent key" })
        }
        Err(RelayError::Other(e)) => {
            json!({ "ok": false, "error": "relay", "message": crate::redact::redact(&format!("{e:#}")) })
        }
    }
}

async fn serve_control(
    listener: UnixListener,
    relay: Relay,
    mirror: Mirror,
    cfg: Config,
    signal: mpsc::UnboundedSender<Signal>,
) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let (relay, mirror, cfg, signal) =
            (relay.clone(), mirror.clone(), cfg.clone(), signal.clone());
        tokio::spawn(async move {
            let (r, mut w) = stream.into_split();
            let mut lines = BufReader::new(r).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let req: Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(e) => json!({ "op": "bad", "error": e.to_string() }),
                };
                let resp = if req["op"] == "bad" {
                    json!({ "ok": false, "error": "bad_json", "message": req["error"] })
                } else {
                    handle_control(&relay, &mirror, &cfg, Some(&signal), &req).await
                };
                let mut out = resp.to_string();
                out.push('\n');
                if w.write_all(out.as_bytes()).await.is_err() {
                    break;
                }
            }
        });
    }
}

/// Is another daemon alive on the socket?
pub async fn daemon_alive(paths: &Paths) -> bool {
    crate::control::call(paths, &json!({ "op": "ping" }))
        .await
        .is_ok()
}

/// `arbos-sidecar run`.
pub async fn run(paths: Paths, cfg: Config) -> Result<i32> {
    paths.ensure()?;
    let key = crate::config::read_key(&cfg.key_path(&paths))?;
    let relay = Relay::new(&cfg.relay, key)?;
    let mirror = Mirror::new(paths.clone());

    if daemon_alive(&paths).await {
        eprintln!("arbos-sidecar is already running (control socket answers)");
        return Ok(EXIT_ALREADY_RUNNING);
    }
    let _ = std::fs::remove_file(paths.socket_file());
    let listener = UnixListener::bind(paths.socket_file()).context("control socket")?;
    std::fs::set_permissions(
        paths.socket_file(),
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )?;
    let _ = std::fs::write(paths.pid_file(), std::process::id().to_string());

    let me = match relay.me().await {
        Ok(v) => v,
        Err(RelayError::KeyRejected) => {
            eprintln!(
                "The relay rejected the agent key. Paste the new key with `arbos-sidecar init`."
            );
            return Ok(EXIT_KEY_REJECTED);
        }
        Err(e) => return Err(anyhow::anyhow!("{e}")),
    };
    let agent = me["agent"].clone();
    tracing::info!(
        agent = %agent["name"].as_str().unwrap_or("?"),
        handle = %agent["handle"].as_str().unwrap_or("?"),
        relay = %relay.base(),
        "signed in"
    );
    let mut state = mirror.state();
    state.agent = agent.clone();
    state.relay = cfg.relay.clone();
    // A fresh mirror starts at the inbox head: old queued events are history, not wakes.
    if state.updated == 0 {
        state.cursor = me["inbox_head"].as_i64().unwrap_or(0);
    }
    mirror.save_state(&state)?;
    backfill(&relay, &mirror, &cfg, &me).await;

    let plugin = Plugin::from_config(&cfg.waker, paths.wakes_file())?;
    // The env contract for spawned agents is explicit and logged — names only (Buzz #8452, L24).
    if let Plugin::Command {
        env_allow, argv, ..
    } = &plugin
    {
        tracing::info!(
            command = %argv.first().cloned().unwrap_or_default(),
            env_forwarded = %env_allow.join(","),
            "command waker: ARBOS_* is never forwarded; the nudge arrives on stdin and in {{prompt}}/{{json}}"
        );
    }
    let engine = Engine::new(cfg.waker.clone(), plugin, mirror.clone(), agent.clone());
    let (wake_tx, wake_rx) = mpsc::unbounded_channel();
    tokio::spawn(engine.run(wake_rx));
    tokio::spawn(serve_control(
        listener,
        relay.clone(),
        mirror.clone(),
        cfg.clone(),
        wake_tx.clone(),
    ));
    let hb_relay = relay.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(HEARTBEAT);
        loop {
            tick.tick().await;
            if let Err(e) = hb_relay.heartbeat().await {
                tracing::debug!(error = %e, "heartbeat");
            }
        }
    });

    match poll_loop(&relay, &mirror, state, wake_tx, None).await {
        Ok(_) => Ok(0),
        Err(e) if e.to_string() == "key rejected" => Ok(EXIT_KEY_REJECTED),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mirror::tests::{received, temp_mirror};
    use crate::relay::InboxPage;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// A scripted relay: pages are handed out in order; the same page can be served twice to
    /// stand in for a retry after a crash between fetch and persist.
    struct Script {
        pages: Mutex<Vec<InboxPage>>,
        calls: AtomicUsize,
        delay: Duration,
    }

    impl Source for Script {
        fn next_page(
            &self,
            since: i64,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = crate::relay::RelayResult<InboxPage>> + Send + '_>,
        > {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(self.delay).await;
                let next = {
                    let mut pages = self.pages.lock().unwrap();
                    if pages.is_empty() {
                        None
                    } else {
                        Some(pages.remove(0))
                    }
                };
                match next {
                    Some(p) => Ok(p),
                    None => {
                        // Park like the real relay does, then answer empty.
                        tokio::time::sleep(Duration::from_secs(25)).await;
                        Ok(InboxPage {
                            events: vec![],
                            next: since,
                        })
                    }
                }
            })
        }
    }

    fn page(ids: std::ops::Range<i64>, chat: &str) -> InboxPage {
        let events: Vec<Value> = ids
            .clone()
            .map(|i| received(i, chat, &format!("m{i}"), &format!("@bot {i}"), true))
            .collect();
        InboxPage {
            events,
            next: ids.end - 1,
        }
    }

    /// Buzz L1: the same page served twice (a retry after a crash between fetch and persist)
    /// leaves one row per message and one wake per message.
    #[tokio::test(start_paused = true)]
    async fn reconnect_replays_exactly_once() {
        let (_d, m) = temp_mirror();
        let p = page(1..6, "c1");
        let script = Script {
            pages: Mutex::new(vec![p.clone(), p.clone(), page(6..9, "c1")]),
            calls: AtomicUsize::new(0),
            delay: Duration::from_millis(10),
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let m2 = m.clone();
        let h = tokio::spawn(async move {
            poll_loop(&script, &m2, State::default(), tx, Some(stop_rx)).await
        });
        tokio::time::sleep(Duration::from_secs(5)).await;
        let _ = stop_tx.send(());
        let state = h.await.unwrap().unwrap();
        assert_eq!(state.cursor, 8);
        assert_eq!(m.rows("c1").len(), 8);
        let mut wakes = 0;
        while rx.try_recv().is_ok() {
            wakes += 1;
        }
        // Events 1–5 (served twice) and 6–8: eight rows, eight wakes, no duplicates.
        assert_eq!(wakes, 8);
    }

    /// Buzz L1: the cursor is written after the rows, never before. A failure mid-apply leaves
    /// the cursor behind so the page is fetched again.
    #[test]
    fn cursor_persists_after_apply_not_before() {
        let (_d, m) = temp_mirror();
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut state = State::default();
        let p = page(1..4, "c1");
        apply_page(&m, &mut state, &p, &tx).unwrap();
        assert_eq!(m.rows("c1").len(), 3);
        assert_eq!(m.state().cursor, 3);
        // Re-applying the same page is a no-op for rows and the cursor.
        apply_page(&m, &mut state, &p, &tx).unwrap();
        assert_eq!(m.rows("c1").len(), 3);
        assert_eq!(m.state().cursor, 3);
        // A page with a lower `next` never moves the cursor backwards.
        apply_page(
            &m,
            &mut state,
            &InboxPage {
                events: vec![],
                next: 1,
            },
            &tx,
        )
        .unwrap();
        assert_eq!(m.state().cursor, 3);
    }

    /// Buzz L2: a slow mirror never loses events — there is no intermediate buffer to overflow.
    #[tokio::test(start_paused = true)]
    async fn slow_mirror_never_loses_events() {
        let (_d, m) = temp_mirror();
        let pages: Vec<InboxPage> = (0..5)
            .map(|k| page(k * 10 + 1..k * 10 + 11, "c1"))
            .collect();
        let script = Script {
            pages: Mutex::new(pages),
            calls: AtomicUsize::new(0),
            delay: Duration::from_millis(200),
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let m2 = m.clone();
        let h = tokio::spawn(async move {
            poll_loop(&script, &m2, State::default(), tx, Some(stop_rx)).await
        });
        tokio::time::sleep(Duration::from_secs(10)).await;
        let _ = stop_tx.send(());
        let state = h.await.unwrap().unwrap();
        assert_eq!(state.cursor, 50);
        assert_eq!(m.rows("c1").len(), 50);
        let mut wakes = 0;
        while rx.try_recv().is_ok() {
            wakes += 1;
        }
        assert_eq!(wakes, 50);
    }

    /// Buzz L3: failures walk the ladder and a stable minute resets it.
    #[tokio::test(start_paused = true)]
    async fn backoff_ladder_resets_after_stable() {
        struct Flaky {
            calls: AtomicUsize,
            fail_first: usize,
        }
        impl Source for Flaky {
            fn next_page(
                &self,
                since: i64,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = crate::relay::RelayResult<InboxPage>>
                        + Send
                        + '_,
                >,
            > {
                Box::pin(async move {
                    let n = self.calls.fetch_add(1, Ordering::SeqCst);
                    if n < self.fail_first {
                        return Err(RelayError::Other(anyhow::anyhow!("down")));
                    }
                    tokio::time::sleep(Duration::from_secs(25)).await;
                    Ok(InboxPage {
                        events: vec![],
                        next: since,
                    })
                })
            }
        }
        let (_d, m) = temp_mirror();
        let flaky = Arc::new(Flaky {
            calls: AtomicUsize::new(0),
            fail_first: 4,
        });
        let (tx, _rx) = mpsc::unbounded_channel();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let f2 = flaky.clone();
        let h = tokio::spawn(async move {
            poll_loop(f2.as_ref(), &m, State::default(), tx, Some(stop_rx)).await
        });
        // 1 + 2 + 4 + 8 s of backoff (+ jitter) → the 5th call succeeds before 20 s.
        tokio::time::sleep(Duration::from_secs(20)).await;
        assert!(flaky.calls.load(Ordering::SeqCst) >= 5);
        let _ = stop_tx.send(());
        let _ = h.await;
    }

    /// Buzz L17 / spec §7: a rejected key ends the loop (the service sees exit 78 and stops).
    #[tokio::test]
    async fn rejected_key_exits_78() {
        struct Rejecting;
        impl Source for Rejecting {
            fn next_page(
                &self,
                _since: i64,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = crate::relay::RelayResult<InboxPage>>
                        + Send
                        + '_,
                >,
            > {
                Box::pin(async { Err(RelayError::KeyRejected) })
            }
        }
        let (_d, m) = temp_mirror();
        let (tx, _rx) = mpsc::unbounded_channel();
        let r = poll_loop(&Rejecting, &m, State::default(), tx, None).await;
        assert_eq!(r.unwrap_err().to_string(), "key rejected");
        assert_eq!(EXIT_KEY_REJECTED, 78);
    }
}
