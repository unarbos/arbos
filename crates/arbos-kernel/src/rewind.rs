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

pub const USAGE: &str = "arbos-kernel rewind <place> [--agent ID] (--list | --to LINE | --back N | --commit SHA) [--files] [--yes]\narbos-kernel log <place> [-n N]";

#[derive(Debug, Clone)]
pub struct Args {
    pub place: PathBuf,
    pub agent: String,
    pub list: bool,
    pub to: Option<u64>,
    pub back: Option<u64>,
    /// Put the whole `.arbos/` record back to this commit of its repository.
    pub commit: Option<String>,
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
            commit: None,
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
                "--commit" | "-c" => {
                    out.commit = Some(args.next().context("--commit needs a sha")?)
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
        if !out.list && out.to.is_none() && out.back.is_none() && out.commit.is_none() {
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
    let cut_from = open_wake(&events, checkpoint.line.saturating_sub(1) as usize);
    if cut_from >= events.len() {
        bail!(
            "line {} is at or past the end of the transcript ({} lines); nothing to rewind",
            checkpoint.line,
            events.len()
        );
    }
    let (dropped, archive) = truncate(place, agent, &layout, cut_from, &cps, checkpoint.line)?;
    Ok(Cut {
        checkpoint,
        dropped,
        archive,
    })
}

/// The transcript cut at line `line` (0-based; that line and after go),
/// no checkpoint needed: for a turn the kernel itself is taking back —
/// one superseded before it did anything. Returns lines dropped and
/// where they went.
pub fn cut_from_line(place: &Place, agent: &str, line: u64) -> Result<(u64, PathBuf)> {
    let layout = Layout::new(place, agent);
    let events = load_transcript(&layout.transcript()).unwrap_or_default();
    let at = line as usize;
    if at >= events.len() {
        bail!(
            "line {line} is at or past the end of the transcript ({} lines); nothing to cut",
            events.len()
        );
    }
    if !events[at].is_wake() {
        bail!("line {line} is not a turn's wake; refusing to cut mid-turn");
    }
    let cps = checkpoints(&layout.dir);
    truncate(place, agent, &layout, at, &cps, line)
}

/// Lines `at..` of the transcript to the rewind archive, the file
/// replaced whole; checkpoints from `keep_below` on dropped with them.
fn truncate(
    place: &Place,
    agent: &str,
    layout: &Layout,
    at: usize,
    cps: &[Checkpoint],
    keep_below: u64,
) -> Result<(u64, PathBuf)> {
    let raw = std::fs::read_to_string(layout.transcript())?;
    let lines: Vec<&str> = raw.lines().collect();
    let at = at.min(lines.len());
    let keep = lines[..at].join("\n");
    let gone = lines[at..].join("\n");
    let archive = layout
        .dir
        .join(format!("transcript.rewound-{}.jsonl", arbos_core::now_ms()));
    std::fs::write(&archive, format!("{gone}\n"))?;
    // Whole or not at all: `fs::write` truncates first, and a reader in
    // that gap (the desktop's tail, a test) saw an empty transcript. Write
    // beside it and rename over it.
    replace_file(
        &layout.transcript(),
        &if keep.is_empty() {
            String::new()
        } else {
            format!("{keep}\n")
        },
    )?;
    // Checkpoints of the cut turns go too, the target's own included: the
    // next turn starts on that line and writes a fresh one — and their
    // tree sidecars with them, or a later turn at the same line inherits
    // a cut turn's tree (qal-j20).
    let mut text = String::new();
    for cp in cps.iter().filter(|cp| cp.line < keep_below) {
        text.push_str(&serde_json::to_string(cp)?);
        text.push('\n');
    }
    for cp in cps.iter().filter(|cp| cp.line >= keep_below) {
        let _ = std::fs::remove_file(arbos_engine::git::tree_sidecar(&layout.dir, cp.line));
    }
    // And the refs that kept the cut turns' trees alive: no rewind can
    // reach them now, and a project should not carry every cut turn's
    // working tree in its repository for ever.
    // The agent's repository is its work dir (a worktree shares refs with
    // the place's repository, so the place path reaches them too).
    let cwd = arbos_core::load_agent(place, &arbos_core::AgentId::new(agent))
        .map(|a| a.work_dir(&place.path))
        .unwrap_or_else(|_| place.path.clone());
    arbos_engine::git::drop_checkpoint_refs(
        &cwd,
        agent,
        cps.iter()
            .filter(|cp| cp.line >= keep_below && cp.work.is_some())
            .map(|cp| cp.line),
    );
    if !cps.is_empty() || layout.dir.join("checkpoints.jsonl").exists() {
        replace_file(&layout.dir.join("checkpoints.jsonl"), &text)?;
    }
    Ok(((lines.len() - at) as u64, archive))
}

/// Replace `path` with `text` in one step: a temp file in the same folder,
/// fsynced, renamed over it. No reader sees the file half-written or empty.
fn replace_file(path: &std::path::Path, text: &str) -> Result<()> {
    use std::io::Write;
    let dir = path.parent().context("file has no parent folder")?;
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id()
    ));
    {
        let mut f =
            std::fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename {} over {}", tmp.display(), path.display()))
}

/// The index to cut at so the turn goes whole: the checkpoint names the
/// turn's `user` line, but the `wake` that opened the turn sits just
/// before it (qa-032: left behind, it read as an unfinished turn and
/// fired an empty one at the next kernel start). Walk back from `at` to
/// the previous `turn_complete`; the first wake after it is the cut.
fn open_wake(events: &[arbos_core::Event], at: usize) -> usize {
    let mut cut = at;
    let mut i = at.min(events.len());
    while i > 0 && !events[i - 1].is_turn_complete() {
        i -= 1;
        if events[i].is_wake() {
            cut = i;
        }
    }
    cut
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
    // A rewind pressed right after a turn on a busy machine: the turn's
    // record is on disk but its tree is still being saved. Wait for it
    // here, on the blocking pool, rather than refuse at once.
    let cp = arbos_engine::git::settle_tree(&layout.dir, cp, TREE_WAIT);
    let out = restore(&cwd, &cp);
    let _ = std::fs::remove_file(arbos_engine::git::tree_sidecar(&layout.dir, cp.line));
    out
}

/// How long a `files: true` restore waits for a turn's tree to finish
/// saving before it refuses.
const TREE_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

pub fn run(args: Args) -> Result<i32> {
    let place = Place::new(std::fs::canonicalize(&args.place).unwrap_or(args.place.clone()));
    let layout = Layout::new(&place, &args.agent);
    if !layout.agent_md().exists() {
        bail!("no agent {} in {}", args.agent, place.path.display());
    }
    let events = load_transcript(&layout.transcript()).unwrap_or_default();
    let cps = checkpoints(&layout.dir);
    if cps.is_empty() {
        // The folder may be one that never gets a checkpoint (inside a
        // repository, no repository, no commit): that is the reason, not
        // the kernel's version.
        let cwd = arbos_core::load_agent(&place, &arbos_core::AgentId::new(&args.agent))
            .map(|a| a.work_dir(&place.path))
            .unwrap_or_else(|_| place.path.clone());
        match arbos_engine::git::why_no_checkpoints(&cwd) {
            Some(why) => bail!("{} has no checkpoints: {why}", args.agent),
            None => bail!(
                "{} has no checkpoints yet (they are written when a turn starts, from this version on)",
                args.agent
            ),
        }
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
    // The whole record back to a commit of .arbos/: every agent, every
    // plan, as they were. Forward commit; the pre-rewind state is one back.
    if let Some(sha) = &args.commit {
        if kernel_alive(&place) {
            bail!(
                "a kernel is serving {}; stop it first",
                place.path.display()
            );
        }
        if !args.yes {
            print!(
                "rewind the whole .arbos/ record of {} to {sha}? [y/N] ",
                place.path.display()
            );
            use std::io::Write;
            std::io::stdout().flush()?;
            let mut answer = String::new();
            std::io::stdin().read_line(&mut answer)?;
            if !matches!(answer.trim(), "y" | "Y" | "yes") {
                println!("nothing done");
                return Ok(1);
            }
        }
        let new = crate::snapshot::rewind_to(&place, sha)?;
        println!(
            ".arbos/ is back at {sha}; recorded as commit {new}. Start the kernel to continue from there."
        );
        if args.files {
            println!(
                "(--files applies to per-agent rewinds; the project's own repository was not touched)"
            );
        }
        return Ok(0);
    }
    if args.list {
        match crate::snapshot::log(&place, 8) {
            Ok(lines) if !lines.is_empty() => {
                println!(
                    "recent .arbos/ commits (rewind --commit SHA takes the whole record back):"
                );
                for l in &lines {
                    println!("  {l}");
                }
            }
            _ => {}
        }
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
    if let Ok(Some(sha)) = crate::snapshot::commit(
        &place,
        &format!("rewind {} to line {}", args.agent, done.checkpoint.line),
    ) {
        println!("recorded in .arbos/ as commit {sha}");
    }
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
    let Ok(text) = std::fs::read_to_string(place.kernel_json_read()) else {
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

#[cfg(test)]
mod roll_tests {
    use super::*;
    use arbos_core::{Event, append_event};

    /// The third instance of "cut too much": after the transcript rolled
    /// into the archive, the checkpoints still indexed the *old* file.
    /// `rewind turn 1` on the fresh file found a user line at seq 3 and
    /// the checkpoint "on or before" it — the project's first, at line 1
    /// — and with `--files` would have put the working tree back to the
    /// project's first turn. The checkpoints roll with the lines they
    /// describe, and a rewind into rolled history is refused.
    #[test]
    fn a_rewind_after_a_roll_does_not_reach_the_rolled_checkpoints() {
        let dir = std::env::temp_dir().join(format!("arbos-roll-rewind-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let place = Place::new(dir.clone());
        let layout = Layout::new(&place, "root");
        std::fs::create_dir_all(&layout.dir).unwrap();
        std::fs::write(layout.agent_md(), "# root\n").unwrap();
        let path = layout.transcript();
        // Six turns, a checkpoint at each wake line (as the turn writes it).
        let mut cps = String::new();
        for i in 0..6u64 {
            let at = load_transcript(&path).unwrap().len() as u64;
            cps.push_str(&format!(
                "{{\"line\":{at},\"ts\":0,\"head\":\"head{i}\",\"work\":null,\"clean\":true}}\n"
            ));
            append_event(
                &path,
                &Event::new(EventKind::Wake {
                    wake: "user".into(),
                    text: Some(format!("ask {i}")),
                    brief: None,
                }),
            )
            .unwrap();
            append_event(
                &path,
                &Event::new(EventKind::User {
                    text: format!("ask {i}"),
                    attachments: vec![],
                    channel: String::new(),
                    device: String::new(),
                }),
            )
            .unwrap();
            append_event(
                &path,
                &Event::new(EventKind::TurnComplete {
                    usage: None,
                    model: None,
                }),
            )
            .unwrap();
        }
        std::fs::write(layout.dir.join("checkpoints.jsonl"), &cps).unwrap();
        assert_eq!(checkpoints(&layout.dir).len(), 6);
        let rolled = arbos_core::files::roll_transcript(&place, "root", 10)
            .unwrap()
            .expect("18 lines roll past a cap of 10");
        assert_eq!(rolled.lines, 18);
        // One turn after the roll: its checkpoint is the only one.
        let at = load_transcript(&path).unwrap().len() as u64;
        std::fs::write(
            layout.dir.join("checkpoints.jsonl"),
            format!(
                "{{\"line\":{at},\"ts\":0,\"head\":\"head-after\",\"work\":null,\"clean\":true}}\n"
            ),
        )
        .unwrap();
        append_event(
            &path,
            &Event::new(EventKind::Wake {
                wake: "user".into(),
                text: Some("after".into()),
                brief: None,
            }),
        )
        .unwrap();
        append_event(
            &path,
            &Event::new(EventKind::User {
                text: "after".into(),
                attachments: vec![],
                channel: String::new(),
                device: String::new(),
            }),
        )
        .unwrap();
        append_event(
            &path,
            &Event::new(EventKind::TurnComplete {
                usage: None,
                model: None,
            }),
        )
        .unwrap();
        let events = load_transcript(&path).unwrap();
        let cps = checkpoints(&layout.dir);
        assert_eq!(cps.len(), 1, "only the post-roll checkpoint: {cps:?}");
        // `turn 1` of the fresh file is the post-roll turn, and resolves
        // to its own checkpoint — not head0, the project's first.
        let cp = resolve(&events, &cps, Target::Turn(1)).unwrap();
        assert_eq!(cp.head, "head-after");
        // A line of the old file no longer names anything here.
        assert!(resolve(&events, &cps, Target::Line(0)).is_err());
        assert!(resolve(&events, &cps, Target::Back(2)).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// qal-j20's second fix: a rewind's cut takes the cut turns' tree
    /// sidecars with their records, so no later turn at the same line
    /// inherits a cut turn's tree.
    #[test]
    fn a_cut_removes_the_cut_turns_sidecars_and_keeps_the_kept_ones() {
        let dir = std::env::temp_dir().join(format!("arbos-cut-sidecars-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let place = Place::new(dir.clone());
        let layout = Layout::new(&place, "root");
        std::fs::create_dir_all(layout.dir.join("checkpoints.d")).unwrap();
        std::fs::write(layout.agent_md(), "# root\n").unwrap();
        let path = layout.transcript();
        let mut cps = String::new();
        for i in 0..3u64 {
            let at = load_transcript(&path).unwrap().len() as u64;
            cps.push_str(&format!(
                "{{\"line\":{at},\"ts\":{i},\"head\":\"h{i}\",\"work\":null,\"clean\":true}}\n"
            ));
            std::fs::write(layout.dir.join(format!("checkpoints.d/{at}.json")), "{}").unwrap();
            for kind in [
                EventKind::Wake {
                    wake: "user".into(),
                    text: Some(format!("ask {i}")),
                    brief: None,
                },
                EventKind::User {
                    text: format!("ask {i}"),
                    attachments: vec![],
                    channel: String::new(),
                    device: String::new(),
                },
                EventKind::TurnComplete {
                    usage: None,
                    model: None,
                },
            ] {
                append_event(&path, &Event::new(kind)).unwrap();
            }
        }
        std::fs::write(layout.dir.join("checkpoints.jsonl"), &cps).unwrap();
        let before: Vec<_> = checkpoints(&layout.dir);
        assert_eq!(before.len(), 3);
        // Rewind to turn 2: turns 2 and 3 are cut; turn 1 is kept.
        cut(&place, "root", Target::Turn(2)).unwrap();
        assert!(
            layout
                .dir
                .join(format!("checkpoints.d/{}.json", before[0].line))
                .exists(),
            "the kept turn's sidecar stays"
        );
        assert!(
            !layout
                .dir
                .join(format!("checkpoints.d/{}.json", before[1].line))
                .exists(),
            "the target's sidecar goes"
        );
        assert!(
            !layout
                .dir
                .join(format!("checkpoints.d/{}.json", before[2].line))
                .exists(),
            "the later cut turn's sidecar goes"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
