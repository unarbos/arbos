//! Carrying a box's `/work` through the relay in parts (ARB-339).
//!
//! The one-piece carry (`tar` of all of `/work` → one download → one 30-s multipart upload) failed
//! three ways in production: upload timeouts and 502s from the toolbox proxy above a few hundred MB,
//! download stream resets, and the relay's own `MAX_WORK_MIB` refusal — which came *after* the
//! box's agentd had been killed, so an oversize box was restarted on every attempt (gh_268993031,
//! 3979 MiB, 2026-10-26). Now:
//!
//! 1. **Measure first.** `du -sm /work`, the free space where the pack stages its parts, the
//!    biggest single file and the target snapshot's disk size are all read while agentd keeps
//!    serving. Over the limit, or no room on the relay's spool, on the source itself or on the box
//!    the snapshot will make → a clean refusal naming the figures, nothing stopped, box untouched.
//! 2. **Quiesce, list, plan.** agentd is stopped, `/work` is listed once (`find -printf`), and the
//!    listing is cut into parts of at most `part_mib` apparent bytes, in walk order (a directory
//!    precedes its children). Every part is its own tarball (`tar --no-recursion -T list`), so a
//!    part never needs the others to extract.
//! 3. **Pack per part.** One part at a time on the source box: tar → sha256 → download (long
//!    per-part deadline, retried) → verify → spool to the relay's disk → delete on the source. The
//!    source never holds more than one part's tarball; the relay never holds more than one part in
//!    memory. The staging directory is removed when the pack ends, whether it ended well or not.
//! 4. **Unpack per part.** On the new box: the room check again, on the box itself (bytes *and*
//!    inodes, the same floor as agentd's disk gate), then per part: upload (deadline scaled to the size, retried) → sha256
//!    check on the box → extract → delete. A failed transfer resumes at the failed part, never from
//!    byte 0. At the end the carried tree is counted and summed against the plan.
//!
//! Rebuildable trees — `<project>/.arbos/pi/extensions/arbos-pi-ext/node_modules`, which the image's
//! seed holds as two symlinks and which images before 0.1.145 dereferenced into ~18 000 files /
//! 119 MB per project — are measured on every carry and, only when `ARBOS_UPGRADE_SKIP_REBUILDABLE=1`,
//! left out: agentd's `seed_state` recreates the directory and its links from `/etc/arbos/pi-seed`
//! at the first workspace start on the new image (`copy_dir` replaces a copied tree with the link).
//! Off by default.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::daytona::Toolbox;

/// Default ceiling for `/work` (MiB, as `du -sm` reports). `ARBOS_UPGRADE_MAX_WORK_MIB` overrides.
/// 16 GiB is above anything a 10 GiB Daytona disk or a 2 GiB gVisor work image can hold today;
/// the ceiling is a sanity stop, not the mechanism's limit.
pub const DEFAULT_MAX_WORK_MIB: i64 = 16 * 1024;
/// Default part size (MiB of apparent bytes before compression). `ARBOS_UPGRADE_PART_MIB` overrides.
pub const DEFAULT_PART_MIB: u64 = 256;
/// Transfer attempts per part (download and upload each).
pub const DEFAULT_ATTEMPTS: u32 = 4;
/// Smallest piece the adaptive download falls back to (bytes). A piece that still times out at
/// this size is a dead link, not a slow one.
pub const MIN_PIECE_BYTES: u64 = 4 * MIB;
/// Ceiling for a throughput-derived piece deadline, so a stalled link is noticed within the hour.
pub const PIECE_TIMEOUT_MAX: Duration = Duration::from_secs(1800);
/// Carries that may run at once (`ARBOS_UPGRADE_CONCURRENT_CARRIES`). One: on 2026-10-29 two
/// carries shared a ~0.3 MiB/s toolbox download path and both timed out on every piece.
pub const DEFAULT_CONCURRENT_CARRIES: usize = 1;
/// Same floor as `arbos_agentd::disk::INODE_FLOOR`: a destination with fewer free file slots than
/// the carried tree needs plus this is refused before anything is uploaded.
pub const INODE_FLOOR: u64 = 1024;
/// Disk the relay keeps free on its spool filesystem beyond the carry itself (MiB).
pub const SPOOL_MARGIN_MIB: u64 = 2048;
/// Room the new box keeps beyond the carried bytes and one staged part (MiB).
pub const DEST_MARGIN_MIB: u64 = 512;
/// Room the SOURCE box keeps beyond the part it is packing (MiB). The pack stages its tarballs on
/// the source's own disk, so the source needs room too — 2026-11-05, gh_268993031: part 80 was a
/// lone 357 MiB file, its tarball filled the 365 MiB that was free and `head` cutting the first
/// piece failed with ENOSPC. agentd is stopped while the pack runs, so the gate's floor is not in
/// play; this margin covers `work.list` (26 MiB on that box), the per-part list, tar's own headers
/// and gzip's worst case on incompressible data.
pub const SOURCE_MARGIN_MIB: u64 = 256;
/// What a fresh box's own files take off its disk before `/work` holds any of the carry (MiB).
/// A 10 GiB Daytona box reported 10 111 MiB free of 10 240 at its first boot (2026-11-04): the
/// image's writable layer and the filesystem's reserve. Doubled, so a bigger image still fits.
pub const FRESH_BOX_OVERHEAD_MIB: u64 = 256;

const MIB: u64 = 1024 * 1024;
/// Staging directory on both boxes (the toolbox user's home; readable by the toolbox download).
pub const STAGE_DIR: &str = "/home/daytona/arbos-upgrade";
/// The directory [`STAGE_DIR`] is made in: `df` of this says how much room the pack has. The box's
/// `/work` and `/home` are one overlay today, but the pack writes here, so this is what is measured.
pub const STAGE_PARENT: &str = "/home/daytona";
/// Relative path (from `/work`) of the tree agentd rebuilds from the image seed.
pub const REBUILDABLE_SUFFIX: &str = "/.arbos/pi/extensions/arbos-pi-ext/node_modules";

/// Knobs, read once per upgrade.
#[derive(Debug, Clone)]
pub struct CarryConfig {
    pub max_work_mib: i64,
    pub part_mib: u64,
    pub attempts: u32,
    pub skip_rebuildable: bool,
    /// How many carries may pack/unpack at the same time (see `DEFAULT_CONCURRENT_CARRIES`).
    pub concurrent_carries: usize,
    /// `<data_dir>/upgrade-spool`; one subdirectory per user while a carry runs.
    pub spool_root: PathBuf,
}

impl CarryConfig {
    pub fn from_env(data_dir: &Path) -> Self {
        fn num<T: std::str::FromStr>(name: &str, default: T) -> T {
            std::env::var(name)
                .ok()
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(default)
        }
        Self {
            max_work_mib: num("ARBOS_UPGRADE_MAX_WORK_MIB", DEFAULT_MAX_WORK_MIB),
            part_mib: num("ARBOS_UPGRADE_PART_MIB", DEFAULT_PART_MIB).max(16),
            attempts: num("ARBOS_UPGRADE_ATTEMPTS", DEFAULT_ATTEMPTS).max(1),
            skip_rebuildable: std::env::var("ARBOS_UPGRADE_SKIP_REBUILDABLE")
                .is_ok_and(|v| v == "1"),
            concurrent_carries: num(
                "ARBOS_UPGRADE_CONCURRENT_CARRIES",
                DEFAULT_CONCURRENT_CARRIES,
            )
            .max(1),
            spool_root: data_dir.join("upgrade-spool"),
        }
    }
}

// ---- pure decisions ----------------------------------------------------------------------------

/// What the measure step decided, before anything on the box is touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Carry,
    /// Still says "too large to carry": the deploy worker's skip rule matches on that phrase.
    TooLarge {
        mib: i64,
        max_mib: i64,
    },
    /// The relay's spool disk cannot hold the carry.
    NoSpoolRoom {
        need_mib: u64,
        free_mib: u64,
    },
    /// The source box cannot hold the tarball of its own biggest part.
    NoSourceRoom {
        free_mib: u64,
        need_mib: u64,
        max_part_mib: u64,
        part_mib: u64,
    },
    /// A fresh box from the target snapshot cannot hold the carry (the snapshot's disk is too
    /// small). The real `df` on the new box still has the last word; this is the same arithmetic
    /// read early enough that nothing has been stopped yet.
    NoDestinationRoom {
        work_mib: u64,
        free_mib: u64,
        need_mib: u64,
        max_part_mib: u64,
        part_mib: u64,
    },
}

impl Verdict {
    /// Every reason carries the words `too large to carry` (the relay's `REFUSED_PHRASE`) inside
    /// its first 200 characters, which is all `last_refused` and the deploy worker keep.
    pub fn reason(&self) -> Option<String> {
        match self {
            Verdict::Carry => None,
            Verdict::TooLarge { mib, max_mib } => Some(format!(
                "/work is {mib} MiB; too large to carry through the relay (limit {max_mib} MiB, ARBOS_UPGRADE_MAX_WORK_MIB)"
            )),
            Verdict::NoSpoolRoom { need_mib, free_mib } => Some(format!(
                "/work is {need_mib} MiB but the relay's spool disk has {free_mib} MiB free; too large to carry through the relay right now"
            )),
            Verdict::NoSourceRoom { free_mib, need_mib, max_part_mib, part_mib } => Some(format!(
                "the source box has {free_mib} MiB free where the pack stages parts but needs {need_mib} (biggest part {max_part_mib} + one {part_mib} MiB piece + {SOURCE_MARGIN_MIB} margin); too large to carry through the relay right now"
            )),
            Verdict::NoDestinationRoom { work_mib, free_mib, need_mib, max_part_mib, part_mib } => Some(format!(
                "/work is {work_mib} MiB and a fresh box from the target snapshot has about {free_mib} MiB free but needs {need_mib} (+ biggest part {max_part_mib}, 2x{part_mib} MiB pieces, {DEST_MARGIN_MIB} margin); too large to carry onto that snapshot"
            )),
        }
    }
}

/// Every figure the room checks compare (MiB), all of them readable while the box still serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Room {
    /// `du -sm /work` on the source.
    pub work_mib: i64,
    /// Free on the relay's spool filesystem.
    pub spool_free_mib: u64,
    /// Free on the source filesystem that holds [`STAGE_DIR`].
    pub source_free_mib: u64,
    /// The most any one part's tarball can need — see [`max_part_bound_mib`].
    pub max_part_mib: u64,
    /// Free `/work` a fresh box from the target snapshot is expected to have, from the snapshot's
    /// own disk size. `None` when Daytona does not say: then only the `df` after the create judges.
    pub target_free_mib: Option<u64>,
}

/// Peak the pack needs on the source: the biggest part's tarball, one piece cut beside it, margin.
pub fn source_need_mib(max_part_mib: u64, part_mib: u64) -> u64 {
    max_part_mib + part_mib + SOURCE_MARGIN_MIB
}

/// Peak the carry needs on the destination: every carried byte, plus the biggest part's tarball
/// staged for extraction, plus two pieces in flight (the uploaded piece and the daemon's `/tmp`
/// copy of it), plus a margin. One function so the check before the pack and the check on the new
/// box cannot drift apart.
pub fn destination_need_mib(work_mib: u64, part_mib: u64, max_part_mib: u64) -> u64 {
    work_mib + max_part_mib + 2 * part_mib + DEST_MARGIN_MIB
}

/// The most one part's tarball can need, in MiB, known before the listing: a part holds at most
/// `part_mib` apparent bytes, except a single file bigger than that, which becomes a part of its own.
pub fn max_part_bound_mib(part_mib: u64, max_file_bytes: u64) -> u64 {
    part_mib.max(max_file_bytes.div_ceil(MIB))
}

/// Free `/work` MiB a fresh box from a snapshot of `disk_gib` is expected to have.
pub fn fresh_box_free_mib(disk_gib: i64) -> u64 {
    (disk_gib.max(0) as u64 * 1024).saturating_sub(FRESH_BOX_OVERHEAD_MIB)
}

/// Decide, before anything on the box is stopped, whether the carry fits everywhere it has to: the
/// relay's spool, the source's own disk (the pack stages there) and the box the snapshot will make.
pub fn verdict(room: &Room, cfg: &CarryConfig) -> Verdict {
    if room.work_mib > cfg.max_work_mib {
        return Verdict::TooLarge {
            mib: room.work_mib,
            max_mib: cfg.max_work_mib,
        };
    }
    let work = room.work_mib.max(0) as u64;
    if room.spool_free_mib < work + SPOOL_MARGIN_MIB {
        return Verdict::NoSpoolRoom {
            need_mib: work,
            free_mib: room.spool_free_mib,
        };
    }
    let source_need = source_need_mib(room.max_part_mib, cfg.part_mib);
    if room.source_free_mib < source_need {
        return Verdict::NoSourceRoom {
            free_mib: room.source_free_mib,
            need_mib: source_need,
            max_part_mib: room.max_part_mib,
            part_mib: cfg.part_mib,
        };
    }
    if let Some(free) = room.target_free_mib {
        let need = destination_need_mib(work, cfg.part_mib, room.max_part_mib);
        if free < need {
            return Verdict::NoDestinationRoom {
                work_mib: work,
                free_mib: free,
                need_mib: need,
                max_part_mib: room.max_part_mib,
                part_mib: cfg.part_mib,
            };
        }
    }
    Verdict::Carry
}

/// One entry of the `/work` listing: `find . -printf '%y\t%s\t%i\t' -print0`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// `f` file, `d` directory, `l` symlink (anything else is not carried).
    pub kind: char,
    pub size: u64,
    pub inode: u64,
    /// Relative to `/work`, starting with `./` (the root itself is `.`).
    pub path: String,
}

impl Entry {
    pub fn carried(&self) -> bool {
        matches!(self.kind, 'f' | 'd' | 'l')
    }
}

/// Parse the NUL-separated listing. Malformed records are dropped (a path with a tab would be
/// split wrongly; `find` prints the path last, so tabs inside it survive).
pub fn parse_listing(bytes: &[u8]) -> Vec<Entry> {
    bytes
        .split(|b| *b == 0)
        .filter(|rec| !rec.is_empty())
        .filter_map(|rec| {
            let rec = String::from_utf8_lossy(rec);
            let mut it = rec.splitn(4, '\t');
            let kind = it.next()?.chars().next()?;
            let size = it.next()?.parse().ok()?;
            let inode = it.next()?.parse().ok()?;
            let path = it.next()?.to_owned();
            Some(Entry {
                kind,
                size,
                inode,
                path,
            })
        })
        .collect()
}

/// `true` for the rebuildable pi-ext package tree of a project (the directory and everything in it).
pub fn is_rebuildable(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("./") else {
        return false;
    };
    // `<project>/.arbos/pi/extensions/arbos-pi-ext/node_modules[/...]` — exactly one path segment
    // before the suffix, so the box-level `./.arbos/...` is never matched.
    let Some(project_end) = rest.find('/') else {
        return false;
    };
    let tail = &rest[project_end..];
    tail == REBUILDABLE_SUFFIX || tail.starts_with(&format!("{REBUILDABLE_SUFFIX}/"))
}

/// What the listing says about `/work`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Measure {
    /// Entries that will be carried (files, directories, symlinks).
    pub entries: u64,
    /// Apparent bytes of regular files (hardlinks counted once).
    pub bytes: u64,
    /// Entries skipped because they are not files, directories or symlinks (sockets, fifos).
    pub skipped_special: u64,
    pub rebuildable_entries: u64,
    pub rebuildable_bytes: u64,
    /// Projects that hold a dereferenced pi-ext package tree (an image before 0.1.145 made it).
    pub projects_with_copies: Vec<String>,
}

pub fn measure(entries: &[Entry]) -> Measure {
    let mut m = Measure::default();
    let mut seen = std::collections::HashSet::new();
    // Projects whose pi-ext package tree holds regular files: on a current image the directory
    // holds only the two seed symlinks, so a file below it means a dereferenced copy.
    let mut copies = Vec::new();
    for e in entries {
        if !e.carried() {
            m.skipped_special += 1;
            continue;
        }
        m.entries += 1;
        let bytes = if e.kind == 'f' && seen.insert(e.inode) {
            e.size
        } else {
            0
        };
        m.bytes += bytes;
        if is_rebuildable(&e.path) {
            m.rebuildable_entries += 1;
            m.rebuildable_bytes += bytes;
            if e.kind == 'f' {
                if let Some(p) = e.path.strip_prefix("./").and_then(|r| r.split('/').next()) {
                    if copies.last().is_none_or(|last: &String| last != p)
                        && !copies.iter().any(|c| c == p)
                    {
                        copies.push(p.to_owned());
                    }
                }
            }
        }
    }
    m.projects_with_copies = copies;
    m
}

/// One part of the plan: paths (walk order) and their apparent bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    pub paths: Vec<String>,
    pub bytes: u64,
}

/// Cut the listing into parts of at most `part_bytes` apparent bytes (a single larger file is a
/// part of its own). Walk order is kept so a directory entry lands in the same part as, or an
/// earlier one than, its children. Special files are left out; rebuildable trees when asked.
pub fn partition(entries: &[Entry], part_bytes: u64, skip_rebuildable: bool) -> Vec<Part> {
    let mut parts = Vec::new();
    let mut cur = Part {
        paths: Vec::new(),
        bytes: 0,
    };
    for e in entries {
        if !e.carried() || (skip_rebuildable && is_rebuildable(&e.path)) {
            continue;
        }
        let size = if e.kind == 'f' { e.size } else { 0 };
        if !cur.paths.is_empty() && cur.bytes + size > part_bytes {
            parts.push(std::mem::replace(
                &mut cur,
                Part {
                    paths: Vec::new(),
                    bytes: 0,
                },
            ));
        }
        cur.paths.push(e.path.clone());
        cur.bytes += size;
    }
    if !cur.paths.is_empty() {
        parts.push(cur);
    }
    parts
}

/// The `-T` file for `tar --null`: one path per NUL.
pub fn list_file(paths: &[String]) -> Vec<u8> {
    let mut out = Vec::with_capacity(paths.iter().map(|p| p.len() + 1).sum());
    for p in paths {
        out.extend_from_slice(p.as_bytes());
        out.push(0);
    }
    out
}

/// Expected count and byte sum on the destination after every part is extracted.
pub fn expected_after_carry(parts: &[Part]) -> (u64, u64) {
    let n = parts.iter().map(|p| p.paths.len() as u64).sum();
    let b = parts.iter().map(|p| p.bytes).sum();
    (n, b)
}

/// Does the new box have room, measured on the box itself once it exists? Bytes are
/// [`destination_need_mib`] — the same arithmetic the measure step already applied to the
/// snapshot's disk size, so this is the confirmation, not the first look. Inodes (when the
/// filesystem counts them): one per entry plus the gate's floor.
pub fn destination_fits(
    avail_mib: u64,
    avail_inodes: Option<u64>,
    need_mib: u64,
    entries: u64,
    part_mib: u64,
    max_part_mib: u64,
) -> std::result::Result<(), String> {
    let need_bytes = destination_need_mib(need_mib, part_mib, max_part_mib);
    if avail_mib < need_bytes {
        return Err(format!(
            "the new box has {avail_mib} MiB free on /work; carrying {need_mib} MiB needs {need_bytes} MiB (the biggest part's {max_part_mib} MiB tarball, two {part_mib} MiB pieces in flight and a {DEST_MARGIN_MIB} MiB margin included)"
        ));
    }
    if let Some(inodes) = avail_inodes {
        let need_inodes = entries + INODE_FLOOR;
        if inodes < need_inodes {
            return Err(format!(
                "the new box has {inodes} free file slots (inodes) on /work; the carried tree has {entries} entries and the gate keeps {INODE_FLOOR} free"
            ));
        }
    }
    Ok(())
}

/// Per-part transfer deadline: a floor plus time proportional to the size. The proxy moved a
/// 463 MB upload for 30 s and timed out; 2 s/MiB allows ~0.5 MB/s before a retry.
pub fn transfer_timeout(bytes: u64) -> Duration {
    let mib = bytes.div_ceil(MIB);
    Duration::from_secs(120 + 2 * mib)
}

/// Measured toolbox throughput of one carry: bytes moved and the time it took, over every piece
/// that completed. Starts empty; `piece_timeout` falls back to `transfer_timeout` until a piece
/// has been measured.
#[derive(Debug, Default, Clone, Copy)]
pub struct Throughput {
    pub bytes: u64,
    pub elapsed: Duration,
}

impl Throughput {
    pub fn record(&mut self, bytes: u64, elapsed: Duration) {
        self.bytes += bytes;
        self.elapsed += elapsed;
    }

    /// Bytes per second seen so far, `None` before the first completed piece.
    pub fn rate(&self) -> Option<f64> {
        let secs = self.elapsed.as_secs_f64();
        (self.bytes > 0 && secs > 0.0).then(|| self.bytes as f64 / secs)
    }

    pub fn mib_per_s(&self) -> Option<f64> {
        self.rate().map(|r| r / MIB as f64)
    }

    /// Deadline for a piece of `bytes`: the static `transfer_timeout`, or — once the link has been
    /// measured — twice the time the measured rate needs plus a minute, whichever is longer, capped
    /// at `PIECE_TIMEOUT_MAX`. A slow but moving link gets the time it needs instead of four
    /// identical timeouts; a stalled one still fails within the cap.
    pub fn piece_timeout(&self, bytes: u64) -> Duration {
        let base = transfer_timeout(bytes);
        match self.rate() {
            Some(rate) if rate > 0.0 => {
                let need = Duration::from_secs_f64(bytes as f64 / rate * 2.0 + 60.0);
                base.max(need).min(PIECE_TIMEOUT_MAX)
            }
            _ => base,
        }
    }
}

/// Whether a transfer error is the toolbox link being too slow (or stalled) for the deadline, as
/// opposed to a checksum or size mismatch that a smaller piece would not fix.
pub fn is_timeout(err: &anyhow::Error) -> bool {
    let text = format!("{err:#}").to_ascii_lowercase();
    text.contains("timed out") || text.contains("timeout") || text.contains("deadline")
}

/// Next piece size after a timeout: half, never below `MIN_PIECE_BYTES`. `None` when the piece
/// is already at the floor (the link is dead, not slow).
pub fn shrink_piece(bytes: u64) -> Option<u64> {
    if bytes <= MIN_PIECE_BYTES {
        return None;
    }
    Some((bytes / 2).max(MIN_PIECE_BYTES))
}

/// One permit per allowed concurrent carry (`CarryConfig::concurrent_carries`); the first carry
/// sizes it. Carries wait here before packing so they do not share the toolbox link.
pub fn carry_permits(cfg: &CarryConfig) -> &'static tokio::sync::Semaphore {
    static PERMITS: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    PERMITS.get_or_init(|| tokio::sync::Semaphore::new(cfg.concurrent_carries))
}

/// Run `op` up to `attempts` times with 2 s, 4 s, 8 s… between tries; the last error is returned
/// with every earlier one in its context.
pub async fn retry<T, F, Fut>(what: &str, attempts: u32, op: F) -> Result<T>
where
    F: FnMut(u32) -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    retry_with_backoff(what, attempts, Duration::from_secs(2), op).await
}

pub async fn retry_with_backoff<T, F, Fut>(
    what: &str,
    attempts: u32,
    base: Duration,
    mut op: F,
) -> Result<T>
where
    F: FnMut(u32) -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let attempts = attempts.max(1);
    let mut errors = Vec::new();
    for attempt in 1..=attempts {
        match op(attempt).await {
            Ok(v) => return Ok(v),
            Err(e) => {
                tracing::warn!(what, attempt, attempts, error = %format!("{e:#}"), "carry step failed; retrying");
                errors.push(format!("attempt {attempt}: {e:#}"));
                if attempt < attempts {
                    tokio::time::sleep(base * (1u32 << (attempt - 1).min(5))).await;
                }
            }
        }
    }
    Err(anyhow!(
        "{what} failed after {attempts} attempts: {}",
        errors.join("; ")
    ))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

// ---- box-side steps ----------------------------------------------------------------------------

fn exec_ok(out: &Value, what: &str, ok_codes: &[i64]) -> Result<String> {
    let code = out.get("exitCode").and_then(Value::as_i64).unwrap_or(1);
    let text = out
        .get("result")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if ok_codes.contains(&code) {
        Ok(text)
    } else {
        Err(anyhow!(
            "{what}: exit {code}: {}",
            text.chars().take(600).collect::<String>()
        ))
    }
}

/// `du -sm /work` (logs excluded) on a running box. Read-only; agentd keeps running.
pub async fn measure_du_mib(tb: &Toolbox<'_>) -> Result<i64> {
    let out = tb
        .exec(
            "cd /work 2>/dev/null || { echo 0; exit 0; }; sudo -n du -sm --exclude='./.arbos/logs' --exclude='./*/.arbos/logs' . | cut -f1",
            600,
        )
        .await
        .context("measure /work")?;
    let text = exec_ok(&out, "du /work", &[0, 1])?;
    text.lines()
        .last()
        .and_then(|s| s.trim().parse().ok())
        .ok_or_else(|| anyhow!("du /work: unreadable answer {:?}", text.trim()))
}

/// What the source box can spare for the pack, read on a running box: free MiB where the parts are
/// staged, and the biggest single carried file in bytes (it bounds the biggest part — see
/// [`max_part_bound_mib`]). Logs are excluded, exactly as the carry excludes them. Read-only.
pub async fn source_room(tb: &Toolbox<'_>) -> Result<(u64, u64)> {
    let cmd = format!(
        "df -Pm {STAGE_PARENT} | awk 'NR==2{{print $4}}'; cd /work 2>/dev/null || exit 0; sudo -n find . -xdev \\( -path ./.arbos/logs -o -path './*/.arbos/logs' \\) -prune -o -type f -printf '%s\\n' | awk 'm<$1{{m=$1}} END{{printf \"%.0f\\n\", m+0}}'"
    );
    let out = tb.exec(&cmd, 600).await.context("room on the source box")?;
    let text = exec_ok(&out, "df and the biggest file on the source", &[0])?;
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let free_mib: u64 = lines
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| anyhow!("df {STAGE_PARENT}: unreadable {text:?}"))?;
    let max_file_bytes = lines.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    Ok((free_mib, max_file_bytes))
}

/// Free `/work` MiB a fresh box from `snapshot` is expected to have, from the disk size Daytona
/// holds for the snapshot. `None` when it cannot be read: the carry then goes ahead and the `df`
/// on the new box decides, as it did before this was checked up front.
pub async fn target_free_mib(dt: &crate::daytona::Daytona, snapshot: &str) -> Option<u64> {
    match dt.get_snapshot(snapshot).await {
        Ok(s) => s.disk.filter(|gib| *gib > 0).map(fresh_box_free_mib),
        Err(e) => {
            tracing::warn!(
                snapshot,
                error = %format!("{e:#}"),
                "the target snapshot's disk size is unknown; the destination is checked on the new box only"
            );
            None
        }
    }
}

/// Free MiB on the filesystem holding `path` (created when missing).
pub fn free_mib(path: &Path) -> Result<u64> {
    std::fs::create_dir_all(path)?;
    let s = nix::sys::statvfs::statvfs(path)?;
    #[allow(clippy::unnecessary_cast)]
    Ok(s.blocks_available() as u64 * s.fragment_size() as u64 / MIB)
}

/// Stop agentd and its harnesses on a box.
pub async fn quiesce(tb: &Toolbox<'_>) -> Result<()> {
    let _ = tb
        .exec(
            "sudo -n pkill -TERM -x arbos-agentd 2>/dev/null; sleep 3; sudo -n pkill -KILL -x arbos-agentd 2>/dev/null; sudo -n pkill -KILL -f 'pi --mode rpc' 2>/dev/null; sudo -n pkill -KILL -x prime-agent 2>/dev/null; true",
            60,
        )
        .await?;
    Ok(())
}

/// Remove the carry's own staging directory from a box, and nothing else: `/work` is never touched
/// here. Every step that writes into [`STAGE_DIR`] ends with this, on its error paths as well as
/// its happy one — 2026-11-05, gh_268993031: a pack that failed on ENOSPC returned before its
/// cleanup and left 389 MiB of tarball, listing and piece on the box, which took the box from
/// 365 MiB free to 122 and made agentd release its ballast at the next turn.
pub async fn clear_staging(tb: &Toolbox<'_>) {
    if let Err(e) = tb.exec(&format!("sudo -n rm -rf {STAGE_DIR}"), 60).await {
        tracing::warn!(
            error = %format!("{e:#}"),
            "the carry's staging directory could not be removed from the box"
        );
    }
}

/// List `/work` on a quiesced box and parse it. `work.list` stays on the box for the pack that
/// follows; a failure here takes the whole staging directory with it.
pub async fn list_work(tb: &Toolbox<'_>, attempts: u32) -> Result<Vec<Entry>> {
    let listed = list_work_inner(tb, attempts).await;
    if listed.is_err() {
        clear_staging(tb).await;
    }
    listed
}

async fn list_work_inner(tb: &Toolbox<'_>, attempts: u32) -> Result<Vec<Entry>> {
    let cmd = format!(
        "sudo -n rm -rf {d} && mkdir -p {d} && cd /work && sudo -n find . -xdev \\( -path ./.arbos/logs -o -path './*/.arbos/logs' \\) -prune -o -printf '%y\\t%s\\t%i\\t' -print0 > {d}/work.list && wc -c < {d}/work.list",
        d = STAGE_DIR
    );
    let out = tb.exec(&cmd, 900).await.context("list /work")?;
    let text = exec_ok(&out, "find /work", &[0])?;
    let expect: u64 = text
        .lines()
        .last()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    let bytes: Vec<u8> = retry("download the /work listing", attempts, |_| async {
        let b: Vec<u8> = tb
            .download(&format!("{STAGE_DIR}/work.list"), transfer_timeout(expect))
            .await?;
        if b.len() as u64 != expect {
            return Err(anyhow!(
                "listing is {} bytes, the box wrote {expect}",
                b.len()
            ));
        }
        Ok(b)
    })
    .await?;
    Ok(parse_listing(&bytes))
}

/// One piece of a part's tarball as it crosses the proxy: at most `part_mib` bytes per request.
#[derive(Debug, Clone)]
pub struct SpooledPiece {
    pub file: PathBuf,
    pub sha256: String,
    pub bytes: u64,
}

/// One packed part on the relay's disk: the tarball's own sha256 and size, and the pieces it
/// travels in (one when it fits the part size; more for a single file bigger than that).
#[derive(Debug, Clone)]
pub struct SpooledPart {
    pub index: usize,
    pub sha256: String,
    pub tar_bytes: u64,
    pub pieces: Vec<SpooledPiece>,
}

/// Pack every part on the source box and spool it on the relay. The source holds one part's
/// tarball plus one piece at a time; the relay holds one piece in memory. `progress(k, n)` is
/// called before part `k` of `n` is made. The staging directory is gone when this returns,
/// whichever way it returns.
pub async fn pack_parts<P>(
    tb: &Toolbox<'_>,
    parts: &[Part],
    spool: &Path,
    cfg: &CarryConfig,
    progress: P,
) -> Result<Vec<SpooledPart>>
where
    P: FnMut(usize, usize),
{
    let packed = pack_each_part(tb, parts, spool, cfg, progress).await;
    clear_staging(tb).await;
    packed
}

async fn pack_each_part<P>(
    tb: &Toolbox<'_>,
    parts: &[Part],
    spool: &Path,
    cfg: &CarryConfig,
    mut progress: P,
) -> Result<Vec<SpooledPart>>
where
    P: FnMut(usize, usize),
{
    let _ = std::fs::remove_dir_all(spool);
    std::fs::create_dir_all(spool)?;
    let mut piece_bytes = cfg.part_mib * MIB;
    let mut throughput = Throughput::default();
    let n = parts.len();
    let mut out = Vec::with_capacity(n);
    for (k, part) in parts.iter().enumerate() {
        progress(k + 1, n);
        let list = format!("{STAGE_DIR}/part-{k}.list");
        let tgz = format!("{STAGE_DIR}/part-{k}.tgz");
        tb.upload(&list, list_file(&part.paths), transfer_timeout(0))
            .await
            .with_context(|| format!("upload the list of part {k}"))?;
        // Exit 1 = "file changed as we read it" warnings; the archive is complete. Missing files
        // (deleted after the listing) are warnings too (--ignore-failed-read), counted at the end.
        let cmd = format!(
            "cd /work && sudo -n tar --null --no-recursion --ignore-failed-read -T {list} -I 'gzip -1' -cf {tgz}; rc=$?; [ $rc -le 1 ] || exit $rc; sudo -n chmod 644 {tgz} && sha256sum {tgz} | cut -d' ' -f1 && stat -c %s {tgz}"
        );
        let res = tb.exec(&cmd, 1800).await?;
        let text = exec_ok(&res, &format!("tar part {k}"), &[0])?;
        let mut lines = text.lines().rev();
        let tar_bytes: u64 = lines
            .next()
            .and_then(|s| s.trim().parse().ok())
            .ok_or_else(|| anyhow!("tar part {k}: no size in {text:?}"))?;
        let sha = lines
            .next()
            .map(|s| s.trim().to_owned())
            .filter(|s| s.len() == 64)
            .ok_or_else(|| anyhow!("tar part {k}: no sha256 in {text:?}"))?;
        // Pieces are cut on the fly so a timeout can shrink the next cut from the same offset:
        // half the size (floor MIN_PIECE_BYTES), with the deadline derived from the throughput
        // measured so far. 2026-10-29: fixed 32–256 MiB pieces timed out four times in a row on a
        // ~0.3 MiB/s toolbox link and every upgrade of that box failed.
        let mut pieces = Vec::new();
        let mut off: u64 = 0;
        let mut j: usize = 0;
        // An empty tarball still yields one (empty) piece, as `piece_ranges` does.
        while off < tar_bytes || pieces.is_empty() {
            let len = piece_bytes.min(tar_bytes - off);
            let piece = format!("{tgz}.{j:04}");
            // One piece at a time next to the tarball: `tail | head` copies the range, nothing more.
            let cut = format!(
                "tail -c +{} {tgz} | head -c {len} > {piece} && sha256sum {piece} | cut -d' ' -f1",
                off + 1
            );
            let res = tb.exec(&cut, 900).await?;
            let piece_sha = exec_ok(&res, &format!("cut piece {j} of part {k}"), &[0])?
                .trim()
                .to_owned();
            if piece_sha.len() != 64 {
                return Err(anyhow!("piece {j} of part {k}: no sha256 in {piece_sha:?}"));
            }
            let file = spool.join(format!("part-{k}.tgz.{j:04}"));
            let timeout = throughput.piece_timeout(len);
            let (sha_ok, piece_ok, file_ok) = (piece_sha.clone(), piece.clone(), file.clone());
            let started = std::time::Instant::now();
            let downloaded = retry(
                &format!("download piece {j} of part {k}"),
                cfg.attempts,
                |_| {
                    let (sha, piece, file) = (sha_ok.clone(), piece_ok.clone(), file_ok.clone());
                    async move {
                        let bytes = tb.download(&piece, timeout).await?;
                        if bytes.len() as u64 != len {
                            return Err(anyhow!("got {} bytes, the box wrote {len}", bytes.len()));
                        }
                        let got = sha256_hex(&bytes);
                        if got != sha {
                            return Err(anyhow!("sha256 mismatch ({got} vs {sha} on the box)"));
                        }
                        tokio::fs::write(&file, &bytes).await?;
                        Ok(())
                    }
                },
            )
            .await;
            let _ = tb.exec(&format!("rm -f {piece}"), 60).await;
            match downloaded {
                Ok(()) => {
                    throughput.record(len, started.elapsed());
                    tracing::info!(
                        part = k, piece = j, bytes = len, ms = started.elapsed().as_millis() as u64,
                        mib_per_s = throughput.mib_per_s().unwrap_or(0.0),
                        "carry piece downloaded"
                    );
                    pieces.push(SpooledPiece {
                        file,
                        sha256: piece_sha,
                        bytes: len,
                    });
                    off += len;
                    j += 1;
                }
                Err(e) if is_timeout(&e) => match shrink_piece(piece_bytes) {
                    Some(smaller) => {
                        tracing::warn!(
                            part = k, piece = j, bytes = len, next_piece_bytes = smaller,
                            mib_per_s = throughput.mib_per_s().unwrap_or(0.0), error = %e,
                            "carry piece timed out; shrinking the piece and cutting again from the same offset"
                        );
                        piece_bytes = smaller;
                    }
                    None => {
                        return Err(e.context(format!(
                            "piece {j} of part {k} still times out at the {MIN_PIECE_BYTES}-byte floor (toolbox download {:.3} MiB/s so far)",
                            throughput.mib_per_s().unwrap_or(0.0)
                        )))
                    }
                },
                Err(e) => return Err(e),
            }
        }
        let _ = tb.exec(&format!("sudo -n rm -f {tgz} {list}"), 60).await;
        out.push(SpooledPart {
            index: k,
            sha256: sha,
            tar_bytes,
            pieces,
        });
    }
    tracing::info!(
        parts = n,
        bytes = throughput.bytes,
        secs = throughput.elapsed.as_secs(),
        mib_per_s = throughput.mib_per_s().unwrap_or(0.0),
        final_piece_bytes = piece_bytes,
        "source /work packed; toolbox download throughput"
    );
    Ok(out)
}

/// The biggest tarball among the parts (MiB, rounded up) — what the destination must stage.
pub fn max_part_mib(parts: &[SpooledPart]) -> u64 {
    parts
        .iter()
        .map(|p| p.tar_bytes.div_ceil(MIB))
        .max()
        .unwrap_or(0)
}

/// `(avail MiB, avail inodes)` of `/work` on a box. Inodes are `None` when the filesystem does
/// not count them (0 total).
pub async fn destination_room(tb: &Toolbox<'_>) -> Result<(u64, Option<u64>)> {
    let out = tb
        .exec(
            "df -Pm /work | awk 'NR==2{print $4}'; df -Pi /work | awk 'NR==2{print $2, $4}'",
            60,
        )
        .await
        .context("df on the new box")?;
    let text = exec_ok(&out, "df /work", &[0])?;
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let avail_mib: u64 = lines
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| anyhow!("df /work: unreadable {text:?}"))?;
    let inodes = lines.next().and_then(|l| {
        let mut it = l.split_whitespace();
        let total: u64 = it.next()?.parse().ok()?;
        let avail: u64 = it.next()?.parse().ok()?;
        (total > 0).then_some(avail)
    });
    Ok((avail_mib, inodes))
}

/// Upload, verify and extract every spooled part onto the new box's `/work` (emptied first), then
/// count the result against the plan. Pieces are appended at their offset (idempotent on retry);
/// the daemon's `/tmp/multipart-*` copy of each upload is removed at once — it is never cleaned up
/// otherwise (found 2026-10-26: every upload leaves a full copy in `/tmp`, so the one-piece carry
/// left a copy of the whole `/work` tarball on every upgraded box). `progress(k, n)` before part `k` of `n`.
pub async fn unpack_parts<P>(
    tb: &Toolbox<'_>,
    parts: &[SpooledPart],
    expected: (u64, u64),
    cfg: &CarryConfig,
    mut progress: P,
) -> Result<()>
where
    P: FnMut(usize, usize),
{
    let prep = tb
        .exec(
            &format!(
                "sudo -n rm -rf /work/* /work/.[!.]* 2>/dev/null; sudo -n rm -rf {d} /tmp/multipart-* && mkdir -p {d} && echo ok",
                d = STAGE_DIR
            ),
            600,
        )
        .await?;
    exec_ok(&prep, "empty /work on the new box", &[0])?;
    let n = parts.len();
    for (i, part) in parts.iter().enumerate() {
        progress(i + 1, n);
        let k = part.index;
        let tgz = format!("{STAGE_DIR}/part-{k}.tgz");
        let mut off = 0u64;
        for (j, piece) in part.pieces.iter().enumerate() {
            let bytes = tokio::fs::read(&piece.file)
                .await
                .with_context(|| format!("read spooled piece {j} of part {k}"))?;
            if bytes.len() as u64 != piece.bytes {
                return Err(anyhow!(
                    "spooled piece {j} of part {k} is {} bytes, expected {}",
                    bytes.len(),
                    piece.bytes
                ));
            }
            let timeout = transfer_timeout(piece.bytes);
            let bytes = std::sync::Arc::new(bytes);
            let staged = format!("{tgz}.{j:04}");
            // Verify the piece, cut the tarball back to this piece's offset (a retry after a
            // half-done append starts clean), append, drop the piece and the daemon's temp copy.
            let append = format!(
                "echo '{sha}  {staged}' | sha256sum -c --quiet - && truncate -s {off} {tgz} && cat {staged} >> {tgz} && rm -f {staged} && sudo -n rm -f /tmp/multipart-* && echo ok",
                sha = piece.sha256
            );
            retry(
                &format!("upload piece {j} of part {k}"),
                cfg.attempts,
                |_| {
                    let (bytes, staged, append) = (bytes.clone(), staged.clone(), append.clone());
                    async move {
                        tb.upload(&staged, bytes.as_ref().clone(), timeout).await?;
                        let res = tb.exec(&append, 900).await?;
                        exec_ok(&res, &format!("append piece {j} of part {k}"), &[0])?;
                        Ok(())
                    }
                },
            )
            .await?;
            off += piece.bytes;
        }
        let extract = format!(
            "echo '{sha}  {tgz}' | sha256sum -c --quiet - && cd /work && sudo -n tar -xzf {tgz} -C /work && rm -f {tgz} && echo ok",
            sha = part.sha256
        );
        let res = tb.exec(&extract, 1800).await?;
        exec_ok(&res, &format!("verify and extract part {k}"), &[0])?;
        for piece in &part.pieces {
            let _ = tokio::fs::remove_file(&piece.file).await;
        }
    }
    let count = tb
        .exec(
            "cd /work && sudo -n find . -xdev \\( -type f -o -type d -o -type l \\) -printf '%y\\t%s\\n' | awk -F'\\t' '{n++; if($1==\"f\") b+=$2} END{printf \"%d %.0f\\n\", n, b}'; sudo -n rm -rf /home/daytona/arbos-upgrade /tmp/multipart-*",
            900,
        )
        .await?;
    let text = exec_ok(&count, "count /work on the new box", &[0])?;
    let mut it = text.split_whitespace();
    let got_n: u64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let got_b: u64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    // The new box may already have ADDED to /work while the parts unpacked (agentd starts, writes
    // its state, rebuilds node_modules): 2026-10-28 21:42 a carry was rejected for 27 extra entries
    // / 2.8 KB more than packed. Fewer entries or fewer bytes than packed is the corruption this
    // check exists for; more is the box being alive.
    if got_n < expected.0 || got_b < expected.1 {
        return Err(anyhow!(
            "carried tree does not match the plan: {got_n} entries / {got_b} bytes on the new box, {} entries / {} bytes packed",
            expected.0,
            expected.1
        ));
    }
    if (got_n, got_b) != expected {
        tracing::info!(
            got_entries = got_n,
            got_bytes = got_b,
            packed_entries = expected.0,
            packed_bytes = expected.1,
            "carried tree has grown since the pack (the new box is writing); accepted"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> CarryConfig {
        CarryConfig {
            max_work_mib: DEFAULT_MAX_WORK_MIB,
            part_mib: DEFAULT_PART_MIB,
            attempts: 3,
            skip_rebuildable: false,
            concurrent_carries: DEFAULT_CONCURRENT_CARRIES,
            spool_root: std::env::temp_dir().join("arbos-carry-test"),
        }
    }

    fn listing(rows: &[(&str, u64, u64, &str)]) -> Vec<u8> {
        let mut b = Vec::new();
        for (k, s, i, p) in rows {
            b.extend_from_slice(format!("{k}\t{s}\t{i}\t{p}").as_bytes());
            b.push(0);
        }
        b
    }

    /// A carry that fits everywhere, as the measure step sees it.
    fn room(work_mib: i64, spool_free_mib: u64) -> Room {
        Room {
            work_mib,
            spool_free_mib,
            source_free_mib: 100_000,
            max_part_mib: DEFAULT_PART_MIB,
            target_free_mib: None,
        }
    }

    /// The limit is a sanity stop far above today's boxes: Jacob's 3979 MiB carries; 16 GiB + 1 MiB
    /// does not, and the refusal keeps the phrase the deploy worker's skip rule matches.
    #[test]
    fn limit_is_16_gib_and_refusal_keeps_the_skip_phrase() {
        let c = cfg();
        assert_eq!(verdict(&room(3979, 500_000), &c), Verdict::Carry);
        assert_eq!(verdict(&room(16 * 1024, 500_000), &c), Verdict::Carry);
        let v = verdict(&room(16 * 1024 + 1, 500_000), &c);
        assert!(matches!(v, Verdict::TooLarge { .. }));
        assert!(v.reason().unwrap().contains("too large to carry"));
        let v = verdict(&room(4000, 4000), &c);
        assert!(matches!(v, Verdict::NoSpoolRoom { .. }));
        assert!(v.reason().unwrap().contains("too large to carry"));
        assert_eq!(
            verdict(&room(4000, 4000 + SPOOL_MARGIN_MIB), &c),
            Verdict::Carry
        );
    }

    /// The pack stages its tarballs on the source's own disk, so the source is measured before
    /// agentd is stopped. gh_268993031, 2026-11-05: `/work` was 9557 MiB with a lone 356.9 MiB file
    /// in it and 365 MiB free where the parts are staged. Nothing in the old checks looked at that,
    /// the tarball of that one file filled the disk, and `head` cutting its first piece hit ENOSPC.
    #[test]
    fn the_source_box_is_measured_before_it_is_stopped() {
        let c = CarryConfig {
            part_mib: 16,
            ..cfg()
        };
        let big_file = 374_241_280; // 356.9 MiB: the pip cache entry that filled the disk
        let test5 = Room {
            work_mib: 9557,
            spool_free_mib: 500_000,
            source_free_mib: 365,
            max_part_mib: max_part_bound_mib(c.part_mib, big_file),
            target_free_mib: None,
        };
        assert_eq!(test5.max_part_mib, 357, "a lone big file is its own part");
        let v = verdict(&test5, &c);
        assert_eq!(
            v,
            Verdict::NoSourceRoom {
                free_mib: 365,
                need_mib: 357 + 16 + SOURCE_MARGIN_MIB,
                max_part_mib: 357,
                part_mib: 16,
            }
        );
        let reason = v.reason().unwrap();
        for figure in ["365", "629", "357", "16", "too large to carry"] {
            assert!(reason.contains(figure), "{figure} missing from {reason:?}");
        }
        assert!(
            reason.len() <= 200,
            "the reason is cut at 200 characters: {reason:?}"
        );
        // Exactly the peak fits; one MiB less does not.
        assert_eq!(
            verdict(
                &Room {
                    source_free_mib: 629,
                    ..test5
                },
                &c
            ),
            Verdict::Carry
        );
        assert!(matches!(
            verdict(
                &Room {
                    source_free_mib: 628,
                    ..test5
                },
                &c
            ),
            Verdict::NoSourceRoom { .. }
        ));
        // Without a file bigger than a part, the peak is one part plus one piece plus the margin.
        assert_eq!(source_need_mib(max_part_bound_mib(16, 4096), 16), 288);
        assert_eq!(STAGE_DIR.rsplit_once('/').unwrap().0, STAGE_PARENT);
    }

    /// The destination's arithmetic is the one the new box is held to; the measure step applies it
    /// to the target snapshot's disk size, so a snapshot that cannot hold the carry is refused
    /// before the pack rather than after it. Test5's shape on a 10 GiB snapshot: 9557 + 357 + 32 +
    /// 512 = 10 458 MiB needed, about 9984 MiB free on a fresh box.
    #[test]
    fn a_snapshot_too_small_for_the_carry_is_refused_at_the_measure_step() {
        let c = CarryConfig {
            part_mib: 16,
            ..cfg()
        };
        let test5 = Room {
            work_mib: 9557,
            spool_free_mib: 500_000,
            source_free_mib: 100_000,
            max_part_mib: 357,
            target_free_mib: Some(fresh_box_free_mib(10)),
        };
        assert_eq!(fresh_box_free_mib(10), 9984);
        let v = verdict(&test5, &c);
        assert_eq!(
            v,
            Verdict::NoDestinationRoom {
                work_mib: 9557,
                free_mib: 9984,
                need_mib: 10_458,
                max_part_mib: 357,
                part_mib: 16,
            }
        );
        let reason = v.reason().unwrap();
        for figure in ["9557", "9984", "10458", "357", "too large to carry"] {
            assert!(reason.contains(figure), "{figure} missing from {reason:?}");
        }
        assert!(
            reason.len() <= 200,
            "the reason is cut at 200 characters: {reason:?}"
        );
        // The same figures on a 20 GiB snapshot carry, and so does the 0.1.193 upgrade of the same
        // box, which fitted a 10 GiB snapshot when /work was 3 GiB smaller.
        assert_eq!(
            verdict(
                &Room {
                    target_free_mib: Some(fresh_box_free_mib(20)),
                    ..test5
                },
                &c
            ),
            Verdict::Carry
        );
        assert_eq!(
            verdict(
                &Room {
                    work_mib: 6500,
                    ..test5
                },
                &c
            ),
            Verdict::Carry
        );
        // An unknown snapshot disk leaves the verdict to the `df` on the new box, as before.
        assert_eq!(
            verdict(
                &Room {
                    target_free_mib: None,
                    ..test5
                },
                &c
            ),
            Verdict::Carry
        );
        // One arithmetic, two moments: what the measure step needs is what the new box is held to.
        let need = destination_need_mib(9557, 16, 357);
        assert_eq!(need, 10_458);
        assert!(destination_fits(need, None, 9557, 10, 16, 357).is_ok());
        assert!(destination_fits(need - 1, None, 9557, 10, 16, 357).is_err());
    }

    #[test]
    fn env_knobs_override_the_defaults() {
        let dir = std::env::temp_dir();
        std::env::remove_var("ARBOS_UPGRADE_MAX_WORK_MIB");
        std::env::remove_var("ARBOS_UPGRADE_PART_MIB");
        std::env::remove_var("ARBOS_UPGRADE_SKIP_REBUILDABLE");
        let c = CarryConfig::from_env(&dir);
        assert_eq!(c.max_work_mib, DEFAULT_MAX_WORK_MIB);
        assert_eq!(c.part_mib, DEFAULT_PART_MIB);
        assert!(!c.skip_rebuildable);
        assert_eq!(c.spool_root, dir.join("upgrade-spool"));
        std::env::set_var("ARBOS_UPGRADE_MAX_WORK_MIB", "3500");
        std::env::set_var("ARBOS_UPGRADE_PART_MIB", "4");
        std::env::set_var("ARBOS_UPGRADE_SKIP_REBUILDABLE", "1");
        let c = CarryConfig::from_env(&dir);
        assert_eq!(c.max_work_mib, 3500);
        assert_eq!(c.part_mib, 16, "part size has a 16 MiB floor");
        assert!(c.skip_rebuildable);
        std::env::remove_var("ARBOS_UPGRADE_MAX_WORK_MIB");
        std::env::remove_var("ARBOS_UPGRADE_PART_MIB");
        std::env::remove_var("ARBOS_UPGRADE_SKIP_REBUILDABLE");
    }

    #[test]
    fn listing_parses_and_measures() {
        let b = listing(&[
            ("d", 4096, 1, "."),
            ("d", 4096, 2, "./p1"),
            ("f", 100, 3, "./p1/a.txt"),
            ("f", 100, 3, "./p1/a-hardlink.txt"),
            ("l", 7, 4, "./p1/link"),
            ("s", 0, 5, "./.arbos/agentd.sock"),
            ("f", 5, 6, "./p1/tab\tin name"),
            (
                "d",
                4096,
                7,
                "./p1/.arbos/pi/extensions/arbos-pi-ext/node_modules",
            ),
            (
                "f",
                50,
                8,
                "./p1/.arbos/pi/extensions/arbos-pi-ext/node_modules/typebox/x.js",
            ),
            (
                "d",
                4096,
                9,
                "./.arbos/pi/extensions/arbos-pi-ext/node_modules",
            ),
            (
                "d",
                4096,
                10,
                "./p2/.arbos/pi/extensions/arbos-pi-ext/node_modules",
            ),
            (
                "l",
                40,
                11,
                "./p2/.arbos/pi/extensions/arbos-pi-ext/node_modules/typebox",
            ),
            (
                "l",
                40,
                12,
                "./p2/.arbos/pi/extensions/arbos-pi-ext/node_modules/@earendil-works",
            ),
        ]);
        let e = parse_listing(&b);
        assert_eq!(e.len(), 13);
        assert_eq!(e[6].path, "./p1/tab\tin name");
        let m = measure(&e);
        assert_eq!(m.entries, 12, "the socket is not carried");
        assert_eq!(m.skipped_special, 1);
        assert_eq!(m.bytes, 100 + 5 + 50, "hardlinked bytes count once");
        assert_eq!(m.rebuildable_entries, 5);
        assert_eq!(m.rebuildable_bytes, 50);
        assert_eq!(
            m.projects_with_copies,
            vec!["p1".to_owned()],
            "p2 holds only the seed's symlinks: a current-image project, not a copy"
        );
        assert!(!is_rebuildable(
            "./.arbos/pi/extensions/arbos-pi-ext/node_modules"
        ));
        assert!(!is_rebuildable(
            "./p1/.arbos/pi/extensions/arbos-pi-ext/node_modules-other"
        ));
        assert!(is_rebuildable(
            "./p1/.arbos/pi/extensions/arbos-pi-ext/node_modules/@earendil-works/a"
        ));
    }

    /// Parts never exceed the cap (a lone bigger file is its own part), directories stay ahead of
    /// their children, special files are dropped, and the rebuildable tree goes only when asked.
    #[test]
    fn partition_respects_cap_order_and_exclusions() {
        let e = parse_listing(&listing(&[
            ("d", 4096, 1, "."),
            ("d", 4096, 2, "./p1"),
            ("f", 60, 3, "./p1/a"),
            ("f", 60, 4, "./p1/b"),
            ("f", 250, 5, "./p1/big"),
            ("s", 0, 6, "./p1/sock"),
            (
                "d",
                4096,
                7,
                "./p1/.arbos/pi/extensions/arbos-pi-ext/node_modules",
            ),
            (
                "f",
                10,
                8,
                "./p1/.arbos/pi/extensions/arbos-pi-ext/node_modules/t/x.js",
            ),
            ("l", 1, 9, "./p1/ln"),
        ]));
        let parts = partition(&e, 100, false);
        let paths: Vec<Vec<&str>> = parts
            .iter()
            .map(|p| p.paths.iter().map(String::as_str).collect())
            .collect();
        assert_eq!(
            paths,
            vec![
                vec![".", "./p1", "./p1/a"],
                vec!["./p1/b"],
                vec!["./p1/big"],
                vec![
                    "./p1/.arbos/pi/extensions/arbos-pi-ext/node_modules",
                    "./p1/.arbos/pi/extensions/arbos-pi-ext/node_modules/t/x.js",
                    "./p1/ln"
                ],
            ]
        );
        assert_eq!(parts[2].bytes, 250);
        assert_eq!(expected_after_carry(&parts), (8, 380));
        let skipped = partition(&e, 100, true);
        let all: Vec<&str> = skipped
            .iter()
            .flat_map(|p| p.paths.iter().map(String::as_str))
            .collect();
        assert_eq!(
            all,
            vec![".", "./p1", "./p1/a", "./p1/b", "./p1/big", "./p1/ln"]
        );
        assert_eq!(expected_after_carry(&skipped), (6, 370));
        assert!(partition(&[], 100, false).is_empty());
        assert_eq!(list_file(&["a".into(), "b c".into()]), b"a\0b c\0".to_vec());
    }

    /// A 5 GiB tree in 256 MiB parts is about 20 parts; 16 GiB about 64.
    #[test]
    fn part_counts_for_big_trees() {
        let mut rows = Vec::new();
        let mut owned = Vec::new();
        for i in 0..5 * 1024 {
            owned.push(format!("./f{i}"));
        }
        for (i, p) in owned.iter().enumerate() {
            rows.push(("f", MIB, i as u64 + 1, p.as_str()));
        }
        let e = parse_listing(&listing(&rows));
        let parts = partition(&e, DEFAULT_PART_MIB * MIB, false);
        assert_eq!(parts.len(), 20);
        assert!(parts.iter().all(|p| p.bytes <= DEFAULT_PART_MIB * MIB));
    }

    /// The inode-aware gate (#1857) holds for the carry: a destination with enough bytes but too
    /// few file slots is refused before any upload.
    #[test]
    fn destination_room_counts_bytes_and_inodes() {
        // Jacob's shape: 3979 MiB, 42 000 entries, 256 MiB parts, biggest tarball 256 MiB → needs
        // 3979 + 256 + 512 + 512 = 5259 MiB.
        assert!(destination_fits(9000, Some(131_072), 3979, 42_000, 256, 256).is_ok());
        assert!(destination_fits(5259, Some(131_072), 3979, 42_000, 256, 256).is_ok());
        let e = destination_fits(5258, Some(131_072), 3979, 42_000, 256, 256).unwrap_err();
        assert!(e.contains("MiB free"), "{e}");
        // A lone 1 GiB file makes a 1 GiB part: the staging room grows with it.
        assert!(destination_fits(6027, None, 3979, 10, 256, 1024).is_ok());
        assert!(destination_fits(6026, None, 3979, 10, 256, 1024).is_err());
        let e = destination_fits(9000, Some(40_000), 3979, 42_000, 256, 256).unwrap_err();
        assert!(e.contains("inodes"), "{e}");
        assert!(
            destination_fits(9000, None, 3979, 400_000, 256, 256).is_ok(),
            "no inode count = no verdict"
        );
        assert!(destination_fits(9000, Some(43_024), 3979, 42_000, 256, 256).is_ok());
        assert!(destination_fits(9000, Some(43_023), 3979, 42_000, 256, 256).is_err());
    }

    /// A tarball travels in pieces of at most the part size; an empty one is one empty piece.
    #[test]
    fn throughput_derives_a_longer_deadline_on_a_slow_link() {
        let mut t = Throughput::default();
        assert_eq!(t.piece_timeout(32 * MIB), transfer_timeout(32 * MIB));
        // 16 MiB in 160 s = 0.1 MiB/s: a 32 MiB piece needs 320 s; ×2 + 60 = 700 s > the static 184 s.
        t.record(16 * MIB, Duration::from_secs(160));
        assert_eq!(t.piece_timeout(32 * MIB), Duration::from_secs(700));
        // a fast link keeps the static floor
        let mut fast = Throughput::default();
        fast.record(256 * MIB, Duration::from_secs(10));
        assert_eq!(fast.piece_timeout(32 * MIB), transfer_timeout(32 * MIB));
        // and nothing exceeds the cap
        let mut crawl = Throughput::default();
        crawl.record(MIB, Duration::from_secs(3600));
        assert_eq!(crawl.piece_timeout(256 * MIB), PIECE_TIMEOUT_MAX);
        assert!((t.mib_per_s().unwrap() - 0.1).abs() < 1e-9);
    }

    #[test]
    fn piece_shrinks_by_half_down_to_the_floor() {
        assert_eq!(shrink_piece(256 * MIB), Some(128 * MIB));
        assert_eq!(shrink_piece(6 * MIB), Some(MIN_PIECE_BYTES));
        assert_eq!(shrink_piece(MIN_PIECE_BYTES), None);
        assert!(is_timeout(&anyhow!(
            "toolbox download body: operation timed out"
        )));
        assert!(!is_timeout(&anyhow!("sha256 mismatch (a vs b on the box)")));
    }

    #[test]
    fn concurrent_carries_default_to_one() {
        std::env::remove_var("ARBOS_UPGRADE_CONCURRENT_CARRIES");
        let c = CarryConfig::from_env(Path::new("/tmp"));
        assert_eq!(c.concurrent_carries, 1);
        std::env::set_var("ARBOS_UPGRADE_CONCURRENT_CARRIES", "0");
        assert_eq!(
            CarryConfig::from_env(Path::new("/tmp")).concurrent_carries,
            1
        );
        std::env::remove_var("ARBOS_UPGRADE_CONCURRENT_CARRIES");
    }

    #[test]
    fn transfer_deadline_scales_with_size() {
        assert_eq!(transfer_timeout(0), Duration::from_secs(120));
        assert_eq!(transfer_timeout(256 * MIB), Duration::from_secs(120 + 512));
        assert_eq!(
            transfer_timeout(463_000_000),
            Duration::from_secs(120 + 2 * 442)
        );
    }

    /// Resume: a part that fails twice and succeeds the third time is carried without starting
    /// over; one that never succeeds reports every attempt.
    #[tokio::test]
    async fn retry_resumes_the_failed_step() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let c2 = calls.clone();
        let fast = Duration::from_millis(1);
        let fut = retry_with_backoff("upload part 3", 4, fast, move |attempt| {
            let c = c2.clone();
            async move {
                c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if attempt < 3 {
                    Err(anyhow!("operation timed out"))
                } else {
                    Ok(attempt)
                }
            }
        });
        let got = fut.await.unwrap();
        assert_eq!(got, 3);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);
        let err = retry_with_backoff("upload part 9", 2, fast, |a| async move {
            Err::<(), _>(anyhow!("HTTP 502 Bad Gateway #{a}"))
        })
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("after 2 attempts"), "{err}");
        assert!(err.contains("#1") && err.contains("#2"), "{err}");
    }

    #[test]
    fn sha256_matches_coreutils() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// Real-path carry between two fresh Daytona sandboxes (no relay rows involved). Needs
    /// `DAYTONA_API_KEY` and `ARBOS_CARRY_E2E_SNAPSHOT` (an `arbos-agent-<v>` snapshot); optional
    /// `ARBOS_CARRY_E2E_GIB` (default 5), `ARBOS_CARRY_E2E_PART_MIB` (default 256),
    /// `ARBOS_CARRY_E2E_KEEP=1` to leave the sandboxes for inspection. Run with
    /// `cargo test -p arbos-relay --locked carry_e2e -- --ignored --nocapture`. Checks, in order:
    /// measure runs with agentd alive; a refusal (tiny limit) leaves agentd alive; the 5 GiB tree
    /// goes over in parts and the destination's manifest (modes, owners, sizes, link targets,
    /// file sha256s) equals the source's; staging dirs are gone on both boxes.
    #[tokio::test]
    #[ignore = "needs DAYTONA_API_KEY and ARBOS_CARRY_E2E_SNAPSHOT; creates two sandboxes"]
    async fn carry_e2e_between_two_daytona_sandboxes() {
        let key = std::env::var("DAYTONA_API_KEY").expect("DAYTONA_API_KEY");
        let snapshot = std::env::var("ARBOS_CARRY_E2E_SNAPSHOT").expect("ARBOS_CARRY_E2E_SNAPSHOT");
        let gib: u64 = std::env::var("ARBOS_CARRY_E2E_GIB")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5);
        let part_mib: u64 = std::env::var("ARBOS_CARRY_E2E_PART_MIB")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_PART_MIB);
        let keep = std::env::var("ARBOS_CARRY_E2E_KEEP").is_ok_and(|v| v == "1");
        let dt = crate::daytona::Daytona::new(
            key,
            std::env::var("DAYTONA_API_URL").ok(),
            std::env::var("DAYTONA_ORG_ID").ok(),
        );
        let stamp = arbos_core::ids::now_ms();
        let mk = |role: &str| crate::daytona::CreateSandbox {
            name: Some(format!("arbos-test-carry-{role}-{stamp}")),
            snapshot: Some(snapshot.clone()),
            auto_stop_interval: Some(0),
            auto_archive_interval: Some(60),
            auto_delete_interval: Some(24 * 60),
            ttl_minutes: Some(0),
            ..Default::default()
        };
        let t0 = std::time::Instant::now();
        let src = dt.create(&mk("src")).await.expect("create source");
        let dst = dt.create(&mk("dst")).await.expect("create destination");
        eprintln!("sandboxes: src={} dst={}", src.id, dst.id);
        let ids = (src.id.clone(), dst.id.clone());
        let result: Result<()> = async {
            for id in [&ids.0, &ids.1] {
                dt.wait_for(id, Duration::from_secs(300), |s| {
                    s.state == crate::daytona::SandboxState::Started
                })
                .await?;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
            let a = dt.toolbox(&ids.0).await;
            let b = dt.toolbox(&ids.1).await;
            eprintln!("both started after {:?}; seeding {gib} GiB", t0.elapsed());
            // Seed: big random files, a 16 000-file dereferenced pi-ext tree (the #1857 shape), a
            // hardlink pair, a symlink, names with spaces, excluded logs, an ignored socket.
            let bigs = gib.saturating_sub(1).max(1);
            let seed = format!(
                "sudo -n bash -c 'set -e; mkdir -p /work/proj-a/.arbos/pi/extensions/arbos-pi-ext/node_modules/typebox /work/proj-b/data \"/work/proj-a/dir with space\" /work/.arbos/logs /work/proj-a/.arbos/logs; \
                 for i in $(seq 1 {bigs}); do head -c 1073741824 /dev/urandom > /work/proj-b/data/big-$i.bin; done; \
                 head -c 536870912 /dev/urandom > \"/work/proj-a/dir with space/half a.bin\"; head -c 536870912 /dev/urandom > /work/proj-a/half-b.bin; \
                 cd /work/proj-a/.arbos/pi/extensions/arbos-pi-ext/node_modules/typebox && for d in $(seq 1 40); do mkdir -p d$d; for f in $(seq 1 400); do echo \"$d-$f\" > d$d/f$f.js; done; done; \
                 echo hello > /work/proj-a/a.txt; ln /work/proj-a/a.txt /work/proj-a/hard.txt; ln -s a.txt /work/proj-a/link; \
                 echo log > /work/.arbos/logs/x.log; echo log > /work/proj-a/.arbos/logs/y.log; \
                 python3 -c \"import socket; socket.socket(socket.AF_UNIX).bind(\\\"/work/proj-a/.arbos/agent.sock\\\")\" || true; \
                 chown -R 1000:1000 /work/proj-a; sync; du -sm /work | cut -f1'"
            );
            let out = a.exec(&seed, 1800).await?;
            let seeded = exec_ok(&out, "seed", &[0])?;
            eprintln!("seeded: du={} MiB after {:?}", seeded.trim(), t0.elapsed());
            let manifest_cmd = "cd /work && sudo -n find . -xdev \\( -path ./.arbos/logs -o -path './*/.arbos/logs' \\) -prune -o -type f -printf '%m %U %G %s %p\\n' -o -type d -printf '%m %U %G d %p\\n' -o -type l -printf 'l %U %G %l %p\\n' | LC_ALL=C sort | sha256sum | cut -d' ' -f1; cd /work && sudo -n find . -xdev \\( -path ./.arbos/logs -o -path './*/.arbos/logs' \\) -prune -o -type f -print0 | LC_ALL=C sort -z | xargs -0 sudo -n sha256sum | sha256sum | cut -d' ' -f1";
            let m_src = exec_ok(&a.exec(manifest_cmd, 1800).await?, "manifest src", &[0])?;
            eprintln!("source manifest: {}", m_src.replace('\n', " "));
            async fn alive(tb: &Toolbox<'_>) -> Result<bool> {
                let r = tb
                    .exec(
                        "pgrep -x arbos-agentd >/dev/null && echo alive || echo dead",
                        30,
                    )
                    .await?;
                Ok(exec_ok(&r, "pgrep", &[0])?.trim() == "alive")
            }
            let agentd_before = alive(&a).await?;
            eprintln!("agentd on the source before measure: {}", if agentd_before { "alive" } else { "not running (no relay for this bare sandbox)" });
            // 1. measure with agentd untouched
            let du = measure_du_mib(&a).await?;
            assert!(du >= (gib as i64) * 1000, "du {du} MiB for a {gib} GiB seed");
            assert_eq!(alive(&a).await?, agentd_before, "measure must not touch agentd");
            // 2. a refusal leaves agentd exactly as it was
            let tiny = CarryConfig { max_work_mib: 100, part_mib, attempts: 3, skip_rebuildable: false, concurrent_carries: 1, spool_root: std::env::temp_dir().join(format!("arbos-carry-e2e-{stamp}")) };
            let (source_free_mib, max_file_bytes) = source_room(&a).await?;
            eprintln!("source: {source_free_mib} MiB free at {STAGE_PARENT}, biggest file {max_file_bytes} bytes");
            let mut room = Room { work_mib: du, spool_free_mib: 1 << 20, source_free_mib, max_part_mib: max_part_bound_mib(part_mib, max_file_bytes), target_free_mib: None };
            let v = verdict(&room, &tiny);
            assert!(matches!(v, Verdict::TooLarge { .. }), "{v:?}");
            assert_eq!(alive(&a).await?, agentd_before, "a refused attempt must not touch agentd");
            eprintln!("refused at {du} MiB with a 100 MiB limit; agentd untouched");
            // 3. the real carry
            let cfg = CarryConfig { max_work_mib: DEFAULT_MAX_WORK_MIB, ..tiny.clone() };
            room.spool_free_mib = free_mib(&cfg.spool_root)?;
            assert_eq!(verdict(&room, &cfg), Verdict::Carry);
            quiesce(&a).await?;
            let entries = list_work(&a, cfg.attempts).await?;
            let m = measure(&entries);
            eprintln!("listed {} entries / {} MiB; rebuildable {} entries / {} MiB in {:?}; skipped special {}", m.entries, m.bytes / MIB, m.rebuildable_entries, m.rebuildable_bytes / MIB, m.projects_with_copies, m.skipped_special);
            assert_eq!(m.projects_with_copies, vec!["proj-a".to_owned()]);
            assert_eq!(m.skipped_special, 1, "the socket");
            let parts = partition(&entries, cfg.part_mib * MIB, false);
            let expected = expected_after_carry(&parts);
            eprintln!("plan: {} parts of ≤ {} MiB; expected {expected:?}", parts.len(), cfg.part_mib);
            let t_pack = std::time::Instant::now();
            let spool = cfg.spool_root.join("e2e");
            let spooled = pack_parts(&a, &parts, &spool, &cfg, |k, n| eprintln!("  pack {k}/{n}")).await?;
            let tar_total: u64 = spooled.iter().map(|p| p.tar_bytes).sum();
            let n_pieces: usize = spooled.iter().map(|p| p.pieces.len()).sum();
            eprintln!("packed {} parts / {n_pieces} pieces, {} MiB of tar (biggest part {} MiB), in {:?}", spooled.len(), tar_total / MIB, max_part_mib(&spooled), t_pack.elapsed());
            let gone = exec_ok(&a.exec(&format!("test -e {STAGE_DIR} && echo present || echo gone"), 30).await?, "stage", &[0])?;
            assert_eq!(gone.trim(), "gone", "staging dir left on the source");
            let (avail, inodes) = destination_room(&b).await?;
            eprintln!("destination: {avail} MiB free, inodes {inodes:?}");
            destination_fits(avail, inodes, du as u64, expected.0, cfg.part_mib, max_part_mib(&spooled)).map_err(|e| anyhow!(e))?;
            let t_unpack = std::time::Instant::now();
            unpack_parts(&b, &spooled, expected, &cfg, |k, n| eprintln!("  unpack {k}/{n}")).await?;
            eprintln!("unpacked in {:?}", t_unpack.elapsed());
            let m_dst = exec_ok(&b.exec(manifest_cmd, 1800).await?, "manifest dst", &[0])?;
            eprintln!("destination manifest: {}", m_dst.replace('\n', " "));
            assert_eq!(m_dst.trim(), m_src.trim(), "destination /work differs from the source");
            let gone = exec_ok(&b.exec(&format!("test -e {STAGE_DIR} && echo present || echo gone"), 30).await?, "stage", &[0])?;
            assert_eq!(gone.trim(), "gone", "staging dir left on the destination");
            assert!(std::fs::read_dir(&spool).map(|d| d.count()).unwrap_or(0) == 0, "spool not emptied");
            let _ = std::fs::remove_dir_all(&cfg.spool_root);
            eprintln!("e2e ok: {gib} GiB / {} entries carried in {} parts; total {:?}", expected.0, spooled.len(), t0.elapsed());
            Ok(())
        }
        .await;
        if !keep {
            for id in [&ids.0, &ids.1] {
                let _ = dt.stop(id, true).await;
                let _ = dt
                    .wait_for(id, Duration::from_secs(120), |s| {
                        s.state == crate::daytona::SandboxState::Stopped
                    })
                    .await;
                match dt.delete(id).await {
                    Ok(_) => eprintln!("deleted {id}"),
                    Err(e) => eprintln!("delete {id}: {e}"),
                }
            }
        }
        result.expect("carry e2e");
    }
}
