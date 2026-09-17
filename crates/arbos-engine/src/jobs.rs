//! A job is a folder.
//!
//! Every `bash` command runs as a job under `.arbos/agents/<id>/jobs/jN/`:
//!
//! ```text
//! meta.json   command, cwd, pid, started_ms, timeout_ms   written once at spawn
//! out.log     combined stdout+stderr                       written by the child's own fds
//! exit        "<code>\n"                                   written by the wrapper shell
//! seen        "<byte offset>\n"                            how much the model has been shown
//! detached    present once the tool call returned with the job still running
//! notified    present once the completion reached the transcript
//! ```
//!
//! Status is derived, never stored: `exit` wins, then pid liveness. The child
//! writes its own journal and exit code, so a kernel restart loses nothing;
//! the kernel is a cache over the folder, not its owner.

use anyhow::{Context, Result, bail};
use arbos_core::{AgentId, Place};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::{Child, Command};

/// The author a commit gets when the machine has none: workers on a
/// fresh machine reported "the author identity (user.name / user.email)
/// is not configured" and asked the user (mobile cycle 1, item 3).
pub const DEFAULT_GIT_AUTHOR: (&str, &str) = ("Arbos", "unarbos@users.noreply.github.com");

/// `GIT_AUTHOR_*` and `GIT_COMMITTER_*` for a job in `cwd`: nothing when
/// git has an identity there (the user's config, or an exported variable
/// the allowlist passes), the default otherwise. One `git config` read
/// per cwd per minute.
pub fn git_identity_env(cwd: &Path) -> Vec<(String, String)> {
    for var in ["GIT_AUTHOR_NAME", "GIT_COMMITTER_NAME"] {
        if std::env::var_os(var).is_some_and(|v| !v.is_empty()) {
            return Vec::new();
        }
    }
    static SEEN: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<PathBuf, (std::time::Instant, bool)>>,
    > = std::sync::OnceLock::new();
    let cache = SEEN.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let key = cwd.to_path_buf();
    let cached = cache
        .lock()
        .ok()
        .and_then(|m| m.get(&key).copied())
        .filter(|(at, _)| at.elapsed() < Duration::from_secs(60))
        .map(|(_, has)| has);
    let has_identity = match cached {
        Some(v) => v,
        None => {
            let has = git_has_identity(cwd);
            if let Ok(mut m) = cache.lock() {
                m.insert(key, (std::time::Instant::now(), has));
            }
            has
        }
    };
    if has_identity {
        return Vec::new();
    }
    let (name, email) = DEFAULT_GIT_AUTHOR;
    vec![
        ("GIT_AUTHOR_NAME".into(), name.into()),
        ("GIT_AUTHOR_EMAIL".into(), email.into()),
        ("GIT_COMMITTER_NAME".into(), name.into()),
        ("GIT_COMMITTER_EMAIL".into(), email.into()),
    ]
}

/// Whether `git commit` in `cwd` would find a user.name and user.email
/// (repository, global, or system config). No git at all reads as
/// "has" — nothing to default for.
fn git_has_identity(cwd: &Path) -> bool {
    let read = |key: &str| {
        std::process::Command::new("git")
            .args(["config", "--get", key])
            .current_dir(cwd)
            .stdin(Stdio::null())
            .output()
            .ok()
            .map(|o| o.status.success() && !String::from_utf8_lossy(&o.stdout).trim().is_empty())
    };
    match (read("user.name"), read("user.email")) {
        (Some(n), Some(e)) => n && e,
        _ => true,
    }
}

/// Only this much of a journal is ever loaded for one read.
pub const JOURNAL_WINDOW: u64 = 4 * 1024 * 1024;
/// `out.log` is cut back to empty (with a notice line) when it passes this
/// (qa-025: a `yes` job wrote 8.7 GB in an hour). `ARBOS_JOB_LOG_CAP` in
/// bytes overrides it.
pub const JOURNAL_CAP: u64 = 64 * 1024 * 1024;
/// Finished job folders older than this are pruned at the next spawn.
pub const JOB_TTL: Duration = Duration::from_secs(48 * 3600);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    pub command: String,
    pub cwd: PathBuf,
    pub pid: u32,
    pub started_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Running,
    Exited(i32),
    /// Process gone, no exit file: SIGKILL (timeout, user), or a reboot.
    Killed,
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: String,
    pub dir: PathBuf,
    pub meta: Meta,
    pub status: Status,
    /// mtime of the exit (or killed) file. Not for `Running`.
    pub ended_ms: Option<i64>,
    pub journal_bytes: u64,
    /// Why a `Killed` job ended, when whoever killed it said (the kernel's
    /// kill, the leash when the kernel was gone). None: a signal from
    /// outside, or the machine.
    pub killed_why: Option<String>,
}

impl Job {
    pub fn journal(&self) -> PathBuf {
        self.dir.join("out.log")
    }

    pub fn running(&self) -> bool {
        self.status == Status::Running
    }

    pub fn detached(&self) -> bool {
        self.dir.join("detached").exists()
    }

    /// `running for 41s (pid 4812)` / `exited with code 0 after 41s` / `killed (no exit recorded)`.
    pub fn status_line(&self) -> String {
        match self.status {
            Status::Exited(code) => match self.ended_ms {
                Some(end) => format!(
                    "exited with code {code} after {}",
                    human_secs(end - self.meta.started_ms)
                ),
                None => format!("exited with code {code}"),
            },
            Status::Killed => {
                let after = self
                    .ended_ms
                    .map(|end| format!(" after {}", human_secs(end - self.meta.started_ms)))
                    .unwrap_or_default();
                match &self.killed_why {
                    Some(why) => format!("{why}{after}"),
                    None => {
                        format!("killed by a signal from outside the kernel{after} (no exit code)")
                    }
                }
            }
            Status::Running => format!(
                "running for {} (pid {})",
                human_secs(arbos_core::now_ms() - self.meta.started_ms),
                self.meta.pid
            ),
        }
    }
}

fn human_secs(ms: i64) -> String {
    let s = (ms.max(0) / 1000) as u64;
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m{}s", s / 60, s % 60)
    } else {
        format!("{}h{}m", s / 3600, (s % 3600) / 60)
    }
}

/// One agent's `jobs/` directory.
#[derive(Debug, Clone)]
pub struct JobsRoot(PathBuf);

impl JobsRoot {
    pub fn new(dir: PathBuf) -> Self {
        Self(dir)
    }

    pub fn for_agent(place: &Place, agent: &AgentId) -> Self {
        Self(arbos_core::Layout::new(place, agent.as_str()).jobs())
    }

    pub fn dir(&self) -> &Path {
        &self.0
    }

    /// Start `command` as a job. Returns the job and the child handle; the
    /// caller must `wait()` the child (or hand it to a reaper) so it is not
    /// left a zombie. The exit code is persisted by the wrapper shell, not
    /// by whoever waits.
    pub fn spawn(
        &self,
        command: &str,
        cwd: &Path,
        timeout_ms: Option<u64>,
        sandbox: Option<&crate::sandbox::Sandbox>,
        granted: Vec<(String, String)>,
    ) -> Result<(Job, Child)> {
        fs::create_dir_all(&self.0).with_context(|| format!("jobs dir {}", self.0.display()))?;
        self.prune();
        let (id, dir) = self.alloc()?;
        let journal = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("out.log"))?;
        let err_fd = journal.try_clone()?;
        let exit_path = dir.join("exit");
        // `pipefail` where the shell has it (bash, zsh, macOS sh; dash does
        // not): `curl | jq` then fails when curl does. The probe is silent
        // so a shell without it leaves nothing in the journal.
        // After the login shell has read the user's profile, secrets that
        // came back with it go out again unless the secrets door granted
        // them (see `envsafe::scrub_prologue`).
        // The command runs as a background child of this wrapper, which
        // polls its own parent (the leash, or the sandbox that dies with
        // it) the way the leash polls the kernel: parent gone, the whole
        // group goes. Without this, a leash killed on its own left the
        // wrapper and the command running with nothing watching the cap,
        // and the protection depended on which process someone happened
        // to kill. `ARBOS_LEASH` is the group (the leash's pid).
        let script = format!(
            "(set -o pipefail) 2>/dev/null && set -o pipefail; {scrub} ( cd {} && {command}\n) & J=$!; trap 'kill -TERM \"$J\" 2>/dev/null' INT TERM; while kill -0 \"$J\" 2>/dev/null; do if ! kill -0 \"$PPID\" 2>/dev/null; then kill -9 \"$J\" 2>/dev/null; kill -9 -\"${{ARBOS_LEASH:-$J}}\" 2>/dev/null; exit 137; fi; sleep 0.1; done; wait \"$J\"; echo $? > {}",
            sh_quote(&cwd.display().to_string()),
            sh_quote(&exit_path.display().to_string()),
            scrub = arbos_core::envsafe::scrub_prologue(),
        );
        // Inside a sandbox the wrapper shell runs there too, so the exit
        // file is written from within (the jobs dir is under the place).
        let (program, args) = match sandbox {
            Some(sb) => match sb.wrap(&script, cwd) {
                Ok(pair) => pair,
                Err(e) => {
                    let _ = fs::remove_dir_all(&dir);
                    return Err(e);
                }
            },
            None => shell_command(&script),
        };
        let (program, args) = leashed(&dir, program, args);
        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::from(journal))
            .stderr(Stdio::from(err_fd));
        // An allowlisted environment, not the kernel's whole one, plus the
        // secrets granted to this agent or one above it (`granted`, from
        // `secrets::Store::env_for`; their values are redacted from
        // everything that comes back). `ARBOS_GRANTED` tells the
        // shell-side scrub which secret-looking names to keep.
        cmd.env_clear();
        cmd.envs(arbos_core::envsafe::filtered(&[]));
        cmd.envs(git_identity_env(cwd));
        cmd.env(
            "ARBOS_GRANTED",
            granted
                .iter()
                .map(|(k, _)| k.as_str())
                .collect::<Vec<_>>()
                .join(" "),
        );
        cmd.envs(granted);
        #[cfg(unix)]
        cmd.process_group(0);
        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = fs::remove_dir_all(&dir);
                return Err(anyhow::anyhow!("bash: {e}"));
            }
        };
        let meta = Meta {
            command: command.to_string(),
            cwd: cwd.to_path_buf(),
            pid: child.id().unwrap_or(0),
            started_ms: arbos_core::now_ms(),
            timeout_ms,
        };
        fs::write(dir.join("meta.json"), serde_json::to_vec(&meta)?)?;
        let job = Job {
            id,
            dir,
            meta,
            status: Status::Running,
            ended_ms: None,
            journal_bytes: 0,
            killed_why: None,
        };
        Ok((job, child))
    }

    pub fn load(&self, id: &str) -> Result<Job> {
        load_dir(&self.0.join(id)).with_context(|| format!("no such job {id:?} (use jobs to list)"))
    }

    /// Every job, oldest first.
    pub fn list(&self) -> Vec<Job> {
        let Ok(entries) = fs::read_dir(&self.0) else {
            return Vec::new();
        };
        let mut out: Vec<Job> = entries
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| load_dir(&e.path()).ok())
            .collect();
        out.sort_by_key(|j| job_num(&j.id));
        out
    }

    /// Journal bytes beyond what the model has already been shown. Advances
    /// the cursor. Anything more than `JOURNAL_WINDOW` behind the end is
    /// skipped and reported in bytes.
    pub fn read_new(&self, job: &Job) -> (String, u64) {
        let seen_path = job.dir.join("seen");
        let offset: u64 = fs::read_to_string(&seen_path)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let Ok(mut f) = File::open(job.journal()) else {
            return (String::new(), 0);
        };
        let size = f.metadata().map(|m| m.len()).unwrap_or(0);
        // A log cut back by the cap is shorter than what was already
        // shown: start over from its new beginning.
        let offset = if offset > size { 0 } else { offset };
        if size <= offset {
            return (String::new(), 0);
        }
        let mut start = offset;
        let mut skipped = 0;
        if size - start > JOURNAL_WINDOW {
            skipped = size - start - JOURNAL_WINDOW;
            start = size - JOURNAL_WINDOW;
        }
        // Only up to the size just measured: a job writing faster than
        // this reads (`yes` does 500 MB/s) would otherwise be read to
        // whatever EOF it reaches, and that took a kernel down at 14 GB.
        let mut buf = Vec::with_capacity((size - start) as usize);
        if f.seek(SeekFrom::Start(start)).is_err()
            || f.by_ref().take(size - start).read_to_end(&mut buf).is_err()
        {
            return (String::new(), 0);
        }
        let _ = fs::write(&seen_path, format!("{}\n", start + buf.len() as u64));
        (String::from_utf8_lossy(&buf).into_owned(), skipped)
    }

    /// The tool call returned while the job was still running. Arms the
    /// completion notice.
    pub fn mark_detached(&self, job: &Job) {
        let _ = fs::write(job.dir.join("detached"), b"");
    }

    /// Detached jobs that have finished and have not yet been announced.
    /// Marks them announced; the caller delivers the notice.
    pub fn sweep(&self) -> Vec<Job> {
        self.list()
            .into_iter()
            .filter(|j| !j.running() && j.detached() && !j.dir.join("notified").exists())
            .inspect(|j| {
                let _ = fs::write(j.dir.join("notified"), b"");
            })
            .collect()
    }

    /// SIGKILL the job's process group. A finished job is a no-op.
    pub fn kill(&self, job: &Job) -> bool {
        if !job.running() {
            return false;
        }
        // Said before the signal, so a reader that comes between never
        // sees "no exit recorded" (qa-024).
        let _ = fs::write(job.dir.join("killed"), "killed by the kernel\n");
        crate::tools::kill_job(job.meta.pid);
        true
    }

    /// Jobs of this agent still alive from an earlier kernel run, ended.
    /// The leash ends a job when its kernel dies, but a job from before
    /// the leash, or one whose leash was killed first, runs on with parent
    /// pid 1 — the Mac wake-up incident: a feed script from three days
    /// earlier appended to `.arbos/user.md` every 30 s across every
    /// restart. A folder holding a `keep` file is left alone (a job the
    /// user asked to survive). The pid is checked against the job's own
    /// command before the signal, so a reused pid is never killed.
    pub fn reap_leftovers(&self) -> Reaped {
        let mut out = Reaped::default();
        let Ok(entries) = fs::read_dir(&self.0) else {
            return out;
        };
        for e in entries.flatten() {
            let dir = e.path();
            let Ok(job) = load_dir(&dir) else {
                continue;
            };
            if dir.join("keep").exists() {
                continue;
            }
            if !job.running() {
                // The leash died without a word (no `exit`, no `killed`)
                // but its group did not: children of the command outlived
                // every shell of ours — the leash and the wrapper killed
                // together by a cleanup that matched them and not `yes`.
                // A group whose leader is gone takes no new members, so
                // what is in it descends from our leash; on Linux the
                // start times say so too. The folder read "killed" while
                // the work ran on: the mirror of a turn that looks alive
                // after it ended.
                if job.status == Status::Killed && job.killed_why.is_none() {
                    let orphans = group_survivors(job.meta.pid, job.meta.started_ms);
                    if !orphans.is_empty() {
                        let _ = fs::write(
                            dir.join("killed"),
                            format!(
                                "killed: the job's shells were gone but {} process(es) of its group still ran with nothing watching them (reaped at start)\n",
                                orphans.len()
                            ),
                        );
                        crate::tools::kill_job(job.meta.pid);
                        out.reaped.push(format!(
                            "{} (pid {}): {} [{} orphan(s) of its group]",
                            job.id,
                            job.meta.pid,
                            arbos_core::text::clip(job.meta.command.trim(), 80),
                            orphans.len()
                        ));
                    }
                }
                continue;
            }
            let line = format!(
                "{} (pid {}, started {}): {}",
                job.id,
                job.meta.pid,
                arbos_core::inbox::rfc3339(job.meta.started_ms),
                arbos_core::text::clip(job.meta.command.trim(), 80)
            );
            // A job whose leash is this very process's child was started
            // by the image that ran before an `execv` (same pid): not a
            // leftover, ours, still leashed to us. Driving the claim that
            // jobs survive a self-update found the reap took them (the
            // pid-1 case in the comment above was the intent, not the
            // check).
            if parent_pid(job.meta.pid) == Some(std::process::id()) {
                out.inherited.push(line);
                continue;
            }
            match pid_identity(job.meta.pid, &dir, &job.meta) {
                PidIdentity::Ours => {
                    let _ = fs::write(
                        dir.join("killed"),
                        "killed: left over from an earlier kernel run (reaped at start)\n",
                    );
                    crate::tools::kill_job(job.meta.pid);
                    out.reaped.push(line);
                }
                PidIdentity::Foreign => {
                    // The pid is someone else's now; the job itself is gone.
                    let _ = fs::write(
                        dir.join("killed"),
                        "killed: the process was gone when the kernel started (its pid now belongs to another program)\n",
                    );
                    out.foreign.push(line);
                }
                PidIdentity::Unverified => out.unverified.push(line),
                PidIdentity::Gone => {}
            }
        }
        out
    }

    /// Jobs of this agent still alive from an earlier kernel run, listed
    /// (for `check`): id, pid, command, and whether the pid was verified
    /// as the job's own.
    pub fn leftovers(&self) -> Vec<(String, u32, String, PidIdentity)> {
        let Ok(entries) = fs::read_dir(&self.0) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|e| load_dir(&e.path()).ok().map(|j| (e.path(), j)))
            .filter(|(_, j)| j.running())
            .map(|(dir, j)| {
                let who = pid_identity(j.meta.pid, &dir, &j.meta);
                (j.id.clone(), j.meta.pid, j.meta.command.clone(), who)
            })
            .filter(|(_, _, _, who)| matches!(who, PidIdentity::Ours | PidIdentity::Unverified))
            .collect()
    }

    /// Remove finished folders older than `JOB_TTL`. Running jobs are never touched.
    pub fn prune(&self) {
        let cutoff = arbos_core::now_ms() - JOB_TTL.as_millis() as i64;
        let Ok(entries) = fs::read_dir(&self.0) else {
            return;
        };
        for e in entries.flatten() {
            let dir = e.path();
            match load_dir(&dir) {
                Ok(j) => {
                    if !j.running() && j.meta.started_ms < cutoff {
                        let _ = fs::remove_dir_all(&dir);
                    }
                }
                Err(_) => {
                    // Crash between mkdir and meta write. Age by mtime.
                    let old = e
                        .metadata()
                        .and_then(|m| m.modified())
                        .map(|t| t.elapsed().map(|d| d > JOB_TTL).unwrap_or(false))
                        .unwrap_or(false);
                    if old {
                        let _ = fs::remove_dir_all(&dir);
                    }
                }
            }
        }
    }

    /// Claim the next `jN` with an exclusive mkdir.
    fn alloc(&self) -> Result<(String, PathBuf)> {
        let mut next = 1;
        if let Ok(entries) = fs::read_dir(&self.0) {
            for e in entries.flatten() {
                let n = job_num(&e.file_name().to_string_lossy());
                if n >= next {
                    next = n + 1;
                }
            }
        }
        for _ in 0..100 {
            let id = format!("j{next}");
            let dir = self.0.join(&id);
            match fs::create_dir(&dir) {
                Ok(()) => return Ok((id, dir)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => next += 1,
                Err(e) => return Err(e.into()),
            }
        }
        bail!("could not allocate a job id")
    }
}

fn load_dir(dir: &Path) -> Result<Job> {
    let meta: Meta = serde_json::from_slice(&fs::read(dir.join("meta.json"))?)?;
    let id = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let journal_bytes = fs::metadata(dir.join("out.log"))
        .map(|m| m.len())
        .unwrap_or(0);
    let exit_path = dir.join("exit");
    if let Ok(text) = fs::read_to_string(&exit_path) {
        if let Ok(code) = text.trim().parse::<i32>() {
            let ended_ms = fs::metadata(&exit_path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64);
            return Ok(Job {
                id,
                dir: dir.to_path_buf(),
                meta,
                status: Status::Exited(code),
                ended_ms,
                journal_bytes,
                killed_why: None,
            });
        }
    }
    let killed_path = dir.join("killed");
    let killed_why = fs::read_to_string(&killed_path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let status = if killed_why.is_none() && pid_alive(meta.pid) {
        Status::Running
    } else {
        Status::Killed
    };
    let ended_ms = (status == Status::Killed)
        .then(|| mtime_ms(&killed_path))
        .flatten();
    Ok(Job {
        id,
        dir: dir.to_path_buf(),
        meta,
        status,
        ended_ms,
        journal_bytes,
        killed_why,
    })
}

fn mtime_ms(path: &Path) -> Option<i64> {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
}

/// The job wrapped in a shell that ends it when the kernel is gone and
/// keeps its log under the cap. A job outliving its kernel had nobody to
/// read it and no one to stop it (qa-025: `yes` at 100% CPU for an hour,
/// 8.7 GB of out.log). `$PPID` is the kernel; the leash is the process
/// group the kernel's kill signals. When the kernel dies the leash writes
/// `killed` and kills the group, itself included (`kill -9 -PGID`: dash
/// rejects the `--` form). `$1` is the job folder.
/// The log is cut back when it passes the cap; a job that refills it past
/// the cap on the very next look is writing faster than anyone reads and
/// is ended as runaway (a poll cannot hard-cap a writer doing 500 MB/s).
/// `$2` is the place's `.arbos` store: gone (the project deleted, a
/// scratch folder removed), the job has no folder to be checked, read,
/// capped or killed from, and its `out.log` is an unlinked inode growing
/// on the disk unseen — 164 GB on QA's machine, with the cap blind
/// because `wc -c` on a deleted path reads as 0. No store, no job; the
/// kernel's own liveness is not asked, since the kernel may be the thing
/// that leaked.
///
/// The rule the rest of the script holds: no job runs with an
/// uncheckable cap.
/// - The job folder gone but the store there (an archived child: the
///   kernel moved the folder): the leash re-points itself from
///   `<store>/runtime/leash/<pid>`, which the kernel writes before the
///   move. Not found within 20 looks (5 s), the group ends
///   and a `job_folder_lost` line goes on kernel.log. The job is not
///   ended by the move itself: a server a worker started on purpose
///   survives being archived.
/// - A signal at the leash's own pid — the pid `jobs` shows, so the pid
///   an agent or a user kills — ends the whole group, not the leash
///   alone. QA's 164 GB: the model ran `kill <pid>` on what `jobs`
///   displayed, the leash forwarded TERM to the wrapper shell only, the
///   loop under it lived on with PPID 1 and nothing polling anything,
///   and the folder later went, taking the cap with it. The pid we show
///   must be safe to kill. (`kill -9` on it cannot be trapped; the
///   wrapper's parent poll is the backstop for that.)
/// - The wrapper gone but children of it still in the group (a command
///   that backgrounded something, or a wrapper killed on its own): the
///   leash stays with the survivors — same cap, same kernel and store
///   checks — until the group is empty, and writes the wrapper's exit
///   if the wrapper could not (its exit path was the old folder).
fn leashed(dir: &Path, program: String, args: Vec<String>) -> (String, Vec<String>) {
    const LEASH: &str = r#"D=$1; P=$2; shift 2; K=$PPID; C=${ARBOS_JOB_LOG_CAP:-67108864}; R=0; L=0; X=; T=0.25
export ARBOS_LEASH=$$
"$@" & F=$!
ended() { trap '' INT TERM; echo "killed: a signal to the job's pid $$ ended it (the whole job, not only its shell)" > "$D/killed" 2>/dev/null; rm -f "$P/runtime/leash/$$" 2>/dev/null; kill -TERM -$$ 2>/dev/null; sleep 1; kill -9 -$$ 2>/dev/null; exit 143; }
trap ended INT TERM
die() { kill -9 "$F" 2>/dev/null; rm -f "$P/runtime/leash/$$" 2>/dev/null; kill -9 -$$ 2>/dev/null; exit 137; }
note() { printf '{"ts":%s000,"level":"warn","event":"%s","detail":"%s"}\n' "$(date +%s)" "$1" "$2" >> "$P/runtime/kernel.log" 2>/dev/null; }
while :; do
  if ! kill -0 "$F" 2>/dev/null; then
    if [ -z "$X" ]; then
      wait "$F"; X=$?
      [ -e "$D/exit" ] || echo "$X" > "$D/exit" 2>/dev/null
      T=1
    fi
    A=0; for Q in $(pgrep -g $$ 2>/dev/null); do [ "$Q" != "$$" ] && kill -0 "$Q" 2>/dev/null && A=1; done
    if [ "$A" -eq 0 ]; then
      rm -f "$P/runtime/leash/$$" 2>/dev/null
      exit "$X"
    fi
  fi
  [ -d "$P" ] || die
  if [ ! -d "$D" ]; then
    N=$(cat "$P/runtime/leash/$$" 2>/dev/null)
    if [ -n "$N" ] && [ -d "$N" ]; then
      D=$N; L=0
    else
      L=$((L+1))
      if [ "$L" -ge 20 ]; then
        note job_folder_lost "job folder $D gone for 5s with no new path from the kernel; the job's cap could not be checked and its group was ended (leash $$)"
        die
      fi
    fi
  fi
  if ! kill -0 "$K" 2>/dev/null; then
    echo "killed: the kernel exited and the job was ended with it" > "$D/killed" 2>/dev/null
    die
  fi
  S=$(wc -c < "$D/out.log" 2>/dev/null || echo 0)
  if [ "${S:-0}" -gt "$C" ]; then
    if [ "$R" = 1 ]; then
      echo "killed: runaway output (over $C bytes twice in a row after the log was cut back)" > "$D/killed"
      die
    fi
    R=1
    : > "$D/out.log"
    echo "[arbos: out.log passed $C bytes; older output dropped]" >> "$D/out.log"
  else
    R=0
  fi
  sleep $T
done"#;
    let mut all = vec![
        "-c".to_string(),
        LEASH.to_string(),
        "job-leash".to_string(),
        dir.display().to_string(),
        store_of(dir).display().to_string(),
        program,
    ];
    all.extend(args);
    ("sh".to_string(), all)
}

/// Live members of process group `pgid` other than the leader, started no
/// earlier than the job where the machine can say (Linux). `pgrep -g` is
/// on Linux and macOS alike.
fn group_survivors(pgid: u32, job_started_ms: i64) -> Vec<u32> {
    let Ok(out) = std::process::Command::new("pgrep")
        .args(["-g", &pgid.to_string()])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .filter(|&pid| pid != pgid && pid_alive(pid))
        .filter(|&pid| {
            process_start_ms(pid).is_none_or(|started| started >= job_started_ms - 120_000)
        })
        .collect()
}

/// Where a leash reads its job folder's new path from when the old one is
/// gone: `<store>/runtime/leash/<leash pid>`, one line, the folder. The
/// kernel writes it before it moves an agent's folder (archive), so the
/// cap never goes blind; the leash removes it when it ends.
pub const LEASH_POINTERS: &str = "runtime/leash";

/// Tell the leash of `pid` that its job folder is now `new_dir`. Written
/// before the move: a look between the two finds the old path still
/// there, or the new one already named.
pub fn repoint_leash(store: &Path, pid: u32, new_dir: &Path) -> std::io::Result<()> {
    let dir = store.join(LEASH_POINTERS);
    fs::create_dir_all(&dir)?;
    fs::write(dir.join(pid.to_string()), new_dir.display().to_string())
}

/// Pointers whose leash is gone (a leash killed before it could remove
/// its own): removed, at kernel start.
pub fn sweep_leash_pointers(store: &Path) -> usize {
    let Ok(rd) = fs::read_dir(store.join(LEASH_POINTERS)) else {
        return 0;
    };
    let mut n = 0;
    for e in rd.flatten() {
        let dead = e
            .file_name()
            .to_string_lossy()
            .parse::<u32>()
            .map(|pid| !pid_alive(pid))
            .unwrap_or(true);
        if dead && fs::remove_file(e.path()).is_ok() {
            n += 1;
        }
    }
    n
}

/// The `.arbos` store a job folder lives under: the nearest ancestor so
/// named, or three levels up (`.arbos/agents/<id>/jobs/<job>`).
fn store_of(job_dir: &Path) -> PathBuf {
    job_dir
        .ancestors()
        .find(|p| p.file_name().is_some_and(|n| n == ".arbos"))
        .map(Path::to_path_buf)
        .or_else(|| job_dir.ancestors().nth(4).map(Path::to_path_buf))
        .unwrap_or_else(|| job_dir.to_path_buf())
}

fn job_num(id: &str) -> i64 {
    id.strip_prefix('j')
        .and_then(|n| n.parse().ok())
        .unwrap_or(-1)
}

/// Signal 0 probes without sending. EPERM still means "exists".
#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let r = unsafe { libc::kill(pid as i32, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn pid_alive(_pid: u32) -> bool {
    false
}

/// What `reap_leftovers` found, as lines for the log.
#[derive(Debug, Default, Clone)]
pub struct Reaped {
    /// Ended: the job's own process, still running.
    pub reaped: Vec<String>,
    /// Left running: leashed to this process itself — started by the
    /// image before an `execv`.
    pub inherited: Vec<String>,
    /// Left alone: the pid now belongs to another program.
    pub foreign: Vec<String>,
    /// Left alone: no way to tell whose the process is on this machine.
    pub unverified: Vec<String>,
}

/// What a job's recorded pid is today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PidIdentity {
    /// No process with that pid.
    Gone,
    /// The job's own process: the leash's argv names this job folder, or
    /// (Linux) the process started within two minutes of the job.
    Ours,
    /// A live process that is provably not the job (a reused pid).
    Foreign,
    /// A live process this machine gives no way to check (no /proc start
    /// time, no job-folder marker in its argv — a job from before the
    /// leash on macOS, say). Never killed; `check` and the log say so.
    Unverified,
}

/// Whether `pid` is (still) the process of this job and not a later
/// process that got the same number. Only two proofs count: the leash's
/// argv carries the job folder, and on Linux /proc gives a start time.
/// Nothing is ever matched by program name — a reused pid that is now
/// any `bash` must not be `killpg`'d at kernel start (steward's hold on
/// #130, and qa-020's history).
fn pid_identity(pid: u32, dir: &Path, meta: &Meta) -> PidIdentity {
    if !pid_alive(pid) {
        return PidIdentity::Gone;
    }
    let dir_s = dir.to_string_lossy();
    let args = process_args(pid);
    if args.as_deref().is_some_and(|a| a.contains(dir_s.as_ref())) {
        return PidIdentity::Ours;
    }
    if let Some(started) = process_start_ms(pid) {
        return if (started - meta.started_ms).abs() < 120_000 {
            PidIdentity::Ours
        } else {
            PidIdentity::Foreign
        };
    }
    PidIdentity::Unverified
}

/// Linux: when `pid` started, as Unix millis, from /proc/<pid>/stat field
/// 22 (clock ticks since boot) and /proc/stat's btime.
fn process_start_ms(pid: u32) -> Option<i64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name is in parentheses and may hold spaces: split after it.
    let after = stat.rsplit_once(')')?.1;
    let fields: Vec<&str> = after.split_whitespace().collect();
    // Field 22 overall; `after` starts at field 3.
    let ticks: i64 = fields.get(19)?.parse().ok()?;
    let btime: i64 = fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()?;
    // SAFETY: sysconf is a plain query.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if hz <= 0 {
        return None;
    }
    Some(btime * 1000 + ticks * 1000 / hz)
}

/// The command line of `pid`, from /proc or `ps`.
/// The parent pid of `pid`, when the machine can say.
pub fn parent_pid(pid: u32) -> Option<u32> {
    if let Ok(status) = fs::read_to_string(format!("/proc/{pid}/status")) {
        return status
            .lines()
            .find_map(|l| l.strip_prefix("PPid:"))
            .and_then(|v| v.trim().parse().ok());
    }
    let out = std::process::Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

fn process_args(pid: u32) -> Option<String> {
    if let Ok(raw) = fs::read(format!("/proc/{pid}/cmdline")) {
        return Some(String::from_utf8_lossy(&raw).replace('\0', " "));
    }
    let out = std::process::Command::new("ps")
        .args(["-o", "args=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// The wrapper shell: bash when the machine has it (it has `pipefail`;
/// Debian's `sh` is dash, which does not), else `sh`.
pub(crate) fn job_shell() -> &'static str {
    static SHELL: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    SHELL.get_or_init(|| {
        let found = std::env::var_os("PATH")
            .is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join("bash").is_file()));
        if found { "bash" } else { "sh" }
    })
}

/// The shell and arguments that run `script`. bash runs as a login shell
/// (`-l`): the machine's profile puts a conda env, a venv, or a toolchain
/// on PATH the way the user's own terminal has them (SWE-bench images keep
/// their interpreter in a conda env that only a login shell activates).
/// `ARBOS_NO_LOGIN_SHELL=1` turns that off for a profile that misbehaves.
pub(crate) fn shell_command(script: &str) -> (String, Vec<String>) {
    let shell = job_shell();
    (shell.to_string(), shell_args(shell, script))
}

pub(crate) fn shell_args(shell: &str, script: &str) -> Vec<String> {
    let login = shell == "bash" && std::env::var_os("ARBOS_NO_LOGIN_SHELL").is_none();
    let flag = if login { "-lc" } else { "-c" };
    vec![flag.to_string(), script.to_string()]
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("arbos-jobs-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// qa-024: a job the kernel killed says so, with the time it ran.
    #[tokio::test]
    async fn a_killed_job_says_who_and_after_how_long() {
        let root = JobsRoot::new(scratch("killed"));
        let (job, mut child) = root
            .spawn(
                "sleep 30",
                &root.dir().to_path_buf(),
                None,
                None,
                Vec::new(),
            )
            .unwrap();
        assert!(job.running());
        assert!(root.kill(&job));
        let _ = child.wait().await;
        let again = root.load(&job.id).unwrap();
        assert_eq!(again.status, Status::Killed);
        let line = again.status_line();
        assert!(line.starts_with("killed by the kernel after"), "{line}");
        assert!(!line.contains("no exit recorded"), "{line}");
    }

    /// qa-025: the log is cut back at the cap, and a reader whose cursor is
    /// past the new end starts over instead of seeing nothing for ever.
    #[test]
    fn a_cut_back_log_is_read_from_its_new_start() {
        let root = JobsRoot::new(scratch("cut"));
        fs::create_dir_all(root.dir().join("j1")).unwrap();
        let meta = Meta {
            command: "x".into(),
            cwd: root.dir().to_path_buf(),
            pid: 0,
            started_ms: 0,
            timeout_ms: None,
        };
        fs::write(
            root.dir().join("j1/meta.json"),
            serde_json::to_vec(&meta).unwrap(),
        )
        .unwrap();
        fs::write(root.dir().join("j1/out.log"), "a".repeat(100)).unwrap();
        fs::write(root.dir().join("j1/exit"), "0\n").unwrap();
        let job = root.load("j1").unwrap();
        let (first, _) = root.read_new(&job);
        assert_eq!(first.len(), 100);
        fs::write(root.dir().join("j1/out.log"), "[cut]\nbb").unwrap();
        let (after, _) = root.read_new(&job);
        assert_eq!(after, "[cut]\nbb");
    }
}

#[cfg(test)]
mod git_identity_tests {
    use super::{DEFAULT_GIT_AUTHOR, git_has_identity, git_identity_env};
    use std::process::Command;

    fn repo(tag: &str, with_identity: bool) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "arbos-git-id-{tag}-{}-{}",
            std::process::id(),
            arbos_core::now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(&dir)
                    // No global config: what a fresh machine has.
                    .env("HOME", &dir)
                    .env("GIT_CONFIG_NOSYSTEM", "1")
                    .env("GIT_CONFIG_GLOBAL", dir.join("no-such-gitconfig"))
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        };
        git(&["init", "-q", "-b", "main"]);
        if with_identity {
            git(&["config", "user.name", "Jacob"]);
            git(&["config", "user.email", "j@example.com"]);
        }
        dir
    }

    /// Mobile cycle 1, item 3: workers on a machine with no git identity
    /// asked the user for one. A job there gets the Arbos default in its
    /// environment; a repository with an identity gets nothing.
    #[test]
    fn a_job_gets_the_default_author_only_where_git_has_none() {
        // The test process itself must not carry an identity in env.
        for v in [
            "GIT_AUTHOR_NAME",
            "GIT_AUTHOR_EMAIL",
            "GIT_COMMITTER_NAME",
            "GIT_COMMITTER_EMAIL",
        ] {
            // SAFETY: test-local; other tests in this module do not read these.
            unsafe { std::env::remove_var(v) };
        }
        let with = repo("with", true);
        assert!(git_has_identity(&with));
        assert!(git_identity_env(&with).is_empty());

        let without = repo("without", false);
        // The kernel's own global config, if any, would count: make sure
        // the check sees none.
        // SAFETY: test-local.
        unsafe {
            std::env::set_var("GIT_CONFIG_GLOBAL", without.join("no-such-gitconfig"));
            std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        }
        assert!(!git_has_identity(&without));
        let env = git_identity_env(&without);
        let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("GIT_AUTHOR_NAME"), Some(DEFAULT_GIT_AUTHOR.0));
        assert_eq!(get("GIT_AUTHOR_EMAIL"), Some(DEFAULT_GIT_AUTHOR.1));
        assert_eq!(get("GIT_COMMITTER_NAME"), Some(DEFAULT_GIT_AUTHOR.0));
        assert_eq!(get("GIT_COMMITTER_EMAIL"), Some(DEFAULT_GIT_AUTHOR.1));
        // And a commit with that environment succeeds and says so.
        std::fs::write(without.join("a.txt"), "a\n").unwrap();
        let mut cmd = Command::new("sh");
        cmd.args([
            "-c",
            "git add a.txt && git commit -q -m one && git log -1 --format=%an,%ae",
        ])
        .current_dir(&without)
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            format!("{},{}", DEFAULT_GIT_AUTHOR.0, DEFAULT_GIT_AUTHOR.1)
        );
        unsafe {
            std::env::remove_var("GIT_CONFIG_GLOBAL");
            std::env::remove_var("GIT_CONFIG_NOSYSTEM");
        }
    }
}
