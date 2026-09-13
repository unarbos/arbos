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
        let script = format!(
            "(set -o pipefail) 2>/dev/null && set -o pipefail; ( cd {} && {command}\n); echo $? > {}",
            sh_quote(&cwd.display().to_string()),
            sh_quote(&exit_path.display().to_string())
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
        // Secrets the agent asked to use, by name; their values are
        // redacted from everything that comes back.
        cmd.envs(crate::secrets::store().env());
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
fn leashed(dir: &Path, program: String, args: Vec<String>) -> (String, Vec<String>) {
    const LEASH: &str = r#"D=$1; shift; K=$PPID; C=${ARBOS_JOB_LOG_CAP:-67108864}; R=0
"$@" & F=$!
trap 'kill -TERM "$F" 2>/dev/null' INT TERM
while kill -0 "$F" 2>/dev/null; do
  if ! kill -0 "$K" 2>/dev/null; then
    echo "killed: the kernel exited and the job was ended with it" > "$D/killed"
    kill -9 "$F" 2>/dev/null
    kill -9 -$$ 2>/dev/null
    exit 137
  fi
  S=$(wc -c < "$D/out.log" 2>/dev/null || echo 0)
  if [ "${S:-0}" -gt "$C" ]; then
    if [ "$R" = 1 ]; then
      echo "killed: runaway output (over $C bytes twice in a row after the log was cut back)" > "$D/killed"
      kill -9 "$F" 2>/dev/null
      kill -9 -$$ 2>/dev/null
      exit 137
    fi
    R=1
    : > "$D/out.log"
    echo "[arbos: out.log passed $C bytes; older output dropped]" >> "$D/out.log"
  else
    R=0
  fi
  sleep 0.25
done
wait "$F""#;
    let mut all = vec![
        "-c".to_string(),
        LEASH.to_string(),
        "job-leash".to_string(),
        dir.display().to_string(),
        program,
    ];
    all.extend(args);
    ("sh".to_string(), all)
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
            .spawn("sleep 30", &root.dir().to_path_buf(), None, None)
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
