//! Carrying a box's `/work` through the relay in parts (ARB-339).
//!
//! The one-piece carry (`tar` of all of `/work` → one download → one 30-s multipart upload) failed
//! three ways in production: upload timeouts and 502s from the toolbox proxy above a few hundred MB,
//! download stream resets, and the relay's own `MAX_WORK_MIB` refusal — which came *after* the
//! box's agentd had been killed, so an oversize box was restarted on every attempt (gh_268993031,
//! 3979 MiB, 2026-10-26). Now:
//!
//! 1. **Measure first.** `du -sm /work` runs while agentd keeps serving. Over the limit (or no room
//!    on the relay's spool disk) → a clean refusal, nothing stopped, box state untouched.
//! 2. **Quiesce, list, plan.** agentd is stopped, `/work` is listed once (`find -printf`), and the
//!    listing is cut into parts of at most `part_mib` apparent bytes, in walk order (a directory
//!    precedes its children). Every part is its own tarball (`tar --no-recursion -T list`), so a
//!    part never needs the others to extract.
//! 3. **Pack per part.** One part at a time on the source box: tar → sha256 → download (long
//!    per-part deadline, retried) → verify → spool to the relay's disk → delete on the source. The
//!    source never holds more than one part's tarball; the relay never holds more than one part in
//!    memory.
//! 4. **Unpack per part.** On the new box: room check (bytes *and* inodes, the same floor as
//!    agentd's disk gate), then per part: upload (deadline scaled to the size, retried) → sha256
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
/// Same floor as `arbos_agentd::disk::INODE_FLOOR`: a destination with fewer free file slots than
/// the carried tree needs plus this is refused before anything is uploaded.
pub const INODE_FLOOR: u64 = 1024;
/// Disk the relay keeps free on its spool filesystem beyond the carry itself (MiB).
pub const SPOOL_MARGIN_MIB: u64 = 2048;
/// Room the new box keeps beyond the carried bytes and one staged part (MiB).
pub const DEST_MARGIN_MIB: u64 = 512;

const MIB: u64 = 1024 * 1024;
/// Staging directory on both boxes (the toolbox user's home; readable by the toolbox download).
pub const STAGE_DIR: &str = "/home/daytona/arbos-upgrade";
/// Relative path (from `/work`) of the tree agentd rebuilds from the image seed.
pub const REBUILDABLE_SUFFIX: &str = "/.arbos/pi/extensions/arbos-pi-ext/node_modules";

/// Knobs, read once per upgrade.
#[derive(Debug, Clone)]
pub struct CarryConfig {
    pub max_work_mib: i64,
    pub part_mib: u64,
    pub attempts: u32,
    pub skip_rebuildable: bool,
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
}

impl Verdict {
    pub fn reason(&self) -> Option<String> {
        match self {
            Verdict::Carry => None,
            Verdict::TooLarge { mib, max_mib } => Some(format!(
                "/work is {mib} MiB; too large to carry through the relay (limit {max_mib} MiB, ARBOS_UPGRADE_MAX_WORK_MIB)"
            )),
            Verdict::NoSpoolRoom { need_mib, free_mib } => Some(format!(
                "/work is {need_mib} MiB but the relay's spool disk has {free_mib} MiB free; too large to carry through the relay right now"
            )),
        }
    }
}

/// Decide from the `du -sm /work` figure and the free MiB on the spool filesystem.
pub fn verdict(du_mib: i64, spool_free_mib: u64, cfg: &CarryConfig) -> Verdict {
    if du_mib > cfg.max_work_mib {
        return Verdict::TooLarge {
            mib: du_mib,
            max_mib: cfg.max_work_mib,
        };
    }
    let need = du_mib.max(0) as u64 + SPOOL_MARGIN_MIB;
    if spool_free_mib < need {
        return Verdict::NoSpoolRoom {
            need_mib: du_mib.max(0) as u64,
            free_mib: spool_free_mib,
        };
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

/// Does the new box have room? Bytes: the whole carry, plus the biggest part's tarball staged
/// for extraction, plus two pieces in flight (the uploaded piece and the daemon's `/tmp` copy of
/// it), plus a margin. Inodes (when the filesystem counts them): one per entry plus the gate's floor.
pub fn destination_fits(
    avail_mib: u64,
    avail_inodes: Option<u64>,
    need_mib: u64,
    entries: u64,
    part_mib: u64,
    max_part_mib: u64,
) -> std::result::Result<(), String> {
    let need_bytes = need_mib + max_part_mib + 2 * part_mib + DEST_MARGIN_MIB;
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

/// List `/work` on a quiesced box and parse it.
pub async fn list_work(tb: &Toolbox<'_>, attempts: u32) -> Result<Vec<Entry>> {
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

/// Byte ranges `(offset, len)` that cut `total` into pieces of at most `piece`.
pub fn piece_ranges(total: u64, piece: u64) -> Vec<(u64, u64)> {
    let piece = piece.max(1);
    let mut out = Vec::new();
    let mut off = 0;
    while off < total {
        let len = piece.min(total - off);
        out.push((off, len));
        off += len;
    }
    if out.is_empty() {
        out.push((0, 0));
    }
    out
}

/// Pack every part on the source box and spool it on the relay. The source holds one part's
/// tarball plus one piece at a time; the relay holds one piece in memory. `progress(k, n)` is
/// called before part `k` of `n` is made.
pub async fn pack_parts<P>(
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
    let piece_bytes = cfg.part_mib * MIB;
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
        let mut pieces = Vec::new();
        for (j, (off, len)) in piece_ranges(tar_bytes, piece_bytes).into_iter().enumerate() {
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
            let (sha_ok, piece_ok, file_ok) = (piece_sha.clone(), piece.clone(), file.clone());
            retry(
                &format!("download piece {j} of part {k}"),
                cfg.attempts,
                |_| {
                    let (sha, piece, file) = (sha_ok.clone(), piece_ok.clone(), file_ok.clone());
                    async move {
                        let bytes = tb.download(&piece, transfer_timeout(len)).await?;
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
            .await?;
            let _ = tb.exec(&format!("rm -f {piece}"), 60).await;
            pieces.push(SpooledPiece {
                file,
                sha256: piece_sha,
                bytes: len,
            });
        }
        let _ = tb.exec(&format!("sudo -n rm -f {tgz} {list}"), 60).await;
        out.push(SpooledPart {
            index: k,
            sha256: sha,
            tar_bytes,
            pieces,
        });
    }
    let _ = tb.exec(&format!("sudo -n rm -rf {STAGE_DIR}"), 60).await;
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
    if (got_n, got_b) != expected {
        return Err(anyhow!(
            "carried tree does not match the plan: {got_n} entries / {got_b} bytes on the new box, {} entries / {} bytes packed",
            expected.0,
            expected.1
        ));
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

    /// The limit is a sanity stop far above today's boxes: Jacob's 3979 MiB carries; 16 GiB + 1 MiB
    /// does not, and the refusal keeps the phrase the deploy worker's skip rule matches.
    #[test]
    fn limit_is_16_gib_and_refusal_keeps_the_skip_phrase() {
        let c = cfg();
        assert_eq!(verdict(3979, 500_000, &c), Verdict::Carry);
        assert_eq!(verdict(16 * 1024, 500_000, &c), Verdict::Carry);
        let v = verdict(16 * 1024 + 1, 500_000, &c);
        assert!(matches!(v, Verdict::TooLarge { .. }));
        assert!(v.reason().unwrap().contains("too large to carry"));
        let v = verdict(4000, 4000, &c);
        assert!(matches!(v, Verdict::NoSpoolRoom { .. }));
        assert!(v.reason().unwrap().contains("too large to carry"));
        assert_eq!(verdict(4000, 4000 + SPOOL_MARGIN_MIB, &c), Verdict::Carry);
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
    fn piece_ranges_cover_the_tarball() {
        assert_eq!(piece_ranges(0, 256), vec![(0, 0)]);
        assert_eq!(piece_ranges(100, 256), vec![(0, 100)]);
        assert_eq!(piece_ranges(256, 256), vec![(0, 256)]);
        assert_eq!(
            piece_ranges(600, 256),
            vec![(0, 256), (256, 256), (512, 88)]
        );
        let gib = 1024 * MIB;
        let r = piece_ranges(gib + 1, 256 * MIB);
        assert_eq!(r.len(), 5);
        assert_eq!(r.iter().map(|(_, l)| l).sum::<u64>(), gib + 1);
        assert_eq!(max_part_mib(&[]), 0);
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
            let tiny = CarryConfig { max_work_mib: 100, part_mib, attempts: 3, skip_rebuildable: false, spool_root: std::env::temp_dir().join(format!("arbos-carry-e2e-{stamp}")) };
            let v = verdict(du, 1 << 20, &tiny);
            assert!(matches!(v, Verdict::TooLarge { .. }), "{v:?}");
            assert_eq!(alive(&a).await?, agentd_before, "a refused attempt must not touch agentd");
            eprintln!("refused at {du} MiB with a 100 MiB limit; agentd untouched");
            // 3. the real carry
            let cfg = CarryConfig { max_work_mib: DEFAULT_MAX_WORK_MIB, ..tiny.clone() };
            assert_eq!(verdict(du, free_mib(&cfg.spool_root)?, &cfg), Verdict::Carry);
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
