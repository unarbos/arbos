//! `arbos-kernel feedback <place>`: the in-app report's material through a
//! second door. A window whose attach never connected has no kernel to
//! ask for a `feedback` frame — and the disconnection is the bug it wants
//! to report. ssh to the machine can be fine while the attach is not, so
//! the desktop runs this over ssh and takes stdout as the bundle: the same
//! assembler (`feedback::bundle`), the same redaction, the same budget,
//! printed as the `feedback_bundle` frame so no second parser is needed.
//!
//! Exit codes keep "the far side has nothing to say" apart from "the
//! command failed": 0 with a bundle on stdout (an empty transcript is a
//! bundle with no lines, and still 0); 2 when the place cannot be read (no
//! `.arbos/` there, or no such agent); 1 when the host config cannot be
//! read. Every non-zero exit puts one plain reason on stderr.

use anyhow::{Context, Result};
use arbos_core::Place;
use arbos_core::wire::Frame;
use arbos_engine::Host;
use serde_json::{Value, json};
use std::path::PathBuf;

pub const USAGE: &str = "arbos-kernel feedback <place> [--agent root] [--seq N] [--call-id ID] [--tail 200] [--note TEXT]   (prints the feedback_bundle frame as JSON on stdout; exit 2 = the place cannot be read, 1 = the host config cannot be read)";

pub struct Args {
    pub place: PathBuf,
    pub agent: String,
    pub seq: Option<u64>,
    pub call_id: Option<String>,
    pub tail: u32,
    pub note: String,
}

impl Args {
    pub fn parse(mut argv: impl Iterator<Item = String>) -> Result<Self> {
        let mut args = Args {
            place: PathBuf::new(),
            agent: "root".into(),
            seq: None,
            call_id: None,
            tail: 200,
            note: String::new(),
        };
        while let Some(a) = argv.next() {
            match a.as_str() {
                "--agent" | "-a" => args.agent = argv.next().context("--agent needs an id")?,
                "--seq" => {
                    args.seq = Some(
                        argv.next()
                            .context("--seq needs a line number")?
                            .parse()
                            .context("--seq needs a number")?,
                    )
                }
                "--call-id" => args.call_id = Some(argv.next().context("--call-id needs an id")?),
                "--tail" => {
                    args.tail = argv
                        .next()
                        .context("--tail needs a count")?
                        .parse()
                        .context("--tail needs a number")?
                }
                "--note" => args.note = argv.next().context("--note needs text")?,
                "-h" | "--help" => anyhow::bail!("{USAGE}"),
                other if other.starts_with('-') => anyhow::bail!("unknown flag {other}\n{USAGE}"),
                other => args.place = PathBuf::from(other),
            }
        }
        if args.place.as_os_str().is_empty() {
            anyhow::bail!("feedback needs a place\n{USAGE}");
        }
        Ok(args)
    }
}

/// The kernel that serves (or last served) the place, from its
/// `kernel.json`: the CLI is not that kernel, so the frame's `kernel`
/// facts describe this binary and this object says who was serving.
fn serving(place: &Place) -> Value {
    let path = place.kernel_json_read();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return json!({ "known": false, "why": "no kernel.json: no kernel has run here" });
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return json!({ "known": false, "why": format!("{} is not JSON", path.display()) });
    };
    let pid = v["pid"].as_u64().unwrap_or(0) as u32;
    json!({
        "known": true,
        "pid": pid,
        "live": pid != 0 && pid_alive(pid),
        "url": v["url"],
        "started": v["started"],
        "version": v["version"],
        "git_sha": v["git_sha"],
        "log": v["log"],
    })
}

fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// Runs the command; the exit code is the caller's to apply.
pub fn run(args: Args) -> Result<i32> {
    let root = args
        .place
        .canonicalize()
        .unwrap_or_else(|_| args.place.clone());
    let place = Place::new(root.clone());
    if !place.arbos().is_dir() {
        eprintln!(
            "feedback: {} is not an Arbos place (no .arbos/ folder)",
            root.display()
        );
        return Ok(2);
    }
    if !arbos_core::agent_exists(&place, &args.agent) {
        eprintln!(
            "feedback: no agent is named {:?} in {}",
            args.agent,
            root.display()
        );
        return Ok(2);
    }
    let host = match Host::load().or_else(|_| Host::peek()) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("feedback: the host config could not be read: {e:#}");
            return Ok(1);
        }
    };
    let req = crate::feedback::Request {
        agent: &args.agent,
        seq: args.seq,
        call_id: args.call_id.as_deref(),
        tail: args.tail,
        note: &args.note,
    };
    let mut b = crate::feedback::bundle(&place, &req, &host);
    // Not the serving kernel's own answer: say so, and say who was serving.
    if let Value::Object(map) = &mut b.kernel {
        map.insert("door".into(), json!("cli"));
        map.insert("serving".into(), serving(&place));
    }
    let frame = Frame::FeedbackBundle {
        agent: args.agent.clone(),
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
    };
    println!("{}", serde_json::to_string(&frame)?);
    Ok(0)
}
