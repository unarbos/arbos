//! Workspace disk headroom (#1641). A user's `/work` is small (2 GiB on a D89 host sandbox)
//! and one `pip install` or toolchain can fill it; when it is full, agentd's own writes (pi
//! session JSONL, agentd.db, the prompt files) fail too and every chat dies with a raw
//! `No space left on device (os error 28)`.
//!
//! Three defences, all inside agentd so they work on every box provider:
//!
//! 1. **Ballast.** At boot agentd (root) pre-allocates `<work_root>/.arbos/ballast` (128 MiB by
//!    default, `ARBOS_DISK_BALLAST_MB`). Workspace users cannot remove it (`/work` is 711 and
//!    `/work/.arbos` is root's). When the disk is full the ballast is released, which gives
//!    agentd and one recovery turn the room they need; it is re-armed once space is back.
//!    (ext4 root-reserved blocks do not work under gVisor: the sentry writes as host root.)
//! 2. **Gate.** Before each turn the free space decides: plenty → nothing; ≥ 85 % used → the
//!    working row says so and the prompt carries a short note; below the floor → ballast
//!    released, the turn runs in recovery mode with a note that tells the agent to free space
//!    first; nothing left even after that → no model call, a plain line that names the biggest
//!    folders and how to clean up. "clean up" / "free space" from the user is handled here.
//! 3. **Plain words.** Any error text that is really ENOSPC becomes the same plain line.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Value};

/// Used share of the disk above which the working row and the prompt mention it.
pub const WARN_PCT: u64 = 85;
/// Below this much free space (MiB) a turn runs in recovery mode: ballast released, the agent
/// is told to free space first.
pub const FLOOR_MB: u64 = 100;
/// Below this (MiB), even after releasing the ballast, no turn runs at all.
pub const HARD_MB: u64 = 24;
const DEFAULT_BALLAST_MB: u64 = 128;
const MIB: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DiskStatus {
    pub total_mb: u64,
    pub avail_mb: u64,
    pub used_pct: u64,
    /// Free inodes (file slots), when the filesystem reports them. A 2 GiB work image has
    /// 131 072; one dereferenced copy of pi's package tree is 18 000 of them (2026-10-26: the
    /// lt-eval box refused every write at "87 % full" with 240 MB free and 0 inodes).
    #[serde(default)]
    pub inodes_avail: Option<u64>,
}

/// Fewer free inodes than this and the next session file, log or blob cannot be created,
/// whatever the byte count says: the gate treats it as a full disk.
pub const INODE_FLOOR: u64 = 1024;

impl DiskStatus {
    pub fn new(total_mb: u64, avail_mb: u64) -> Self {
        let used_pct = (total_mb.saturating_sub(avail_mb) * 100)
            .checked_div(total_mb)
            .unwrap_or(0);
        Self {
            total_mb,
            avail_mb,
            used_pct,
            inodes_avail: None,
        }
    }

    pub fn with_inodes(mut self, avail: u64) -> Self {
        self.inodes_avail = Some(avail);
        self
    }

    /// No file can be created even though bytes are free.
    pub fn inodes_exhausted(&self) -> bool {
        self.inodes_avail.is_some_and(|n| n < INODE_FLOOR)
    }

    /// "1.9 GB of 2.0 GB used (96 %)" — or, when the file slots ran out first, says so.
    pub fn human(&self) -> String {
        let bytes = format!(
            "{} of {} used ({} %)",
            fmt_mb(self.total_mb.saturating_sub(self.avail_mb)),
            fmt_mb(self.total_mb),
            self.used_pct
        );
        if self.inodes_exhausted() {
            format!(
                "{bytes}; no free file slots left — too many small files, {} MB free cannot be used",
                self.avail_mb
            )
        } else {
            bytes
        }
    }
}

pub fn fmt_mb(mb: u64) -> String {
    if mb >= 1024 {
        format!("{:.1} GB", mb as f64 / 1024.0)
    } else {
        format!("{mb} MB")
    }
}

// statvfs field widths differ per libc (u32 on macOS, u64 on Linux).
#[allow(clippy::unnecessary_cast)]
pub fn status(path: &Path) -> Option<DiskStatus> {
    let s = nix::sys::statvfs::statvfs(path).ok()?;
    let frag = s.fragment_size() as u64;
    let total = s.blocks() as u64 * frag / MIB;
    // what a non-root process can still write
    let avail = s.blocks_available() as u64 * frag / MIB;
    let st = DiskStatus::new(total, avail);
    // 0 total inodes = the filesystem does not count them (btrfs, some 9p mounts): no verdict.
    if s.files() as u64 > 0 {
        return Some(st.with_inodes(s.files_available() as u64));
    }
    Some(st)
}

/// What the gate decided for one turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    Ok,
    /// Disk is filling: say so, carry on.
    Warn(DiskStatus),
    /// Disk is (nearly) full: ballast released, run the turn with a "free space first" note.
    Recover(DiskStatus),
    /// Nothing can run; answer with the plain line (or clean up when asked).
    Blocked(DiskStatus),
}

/// Pure decision: `after_release` is the status after the ballast was dropped (None when there
/// was no ballast to drop).
pub fn decide(before: DiskStatus, after_release: Option<DiskStatus>) -> Gate {
    // Out of inodes: releasing the ballast frees one slot, not thousands; the turn cannot run.
    if before.inodes_exhausted() {
        return Gate::Blocked(after_release.unwrap_or(before));
    }
    if before.avail_mb >= FLOOR_MB {
        return if before.used_pct >= WARN_PCT {
            Gate::Warn(before)
        } else {
            Gate::Ok
        };
    }
    let now = after_release.unwrap_or(before);
    if now.avail_mb >= HARD_MB {
        Gate::Recover(now)
    } else {
        Gate::Blocked(now)
    }
}

// ---- ballast ---------------------------------------------------------------------------------

pub fn ballast_mb() -> u64 {
    std::env::var("ARBOS_DISK_BALLAST_MB")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(DEFAULT_BALLAST_MB)
}

fn ballast_path(work_root: &Path) -> PathBuf {
    work_root.join(".arbos").join("ballast")
}

pub fn ballast_present(work_root: &Path) -> bool {
    ballast_path(work_root).exists()
}

/// Create the ballast when it is missing and there is room for it plus the floor. Returns
/// whether it exists afterwards.
pub fn ensure_ballast(work_root: &Path) -> bool {
    let mb = ballast_mb();
    if mb == 0 {
        return false;
    }
    let p = ballast_path(work_root);
    if p.exists() {
        return true;
    }
    let Some(st) = status(work_root) else {
        return false;
    };
    if st.avail_mb < mb + FLOOR_MB {
        return false;
    }
    let _ = std::fs::create_dir_all(work_root.join(".arbos"));
    match write_ballast(&p, mb) {
        Ok(()) => {
            tracing::info!(path = %p.display(), mb, "disk ballast armed");
            true
        }
        Err(e) => {
            tracing::warn!(error = %e, "disk ballast could not be written");
            let _ = std::fs::remove_file(&p);
            false
        }
    }
}

fn write_ballast(p: &Path, mb: u64) -> Result<()> {
    use std::io::Write;
    // Real bytes, not a sparse file: a hole reserves nothing.
    let f = std::fs::File::create(p)?;
    let mut w = std::io::BufWriter::new(f);
    let chunk = vec![0u8; MIB as usize];
    for _ in 0..mb {
        w.write_all(&chunk)?;
    }
    w.flush()?;
    w.into_inner()
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .sync_all()?;
    Ok(())
}

/// Drop the ballast. Returns true when there was one.
pub fn release_ballast(work_root: &Path) -> bool {
    let p = ballast_path(work_root);
    if p.exists() && std::fs::remove_file(&p).is_ok() {
        tracing::warn!(path = %p.display(), "disk ballast released: workspace disk is full");
        return true;
    }
    false
}

/// The gate for one turn: measures, releases the ballast when needed, re-arms it when there
/// is room again.
pub fn gate(work_root: &Path) -> Gate {
    let Some(before) = status(work_root) else {
        return Gate::Ok;
    };
    if before.avail_mb >= FLOOR_MB {
        // Room again (the agent cleaned up, the user deleted files): put the ballast back
        // when that still leaves the floor.
        ensure_ballast(work_root);
        return decide(status(work_root).unwrap_or(before), None);
    }
    let after = if release_ballast(work_root) {
        status(work_root)
    } else {
        None
    };
    decide(before, after)
}

// ---- what is big, what can go --------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct DirSize {
    pub path: String,
    pub mb: u64,
}

const WALK_FILE_CAP: usize = 400_000;

fn dir_size(p: &Path, budget: &mut usize) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            if *budget == 0 {
                return total;
            }
            *budget -= 1;
            let Ok(md) = e.metadata() else {
                continue;
            };
            if md.file_type().is_symlink() {
                continue;
            }
            if md.is_dir() {
                stack.push(e.path());
            } else {
                use std::os::unix::fs::MetadataExt;
                total += md.blocks() * 512;
            }
        }
    }
    total
}

/// The biggest folders a user can act on: each workspace's own files (first level under
/// `/work/<project>`) and each workspace home's first level (`~/.cache`, `~/.cargo`, …), plus
/// pi sessions. Sorted by size, at most `n`, folders under 1 MB dropped.
pub fn biggest_dirs(work_root: &Path, n: usize) -> Vec<DirSize> {
    let mut out: Vec<DirSize> = Vec::new();
    let mut budget = WALK_FILE_CAP;
    let Ok(projects) = std::fs::read_dir(work_root) else {
        return out;
    };
    for p in projects.flatten() {
        let name = p.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || !p.path().is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(p.path()) else {
            continue;
        };
        for e in entries.flatten() {
            let en = e.file_name().to_string_lossy().to_string();
            if en == ".arbos" {
                let home = e.path().join("home");
                if let Ok(hd) = std::fs::read_dir(&home) {
                    for h in hd.flatten() {
                        if h.path().is_dir() {
                            let mb = dir_size(&h.path(), &mut budget) / MIB;
                            out.push(DirSize {
                                path: format!("{name}/~/{}", h.file_name().to_string_lossy()),
                                mb,
                            });
                        }
                    }
                }
                let sessions = e.path().join("pi").join("sessions");
                if sessions.is_dir() {
                    let mb = dir_size(&sessions, &mut budget) / MIB;
                    out.push(DirSize {
                        path: format!("{name}/.arbos/pi/sessions"),
                        mb,
                    });
                }
                continue;
            }
            if e.path().is_dir() {
                let mb = dir_size(&e.path(), &mut budget) / MIB;
                out.push(DirSize {
                    path: format!("{name}/{en}"),
                    mb,
                });
            }
        }
    }
    out.retain(|d| d.mb >= 1);
    out.sort_by_key(|d| std::cmp::Reverse(d.mb));
    out.truncate(n);
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PruneLevel {
    /// Package and download caches: always safe, rebuilt on demand.
    Caches,
    /// Caches plus installed toolchains and build outputs (`~/.rustup`, `~/.local/lib/python*`,
    /// `<project>/target`, `node_modules`): re-installable, slower to get back.
    All,
}

impl PruneLevel {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "caches" | "cache" | "safe" => Some(PruneLevel::Caches),
            "all" | "toolchains" | "aggressive" => Some(PruneLevel::All),
            _ => None,
        }
    }
}

/// Relative to a workspace home.
const HOME_CACHES: &[&str] = &[
    ".cache",
    ".npm/_cacache",
    ".cargo/registry",
    ".cargo/git",
    ".bun/install/cache",
    ".yarn/cache",
    ".gradle/caches",
    ".m2/repository",
    "go/pkg/mod/cache",
];
const HOME_TOOLCHAINS: &[&str] = &[".rustup", ".local/lib", ".local/share/uv", ".pyenv", ".nvm"];
/// Relative to `/work/<project>`, first level only.
const PROJECT_BUILD_OUTPUTS: &[&str] =
    &["target", "node_modules", ".venv", "venv", "dist", "build"];

#[derive(Debug, Clone, Default, Serialize)]
pub struct PruneReport {
    pub removed: Vec<DirSize>,
    pub freed_mb: u64,
    pub before: Option<DiskStatus>,
    pub after: Option<DiskStatus>,
}

/// Remove the known regenerable folders in every workspace. Never touches user files outside
/// those lists, never `.arbos/agentd`, pi sessions, memory or secrets.
pub fn prune(work_root: &Path, level: PruneLevel) -> PruneReport {
    let mut rep = PruneReport {
        before: status(work_root),
        ..Default::default()
    };
    let Ok(projects) = std::fs::read_dir(work_root) else {
        return rep;
    };
    let mut budget = WALK_FILE_CAP;
    for p in projects.flatten() {
        let name = p.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || !p.path().is_dir() {
            continue;
        }
        let home = p.path().join(".arbos").join("home");
        let mut targets: Vec<(String, PathBuf)> = HOME_CACHES
            .iter()
            .map(|r| (format!("{name}/~/{r}"), home.join(r)))
            .collect();
        if level == PruneLevel::All {
            targets.extend(
                HOME_TOOLCHAINS
                    .iter()
                    .map(|r| (format!("{name}/~/{r}"), home.join(r))),
            );
            targets.extend(
                PROJECT_BUILD_OUTPUTS
                    .iter()
                    .map(|r| (format!("{name}/{r}"), p.path().join(r))),
            );
        }
        for (label, path) in targets {
            if !path.is_dir() || path.is_symlink() {
                continue;
            }
            let mb = dir_size(&path, &mut budget) / MIB;
            if std::fs::remove_dir_all(&path).is_ok() {
                // Keep the cache folder itself so tools do not fail on a missing parent.
                let _ = std::fs::create_dir_all(&path);
                if let Ok(md) = std::fs::metadata(&home) {
                    use std::os::unix::fs::MetadataExt;
                    let _ = nix::unistd::chown(
                        &path,
                        Some(nix::unistd::Uid::from_raw(md.uid())),
                        Some(nix::unistd::Gid::from_raw(md.gid())),
                    );
                }
                rep.freed_mb += mb;
                rep.removed.push(DirSize { path: label, mb });
            }
        }
    }
    rep.removed.sort_by_key(|d| std::cmp::Reverse(d.mb));
    rep.after = status(work_root);
    if rep
        .after
        .is_some_and(|a| a.avail_mb >= FLOOR_MB + ballast_mb())
    {
        ensure_ballast(work_root);
        rep.after = status(work_root);
    }
    rep
}

// ---- words -----------------------------------------------------------------------------------

/// Is this error really "the disk is full"?
pub fn is_enospc(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    t.contains("no space left on device") || t.contains("enospc") || t.contains("os error 28")
}

/// Did the user ask us to free space?
pub fn is_cleanup_request(text: &str) -> bool {
    let t = text.trim().to_ascii_lowercase();
    t.len() <= 60
        && (t.starts_with("clean up")
            || t.starts_with("cleanup")
            || t.starts_with("free space")
            || t.starts_with("free up space")
            || t.starts_with("free disk")
            || t.starts_with("make space")
            || t.starts_with("make room"))
}

fn dirs_line(dirs: &[DirSize]) -> String {
    if dirs.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = dirs
        .iter()
        .take(4)
        .map(|d| format!("{} ({})", d.path, fmt_mb(d.mb)))
        .collect();
    format!(" The biggest folders are {}.", parts.join(", "))
}

/// The one plain line for a full disk, with the biggest folders and the way out.
pub fn full_line(st: &DiskStatus, dirs: &[DirSize]) -> String {
    format!(
        "Your workspace disk is full ({}).{} Say *clean up* and I will remove caches and toolchains, or delete files yourself; then send your message again.",
        st.human(),
        dirs_line(dirs)
    )
}

/// Replace a raw ENOSPC error with the plain line; anything else passes through.
pub fn friendly(text: &str, work_root: &Path) -> String {
    if !is_enospc(text) {
        return text.to_owned();
    }
    let st = status(work_root).unwrap_or(DiskStatus::new(0, 0));
    full_line(&st, &biggest_dirs(work_root, 4))
}

/// Note for the prompt while the disk is filling (≥ 85 %).
pub fn warn_note(st: &DiskStatus) -> String {
    format!(
        "[Note: this computer's disk is {} % full ({} free of {}). Mention it briefly if the task writes files; the free_space tool reports and clears caches.]",
        st.used_pct,
        fmt_mb(st.avail_mb),
        fmt_mb(st.total_mb)
    )
}

/// Note for the prompt in recovery mode (floor crossed, ballast released).
pub fn recover_note(st: &DiskStatus, dirs: &[DirSize]) -> String {
    format!(
        "[Important: this computer's disk is full ({}; only an emergency reserve is free).{} Before anything else, free space: call free_space with level \"caches\" (or \"all\" for toolchains and build outputs) or delete files, tell the user what you removed, and do not install or download anything until free space is above 200 MB.]",
        st.human(),
        dirs_line(dirs)
    )
}

/// "Working… · disk 91 % full".
pub fn working_text(gate: &Gate) -> Option<String> {
    match gate {
        Gate::Ok => None,
        Gate::Warn(st) => Some(format!("Working… · disk {} % full", st.used_pct)),
        Gate::Recover(_) => Some("Disk full — freeing space first…".into()),
        Gate::Blocked(_) => Some("Disk full".into()),
    }
}

/// MCP tool `free_space` (every harness): report, or prune at a level, and report.
pub fn tool_specs() -> Vec<Value> {
    vec![json!({
        "name": "free_space",
        "description": "This computer's disk: free/used, the biggest folders per workspace, and the emergency reserve. level \"caches\" removes package and download caches (~/.cache, cargo registry, npm cache, …); level \"all\" also removes installed toolchains and build outputs (~/.rustup, ~/.local/lib, target/, node_modules, .venv). Never touches the user's own files. Use it when the disk is over 85 % full or a write failed with no space left.",
        "inputSchema": { "type": "object", "properties": { "level": { "type": "string", "enum": ["report", "caches", "all"] } } }
    })]
}

pub async fn free_space_tool(a: &std::sync::Arc<crate::agent::Agent>, args: &Value) -> Value {
    let root = a.work_root();
    let level = args.get("level").and_then(Value::as_str).map(str::to_owned);
    let out = tokio::task::spawn_blocking(move || match level.as_deref() {
        None | Some("report") => Ok(report_json(&root, None)),
        Some(l) => match PruneLevel::parse(l) {
            Some(level) => {
                let rep = prune(&root, level);
                Ok(report_json(&root, Some(&rep)))
            }
            None => Err(format!("unknown level {l}: use report, caches or all")),
        },
    })
    .await;
    match out {
        Ok(Ok(v)) => crate::mcp::text_result(v.to_string()),
        Ok(Err(e)) => crate::mcp::err_result(e),
        Err(e) => crate::mcp::err_result(e),
    }
}

pub fn report_json(work_root: &Path, rep: Option<&PruneReport>) -> Value {
    json!({
        "disk": status(work_root),
        "biggest": biggest_dirs(work_root, 8),
        "ballast_mb": if ballast_present(work_root) { ballast_mb() } else { 0 },
        "pruned": rep,
    })
}

/// Clean-up handled without the model (the user typed "clean up" on a full disk).
pub fn cleanup_line(rep: &PruneReport) -> String {
    let after = rep
        .after
        .map(|a| format!(" {} free now.", fmt_mb(a.avail_mb)))
        .unwrap_or_default();
    if rep.removed.is_empty() {
        return format!(
            "I found no caches or toolchains to remove.{after} The biggest folders are listed above; delete what you no longer need, then send your message again."
        );
    }
    let items: Vec<String> = rep
        .removed
        .iter()
        .take(6)
        .map(|d| format!("{} ({})", d.path, fmt_mb(d.mb)))
        .collect();
    format!(
        "Removed {} of caches and toolchains: {}.{after} Send your message again.",
        fmt_mb(rep.freed_mb),
        items.join(", ")
    )
}

/// The periodic re-arm: every minute, when there is room, make sure the ballast exists.
pub fn spawn_ballast_keeper(work_root: PathBuf) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tick.tick().await;
            let root = work_root.clone();
            let _ = tokio::task::spawn_blocking(move || ensure_ballast(&root)).await;
        }
    });
}

pub fn boot(work_root: &Path) -> Result<()> {
    std::fs::create_dir_all(work_root.join(".arbos")).context("create /work/.arbos")?;
    let armed = ensure_ballast(work_root);
    if let Some(st) = status(work_root) {
        tracing::info!(
            total_mb = st.total_mb,
            avail_mb = st.avail_mb,
            used_pct = st.used_pct,
            ballast = armed,
            "workspace disk"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("arbos-disk-{}", arbos_core::ids::ulid()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn gate_thresholds() {
        let ok = DiskStatus::new(2048, 1000);
        assert_eq!(decide(ok, None), Gate::Ok);
        let warn = DiskStatus::new(2048, 200);
        assert_eq!(warn.used_pct, 90);
        assert_eq!(decide(warn, None), Gate::Warn(warn));
        let full = DiskStatus::new(2048, 10);
        let after = DiskStatus::new(2048, 138);
        assert_eq!(decide(full, Some(after)), Gate::Recover(after));
        assert_eq!(
            decide(full, None),
            Gate::Blocked(full),
            "no ballast left: blocked"
        );
        let hard = DiskStatus::new(2048, 5);
        assert_eq!(decide(full, Some(hard)), Gate::Blocked(hard));
        // lt-eval box, 2026-10-26 02:07 UTC: 244 MB free, 0 of 131 072 inodes free — every
        // create failed with ENOSPC while the gate said "87 % full, carry on".
        let no_inodes = DiskStatus::new(2048, 244).with_inodes(0);
        assert!(no_inodes.inodes_exhausted());
        assert_eq!(decide(no_inodes, None), Gate::Blocked(no_inodes));
        assert!(no_inodes.human().contains("no free file slots"));
        let plenty = DiskStatus::new(2048, 1000).with_inodes(90_000);
        assert_eq!(decide(plenty, None), Gate::Ok);
        assert!(!plenty.human().contains("file slots"));
    }

    #[test]
    fn enospc_and_cleanup_words() {
        assert!(is_enospc("No space left on device (os error 28)"));
        assert!(is_enospc("ENOSPC: no space left on device, write"));
        assert!(is_enospc("switch_session: ENOSPC"));
        assert!(!is_enospc("Permission denied (os error 13)"));
        assert!(is_cleanup_request("clean up"));
        assert!(is_cleanup_request("Free space please"));
        assert!(is_cleanup_request("make room"));
        assert!(!is_cleanup_request(
            "clean up my essay and make it shorter please, it is about the history of rome"
        ));
        assert!(!is_cleanup_request("hello"));
    }

    #[test]
    fn friendly_line_names_folders_and_the_way_out() {
        let st = DiskStatus::new(2048, 3);
        let dirs = vec![
            DirSize {
                path: "lane-b/~/.rustup".into(),
                mb: 593,
            },
            DirSize {
                path: "lane-b/~/.cargo".into(),
                mb: 96,
            },
        ];
        let line = full_line(&st, &dirs);
        assert!(line.contains("disk is full"));
        assert!(line.contains("lane-b/~/.rustup (593 MB)"));
        assert!(line.contains("clean up"));
        assert!(!line.contains("os error"));
        assert_eq!(friendly("fine", Path::new("/nonexistent")), "fine");
        assert!(friendly(
            "No space left on device (os error 28)",
            Path::new("/nonexistent")
        )
        .contains("disk is full"));
    }

    #[test]
    fn ballast_arms_and_releases() {
        let root = tmp();
        std::env::set_var("ARBOS_DISK_BALLAST_MB", "2");
        assert!(ensure_ballast(&root));
        let p = root.join(".arbos").join("ballast");
        assert_eq!(std::fs::metadata(&p).unwrap().len(), 2 * MIB);
        assert!(ensure_ballast(&root), "idempotent");
        assert!(release_ballast(&root));
        assert!(!p.exists());
        assert!(!release_ballast(&root));
        std::env::remove_var("ARBOS_DISK_BALLAST_MB");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn biggest_dirs_and_prune_touch_only_the_known_folders() {
        let root = tmp();
        let home = root.join("p1").join(".arbos").join("home");
        let big = |rel: &str, mb: usize| {
            let d = home.join(rel);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("blob"), vec![1u8; mb * MIB as usize]).unwrap();
        };
        big(".rustup/toolchains", 3);
        big(".cache/pip", 2);
        std::fs::create_dir_all(root.join("p1").join("src")).unwrap();
        std::fs::write(
            root.join("p1").join("src").join("main.rs"),
            vec![1u8; (MIB * 2) as usize],
        )
        .unwrap();
        std::fs::create_dir_all(root.join("p1").join(".arbos").join("agentd")).unwrap();
        std::fs::write(
            root.join("p1")
                .join(".arbos")
                .join("agentd")
                .join("agentd.db"),
            vec![1u8; MIB as usize],
        )
        .unwrap();

        let dirs = biggest_dirs(&root, 10);
        let paths: Vec<&str> = dirs.iter().map(|d| d.path.as_str()).collect();
        assert_eq!(paths[0], "p1/~/.rustup", "{paths:?}");
        assert!(paths.contains(&"p1/~/.cache"));
        assert!(paths.contains(&"p1/src"));
        assert!(
            !paths.iter().any(|p| p.contains("agentd")),
            "agentd state is not a user folder"
        );

        let rep = prune(&root, PruneLevel::Caches);
        assert_eq!(rep.removed.len(), 1);
        assert_eq!(rep.removed[0].path, "p1/~/.cache");
        assert!(
            home.join(".rustup").join("toolchains").exists(),
            "toolchains survive level caches"
        );
        assert!(home.join(".cache").is_dir(), "cache folder itself stays");
        assert!(!home.join(".cache").join("pip").exists());

        let rep = prune(&root, PruneLevel::All);
        assert!(rep.removed.iter().any(|d| d.path == "p1/~/.rustup"));
        assert!(
            root.join("p1").join("src").join("main.rs").exists(),
            "user files untouched"
        );
        assert!(root
            .join("p1")
            .join(".arbos")
            .join("agentd")
            .join("agentd.db")
            .exists());
        assert!(cleanup_line(&rep).contains("Removed"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
