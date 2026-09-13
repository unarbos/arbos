//! `arbos-kernel rewind <place> [--agent ID] (--list | --to LINE | --back N) [--files] [--yes]`
//!
//! Put an agent back to the start of an earlier turn. Every turn records a
//! checkpoint when it starts (`checkpoints.jsonl`: transcript line, HEAD,
//! and a commit of the working tree — see `arbos_engine::git`). A
//! rewind cuts the transcript at that line (the cut lines go to
//! `transcript.rewound-<ts>.jsonl`, nothing is lost) and, with `--files`,
//! puts the project's tracked files back to the checkpoint too. Without
//! `--files` it prints the checkpoint so a hand can do it.
//!
//! Runs against a stopped kernel only: a live kernel tails the transcript
//! and would replay a shrunken file as new lines to every attached
//! client. Stop it, rewind, start it again; the desktop reloads the chat
//! from the files.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use arbos_core::{EventKind, Layout, Place, load_transcript};
use arbos_engine::git::{Checkpoint, checkpoints, restore};

pub const USAGE: &str =
    "arbos-kernel rewind <place> [--agent ID] (--list | --to LINE | --back N) [--files] [--yes]";

#[derive(Debug, Clone)]
pub struct Args {
    pub place: PathBuf,
    pub agent: String,
    pub list: bool,
    pub to: Option<u64>,
    pub back: Option<u64>,
    pub files: bool,
    pub yes: bool,
}

impl Args {
    pub fn parse(mut args: impl Iterator<Item = String>) -> Result<Self> {
        let mut out = Self {
            place: std::env::current_dir()?,
            agent: "root".into(),
            list: false,
            to: None,
            back: None,
            files: false,
            yes: false,
        };
        let mut place_given = false;
        while let Some(a) = args.next() {
            match a.as_str() {
                "--agent" | "-a" => out.agent = args.next().context("--agent needs an id")?,
                "--list" | "-l" => out.list = true,
                "--to" => {
                    out.to = Some(
                        args.next()
                            .context("--to needs a transcript line")?
                            .parse()
                            .context("--to: not a number")?,
                    )
                }
                "--back" => {
                    out.back = Some(
                        args.next()
                            .context("--back needs a count of turns")?
                            .parse()
                            .context("--back: not a number")?,
                    )
                }
                "--files" => out.files = true,
                "--yes" | "-y" => out.yes = true,
                other if other.starts_with('-') => bail!("rewind: unknown flag {other}\n{USAGE}"),
                other if !place_given => {
                    out.place = PathBuf::from(other);
                    place_given = true;
                }
                other => bail!("rewind: unexpected argument {other}\n{USAGE}"),
            }
        }
        if !out.list && out.to.is_none() && out.back.is_none() {
            bail!("rewind: say --list, --to LINE, or --back N\n{USAGE}");
        }
        Ok(out)
    }
}

/// Which turn to go back to.
#[derive(Debug, Clone, Copy)]
pub enum Target {
    /// The checkpoint at exactly this transcript line.
    Line(u64),
    /// This many turns back from the last one.
    Back(u64),
    /// The Nth user turn (1-based), counted over `user` transcript lines.
    Turn(u32),
}

/// What a cut did.
#[derive(Debug, Clone)]
pub struct Cut {
    pub checkpoint: Checkpoint,
    /// Transcript lines removed.
    pub dropped: u64,
    /// Where they went.
    pub archive: PathBuf,
}

/// The checkpoint `target` means, given the transcript and checkpoints.
pub fn resolve(
    events: &[arbos_core::Event],
    cps: &[Checkpoint],
    target: Target,
) -> Result<Checkpoint> {
    match target {
        Target::Line(line) => cps
            .iter()
            .find(|cp| cp.line == line)
            .cloned()
            .with_context(|| format!("no turn starts at line {line}; see --list")),
        Target::Back(n) => {
            let ix = cps
                .len()
                .checked_sub(n as usize)
                .with_context(|| format!("only {} turns to go back over", cps.len()))?;
            Ok(cps[ix].clone())
        }
        Target::Turn(n) => {
            // The Nth user line; the turn starts at the checkpoint on or
            // just before it (the wake line).
            let user_line = events
                .iter()
                .filter(|e| matches!(e.kind, EventKind::User { .. }))
                .nth(n.saturating_sub(1) as usize)
                .map(|e| e.seq)
                .with_context(|| format!("no user turn {n}"))?;
            cps.iter()
                .filter(|cp| cp.line <= user_line)
                .max_by_key(|cp| cp.line)
                .cloned()
                .with_context(|| format!("no checkpoint for user turn {n} (line {user_line})"))
        }
    }
}

/// Cut the transcript at the checkpoint's line and trim the checkpoints.
/// The cut lines go to `transcript.rewound-<ts>.jsonl` first, whole.
pub fn cut(place: &Place, agent: &str, target: Target) -> Result<Cut> {
    let layout = Layout::new(place, agent);
    if !layout.agent_md().exists() {
        bail!("no agent {agent} in {}", place.path.display());
    }
    let events = load_transcript(&layout.transcript()).unwrap_or_default();
    let cps = checkpoints(&layout.dir);
    if cps.is_empty() {
        bail!(
            "{agent} has no checkpoints yet (they are written when a turn starts, from this version on)"
        );
    }
    let checkpoint = resolve(&events, &cps, target)?;
    let cut_from = checkpoint.line.saturating_sub(1) as usize;
    if cut_from >= events.len() {
        bail!(
            "line {} is at or past the end of the transcript ({} lines); nothing to rewind",
            checkpoint.line,
            events.len()
        );
    }
    let raw = std::fs::read_to_string(layout.transcript())?;
    let lines: Vec<&str> = raw.lines().collect();
    let at = cut_from.min(lines.len());
    let keep = lines[..at].join("\n");
    let gone = lines[at..].join("\n");
    let archive = layout
        .dir
        .join(format!("transcript.rewound-{}.jsonl", arbos_core::now_ms()));
    std::fs::write(&archive, format!("{gone}\n"))?;
    std::fs::write(
        layout.transcript(),
        if keep.is_empty() {
            String::new()
        } else {
            format!("{keep}\n")
        },
    )?;
    // Checkpoints of the cut turns go too, the target's own included: the
    // next turn starts on that line and writes a fresh one.
    let mut text = String::new();
    for cp in cps.iter().filter(|cp| cp.line < checkpoint.line) {
        text.push_str(&serde_json::to_string(cp)?);
        text.push('\n');
    }
    std::fs::write(layout.dir.join("checkpoints.jsonl"), text)?;
    Ok(Cut {
        checkpoint,
        dropped: (lines.len() - at) as u64,
        archive,
    })
}

/// Put the agent's working directory back to the checkpoint.
pub fn restore_files(place: &Place, agent: &str, cp: &Checkpoint) -> Result<String> {
    let layout = Layout::new(place, agent);
    let a = arbos_core::Agent::load(&layout.dir)?;
    let cwd = a.cwd.clone().unwrap_or_else(|| place.path.clone());
    if !cwd.join(".git").exists() {
        bail!(
            "{} is not a git repository; files left as they are",
            cwd.display()
        );
    }
    restore(&cwd, cp)
}

pub fn run(args: Args) -> Result<i32> {
    let place = Place::new(std::fs::canonicalize(&args.place).unwrap_or(args.place.clone()));
    let layout = Layout::new(&place, &args.agent);
    if !layout.agent_md().exists() {
        bail!("no agent {} in {}", args.agent, place.path.display());
    }
    let events = load_transcript(&layout.transcript()).unwrap_or_default();
    let cps = checkpoints(&layout.dir);
    if cps.is_empty() {
        bail!(
            "{} has no checkpoints yet (they are written when a turn starts, from this version on)",
            args.agent
        );
    }
    // The turns as a table: the checkpoint's line, when, and the words
    // that started the turn.
    let turns: Vec<(usize, &Checkpoint, String)> = cps
        .iter()
        .enumerate()
        .map(|(i, cp)| {
            let opener = events
                .iter()
                .skip(cp.line.saturating_sub(1) as usize)
                .take(3)
                .find_map(|e| match &e.kind {
                    EventKind::User { text, .. } => Some(text.clone()),
                    EventKind::Wake { text: Some(t), .. } => Some(t.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            (i + 1, cp, one_line(&opener, 70))
        })
        .collect();
    if args.list {
        println!(
            "{} turns on {} ({} transcript lines):",
            turns.len(),
            args.agent,
            events.len()
        );
        for (n, cp, opener) in &turns {
            println!(
                "  turn {n:>3}  line {:>5}  {}  {}{}  {opener}",
                cp.line,
                stamp(cp.ts),
                &cp.head[..cp.head.len().min(8)],
                if cp.work.is_some() { "+wt" } else { "   " },
            );
        }
        println!("rewind --to LINE puts the agent back to the start of that turn.");
        return Ok(0);
    }
    if kernel_alive(&place) {
        bail!(
            "a kernel is serving {}; use the desktop's Rewind here, or stop it first (its tail would replay the cut transcript to every window)",
            place.path.display()
        );
    }
    let target = match (args.to, args.back) {
        (Some(line), _) => Target::Line(line),
        (None, Some(n)) => Target::Back(n),
        (None, None) => unreachable!("parse requires one"),
    };
    let planned = resolve(&events, &cps, target)?;
    let cut_from = planned.line.saturating_sub(1) as usize;
    if cut_from >= events.len() {
        bail!(
            "line {} is at or past the end of the transcript ({} lines); nothing to rewind",
            planned.line,
            events.len()
        );
    }
    println!(
        "rewind {}: cut {} transcript lines from line {} on; project {}{}",
        args.agent,
        events.len() - cut_from,
        planned.line,
        &planned.head[..planned.head.len().min(12)],
        match (&planned.work, args.files) {
            (Some(_), true) => " + saved working tree (restoring)",
            (None, true) => " (restoring)",
            (_, false) => " (files untouched: add --files)",
        }
    );
    if !args.yes {
        print!("proceed? [y/N] ");
        use std::io::Write;
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim(), "y" | "Y" | "yes") {
            println!("nothing done");
            return Ok(1);
        }
    }
    let done = cut(&place, &args.agent, target)?;
    println!(
        "transcript cut; the {} lines are in {}",
        done.dropped,
        done.archive.display()
    );
    if args.files {
        match restore_files(&place, &args.agent, &done.checkpoint) {
            Ok(what) => println!("project restored to {what}"),
            Err(e) => {
                println!("project not restored: {e:#}");
                return Ok(2);
            }
        }
    }
    Ok(0)
}

fn kernel_alive(place: &Place) -> bool {
    let Ok(text) = std::fs::read_to_string(place.kernel_json()) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    let Some(pid) = v.get("pid").and_then(|p| p.as_u64()) else {
        return false;
    };
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

fn one_line(s: &str, max: usize) -> String {
    let t: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > max {
        t.chars().take(max).collect::<String>() + "…"
    } else {
        t
    }
}

/// `09:00:01` from unix millis (UTC), enough to tell turns apart.
fn stamp(ms: i64) -> String {
    let secs = ms.div_euclid(1000).rem_euclid(86_400);
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}
