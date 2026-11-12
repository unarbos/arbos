//! `POST /box/prepare-sleep` (docs/box-scaling.md §3.2): what the box does to itself before its
//! supervisor stops or checkpoints it, and what it reports back so the supervisor can choose.
//!
//! The answer is a report, not an action. agentd does not exit here: the supervisor reads
//! `active_user_pids`, and either stops the sandbox (SIGTERM is the flush-and-exit path the box
//! has always had, `Supervisor::shutdown`) or checkpoints it with every process in place.
//!
//! Three things here delete, and each is held to what it can prove:
//!
//! - **Caches** ([`SLEEP_CACHES`], and `/tmp` on a box that is not a dev machine) go only when
//!   the workspace cgroups were read and hold no process at all. A box about to be checkpointed
//!   keeps them: a dev server started with `npx` runs out of `~/.npm`, and `tmux` keeps its
//!   socket in `/tmp`, so purging under a process that will be restored breaks it on wake.
//! - **Trash** is `/work/.trash`, by age. Nothing in the tree writes a trash yet, so there is no
//!   record of when an entry was deleted; the entry's own clock is read instead (see
//!   [`sweep_trash`]).
//! - **SQLite** is checkpointed, and vacuumed only past [`VACUUM_FREE_PCT`].

use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::agent::Agent;
use crate::store::Store;
use crate::workspace::Supervisor;

/// The caches a sleeping box does not keep, relative to a workspace's home
/// (`/work/<project>/.arbos/home`). Each is one a tool refills on its own the next time it runs.
///
/// The spec also names the Playwright cache (`.cache/ms-playwright`). It is left out on purpose:
/// those are installed browsers, and Playwright does not fetch them again by itself — a script
/// fails until someone runs `playwright install`. Adding it is one entry here.
pub const SLEEP_CACHES: &[&str] = &[".cache/pip", ".cache/uv", ".npm"];

/// How long an entry stays in `/work/.trash`.
pub const TRASH_KEEP_MS: i64 = 7 * 24 * 60 * 60 * 1000;
const TRASH_DIR: &str = ".trash";

/// Free pages, as a share of the file, past which the store is vacuumed.
const VACUUM_FREE_PCT: i64 = 25;

/// Why this agent is not idle (§3.1, conditions 1 and 2), or `None` when it is.
///
/// A job parked on an approval card is open and reads `queued`, and it is deliberately not
/// counted: it holds no worker and no permit, the card is a row in SQLite, and the decision
/// resumes the job whenever it comes (`approval.rs`). An unanswered card must not keep a box
/// awake.
pub(crate) fn agent_busy(a: &Agent) -> Option<String> {
    if a.any_turn_running() {
        return Some("a turn is running".into());
    }
    let sends = a.sends_waiting();
    if sends > 0 {
        return Some(format!("{sends} sends are waiting"));
    }
    // An unreadable store is not an idle one.
    let open = match a.store.open_jobs() {
        Ok(jobs) => jobs,
        Err(e) => return Some(format!("jobs could not be read: {e}")),
    };
    let working = open
        .iter()
        .filter(|j| !matches!(a.store.pending_approval_for_job(&j.id), Ok(Some(_))))
        .count();
    (working > 0).then(|| format!("{working} background jobs are running or queued"))
}

async fn busy(sup: &Supervisor) -> Option<String> {
    if !sup.boot.ready() {
        return Some("the box is still starting its workspaces".into());
    }
    for w in sup.list().await {
        if let Some(why) = agent_busy(&w.agent) {
            return Some(format!("{}: {why}", w.project_id));
        }
    }
    None
}

/// The processes in one workspace cgroup that are not agentd's own, or `None` when the cgroup
/// cannot be read — which is not the same as empty, and must not be reported as it.
///
/// `own` is agentd's pid and the children it holds a handle for. Everything else in the cgroup
/// is something a tool call started and left running, which is what a clean stop would kill.
pub(crate) fn user_pids(cgroup: &Path, own: &HashSet<u32>) -> Option<Vec<u32>> {
    std::fs::metadata(cgroup.join("cgroup.procs")).ok()?;
    let mut pids: Vec<u32> = crate::isolation::cgroup_procs(cgroup)
        .into_iter()
        .filter(|p| !own.contains(p))
        .collect();
    pids.sort_unstable();
    pids.dedup();
    Some(pids)
}

/// Remove the [`SLEEP_CACHES`] of one workspace home; returns the bytes that went.
///
/// A cache is removed only when resolving it lands exactly where it was named, so a link
/// anywhere on the way (`~/.cache` pointed at another folder) leaves it alone.
fn purge_caches(home: &Path, budget: &mut usize) -> u64 {
    let Ok(real_home) = home.canonicalize() else {
        return 0;
    };
    let mut freed = 0;
    for rel in SLEEP_CACHES {
        let path = real_home.join(rel);
        if !path.is_dir() || path.canonicalize().ok().as_deref() != Some(path.as_path()) {
            continue;
        }
        let bytes = crate::disk::dir_size(&path, budget);
        if std::fs::remove_dir_all(&path).is_ok() {
            freed += bytes;
        }
    }
    freed
}

/// Empty `dir`, keeping the directory itself. A link inside it is removed, never followed.
fn clear_dir(dir: &Path, budget: &mut usize) -> u64 {
    let bytes = crate::disk::dir_size(dir, budget);
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let _ = match e.file_type() {
            Ok(t) if t.is_dir() => std::fs::remove_dir_all(e.path()),
            _ => std::fs::remove_file(e.path()),
        };
    }
    bytes
}

/// Delete the entries of `trash` older than [`TRASH_KEEP_MS`]; returns how many went.
///
/// An entry's age is read from the later of its mtime and its ctime. mtime alone would be wrong:
/// it survives a move, so a file last edited a month ago and trashed a minute ago would read a
/// month old. A rename changes ctime, so the later of the two is no earlier than the move.
fn sweep_trash(trash: &Path, now_ms: i64) -> usize {
    if trash.is_symlink() {
        return 0;
    }
    let mut removed = 0;
    for e in std::fs::read_dir(trash).into_iter().flatten().flatten() {
        // `DirEntry::metadata` does not follow a link.
        let Ok(md) = e.metadata() else {
            continue;
        };
        if now_ms - md.mtime().max(md.ctime()) * 1000 <= TRASH_KEEP_MS {
            continue;
        }
        let gone = if md.is_dir() {
            std::fs::remove_dir_all(e.path())
        } else {
            std::fs::remove_file(e.path())
        };
        removed += usize::from(gone.is_ok());
    }
    removed
}

fn size_of(path: &Path, budget: &mut usize) -> u64 {
    match std::fs::symlink_metadata(path) {
        Ok(md) if md.is_dir() => crate::disk::dir_size(path, budget),
        Ok(md) if md.is_file() => md.blocks() * 512,
        _ => 0,
    }
}

/// `(files, scratch, trash)` in bytes on disk under the work root.
///
/// Nothing measured this split before, so it is defined here: **trash** is `/work/.trash`,
/// **scratch** is everything Arbos and the tools keep beside a project (`/work/<project>/.arbos`
/// — the workspace home with its caches and toolchains, sessions, the store — and the box's own
/// `/work/.arbos`), and **files** is the rest of each project folder.
fn measure(work_root: &Path, budget: &mut usize) -> (u64, u64, u64) {
    let (mut files, mut scratch, mut trash) = (0, 0, 0);
    for e in std::fs::read_dir(work_root).into_iter().flatten().flatten() {
        let name = e.file_name();
        if name == TRASH_DIR {
            trash += size_of(&e.path(), budget);
        } else if name.to_string_lossy().starts_with('.') {
            scratch += size_of(&e.path(), budget);
        } else if e.file_type().is_ok_and(|t| t.is_dir()) {
            for c in std::fs::read_dir(e.path()).into_iter().flatten().flatten() {
                let bytes = size_of(&c.path(), budget);
                if c.file_name() == ".arbos" {
                    scratch += bytes;
                } else {
                    files += bytes;
                }
            }
        } else {
            files += size_of(&e.path(), budget);
        }
    }
    (files, scratch, trash)
}

/// Fold the WAL into the store and, past [`VACUUM_FREE_PCT`] free pages, rebuild the file.
/// Returns whether it vacuumed.
fn compact(store: &Store) -> anyhow::Result<bool> {
    store.with(|c| {
        c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        let pages: i64 = c.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let free: i64 = c.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
        let vacuum = free * 100 > pages * VACUUM_FREE_PCT;
        if vacuum {
            // A vacuum in WAL mode writes the rebuilt file through the WAL.
            c.execute_batch("VACUUM")?;
            c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        }
        Ok(vacuum)
    })
}

/// Run the whole hook. `Err` is the reason the box may not sleep, which the caller answers 409.
///
/// One call at a time, and a second call finds nothing left to do, so calling it twice — in a
/// row or at once — is the same as calling it once.
pub async fn prepare(sup: &Supervisor) -> Result<Value, String> {
    static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _one = ONE_AT_A_TIME.lock().await;
    if let Some(why) = busy(sup).await {
        return Err(why);
    }
    let workspaces = sup.list().await;
    // `None` once any workspace has no cgroup to read: then nobody knows what is running, and
    // the answer says so (`null`) instead of saying zero.
    let mut user: Option<Vec<u32>> = Some(Vec::new());
    let mut children_alive = false;
    for w in &workspaces {
        // Flush: what the rooms hold in memory goes into the store the checkpoint below folds.
        w.sync.persist_all().await;
        let held = w.agent.child_pids();
        children_alive |= !held.is_empty();
        let own: HashSet<u32> = held.into_iter().chain([std::process::id()]).collect();
        let cgroup = w.cfg.run_as.as_ref().and_then(|r| r.cgroup.as_deref());
        user = match (user, cgroup.and_then(|cg| user_pids(cg, &own))) {
            (Some(mut all), Some(pids)) => {
                all.extend(pids);
                Some(all)
            }
            _ => None,
        };
    }
    // Only a box that will be stopped clean loses its caches (see the module note), and `/tmp`
    // is the machine's own on a local box, never ours to empty.
    let purge = user.as_ref().is_some_and(Vec::is_empty) && !children_alive;
    let homes: Vec<PathBuf> = workspaces
        .iter()
        .filter(|_| purge)
        .map(|w| w.cfg.state_dir.join("home"))
        .collect();
    let tmp = (purge && !sup.cfg.local_box).then(|| PathBuf::from("/tmp"));
    let stores: Vec<(String, Store)> = workspaces
        .iter()
        .map(|w| (w.project_id.clone(), w.store.clone()))
        .collect();
    let work_root = sup.cfg.work_root.clone();
    let mut report = tokio::task::spawn_blocking(move || {
        let mut budget = usize::MAX;
        let mut purged: u64 = homes.iter().map(|h| purge_caches(h, &mut budget)).sum();
        if let Some(tmp) = &tmp {
            purged += clear_dir(tmp, &mut budget);
        }
        let trash_removed = sweep_trash(&work_root.join(TRASH_DIR), arbos_core::ids::now_ms());
        let mut vacuumed = Vec::new();
        for (project, store) in &stores {
            match compact(store) {
                Ok(true) => vacuumed.push(project.clone()),
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(%project, error = %e, "prepare-sleep: store not compacted")
                }
            }
        }
        let (files, scratch, trash) = measure(&work_root, &mut budget);
        json!({
            "ok": true,
            "files_bytes": files, "scratch_bytes": scratch, "trash_bytes": trash,
            "purged_bytes": purged, "trash_removed": trash_removed, "vacuumed": vacuumed,
        })
    })
    .await
    .map_err(|e| format!("prepare-sleep did not finish: {e}"))?;
    // Asked again last: a message that arrived while the disk was being walked wins.
    if let Some(why) = busy(sup).await {
        return Err(why);
    }
    report["active_user_pids"] = json!(user.as_ref().map(Vec::len));
    report["user_pids"] = json!(user);
    tracing::info!(%report, "prepare-sleep");
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("arbos-sleep-{tag}-{}", arbos_core::ids::ulid()));
        std::fs::create_dir_all(&d).unwrap();
        d.canonicalize().unwrap()
    }

    /// A walk budget nothing here reaches.
    fn all() -> usize {
        usize::MAX
    }

    fn file(path: &Path, bytes: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![7u8; bytes]).unwrap();
    }

    /// The cgroup read, on a directory standing in for the kernel's: agentd and the children it
    /// holds are left out, a process in a leaf below is found, and a cgroup that is not there is
    /// "unknown" rather than "empty" — the difference between a checkpoint and a clean stop
    /// that kills somebody's dev server.
    #[test]
    fn user_pids_are_the_cgroup_minus_agentd_and_unknown_is_not_empty() {
        let root = scratch("pids");
        let cg = root.join("ws-a");
        std::fs::create_dir_all(cg.join("w-pi-main")).unwrap();
        std::fs::write(cg.join("cgroup.procs"), "40\n10\n").unwrap();
        std::fs::write(cg.join("w-pi-main").join("cgroup.procs"), "11\n41\n").unwrap();
        let own: HashSet<u32> = [10, 11].into();
        assert_eq!(user_pids(&cg, &own), Some(vec![40, 41]));
        assert_eq!(user_pids(&cg, &[10, 11, 40, 41].into()), Some(vec![]));
        assert_eq!(user_pids(&root.join("no-such-cgroup"), &own), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Only the named caches go, and none of them through a link: `~/.cache` pointed at another
    /// folder is somebody's folder, whatever it is called.
    #[test]
    fn the_purge_takes_the_named_caches_and_follows_no_link() {
        let root = scratch("purge");
        let home = root.join("home");
        file(&home.join(".cache/pip/wheels/a.whl"), 8192);
        file(&home.join(".cache/uv/b"), 10);
        file(&home.join(".npm/_cacache/c"), 10);
        file(&home.join(".cache/ms-playwright/chromium/chrome"), 10);
        file(&home.join(".config/keep"), 10);
        let freed = purge_caches(&home, &mut all());
        assert!(freed >= 8192, "freed {freed}");
        for gone in SLEEP_CACHES {
            assert!(!home.join(gone).exists(), "{gone} stayed");
        }
        assert!(home.join(".cache/ms-playwright/chromium/chrome").exists());
        assert!(home.join(".config/keep").exists());

        let outside = root.join("outside");
        file(&outside.join("pip/precious"), 10);
        file(&outside.join("npm/precious"), 10);
        let linked = root.join("linked-home");
        std::fs::create_dir_all(&linked).unwrap();
        std::os::unix::fs::symlink(&outside, linked.join(".cache")).unwrap();
        std::os::unix::fs::symlink(outside.join("npm"), linked.join(".npm")).unwrap();
        assert_eq!(purge_caches(&linked, &mut all()), 0);
        assert!(outside.join("pip/precious").exists(), "followed ~/.cache");
        assert!(outside.join("npm/precious").exists(), "followed ~/.npm");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `/tmp`'s stand-in is emptied and kept, and a link in it is unlinked, not walked.
    #[test]
    fn clearing_a_directory_keeps_it_and_what_its_links_point_at() {
        let root = scratch("tmp");
        let tmp = root.join("tmp");
        file(&tmp.join("a/b/c"), 10);
        file(&tmp.join("d"), 10);
        file(&root.join("outside/precious"), 10);
        std::os::unix::fs::symlink(root.join("outside"), tmp.join("link")).unwrap();
        clear_dir(&tmp, &mut all());
        assert_eq!(std::fs::read_dir(&tmp).unwrap().count(), 0);
        assert!(root.join("outside/precious").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Seven days, read from the entry's own clock — and an entry edited long ago but moved in
    /// just now is new, which is the case mtime alone gets wrong.
    #[test]
    fn trash_goes_after_seven_days_by_when_it_arrived() {
        let root = scratch("trash");
        let trash = root.join(TRASH_DIR);
        file(&trash.join("deleted-folder/inner"), 10);
        file(&trash.join("deleted-file"), 10);
        let month_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86400);
        std::fs::File::options()
            .write(true)
            .open(trash.join("deleted-file"))
            .unwrap()
            .set_modified(month_ago)
            .unwrap();
        let now = arbos_core::ids::now_ms();
        assert_eq!(
            sweep_trash(&trash, now),
            0,
            "an old mtime is not an old delete"
        );
        assert_eq!(sweep_trash(&trash, now + TRASH_KEEP_MS - 60_000), 0);
        assert_eq!(sweep_trash(&trash, now + TRASH_KEEP_MS + 60_000), 2);
        assert!(trash.is_dir(), "the trash itself stays");
        assert_eq!(sweep_trash(&trash, now + TRASH_KEEP_MS + 60_000), 0);
        assert_eq!(sweep_trash(&root.join("no-trash"), now), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_breakdown_splits_files_scratch_and_trash() {
        let root = scratch("measure");
        file(&root.join("proj/src/main.rs"), 8192);
        file(&root.join("proj/notes.md"), 8192);
        file(&root.join("proj/.arbos/home/.cache/x"), 8192);
        file(&root.join(".arbos/ballast"), 8192);
        file(&root.join(".trash/old"), 8192);
        let (files, scratch, trash) = measure(&root, &mut all());
        assert!(files >= 2 * 8192 && scratch >= 2 * 8192 && trash >= 8192);
        // Blocks, so a filesystem may round each file up — but never by a whole other file.
        assert!(files < 3 * 8192 + 8192 && scratch < 3 * 8192 + 8192 && trash < 2 * 8192);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The checkpoint empties the WAL every time; the vacuum runs once the free list is past a
    /// quarter of the file, and not again when there is nothing left to give back.
    #[test]
    fn the_store_is_checkpointed_and_vacuumed_only_when_a_quarter_is_free() {
        let root = scratch("store");
        let db = root.join("agentd.db");
        let store = Store::open(&db).unwrap();
        let wal = root.join("agentd.db-wal");
        assert!(!compact(&store).unwrap(), "a fresh store has no free pages");
        assert_eq!(std::fs::metadata(&wal).unwrap().len(), 0);

        store
            .with(|c| {
                c.execute_batch(
                    "CREATE TABLE filler(x BLOB);
                     WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 400)
                     INSERT INTO filler SELECT zeroblob(4000) FROM n;
                     DROP TABLE filler;",
                )
            })
            .unwrap();
        let before = std::fs::metadata(&wal).unwrap().len();
        assert!(before > 0, "the premise: the writes are still in the WAL");
        assert!(compact(&store).unwrap(), "most of the file is free pages");
        assert_eq!(std::fs::metadata(&wal).unwrap().len(), 0);
        assert!(std::fs::metadata(&db).unwrap().len() < before);
        assert!(
            !compact(&store).unwrap(),
            "a second call finds nothing to do"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
