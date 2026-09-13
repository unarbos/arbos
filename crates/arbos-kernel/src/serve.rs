use anyhow::{Context, Result};
use arbos_core::{
    Event, EventKind, Place, PlaceLock, TranscriptTail, Usage, Wake, WakeKind, append_event,
    bootstrap, files::Layout, list_agents, load_agent, load_transcript, needs_serve, write_focus,
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
    attach::{self, Frame, TreeNode},
    doors,
    grep::PlaceGrep,
    hooks::KernelHooks,
    plan,
    pty::PtyHub,
    sched::Scheduler,
    tools,
};

#[derive(Serialize)]
struct KernelJson {
    url: String,
    pid: u32,
}

pub async fn run(place_path: impl Into<std::path::PathBuf>) -> Result<()> {
    let place = Place::new(
        std::fs::canonicalize(place_path.into())
            .unwrap_or_else(|_| std::env::current_dir().unwrap()),
    );
    let _lock = PlaceLock::acquire(&place)?;
    bootstrap(&place)?;
    let host = Host::load()?;
    host.remember_place(place.path());
    match (host.api_key(), host.config.api_base()) {
        (Some(key), Ok(base)) => {
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

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("bind attach")?;
    let addr = listener.local_addr()?;
    write_kernel_json(&place, addr)?;

    let (wake_tx, mut wake_rx) = mpsc::unbounded_channel::<Wake>();
    let (kick_tx, mut kick_rx) = mpsc::unbounded_channel::<()>();
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<String>();
    let (frame_in_tx, mut frame_in_rx) = mpsc::unbounded_channel::<Frame>();

    let hooks = KernelHooks::new(place.clone(), wake_tx.clone(), kick_tx.clone());
    let sched = Scheduler::sharing(Arc::clone(&hooks.in_flight));
    let clock = plan::Clock::new();
    let ptys = Arc::new(PtyHub::new());
    let (pty_tx, mut pty_rx) = mpsc::unbounded_channel::<Frame>();
    ptys.bind(place.path.clone(), pty_tx);
    let mut registry = Registry::builtin()
        .with(tools::Spawn(Arc::clone(&hooks)))
        .with(tools::Say(Arc::clone(&hooks)))
        .with(tools::PlanTool(Arc::clone(&hooks)))
        .with(tools::Ask(Arc::clone(&hooks)))
        .with(tools::Browser(Arc::clone(&hooks)))
        .with(tools::Terminal {
            hooks: Arc::clone(&hooks),
            ptys: Arc::clone(&ptys),
        });
    // MCP: every tool the env-configured server offers joins the registry
    // as `mcp__<name>`, callable like any builtin.
    if let Some(server) = doors::McpServer::from_env() {
        let server = Arc::new(server);
        match server.tools() {
            Ok(specs) => {
                eprintln!(
                    "mcp: {} offers {}",
                    server.cmd,
                    specs
                        .iter()
                        .map(|s| s.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                for spec in specs {
                    registry = registry.with(tools::McpTool::new(Arc::clone(&server), spec));
                }
            }
            Err(err) => eprintln!("mcp: {err:#}"),
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

    // A dead kernel's half-run nodes go back to pending. Then continue
    // anyone whose last turn never ended.
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
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let (r, w) = stream.into_split();
            let (out_tx, out_rx) = mpsc::unbounded_channel();
            accept_hooks.frames.lock().unwrap().push(out_tx.clone());
            let _ = out_tx.send(snapshot(&accept_place));
            for agent in list_agents(&accept_place).unwrap_or_default() {
                let _ = out_tx.send(accept_hooks.plan_frame(agent.id.as_str()));
            }
            tokio::spawn(attach::write_loop(w, out_rx));
            let tx = accept_frames.clone();
            tokio::spawn(async move {
                let _ = attach::read_loop(r, tx).await;
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
    // One incremental reader per agent. Each poll reads only what was
    // appended since the last one.
    let mut tails: std::collections::HashMap<String, TranscriptTail> =
        std::collections::HashMap::new();
    shutdown_backstop(place.lock_path());
    // Detached jobs the desktop has been told about, as `agent/jN`. The
    // row opens once, and closes when the job finishes.
    let mut announced: std::collections::HashSet<String> = std::collections::HashSet::new();
    println!("arbos-kernel serve {} at {}", place.path.display(), addr);

    // Start one turn. The wake came from the plan (claimed) or from the
    // kernel's own housekeeping (`Serve`, `Compact`).
    let start = |wake: Wake| {
        let paused = load_agent(&place, &wake.agent).is_ok_and(|a| a.paused);
        if paused || sched.has_job(wake.agent.as_str()) {
            // Housekeeping on a busy agent: compact is requested in-turn by
            // handle_frame; serve is moot. A plan wake that lands here lost
            // a race; its node goes back to pending.
            if wake.node.is_some() {
                plan::abandon(&hooks, &clock, wake.agent.as_str());
            }
            return;
        }
        let id = wake.agent.to_string();
        hooks.turn_started(&id);
        hooks.broadcast(Frame::Turn {
            agent: id,
            state: "running".into(),
            budget: None,
        });
        sched.start(
            place.clone(),
            wake,
            host.clone(),
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
                for wake in plan::scan(&hooks, &clock) {
                    start(wake);
                }
            }
            Some(id) = done_rx.recv() => {
                let control = sched.in_flight.lock().unwrap().remove(&id);
                hooks.turn_ended(&id);
                plan::finish_turn(&hooks, &clock, &id);
                // Said to a running agent, but its turn ended before the
                // next tool boundary: each one becomes a turn of its own.
                if let Some(control) = control {
                    let left = control.take_steers();
                    if !left.is_empty() {
                        hooks.requeue_steers(&id, left);
                    }
                }
                hooks.broadcast(Frame::Turn {
                    agent: id.clone(),
                    state: "idle".into(),
                    budget: last_usage(&place, &id),
                });
                hooks.broadcast(tree_frame(&place));
                hooks.kick();
            }
            Some(frame) = frame_in_rx.recv() => {
                handle_frame(
                    &place,
                    frame,
                    &wake_tx,
                    &hooks,
                    &sched,
                    &ptys,
                );
            }
            _ = tick.tick() => {
                hooks.kick();
                hooks.broadcast(tree_frame(&place));
            }
            _ = tail.tick() => {
                for agent in list_agents(&place).unwrap_or_default() {
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
                    for job in jobs.sweep() {
                        announced.remove(&format!("{}/{}", agent.id, job.id));
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
                        let _ = append_event(
                            &Layout::new(&place, agent.id.as_str()).transcript(),
                            &Event::new(EventKind::Notice { text: text.clone(), failed: false }),
                        );
                        if !sched.has_job(agent.id.as_str()) && !agent.paused {
                            let mut w = Wake::new(agent.id.as_str(), WakeKind::Job, Some(text));
                            w.node = None;
                            let _ = wake_tx.send(w);
                        }
                    }
                    let path = Layout::new(&place, agent.id.as_str()).transcript();
                    let tail = tails.entry(agent.id.to_string()).or_default();
                    for ev in tail.read_new(&path).unwrap_or_default() {
                        hooks.broadcast(Frame::Event {
                            agent: agent.id.to_string(),
                            event: ev,
                        });
                    }
                }
            }
            _ = sigint.recv() => {
                println!("arbos-kernel stopping");
                break;
            }
            _ = sigterm.recv() => {
                println!("arbos-kernel stopping");
                break;
            }
        }
    }
    Ok(())
}

fn handle_frame(
    place: &Place,
    frame: Frame,
    wakes: &mpsc::UnboundedSender<Wake>,
    hooks: &KernelHooks,
    sched: &Scheduler,
    ptys: &PtyHub,
) {
    match frame {
        Frame::User {
            agent,
            text,
            steer,
            attachments,
        } => {
            // A steer goes into the live turn at its next tool boundary.
            // Everything else is a node: it fires now if the agent is idle,
            // else after the current turn — and survives a restart either way.
            if steer && sched.has_job(&agent) {
                sched.steer(&agent, text);
                return;
            }
            let mut n = arbos_core::Node::inbox(text, "user");
            n.attachments = attachments;
            if let Err(e) = hooks.inbox(&agent, n) {
                eprintln!("inbox {agent}: {e:#}");
            }
        }
        Frame::PlanOp {
            agent,
            node,
            op,
            text,
        } => {
            if let Err(e) = hooks.plan_op(&agent, node, &op, &text) {
                eprintln!("plan op {op} {agent}#{node}: {e:#}");
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
            let _ = write_focus(place, &path);
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
        Frame::Answer { agent, text } => {
            let _ = append_event(
                &Layout::new(place, &agent).transcript(),
                &Event::new(EventKind::Answer { text: text.clone() }),
            );
            if let Some(tx) = hooks.asks.lock().unwrap().remove(&agent) {
                let _ = tx.send(text);
            }
        }
        Frame::Approve {
            agent,
            call_id,
            allow,
        } => {
            let key = format!("{agent}:bash");
            if let Some(tx) = hooks.approves.lock().unwrap().remove(&key) {
                let _ = tx.send(allow);
            }
            // The decision is part of the record: the transcript shows what
            // was allowed or denied, not just a failed tool.
            let event = Event::new(EventKind::Approval {
                call_id,
                tool: "bash".into(),
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

fn tree_nodes(place: &Place) -> Vec<TreeNode> {
    list_agents(place)
        .unwrap_or_default()
        .into_iter()
        .map(|a| TreeNode {
            id: a.id.to_string(),
            name: a.name,
            parent: a.parent.map(|p| p.to_string()),
            paused: a.paused,
            model: a.model,
            kind: "agent".into(),
        })
        .collect()
}

fn write_kernel_json(place: &Place, addr: SocketAddr) -> Result<()> {
    let info = KernelJson {
        url: format!("tcp://{addr}"),
        pid: std::process::id(),
    };
    std::fs::write(place.kernel_json(), serde_json::to_string_pretty(&info)?)?;
    Ok(())
}
