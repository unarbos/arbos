use anyhow::{Context, Result};
use arbos_core::{
    Event, EventKind, Place, PlaceLock, TranscriptTail, Usage, Wake, append_event, bootstrap,
    files::Layout, inbox, list_agents, load_agent, load_transcript, needs_serve, write_focus,
};
use arbos_engine::{Host, JobsRoot, Registry};
use base64::Engine;
use serde::Serialize;
use std::{net::SocketAddr, sync::Arc};
use tokio::{
    net::TcpListener,
    sync::mpsc,
    time::{Duration, interval},
};

use crate::{
    access,
    attach::{self, Frame, TreeNode},
    doors,
    grep::PlaceGrep,
    hooks::KernelHooks,
    idle, klog, plan,
    pty::PtyHub,
    rewind,
    sched::Scheduler,
    tools,
};

#[derive(Serialize)]
struct KernelJson {
    /// What a client on this machine dials (loopback even for a wildcard bind).
    url: String,
    /// The network address when `--bind` opened the socket; absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bind: Option<String>,
    /// `loopback` (only this machine) or `token` (access.toml clients).
    auth: String,
    /// The same socket as a WebSocket URL, for clients behind an HTTP tunnel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ws: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    clients: Option<usize>,
    pid: u32,
    started: i64,
    /// Names (never values) in this kernel's environment that read like
    /// credentials and are not the model key, the vault token, or a
    /// source in the place's secrets.toml: what came along from the shell
    /// the kernel was started from. `check` warns about them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    stray_secret_env: Vec<String>,
    /// Whether `git` is on this kernel's PATH: without it checkpoints,
    /// rewind and undo are off (said once on root's transcript). Absent
    /// from older kernels; read as unknown.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    git_missing: bool,
    version: String,
    git_sha: String,
    log: String,
}

/// The place lock, or the exit code for a place another kernel holds.
enum Held {
    Taken(PlaceLock),
    /// Another kernel kept the place for the whole wait: exit 3, so a
    /// supervisor can tell "held" from "crashed".
    StillHeld(i32),
}

/// Exit code of a serve that found its place held and gave up waiting.
pub const EXIT_PLACE_HELD: i32 = 3;

/// The place's lock — or, when another kernel holds it, a refusal that
/// says who holds it **once**, falls to a heartbeat, and after a few
/// minutes says plainly that a person needs to look. A supervisor
/// relaunching `serve` every two seconds against a place a stale kernel
/// held logged `place already served` 1411 times over 32 minutes (the pod
/// test, 2026-09-17): the useful facts — who holds it, which build,
/// whether its file is gone — were nowhere, and nothing said that no one
/// was coming. Each relaunch is a fresh process, so "once" lives in
/// `runtime/place-held.json`, keyed on the holder's pid.
///
/// By default the refusal exits at once with `EXIT_PLACE_HELD` (a
/// desktop that lost the spawn race attaches to the winner and must not
/// be kept waiting, nor left a standby kernel that would serve a place
/// the user has since closed). `ARBOS_LOCK_WAIT_SECS=N` makes a
/// supervised kernel wait in-process instead, with the same words.
fn acquire_or_wait(place: &Place) -> Held {
    let wait_secs: u64 = std::env::var("ARBOS_LOCK_WAIT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let started = std::time::Instant::now();
    loop {
        match PlaceLock::acquire(place) {
            Ok(lock) => {
                if let Some(h) = HeldRecord::load(place) {
                    let line = format!(
                        "the place is free after {}s held by pid {}; serving {}",
                        (arbos_core::now_ms() - h.first_ms) / 1000,
                        h.holder_pid,
                        place.path.display()
                    );
                    eprintln!("arbos-kernel: {line}");
                    log_line_to_place(place, "info", "place_freed", &line);
                    HeldRecord::clear(place);
                }
                return Held::Taken(lock);
            }
            Err(e) if e.to_string().contains("place already served") => {
                say_held(place, wait_secs);
                if started.elapsed() >= Duration::from_secs(wait_secs) {
                    return Held::StillHeld(EXIT_PLACE_HELD);
                }
                std::thread::sleep(Duration::from_secs(2));
            }
            Err(e) => {
                // The first thing a kernel writes is its lock, so a folder
                // the person cannot write fails here — and "cannot lock"
                // names our mechanism, not their situation (a shared
                // mount, a folder owned by another account, a read-only
                // disk). Say the situation and what to do.
                let denied = e.chain().any(|c| {
                    c.downcast_ref::<std::io::Error>().is_some_and(|io| {
                        io.kind() == std::io::ErrorKind::PermissionDenied
                            || io.raw_os_error() == Some(libc::EROFS)
                    })
                });
                if denied {
                    eprintln!(
                        "arbos-kernel: cannot start in {}: the folder is not writable by this user ({e:#}). Arbos keeps its records in {}/.arbos and needs to write there — pick another folder, or make this one writable (a shared mount and a folder owned by another account are the usual causes).",
                        place.path.display(),
                        place.path.display()
                    );
                } else {
                    eprintln!("arbos-kernel: cannot lock {}: {e:#}", place.path.display());
                }
                return Held::StillHeld(1);
            }
        }
    }
}

/// Seconds a held place is reported at: the first refusal in full, then
/// one heartbeat a minute, then — after this long — the plain word that a
/// person needs to look, repeated every ten minutes.
const HELD_ESCALATE_SECS: i64 = 300;

/// What was said so far about a held place, across relaunches.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct HeldRecord {
    holder_pid: u32,
    first_ms: i64,
    last_said_ms: i64,
    /// Refusals seen (each is one relaunch, or one poll of a waiter).
    refusals: u64,
    escalated: bool,
}

impl HeldRecord {
    /// Where the record lives: the place's runtime folder, else — when
    /// that cannot be written (qal-j19: `runtime/` read-only turned the
    /// say-once into the long line on every relaunch, then the error
    /// line for ever, because a save that failed was treated as done) —
    /// the machine's temp folder, keyed on the place's path.
    fn paths(place: &Place) -> [std::path::PathBuf; 2] {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&place.path, &mut h);
        [
            place.runtime_dir().join("place-held.json"),
            std::env::temp_dir().join(format!(
                "arbos-place-held-{:016x}.json",
                std::hash::Hasher::finish(&h)
            )),
        ]
    }
    /// The newest copy wherever it lies. A first-match read took a stale
    /// copy in `runtime/` over a live one in the temp folder once the
    /// runtime folder had stopped being writable (qal-j19, the third
    /// shape: writable at first, then not — a full disk, a permission
    /// change), and said the escalation on every relaunch. A write that
    /// landed somewhere other than where the reader looks first is a
    /// write the reader must still find.
    fn load(place: &Place) -> Option<Self> {
        Self::paths(place)
            .iter()
            .filter_map(|p| serde_json::from_str::<Self>(&std::fs::read_to_string(p).ok()?).ok())
            .max_by_key(|r| (r.last_said_ms, r.refusals))
    }
    /// Saved where, or why nowhere. A record that could not be kept is
    /// not a record: the caller says so and speaks as if there were none.
    fn save(&self, place: &Place) -> Result<std::path::PathBuf, String> {
        let text = serde_json::to_string(self).map_err(|e| e.to_string())?;
        let mut why = Vec::new();
        for p in Self::paths(place) {
            let tmp = p.with_extension("json.tmp");
            match std::fs::write(&tmp, &text).and_then(|()| std::fs::rename(&tmp, &p)) {
                Ok(()) => return Ok(p),
                Err(e) => {
                    let _ = std::fs::remove_file(&tmp);
                    why.push(format!("{}: {e}", p.display()));
                }
            }
        }
        Err(why.join("; "))
    }
    fn clear(place: &Place) {
        for p in Self::paths(place) {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// How long the holder has had the place, with no record at all: the
/// lock file is written by the holder when it takes the lock.
fn held_since_lock(place: &Place) -> Option<i64> {
    let modified = place
        .lock_paths()
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok()?.modified().ok())
        .max()?;
    Some(modified.elapsed().ok()?.as_secs() as i64)
}

/// One refusal of a held place, said according to the record: in full
/// the first time for this holder, a heartbeat once a minute, the
/// escalation after `HELD_ESCALATE_SECS`, and otherwise nothing at all.
fn say_held(place: &Place, wait_secs: u64) {
    let now = arbos_core::now_ms();
    let holder_pid = PlaceLock::holder_pid(place).unwrap_or(0);
    let mut rec = match HeldRecord::load(place) {
        Some(r) if r.holder_pid == holder_pid => r,
        _ => HeldRecord {
            holder_pid,
            first_ms: now,
            last_said_ms: 0,
            refusals: 0,
            escalated: false,
        },
    };
    rec.refusals += 1;
    let held_for = (now - rec.first_ms) / 1000;
    let holder = describe_holder(place);
    // The record is what makes "once" possible. When it cannot be kept
    // anywhere, this process cannot know what an earlier one said, so it
    // says the short form — one warn line, the holder and the reason the
    // record failed — and never the long line or the escalation, which
    // would otherwise come on every relaunch (qal-j19: 6 of 6, at error
    // level, for ever).
    let saved = rec.save(place);
    if let Err(why) = &saved {
        let age = held_since_lock(place).unwrap_or(held_for);
        let text = format!(
            "held by {holder} for about {age}s; the held record could not be written ({why}), so this is said in short on every start"
        );
        eprintln!("arbos-kernel: place already served — {text}");
        log_line_to_place(place, "warn", "place_held", &text);
        return;
    }
    let waiting = if wait_secs > 0 {
        format!(
            " This process waits up to {wait_secs}s for the place to be freed (ARBOS_LOCK_WAIT_SECS), then exits {EXIT_PLACE_HELD}."
        )
    } else {
        format!(
            " This process exits {EXIT_PLACE_HELD}; a supervisor that relaunches it will read this once, not every time."
        )
    };
    let line = if rec.refusals == 1 {
        Some((
            "warn",
            format!(
                "another kernel already serves {}: {holder}.{waiting}",
                place.path.display()
            ),
        ))
    } else if held_for >= HELD_ESCALATE_SECS
        && (!rec.escalated || now - rec.last_said_ms >= 600_000)
    {
        rec.escalated = true;
        Some((
            "error",
            format!(
                "a person needs to look: {} has been held for {held_for}s by {holder}; {} start(s) were refused in that time. Stop that kernel (arbos-kernel stop, or kill -TERM its pid) or point this supervisor at another place.",
                place.path.display(),
                rec.refusals
            ),
        ))
    } else if now - rec.last_said_ms >= 60_000 {
        Some((
            "warn",
            format!(
                "still held after {held_for}s: {holder} ({} start(s) refused so far)",
                rec.refusals
            ),
        ))
    } else {
        None
    };
    match line {
        Some((level, text)) => {
            rec.last_said_ms = now;
            // The exact phrase stays on stderr every time: the desktop
            // reads it to tell a lost spawn race from a crash.
            eprintln!("arbos-kernel: place already served — {text}");
            log_line_to_place(place, level, "place_held", &text);
        }
        None => eprintln!(
            "arbos-kernel: place already served by pid {holder_pid} ({held_for}s; said in full in kernel.log)"
        ),
    }
    // Said, so the record must show it: the words above were chosen from
    // the record as loaded; what changed (last_said_ms, escalated) is
    // saved now, and a save that fails here is said in the same breath.
    if let Err(why) = rec.save(place) {
        eprintln!(
            "arbos-kernel: the held record could not be updated ({why}); the next start may say this again"
        );
    }
}

/// Who holds the place, from what is on disk: the pid in the lock file,
/// whether it is alive, which build it runs (kernel.json), whether its
/// file has been replaced under it (a stale image), and its url.
fn describe_holder(place: &Place) -> String {
    let pid = PlaceLock::holder_pid(place);
    let Some(pid) = pid else {
        return "a holder whose pid the lock file does not say".to_string();
    };
    let alive = unsafe { libc::kill(pid as libc::pid_t, 0) } == 0;
    let mut parts = vec![format!(
        "pid {pid}{}",
        if alive {
            ""
        } else {
            " (not alive — the lock should be free momentarily)"
        }
    )];
    if let Ok(text) = std::fs::read_to_string(place.kernel_json_read())
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&text)
        && v["pid"].as_u64() == Some(pid as u64)
    {
        if let Some(sha) = v["git_sha"].as_str().filter(|s| !s.is_empty()) {
            parts.push(format!("build {}", &sha[..sha.len().min(12)]));
        }
        if let Some(url) = v["url"].as_str() {
            parts.push(format!("url {url}"));
        }
    }
    #[cfg(target_os = "linux")]
    if let Ok(exe) = std::fs::read_link(format!("/proc/{pid}/exe")) {
        let s = exe.to_string_lossy();
        if s.ends_with(" (deleted)") {
            parts.push("its file replaced under it — a stale image that restarts onto the new one when idle".to_string());
        }
    }
    parts.join(", ")
}

/// One line into the place's kernel.log before this process has its own
/// logger: the holder's log is the one a person reads.
fn log_line_to_place(place: &Place, level: &str, event: &str, detail: &str) {
    let path = klog::log_path_for(&place.arbos());
    let line = serde_json::json!({
        "ts": arbos_core::now_ms(),
        "level": level,
        "event": event,
        "detail": detail,
        "pid": std::process::id(),
    });
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
    {
        use std::io::Write;
        let _ = writeln!(f, "{line}");
    }
}

/// Every agent transcript whose last line a dead kernel left unfinished,
/// cut back to its last whole line: `(agent id, bytes dropped)`. Archived
/// agents are not touched — nothing appends to them.
/// The other line-a-time records a dead kernel can leave headless — and
/// whose next append would then land on the half line and be lost with
/// it, as a transcript's was (qal-j37, #646): each agent's
/// `checkpoints.jsonl` (the next turn's checkpoint gone, its rewind
/// refused) and `repro.jsonl` (a reproduction gone, the done rule short),
/// the place's `prs.jsonl` (a PR gone from the pill) and
/// `notifications.jsonl` (a notification gone). Cut back to the last
/// whole line at start, under the lock, with `(path, bytes)` for the log.
fn repair_headless_records(place: &Place) -> Vec<(std::path::PathBuf, u64)> {
    let mut paths = vec![
        arbos_core::prs::prs_path(place),
        arbos_core::notify::path(place),
    ];
    for agent in list_agents(place).unwrap_or_default() {
        let dir = Layout::new(place, agent.id.as_str()).dir;
        paths.push(dir.join("checkpoints.jsonl"));
        paths.push(dir.join("repro.jsonl"));
    }
    let mut out = Vec::new();
    for path in paths {
        match arbos_core::files::drop_headless_tail(&path) {
            Ok(Some(dropped)) => out.push((path, dropped)),
            Ok(None) => {}
            Err(e) => eprintln!(
                "arbos: {}: could not check its last line: {e}",
                path.display()
            ),
        }
    }
    out
}

fn repair_headless_tails(place: &Place) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    for agent in list_agents(place).unwrap_or_default() {
        let path = Layout::new(place, agent.id.as_str()).transcript();
        match arbos_core::files::drop_headless_tail(&path) {
            Ok(Some(dropped)) => out.push((agent.id.to_string(), dropped)),
            Ok(None) => {}
            Err(e) => eprintln!(
                "arbos: {}: could not check its last line: {e}",
                path.display()
            ),
        }
    }
    out
}

pub async fn run(place_path: impl Into<std::path::PathBuf>) -> Result<i32> {
    let place = Place::new(
        std::fs::canonicalize(place_path.into())
            .unwrap_or_else(|_| std::env::current_dir().unwrap()),
    );
    let lock = match acquire_or_wait(&place) {
        Held::Taken(lock) => lock,
        Held::StillHeld(code) => return Ok(code),
    };
    // Before anything appends — bootstrap's own notice included: a record
    // the last kernel left mid-line (a crash in a write) would swallow the
    // first line written onto it (qal-j37). The lock is held, so no write
    // is in flight; this is the one moment the cut is safe.
    let repaired = repair_headless_tails(&place);
    let records_repaired = repair_headless_records(&place);
    bootstrap(&place)?;
    klog::init(klog::log_path_for(&place.arbos()));
    for (path, dropped) in records_repaired {
        klog::warn(
            "record_repaired",
            None,
            format!(
                "{}: {dropped} bytes of a half-written last line dropped (a crash mid-write); the record it began is lost, the next one written is not",
                path.display()
            ),
        );
    }
    for (agent, dropped) in repaired {
        klog::warn(
            "transcript_repaired",
            Some(&agent),
            format!("{dropped} bytes of a half-written last line dropped (a crash mid-write)"),
        );
        let _ = append_event(
            &Layout::new(&place, &agent).transcript(),
            &Event::new(EventKind::Notice {
                text: format!(
                    "The kernel that wrote this record before ended in the middle of a line ({dropped} bytes, the head of one event). That half line was dropped so everything from here on reads whole; the event it began was lost with that kernel, not now."
                ),
                failed: false,
            }),
        );
    }
    // Which folder this store is (device, inode): every later look at the
    // path compares against it, so a store renamed out from under the
    // kernel is told apart from a folder recreated where it was.
    let store_id = place
        .store_id()
        .context("the place's .arbos folder could not be identified")?;
    arbos_core::remember_opened(store_id);
    // A second proof beside the inode, for filesystems that re-number
    // theirs; a store that cannot take it is judged by inode alone.
    if let Err(e) = arbos_core::stamp_store(&place) {
        klog::warn(
            "store_token_unwritten",
            None,
            format!("{e} — the store is told apart from a moved one by inode alone"),
        );
    }
    let host = Host::load()?;
    host.remember_place(place.path());
    say_if_host_dir_unwritable(&place, &host);
    let git_present = say_if_git_missing(&place);
    let _ = GIT_PRESENT.set(git_present);
    match (host.api_key(), host.config.api_base()) {
        (Some(key), Ok(base)) => {
            // bash inherits this process's environment, so the model's key is
            // one `env` away; it never reaches the transcript.
            arbos_engine::secrets::store().protect("MODEL_API_KEY", key.clone());
            tokio::spawn(async move {
                arbos_engine::warm(&base, &key).await;
            });
        }
        // Serve anyway: the window opens, and the first turn puts the same
        // line on its transcript. Here it goes to the kernel log.
        (None, _) => eprintln!("{}", host.missing_key_hint()),
        (_, Err(e)) => eprintln!("{e:#}"),
    }

    let grep = PlaceGrep::start(place.path.clone());
    // Jobs from a previous kernel keep running; their folders say what they are.

    // Loopback unless `--bind` says otherwise; off the machine, only a
    // token from access.toml gets in, and there must be at least one.
    let bind = access::bind_addr()?;
    let access = Arc::new(access::Access::load(&place)?);
    let open = !bind.ip().is_loopback();
    if open && !access.has_clients() {
        anyhow::bail!(
            "{bind} is reachable from the network but {} has no [[client]] tokens; refusing to listen (see arbos-kernel help)",
            access::Access::path(&place).display()
        );
    }
    let listener = TcpListener::bind(bind)
        .await
        .with_context(|| format!("bind attach {bind}"))?;
    let addr = listener.local_addr()?;
    write_kernel_json(&place, addr, open, &access, git_present)?;
    if open {
        klog::info(
            "attach_open_bind",
            None,
            format!(
                "bind={addr} clients={} persons={} (persons need the hub)",
                access.client_count(),
                access.person_count()
            ),
        );
    }
    klog::info(
        "kernel_start",
        None,
        format!(
            "pid={} version={} git={} place={} url=tcp://{addr}",
            std::process::id(),
            klog::version(),
            klog::git_sha(),
            place.path.display()
        ),
    );

    let (wake_tx, mut wake_rx) = mpsc::unbounded_channel::<Wake>();
    let (kick_tx, mut kick_rx) = mpsc::unbounded_channel::<()>();
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<String>();
    let (frame_in_tx, mut frame_in_rx) = mpsc::unbounded_channel::<Frame>();

    let hooks = KernelHooks::with_caps(
        place.clone(),
        wake_tx.clone(),
        kick_tx.clone(),
        crate::hooks::Caps::from_config(&host.config),
    );
    let sched = Scheduler::sharing(Arc::clone(&hooks.in_flight));
    let ptys = Arc::new(PtyHub::new());
    let (pty_tx, mut pty_rx) = mpsc::unbounded_channel::<Frame>();
    ptys.bind(place.path.clone(), pty_tx);
    let _ = hooks.ptys.set(Arc::clone(&ptys));
    let mut registry = kernel_registry(&hooks, &ptys);
    // MCP: every tool of every configured server (`.arbos/mcp.toml`,
    // `.cursor/mcp.json`, `~/.config/arbos/mcp.toml`, `ARBOS_MCP_CMD`)
    // joins the registry as `mcp__<server>__<tool>`, callable like any
    // builtin. A server that fails to answer is skipped, not fatal.
    // Discovery talks to processes and HTTP endpoints (blocking clients),
    // so it runs off the async thread.
    let (discovered, problems) = {
        let place = place.clone();
        tokio::task::spawn_blocking(move || {
            let loaded = crate::mcp::load(&place);
            let discovered = loaded
                .servers
                .into_iter()
                .map(|server| {
                    let specs = server.tools();
                    (Arc::new(server), specs)
                })
                .collect::<Vec<_>>();
            (discovered, loaded.problems)
        })
        .await
        .unwrap_or_default()
    };
    // A config file that let the person down is said where they read,
    // not only on stderr (qal-j31: a place file with a typo was skipped in
    // silence and its names fell through to the machine's file).
    for p in &problems {
        let text = crate::mcp::problem_notice(&place, p);
        klog::error("mcp_config", None, &text);
        let _ = append_event(
            &Layout::new(&place, arbos_core::ROOT_ID).transcript(),
            &Event::new(EventKind::Notice { text, failed: true }),
        );
    }
    for (server, specs) in discovered {
        match specs {
            Ok(specs) => {
                klog::info(
                    "mcp",
                    None,
                    format!(
                        "{} offers {}",
                        server.name,
                        specs
                            .iter()
                            .map(|s| s.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
                for spec in specs {
                    registry = registry.with(tools::McpTool::new(Arc::clone(&server), spec));
                }
            }
            Err(err) => klog::error("mcp", None, format!("{}: {err:#}", server.name)),
        }
    }
    let registry = Arc::new(registry);
    {
        let hooks = Arc::clone(&hooks);
        tokio::spawn(async move {
            while let Some(frame) = pty_rx.recv().await {
                hooks.broadcast(frame);
            }
        });
    }

    doors::spawn_telegram_if_configured(Arc::clone(&hooks));
    crate::chatdoor::spawn_if_configured(Arc::clone(&hooks));
    // `--hub`: register outbound so clients and other kernels reach this
    // one by machine name, with no port open here.
    match crate::hub_link::config_from_env() {
        Ok(Some(cfg)) => {
            let project = crate::hub_link::project_name(&place);
            klog::info(
                "hub_link",
                None,
                format!("hub={} machine={} project={project}", cfg.url, cfg.machine),
            );
            crate::hub_link::start(
                place.clone(),
                Arc::clone(&hooks),
                frame_in_tx.clone(),
                cfg,
                project,
            );
        }
        Ok(None) => {}
        Err(e) => eprintln!("hub: {e:#}"),
    }
    crate::remote::RemoteHub::restore(&hooks);

    // A folder under agents/ with no readable agent.md is not an agent:
    // list_agents leaves it out, and nothing else would say so
    // (ba2262db79). Named once at boot; `arbos-kernel check` reports it too.
    for line in arbos_core::unlisted_agent_dirs(&place) {
        klog::warn(
            "agent_unlisted",
            None,
            format!("{line} — not an agent, not listed; remove the folder or give it an agent.md"),
        );
    }

    // Leash pointers (`runtime/leash/<pid>`) whose leash is gone.
    let swept = arbos_engine::sweep_leash_pointers(&place.arbos());
    if swept > 0 {
        klog::info("leash_pointers_swept", None, swept.to_string());
    }
    // Jobs left running by an earlier kernel (parent pid 1) end now: the
    // Mac wake-up incident had one appending to .arbos/user.md every 30 s
    // for three days across restarts. A `keep` file in the job folder
    // spares it.
    for agent in list_agents(&place).unwrap_or_default() {
        // Nothing runs yet: a status line left by a kernel that died
        // mid-turn is stale, and a fresh attach would draw it.
        if arbos_core::status::clear(&place, agent.id.as_str()) {
            klog::info(
                "status_cleared",
                Some(agent.id.as_str()),
                "left by an earlier kernel run",
            );
        }
        let root = arbos_engine::JobsRoot::for_agent(&place, &agent.id);
        let found = root.reap_leftovers();
        // Detached jobs still running after the reap: the ones this
        // kernel inherited (a `keep` file, or an execv — same pid, so
        // their leash never saw a parent die). Logged so a run of the
        // self-updater is a reading: the same job ids and pids before
        // and after the swap.
        let alive: Vec<String> = root
            .list()
            .into_iter()
            .filter(|j| j.running())
            .map(|j| format!("{}:pid={}", j.id, j.meta.pid))
            .collect();
        if !alive.is_empty() {
            crate::klog::info(
                "jobs_alive",
                Some(agent.id.as_str()),
                format!("count={} {}", alive.len(), alive.join(" ")),
            );
        }
        for line in &found.inherited {
            crate::klog::info("job_inherited", Some(agent.id.as_str()), line);
        }
        for line in &found.reaped {
            crate::klog::warn("job_reaped", Some(agent.id.as_str()), line);
        }
        // A subscription's run that the last kernel did not see end — its
        // task died with the image, so its outcome never landed. The
        // subscription says so on its row and fires again at its next
        // due; the log names it (the self-update design's sixth item).
        for job in root.list() {
            let Ok(sub_id) = std::fs::read_to_string(job.dir.join(crate::subs::SUB_MARKER))
                .map(|t| t.trim().parse::<u32>())
            else {
                continue;
            };
            let Ok(sub_id) = sub_id else { continue };
            if job.dir.join("settled").exists() {
                continue;
            }
            if let Err(e) = std::fs::write(job.dir.join("settled"), "cut by a restart\n") {
                // Without the marker the next boot says "cut" again for
                // the same run: a repeat, not a loss, and said as one.
                crate::klog::warn(
                    "settled_unwritten",
                    None,
                    format!(
                        "job {}: {e} — this cut will be reported again at the next start",
                        job.id
                    ),
                );
            }
            let state = if job.running() {
                "still running, its outcome will not be read"
            } else {
                "ended, its outcome was never read"
            };
            crate::klog::warn(
                "subscription_run_cut",
                Some(agent.id.as_str()),
                format!(
                    "#{sub_id} job {}: {state} (kernel restarted mid-run)",
                    job.id
                ),
            );
            if let Some(mut sub) = arbos_core::subscription::get(&place, agent.id.as_str(), sub_id)
            {
                sub.last =
                    format!("run cut by a kernel restart ({state}); fires again at its next due",);
                let _ = arbos_core::subscription::save(&place, agent.id.as_str(), &sub);
            }
        }
        for line in &found.foreign {
            crate::klog::info("job_pid_reused", Some(agent.id.as_str()), line);
        }
        for line in &found.unverified {
            crate::klog::warn(
                "job_unverified",
                Some(agent.id.as_str()),
                format!(
                    "{line} — a process holds this pid but this machine gives no way to tell whether it is the job; left running (kill it by hand if it is, or add a `keep` file)"
                ),
            );
        }
    }

    // A dead kernel's half-run nodes go back to pending. Then continue
    // anyone whose last turn never ended.
    for line in crate::migrate::run(&hooks) {
        crate::klog::info("migrated", None, line);
    }
    plan::reclaim(&hooks);
    warn_if_window_pinned_small(&place, &host, &registry);
    warn_if_model_window_small(&place, &host, &registry).await;
    for agent in list_agents(&place)? {
        if !agent.paused && needs_serve(&place, agent.id.as_str()) {
            let _ = wake_tx.send(Wake::serve(agent.id.as_str()));
        }
    }
    hooks.kick();

    let accept_place = place.clone();
    let accept_hooks = Arc::clone(&hooks);
    let accept_frames = frame_in_tx.clone();
    let accept_access = Arc::clone(&access);
    tokio::spawn(async move {
        loop {
            let Ok((stream, peer)) = listener.accept().await else {
                continue;
            };
            // Transport and login happen off the accept loop: a slow or
            // silent peer must not hold the door for the next one.
            let accept_place = accept_place.clone();
            let accept_hooks = Arc::clone(&accept_hooks);
            let accept_frames = accept_frames.clone();
            let accept_access = Arc::clone(&accept_access);
            tokio::spawn(async move {
                let conn = match attach::Conn::detect(stream).await {
                    Ok(c) => c,
                    Err(e) => {
                        klog::warn("attach_refused", None, format!("peer={peer} {e:#}"));
                        return;
                    }
                };
                // The webhook door: an HTTP POST is a message for an agent,
                // answered and closed here, never a client.
                if let attach::Conn::Hook(req) = conn {
                    webhook(req, peer, &accept_access, &accept_hooks).await;
                    return;
                }
                // A plain GET (a health probe, a browser): a small HTTP
                // reply, then closed — not an attach, not a refusal.
                if let attach::Conn::Http { stream, path } = conn {
                    let auth = if accept_access.has_clients() {
                        "token"
                    } else {
                        "loopback"
                    };
                    let gate = crate::idle::update_gate_json(&accept_hooks, UPDATE_HORIZON_MS);
                    attach::answer_http(stream, &path, klog::version(), PROTOCOL, auth, gate).await;
                    return;
                }
                let (r, w, who) = match admit(conn, peer, &accept_access).await {
                    Ok(x) => x,
                    Err(e) => {
                        klog::warn("attach_refused", None, format!("peer={peer} {e:#}"));
                        return;
                    }
                };
                serve_client(r, w, who, accept_place, accept_hooks, accept_frames).await;
            });
        }
    });
    // One signal stream for the life of the loop. A fresh `ctrl_c()` per
    // `select!` iteration misses a signal that lands while a branch body
    // runs: the old listener is gone and the new one is not yet registered.
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut tick = interval(Duration::from_secs(5));
    let mut tail = interval(Duration::from_millis(200));
    // `--until-idle`: a check a second; the loop ends with its code.
    let mut until_idle = idle::UntilIdle::from_env();
    let mut idle_tick = interval(Duration::from_secs(1));
    // `--leash`: a check every few seconds; alone and idle for the span
    // ends the kernel (a child's kernel on another machine, qa-038).
    let mut leash = idle::Leash::from_env();
    let mut leash_tick = interval(Duration::from_secs(3));
    if let Some(l) = &leash {
        klog::info(
            "leash",
            None,
            format!("exit after {:?} alone and idle", l.after()),
        );
    }
    // `changed` frames for attached clients: a stat pass once a second.
    let mut watch = crate::watch::Watch::default();
    let mut watch_tick = interval(Duration::from_secs(1));
    let mut exit_code = 0;
    // Consecutive five-second looks that found the store missing.
    let mut store_gone = 0u8;
    // `binary_gone` said once; re-exec tried at most once a minute.
    let mut binary_gone_said = false;
    let mut reexec_backoff_until: i64 = 0;
    if let Some(u) = &until_idle {
        klog::info(
            "until_idle",
            None,
            format!(
                "horizon={}s now={}",
                u.horizon_ms() / 1000,
                arbos_core::now_ms()
            ),
        );
    }
    if arbos_core::clock_offset_ms() != 0 {
        klog::info(
            "clock_shift",
            None,
            format!(
                "offset_ms={} now={}",
                arbos_core::clock_offset_ms(),
                arbos_core::now_ms()
            ),
        );
    }
    // One incremental reader per agent. Each poll reads only what was
    // appended since the last one. Primed now, at every existing
    // transcript's end, before the first client can be accepted: a cursor
    // that started at line one made the first tick broadcast every
    // agent's whole record as live `event` frames, and a window that
    // attached during that tick (the desktop connects the moment
    // kernel.json appears) drew every chat twice (F-180, half the launches
    // on a place with history). The record is `history`'s to serve; the
    // tick's word is what is appended from here on.
    let boot_at = std::time::SystemTime::now();
    let mut tails: std::collections::HashMap<String, TranscriptTail> =
        std::collections::HashMap::new();
    for agent in list_agents(&place).unwrap_or_default() {
        let path = Layout::new(&place, agent.id.as_str()).transcript();
        tails.insert(agent.id.to_string(), TranscriptTail::at_end(&path));
    }
    klog::info(
        "tails_primed",
        None,
        format!(
            "agents={} lines={}",
            tails.len(),
            tails.values().map(|t| t.lines).sum::<u64>()
        ),
    );
    shutdown_backstop(place.lock_paths().to_vec());
    // How far each detached job's journal has been streamed (`agent/jN` →
    // bytes, and whether its final frame went out).
    let mut offsets: std::collections::HashMap<String, (u64, bool)> =
        std::collections::HashMap::new();
    // Detached jobs the desktop has been told about, as `agent/jN`. The
    // row opens once, and closes when the job finishes.
    let mut announced: std::collections::HashSet<String> = std::collections::HashSet::new();
    println!("arbos-kernel serve {} at {}", place.path.display(), addr);

    // Start one turn. The wake came from a claimed inbox file or from the
    // kernel's own housekeeping (`Serve`, `Compact`).
    let start = |wake: Wake| {
        let paused = load_agent(&place, &wake.agent).is_ok_and(|a| a.paused);
        if paused || sched.has_job(wake.agent.as_str()) {
            // Housekeeping on a busy agent: compact is requested in-turn by
            // handle_frame; serve is moot. A claimed message that lands
            // here lost a race with a turn already running; the words are
            // in turns/tNNNN/cause.md and the agent reads them next turn
            // through the transcript.
            return;
        }
        let id = wake.agent.to_string();
        // Over the spend cap only the user's own words to a top-level agent
        // open a turn (so the cap can be raised); a worker's brief, a
        // done, or a subscription is refused with a notice on its
        // transcript.
        if arbos_core::spend::over_cap(&place) {
            let top_level = load_agent(&place, &wake.agent).is_ok_and(|a| a.parent.is_none());
            let from_user = wake.kind == arbos_core::WakeKind::User && !wake.steer;
            if !(top_level && from_user) {
                let why = arbos_core::spend::refusal(&place);
                let _ = arbos_core::append_event(
                    &Layout::new(&place, &id).transcript(),
                    &arbos_core::Event::new(arbos_core::EventKind::Notice {
                        text: format!(
                            "[kernel] turn not started — {why}{}",
                            wake.text
                                .as_deref()
                                .map(|t| format!(
                                    " The message was: {}",
                                    arbos_core::text::clip(t, 200)
                                ))
                                .unwrap_or_default()
                        ),
                        failed: true,
                    }),
                );
                klog::warn("turn_refused_spend_cap", Some(&id), why);
                return;
            }
        }
        // Notes waiting in the inbox (wake = false) go on the transcript
        // before the model reads it, whatever started this turn.
        hooks.take_notes(&id);
        hooks.turn_started(&id);
        hooks.broadcast(Frame::Turn {
            agent: id.clone(),
            state: "running".into(),
            budget: None,
        });
        // The sender's label for this turn is the live line until the
        // agent says a step of its own.
        if !wake.title.is_empty() {
            let _ = hooks.set_status(&id, &wake.title, "title");
        }
        // Read afresh each turn: a `configure` frame may have changed the
        // key (in the file, or in memory only).
        let host_now = Host::load().unwrap_or_else(|_| host.clone());
        sched.start(
            place.clone(),
            wake,
            host_now,
            Arc::clone(&registry),
            grep.clone(),
            Arc::clone(&hooks),
            done_tx.clone(),
        );
    };

    loop {
        tokio::select! {
            Some(wake) = wake_rx.recv() => {
                start(wake);
            }
            Some(()) = kick_rx.recv() => {
                // Coalesce a burst of kicks into one scan.
                while kick_rx.try_recv().is_ok() {}
                // A parent's `say mode=stop`: end that turn with its words.
                let stops: Vec<(String, String)> =
                    std::mem::take(&mut *hooks.stop_requests.lock().unwrap());
                for (id, reason) in stops {
                    sched.stop_for(&id, &reason);
                    klog::info("turn_stopped_by_parent", Some(&id), reason);
                }
                for wake in plan::scan(&hooks) {
                    start(wake);
                }
            }
            Some(id) = done_rx.recv() => {
                let control = sched.in_flight.lock().unwrap().remove(&id);
                hooks.turn_ended(&id);
                // The turn's tail — the plan write, the roll, the commit,
                // the idle frame's usage read — is the write that recreated
                // a moved place at its old path (desktop gate, cycle 35).
                // Not one byte of it lands at a path that is not the store
                // this kernel opened.
                if place.store_state(store_id) != arbos_core::StoreState::Intact {
                    say_store_moved(&place, store_id, &lock);
                    crate::remote::stop_all(&hooks).await;
                    exit_code = 4;
                    break;
                }
                // A turn superseded before it did anything is cut from
                // the record once its folder has closed (below), so the
                // fuller message that follows is the only user line.
                let superseded_at = control
                    .as_ref()
                    .filter(|c| c.stop_reason() == SUPERSEDED)
                    .and_then(|_| plan::open_turn_lo(&hooks, &id));
                plan::finish_turn(&hooks, &id);
                if let Some(lo) = superseded_at {
                    supersede_cut(&place, &hooks, &mut tails, &id, lo);
                }
                // A chat nobody named gets its label from the model after
                // its first turn (F-156); decided on disk, called off-loop.
                crate::title::after_turn(&hooks, &id);
                // A standing agent's transcript past the cap rolls into the
                // archive now, between turns; attached windows reload from
                // the short file the way they do after a rewind.
                match arbos_core::files::roll_transcript(&place, &id, hooks.caps.transcript_roll_lines) {
                    Ok(Some(rolled)) => {
                        let mut fresh = TranscriptTail::default();
                        let _ = fresh.read_new(&Layout::new(&place, &id).transcript());
                        tails.insert(id.clone(), fresh);
                        klog::info(
                            "transcript_rolled",
                            Some(&id),
                            format!("{} lines → {}", rolled.lines, rolled.archive.display()),
                        );
                        // The rolled checkpoints are out of a rewind's
                        // reach: their tree commits need no refs now.
                        drop_rolled_checkpoint_refs(&place, &id, &rolled.archive);
                        hooks.broadcast(Frame::Rewound {
                            agent: id.clone(),
                            line: 1,
                            dropped: rolled.lines,
                            restored: None,
                            pending: false,
                        });
                    }
                    Ok(None) => {}
                    Err(e) => klog::warn("transcript_roll_failed", Some(&id), format!("{e:#}")),
                }
                // The record of this turn is a commit in .arbos/.
                crate::snapshot::commit_later(&place, turn_commit_message(&place, &id));
                // A steer the turn never reached is still a file in the
                // inbox; the next scan starts a turn for it.
                drop(control);
                hooks.kick();
                hooks.broadcast(Frame::Turn {
                    agent: id.clone(),
                    state: "idle".into(),
                    budget: last_usage(&place, &id),
                });
                hooks.broadcast(tree_frame(&place));
                hooks.kick();
            }
            // A rewind is handled here, beside the tails: the transcript
            // shrinks, and the tail for that agent must be put at the new
            // end before its next tick, or the cut file would be read again
            // from the top and replayed to every window.
            Some(frame) = frame_in_rx.recv() => {
                if let Frame::Rewind {
                    agent,
                    turn,
                    files,
                    line,
                } = frame
                {
                    let target = match line {
                        Some(line) => rewind::Target::Line(line),
                        None => rewind::Target::Turn(turn),
                    };
                    rewind_live(&place, &hooks, &mut tails, &agent, target, files);
                } else {
                    handle_frame(
                        &place,
                        frame,
                        &wake_tx,
                        &hooks,
                        &sched,
                        &ptys,
                    );
                }
            }
            _ = tick.tick() => {
                // The place's store gone from under the kernel — the folder
                // deleted, a scratch place removed — twice in a row (a
                // mount's hiccup is one look): nothing here can be read or
                // written any more, and a kernel that serves on is the
                // parent every job's leash trusts, so the jobs run on too,
                // writing into unlinked logs. QA's machine: 164 GB. Exit;
                // the leashes see the parent go and end the jobs. Said on
                // stderr, since the log lived in the store.
                match place.store_state(store_id) {
                    arbos_core::StoreState::Gone => {
                        store_gone += 1;
                        if store_gone >= 2 {
                            eprintln!(
                                "arbos-kernel stopping: the place's .arbos store is gone ({}); its jobs end with this kernel",
                                place.arbos().display()
                            );
                            crate::remote::stop_all(&hooks).await;
                            exit_code = 4;
                            break;
                        }
                    }
                    // The folder was renamed and something made a new
                    // `.arbos/` where it was — a kernel's own late write,
                    // through `create_dir_all` on the absolute path, is the
                    // usual maker. One look decides: an inode does not
                    // change and change back. Stop before more lands there.
                    arbos_core::StoreState::Moved => {
                        say_store_moved(&place, store_id, &lock);
                        crate::remote::stop_all(&hooks).await;
                        exit_code = 4;
                        break;
                    }
                    arbos_core::StoreState::Intact => store_gone = 0,
                }
                // The binary replaced under this kernel (an update, an
                // install into the shared PATH): it serves stale code until
                // something restarts it, and a kernel started detached with
                // init as its parent (subnet120) has nothing that will.
                // Under `idle::update_verdict`'s gate — no turn, no
                // question waiting, no run in flight, no remote child
                // mid-turn — it execs onto the file at its own path: same
                // pid, same place lock (flock, released at exec and taken
                // again by the new image), clients reconnect. An exec that
                // fails returns, and the old image serves on and says so.
                if arbos_core::binary_gone() && !binary_gone_said {
                    binary_gone_said = true;
                    klog::warn(
                        "binary_gone",
                        None,
                        "this kernel's file was replaced or moved under it; it runs an old image and will restart onto the new one when idle",
                    );
                }
                if binary_gone_said
                    && arbos_core::binary_gone()
                    && reexec_backoff_until <= arbos_core::now_ms()
                    && matches!(
                        idle::update_verdict_quiet(&hooks, REEXEC_HORIZON_MS),
                        idle::Verdict::Idle
                    )
                {
                    // A new file still being written (the app's swap is a
                    // directory rename, then a copy; an installer streams
                    // the binary) is not a failed restart: look again in
                    // a moment. Only an exec that returned an error waits
                    // the full minute. A restart that missed its window
                    // by a few milliseconds used to wait sixty seconds
                    // for it (binary_gone_e2e red one run in six).
                    reexec_backoff_until = arbos_core::now_ms()
                        + match reexec_onto_new_binary(&place, &hooks) {
                            Reexec::NotReady => REEXEC_LOOK_AGAIN_MS,
                            Reexec::Failed => REEXEC_RETRY_MS,
                        };
                }
                hooks.kick();
                hooks.broadcast(tree_frame(&place));
                say_stalls(&hooks);
            }
            _ = watch_tick.tick() => {
                // Nobody attached: nothing to tell, and no stats to pay for.
                if hooks.frames.lock().unwrap().is_empty() {
                    watch = crate::watch::Watch::default();
                } else {
                    for frame in watch.poll(&place) {
                        hooks.broadcast(frame);
                    }
                }
            }
            _ = leash_tick.tick(), if leash.is_some() => {
                let clients = hooks.frames.lock().unwrap().iter().filter(|tx| !tx.is_closed()).count();
                let busy = !hooks.running.lock().unwrap().is_empty()
                    || !sched.in_flight.lock().unwrap().is_empty();
                if leash.as_mut().is_some_and(|l| l.poll(clients, busy)) {
                    println!("arbos-kernel stopping: no client and nothing running (--leash)");
                    klog::info("kernel_stop", None, "leash");
                    break;
                }
            }
            _ = idle_tick.tick(), if until_idle.is_some() => {
                if let Some(code) = until_idle.as_mut().and_then(|u| u.poll(&hooks)) {
                    let why = if code == idle::EXIT_IDLE { "idle" } else { "waiting on a question" };
                    println!("arbos-kernel stopping: {why} (--until-idle)");
                    klog::info("kernel_stop", None, format!("until_idle:{why}"));
                    exit_code = code;
                    break;
                }
            }
            _ = tail.tick() => {
                let agents = list_agents(&place).unwrap_or_default();
                // A deleted chat takes its cursors with it, so a folder
                // recreated under the same id starts from its first line.
                tails.retain(|id, _| agents.iter().any(|a| a.id.as_str() == id));
                announced.retain(|key| {
                    key.split_once('/')
                        .is_some_and(|(id, _)| agents.iter().any(|a| a.id.as_str() == id))
                });
                for agent in agents {
                    // Detached jobs that finished since the last tick. The
                    // notice goes on the transcript either way; a wake only
                    // when the agent is idle — a running turn reloads the
                    // transcript after every step and sees it there.
                    let jobs = JobsRoot::for_agent(&place, &agent.id);
                    // A job that outlived its tool call is a process row
                    // under the chat until it ends — and so is an attached
                    // command that has run for a while: the user sees what
                    // it is printing instead of a silent live line.
                    let now_ms = arbos_core::now_ms();
                    for job in jobs.list() {
                        let key = format!("{}/{}", agent.id, job.id);
                        if !job.running() || announced.contains(&key) {
                            continue;
                        }
                        if !job.detached() && now_ms - job.meta.started_ms < LONG_ATTACHED_MS {
                            continue;
                        }
                        announced.insert(key);
                        hooks.broadcast(Frame::Board {
                            owner: agent.id.to_string(),
                            action: "open".into(),
                            panel: "process".into(),
                            terminal_ids: vec![job.id.clone()],
                            cwd: Some(job.meta.cwd.display().to_string()),
                            title: Some(job.meta.command.replace('\n', " ")),
                            url: Some(job.journal().display().to_string()),
                            by: "agent".into(),
                        });
                    }
                    // Output streams for every job, attached or detached:
                    // an attached `python3 bubble_sort.py` held its tool
                    // call for minutes and the window heard nothing, so
                    // it told the user "nothing has arrived … check the
                    // model key" while the kernel had the command's
                    // output in its journal the whole time.
                    for job in jobs.list() {
                        let key = format!("{}/{}", agent.id, job.id);
                        if let Some(frame) = job_delta(&mut offsets, &key, &agent.id, &job) {
                            hooks.broadcast(frame);
                        }
                    }
                    for job in jobs.sweep() {
                        announced.remove(&format!("{}/{}", agent.id, job.id));
                        offsets.remove(&format!("{}/{}", agent.id, job.id));
                        hooks.broadcast(Frame::Board {
                            owner: agent.id.to_string(),
                            action: "close".into(),
                            panel: "process".into(),
                            terminal_ids: vec![job.id.clone()],
                            cwd: None,
                            title: Some(job.status_line()),
                            url: Some(job.journal().display().to_string()),
                            by: "agent".into(),
                        });
                        let text = format!(
                            "job {} {} — `{}` — log: {}",
                            job.id,
                            job.status_line(),
                            job.meta.command.replace('\n', " "),
                            job.journal().display()
                        );
                        // One inbox file: a running turn reads it at its
                        // next tool boundary; an idle agent wakes on it.
                        let msg = inbox::Message::new("kernel", "wake", text);
                        if let Err(e) = inbox::deliver(&place, agent.id.as_str(), &msg) {
                            klog::warn("job_notice_failed", Some(agent.id.as_str()), format!("{e:#}"));
                        } else {
                            hooks.kick();
                        }
                    }
                    let path = Layout::new(&place, agent.id.as_str()).transcript();
                    // An agent first seen now: born after boot (a worker
                    // spawned live), its first lines are live and the
                    // cursor starts at line one; from before boot (a fork
                    // or a copy that appeared), its lines are the record,
                    // and the cursor starts at the end.
                    let tail = tails.entry(agent.id.to_string()).or_insert_with(|| {
                        let born_after_boot = Layout::new(&place, agent.id.as_str())
                            .dir
                            .join("agent.md")
                            .metadata()
                            .and_then(|m| m.modified())
                            .is_ok_and(|t| t >= boot_at);
                        if born_after_boot {
                            TranscriptTail::default()
                        } else {
                            TranscriptTail::at_end(&path)
                        }
                    });
                    for mut ev in tail.read_new(&path).unwrap_or_default() {
                        let opened = record_prs(&place, agent.id.as_str(), &ev);
                        if !opened.is_empty() {
                            hooks.broadcast(Frame::Tree { tree: tree_nodes(&place) });
                            follow_prs(&hooks, agent.id.as_str(), &opened);
                        }
                        arbos_core::files::scrub_child_claims(&place, agent.id.as_str(), &mut ev);
                        hooks.broadcast(Frame::Event {
                            agent: agent.id.to_string(),
                            event: ev,
                        });
                    }
                }
            }
            _ = sigint.recv() => {
                println!("arbos-kernel stopping");
                klog::info("kernel_stop", None, "signal");
                arbos_engine::set_kill_reason(STOP_REASON);
                stop_turns(&sched, &hooks, &mut done_rx).await;
                crate::remote::stop_all(&hooks).await;
                end_jobs_for_stop(&place);
                break;
            }
            _ = sigterm.recv() => {
                println!("arbos-kernel stopping");
                klog::info("kernel_stop", None, "signal");
                arbos_engine::set_kill_reason(STOP_REASON);
                stop_turns(&sched, &hooks, &mut done_rx).await;
                crate::remote::stop_all(&hooks).await;
                end_jobs_for_stop(&place);
                break;
            }
        }
    }
    Ok(exit_code)
}

/// A stopping kernel ends its jobs itself, now, with the reason in each
/// folder — not by leaving them for the leash to notice its pid gone.
/// The leash is the backstop for a kernel that dies; a kernel that is
/// asked to stop knows what it started. (SWE-bench cycle 14: 4–10 test
/// processes alive after the kernel had exited, reparented to init.)
/// What every job's `killed` marker says during a graceful stop, whoever
/// writes it — the stop's own sweep or a turn's cancel path.
pub const STOP_REASON: &str = "the kernel was stopped and ended its jobs with it";

fn end_jobs_for_stop(place: &Place) {
    let agents = arbos_core::list_agents(place).unwrap_or_default();
    let mut ended = 0usize;
    for a in agents {
        let root = arbos_engine::JobsRoot::for_agent(place, &a.id);
        for job in root.list() {
            if !job.running() {
                continue;
            }
            // #407: `kill` says whether the signal was delivered; a refusal
            // is `Err` and has already withdrawn the `killed` marker, so the
            // folder keeps reading `running` and the leash stays with it.
            match root.kill(&job) {
                // The marker's words come from the kill reason set at the
                // start of the stop (`STOP_REASON`), the same for every
                // path that kills during it.
                Ok(true) => ended += 1,
                Ok(false) => {}
                Err(e) => klog::warn(
                    "kernel_stop_jobs",
                    None,
                    format!("could not end job {} (pid {}): {e:#}", job.id, job.meta.pid),
                ),
            }
        }
    }
    if ended > 0 {
        klog::info(
            "kernel_stop_jobs",
            None,
            format!("{ended} running job(s) ended"),
        );
    }
}

/// A graceful stop ends every running turn the way the stop button does:
/// the turn writes `interrupted` + `turn_complete`, and its plan node
/// closes as stopped. Without this the folder looked exactly like a crash,
/// and the next start silently resumed a turn the user had ended.
async fn stop_turns(
    sched: &Scheduler,
    hooks: &KernelHooks,
    done_rx: &mut mpsc::UnboundedReceiver<String>,
) {
    let mut pending: std::collections::HashSet<String> =
        sched.in_flight.lock().unwrap().keys().cloned().collect();
    for id in &pending {
        sched.stop_for(id, "kernel stopping");
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !pending.is_empty() {
        match tokio::time::timeout_at(deadline, done_rx.recv()).await {
            Ok(Some(id)) => {
                sched.in_flight.lock().unwrap().remove(&id);
                hooks.turn_ended(&id);
                plan::finish_turn(hooks, &id);
                pending.remove(&id);
            }
            _ => {
                eprintln!(
                    "arbos-kernel: {} turn(s) did not end within 5s: {}",
                    pending.len(),
                    pending.iter().cloned().collect::<Vec<_>>().join(", ")
                );
                break;
            }
        }
    }
}

/// A client asked for something the kernel will not do. Say so on the log
/// and to every attached client; stderr alone reached nobody.
fn refuse(hooks: &KernelHooks, agent: Option<&str>, detail: String) {
    klog::warn("frame_rejected", agent, &detail);
    hooks.broadcast(Frame::Error {
        agent: agent.map(str::to_string),
        detail,
    });
}

fn handle_frame(
    place: &Place,
    frame: Frame,
    wakes: &mpsc::UnboundedSender<Wake>,
    // The hub itself, not a borrow of it: work that must leave this loop —
    // a browser start — takes a handle with it.
    hooks: &Arc<KernelHooks>,
    sched: &Scheduler,
    ptys: &PtyHub,
) {
    // Frames that name an agent must name one the kernel lists. Writing
    // for an unknown id minted a folder with a transcript and no agent.md.
    let names = match &frame {
        Frame::User { agent, .. }
        | Frame::Pause { agent, .. }
        | Frame::Stop { agent, .. }
        | Frame::Compact { agent }
        | Frame::Answer { agent, .. }
        | Frame::Approve { agent, .. }
        | Frame::Kickoff { agent }
        | Frame::Undo { agent }
        | Frame::SetModel { agent, .. }
        | Frame::JobStop { agent, .. }
        | Frame::PlanOp { agent, .. } => Some(agent.clone()),
        _ => None,
    };
    if let Some(agent) = names {
        if !arbos_core::agent_exists(place, &agent) {
            // Answered and logged, not just printed: the client that named
            // a missing agent is the one that needs to hear it (#14 + #24).
            refuse(
                hooks,
                Some(&agent),
                format!("no agent {agent:?} in this place"),
            );
            return;
        }
    }
    match frame {
        Frame::User {
            agent,
            text,
            steer,
            attachments,
            channel,
            device,
            model,
        } => {
            // A path a `put` wrote (`attachments/x.jpg`) is relative to the
            // store, not to the agent's cwd: made absolute here so the
            // engine reads the file the client sent.
            let attachments: Vec<String> = attachments
                .into_iter()
                .map(|a| store_attachment(place, &a))
                .collect();
            // A path that is not a file on this machine (a desktop attaching
            // by path to a remote place, a phone that skipped `put`) used
            // to ride to the model as a name it could not read, and nothing
            // said so. It is dropped from the line, and the client and the
            // transcript hear which file did not arrive and what sends it.
            let (attachments, missing): (Vec<String>, Vec<String>) = attachments
                .into_iter()
                .partition(|a| attachment_present(&agent, place, a));
            if !missing.is_empty() {
                let names: Vec<String> = missing
                    .iter()
                    .map(|m| {
                        std::path::Path::new(m)
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| m.clone())
                    })
                    .collect();
                let detail = format!(
                    "{} did not reach this kernel: the path names no file on this machine ({}). Send the bytes with a `put` frame (path attachments/<name>, data base64) and name that path in `attachments`; the words went through without it.",
                    if names.len() == 1 {
                        "attachment"
                    } else {
                        "attachments"
                    },
                    names.join(", ")
                );
                klog::warn("attachment_missing", Some(&agent), &detail);
                let _ = append_event(
                    &Layout::new(place, &agent).transcript(),
                    &Event::new(EventKind::Notice {
                        text: detail.clone(),
                        failed: true,
                    }),
                );
                hooks.broadcast(Frame::Error {
                    agent: Some(agent.clone()),
                    detail,
                });
            }
            // Where the words came from. A frame without a channel is a
            // typed line (the desktop, the CLI); the voice gateway says so.
            let channel = if channel.is_empty() {
                "text".to_string()
            } else {
                channel
            };
            // A steer goes into the live turn at its next tool boundary.
            // Everything else is a node: it fires now if the agent is idle,
            // else after the current turn — and survives a restart either way.
            if text.trim().is_empty() && attachments.is_empty() {
                eprintln!("inbox {agent}: empty prompt");
                return;
            }
            // `/mode <skill>` pins a skill to this chat as its mode (`/mode
            // off` clears it): a setting, not a prompt. The transcript gets
            // a notice; the next turn's prompt carries the skill.
            if attachments.is_empty()
                && let Some(rest) = text.trim().strip_prefix("/mode")
                && (rest.is_empty() || rest.starts_with(char::is_whitespace))
            {
                let line = match hooks.set_mode_skill(&agent, rest.trim()) {
                    Ok(line) => line,
                    Err(e) => format!("/mode: {e:#}"),
                };
                let _ = append_event(
                    &Layout::new(place, &agent).transcript(),
                    &Event::new(EventKind::Notice {
                        text: line,
                        failed: false,
                    }),
                );
                hooks.broadcast_tree();
                return;
            }
            // "stop" typed at a running agent is the Stop button, not a
            // follow-up: the turn ends now and the transcript says who did it.
            if attachments.is_empty() && sched.has_job(&agent) && arbos_core::is_stop_word(&text) {
                // The turn writes its own `interrupted` line ("Stopped by
                // you" in the window); nothing else to record.
                for id in hooks.stop_work(&agent) {
                    sched.stop(&id);
                }
                return;
            }
            // The same words again while the first copy still waits: a
            // person repeating themselves into a silent turn ("run it"
            // four times, Jacob's Mac, 2026-09-16). Not stacked — one
            // answer is owed, not four — and told so on the transcript,
            // where the window shows it under the bubble.
            if attachments.is_empty()
                && let Some(dup) = inbox::pending_duplicate(place, &agent, "user", &text)
            {
                let what = if inbox::is_steer_kind(&dup.msg.kind) {
                    "waits for the running step to end and will be read then"
                } else if dup.msg.wake {
                    "is queued to run when this turn ends"
                } else {
                    "is held under the composer — Send now runs it"
                };
                let _ = append_event(
                    &Layout::new(place, &agent).transcript(),
                    &Event::new(EventKind::Notice {
                        text: format!(
                            "Already queued: \"{}\" {what}; it was not added again.",
                            arbos_core::text::clip(text.trim(), 80)
                        ),
                        failed: false,
                    }),
                );
                klog::info(
                    "user_line_repeated",
                    Some(&agent),
                    format!("dup of {}", dup.name),
                );
                return;
            }
            // A steer is an inbox file of kind `steer`: the running turn
            // takes it at its next tool boundary; if the turn ends first,
            // the file starts the next turn. Nothing lives in memory.
            if steer && sched.has_job(&agent) {
                let mut msg = inbox::Message::new("user", "steer", text.clone());
                msg.attachments = attachments.clone();
                msg.channel = channel.clone();
                msg.device = device.clone();
                match inbox::deliver(place, &agent, &msg) {
                    Ok(_) => hooks.broadcast(hooks.plan_frame(&agent)),
                    Err(e) => refuse(hooks, Some(&agent), format!("steer: {e:#}")),
                }
                return;
            }
            // A child on another machine: the words go to its kernel.
            if load_agent(place, &arbos_core::AgentId::new(&agent))
                .is_ok_and(|a| a.remote.is_some())
            {
                if let Err(e) = hooks.remotes.forward(hooks, &agent, "user", &text, steer) {
                    let _ = append_event(
                        &Layout::new(place, &agent).transcript(),
                        &Event::new(EventKind::Notice {
                            text: format!("{e:#}"),
                            failed: true,
                        }),
                    );
                }
                return;
            }
            if let Err(e) =
                hooks.inbox_user_on(&agent, &text, attachments, &channel, &device, model.trim())
            {
                refuse(hooks, Some(&agent), format!("inbox: {e:#}"));
            }
        }
        Frame::Configure {
            provider,
            api_base,
            model,
            api_key,
            remember,
        } => match configure(place, &provider, &api_base, &model, &api_key, remember) {
            Ok(frame) => {
                hooks.broadcast(frame);
                let _ = append_event(
                    &Layout::new(place, &focus_agent(place)).transcript(),
                    &Event::new(EventKind::Notice {
                        text: format!(
                            "Model key set for {provider}{}: {}",
                            if model.is_empty() {
                                String::new()
                            } else {
                                format!(" ({model})")
                            },
                            if remember {
                                "saved to this machine's config.toml (owner-readable)"
                            } else {
                                "kept in memory for this kernel only"
                            }
                        ),
                        failed: false,
                    }),
                );
                // Words held in an inbox for want of a key run now.
                hooks.kick();
            }
            Err(e) => refuse(hooks, None, format!("configure: {e:#}")),
        },
        Frame::PlanOp {
            agent,
            node,
            op,
            text,
        } => {
            if let Err(e) = hooks.plan_op(&agent, node, &op, &text) {
                refuse(hooks, Some(&agent), format!("plan op {op} #{node}: {e:#}"));
            }
        }
        Frame::Pause { agent, paused } => {
            if let Ok(mut a) = load_agent(place, &arbos_core::AgentId::new(&agent)) {
                a.paused = paused;
                say_if_unsaved(hooks, &agent, "pause", a.save(&place.agent_dir(&agent)));
            }
            if paused {
                sched.stop(&agent);
            } else {
                // Timers that came due while paused start one period from
                // now: a resume is not a burst of everything missed.
                let reset = crate::subs::resume_subscriptions(place, &agent, arbos_core::now_ms());
                if reset > 0 {
                    klog::info(
                        "subscriptions_resumed",
                        Some(&agent),
                        format!("{reset} rescheduled"),
                    );
                    hooks.plan_changed(&agent);
                }
                hooks.kick();
            }
            hooks.broadcast(tree_frame(place));
        }
        Frame::Focus { path } => {
            // Only an existing agent folder of this place. Anything else is
            // an attach client writing where it should not.
            if let Err(e) = write_focus(place, &path) {
                refuse(hooks, None, format!("{e:#}"));
            }
        }
        Frame::Stop { agent, reason } => {
            if reason.as_deref() == Some(SUPERSEDED) {
                // Not a person stopping anything: the message this turn
                // answers is about to be replaced by a fuller one (a
                // caller paused mid-sentence; the speech gateway merges
                // and resends). Only the turn ends — nothing held, no
                // standing work blocked, no children stopped — and when
                // it ends the done handler cuts its lines if it had done
                // nothing yet, so the record shows one utterance once.
                klog::info(
                    "turn_superseded",
                    Some(&agent),
                    "stop with reason=superseded",
                );
                sched.stop_for(&agent, SUPERSEDED);
            } else {
                // Stop means all of it: the turn, the standing work, the
                // children. A running turn ends; scheduled nodes block until
                // someone presses run.
                for id in hooks.stop_work(&agent) {
                    sched.stop(&id);
                }
            }
        }
        Frame::Seen { through } => {
            // Read on one client is read on all: every window drops its
            // badge together.
            // A client clearing "everything" may send an id past the
            // newest; it means the newest.
            let newest = arbos_core::notify::load(place)
                .last()
                .map(|n| n.id)
                .unwrap_or(0);
            let before = arbos_core::notify::seen_through(place);
            match arbos_core::notify::mark_seen(place, through.min(newest)) {
                Ok(now) => {
                    let unseen = arbos_core::notify::unseen(place).len() as u64;
                    klog::info(
                        "seen_marked",
                        None,
                        format!(
                            "through={through} newest={newest} was={before} now={now} unseen_left={unseen}"
                        ),
                    );
                    hooks.broadcast(Frame::Seen { through: now });
                    hooks.tell_hub(|project| arbos_core::hub::HubFrame::Seen {
                        project,
                        through: now,
                        unseen,
                    });
                }
                Err(e) => klog::warn("seen_failed", None, format!("{e:#}")),
            }
        }
        Frame::Compact { agent } => {
            if !sched.request_compact(&agent) {
                let _ = wakes.send(Wake::compact(&agent));
            }
        }
        Frame::Answer { agent, text, id } => {
            // An approval goes out as an `ask` frame with allow/deny
            // options, and a client may answer it as one (the desktop did:
            // "answer refused: no question is pending" after Jacob clicked
            // allow, 2026-09-15). When the id names the pending approval —
            // or nothing else is pending for this agent — the answer is
            // the verdict.
            let approve_pending = hooks
                .approves
                .lock()
                .unwrap()
                .get(&agent)
                .map(|(_, call_id, _)| call_id.clone());
            let asks_pending = hooks.pending_asks(&agent);
            if let Some(call_id) = approve_pending
                && (id.as_deref() == Some(call_id.as_str())
                    || (asks_pending.is_empty()
                        && id.as_deref().is_none_or(|i| i.is_empty() || i == agent)))
            {
                let allow = matches!(
                    text.trim().to_ascii_lowercase().as_str(),
                    "allow" | "yes" | "y" | "ok" | "approve" | "approved" | "go" | "a"
                );
                resolve_approve(hooks, place, agent, call_id, allow);
                return;
            }
            // Only a question that is pending may resolve; a late or
            // duplicate answer (the same ask arriving twice through the live
            // frame and the transcript tail, qa-021) is refused, not applied.
            let pending: Vec<String> = asks_pending.into_iter().map(|w| w.id).collect();
            let ask_id = match hooks.answer_allowed(&agent, id.as_deref().unwrap_or(""), &pending) {
                Ok(ask_id) => ask_id,
                Err(why) => {
                    refuse(hooks, Some(&agent), format!("answer refused: {why}"));
                    return;
                }
            };
            if let Err(e) = hooks.answer(&agent, &ask_id, &text) {
                refuse(hooks, Some(&agent), format!("answer: {e:#}"));
            }
        }
        Frame::Approve {
            agent,
            call_id,
            allow,
        } => {
            let verdict = {
                let approves = hooks.approves.lock().unwrap();
                let pending: Vec<String> = approves
                    .get(&agent)
                    .map(|(_, id, _)| vec![id.clone()])
                    .unwrap_or_default();
                hooks.answer_allowed(&agent, &call_id, &pending)
            };
            if let Err(why) = verdict {
                refuse(hooks, Some(&agent), format!("approval refused: {why}"));
                return;
            }
            resolve_approve(hooks, place, agent, call_id, allow);
        }
        // A `kickoff` is always answered: filed (a turn follows), refused
        // (an `error` frame says what is missing), or declined because
        // root has a turn on record or one running (a `turn idle` frame,
        // so a client holding words behind the kickoff lets them go).
        // Silence here held a new user's first line forever (qa, keyless
        // first install).
        Frame::Kickoff { agent } => {
            if let Some(hint) = keyless(place) {
                refuse(
                    hooks,
                    Some(&agent),
                    format!(
                        "kickoff not started: {hint} Your first message is kept and runs once a key is in place."
                    ),
                );
                if !hooks.is_live(&agent) {
                    hooks.broadcast(Frame::Turn {
                        agent: agent.clone(),
                        state: "idle".into(),
                        budget: None,
                    });
                }
                return;
            }
            match hooks.kickoff(&agent) {
                Ok(true) => {
                    crate::klog::info("kickoff", Some(&agent), "first open: kickoff turn filed");
                    hooks.kick();
                }
                Ok(false) if hooks.is_live(&agent) => {}
                Ok(false) => hooks.broadcast(Frame::Turn {
                    agent: agent.clone(),
                    state: "idle".into(),
                    budget: None,
                }),
                Err(e) => refuse(hooks, Some(&agent), format!("kickoff: {e:#}")),
            }
        }
        Frame::Undo { agent } => {
            let cwd = load_agent(place, &arbos_core::AgentId::new(&agent))
                .ok()
                .and_then(|a| a.cwd)
                .unwrap_or_else(|| place.path.clone());
            // The mark must be the last turn's: its start line is the
            // last checkpoint's (qal-j10).
            let last = arbos_engine::git::checkpoints(&place.agent_dir(&agent))
                .last()
                .cloned();
            let turn_line = last.as_ref().map(|cp| cp.line).unwrap_or(0);
            let turn_ts = last.as_ref().map(|cp| cp.ts);
            match arbos_engine::git::undo(&cwd, turn_line, turn_ts) {
                Ok(out) => klog::info("undo", Some(&agent), arbos_core::text::clip(&out.body, 200)),
                Err(e) => refuse(hooks, Some(&agent), format!("undo: {e:#}")),
            }
        }
        Frame::SetModel { agent, model } => {
            if let Ok(mut a) = load_agent(place, &arbos_core::AgentId::new(&agent)) {
                a.model = model;
                say_if_unsaved(hooks, &agent, "model", a.save(&place.agent_dir(&agent)));
            }
        }
        Frame::SetMode { agent, mode } => {
            let Some(mode) = arbos_core::Mode::parse(&mode) else {
                eprintln!("set mode {agent}: unknown mode {mode:?}");
                return;
            };
            if let Ok(mut a) = load_agent(place, &arbos_core::AgentId::new(&agent)) {
                a.mode = mode;
                say_if_unsaved(hooks, &agent, "mode", a.save(&place.agent_dir(&agent)));
                // On the record, so the transcript says when the leash changed.
                let _ = append_event(
                    &Layout::new(place, &agent).transcript(),
                    &Event::new(EventKind::Notice {
                        text: format!("mode: {} — {}", mode.as_str(), mode.describe()),
                        failed: false,
                    }),
                );
                // The leash is on the tree: the agent's live children take
                // the mode too, so a switch to ask mid-run reaches the
                // worker that does the writing (desktop cycle 39).
                for id in hooks
                    .descendants(&agent)
                    .into_iter()
                    .filter(|d| d != &agent)
                {
                    if let Ok(mut c) = load_agent(place, &arbos_core::AgentId::new(&id))
                        && c.mode != mode
                    {
                        c.mode = mode;
                        say_if_unsaved(hooks, &id, "mode", c.save(&place.agent_dir(&id)));
                        let _ = append_event(
                            &Layout::new(place, &id).transcript(),
                            &Event::new(EventKind::Notice {
                                text: format!(
                                    "mode: {} — set on {agent}, and on its workers with it",
                                    mode.as_str()
                                ),
                                failed: false,
                            }),
                        );
                    }
                }
            }
            hooks.broadcast(tree_frame(place));
        }
        Frame::VoiceStart => {
            let _ = doors::voice_start();
        }
        Frame::VoiceStop => {
            let _ = doors::voice_stop(hooks);
        }
        Frame::Refresh => doors::refresh(place, wakes),
        Frame::Screen { agent } => {
            // Try Live. A remote child: the kernel on the other machine
            // captures; its answer comes back through the relay and is
            // rebroadcast here under the local id. Local: capture now.
            if let Some(link) = hooks.remotes.link(&agent) {
                let machine = link.record.machine.clone();
                if let Err(e) = link.send(Frame::Screen {
                    agent: "root".into(),
                }) {
                    hooks.broadcast(Frame::Screenshot {
                        agent,
                        machine,
                        png: String::new(),
                        mime: String::new(),
                        width: 0,
                        height: 0,
                        at_ms: arbos_core::now_ms(),
                        error: Some(format!("{e:#}")),
                    });
                    return;
                }
                // A kernel from before Try Live ignores the frame: say so
                // after a while instead of leaving the view waiting.
                hooks.screen_pending.lock().unwrap().insert(agent.clone());
                let senders: Vec<mpsc::UnboundedSender<Frame>> =
                    hooks.frames.lock().unwrap().clone();
                let pending = Arc::clone(&hooks.screen_pending);
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(12)).await;
                    if pending.lock().unwrap().remove(&agent) {
                        let frame = Frame::Screenshot {
                            agent,
                            machine: machine.clone(),
                            png: String::new(),
                            mime: String::new(),
                            width: 0,
                            height: 0,
                            at_ms: arbos_core::now_ms(),
                            error: Some(format!(
                                "no screen from {machine} after 12 s: its kernel may predate Try Live (update the remote kernel) or has no display"
                            )),
                        };
                        for tx in senders {
                            let _ = tx.send(frame.clone());
                        }
                    }
                });
                return;
            }
            let senders: Vec<mpsc::UnboundedSender<Frame>> = hooks.frames.lock().unwrap().clone();
            let place = place.clone();
            tokio::task::spawn_blocking(move || {
                let machine = hostname();
                let frame = match crate::screenshot::grab_screen(&place) {
                    Ok((bytes, mime, width, height)) => Frame::Screenshot {
                        agent,
                        machine,
                        png: Engine::encode(&base64::engine::general_purpose::STANDARD, bytes),
                        mime: mime.into(),
                        width,
                        height,
                        at_ms: arbos_core::now_ms(),
                        error: None,
                    },
                    Err(e) => Frame::Screenshot {
                        agent,
                        machine,
                        png: String::new(),
                        mime: String::new(),
                        width: 0,
                        height: 0,
                        at_ms: arbos_core::now_ms(),
                        error: Some(format!("{e:#}")),
                    },
                };
                for tx in senders {
                    let _ = tx.send(frame.clone());
                }
            });
        }
        Frame::PtyIn { agent, page, data } => {
            if let Ok(bytes) = Engine::decode(&base64::engine::general_purpose::STANDARD, data) {
                let _ = ptys.write(&agent, &page, &bytes);
            }
        }
        Frame::Shell { owner, cwd } => {
            // A person's own shell, asked for from a window: the same
            // `PtyHub` shell the `terminal` tool mints, announced with
            // `by: user` so the drawer opens for it.
            let owner = owner.unwrap_or_else(|| "root".to_string());
            if !arbos_core::agent_exists(place, &owner) {
                refuse(
                    hooks,
                    Some(&owner),
                    format!("shell: no agent {owner:?} in this place"),
                );
                return;
            }
            let dir = match cwd.as_deref().filter(|c| !c.trim().is_empty()) {
                Some(c) => {
                    let p = std::path::PathBuf::from(c);
                    if p.is_absolute() {
                        p
                    } else {
                        place.path.join(p)
                    }
                }
                None => place.path.clone(),
            };
            if !dir.is_dir() {
                refuse(
                    hooks,
                    Some(&owner),
                    format!("shell: {} is not a directory", dir.display()),
                );
                return;
            }
            let id = ptys.next_id();
            match ptys.spawn_shell(&id, &dir, &owner, "user") {
                Ok(_) => {
                    klog::info(
                        "shell_opened",
                        Some(&owner),
                        format!("{id} in {}", dir.display()),
                    );
                    hooks.broadcast(Frame::Board {
                        owner,
                        action: "open".into(),
                        panel: "terminal".into(),
                        terminal_ids: vec![id],
                        cwd: Some(dir.display().to_string()),
                        title: None,
                        url: None,
                        by: "user".into(),
                    });
                }
                Err(e) => refuse(hooks, Some(&owner), format!("shell: {e:#}")),
            }
        }
        Frame::Browse { owner, url } => {
            // A person's own browser tab, asked for from a window: the
            // same Chromium page the `browser` tool uses, announced with
            // `by: user` so the drawer opens on it.
            let owner = owner.unwrap_or_else(|| "root".to_string());
            if !arbos_core::agent_exists(place, &owner) {
                refuse(
                    hooks,
                    Some(&owner),
                    format!("browse: no agent {owner:?} in this place"),
                );
                return;
            }
            let url = url
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .unwrap_or("about:blank")
                .to_string();
            hooks.browsers.touch(&owner);
            hooks.broadcast(Frame::Board {
                owner: owner.clone(),
                action: "open".into(),
                panel: "browser".into(),
                terminal_ids: vec!["b1".into()],
                cwd: None,
                title: None,
                url: Some(url.clone()),
                by: "user".into(),
            });
            // Off this loop, and this is not a nicety. Starting Chromium is
            // seconds of blocking work — spawn the process, poll for its
            // devtools port, open a WebSocket, wait for the page to load —
            // and every client frame in the place comes through here, so a
            // person's Browser click stopped their own typing, their
            // terminals, and every agent's turn until it was done. Worse
            // than the wait: the blocking HTTP client inside it drops a
            // Tokio runtime, and dropping one on a runtime thread is a
            // panic, so the first Browser click killed the kernel outright
            // and the window then had to start a replacement.
            let hooks = Arc::clone(hooks);
            tokio::task::spawn_blocking(move || {
                let png = match hooks.browsers.show(&owner, &url) {
                    Ok(png) => png.or_else(|| hooks.browsers.preview()),
                    Err(e) => {
                        klog::warn("browse", Some(&owner), format!("{e:#}"));
                        hooks.browsers.preview()
                    }
                };
                let at = hooks.browsers.url(&owner);
                hooks.broadcast(Frame::Browser {
                    agent: owner,
                    page: "b1".into(),
                    url: if at.is_empty() { url } else { at },
                    screenshot: png.as_deref().map(|png| {
                        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, png)
                    }),
                });
            });
        }
        Frame::JobStop { agent, id } => {
            let root = arbos_engine::JobsRoot::for_agent(place, &arbos_core::AgentId::new(&agent));
            let Some(job) = root.list().into_iter().find(|j| j.id == id) else {
                refuse(
                    hooks,
                    Some(&agent),
                    format!("job_stop: {agent} has no job {id:?}"),
                );
                return;
            };
            match root.kill_saying(&job, USER_STOP_LINE) {
                Ok(true) => klog::info(
                    "job_stopped_by_user",
                    Some(&agent),
                    format!(
                        "{id} pid {} — `{}`",
                        job.meta.pid,
                        job.meta.command.replace('\n', " ")
                    ),
                ),
                // Already over: the row that asked gets the last word again.
                Ok(false) => hooks.broadcast(Frame::Job {
                    agent,
                    id,
                    delta: String::new(),
                    running: false,
                    exit: match job.status {
                        arbos_engine::JobStatus::Exited(code) => Some(code),
                        _ => None,
                    },
                }),
                Err(e) => refuse(hooks, Some(&agent), format!("job_stop: {e:#}")),
            }
        }
        _ => {}
    }
}

/// What a job's `killed` marker says when a window's Stop ended it.
pub const USER_STOP_LINE: &str = "killed: stopped by the user from the window";

/// Ctrl-C must end the process even when the serve loop is busy. The loop
/// handles the signal itself and exits cleanly; this task is the backstop:
/// if the loop has not returned a few seconds later, drop the lock file
/// (the `PlaceLock` guard would have) and exit, rather than leave a kernel
/// the user cannot stop.
fn shutdown_backstop(lock_paths: Vec<std::path::PathBuf>) {
    tokio::spawn(async move {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut int), Ok(mut term)) = (
            signal(SignalKind::interrupt()),
            signal(SignalKind::terminate()),
        ) else {
            return;
        };
        tokio::select! {
            _ = int.recv() => {}
            _ = term.recv() => {}
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
        eprintln!("arbos-kernel: serve loop did not stop within 5s of the signal; exiting");
        for p in &lock_paths {
            let _ = std::fs::remove_file(p);
        }
        std::process::exit(130);
    });
}

/// What this kernel speaks on the attach socket.
pub const PROTOCOL: u32 = 1;
/// Transcript lines replayed on attach for the focused agent.
const ATTACH_TAIL: u32 = 200;
/// Most lines one `history` request returns.
const HISTORY_MAX: u32 = 2000;
/// Attach connections, numbered: a claim is held by the connection that
/// made it.
static CONN_N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// How many turns `turn_changes` answers with when the asker names none,
/// and the most it will.
const TURN_CHANGES_DEFAULT: u64 = 20;
const TURN_CHANGES_MAX: u64 = 200;

fn focus_agent(place: &Place) -> String {
    let focus = arbos_core::read_focus(place);
    let agent = focus.rsplit('/').next().unwrap_or("root").trim();
    if agent.is_empty() {
        "root".to_string()
    } else {
        agent.to_string()
    }
}

/// Send `agent`'s transcript lines to one client: the last `limit` when
/// `since` is `None` (attach), else those with `seq > since`, oldest
/// first, at most `limit`. Always closed by a `history_end`.
/// Which lines a `history` request wants.
#[derive(Debug, Clone, Copy)]
enum Page {
    /// The newest `limit` lines (attach).
    Tail,
    /// Lines after `seq` (paging forward).
    After(u64),
    /// The `limit` lines before `seq`, nearest first kept (paging back
    /// from the top of what the client holds, M-54).
    Before(u64),
}

fn replay(place: &Place, agent: &str, page: Page, limit: u32, out: &mpsc::UnboundedSender<Frame>) {
    // A finished worker's record lives in the archive; a client asking
    // for it — by id or by the name its card shows — gets the lines from
    // there, flagged, not an empty page. No such agent anywhere: an empty
    // page that says so, not one that reads as an empty record.
    let resolved = arbos_core::files::resolve_history_agent(place, agent);
    let unknown = resolved.is_none();
    let (transcript, archived, id) = match resolved {
        Some(r) => (r.transcript, r.archived, r.id),
        None => (
            Layout::new(place, agent).transcript(),
            false,
            agent.to_string(),
        ),
    };
    if unknown {
        klog::warn(
            "history_unknown",
            Some(agent),
            "no agent live or archived by that id or name",
        );
    }
    let events = load_transcript(&transcript).unwrap_or_default();
    let total = events.len() as u64;
    let picked: Vec<&Event> = match page {
        Page::Tail => {
            let skip = events.len().saturating_sub(limit as usize);
            events[skip..].iter().collect()
        }
        Page::After(since) => events
            .iter()
            .filter(|e| e.seq > since)
            .take(limit as usize)
            .collect(),
        Page::Before(before) => {
            let older: Vec<&Event> = events.iter().filter(|e| e.seq < before).collect();
            let skip = older.len().saturating_sub(limit as usize);
            older[skip..].to_vec()
        }
    };
    let anchor = match page {
        Page::Tail => 0,
        Page::After(s) => s,
        // Nothing older: from = to = before, so the client knows the top.
        Page::Before(b) => b,
    };
    let from = picked.first().map(|e| e.seq).unwrap_or(anchor);
    let to = picked.last().map(|e| e.seq).unwrap_or(anchor);
    for ev in picked {
        let mut event = ev.clone();
        arbos_core::files::scrub_child_claims(place, &id, &mut event);
        // A record from before `output` existed gets its glance here.
        if let EventKind::Tool(rec) = &mut event.kind
            && rec.output.is_none()
        {
            rec.output = rec.digest();
        }
        let _ = out.send(Frame::Replayed {
            agent: agent.to_string(),
            event,
        });
    }
    let _ = out.send(Frame::HistoryEnd {
        agent: agent.to_string(),
        from,
        to,
        total,
        archived,
        path: if archived {
            format!("archive/agents/{id}/transcript.jsonl")
        } else {
            String::new()
        },
        id: if id == agent { String::new() } else { id },
        unknown,
    });
}

/// After a roll, the archived checkpoints' refs go: `NNNN.checkpoints.jsonl`
/// beside the rolled transcript names the lines, and no rewind reaches
/// them (a rewind into rolled history is refused). Best effort.
fn drop_rolled_checkpoint_refs(place: &Place, agent: &str, archive: &std::path::Path) {
    let cps_path = archive.with_extension("checkpoints.jsonl");
    let lines: Vec<u64> = std::fs::read_to_string(&cps_path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<arbos_engine::git::Checkpoint>(l).ok())
        .filter(|cp| cp.work.is_some())
        .map(|cp| cp.line)
        .collect();
    if lines.is_empty() {
        return;
    }
    let cwd = arbos_core::load_agent(place, &arbos_core::AgentId::new(agent))
        .map(|a| a.work_dir(&place.path))
        .unwrap_or_else(|_| place.path.clone());
    let n = lines.len();
    arbos_engine::git::drop_checkpoint_refs(&cwd, agent, lines);
    klog::info(
        "checkpoint_refs_dropped",
        Some(agent),
        format!("{n} rolled checkpoint ref(s) released for git to reclaim"),
    );
}

/// Whether `git` was on PATH at start, for the `hello` frame; set once.
static GIT_PRESENT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// The marker that the missing-git notice was said for this place; in
/// `runtime/`, so a reinstall of the machine starts the question afresh.
const HOST_DIR_UNWRITABLE_SAID: &str = "host-dir-unwritable.said";
const GIT_MISSING_SAID: &str = "git-missing.said";

/// A fresh machine without git (a Mac before the command line tools, a
/// minimal container): the kernel serves, but checkpoints, rewind and
/// undo have nothing to stand on, and until now nothing said so — the
/// person met it as a refused rewind with no cause. Said once on root's
/// transcript, and once more when git appears; `kernel.json` carries
/// `git: false` meanwhile so a window can show it.
/// The host folder (`~/.config/arbos`) that cannot be written — a managed
/// machine, a home on a read-only mount, a folder another user made. The
/// kernel used to die at `Host::load` with a bare "Permission denied (os
/// error 13)": no path, no what. The config reads, so it serves; what is
/// not kept meanwhile is said once on the main chat and in the log, and
/// the marker is cleared when the folder takes writes again.
fn say_if_host_dir_unwritable(place: &Place, host: &Host) {
    let marker = place.runtime_dir().join(HOST_DIR_UNWRITABLE_SAID);
    let said = marker.exists();
    let transcript = Layout::new(place, arbos_core::ROOT_ID).transcript();
    match host.writable() {
        Err(why) => {
            klog::warn("host_dir_unwritable", None, &why);
            if !said {
                let _ = arbos_core::append_event(
                    &transcript,
                    &arbos_core::Event::new(EventKind::Notice {
                        text: format!("This kernel serves, but {why}."),
                        failed: true,
                    }),
                );
                let _ = std::fs::write(&marker, "said\n");
            }
        }
        Ok(()) if said => {
            let _ = std::fs::remove_file(&marker);
            klog::info(
                "host_dir_writable",
                None,
                format!("{} takes writes again", host.dir.display()),
            );
            let _ = arbos_core::append_event(
                &transcript,
                &arbos_core::Event::new(EventKind::Notice {
                    text: format!(
                        "The config folder {} can be written again: setup, the places opened and remote keys are kept from now.",
                        host.dir.display()
                    ),
                    failed: false,
                }),
            );
        }
        Ok(()) => {}
    }
}

fn say_if_git_missing(place: &Place) -> bool {
    let present = std::process::Command::new("git")
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    let marker = place.runtime_dir().join(GIT_MISSING_SAID);
    let said = marker.exists();
    let transcript = Layout::new(place, arbos_core::ROOT_ID).transcript();
    if !present && !said {
        klog::warn(
            "git_missing",
            None,
            "no `git` on PATH: checkpoints, rewind and undo are off until it is installed",
        );
        let _ = arbos_core::append_event(
            &transcript,
            &arbos_core::Event::new(EventKind::Notice {
                text: "git is not installed on this machine (or not on the kernel's PATH), so checkpoints, rewind and undo are off: a turn's changes cannot be taken back until git is installed. On a Mac, `xcode-select --install`; on Debian or Ubuntu, `apt install git`.".into(),
                failed: true,
            }),
        );
        let _ = std::fs::write(&marker, "said\n");
    } else if present && said {
        let _ = std::fs::remove_file(&marker);
        klog::info(
            "git_found",
            None,
            "git is on PATH again: checkpoints, rewind and undo are on",
        );
        let _ = arbos_core::append_event(
            &transcript,
            &arbos_core::Event::new(EventKind::Notice {
                text: "git is installed now: checkpoints, rewind and undo are on from this turn."
                    .into(),
                failed: false,
            }),
        );
    }
    present
}

/// The store is not at its path any more — said on stderr, since the log
/// lived in it; and into the store itself where it is now, when the
/// kernel can tell (its cwd followed the folder), so the person opening
/// the moved project reads why its kernel stopped. Nothing is written at
/// the old path: that would be the ghost.
fn say_store_moved(place: &Place, opened: arbos_core::StoreId, lock: &arbos_core::PlaceLock) {
    let now_at = arbos_core::store_now_at(opened);
    // The lock files went with the folder; `Drop` would look for them at
    // the old path. Take ours away where they are, so the moved store is
    // not left with a lock that names a kernel that is gone.
    if let Some(p) = &now_at {
        lock.release_at(&Place::new(p.clone()));
    }
    let where_ = match &now_at {
        Some(p) => format!("it is now at {}", p.display()),
        None => "where it went, this kernel cannot tell".to_string(),
    };
    eprintln!(
        "arbos-kernel stopping: the .arbos store at {} is not the one this kernel opened (the folder was moved or replaced; {where_}); nothing more is written here, and its jobs end with this kernel",
        place.arbos().display()
    );
    if let Some(p) = now_at {
        let moved = Place::new(p);
        let _ = arbos_core::append_event(
            &Layout::new(&moved, arbos_core::ROOT_ID).transcript(),
            &arbos_core::Event::new(EventKind::Notice {
                text: format!(
                    "This project's folder was moved from {} while its kernel ran. The kernel stopped rather than write into the old path; open the project here to start a new one.",
                    place.path.display()
                ),
                failed: true,
            }),
        );
    }
}

fn snapshot(place: &Place, hooks: &KernelHooks) -> Frame {
    let focus = arbos_core::read_focus(place);
    // The focused agent's last measured context, so a client attaching
    // mid-conversation shows the real meter rather than a placeholder.
    let agent = focus.rsplit('/').next().unwrap_or("root").to_string();
    Frame::Snapshot {
        tree: tree_nodes(place),
        focus,
        budget: last_usage(place, &agent),
        // The kernel's record of what it holds, read now: a window that
        // reattaches after a kernel died draws rows from this, not from
        // what it remembers (#468's frame, at the moment it matters most).
        surfaces: crate::surfaces::list(place, hooks, None),
    }
}

/// The usage the agent's last completed turn reported, if any.
fn last_usage(place: &Place, agent: &str) -> Option<Usage> {
    let events = load_transcript(&Layout::new(place, agent).transcript()).ok()?;
    events.iter().rev().find_map(|e| match &e.kind {
        EventKind::TurnComplete { usage, .. } => *usage,
        _ => None,
    })
}

fn tree_frame(place: &Place) -> Frame {
    Frame::Tree {
        tree: tree_nodes(place),
    }
}

/// Bytes streamed per tick at most. A chatty job still shows its latest
/// lines; the rest stays in the journal file.
const JOB_DELTA_CAP: u64 = 16 * 1024;

/// An attached command running this long gets a process row of its own,
/// as a detached job does: what it prints is visible while the tool
/// call holds the turn.
const LONG_ATTACHED_MS: i64 = 20_000;

/// The journal bytes appended since the last frame, and the job's state, as
/// one `Frame::Job`. `None` when nothing changed, and never again after the
/// final frame. The first look streams from the start of the journal (a job
/// has usually just detached), capped like every tick.
fn job_delta(
    offsets: &mut std::collections::HashMap<String, (u64, bool)>,
    key: &str,
    agent: &arbos_core::AgentId,
    job: &arbos_engine::Job,
) -> Option<Frame> {
    use std::io::{Read, Seek, SeekFrom};
    let size = job.journal_bytes;
    let running = job.running();
    let (seen, finished) = *offsets.entry(key.to_string()).or_insert((0, false));
    if finished {
        return None;
    }
    let mut delta = String::new();
    if size > seen {
        let mut start = seen;
        let mut skipped = 0;
        if size - seen > JOB_DELTA_CAP {
            skipped = size - seen - JOB_DELTA_CAP;
            start = size - JOB_DELTA_CAP;
        }
        if let Ok(mut f) = std::fs::File::open(job.journal()) {
            if f.seek(SeekFrom::Start(start)).is_ok() {
                let mut buf = Vec::with_capacity((size - start) as usize);
                let _ = f.take(size - start).read_to_end(&mut buf);
                if skipped > 0 {
                    delta.push_str(&format!("[… {skipped} bytes skipped]\n"));
                }
                delta.push_str(&String::from_utf8_lossy(&buf));
            }
        }
        offsets.insert(key.to_string(), (size, false));
    }
    if delta.is_empty() && running {
        return None;
    }
    if !running {
        offsets.insert(key.to_string(), (size.max(seen), true));
    }
    // The process row shows the journal live; it gets the same redaction
    // as the transcript, or a key echoed by a job would sit on screen.
    let secrets = arbos_engine::secrets::store();
    let delta = if secrets.has_any() {
        secrets.redact(&delta)
    } else {
        delta
    };
    let exit = match job.status {
        arbos_engine::JobStatus::Exited(code) => Some(code),
        arbos_engine::JobStatus::Running | arbos_engine::JobStatus::Killed => None,
    };
    Some(Frame::Job {
        agent: agent.to_string(),
        id: job.id.clone(),
        delta,
        running,
        exit,
    })
}

fn tree_nodes(place: &Place) -> Vec<TreeNode> {
    let agents = list_agents(place).unwrap_or_default();
    let prs = arbos_core::load_prs(place);
    agents
        .iter()
        .map(|a| TreeNode {
            id: a.id.to_string(),
            name: a.name.clone(),
            title: a.title.clone(),
            // Never an agent as its own ancestor: a parent that is itself,
            // is missing, or leads back around reads as top-level.
            parent: sane_parent(&agents, a),
            paused: a.paused,
            model: a.model.clone(),
            kind: "agent".into(),
            mode: a.mode.as_str().into(),
            prs: arbos_core::prs::prs_of_tree(&prs, a.id.as_str(), &agents).len() as u32,
            step: arbos_core::status::read(place, a.id.as_str()).map(|s| s.step),
            agent_kind: a.kind.clone(),
            readonly: a.readonly,
        })
        .collect()
}

/// `a.parent` unless it is `a` itself, names no agent here, or the chain
/// of parents from it comes back to `a` (or never ends). A window that
/// walks the tree must never loop (#138).
fn sane_parent(agents: &[arbos_core::Agent], a: &arbos_core::Agent) -> Option<String> {
    let parent = a.parent.as_ref()?;
    if parent == &a.id {
        return None;
    }
    let mut cur = parent.clone();
    for _ in 0..agents.len() + 1 {
        let Some(node) = agents.iter().find(|x| x.id == cur) else {
            return None;
        };
        match &node.parent {
            None => return Some(parent.to_string()),
            Some(p) if p == &a.id || p == &node.id => return None,
            Some(p) => cur = p.clone(),
        }
    }
    None
}

/// A finished `bash` whose command ran `gh pr create` and whose output
/// names the pull request: one record per new URL in `.arbos/prs.jsonl`.
/// Returns the records that were new.
fn record_prs(place: &Place, agent: &str, ev: &Event) -> Vec<arbos_core::PrRec> {
    let EventKind::Tool(rec) = &ev.kind else {
        return Vec::new();
    };
    if !matches!(rec.name.as_str(), "bash" | "terminal") || rec.error.is_some() {
        return Vec::new();
    }
    let Some(command) = rec
        .args
        .as_ref()
        .and_then(|a| a.get("command"))
        .and_then(|c| c.as_str())
    else {
        return Vec::new();
    };
    if !arbos_core::prs::opens_pr(command) {
        return Vec::new();
    }
    let Some(body) = rec.body.as_deref() else {
        return Vec::new();
    };
    let branch = arbos_core::prs::head_branch(command);
    let mut new = Vec::new();
    for (url, repo, number) in arbos_core::prs::pr_urls(body) {
        let pr = arbos_core::PrRec {
            ts: arbos_core::now_ms(),
            agent: agent.to_string(),
            url,
            repo,
            number,
            branch: branch.clone(),
        };
        match arbos_core::record_pr(place, &pr) {
            Ok(true) => new.push(pr),
            Ok(false) => {}
            Err(e) => eprintln!("prs: {e:#}"),
        }
    }
    new
}

/// The agent that opened a pull request follows it (Cursor: "cloud agents
/// automatically subscribe to PRs they create and drive them to
/// completion"): one `github_pr` and one `github_ci` subscription per new
/// PR, unless `project.toml` says `follow_prs = false` or the agent
/// already has them. Both go when the PR is merged or closed.
pub(crate) fn follow_prs(hooks: &Arc<KernelHooks>, agent: &str, opened: &[arbos_core::PrRec]) {
    if !arbos_core::project::load(&hooks.place).follows_prs() {
        return;
    }
    let existing = arbos_core::subscription::list(&hooks.place, agent);
    for pr in opened {
        for kind in ["github_pr", "github_ci"] {
            let dup = existing.iter().any(|s| {
                s.kind == kind
                    && s.repo.as_deref() == Some(pr.repo.as_str())
                    && s.pr == Some(pr.number)
            });
            if dup {
                continue;
            }
            let prompt = match kind {
                "github_pr" => format!(
                    "You opened {}. A review comment or a new commit landed: read it (gh pr view {} --repo {} --comments), address it or answer it, and push; if it was merged or closed there is nothing left to do.",
                    pr.url, pr.number, pr.repo
                ),
                _ => format!(
                    "You opened {}. A check changed: when one failed, read its log (gh run view --log-failed), fix the cause on the branch, push, and say what it was; when all are green, say so briefly.",
                    pr.url
                ),
            };
            let sub = arbos_core::subscription::Subscription {
                id: 0,
                kind: kind.into(),
                prompt,
                every: None,
                at: None,
                once: false,
                cmd: None,
                path: None,
                repo: Some(pr.repo.clone()),
                pr: Some(pr.number),
                author: None,
                branch: None,
                channel: None,
                thread: None,
                match_text: None,
                deliver_to: "agent".into(),
                notify: None,
                expires: None,
                paused: false,
                internal: false,
                continuity: false,
                created: String::new(),
                next_due: None,
                last_fired: None,
                last: String::new(),
                error: None,
                seen: None,
            };
            match hooks.subscribe(agent, sub, None) {
                Ok(s) => klog::info(
                    "pr_followed",
                    Some(agent),
                    format!("#{} {kind} {}#{}", s.id, pr.repo, pr.number),
                ),
                Err(e) => klog::warn("pr_follow_failed", Some(agent), format!("{e:#}")),
            }
        }
    }
}

/// Secret-looking variable names in this process's environment that the
/// secrets door does not account for. Names only.
fn stray_secret_env(place: &Place) -> Vec<String> {
    let mut managed = vec![
        "OP_SERVICE_ACCOUNT_TOKEN".to_string(),
        "OP_SESSION".to_string(),
    ];
    if let Ok(host) = Host::load().or_else(|_| Host::peek()) {
        managed.push(host.config.key_env());
    }
    managed.extend(arbos_core::envsafe::place_secret_env_names(place.path()));
    arbos_core::envsafe::stray_secrets(&managed)
        .into_iter()
        .filter(|n| !n.starts_with("OP_SESSION_"))
        .collect()
}

fn write_kernel_json(
    place: &Place,
    addr: SocketAddr,
    open: bool,
    access: &access::Access,
    git_present: bool,
) -> Result<()> {
    arbos_core::check_store(&place.arbos())?;
    let info = KernelJson {
        url: access::local_url(addr),
        bind: open.then(|| addr.to_string()),
        auth: if open { "token" } else { "loopback" }.into(),
        ws: open.then(|| format!("ws://{addr}/")),
        clients: open.then(|| access.client_count()),
        pid: std::process::id(),
        started: arbos_core::now_ms(),
        version: klog::version().into(),
        git_sha: klog::git_sha().into(),
        log: klog::log_path_for(&place.arbos()).display().to_string(),
        stray_secret_env: stray_secret_env(place),
        git_missing: !git_present,
    };
    if !info.stray_secret_env.is_empty() {
        klog::warn(
            "env_stray_secrets",
            None,
            format!(
                "{} variable(s) in this kernel's environment look like credentials and are not managed by the secrets door: {} — start the kernel from a clean environment or declare them in .arbos/secrets.toml",
                info.stray_secret_env.len(),
                info.stray_secret_env.join(", ")
            ),
        );
    }
    let text = serde_json::to_string_pretty(&info)?;
    std::fs::create_dir_all(place.runtime_dir())?;
    std::fs::write(place.kernel_json(), &text)?;
    // One release of the old location too, for windows and phones that
    // still look there. It is ignored by the .arbos/ repository.
    let _ = std::fs::write(place.legacy_kernel_json(), &text);
    Ok(())
}

/// Detect the transport, then let the peer in: loopback as the owner,
/// anyone else with a token from access.toml. Returns the split
/// connection and who it is.
async fn admit(
    conn: attach::Conn,
    peer: SocketAddr,
    access: &access::Access,
) -> Result<(attach::Reader, attach::Writer, access::Identity)> {
    if let attach::Conn::Hook(req) = conn {
        anyhow::bail!("webhook on the attach path: {}", req.uri);
    }
    // Only a plain TCP peer on loopback is "this machine": the desktop, the
    // CLI. A WebSocket from loopback is a tunnel daemon (cloudflared) or a
    // browser fronting for someone else, so it logs in like the network.
    if access::is_local(&peer) && matches!(conn, attach::Conn::Tcp(_)) {
        let (r, w) = conn.split();
        return Ok((r, w, access::Identity::local()));
    }
    // A WebSocket peer may have logged in on the upgrade request itself.
    let presented = match &conn {
        attach::Conn::Ws(_, up) => access::token_from_request(&up.uri, up.authorization.as_deref()),
        attach::Conn::Tcp(_) | attach::Conn::Hook(_) | attach::Conn::Http { .. } => None,
    };
    let (mut r, mut w) = conn.split();
    let token = match presented {
        Some(t) => t,
        None => {
            let first = tokio::time::timeout(
                Duration::from_secs(access::AUTH_TIMEOUT_SECS),
                r.next_line(),
            )
            .await
            .ok()
            .flatten();
            match first.and_then(|l| serde_json::from_str::<Frame>(&l).ok()) {
                Some(Frame::Auth { token }) => token,
                _ => {
                    let _ = w
                        .send(&Frame::Error {
                            agent: None,
                            detail: "auth required: send {\"type\":\"auth\",\"token\":\"…\"} first"
                                .into(),
                        })
                        .await;
                    anyhow::bail!("no auth frame");
                }
            }
        }
    };
    match access.authenticate(&token) {
        Some(who) => Ok((r, w, who)),
        None => {
            let detail = if access.has_clients() {
                "auth failed: unknown token"
            } else {
                "auth failed: this kernel has no [[client]] tokens in .arbos/access.toml"
            };
            let _ = w
                .send(&Frame::Error {
                    agent: None,
                    detail: detail.into(),
                })
                .await;
            anyhow::bail!("bad token")
        }
    }
}

/// The webhook door (K-04): `POST /hook/<agent>` on the attach port
/// becomes an inbox file for that agent — a Slack or Discord outgoing
/// webhook, a CI job's curl, a Zapier step. Loopback needs no token; from
/// anywhere else a `[[client]]` token with the writer role or better, as
/// `Authorization: Bearer …` or `?token=…`. The body is the message: a
/// JSON object's `text` / `content` / `message` / `body` field, else the
/// raw text. Answers `{"ok":true,"agent":…,"file":…}`.
async fn webhook(
    req: attach::HookRequest,
    peer: SocketAddr,
    access: &access::Access,
    hooks: &Arc<KernelHooks>,
) {
    let path = req.uri.split('?').next().unwrap_or("/").to_string();
    let Some(agent) = path
        .strip_prefix("/hook/")
        .map(|a| a.trim_matches('/').to_string())
    else {
        klog::warn("webhook_refused", None, format!("peer={peer} path={path}"));
        req.respond(
            404,
            "Not Found",
            r#"{"ok":false,"error":"POST /hook/<agent>"}"#,
        )
        .await;
        return;
    };
    let who = if access::is_local(&peer) {
        access::Identity::local()
    } else {
        let token = access::token_from_request(&req.uri, req.authorization.as_deref());
        match token.and_then(|t| access.authenticate(&t)) {
            Some(who) if who.role != access::Role::Reader => who,
            Some(_) => {
                req.respond(
                    403,
                    "Forbidden",
                    r#"{"ok":false,"error":"reader tokens cannot post"}"#,
                )
                .await;
                return;
            }
            None => {
                klog::warn(
                    "webhook_refused",
                    None,
                    format!("peer={peer} agent={agent}: bad or missing token"),
                );
                req.respond(401, "Unauthorized", r#"{"ok":false,"error":"token required: Authorization: Bearer <token> or ?token="}"#)
                    .await;
                return;
            }
        }
    };
    let text = webhook_text(&req.body, req.content_type.as_deref());
    if text.trim().is_empty() {
        req.respond(400, "Bad Request", r#"{"ok":false,"error":"empty body"}"#)
            .await;
        return;
    }
    let from = format!("webhook:{}", who.name);
    match hooks.inbox(&agent, &text, &from, Vec::new()) {
        Ok(_) => {
            klog::info(
                "webhook",
                Some(&agent),
                format!("from {} ({} bytes)", who.name, req.body.len()),
            );
            let body = serde_json::json!({"ok": true, "agent": agent, "from": from}).to_string();
            req.respond(200, "OK", &body).await;
        }
        Err(e) => {
            let body = serde_json::json!({"ok": false, "error": format!("{e:#}")}).to_string();
            req.respond(404, "Not Found", &body).await;
        }
    }
}

/// The message in a webhook body: the common text field of a JSON object
/// (Slack `text`, Discord `content`, generic `message`/`body`), with the
/// rest of the object appended compact when there is more; else the body
/// as text.
fn webhook_text(body: &[u8], content_type: Option<&str>) -> String {
    let raw = String::from_utf8_lossy(body).trim().to_string();
    let looks_json = content_type.is_some_and(|c| c.contains("json")) || raw.starts_with('{');
    if looks_json
        && let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(&raw)
    {
        for key in ["text", "content", "message", "body"] {
            if let Some(serde_json::Value::String(t)) = map.get(key)
                && !t.trim().is_empty()
            {
                let mut rest = map.clone();
                rest.remove(key);
                return if rest.is_empty() {
                    t.clone()
                } else {
                    format!(
                        "{t}\n\n[webhook fields] {}",
                        serde_json::Value::Object(rest)
                    )
                };
            }
        }
        return format!("[webhook] {}", serde_json::Value::Object(map));
    }
    raw
}

/// Every tool the kernel offers, builtins and its own. `arbos-kernel
/// prompt` builds the same set to measure what the model is sent.
pub fn kernel_registry(hooks: &Arc<KernelHooks>, ptys: &Arc<PtyHub>) -> Registry {
    Registry::builtin()
        .with(tools::Spawn(Arc::clone(hooks)))
        .with(tools::Say(Arc::clone(hooks)))
        .with(tools::PlanTool(Arc::clone(hooks)))
        .with(tools::TodoTool(Arc::clone(hooks)))
        .with(tools::Ask(Arc::clone(hooks)))
        .with(tools::StatusTool(Arc::clone(hooks)))
        .with(tools::Agents(Arc::clone(hooks)))
        .with(tools::Transcript(Arc::clone(hooks)))
        .with(tools::Browser(Arc::clone(hooks)))
        .with(crate::screenshot::Screenshot(Arc::clone(hooks)))
        .with(crate::secret_tool::Secret)
        .with(crate::tools::SubscribeTool(Arc::clone(hooks)))
        .with(crate::record::Record::default())
        .with(crate::pr_tool::Pr(Arc::clone(hooks)))
        .with(tools::Terminal {
            hooks: Arc::clone(hooks),
            ptys: Arc::clone(ptys),
        })
}

/// Greet, then run the two loops for one admitted client. A client that
/// came through the hub is served here too, on a channel instead of a
/// socket (`attach::HubChannel`).
pub async fn serve_client(
    r: attach::Reader,
    w: attach::Writer,
    who: access::Identity,
    accept_place: Place,
    accept_hooks: Arc<KernelHooks>,
    accept_frames: mpsc::UnboundedSender<Frame>,
) {
    {
        {
            let (out_tx, out_rx) = mpsc::unbounded_channel();
            accept_hooks.frames.lock().unwrap().push(out_tx.clone());
            // Greeting, snapshot, plans, then the focused agent's recent
            // transcript, so a client that cannot read the files (a phone)
            // has the conversation before the first live frame.
            let focus_agent = focus_agent(&accept_place);
            let _ = out_tx.send(Frame::Hello {
                identity: Some(arbos_core::project::identity(&accept_place)),
                store: crate::hub_link::self_store().map(|a| a.to_string()),
                protocol: PROTOCOL,
                kernel: env!("CARGO_PKG_VERSION").to_string(),
                git_sha: klog::git_sha().to_string(),
                built_at: klog::built_at().to_string(),
                binary_gone: arbos_core::binary_gone(),
                git_missing: !GIT_PRESENT.get().copied().unwrap_or(true),
                tail: ATTACH_TAIL,
                focus: focus_agent.clone(),
            });
            let _ = out_tx.send(snapshot(&accept_place, &accept_hooks));
            let _ = out_tx.send(provider_frame(&accept_place));
            for agent in list_agents(&accept_place).unwrap_or_default() {
                let _ = out_tx.send(accept_hooks.plan_frame(agent.id.as_str()));
            }
            klog::info(
                "attach_open",
                None,
                format!(
                    "clients={} who={} role={}",
                    accept_hooks.frames.lock().unwrap().len(),
                    who.name,
                    who.role.as_str()
                ),
            );
            replay(
                &accept_place,
                &focus_agent,
                Page::Tail,
                ATTACH_TAIL,
                &out_tx,
            );
            // What the user missed while no client was attached: the
            // unseen notifications, oldest first, marked as replayed.
            // Logged with the count and the range, so "no badge after
            // reopening" can be told apart from a client clearing it
            // (qal-j03): this line says the kernel sent them; a
            // `seen_marked` line after it says a client cleared them.
            let unseen = arbos_core::notify::unseen(&accept_place);
            klog::info(
                "notify_replayed",
                None,
                format!(
                    "who={} count={} ids={}..{} seen_through={}",
                    who.name,
                    unseen.len(),
                    unseen.first().map(|n| n.id).unwrap_or(0),
                    unseen.last().map(|n| n.id).unwrap_or(0),
                    arbos_core::notify::seen_through(&accept_place)
                ),
            );
            for n in unseen {
                let _ = out_tx.send(Frame::Notify {
                    id: n.id,
                    ts: n.ts,
                    agent: n.agent,
                    kind: n.kind,
                    title: n.title,
                    body: n.body,
                    replayed: true,
                });
            }
            // Questions still parked on the user: offered again, with
            // their ids, so a client that was away — or one attaching to
            // a kernel that restarted with the question open — has the
            // card to answer, not only the transcript line that asked.
            let mut asks = 0;
            for agent in list_agents(&accept_place).unwrap_or_default() {
                for w in accept_hooks.pending_asks(agent.id.as_str()) {
                    asks += 1;
                    let _ = out_tx.send(Frame::Ask {
                        agent: agent.id.to_string(),
                        question: w.question,
                        options: w.options,
                        id: Some(w.id),
                    });
                }
            }
            if asks > 0 {
                klog::info(
                    "asks_replayed",
                    None,
                    format!("who={} count={asks}", who.name),
                );
            }
            // Paths other windows hold open with unsaved edits, so this
            // one can show them held from the start.
            for f in accept_hooks.claims_held() {
                let _ = out_tx.send(f);
            }
            tokio::spawn(attach::write_loop(w, out_rx));
            // History requests are answered on this connection alone;
            // everything else goes to the kernel like before.
            let (local_tx, mut local_rx) = mpsc::unbounded_channel::<Frame>();
            let tx = accept_frames.clone();
            let place_for_history = accept_place.clone();
            let hooks_for_feedback = Arc::clone(&accept_hooks);
            let out_for_history = out_tx.clone();
            let out_for_read = out_tx;
            let who_name = who.name.clone();
            let who_owns = who.role == access::Role::Owner;
            let hooks_for_save = Arc::clone(&accept_hooks);
            // This connection's number: what its claims are held under,
            // and released with.
            let conn = CONN_N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            let hooks_for_claims = Arc::clone(&accept_hooks);
            // Screencasts this connection asked for, by agent: a flag the
            // thread reads; raised on `off` and when the connection ends.
            let mut watches: std::collections::HashMap<String, Arc<std::sync::atomic::AtomicBool>> =
                std::collections::HashMap::new();
            tokio::spawn(async move {
                while let Some(frame) = local_rx.recv().await {
                    match frame {
                        // The page as it changes, to this window alone,
                        // on a thread with its own CDP socket.
                        Frame::BrowserWatch { agent, on } => {
                            if let Some(stop) = watches.remove(&agent) {
                                stop.store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                            if !on {
                                continue;
                            }
                            if !arbos_core::agent_exists(&place_for_history, &agent) {
                                let _ = out_for_history.send(Frame::Error {
                                    agent: Some(agent.clone()),
                                    detail: format!("browser_watch: no agent is named {agent}"),
                                });
                                continue;
                            }
                            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
                            watches.insert(agent.clone(), Arc::clone(&stop));
                            let hooks = Arc::clone(&hooks_for_claims);
                            let out = out_for_history.clone();
                            let who = who_name.clone();
                            std::thread::Builder::new()
                                .name(format!("screencast-{agent}"))
                                .spawn(move || {
                                    klog::info(
                                        "browser_watch",
                                        Some(&agent),
                                        format!("who={who} on"),
                                    );
                                    if let Err(e) =
                                        hooks.browsers.screencast(&agent, out.clone(), stop)
                                    {
                                        klog::warn("browser_watch", Some(&agent), format!("{e:#}"));
                                        let _ = out.send(Frame::Error {
                                            agent: Some(agent.clone()),
                                            detail: format!("browser_watch: {e:#}"),
                                        });
                                    }
                                    klog::info(
                                        "browser_watch",
                                        Some(&agent),
                                        format!("who={who} off"),
                                    );
                                })
                                .ok();
                        }
                        // A person's input on the page, only while they
                        // drive it; the agent's own turn is refused then.
                        Frame::BrowserInput {
                            agent,
                            kind,
                            x,
                            y,
                            dx,
                            dy,
                            button,
                            count,
                            text,
                            key,
                        } => {
                            if hooks_for_claims.browsers.driver(&agent).0 != "user" {
                                let _ = out_for_history.send(Frame::Error {
                                    agent: Some(agent),
                                    detail: format!(
                                        "browser_input: {}",
                                        crate::browser::USER_NOT_DRIVING
                                    ),
                                });
                                continue;
                            }
                            let args = serde_json::json!({
                                "x": x, "y": y, "dx": dx, "dy": dy, "button": button,
                                "count": count, "text": text, "key": key,
                            });
                            let hooks = Arc::clone(&hooks_for_claims);
                            let out = out_for_history.clone();
                            tokio::task::spawn_blocking(move || {
                                if let Err(e) = hooks.browsers.input(&agent, &kind, &args) {
                                    let _ = out.send(Frame::Error {
                                        agent: Some(agent),
                                        detail: format!("browser_input {kind}: {e:#}"),
                                    });
                                }
                            });
                        }
                        // Who drives: the person takes the page or hands
                        // it back; every window hears.
                        Frame::BrowserDrive { agent, driver } => {
                            if !matches!(driver.as_str(), "user" | "agent") {
                                let _ = out_for_history.send(Frame::Error {
                                    agent: Some(agent),
                                    detail: format!(
                                        "browser_drive: driver is user or agent, not {driver:?}"
                                    ),
                                });
                                continue;
                            }
                            if hooks_for_claims.browsers.set_driver(&agent, &driver) {
                                klog::info(
                                    "browser_drive",
                                    Some(&agent),
                                    format!("who={who_name} driver={driver}"),
                                );
                            }
                            let (driver, since_ms) = hooks_for_claims.browsers.driver(&agent);
                            hooks_for_claims.broadcast(Frame::BrowserDriver {
                                agent,
                                driver,
                                by: who_name.clone(),
                                since_ms,
                            });
                        }
                        // A person's save from an editor in a window:
                        // compare-and-swap, whole; every window hears a
                        // save that landed, the asker alone a refusal.
                        Frame::Save {
                            path,
                            text,
                            base_hash,
                        } => {
                            let reply = crate::files::save(
                                &place_for_history,
                                &path,
                                &text,
                                &base_hash,
                                who_owns,
                                &who_name,
                            );
                            match &reply {
                                Frame::Saved {
                                    error: None, size, ..
                                } => {
                                    klog::info(
                                        "save",
                                        None,
                                        format!("who={who_name} path={path} bytes={size}"),
                                    );
                                    hooks_for_save.broadcast(reply);
                                }
                                Frame::Saved { error: Some(e), .. } => {
                                    klog::info(
                                        "save_refused",
                                        None,
                                        format!("who={who_name} path={path}: {e}"),
                                    );
                                    let _ = out_for_history.send(reply);
                                }
                                _ => {}
                            }
                        }
                        // A person's editor holds a path with unsaved
                        // edits (or let it go): kept per connection, told
                        // to every window; a write there is refused in
                        // the agent's own turn (side-panels handover 6).
                        Frame::Claim { path, held } => {
                            let p = std::path::Path::new(&path);
                            if hooks_for_claims.claim_key(p).is_none() {
                                let _ = out_for_history.send(Frame::Error {
                                    agent: None,
                                    detail: format!("claim: {path} is outside this place"),
                                });
                                continue;
                            }
                            if let Some(f) = hooks_for_claims.set_claim(p, held, &who_name, conn) {
                                klog::info(
                                    "claim",
                                    None,
                                    format!("who={who_name} conn={conn} held={held} path={path}"),
                                );
                                hooks_for_claims.broadcast(f);
                            }
                        }
                        Frame::History {
                            agent,
                            since,
                            before,
                            limit,
                        } => {
                            let limit = if limit == 0 {
                                ATTACH_TAIL
                            } else {
                                limit.min(HISTORY_MAX)
                            };
                            let page = match before {
                                Some(b) => Page::Before(b),
                                None => Page::After(since),
                            };
                            replay(&place_for_history, &agent, page, limit, &out_for_history);
                        }
                        // A feedback report's material: this client's
                        // own, built here so a slow disk stalls it alone.
                        Frame::ToolBody { agent, call_id } => {
                            match crate::feedback::tool_body(&place_for_history, &agent, &call_id) {
                                Some((body, truncated, size)) => {
                                    let _ = out_for_history.send(Frame::ToolBodyReply {
                                        agent,
                                        call_id,
                                        body,
                                        size,
                                        truncated,
                                    });
                                }
                                None => {
                                    let _ = out_for_history.send(Frame::Error {
                                        agent: Some(agent.clone()),
                                        detail: format!(
                                            "tool_body: no call {call_id} on {agent}'s transcript"
                                        ),
                                    });
                                }
                            }
                        }
                        Frame::Feedback {
                            agent,
                            seq,
                            call_id,
                            tail,
                            note,
                        } => {
                            if !arbos_core::agent_exists(&place_for_history, &agent) {
                                let _ = out_for_history.send(Frame::Error {
                                    agent: Some(agent.clone()),
                                    detail: format!("feedback: no agent is named {agent}"),
                                });
                                continue;
                            }
                            let host = Host::load().or_else(|_| Host::peek());
                            let Ok(host) = host else {
                                let _ = out_for_history.send(Frame::Error {
                                    agent: Some(agent.clone()),
                                    detail: "feedback: the host config could not be read".into(),
                                });
                                continue;
                            };
                            let req = crate::feedback::Request {
                                agent: &agent,
                                seq,
                                call_id: call_id.as_deref(),
                                tail,
                                note: &note,
                            };
                            let b = crate::feedback::bundle_with(
                                &place_for_history,
                                &req,
                                &host,
                                Some(&hooks_for_feedback),
                            );
                            klog::info(
                                "feedback_bundle",
                                Some(&agent),
                                format!(
                                    "seq={} call_id={} tail={tail} lines={} tail_lines={} children={} log={} bytes={} redacted={} truncated={}",
                                    seq.map(|s| s.to_string())
                                        .unwrap_or_else(|| "latest".into()),
                                    call_id.as_deref().unwrap_or("-"),
                                    b.events.len(),
                                    b.tail.len(),
                                    b.children.len(),
                                    b.log.len(),
                                    b.bytes,
                                    b.redacted,
                                    b.truncated
                                ),
                            );
                            let _ = out_for_history.send(Frame::FeedbackBundle {
                                agent,
                                turn: b.turn,
                                events: b.events,
                                tail: b.tail,
                                children: b.children,
                                log: b.log,
                                kernel: b.kernel,
                                place: b.place,
                                agents: b.agents,
                                note: b.note,
                                redacted: b.redacted,
                                truncated: b.truncated,
                                bytes: b.bytes,
                            });
                        }
                        // What each turn did to the working tree, for a
                        // files panel: the checkpoints' diff, read now on
                        // the blocking pool (the newest turn costs one
                        // `add -A` into a scratch index), answered to the
                        // asker alone.
                        Frame::TurnChanges { agent, limit } => {
                            if !arbos_core::agent_exists(&place_for_history, &agent) {
                                let _ = out_for_history.send(Frame::Error {
                                    agent: Some(agent.clone()),
                                    detail: format!("turn_changes: no agent is named {agent}"),
                                });
                                continue;
                            }
                            let place = place_for_history.clone();
                            let out = out_for_history.clone();
                            let who = who_name.clone();
                            tokio::task::spawn_blocking(move || {
                                let limit = if limit == 0 {
                                    TURN_CHANGES_DEFAULT
                                } else {
                                    limit.min(TURN_CHANGES_MAX)
                                } as usize;
                                let layout = Layout::new(&place, &agent);
                                let events = arbos_core::load_transcript(&layout.transcript())
                                    .unwrap_or_default();
                                let cwd = arbos_core::load_agent(
                                    &place,
                                    &arbos_core::AgentId::new(&agent),
                                )
                                .map(|a| a.work_dir(&place.path))
                                .unwrap_or_else(|_| place.path.clone());
                                let turns = arbos_engine::git::turn_changes(
                                    &cwd,
                                    &layout.dir,
                                    &events,
                                    limit,
                                );
                                klog::info(
                                    "turn_changes",
                                    Some(&agent),
                                    format!(
                                        "who={who} turns={} files={} unmeasured={}",
                                        turns.len(),
                                        turns.iter().map(|t| t.files.len()).sum::<usize>(),
                                        turns.iter().filter(|t| !t.unmeasured.is_empty()).count()
                                    ),
                                );
                                let _ = out.send(Frame::TurnChangeList {
                                    agent,
                                    turns,
                                    at_ms: arbos_core::now_ms(),
                                });
                            });
                        }
                        // What this kernel holds, for a window reconciling
                        // its rows after a kernel's death: answered to the
                        // asker alone, read now, never from a cache.
                        Frame::Surfaces { agent } => {
                            if let Some(a) = agent.as_deref()
                                && !arbos_core::agent_exists(&place_for_history, a)
                            {
                                let _ = out_for_history.send(Frame::Error {
                                    agent: Some(a.to_string()),
                                    detail: format!("surfaces: no agent is named {a}"),
                                });
                                continue;
                            }
                            let surfaces = crate::surfaces::list(
                                &place_for_history,
                                &hooks_for_feedback,
                                agent.as_deref(),
                            );
                            klog::info(
                                "surfaces",
                                agent.as_deref(),
                                format!(
                                    "who={who_name} rows={} running={}",
                                    surfaces.len(),
                                    surfaces.iter().filter(|s| s.running).count()
                                ),
                            );
                            let _ = out_for_history.send(Frame::SurfaceList {
                                agent,
                                surfaces,
                                at_ms: arbos_core::now_ms(),
                            });
                        }
                        // Files under .arbos/, answered here too; a slow
                        // disk stalls this client alone. `put` is a peer's
                        // write by address; the store rules apply inside.
                        f @ (Frame::Read { .. }
                        | Frame::Tail { .. }
                        | Frame::List { .. }
                        | Frame::Put { .. }) => {
                            if let Frame::Put { path, .. } = &f {
                                klog::info(
                                    "store_put",
                                    None,
                                    format!("who={who_name} path={path}"),
                                );
                            }
                            if let Some(reply) = crate::files::handle(&place_for_history, f) {
                                let _ = out_for_history.send(reply);
                            }
                        }
                        other => {
                            if tx.send(other).is_err() {
                                break;
                            }
                        }
                    }
                }
                // The connection is gone: its screencasts end, and what
                // it held is held no more.
                for stop in watches.values() {
                    stop.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                let released = hooks_for_claims.release_claims_of(conn);
                if !released.is_empty() {
                    klog::info(
                        "claims_released",
                        None,
                        format!("who={who_name} conn={conn} count={}", released.len()),
                    );
                }
                for f in released {
                    hooks_for_claims.broadcast(f);
                }
            });
            let role = who.role;
            let _ = attach::read_loop(r, role, local_tx, out_for_read).await;
            klog::info("attach_close", None, format!("who={}", who.name));
        }
    }
}

/// What this kernel can say about its model provider without saying the
/// key: which provider and model, whether a key is there, where from.
/// Why a turn cannot start on this kernel, or `None` when a model key is
/// in reach (config, environment, the place's secrets.toml, or the replay
/// provider, which asks the network nothing). The text is
/// `missing_key_hint`: what to run, set, or write, and where keys come
/// from.
/// The self-updater's expected downtime: a subscription due within it
/// holds the gate. What `/healthz` reports the gate against.
pub const UPDATE_HORIZON_MS: i64 = 10_000;

pub fn keyless(place: &Place) -> Option<String> {
    if arbos_engine::replay::current().ok().flatten().is_some() {
        return None;
    }
    let host = Host::load().or_else(|_| Host::peek()).ok()?;
    match key_source(place, &host) {
        (true, _) => None,
        (false, _) => Some(host.missing_key_hint()),
    }
}

/// Whether a key is in reach and where from, as the `provider` frame says.
fn key_source(place: &Place, host: &Host) -> (bool, String) {
    let env = host.config.key_env();
    match host.key_source() {
        arbos_core::KeySource::Config => (
            true,
            if arbos_core::host::is_overridden() {
                "memory".to_string()
            } else {
                "config".to_string()
            },
        ),
        arbos_core::KeySource::Env(var) => (true, format!("env:{var}")),
        arbos_core::KeySource::Missing(_) => {
            // secrets.toml may name it; whether it resolves shows at the turn.
            match arbos_engine::secrets::Config::load(place.path()) {
                Ok(cfg) if cfg.secrets.contains_key(&env) => (true, format!("secrets:{env}")),
                _ => (false, "none".into()),
            }
        }
    }
}

/// How far ahead the re-exec gate looks for due work (a subscription
/// about to fire is a reason to wait), and how long between attempts.
const REEXEC_HORIZON_MS: i64 = 60_000;
const REEXEC_RETRY_MS: i64 = 60_000;
/// How soon to look again when the new file was not there or was still
/// being written.
const REEXEC_LOOK_AGAIN_MS: i64 = 2_000;

/// Why a re-exec did not happen (a successful one never returns).
enum Reexec {
    /// No usable new file yet, or one whose bytes were still changing.
    NotReady,
    /// `execv` itself returned an error; the old image serves on.
    Failed,
}

/// Replace this process with the arbos-kernel now at its own path, same
/// arguments, same environment. Returns only when the exec failed — the
/// old image then serves on. Set `ARBOS_NO_REEXEC=1` to keep a kernel on
/// its old image (a test of the notice alone, or a person who wants to
/// choose the moment).
fn reexec_onto_new_binary(place: &Place, hooks: &Arc<KernelHooks>) -> Reexec {
    if std::env::var_os("ARBOS_NO_REEXEC").is_some() {
        return Reexec::NotReady;
    }
    let chosen = match crate::binary::kernel_binary() {
        Ok(c) => c,
        Err(e) => {
            klog::info(
                "reexec_wait",
                None,
                format!("no binary to restart onto yet: {e:#}; looking again"),
            );
            return Reexec::NotReady;
        }
    };
    // The file must be whole and at rest: the same size and mtime across
    // a short pause, executable, and not this process's own image.
    let settled = {
        let first = arbos_core::binary_identity::of(&chosen.path);
        std::thread::sleep(Duration::from_millis(250));
        let second = arbos_core::binary_identity::of(&chosen.path);
        match (first, second) {
            (Some(a), Some(b))
                if a == b
                    && std::fs::metadata(&chosen.path)
                        .map(|m| m.len() > 0)
                        .unwrap_or(false) =>
            {
                true
            }
            _ => false,
        }
    };
    if !settled {
        klog::info(
            "reexec_wait",
            None,
            format!(
                "{} is still being written or is not there; looking again",
                chosen.path.display()
            ),
        );
        return Reexec::NotReady;
    }
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    klog::info(
        "reexec",
        None,
        format!(
            "restarting onto {} (git {} was serving); clients reconnect",
            chosen.path.display(),
            klog::git_sha()
        ),
    );
    let _ = arbos_core::append_event(
        &Layout::new(place, arbos_core::ROOT_ID).transcript(),
        &arbos_core::Event::new(arbos_core::EventKind::Notice {
            text: format!(
                "The kernel's program file was replaced under it (an update); restarting onto the new build now, nothing in flight. Windows reconnect on their own."
            ),
            failed: false,
        }),
    );
    hooks.broadcast(Frame::Error {
        agent: None,
        detail: "kernel restarting onto its new build; reconnecting".into(),
    });
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(&chosen.path).args(&args).exec();
        klog::warn(
            "reexec_failed",
            None,
            format!("{}: {err}; the old image serves on", chosen.path.display()),
        );
    }
    Reexec::Failed
}

/// A running turn that has shown nothing for [`crate::hooks::stall_secs`] gets
/// one line on its transcript saying what it is waiting on — the tool
/// calls in flight (from the `inflight/` records) or, with none, the model
/// — and since when. A command that never returns, a model that streams
/// nothing, a wait on a child whose kernel is gone: to the user each of
/// them is the app hanging, and the working line alone cannot tell
/// "still running" from "finished and unnoticed". Said once per silence;
/// progress resets it. Not a stop: Stop stays the user's call, and the
/// line says so.
fn say_stalls(hooks: &Arc<KernelHooks>) {
    let stall_ms = crate::hooks::stall_secs() as i64 * 1000;
    for (agent, since_ms) in hooks.stalled(stall_ms) {
        let now = arbos_core::now_ms();
        let quiet = arbos_core::subscription::human_ms((now - since_ms).max(0) as u64);
        let id = arbos_core::AgentId::new(&agent);
        let running: Vec<String> = arbos_engine::inflight::peek(&hooks.place, &id)
            .iter()
            .filter(|r| r.name != "status")
            .map(|r| {
                let started = r.started.unwrap_or(since_ms);
                format!(
                    "`{}` ({}) since {}",
                    r.name,
                    arbos_core::status::derived(&r.name, r.args.as_ref()),
                    arbos_core::subscription::clock(started)
                )
            })
            .collect();
        let what = if running.is_empty() {
            format!(
                "waiting on the model, which has returned nothing since {}",
                arbos_core::subscription::clock(since_ms)
            )
        } else {
            format!("waiting on {}", running.join("; "))
        };
        let text = format!(
            "Still working, but nothing has happened for {quiet}: {what}. If it is stuck, Stop ends the turn; what ran so far stands."
        );
        klog::warn("turn_stalled", Some(&agent), &what);
        let _ = arbos_core::append_event(
            &hooks.layout(&agent).transcript(),
            &arbos_core::Event::new(arbos_core::EventKind::Notice {
                text,
                failed: false,
            }),
        );
    }
}

/// The sibling of the pin: the *model's* own context, as the provider
/// lists it, smaller than the place's standing prompt. A first install
/// on an 8k model (a small local model behind a compatible endpoint, a
/// cheap tier) met "over budget … nothing old enough to compact" on
/// every turn, naming a window and not the model. Said once per model,
/// at start, on the top-level agents' transcripts and in the log; no
/// network when there is no key or the provider is the replay script.
async fn warn_if_model_window_small(
    place: &Place,
    host: &Host,
    registry: &Arc<arbos_engine::Registry>,
) {
    if host.config.window_tokens != 0 || keyless(place).is_some() {
        return;
    }
    if arbos_engine::replay::current().ok().flatten().is_some() {
        return;
    }
    let (Some(key), Ok(base)) = (host.api_key(), host.config.api_base()) else {
        return;
    };
    let model = host.config.model();
    let Some(listed) = arbos_engine::context_window(&base, &key, &model).await else {
        return;
    };
    for agent in list_agents(place).unwrap_or_default() {
        if agent.parent.is_some() {
            continue;
        }
        let standing = arbos_engine::standing_tokens(place, &agent, registry);
        if listed >= standing.needed_window() {
            continue;
        }
        let text = format!(
            "model {model} has a {}k context, but this place's standing prompt is ~{}k tokens (system ~{}k, tools ~{}k) and needs about {}k: every turn would run over budget with nothing to compact. Choose a model with a larger context in Settings › Model (or `model` in config.toml).",
            listed / 1000,
            standing.total() / 1000,
            standing.system / 1000,
            standing.tools / 1000,
            standing.needed_window() / 1000
        );
        klog::warn("model_window_small", Some(agent.id.as_str()), &text);
        let transcript = Layout::new(place, agent.id.as_str()).transcript();
        let already = load_transcript(&transcript)
            .unwrap_or_default()
            .iter()
            .rev()
            .find_map(|e| match &e.kind {
                EventKind::Notice { text: t, .. }
                    if t.starts_with("model ")
                        && t.contains("context, but this place's standing prompt") =>
                {
                    Some(t == &text)
                }
                _ => None,
            })
            .unwrap_or(false);
        if already {
            continue;
        }
        // A warning about the configuration, not a failed turn: `failed`
        // is for what went wrong, and nothing has yet.
        let _ = arbos_core::append_event(
            &transcript,
            &arbos_core::Event::new(EventKind::Notice {
                text,
                failed: false,
            }),
        );
    }
}

/// A `window_tokens` pin in config.toml smaller than the place's own
/// standing prompt (system prefix + tool schemas, doubled for a reply and
/// some conversation) leaves every turn over budget with nothing old
/// enough to compact — thirteen notices in one turn on a 32k pin left
/// over from a small-window model (JB-4). Said once, at start, on the
/// top-level agents' transcripts and in the log; the pin is the fix.
fn warn_if_window_pinned_small(place: &Place, host: &Host, registry: &Arc<arbos_engine::Registry>) {
    let pinned = host.config.window_tokens;
    if pinned == 0 {
        return;
    }
    for agent in list_agents(place).unwrap_or_default() {
        if agent.parent.is_some() {
            continue;
        }
        let standing = arbos_engine::standing_tokens(place, &agent, registry);
        if pinned >= standing.needed_window() {
            continue;
        }
        let text = format!(
            "config.toml pins window_tokens = {pinned}, but this place's standing prompt is ~{}k tokens (system ~{}k, tools ~{}k) and needs a window of about {}k: every turn would run over budget with nothing to compact. Remove the pin (the model's own window is used) or raise it.",
            standing.total() / 1000,
            standing.system / 1000,
            standing.tools / 1000,
            standing.needed_window() / 1000
        );
        klog::warn("window_pinned_small", Some(agent.id.as_str()), &text);
        let transcript = Layout::new(place, agent.id.as_str()).transcript();
        // Once per pin, not once per boot: the last notice already saying
        // this is enough.
        let already = load_transcript(&transcript)
            .unwrap_or_default()
            .iter()
            .rev()
            .find_map(|e| match &e.kind {
                EventKind::Notice { text: t, .. }
                    if t.starts_with("config.toml pins window_tokens") =>
                {
                    Some(t == &text)
                }
                _ => None,
            })
            .unwrap_or(false);
        if !already {
            let _ = append_event(
                &transcript,
                &Event::new(EventKind::Notice { text, failed: true }),
            );
        }
    }
}

fn provider_frame(place: &Place) -> Frame {
    let host = Host::load().or_else(|_| Host::peek());
    let Ok(host) = host else {
        return Frame::Provider {
            provider: String::new(),
            model: String::new(),
            key: false,
            source: "none".into(),
        };
    };
    let (key, source) = key_source(place, &host);
    Frame::Provider {
        provider: host.config.provider().as_str().to_string(),
        model: host.config.model(),
        key,
        source,
    }
}

/// `configure`: take a provider and key from an owner. The same config
/// shape `spawn host=` writes onto a remote (`HostConfig::with_key`), saved
/// 0600 when `remember`, else held in memory for this process.
fn configure(
    place: &Place,
    provider: &str,
    api_base: &str,
    model: &str,
    api_key: &str,
    remember: bool,
) -> Result<Frame> {
    let key = api_key.trim();
    if key.len() < 16 {
        anyhow::bail!("that is not a key (too short)");
    }
    let mut host = Host::load().or_else(|_| Host::peek())?;
    let kind = match provider.trim().to_ascii_lowercase().as_str() {
        "" | "openrouter" => arbos_core::ProviderKind::OpenRouter,
        "openai" => arbos_core::ProviderKind::OpenAi,
        "custom" => arbos_core::ProviderKind::Custom,
        other => anyhow::bail!("unknown provider {other:?} (openrouter, openai, custom)"),
    };
    if host.config.provider() != kind {
        host.config.set_provider(kind);
    } else {
        host.config.provider = Some(kind);
    }
    if !api_base.trim().is_empty() {
        host.config.api_base = api_base.trim().to_string();
    }
    if !model.trim().is_empty() {
        host.config.model = model.trim().to_string();
    }
    let cfg = host.config.with_key(key);
    // Tool output redacts it from now on, like the key the kernel started with.
    arbos_engine::secrets::store().protect("MODEL_API_KEY", key.to_string());
    if remember {
        arbos_core::host::set_override(None);
        let saved = Host {
            config: cfg,
            dir: host.dir.clone(),
        };
        saved.save()?;
        klog::info(
            "configured",
            None,
            format!(
                "provider={} model={} remembered=true",
                kind.as_str(),
                saved.config.model()
            ),
        );
    } else {
        klog::info(
            "configured",
            None,
            format!(
                "provider={} model={} remembered=false",
                kind.as_str(),
                cfg.model()
            ),
        );
        arbos_core::host::set_override(Some(cfg));
    }
    let base = Host::load()?;
    if let (Some(key), Ok(url)) = (base.api_key(), base.config.api_base()) {
        tokio::spawn(async move {
            arbos_engine::warm(&url, &key).await;
        });
    }
    Ok(provider_frame(place))
}

/// `rewind` from an attached client. The agent must be idle. The cut is
/// done here (fast: two file writes); the tail is moved to the new end;
/// files are restored on the blocking pool, and `rewound` goes out to
/// every client when that is done.
/// A setting the client asked for that did not reach the agent's file:
/// the window shows it set, the next start would not — said as an
/// error frame instead of believed (the unchecked-write pass).
fn say_if_unsaved(hooks: &KernelHooks, agent: &str, what: &str, saved: anyhow::Result<()>) {
    if let Err(e) = saved {
        klog::warn("agent_save_failed", Some(agent), format!("{what}: {e:#}"));
        hooks.broadcast(Frame::Error {
            agent: Some(agent.to_string()),
            detail: format!(
                "{what} changed for this run only: the agent's file could not be written ({e:#}); it would revert at the next start"
            ),
        });
    }
}

/// The `reason` on a `stop` that replaces a message rather than ending work.
pub const SUPERSEDED: &str = "superseded";

/// A superseded turn's lines, cut when the turn had done nothing a
/// reader would miss: its wake, the half-said user line, thinking, and
/// the interrupted/turn_complete close. A turn that had already spoken
/// or run a tool keeps its lines — the `interrupted` line says
/// `superseded`, and a client may draw that softly or not at all, but
/// the record of what ran stands. The cut lines go to the rewind
/// archive like any rewind's, so what was heard is not lost, only out
/// of the chat.
fn supersede_cut(
    place: &Place,
    hooks: &Arc<KernelHooks>,
    tails: &mut std::collections::HashMap<String, TranscriptTail>,
    agent: &str,
    lo: u64,
) {
    let events = load_transcript(&Layout::new(place, agent).transcript()).unwrap_or_default();
    let span = events.iter().filter(|e| e.seq >= lo);
    let quiet = span.clone().count() > 0
        && span.clone().all(|e| {
            matches!(
                e.kind,
                EventKind::Wake { .. }
                    | EventKind::User { .. }
                    | EventKind::Thinking { .. }
                    | EventKind::Interrupted { .. }
                    | EventKind::TurnComplete { .. }
                    | EventKind::Notice { failed: false, .. }
            )
        });
    if !quiet {
        klog::info(
            "turn_superseded",
            Some(agent),
            format!(
                "kept: the turn from line {lo} had spoken or run a tool before the fuller message came"
            ),
        );
        return;
    }
    let (dropped, archive) = match rewind::cut_from_line(place, agent, lo) {
        Ok(c) => c,
        Err(e) => {
            klog::warn("turn_superseded", Some(agent), format!("kept: {e:#}"));
            return;
        }
    };
    // The tail's next read starts where the file now ends, so nothing of
    // what remains is replayed; windows drop the cut lines.
    let mut fresh = TranscriptTail::default();
    let _ = fresh.read_new(&Layout::new(place, agent).transcript());
    tails.insert(agent.to_string(), fresh);
    hooks.broadcast(Frame::Rewound {
        agent: agent.to_string(),
        line: lo,
        dropped,
        restored: None,
        pending: false,
    });
    klog::info(
        "turn_superseded",
        Some(agent),
        format!(
            "cut: the turn from line {lo} had done nothing yet; {dropped} line(s) to {}; one utterance, one line",
            archive.display()
        ),
    );
}

fn rewind_live(
    place: &Place,
    hooks: &Arc<KernelHooks>,
    tails: &mut std::collections::HashMap<String, TranscriptTail>,
    agent: &str,
    target: rewind::Target,
    files: bool,
) {
    let refuse = |detail: String| {
        klog::warn("rewind_refused", Some(agent), &detail);
        hooks.broadcast(Frame::Error {
            agent: Some(agent.to_string()),
            detail,
        });
    };
    if hooks.is_running(agent) {
        return refuse("rewind: the agent is running; stop the turn first".into());
    }
    let done = match rewind::cut(place, agent, target) {
        Ok(c) => c,
        Err(e) => return refuse(format!("rewind: {e:#}")),
    };
    // The tail's next read starts where the file now ends, so nothing of
    // what remains is replayed.
    let mut fresh = TranscriptTail::default();
    let _ = fresh.read_new(&Layout::new(place, agent).transcript());
    tails.insert(agent.to_string(), fresh);
    klog::info(
        "rewind",
        Some(agent),
        format!(
            "target={target:?} line={} dropped={} files={files} archive={}",
            done.checkpoint.line,
            done.dropped,
            done.archive.display()
        ),
    );
    // The transcript is cut now: say so now. Windows reload from the
    // shorter file at once instead of after the file restore below, which
    // runs git and took seconds on a large store (the layout pass needed a
    // 6 s settle). A second `rewound` follows with what was restored.
    hooks.broadcast(Frame::Rewound {
        agent: agent.to_string(),
        line: done.checkpoint.line,
        dropped: done.dropped,
        restored: None,
        pending: files,
    });
    hooks.broadcast(hooks.plan_frame(agent));
    if !files {
        return;
    }
    let hooks = Arc::clone(hooks);
    let place = place.clone();
    let agent = agent.to_string();
    tokio::task::spawn_blocking(move || {
        let restored = if files {
            match rewind::restore_files(&place, &agent, &done.checkpoint) {
                Ok(what) => Some(what),
                Err(e) if e.downcast_ref::<arbos_engine::git::NoTree>().is_some() => {
                    // Not a failure: the record for this turn has no tree
                    // (recorded before the kernel kept one, or its save
                    // failed and was said at the time). The transcript is
                    // cut; the files stand; and on Jacob's existing places
                    // most old-turn rewinds land here. A notice, drawn as
                    // a kernel line, that also says when it stops — not
                    // an `error` frame that draws the rewind as a crash.
                    let text = format!(
                        "Rewound the transcript. Files were not restored: {e}. This turn was recorded before the kernel kept each turn's working tree; turns recorded from now on restore their files."
                    );
                    let _ = arbos_core::append_event(
                        &Layout::new(&place, &agent).transcript(),
                        &arbos_core::Event::new(arbos_core::EventKind::Notice {
                            text,
                            failed: false,
                        }),
                    );
                    klog::info("rewind_files_skipped", Some(&agent), format!("{e}"));
                    hooks.broadcast(Frame::Rewound {
                        agent: agent.clone(),
                        line: done.checkpoint.line,
                        dropped: done.dropped,
                        restored: None,
                        pending: false,
                    });
                    None
                }
                Err(e) => {
                    hooks.broadcast(Frame::Error {
                        agent: Some(agent.clone()),
                        detail: format!("rewind: transcript cut, files not restored: {e:#}"),
                    });
                    None
                }
            }
        } else {
            None
        };
        if restored.is_some() {
            hooks.broadcast(Frame::Rewound {
                agent: agent.clone(),
                line: done.checkpoint.line,
                dropped: done.dropped,
                restored,
                pending: false,
            });
        }
    });
}

/// The pending approval for `agent` gets its verdict: the waiting tool
/// call goes on (or fails), the parked file goes, and the decision is on
/// the transcript so the record shows what was allowed or denied, not
/// just a failed tool.
fn resolve_approve(
    hooks: &KernelHooks,
    place: &Place,
    agent: String,
    call_id: String,
    allow: bool,
) {
    let pending = hooks.approves.lock().unwrap().remove(&agent);
    if let Some((_, id, _)) = &pending {
        arbos_core::waiting::remove(place, &agent, "approve", id);
    }
    let tool = pending
        .as_ref()
        .map(|(tool, _, _)| tool.clone())
        .unwrap_or_else(|| "bash".into());
    let call_id = pending
        .as_ref()
        .map(|(_, id, _)| id.clone())
        .unwrap_or(call_id);
    if let Some((_, _, tx)) = pending {
        let _ = tx.send(allow);
    }
    let event = Event::new(EventKind::Approval {
        call_id,
        tool,
        allowed: allow,
    });
    // The decision drove the tool whether or not this line lands; a
    // record without it would show a tool that ran with no one's say-so.
    if let Err(e) = append_event(&Layout::new(place, &agent).transcript(), &event) {
        klog::warn(
            "approval_unrecorded",
            Some(&agent),
            format!("allowed={allow}: the decision could not be written to the transcript: {e:#}"),
        );
    }
    hooks.broadcast(Frame::Event { agent, event });
}

/// `<agent> turn L<line>: <the last words>` — what the .arbos/ commit for a
/// finished turn says.
fn turn_commit_message(place: &Place, agent: &str) -> String {
    let events = load_transcript(&Layout::new(place, agent).transcript()).unwrap_or_default();
    let line = events.len();
    let last = events
        .iter()
        .rev()
        .find_map(|e| match &e.kind {
            EventKind::Assistant { text, .. } if !text.trim().is_empty() => {
                Some(text.lines().next().unwrap_or("").trim().to_string())
            }
            EventKind::Interrupted { detail } => Some(format!("interrupted: {detail}")),
            EventKind::Notice { text, failed: true } => Some(format!("failed: {text}")),
            _ => None,
        })
        .unwrap_or_default();
    let last: String = last.chars().take(120).collect();
    format!("{agent} turn L{line}: {last}")
}

/// This machine's name, for a Try Live frame's `machine`.
fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "this machine".into())
}

/// An attachment path as the client wrote it: an absolute path (the
/// desktop) as is; a relative one that names a file under `.arbos/` (a
/// `put` from the phone: `attachments/<id>.jpg`, with or without the
/// `.arbos/` head) as that file's absolute path; anything else as is
/// (the CLI's paths relative to the cwd).
/// Whether an attachment path names a file this kernel can read: as
/// written when absolute, else relative to the agent's cwd (the CLI's
/// shape). Store-relative paths were made absolute already.
fn attachment_present(agent: &str, place: &Place, a: &str) -> bool {
    let p = std::path::Path::new(a);
    if a.trim().is_empty() {
        return false;
    }
    if p.is_absolute() {
        return p.is_file();
    }
    let cwd = load_agent(place, &arbos_core::AgentId::new(agent))
        .map(|ag| ag.work_dir(place.path()))
        .unwrap_or_else(|_| place.path().to_path_buf());
    cwd.join(p).is_file()
}

pub fn store_attachment(place: &Place, a: &str) -> String {
    let p = std::path::Path::new(a);
    if p.is_absolute() || a.trim().is_empty() {
        return a.to_string();
    }
    let rel = a.trim_start_matches("./");
    let rel = rel.strip_prefix(".arbos/").unwrap_or(rel);
    if rel.contains("..") {
        return a.to_string();
    }
    let under_store = place.arbos().join(rel);
    if under_store.is_file() {
        return under_store.display().to_string();
    }
    a.to_string()
}
