//! File kernel types and the place store.
//!
//! Five types: [`Place`], [`Agent`], [`Page`], [`Event`], [`Node`].
//! A [`Wake`] is derived from a node whose moment came.
//! The tree is the directory. The log is JSONL.

mod agent;
pub mod agent_def;
pub mod chattitle;
pub mod cloudsync;
pub mod containment;
pub mod correction;
pub mod envsafe;
mod event;
pub mod files;
pub mod host;
pub mod hub;
pub mod inbox;
mod lock;
pub mod machines;
pub mod models;
pub mod notes;
pub mod notify;
mod page;
mod place;
pub mod project;
pub mod protocol;
pub mod prs;
pub mod redact;
pub mod remote_kernel;
pub mod skills;
pub mod spend;
pub mod status;
pub mod store;
pub mod subscription;
pub mod text;
pub mod waiting;
mod wake;
pub mod wire;

pub use agent::validate_id;
pub use agent::{ALL_TOOLS, Agent, AgentId, Mode};
pub use agent_def::{AgentDef, find_def, load_defs};
pub use event::{DIGEST_CHARS, Event, EventKind, ToolRec, Usage, tool_digest};
pub use files::{
    Layout, ROOT_ID, TranscriptTail, agent_exists, append_event, append_events, bootstrap,
    create_chat, lineage, list_agents, load_agent, load_transcript, needs_serve, read_focus,
    subtree, unlisted_agent_dirs, validate_focus, write_focus,
};
pub use host::{Host, HostConfig, KeySource, ProviderKind};
pub use hub::{HubConfig, HubFrame, MachineInfo, MeshTarget, ProjectInfo, RegistrantKind};
pub use lock::PlaceLock;
pub use machines::{Machine, Machines};
/// A row id in the window's plan frame (inbox file, subscription, notes
/// item): see `wire::PlanNode`.
pub type NodeId = u64;
pub use page::{Page, PageKind};
pub use place::Place;
pub use project::{ArchivedChild, archived_children};
pub use prs::{PrRec, load_prs, record_pr};
pub use skills::{Skill, load_skills, slash_skill};
pub use wake::{Wake, WakeKind};

/// `~/.config/arbos` (or `$XDG_CONFIG_HOME/arbos`): the host's own files.
pub fn host_dir() -> std::path::PathBuf {
    if let Some(base) = std::env::var_os("XDG_CONFIG_HOME") {
        return std::path::PathBuf::from(base).join("arbos");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return std::path::PathBuf::from(home).join(".config").join("arbos");
    }
    std::path::PathBuf::from(".arbos-host")
}

/// Whether a message is a bare control word — `stop`, `cancel`, `halt`,
/// `wait`, `pause` — the whole message, any case, trailing punctuation
/// allowed. Typed while a turn runs it means "interrupt", never a prompt.
pub fn is_stop_word(text: &str) -> bool {
    let word = text
        .trim()
        .trim_end_matches(['.', '!', '…', ',', ';'])
        .trim()
        .to_ascii_lowercase();
    matches!(
        word.as_str(),
        "stop" | "cancel" | "halt" | "wait" | "pause" | "stop it" | "stop now" | "cancel that"
    )
}

/// Env var that moves the clock: `ARBOS_NOW=2026-09-13T09:00:00Z` (or unix
/// millis). The process's clock reads that instant at start and runs
/// forward from it, so crons and `after` nodes in a fixture fire on cue
/// while timeouts and elapsed times still make sense. Every timestamp the
/// kernel writes comes from [`now_ms`], so they all shift together.
pub const NOW_ENV: &str = "ARBOS_NOW";

pub mod binary_identity {
    //! Whether the file this process was started from is still the file
    //! it is running.
    //!
    //! Two ways it can stop being: the file is **gone** (unlinked or
    //! moved — on Linux `/proc/self/exe` then reads `… (deleted)`, and
    //! the path no longer exists), or it is **replaced** by a new file at
    //! the same path, which is what an update does (stage, then rename
    //! over). Linux shows the second as the first, because `/proc` names
    //! the inode; macOS does not — `current_exe()` there is the start
    //! *path*, and the path exists, holding the new file. A check on the
    //! path alone would say "not gone" on a Mac right after the update
    //! that made the running image stale, which is the one moment the
    //! answer matters. So the file's identity — device and inode, with
    //! size and mtime beside them — is taken at start and compared live.

    use std::path::Path;
    use std::sync::OnceLock;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Identity {
        dev: u64,
        ino: u64,
        len: u64,
        mtime_ms: i128,
    }

    /// The path this process was started from and that file's identity,
    /// both taken at start. The path, not `current_exe()` later: when an
    /// update renames the whole `Arbos.app` directory to a backup, the
    /// running kernel's inode travels with it and `current_exe()` names
    /// the *backup* — a real file, identical to the one at start — while
    /// the path the kernel was started from now holds the new build. The
    /// question is what is at that path now.
    static AT_START: OnceLock<Option<(std::path::PathBuf, Identity)>> = OnceLock::new();

    /// Read `path`'s identity now.
    pub fn of(path: &Path) -> Option<Identity> {
        let m = std::fs::metadata(path).ok()?;
        #[cfg(unix)]
        let (dev, ino) = {
            use std::os::unix::fs::MetadataExt;
            (m.dev(), m.ino())
        };
        #[cfg(not(unix))]
        let (dev, ino) = (0, 0);
        let mtime_ms = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i128)
            .unwrap_or(-1);
        Some(Identity {
            dev,
            ino,
            len: m.len(),
            mtime_ms,
        })
    }

    /// Take this process's own file's identity. Called once at start,
    /// before anything can replace the file; a later first call takes
    /// whatever is there then, which is the best a late start can do.
    pub fn remember_start() {
        let _ = AT_START.get_or_init(|| {
            let p = std::env::current_exe().ok()?;
            let id = of(&p)?;
            Some((p, id))
        });
    }

    /// The path this process was started from, as remembered at start.
    pub fn start_path() -> Option<std::path::PathBuf> {
        remember_start();
        AT_START
            .get()
            .and_then(|s| s.as_ref().map(|(p, _)| p.clone()))
    }

    /// Whether the file at `path` is not the one recorded as `start`:
    /// gone, or a different file (device/inode, size or mtime changed).
    pub fn replaced(start: Option<Identity>, path: &Path) -> bool {
        match (start, of(path)) {
            (_, None) => true,
            (None, Some(_)) => false,
            (Some(a), Some(b)) => a != b,
        }
    }

    /// True when this process no longer runs the file at its own path:
    /// the file was unlinked or moved (Linux: `(deleted)`; the path is
    /// gone), or replaced at the same path (any platform: its identity
    /// differs from the one taken at start). Computed live at every use.
    /// Seven such processes on two machines ran for up to four days
    /// looking healthy from outside (mesh sweep, 2026-09-17).
    pub fn gone() -> bool {
        remember_start();
        let Ok(now) = std::env::current_exe() else {
            return true;
        };
        // Linux names the unlinked inode "… (deleted)": gone whatever a
        // same-named file says.
        if now.to_string_lossy().ends_with(" (deleted)") {
            return true;
        }
        match AT_START.get().and_then(|s| s.as_ref()) {
            // What is at the start path *now* against what was there at
            // start — not where this inode has been moved to.
            Some((start_path, start_id)) => replaced(Some(*start_id), start_path),
            // No identity taken at start: only absence can be known.
            None => !now.exists(),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_file_replaced_at_the_same_path_reads_as_replaced_and_an_unchanged_one_does_not() {
            let dir = std::env::temp_dir().join(format!("arbos-binid-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let bin = dir.join("kernel");
            std::fs::write(&bin, b"old image").unwrap();
            let start = of(&bin);
            assert!(start.is_some());
            assert!(!replaced(start, &bin), "unchanged");
            // The update: stage, then rename over the same path.
            let staged = dir.join("kernel.new");
            std::fs::write(&staged, b"new image!").unwrap();
            std::fs::rename(&staged, &bin).unwrap();
            assert!(
                bin.exists(),
                "the path still exists — a path check would say not gone"
            );
            assert!(replaced(start, &bin), "a different file at the same path");
            // Moved away: gone.
            std::fs::rename(&bin, dir.join("kernel.old")).unwrap();
            assert!(replaced(start, &bin));
            // No start identity (a late first call): only absence counts.
            assert!(!replaced(None, &dir.join("kernel.old")));
            assert!(replaced(None, &bin));
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn this_test_binary_is_not_gone() {
            assert!(!gone());
        }

        /// The app's update renames the whole directory to a backup: the
        /// running inode travels, so its own current path is a real,
        /// unchanged file — and the start path holds a new build. Judged
        /// by the start path, that is replaced; judged by where the inode
        /// went, it would read as fine for ever.
        #[test]
        fn a_directory_renamed_to_a_backup_reads_as_replaced_by_the_start_path() {
            let dir = std::env::temp_dir().join(format!("arbos-binmove-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let app = dir.join("Arbos.app");
            std::fs::create_dir_all(&app).unwrap();
            let start_path = app.join("kernel");
            std::fs::write(&start_path, b"old image").unwrap();
            let start_id = of(&start_path).unwrap();
            // The update: the directory becomes the backup, a new one takes
            // its place with a new build at the same relative path.
            std::fs::rename(&app, dir.join("Arbos.app.backup")).unwrap();
            std::fs::create_dir_all(&app).unwrap();
            std::fs::write(&start_path, b"new image!").unwrap();
            let moved_inode_path = dir.join("Arbos.app.backup").join("kernel");
            assert_eq!(
                of(&moved_inode_path),
                Some(start_id),
                "the inode travelled unchanged"
            );
            assert!(
                !replaced(Some(start_id), &moved_inode_path),
                "by its own current path: fine"
            );
            assert!(
                replaced(Some(start_id), &start_path),
                "by the start path: replaced"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

/// See [`binary_identity::gone`].
pub fn binary_gone() -> bool {
    binary_identity::gone()
}

/// Unix millis, on the shifted clock when `ARBOS_NOW` is set.
pub fn now_ms() -> i64 {
    real_now_ms() + clock_offset_ms()
}

fn real_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `ARBOS_NOW` minus the real time when it was first read; zero without it.
/// An unreadable value is zero too, but says so once on stderr rather than
/// failing every caller.
pub fn clock_offset_ms() -> i64 {
    static OFFSET: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *OFFSET.get_or_init(|| {
        let Some(raw) = std::env::var(NOW_ENV).ok().filter(|s| !s.trim().is_empty()) else {
            return 0;
        };
        match parse_instant_ms(raw.trim()) {
            Some(target) => target - real_now_ms(),
            None => {
                eprintln!("{NOW_ENV}={raw:?}: not a time (want 2026-09-13T09:00:00Z or unix millis); clock unchanged");
                0
            }
        }
    })
}

/// `2026-09-13T09:00:00Z`, `2026-09-13T09:00:00.250Z`, `2026-09-13 09:00`,
/// `2026-09-13` (midnight), or unix millis/seconds. UTC only.
pub fn parse_instant_ms(s: &str) -> Option<i64> {
    if let Ok(n) = s.parse::<i64>() {
        // Seconds until the year 33658; anything larger is millis.
        return Some(if n.abs() < 100_000_000_000 {
            n * 1000
        } else {
            n
        });
    }
    let s = s.trim_end_matches('Z').trim_end_matches("+00:00");
    let (date, time) = match s.split_once(['T', ' ']) {
        Some((d, t)) => (d, t),
        None => (s, "00:00:00"),
    };
    let mut dp = date.split('-');
    let y: i64 = dp.next()?.parse().ok()?;
    let m: i64 = dp.next()?.parse().ok()?;
    let d: i64 = dp.next()?.parse().ok()?;
    if dp.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let mut tp = time.split(':');
    let hh: i64 = tp.next()?.parse().ok()?;
    let mm: i64 = tp.next().unwrap_or("0").parse().ok()?;
    let sec_part = tp.next().unwrap_or("0");
    let (ss, frac_ms) = match sec_part.split_once('.') {
        Some((a, f)) => {
            let digits: String = f.chars().take(3).collect();
            let ms: i64 = format!("{digits:0<3}").parse().ok()?;
            (a.parse::<i64>().ok()?, ms)
        }
        None => (sec_part.parse::<i64>().ok()?, 0),
    };
    if hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    let days = days_from_civil(y, m, d);
    Some((((days * 24 + hh) * 60 + mm) * 60 + ss) * 1000 + frac_ms)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
