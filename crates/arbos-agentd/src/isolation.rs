//! Workspace isolation on one box (D70 hardening): every workspace runs as its own Unix user
//! with `/work/<project>` at mode 700, inside its own cgroup. agentd itself runs as root (it
//! re-execs through `sudo` when the image started it as `daytona`); the model's children never do.
//!
//! What is enforced: file access (another workspace's folder is unreadable), memory (`memory.high`
//! / `memory.max`), pid count, fair CPU share (`cpu.weight`), and the CPU count the workspace sees
//! (`cpuset.cpus` = the box's quota, so `nproc` matches). What is not: the box's own quota is the
//! hard ceiling for everyone, and `/proc` still shows other workspaces' processes (no PID
//! namespace in a Sysbox container). Local `process` boxes (dev machines) run without isolation.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context, Result};

#[derive(Debug, Clone)]
pub struct RunAs {
    pub user: String,
    pub uid: u32,
    pub gid: u32,
    pub home: PathBuf,
    pub cgroup: Option<PathBuf>,
}

/// Where a harness child's descendants can be found again once the child itself is gone.
///
/// Killing a process does not kill its children on Linux, so a job's `bash` tool call outlives
/// the worker that started it — detached or not (Test5 round 10, bug Z: pid 561 wrote all 40
/// lines after the job read `cancelled`). Neither handle here is the child's pid: both name a
/// set the kernel keeps for us, so a tree can be reaped after its root has died and its
/// children have been reparented to PID 1.
///
/// Which one catches what, measured rather than assumed:
///
/// - **cgroup**: everything. Membership is inherited on fork and `setsid` does not change it.
/// - **process group**: a plain child and `nohup … &` (neither leaves the group — `nohup` only
///   ignores `SIGHUP`, and `&` starts no new group in a non-interactive shell), but not an
///   explicit `setsid`.
///
/// So the cgroup is the container and the group is the fallback for a box with no isolation
/// (a dev machine, or agentd not running as root). The fallback is knowingly incomplete, which
/// is why [`ProcessTree::kill`] re-reads and reports survivors instead of assuming a clean kill.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessTree {
    /// The child's own process group. `RpcChild` spawns with `process_group(0)`, so the child
    /// leads a group of its own and a kill of it cannot reach agentd.
    pub pgid: Option<u32>,
    /// The child's cgroup leaf under the workspace cgroup, when isolation made one.
    pub cgroup: Option<PathBuf>,
}

/// What a kill of a [`ProcessTree`] did, read back from the kernel afterwards rather than
/// inferred from the signals sent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sweep {
    /// Processes of the tree that were alive when the kill started.
    pub found: usize,
    /// How many of them are gone now.
    pub killed: usize,
    /// Pids still alive after the re-read. Empty is the only result that lets a caller say the
    /// job's work really stopped.
    pub survivors: Vec<u32>,
}

impl Sweep {
    pub fn clean(&self) -> bool {
        self.survivors.is_empty()
    }
}

/// How long the kill waits for the kernel to reap what it signalled before reading back. A
/// SIGKILL is not synchronous, so a re-read with no pause would report live pids that are
/// already dying — the opposite of the honest answer this is here to give.
const REAP_WAIT: std::time::Duration = std::time::Duration::from_millis(100);
const REAP_TRIES: usize = 20; // 2 s, inside the spec's five-second budget

impl ProcessTree {
    pub fn is_empty(&self) -> bool {
        self.pgid.is_none() && self.cgroup.is_none()
    }

    /// Every live process of this tree: the cgroup's members and the process group's, together.
    ///
    /// Both are read, not whichever looks authoritative: on a box with isolation the cgroup is
    /// the complete set, and the group adds nothing — but on one without it the cgroup is
    /// `None` and the group is all there is.
    pub fn members(&self) -> Vec<u32> {
        let mut pids: Vec<u32> = Vec::new();
        if let Some(dir) = &self.cgroup {
            pids.extend(cgroup_procs(dir));
        }
        if let Some(pgid) = self.pgid {
            pids.extend(procs_in_group(pgid));
        }
        let me = std::process::id();
        pids.retain(|p| *p > 1 && *p != me && is_running(*p));
        pids.sort_unstable();
        pids.dedup();
        pids
    }

    /// SIGKILL everything in the tree, then read the kernel back and report what is left.
    ///
    /// The read-back is the point. `stop_job` told the user a job was cancelled while its loop
    /// kept writing; a cancel that cannot prove the work stopped must say so rather than repeat
    /// that claim.
    pub fn kill(&self) -> Sweep {
        let found = self.members();
        if found.is_empty() {
            return Sweep::default();
        }
        // `cgroup.kill` (cgroup v2, Linux 5.14+) kills the whole subtree in one atomic write, so
        // a process forked while we are enumerating cannot slip out between the read and the
        // signal. Where it is missing, the enumerated kill below is the fallback.
        if let Some(dir) = &self.cgroup {
            let _ = std::fs::write(dir.join("cgroup.kill"), "1");
        }
        for pid in &found {
            signal(*pid);
        }
        if let Some(pgid) = self.pgid {
            // The group as a whole, which covers a child forked since the enumeration.
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(pgid as i32),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        let mut survivors = Vec::new();
        for attempt in 0..REAP_TRIES {
            std::thread::sleep(REAP_WAIT);
            survivors = self.members();
            if survivors.is_empty() {
                break;
            }
            // Something forked after the first pass, or is still dying. Signal again.
            if attempt < REAP_TRIES - 1 {
                for pid in &survivors {
                    signal(*pid);
                }
            }
        }
        Sweep {
            found: found.len(),
            killed: found.len().saturating_sub(survivors.len()),
            survivors,
        }
    }
}

fn signal(pid: u32) {
    if pid <= 1 || pid == std::process::id() {
        return;
    }
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid as i32),
        nix::sys::signal::Signal::SIGKILL,
    );
}

/// Pids in `dir` and every cgroup below it. Recursive because a child that makes its own
/// cgroups (a container runtime, a `systemd-run` that worked) would otherwise be invisible.
pub fn cgroup_procs(dir: &Path) -> Vec<u32> {
    let mut out: Vec<u32> = std::fs::read_to_string(dir.join("cgroup.procs"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            if e.path().is_dir() {
                out.extend(cgroup_procs(&e.path()));
            }
        }
    }
    out
}

/// Is this pid a process that is still doing something?
///
/// A zombie is not. It has been killed and is waiting to be reaped by a parent that may itself
/// be dead, and it keeps its `/proc` entry and its process group while it waits — so counting
/// one as a survivor would make a cancel that worked perfectly report that it had failed.
fn is_running(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false; // gone between the read and here, which is the outcome we wanted
    };
    let Some(after) = stat.rsplit_once(')').map(|(_, rest)| rest) else {
        return false;
    };
    // `Z` zombie, `X` dead.
    !matches!(
        after.split_whitespace().next(),
        Some("Z") | Some("X") | None
    )
}

/// Pids whose process group is `pgid`, from `/proc`.
///
/// `/proc/<pid>/stat` is `pid (comm) state ppid pgrp …` and `comm` may hold spaces and
/// parentheses, so the fields are counted from the last `)` rather than split on whitespace.
fn procs_in_group(pgid: u32) -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(e.path().join("stat")) else {
            continue;
        };
        let Some(after) = stat.rsplit_once(')').map(|(_, rest)| rest) else {
            continue;
        };
        // After the comm: state, ppid, pgrp.
        if after.split_whitespace().nth(2).and_then(|f| f.parse().ok()) == Some(pgid) {
            out.push(pid);
        }
    }
    out
}

#[derive(Debug, Clone)]
pub struct Isolation {
    pub enabled: bool,
    cgroup_root: Option<PathBuf>,
    /// `cpuset.cpus` for workspaces: the first `quota` CPUs of the box's effective set.
    cpuset: Option<String>,
    /// Memory limits per workspace (bytes), from the box's `memory.max`.
    mem_high: Option<u64>,
    mem_max: Option<u64>,
}

const SHARED_GROUP: &str = "arbos";
const CGROUP_PARENT: &str = "arbos-ws";

/// Create `dir` for a harness child and hand it to the workspace user. agentd runs as root and
/// the children do not: a directory root made and never re-owned is unwritable to them. The
/// pi worker's `subagent-sessions` was one (2026-10-25, Jacob's San José ask): every job worker
/// died on its first session write, 400 ms in, with an empty result and no error.
pub fn ensure_worker_dir(dir: &Path, run_as: Option<&RunAs>) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    if let Some(ra) = run_as {
        if let Err(e) = std::os::unix::fs::chown(dir, Some(ra.uid), Some(ra.gid)) {
            tracing::warn!(dir = %dir.display(), uid = ra.uid, error = %e, "worker dir not re-owned");
        }
    }
    Ok(())
}

impl Isolation {
    /// Enabled only when agentd is root and users can be created; `ARBOS_ISOLATION=off` disables.
    pub fn detect(local_box: bool) -> Self {
        let off = Self {
            enabled: false,
            cgroup_root: None,
            cpuset: None,
            mem_high: None,
            mem_max: None,
        };
        if local_box || std::env::var("ARBOS_ISOLATION").is_ok_and(|v| v == "off") {
            return off;
        }
        if nix::unistd::geteuid().as_raw() != 0 {
            tracing::warn!("isolation: agentd is not root; workspaces run as one user");
            return off;
        }
        if !Path::new("/usr/sbin/useradd").exists() && !Path::new("/sbin/useradd").exists() {
            tracing::warn!("isolation: useradd missing; workspaces run as one user");
            return off;
        }
        let _ = Command::new("groupadd").args(["-f", SHARED_GROUP]).status();
        // cgroup v2 with a writable root (the Daytona container gives one).
        let root = Path::new("/sys/fs/cgroup");
        let controllers =
            std::fs::read_to_string(root.join("cgroup.controllers")).unwrap_or_default();
        let parent = root.join(CGROUP_PARENT);
        let cgroup_root = if controllers.contains("memory")
            && std::fs::create_dir_all(&parent).is_ok()
            && std::fs::write(
                parent.join("cgroup.subtree_control"),
                "+cpu +memory +pids +cpuset",
            )
            .is_ok()
        {
            Some(parent)
        } else {
            tracing::warn!("isolation: cgroup v2 not writable; no per-workspace limits");
            None
        };
        let (cpuset, mem_high, mem_max) = match &cgroup_root {
            Some(_) => {
                let quota = std::fs::read_to_string(root.join("cpu.max"))
                    .ok()
                    .and_then(|s| {
                        let mut it = s.split_whitespace();
                        let q: f64 = it.next()?.parse().ok()?;
                        let p: f64 = it.next()?.parse().ok()?;
                        Some((q / p).ceil().max(1.0) as usize)
                    })
                    .unwrap_or(4);
                let effective =
                    std::fs::read_to_string(root.join("cpuset.cpus.effective")).unwrap_or_default();
                let cpus = first_cpus(&effective, quota);
                let box_mem: Option<u64> = std::fs::read_to_string(root.join("memory.max"))
                    .ok()
                    .and_then(|s| s.trim().parse().ok());
                // Half the box for one workspace, enforced by the kernel's OOM killer (it takes
                // the largest task, i.e. the runaway script). No `memory.high`: the throttle
                // stalls every task of the workspace — pi and the harness children included —
                // once one script passes it, and the agent stops answering (Doug, 2026-10-22:
                // 82–89 % "full" memory pressure, 9 s tool round trips, 37 s to first token).
                let mem_max = box_mem.map(|m| m / 2);
                (cpus, None, mem_max)
            }
            None => (None, None, None),
        };
        tracing::info!(
            cgroups = cgroup_root.is_some(),
            ?cpuset,
            ?mem_max,
            "isolation enabled"
        );
        Self {
            enabled: true,
            cgroup_root,
            cpuset,
            mem_high,
            mem_max,
        }
    }

    /// Deterministic user name for a project: `ws-` + lowercase id prefix.
    pub fn user_for(project_id: &str) -> String {
        let p: String = project_id
            .to_ascii_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(16)
            .collect();
        format!("ws-{p}")
    }

    /// Create the user (once), own the workspace folder (mode 700), create the cgroup.
    pub fn prepare(&self, project_id: &str, work_dir: &Path, state_dir: &Path) -> Result<RunAs> {
        let user = Self::user_for(project_id);
        let (uid, gid) = match lookup(&user) {
            Some(ids) => ids,
            None => {
                let st = Command::new("useradd")
                    .args([
                        "--no-create-home",
                        "--shell",
                        "/bin/bash",
                        "--groups",
                        SHARED_GROUP,
                        &user,
                    ])
                    .status()
                    .context("useradd")?;
                if !st.success() {
                    return Err(anyhow!("useradd {user} failed: {st}"));
                }
                lookup(&user).ok_or_else(|| anyhow!("user {user} not found after useradd"))?
            }
        };
        let home = state_dir.join("home");
        std::fs::create_dir_all(home.join(".cache"))?;
        // Ownership: the whole folder once (marker), then only what agentd creates later. The
        // marker alone is not proof: a workspace that moved machines (D89 host sandboxes,
        // move_box) or was written through a differently-mapped mount carries the same marker
        // with foreign uids, and pi then fails with EACCES on its own session files. So the
        // marker counts only while the state folder is really owned by this user's ids.
        let marker = state_dir.join(".owner");
        let marker_ok = std::fs::read_to_string(&marker)
            .map(|s| s.trim() == user)
            .unwrap_or(false);
        let ids_ok = {
            use std::os::unix::fs::MetadataExt;
            let sessions = state_dir.join("pi").join("sessions");
            [state_dir, &sessions]
                .iter()
                .filter_map(|p| std::fs::metadata(p).ok())
                .all(|m| m.uid() == uid && m.gid() == gid)
        };
        let owned = marker_ok && ids_ok;
        if marker_ok && !ids_ok {
            tracing::warn!(
                project_id,
                user,
                uid,
                gid,
                "workspace files carry foreign ids; re-owning them"
            );
        }
        if !owned {
            let st = Command::new("chown")
                .args([
                    "-R",
                    &format!("{uid}:{gid}"),
                    &work_dir.display().to_string(),
                ])
                .status()
                .context("chown -R")?;
            if !st.success() {
                return Err(anyhow!("chown {} failed", work_dir.display()));
            }
            std::fs::write(&marker, format!("{user}\n"))?;
        } else {
            let _ = Command::new("chown")
                .args([
                    &format!("{uid}:{gid}"),
                    &work_dir.display().to_string(),
                    &state_dir.display().to_string(),
                    &home.display().to_string(),
                ])
                .status();
        }
        let _ = Command::new("chmod")
            .args(["700", &work_dir.display().to_string()])
            .status();
        // The work root is traversable but not listable: a workspace reaches its own folder and
        // cannot enumerate the other projects' ids.
        if let Some(root) = work_dir.parent() {
            let _ = Command::new("chmod")
                .args(["711", &root.display().to_string()])
                .status();
        }
        let cgroup = match &self.cgroup_root {
            Some(parent) => {
                let dir = parent.join(&user);
                std::fs::create_dir_all(&dir)?;
                let _ = std::fs::write(dir.join("cpu.weight"), "100");
                if let Some(c) = &self.cpuset {
                    let _ = std::fs::write(dir.join("cpuset.cpus"), c);
                }
                if let Some(h) = self.mem_high {
                    let _ = std::fs::write(dir.join("memory.high"), h.to_string());
                }
                if let Some(m) = self.mem_max {
                    let _ = std::fs::write(dir.join("memory.max"), m.to_string());
                }
                let _ = std::fs::write(dir.join("pids.max"), "1024");
                Some(dir)
            }
            None => None,
        };
        Ok(RunAs {
            user,
            uid,
            gid,
            home,
            cgroup,
        })
    }

    /// Environment the workspace's processes get: their own HOME and caches; toolchains stay the
    /// shared read-only ones under /usr/local, /opt. Tools the agent installs itself (`pip
    /// install`, `uv tool install`, `npm i -g`) land in its own `~/.local`, which leads PATH.
    pub fn env(ra: &RunAs) -> Vec<(String, String)> {
        let home = ra.home.display().to_string();
        let path = std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into());
        vec![
            ("HOME".into(), home.clone()),
            ("PATH".into(), format!("{home}/.local/bin:{path}")),
            ("XDG_CACHE_HOME".into(), format!("{home}/.cache")),
            ("UV_CACHE_DIR".into(), format!("{home}/.cache/uv")),
            ("npm_config_cache".into(), format!("{home}/.cache/npm")),
            ("npm_config_prefix".into(), format!("{home}/.local")),
            ("PIP_CACHE_DIR".into(), format!("{home}/.cache/pip")),
            ("USER".into(), ra.user.clone()),
            ("LOGNAME".into(), ra.user.clone()),
        ]
    }

    /// Put a freshly spawned child (and everything it forks) into the workspace cgroup.
    pub fn attach(ra: &RunAs, pid: u32) {
        if let Some(cg) = &ra.cgroup {
            if let Err(e) = std::fs::write(cg.join("cgroup.procs"), pid.to_string()) {
                tracing::debug!(error = %e, pid, "cgroup attach");
            }
        }
    }

    /// Put a freshly spawned harness child into a cgroup of its own under the workspace's, so
    /// the processes of one worker can be told from another's and reaped on their own
    /// (`ProcessTree`). Returns the leaf, or `None` when there is no cgroup to make one under.
    ///
    /// The leaf enables no controllers, and that is deliberate. cgroup v2 forbids a cgroup from
    /// holding processes while its `subtree_control` is non-empty, so enabling anything here
    /// would mean no process could stay in the workspace cgroup itself and every attach site
    /// would have to move at once. The limits do not need it: `memory.max`, `pids.max` and
    /// `cpu.weight` sit on the workspace cgroup and charge hierarchically, so a process in a
    /// leaf is capped exactly as it was before.
    pub fn attach_leaf(ra: &RunAs, name: &str, pid: u32) -> Option<PathBuf> {
        let parent = ra.cgroup.as_ref()?;
        let dir = parent.join(leaf_name(name));
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::debug!(error = %e, pid, dir = %dir.display(), "cgroup leaf not created");
            Self::attach(ra, pid);
            return None;
        }
        if let Err(e) = std::fs::write(dir.join("cgroup.procs"), pid.to_string()) {
            tracing::debug!(error = %e, pid, dir = %dir.display(), "cgroup leaf attach");
            // Better the shared workspace cgroup than no cgroup at all: the leftover sweep
            // still finds the process there.
            Self::attach(ra, pid);
            let _ = std::fs::remove_dir(&dir);
            return None;
        }
        Some(dir)
    }

    /// The leaf a worker's children are in, when it exists.
    ///
    /// Derived from the session id rather than remembered, so a cancel after an agentd restart
    /// still finds the processes the dead agentd's worker left behind.
    pub fn leaf_path(ra: &RunAs, name: &str) -> Option<PathBuf> {
        let dir = ra.cgroup.as_ref()?.join(leaf_name(name));
        dir.is_dir().then_some(dir)
    }

    /// Remove an empty leaf once its worker is gone. Best-effort: a leaf that still holds a
    /// process refuses to go, which is the right outcome — the sweep wants to find it.
    pub fn remove_leaf(dir: &Path) {
        let _ = std::fs::remove_dir(dir);
    }

    /// Invariant check (D70): workspace A must not read workspace B's folder. Runs a shell as A's
    /// user; `Ok(true)` = blocked as required.
    pub fn cross_read_blocked(a: &RunAs, b_work_dir: &Path) -> Result<bool> {
        let out = Command::new("runuser")
            .args([
                "-u",
                &a.user,
                "--",
                "sh",
                "-c",
                &format!(
                    "ls {d} >/dev/null 2>&1 && echo LEAK; cat {d}/AGENTS.md >/dev/null 2>&1 && echo LEAK; echo done",
                    d = b_work_dir.display()
                ),
            ])
            .output()
            .context("runuser")?;
        let text = String::from_utf8_lossy(&out.stdout);
        Ok(!text.contains("LEAK"))
    }
}

/// A cgroup directory name for a session id: the id itself is already safe, but a name from
/// anywhere else must not escape the workspace cgroup with `..` or a slash.
fn leaf_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(64)
        .collect();
    format!("w-{safe}")
}

fn lookup(user: &str) -> Option<(u32, u32)> {
    let out = Command::new("id").args(["-u", user]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let uid: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    let out = Command::new("id").args(["-g", user]).output().ok()?;
    let gid: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    Some((uid, gid))
}

/// `"0-63"` / `"0-3,8"` → the first `n` CPUs as a cpuset string.
fn first_cpus(effective: &str, n: usize) -> Option<String> {
    let mut cpus = Vec::new();
    for part in effective.trim().split(',') {
        if part.is_empty() {
            continue;
        }
        if let Some((a, b)) = part.split_once('-') {
            let (a, b): (usize, usize) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
            cpus.extend(a..=b);
        } else {
            cpus.push(part.trim().parse().ok()?);
        }
    }
    if cpus.is_empty() {
        return None;
    }
    let picked: Vec<String> = cpus.into_iter().take(n).map(|c| c.to_string()).collect();
    Some(picked.join(","))
}

/// The image starts agentd as `daytona`; isolation needs root. Re-exec through sudo when allowed.
pub fn reexec_as_root_if_needed(local_box: bool) {
    if local_box
        || std::env::var("ARBOS_ISOLATION").is_ok_and(|v| v == "off")
        || nix::unistd::geteuid().as_raw() == 0
    {
        return;
    }
    let can = Command::new("sudo")
        .args(["-n", "true"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !can {
        tracing::warn!("isolation: no passwordless sudo; staying unprivileged");
        return;
    }
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("arbos-agentd"));
    tracing::info!("isolation: re-executing as root through sudo");
    use std::os::unix::process::CommandExt;
    let err = Command::new("sudo")
        .args(["-n", "-E", "--preserve-env=PATH"])
        .arg(&exe)
        .arg("boot")
        .exec();
    tracing::warn!(error = %err, "isolation: sudo re-exec failed; staying unprivileged");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The harness children's directories exist after the call (the re-own needs root and is
    /// exercised by the box: `subagent-sessions` writable by the workspace user).
    #[test]
    fn ensure_worker_dir_creates_nested_dirs() {
        let root = std::env::temp_dir().join(format!("arbos-worker-dir-{}", std::process::id()));
        let dir = root.join("pi").join("subagent-sessions");
        ensure_worker_dir(&dir, None).unwrap();
        assert!(dir.is_dir());
        // Idempotent on an existing dir.
        ensure_worker_dir(&dir, None).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    /// D70 invariant: workspace A cannot read workspace B's folder. Needs root and `useradd`
    /// (run inside the image: `sudo ARBOS_ISOLATION_TEST=1 arbos-agentd-test --ignored`); the
    /// box self-check at boot (`/healthz` → `isolation.cross_read_blocked`) runs the same check.
    #[test]
    #[ignore]
    fn workspace_a_cannot_read_workspace_b() {
        if std::env::var("ARBOS_ISOLATION_TEST").is_err() {
            return;
        }
        let iso = Isolation::detect(false);
        assert!(iso.enabled, "isolation must be enabled (root + useradd)");
        let root = std::env::temp_dir().join(format!("arbos-iso-{}", std::process::id()));
        let (a_dir, b_dir) = (root.join("01TESTA"), root.join("01TESTB"));
        for d in [&a_dir, &b_dir] {
            std::fs::create_dir_all(d.join(".arbos")).unwrap();
            std::fs::write(d.join("AGENTS.md"), "secret").unwrap();
        }
        let a = iso
            .prepare("01TESTA", &a_dir, &a_dir.join(".arbos"))
            .unwrap();
        let _b = iso
            .prepare("01TESTB", &b_dir, &b_dir.join(".arbos"))
            .unwrap();
        assert!(Isolation::cross_read_blocked(&a, &b_dir).unwrap());
        let _ = std::fs::remove_dir_all(&root);
    }
}
