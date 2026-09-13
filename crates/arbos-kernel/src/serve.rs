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
    version: String,
    git_sha: String,
    log: String,
}

pub async fn run(place_path: impl Into<std::path::PathBuf>) -> Result<i32> {
    let place = Place::new(
        std::fs::canonicalize(place_path.into())
            .unwrap_or_else(|_| std::env::current_dir().unwrap()),
    );
    let _lock = PlaceLock::acquire(&place)?;
    bootstrap(&place)?;
    klog::init(klog::log_path_for(&place.arbos()));
    let host = Host::load()?;
    host.remember_place(place.path());
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
    write_kernel_json(&place, addr, open, &access)?;
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
    let mut registry = kernel_registry(&hooks, &ptys);
    // MCP: every tool of every configured server (`.arbos/mcp.toml`,
    // `.cursor/mcp.json`, `~/.config/arbos/mcp.toml`, `ARBOS_MCP_CMD`)
    // joins the registry as `mcp__<server>__<tool>`, callable like any
    // builtin. A server that fails to answer is skipped, not fatal.
    // Discovery talks to processes and HTTP endpoints (blocking clients),
    // so it runs off the async thread.
    let discovered = {
        let place = place.clone();
        tokio::task::spawn_blocking(move || {
            crate::mcp::load_servers(&place)
                .into_iter()
                .map(|server| {
                    let specs = server.tools();
                    (Arc::new(server), specs)
                })
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default()
    };
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

    // A dead kernel's half-run nodes go back to pending. Then continue
    // anyone whose last turn never ended.
    for line in crate::migrate::run(&hooks) {
        crate::klog::info("migrated", None, line);
    }
    plan::reclaim(&hooks);
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
    // `changed` frames for attached clients: a stat pass once a second.
    let mut watch = crate::watch::Watch::default();
    let mut watch_tick = interval(Duration::from_secs(1));
    let mut exit_code = 0;
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
    // appended since the last one.
    let mut tails: std::collections::HashMap<String, TranscriptTail> =
        std::collections::HashMap::new();
    shutdown_backstop(place.lock_path());
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
        // Notes waiting in the inbox (wake = false) go on the transcript
        // before the model reads it, whatever started this turn.
        hooks.take_notes(&id);
        hooks.turn_started(&id);
        hooks.broadcast(Frame::Turn {
            agent: id,
            state: "running".into(),
            budget: None,
        });
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
                for wake in plan::scan(&hooks) {
                    start(wake);
                }
            }
            Some(id) = done_rx.recv() => {
                let control = sched.in_flight.lock().unwrap().remove(&id);
                hooks.turn_ended(&id);
                plan::finish_turn(&hooks, &id);
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
                if let Frame::Rewind { agent, turn, files } = frame {
                    rewind_live(&place, &hooks, &mut tails, &agent, turn, files);
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
                hooks.kick();
                hooks.broadcast(tree_frame(&place));
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
                    // under the chat until it ends.
                    for job in jobs.list() {
                        let key = format!("{}/{}", agent.id, job.id);
                        if !job.detached() || !job.running() || announced.contains(&key) {
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
                        });
                    }
                    for job in jobs.list() {
                        if !job.detached() {
                            continue;
                        }
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
                    let tail = tails.entry(agent.id.to_string()).or_default();
                    for ev in tail.read_new(&path).unwrap_or_default() {
                        if record_prs(&place, agent.id.as_str(), &ev) {
                            hooks.broadcast(Frame::Tree { tree: tree_nodes(&place) });
                        }
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
                stop_turns(&sched, &hooks, &mut done_rx).await;
                break;
            }
            _ = sigterm.recv() => {
                println!("arbos-kernel stopping");
                klog::info("kernel_stop", None, "signal");
                stop_turns(&sched, &hooks, &mut done_rx).await;
                break;
            }
        }
    }
    Ok(exit_code)
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
    hooks: &KernelHooks,
    sched: &Scheduler,
    ptys: &PtyHub,
) {
    // Frames that name an agent must name one the kernel lists. Writing
    // for an unknown id minted a folder with a transcript and no agent.md.
    let names = match &frame {
        Frame::User { agent, .. }
        | Frame::Pause { agent, .. }
        | Frame::Stop { agent }
        | Frame::Compact { agent }
        | Frame::Answer { agent, .. }
        | Frame::Approve { agent, .. }
        | Frame::Undo { agent }
        | Frame::SetModel { agent, .. }
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
                let _ = a.save(&place.agent_dir(&agent));
            }
            if paused {
                sched.stop(&agent);
            } else {
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
        Frame::Stop { agent } => {
            // Stop means all of it: the turn, the standing work, the
            // children. A running turn ends; scheduled nodes block until
            // someone presses run.
            for id in hooks.stop_work(&agent) {
                sched.stop(&id);
            }
        }
        Frame::Compact { agent } => {
            if !sched.request_compact(&agent) {
                let _ = wakes.send(Wake::compact(&agent));
            }
        }
        Frame::Answer { agent, text, id } => {
            // Only a question that is pending may resolve; a late or
            // duplicate answer (the same ask arriving twice through the live
            // frame and the transcript tail, qa-021) is refused, not applied.
            let pending: Vec<String> = hooks
                .pending_asks(&agent)
                .into_iter()
                .map(|w| w.id)
                .collect();
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
            // The decision is part of the record: the transcript shows what
            // was allowed or denied, not just a failed tool.
            let event = Event::new(EventKind::Approval {
                call_id,
                tool,
                allowed: allow,
            });
            let _ = append_event(&Layout::new(place, &agent).transcript(), &event);
            hooks.broadcast(Frame::Event { agent, event });
        }
        Frame::Undo { agent } => {
            let cwd = load_agent(place, &arbos_core::AgentId::new(&agent))
                .ok()
                .and_then(|a| a.cwd)
                .unwrap_or_else(|| place.path.clone());
            let _ = arbos_engine::git::undo(&cwd);
        }
        Frame::SetModel { agent, model } => {
            if let Ok(mut a) = load_agent(place, &arbos_core::AgentId::new(&agent)) {
                a.model = model;
                let _ = a.save(&place.agent_dir(&agent));
            }
        }
        Frame::SetMode { agent, mode } => {
            let Some(mode) = arbos_core::Mode::parse(&mode) else {
                eprintln!("set mode {agent}: unknown mode {mode:?}");
                return;
            };
            if let Ok(mut a) = load_agent(place, &arbos_core::AgentId::new(&agent)) {
                a.mode = mode;
                let _ = a.save(&place.agent_dir(&agent));
                // On the record, so the transcript says when the leash changed.
                let _ = append_event(
                    &Layout::new(place, &agent).transcript(),
                    &Event::new(EventKind::Notice {
                        text: format!("mode: {} — {}", mode.as_str(), mode.describe()),
                        failed: false,
                    }),
                );
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
        Frame::PtyIn { agent, page, data } => {
            if let Ok(bytes) = Engine::decode(&base64::engine::general_purpose::STANDARD, data) {
                let _ = ptys.write(&agent, &page, &bytes);
            }
        }
        _ => {}
    }
}

/// Ctrl-C must end the process even when the serve loop is busy. The loop
/// handles the signal itself and exits cleanly; this task is the backstop:
/// if the loop has not returned a few seconds later, drop the lock file
/// (the `PlaceLock` guard would have) and exit, rather than leave a kernel
/// the user cannot stop.
fn shutdown_backstop(lock_path: std::path::PathBuf) {
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
        let _ = std::fs::remove_file(&lock_path);
        std::process::exit(130);
    });
}

/// What this kernel speaks on the attach socket.
pub const PROTOCOL: u32 = 1;
/// Transcript lines replayed on attach for the focused agent.
const ATTACH_TAIL: u32 = 200;
/// Most lines one `history` request returns.
const HISTORY_MAX: u32 = 2000;

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
fn replay(
    place: &Place,
    agent: &str,
    since: Option<u64>,
    limit: u32,
    out: &mpsc::UnboundedSender<Frame>,
) {
    let events = load_transcript(&Layout::new(place, agent).transcript()).unwrap_or_default();
    let total = events.len() as u64;
    let picked: Vec<&Event> = match since {
        None => {
            let skip = events.len().saturating_sub(limit as usize);
            events[skip..].iter().collect()
        }
        Some(since) => events
            .iter()
            .filter(|e| e.seq > since)
            .take(limit as usize)
            .collect(),
    };
    let from = picked.first().map(|e| e.seq).unwrap_or(since.unwrap_or(0));
    let to = picked.last().map(|e| e.seq).unwrap_or(since.unwrap_or(0));
    for ev in picked {
        let _ = out.send(Frame::Replayed {
            agent: agent.to_string(),
            event: ev.clone(),
        });
    }
    let _ = out.send(Frame::HistoryEnd {
        agent: agent.to_string(),
        from,
        to,
        total,
    });
}

fn snapshot(place: &Place) -> Frame {
    let focus = arbos_core::read_focus(place);
    // The focused agent's last measured context, so a client attaching
    // mid-conversation shows the real meter rather than a placeholder.
    let agent = focus.rsplit('/').next().unwrap_or("root").to_string();
    Frame::Snapshot {
        tree: tree_nodes(place),
        focus,
        budget: last_usage(place, &agent),
    }
}

/// The usage the agent's last completed turn reported, if any.
fn last_usage(place: &Place, agent: &str) -> Option<Usage> {
    let events = load_transcript(&Layout::new(place, agent).transcript()).ok()?;
    events.iter().rev().find_map(|e| match &e.kind {
        EventKind::TurnComplete { usage } => *usage,
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
            parent: a.parent.as_ref().map(|p| p.to_string()),
            paused: a.paused,
            model: a.model.clone(),
            kind: "agent".into(),
            mode: a.mode.as_str().into(),
            prs: arbos_core::prs::prs_of_tree(&prs, a.id.as_str(), &agents).len() as u32,
        })
        .collect()
}

/// A finished `bash` whose command ran `gh pr create` and whose output
/// names the pull request: one record per new URL in `.arbos/prs.jsonl`.
/// Returns whether anything new was recorded.
fn record_prs(place: &Place, agent: &str, ev: &Event) -> bool {
    let EventKind::Tool(rec) = &ev.kind else {
        return false;
    };
    if !matches!(rec.name.as_str(), "bash" | "terminal") || rec.error.is_some() {
        return false;
    }
    let Some(command) = rec
        .args
        .as_ref()
        .and_then(|a| a.get("command"))
        .and_then(|c| c.as_str())
    else {
        return false;
    };
    if !arbos_core::prs::opens_pr(command) {
        return false;
    }
    let Some(body) = rec.body.as_deref() else {
        return false;
    };
    let branch = arbos_core::prs::head_branch(command);
    let mut new = false;
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
            Ok(true) => new = true,
            Ok(false) => {}
            Err(e) => eprintln!("prs: {e:#}"),
        }
    }
    new
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
) -> Result<()> {
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
        attach::Conn::Tcp(_) | attach::Conn::Hook(_) => None,
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
        .with(tools::Ask(Arc::clone(hooks)))
        .with(tools::Browser(Arc::clone(hooks)))
        .with(crate::screenshot::Screenshot)
        .with(crate::secret_tool::Secret)
        .with(crate::tools::SubscribeTool(Arc::clone(hooks)))
        .with(crate::record::Record::default())
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
                protocol: PROTOCOL,
                kernel: env!("CARGO_PKG_VERSION").to_string(),
                tail: ATTACH_TAIL,
                focus: focus_agent.clone(),
            });
            let _ = out_tx.send(snapshot(&accept_place));
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
            replay(&accept_place, &focus_agent, None, ATTACH_TAIL, &out_tx);
            tokio::spawn(attach::write_loop(w, out_rx));
            // History requests are answered on this connection alone;
            // everything else goes to the kernel like before.
            let (local_tx, mut local_rx) = mpsc::unbounded_channel::<Frame>();
            let tx = accept_frames.clone();
            let place_for_history = accept_place.clone();
            let out_for_history = out_tx.clone();
            let out_for_read = out_tx;
            tokio::spawn(async move {
                while let Some(frame) = local_rx.recv().await {
                    match frame {
                        Frame::History {
                            agent,
                            since,
                            limit,
                        } => {
                            let limit = if limit == 0 {
                                ATTACH_TAIL
                            } else {
                                limit.min(HISTORY_MAX)
                            };
                            replay(
                                &place_for_history,
                                &agent,
                                Some(since),
                                limit,
                                &out_for_history,
                            );
                        }
                        // Files under .arbos/, answered here too; a slow
                        // disk stalls this client alone.
                        f @ (Frame::Read { .. } | Frame::Tail { .. } | Frame::List { .. }) => {
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
            });
            let role = who.role;
            let _ = attach::read_loop(r, role, local_tx, out_for_read).await;
            klog::info("attach_close", None, format!("who={}", who.name));
        }
    }
}

/// What this kernel can say about its model provider without saying the
/// key: which provider and model, whether a key is there, where from.
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
    let env = host.config.key_env();
    let (key, source) = match host.key_source() {
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
    };
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
fn rewind_live(
    place: &Place,
    hooks: &Arc<KernelHooks>,
    tails: &mut std::collections::HashMap<String, TranscriptTail>,
    agent: &str,
    turn: u32,
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
    let done = match rewind::cut(place, agent, rewind::Target::Turn(turn)) {
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
            "turn={turn} line={} dropped={} files={files} archive={}",
            done.checkpoint.line,
            done.dropped,
            done.archive.display()
        ),
    );
    let hooks = Arc::clone(hooks);
    let place = place.clone();
    let agent = agent.to_string();
    tokio::task::spawn_blocking(move || {
        let restored = if files {
            match rewind::restore_files(&place, &agent, &done.checkpoint) {
                Ok(what) => Some(what),
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
        hooks.broadcast(Frame::Rewound {
            agent: agent.clone(),
            line: done.checkpoint.line,
            dropped: done.dropped,
            restored,
        });
        hooks.broadcast(hooks.plan_frame(&agent));
    });
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
