//! Local extras for a session: draft, rank, and a cached transcript.
//!
//! Kernel SQLite (`<workspace>/.arbos/sessions.db`) is session truth. These
//! JSON files live under `.arbos/desktop/sessions/` so a write cannot touch
//! the kernel store. One file each, so writing one does not rewrite the rest.
//!
//! Written as the session changes rather than when it ends, so a draft
//! survives a crash and not just an orderly quit.

use crate::model::{project, session::ChatItem};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Condvar, Mutex, MutexGuard, Once, OnceLock, PoisonError},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Serialize, Deserialize)]
pub struct Record {
    /// Label written with the transcript. Always the kernel now; an old
    /// file may still say a catalog name.
    pub agent: String,
    /// The kernel chat id, which is what `open` with `session_id` resumes.
    /// Absent when the session never reached the kernel.
    #[serde(default)]
    pub session: Option<String>,
    /// The parent's kernel session id, when this chat is a child agent.
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegate_number: Option<u64>,
    pub title: String,
    pub name: Option<String>,
    /// Seconds since the epoch — `SystemTime` has no serialization of its own,
    /// and this file is read by a later build than wrote it.
    pub updated: u64,
    /// Whether the user archived it. A closed session sinks below the ones
    /// still going on, and typing into it brings it back.
    #[serde(default)]
    pub closed: bool,
    /// Sidebar place among siblings. The user sets this by dragging; a
    /// running turn must not.
    #[serde(default)]
    pub rank: i64,
    /// How long the kickoff turn took. That turn has no prompt to stamp,
    /// so without this the fold read *Worked 7s* until the relaunch and
    /// the run's own words after it (F-223, cycle 58).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kickoff_secs: Option<u32>,
    /// When the kickoff turn was asked for, in milliseconds since the
    /// epoch. The turn is known by this and not by its shape: without it a
    /// relaunch took the kickoff's first call for the turn's opener and
    /// drew the fold over the calls after it — a one-call kickoff lost its
    /// *Worked 4s* line altogether — and the day line over the turn went
    /// with it (F-244, cycle 75).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kickoff_at: Option<u64>,
    pub items: Vec<ChatItem>,
    /// What was sitting in the composer when the file was last written.
    /// Empty is the common case and stays off the wire.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub draft: String,
}

impl Record {
    pub fn at(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(self.updated)
    }
}

const SESSIONS: &str = "sessions";

fn dir(project: &Path) -> PathBuf {
    project::dir(project).join(SESSIONS)
}

/// Every session filed in the project with the file it came from. Order is
/// the user's rank, then the file name — never last-written, or a running
/// turn would reshuffle the list on every launch.
pub fn list(project: &Path) -> Vec<(PathBuf, Record)> {
    project::adopt(project);
    let mut found = list_in(&dir(project));
    // Older builds wrote under `.cydonia/sessions` or beside the kernel
    // store. Read those only when the new folder is empty, so a migrate
    // that copied the files is not listed twice.
    if found.is_empty() {
        found.extend(list_in(&project.join(".cydonia").join(SESSIONS)));
        found.extend(list_in(&project.join(".arbos").join(SESSIONS)));
    }
    found.sort_by(|(a_path, a), (b_path, b)| {
        a.rank
            .cmp(&b.rank)
            .then_with(|| a_path.file_name().cmp(&b_path.file_name()))
    });
    found
}

fn list_in(dir: &Path) -> Vec<(PathBuf, Record)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .filter_map(|path| {
            let body = std::fs::read_to_string(&path).ok()?;
            Some((path, serde_json::from_str(&body).ok()?))
        })
        .collect()
}

/// Mint the file a session is written to from here on. Called on the first
/// write and not before: opening a project must not put an `.arbos/desktop/`
/// in it.
pub fn create(project: &Path) -> Option<PathBuf> {
    let dir = project::init(project).ok()?.join(SESSIONS);
    std::fs::create_dir_all(&dir).ok()?;
    let stamp = project::stamp();
    let mut path = dir.join(format!("{stamp}.json"));
    for n in 2.. {
        if !path.exists() {
            break;
        }
        path = dir.join(format!("{stamp}-{n}.json"));
    }
    Some(path)
}

/// Queue `record` to be written to `file`. Returns at once: the JSON is made
/// and written on the saver thread, and the UI thread never waits on it.
///
/// Latest wins. A session that changes ten times before the saver gets to it
/// is written once, with the last state; the ones in between were never what
/// the file needed to say. Best effort, like [`remove`]: a session that
/// cannot be written is not worth failing a turn over.
pub fn write(file: &Path, record: Record) {
    let saver = saver();
    let mut queue = saver.lock();
    queue.pending.insert(file.to_path_buf(), record);
    saver.changed.notify_all();
}

/// Delete `file`, and forget any write still queued for it — a save that
/// landed after the delete would bring the session back on the next launch.
pub fn remove(file: &Path) {
    let saver = saver();
    let mut queue = saver.lock();
    queue.pending.remove(file);
    // The saver may be mid-write on this very file. Let it finish so the
    // rename cannot land after the delete.
    let deadline = std::time::Instant::now() + SETTLE_LIMIT;
    while queue.writing.as_deref() == Some(file) {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break;
        }
        queue = saver.wait(queue, left);
    }
    drop(queue);
    let _ = std::fs::remove_file(file);
}

/// Wait for every queued write to reach disk, for at most [`SETTLE_LIMIT`].
/// For the way out: a quit that raced the saver would lose the last change.
pub fn settle() {
    let saver = saver();
    let mut queue = saver.lock();
    let deadline = std::time::Instant::now() + SETTLE_LIMIT;
    while !queue.pending.is_empty() || queue.writing.is_some() {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break;
        }
        queue = saver.wait(queue, left);
    }
}

/// The most a caller will block on the saver. A write is milliseconds; this
/// is only what stops a stuck disk from holding the window hostage.
const SETTLE_LIMIT: Duration = Duration::from_secs(2);

/// The one writer of record files. Records queue by file; a single thread
/// takes them one at a time and puts them on disk. Everything shared lives
/// under the one mutex, and a record in flight is owned by the thread that
/// took it — nothing else holds a reference to it.
struct Saver {
    queue: Mutex<Queue>,
    /// Signalled on any change to `queue`: a record queued, one taken, one
    /// written. Both the drain and the waiters in [`remove`] and [`settle`]
    /// sleep on it.
    changed: Condvar,
}

#[derive(Default)]
struct Queue {
    pending: HashMap<PathBuf, Record>,
    /// The file whose record has left `pending` and is on its way to disk.
    writing: Option<PathBuf>,
}

impl Saver {
    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn wait<'a>(&self, queue: MutexGuard<'a, Queue>, limit: Duration) -> MutexGuard<'a, Queue> {
        self.changed
            .wait_timeout(queue, limit)
            .unwrap_or_else(PoisonError::into_inner)
            .0
    }

    /// The saver thread: take one queued record, write it, repeat.
    fn drain(&self) {
        loop {
            let (file, record) = {
                let mut queue = self.lock();
                loop {
                    if let Some(file) = queue.pending.keys().next().cloned() {
                        let record = queue.pending.remove(&file).expect("key just seen");
                        queue.writing = Some(file.clone());
                        break (file, record);
                    }
                    queue = self
                        .changed
                        .wait(queue)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            };
            write_now(&file, &record);
            self.lock().writing = None;
            self.changed.notify_all();
        }
    }
}

fn saver() -> &'static Saver {
    static SAVER: OnceLock<Saver> = OnceLock::new();
    static THREAD: Once = Once::new();
    let saver = SAVER.get_or_init(|| Saver {
        queue: Mutex::new(Queue::default()),
        changed: Condvar::new(),
    });
    THREAD.call_once(|| {
        std::thread::Builder::new()
            .name("record-saver".into())
            .spawn(move || saver.drain())
            .expect("spawn the record saver");
    });
    saver
}

/// Write beside the live file and rename, so a crash mid-save cannot hide
/// the chat on the next launch.
fn write_now(file: &Path, record: &Record) {
    let Ok(body) = serde_json::to_string_pretty(record) else {
        return;
    };
    let tmp = file.with_extension("json.tmp");
    if std::fs::write(&tmp, body).is_ok() && std::fs::rename(&tmp, file).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}
