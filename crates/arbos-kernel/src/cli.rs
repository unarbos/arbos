//! `arbos-kernel run` and `arbos-kernel attach`: the kernel from a shell.
//!
//! Scripts, CI, and the QA loop talk to a kernel the way the desktop does,
//! over the loopback frames in `.arbos/kernel.json`, without a window.
//! `run` sends one prompt and streams that agent's turn to stdout; `attach`
//! streams everything until Ctrl-C.

use anyhow::{Context, Result, bail};
use arbos_core::{Event, EventKind, Place, wire::Frame};
use serde::Deserialize;
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

pub const USAGE: &str = "arbos-kernel run [--place DIR] [--agent ID] [--json] [--steer] [--timeout SECS] [--no-spawn] \"<prompt>\"\narbos-kernel answer [--place DIR] [--agent ID] [--follow] [--json] (\"<text>\" | --approve | --deny)\narbos-kernel attach [--place DIR | --hub MACHINE[/PROJECT]] [--agent ID] [--json]   (--hub: a kernel on another machine, by name, through ~/.config/arbos/hub.toml)";

/// How long to wait for a kernel this command started to write its port.
const READY_WAIT: Duration = Duration::from_secs(60);
/// After the kernel says the agent is idle, how long the transcript tail
/// has to deliver `turn_complete` (it ticks every 200 ms).
const IDLE_GRACE: Duration = Duration::from_millis(1500);

/// What `run` exits with, so a script can branch on it.
pub const EXIT_OK: i32 = 0;
pub const EXIT_ERROR: i32 = 1;
pub const EXIT_FAILED_TURN: i32 = 2;
pub const EXIT_WAITING: i32 = 3;
pub const EXIT_TIMEOUT: i32 = 4;

#[derive(Debug, Clone)]
pub struct Args {
    pub place: PathBuf,
    pub agent: String,
    pub json: bool,
    pub steer: bool,
    pub timeout: Option<Duration>,
    pub no_spawn: bool,
    pub prompt: Option<String>,
    /// `answer` only: stream the rest of the turn after answering.
    pub follow: bool,
    /// `answer` only: a bash approval instead of a text answer.
    pub allow: Option<bool>,
    /// `attach` only: a kernel by machine name through the hub,
    /// `<machine>` or `<machine>/<project>`.
    pub hub: Option<String>,
}

impl Args {
    pub fn parse(mut argv: impl Iterator<Item = String>) -> Result<Self> {
        let mut args = Self {
            place: std::env::current_dir()?,
            agent: "root".into(),
            json: false,
            steer: false,
            timeout: None,
            no_spawn: false,
            prompt: None,
            follow: false,
            allow: None,
            hub: None,
        };
        let mut rest: Vec<String> = Vec::new();
        while let Some(a) = argv.next() {
            match a.as_str() {
                "--place" | "-C" => {
                    args.place = PathBuf::from(argv.next().context("--place needs a directory")?)
                }
                "--agent" | "-a" => args.agent = argv.next().context("--agent needs an id")?,
                "--json" => args.json = true,
                "--steer" => args.steer = true,
                "--no-spawn" => args.no_spawn = true,
                "--follow" | "-f" => args.follow = true,
                "--approve" => args.allow = Some(true),
                "--deny" => args.allow = Some(false),
                "--hub" => {
                    args.hub = Some(argv.next().context("--hub needs <machine>[/<project>]")?)
                }
                "--timeout" => {
                    let s = argv.next().context("--timeout needs seconds")?;
                    let secs: u64 = s.parse().with_context(|| format!("--timeout {s:?}"))?;
                    args.timeout = (secs > 0).then(|| Duration::from_secs(secs));
                }
                "-h" | "--help" => {
                    println!("{USAGE}");
                    std::process::exit(0);
                }
                other if other.starts_with("--") => bail!("unknown flag {other}\n{USAGE}"),
                _ => rest.push(a),
            }
        }
        if !rest.is_empty() {
            args.prompt = Some(rest.join(" "));
        }
        Ok(args)
    }
}

#[derive(Deserialize)]
struct KernelJson {
    url: String,
    pid: u32,
}

/// Send one prompt, stream the turn, return the exit code.
pub fn run(args: Args) -> Result<i32> {
    let prompt = args
        .prompt
        .clone()
        .filter(|p| !p.trim().is_empty())
        .context("run needs a prompt")?;
    let place = Place::new(std::fs::canonicalize(&args.place).unwrap_or(args.place.clone()));
    let addr = kernel_addr(&place, !args.no_spawn)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let stream = TcpStream::connect(&addr)
            .await
            .with_context(|| format!("connect {addr}"))?;
        let (r, mut w) = stream.into_split();
        let mut lines = BufReader::new(r).lines();
        let frame = Frame::User {
            agent: args.agent.clone(),
            text: prompt.clone(),
            steer: args.steer,
            attachments: vec![],
            channel: String::new(),
            device: String::new(),
            model: String::new(),
        };
        w.write_all(format!("{}\n", serde_json::to_string(&frame)?).as_bytes())
            .await?;
        stream_turn(&mut lines, &mut w, &args, &place, Some(&prompt)).await
    })
}

/// Read frames until the agent's turn ends, printing its transcript lines.
/// `prompt` = the user line that starts our turn (skip everything before
/// it; a fresh kernel replays the whole transcript once); `None` = the
/// turn is already under way.
async fn stream_turn(
    lines: &mut tokio::io::Lines<BufReader<tokio::net::tcp::OwnedReadHalf>>,
    w: &mut tokio::net::tcp::OwnedWriteHalf,
    args: &Args,
    place: &Place,
    prompt: Option<&str>,
) -> Result<i32> {
    let deadline = args.timeout.map(|t| Instant::now() + t);
    // A fresh kernel replays the whole transcript once to whoever is
    // attached; our turn starts at the line that echoes our prompt.
    let mut started = prompt.is_none();
    let mut failed = false;
    // The kernel says "idle" the moment the turn's task ends; the
    // transcript tail that carries turn_complete follows on its own
    // tick, and can even land after the idle. So: once our turn has
    // started and the agent is idle, a quiet stretch means the turn
    // ended without completing (an error path the kernel logged).
    let mut idle = false;
    let mut quiet_since = Instant::now();
    loop {
        let mut limit = deadline.map(|d| d.saturating_duration_since(Instant::now()));
        if started && idle {
            let grace = (quiet_since + IDLE_GRACE).saturating_duration_since(Instant::now());
            limit = Some(limit.map_or(grace, |l| l.min(grace)));
        }
        let next = match limit {
            Some(left) => match tokio::time::timeout(left, lines.next_line()).await {
                Ok(r) => r,
                Err(_) if started && idle && deadline.is_none_or(|d| Instant::now() < d) => {
                    eprintln!(
                        "run: the turn ended without completing; see {}",
                        place.runtime_dir().join("kernel.log").display()
                    );
                    return Ok(EXIT_FAILED_TURN);
                }
                Err(_) => {
                    eprintln!(
                        "run: timed out after {:?}",
                        args.timeout.unwrap_or_default()
                    );
                    return Ok(EXIT_TIMEOUT);
                }
            },
            None => lines.next_line().await,
        };
        let Some(line) = next? else {
            eprintln!("run: the kernel closed the connection");
            return Ok(EXIT_ERROR);
        };
        let Ok(frame) = serde_json::from_str::<Frame>(&line) else {
            continue;
        };
        match frame {
            // Live emits (seq 0: streaming text, a tool starting) are
            // for a window; the transcript lines that follow them are
            // the record, and the only thing printed here.
            Frame::Event { agent, event } if agent == args.agent && event.seq > 0 => {
                if !started {
                    started = matches!((&event.kind, prompt), (EventKind::User { text, .. }, Some(p)) if text == p);
                    if !started {
                        continue;
                    }
                }
                quiet_since = Instant::now();
                if let EventKind::Notice { failed: true, .. } = &event.kind {
                    failed = true;
                }
                print_event(&event, args.json);
                if matches!(event.kind, EventKind::TurnComplete { .. }) {
                    return Ok(if failed { EXIT_FAILED_TURN } else { EXIT_OK });
                }
            }
            Frame::Turn { agent, state, .. } if agent == args.agent => {
                idle = state == "idle";
                quiet_since = Instant::now();
            }
            Frame::Ask {
                agent,
                question,
                options,
                id,
            } if agent == args.agent && started => match answer(&agent, &question, &options, id)? {
                Some(reply) => {
                    w.write_all(format!("{}\n", serde_json::to_string(&reply)?).as_bytes())
                        .await?;
                }
                None => {
                    eprintln!(
                        "run: the agent is waiting on a question and there is no terminal to answer it. Answer with: arbos-kernel answer [--agent ID] [--follow] \"<text>\" (or --approve / --deny)"
                    );
                    return Ok(EXIT_WAITING);
                }
            },
            _ => {}
        }
    }
}

/// Answer the question the agent is waiting on (or approve/deny a bash
/// command), then optionally follow the rest of its turn.
pub fn answer_cmd(args: Args, allow: Option<bool>, follow: bool) -> Result<i32> {
    let text = args.prompt.clone().unwrap_or_default();
    if allow.is_none() && text.trim().is_empty() {
        bail!("answer needs the text of the answer, or --approve / --deny");
    }
    let place = Place::new(std::fs::canonicalize(&args.place).unwrap_or(args.place.clone()));
    // A question is only ever waiting in a live kernel.
    let addr = kernel_addr(&place, false)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let stream = TcpStream::connect(&addr)
            .await
            .with_context(|| format!("connect {addr}"))?;
        let (r, mut w) = stream.into_split();
        let mut lines = BufReader::new(r).lines();
        let frame = match allow {
            Some(allow) => Frame::Approve {
                agent: args.agent.clone(),
                call_id: String::new(),
                allow,
            },
            None => Frame::Answer {
                agent: args.agent.clone(),
                text: text.clone(),
                // Sent blind: accepted only while exactly one question is pending.
                id: None,
            },
        };
        w.write_all(format!("{}\n", serde_json::to_string(&frame)?).as_bytes())
            .await?;
        if !follow {
            eprintln!("answered {} on {}", args.agent, place.path.display());
            return Ok(EXIT_OK);
        }
        stream_turn(&mut lines, &mut w, &args, &place, None).await
    })
}

/// Stream events until Ctrl-C. With `--hub`, the kernel is one on another
/// machine, reached by name through the hub; the frames are the same.
pub fn attach(args: Args, all_agents: bool) -> Result<i32> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let (tx, mut lines) = tokio::sync::mpsc::unbounded_channel::<String>();
    rt.block_on(async move {
        if let Some(target) = args.hub.clone() {
            let cfg = arbos_core::HubConfig::resolve(None, None)?
                .context("attach --hub: no hub in ~/.config/arbos/hub.toml (or ARBOS_HUB)")?;
            let (machine, project) = match target.split_once('/') {
                Some((m, p)) => (m.to_string(), Some(p.to_string())),
                None => (target.clone(), None),
            };
            let ws = crate::hub_link::attach(&cfg, &machine, project.as_deref()).await?;
            eprintln!("attached to {target} through {}; Ctrl-C to stop", cfg.url);
            tokio::spawn(async move {
                let mut ws = ws;
                while let Some(text) = crate::hub_link::next_text(&mut ws).await {
                    for l in text.lines().filter(|l| !l.trim().is_empty()) {
                        if tx.send(l.to_string()).is_err() {
                            return;
                        }
                    }
                }
            });
        } else {
            let place =
                Place::new(std::fs::canonicalize(&args.place).unwrap_or(args.place.clone()));
            let addr = kernel_addr(&place, !args.no_spawn)?;
            let stream = TcpStream::connect(&addr)
                .await
                .with_context(|| format!("connect {addr}"))?;
            let (r, _w) = stream.into_split();
            eprintln!(
                "attached to {} at {addr}; Ctrl-C to stop",
                place.path.display()
            );
            tokio::spawn(async move {
                let mut lines = BufReader::new(r).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    if tx.send(l).is_err() {
                        return;
                    }
                }
            });
        }
        loop {
            let line = tokio::select! {
                l = lines.recv() => l,
                _ = tokio::signal::ctrl_c() => return Ok(EXIT_OK),
            };
            let Some(line) = line else {
                eprintln!("attach: the kernel closed the connection");
                return Ok(EXIT_ERROR);
            };
            let Ok(frame) = serde_json::from_str::<Frame>(&line) else {
                continue;
            };
            print_attached(&frame, &args, all_agents)?;
        }
    })
}

/// One live frame as an attach line, when it is for the agent asked about.
fn print_attached(frame: &Frame, args: &Args, all_agents: bool) -> Result<()> {
    match frame {
        Frame::Event { agent, event } if event.seq > 0 && (all_agents || *agent == args.agent) => {
            if args.json {
                let mut v = serde_json::to_value(event)?;
                if let Some(o) = v.as_object_mut() {
                    o.insert("agent".into(), serde_json::Value::String(agent.clone()));
                }
                println!("{v}");
            } else {
                print!("[{agent}] ");
                print_event(event, false);
            }
        }
        Frame::Ask {
            agent,
            question,
            options,
            ..
        } if all_agents || *agent == args.agent => {
            if args.json {
                println!(
                    "{}",
                    serde_json::json!({"kind": "ask", "agent": agent, "question": question, "options": options})
                );
            } else {
                println!("[{agent}] ? {question} {}", options.join(" / "));
            }
        }
        _ => {}
    }
    Ok(())
}

/// The kernel's loopback address, starting one when none is alive.
fn kernel_addr(place: &Place, may_spawn: bool) -> Result<String> {
    if let Some(addr) = live_addr(place) {
        return Ok(addr);
    }
    if !may_spawn {
        bail!(
            "no kernel is serving {} (looked at {}); start one with `arbos-kernel serve` or drop --no-spawn",
            place.path.display(),
            place.kernel_json().display()
        );
    }
    spawn_kernel(place)?;
    let deadline = Instant::now() + READY_WAIT;
    while Instant::now() < deadline {
        if let Some(addr) = live_addr(place) {
            return Ok(addr);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    bail!(
        "started a kernel for {} but it wrote no live {} within {:?}; see .arbos/runtime/kernel.log",
        place.path.display(),
        place.kernel_json().display(),
        READY_WAIT
    )
}

/// `host:port` from kernel.json when its pid is alive and the port answers.
fn live_addr(place: &Place) -> Option<String> {
    let text = std::fs::read_to_string(place.kernel_json_read()).ok()?;
    let info: KernelJson = serde_json::from_str(&text).ok()?;
    if !pid_alive(info.pid) {
        return None;
    }
    let addr = info.url.strip_prefix("tcp://")?.to_string();
    std::net::TcpStream::connect_timeout(&addr.parse().ok()?, Duration::from_secs(2)).ok()?;
    Some(addr)
}

fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        Path::new(&format!("/proc/{pid}")).exists()
            || Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// `arbos-kernel serve <place>` in its own process group, its stdout and
/// stderr in `.arbos/runtime/kernel.out.log`, so it outlives this command
/// and its terminal.
fn spawn_kernel(place: &Place) -> Result<()> {
    let dir = place.runtime_dir();
    std::fs::create_dir_all(&dir)?;
    let log = std::fs::File::create(dir.join("kernel.out.log"))?;
    let err = log.try_clone()?;
    let me = std::env::current_exe().context("locate arbos-kernel")?;
    let mut cmd = Command::new(me);
    cmd.arg("serve")
        .arg(&place.path)
        .current_dir(&place.path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(err));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn().context("start arbos-kernel serve")?;
    eprintln!("started a kernel for {}", place.path.display());
    Ok(())
}

/// Ask the person at the terminal. None when there is no terminal.
fn answer(
    agent: &str,
    question: &str,
    options: &[String],
    id: Option<String>,
) -> Result<Option<Frame>> {
    if !std::io::stdin().is_terminal() {
        return Ok(None);
    }
    let approval = options.len() == 2
        && options.iter().any(|o| o == "allow")
        && options.iter().any(|o| o == "deny")
        && question.starts_with("allow ");
    let mut out = std::io::stdout();
    writeln!(out, "? {question}")?;
    if !options.is_empty() {
        writeln!(out, "  options: {}", options.join(" / "))?;
    }
    write!(out, "> ")?;
    out.flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let text = line.trim().to_string();
    Ok(Some(if approval {
        Frame::Approve {
            agent: agent.to_string(),
            call_id: id.clone().unwrap_or_default(),
            allow: matches!(
                text.to_ascii_lowercase().as_str(),
                "y" | "yes" | "allow" | "a"
            ),
        }
    } else {
        Frame::Answer {
            agent: agent.to_string(),
            text,
            id,
        }
    }))
}

/// One event to stdout: the transcript line as JSON (with its `seq` and
/// `ts`), or a line a person reads.
fn print_event(event: &Event, json: bool) {
    if json {
        if let Ok(s) = serde_json::to_string(event) {
            println!("{s}");
        }
        return;
    }
    match &event.kind {
        EventKind::Assistant { text, .. } => {
            if !text.trim().is_empty() {
                println!("{text}");
            }
        }
        EventKind::Tool(rec) => {
            let args = rec.args.as_ref().map(|a| a.to_string()).unwrap_or_default();
            let args = clip(&args, 100);
            let result = rec
                .error
                .as_deref()
                .or(rec.body.as_deref())
                .unwrap_or("")
                .lines()
                .next()
                .unwrap_or("");
            println!(
                "  [{}] {args} -> {}{}",
                rec.name,
                clip(result, 120),
                if rec.error.is_some() { " (error)" } else { "" }
            );
        }
        EventKind::Say { from, text } => println!("  [{from}] {text}"),
        EventKind::Notice { text, failed } => {
            println!("  [{}] {text}", if *failed { "failed" } else { "notice" })
        }
        EventKind::Ask {
            question, options, ..
        } => {
            println!("  ? {question} {}", options.join(" / "))
        }
        EventKind::Interrupted { detail } => println!("  [interrupted] {detail}"),
        EventKind::ImageDescribed { path, model, .. } => {
            println!("  [image {path} described by {model}]")
        }
        EventKind::TurnComplete { usage } => {
            if let Some(u) = usage {
                eprintln!(
                    "(turn complete; {} of {} context tokens used)",
                    u.used, u.size
                );
            }
        }
        EventKind::Wake { .. }
        | EventKind::User { .. }
        | EventKind::Thinking { .. }
        | EventKind::Answer { .. }
        | EventKind::Approval { .. }
        | EventKind::Fold { .. }
        | EventKind::Compaction { .. }
        | EventKind::WindowReset {} => {}
    }
}

fn clip(s: &str, n: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= n {
        s
    } else {
        let cut: String = s.chars().take(n).collect();
        format!("{cut}…")
    }
}
