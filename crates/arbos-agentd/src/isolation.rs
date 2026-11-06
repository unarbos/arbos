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

/// The environment variable every harness child is spawned with, carrying its session id.
///
/// An environment is copied on fork and carried through exec, so every process a job's tool
/// calls start inherits this mark — and keeps it when its parent dies, when it is reparented to
/// PID 1, when it changes process group, when it calls `setsid`, and at any depth of tree.
/// `nohup` and a double fork shed none of it either. No set the kernel maintains survives all
/// of that outside a cgroup, which is why a box without one needs the mark to find a tree at
/// all.
///
/// One way out of it, named rather than hidden: a process that execs with its environment
/// cleared (`env -i`) sheds the mark, and on a box with isolation the cgroup is what still
/// holds that process.
///
/// The value is a worker's session id — a ULID agentd minted — so a cancel of one worker reads
/// only its own mark, and the main agent's children, another job's worker and another
/// workspace's processes are all invisible to it.
pub const WORKER_MARK_ENV: &str = "ARBOS_WORKER";

/// Where a harness child's descendants can be found again once the child itself is gone.
///
/// Killing a process does not kill its children on Linux, so a job's `bash` tool call outlives
/// the worker that started it — detached or not (Test5 round 10, bug Z: pid 561 wrote all 40
/// lines after the job read `cancelled`). None of the handles here is the child's pid: each
/// names a set the kernel keeps for us, so a tree can be reaped after its root has died and its
/// children have been reparented to PID 1.
///
/// They are read together and in this order of strength, measured rather than assumed:
///
/// - **cgroup**: everything, and the only handle a process cannot leave by its own action.
///   Membership is inherited on fork, `setsid` does not change it, and `cgroup.kill` ends the
///   whole subtree in one atomic write. It exists only where agentd is root and cgroup v2 is
///   writable, which is the box and not a dev machine — so it is relied on where it is there
///   and the rest of this list is what a box without it has.
/// - **mark**: everything a process cannot shed short of clearing its own environment. This is
///   what reaches a descendant that calls `setsid` itself (Test5 round 14: the job's tree was
///   2571, 2572, 2574 and 2576, and 2576 led a session of its own and wrote on after the job
///   read `cancelled`), and a tree that stacks `setsid` on `nohup` at any depth.
/// - **session**: everything except a deliberate `setsid`. A session id is inherited on fork,
///   is untouched by reparenting, and — unlike the group — is untouched by the `setpgid` a tool
///   runner does to give its command a group it can time out. This is the handle Test5 round 13
///   needed: the job backgrounded `(bash tmp/tick13.sh > tmp/tick13.run.out 2>&1; …) &`, shell
///   1698 was reparented to PID 1 in its own group, and 1698, 1700, 1701 and 1702 ran to
///   60/60 after the job read `cancelled`.
/// - **process group**: a plain child and `nohup … &` (neither leaves the group — `nohup` only
///   ignores `SIGHUP`, and `&` starts no new group in a non-interactive shell), but not a
///   command the tool runner put in a group of its own, and not an explicit `setsid`.
///
/// Each is incomplete on its own and the union is what a cancel kills. The union is still not a
/// proof — a process can leave the last of them by clearing its environment on a box with no
/// cgroup — which is why [`ProcessTree::kill`] reads the kernel back by pid and reports
/// survivors instead of assuming a clean kill.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessTree {
    /// The child's own process group.
    pub pgid: Option<u32>,
    /// The child's own session. `RpcChild` spawns through `setsid`, so the child leads both a
    /// session and a group of its own, and a kill of either cannot reach agentd.
    pub sid: Option<u32>,
    /// The child's cgroup leaf under the workspace cgroup, when isolation made one.
    pub cgroup: Option<PathBuf>,
    /// The child's [`WORKER_MARK_ENV`] value, for the read-back.
    pub mark: Option<String>,
}

/// What a kill of a [`ProcessTree`] did, read back from the kernel afterwards rather than
/// inferred from the signals sent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sweep {
    /// Processes of the tree that were alive when the kill started.
    pub found: usize,
    /// How many of them are gone now.
    pub killed: usize,
    /// Pids still alive after the read-back, each one a process the caller must name rather
    /// than quietly leave to the user.
    pub survivors: Vec<u32>,
    /// Did this sweep read the kernel at all?
    ///
    /// `false` is the default, because a cancel that looked at nothing knows nothing. Test5
    /// round 13, bug AE: `stop_job` reported "every process it had started is dead" from a
    /// sweep that had found nothing to look at, the agent repeated it, and the guard then
    /// refused `kill 1700 1701 1702` because no pid had been reported unkillable.
    pub checked: bool,
}

impl Sweep {
    /// A job with no worker: there were never any processes, and saying so is not a guess.
    pub fn nothing_to_check() -> Self {
        Self {
            checked: true,
            ..Self::default()
        }
    }

    /// The job's work really stopped — which needs the kernel to have been read, not only an
    /// empty survivor list from a sweep that never looked.
    pub fn clean(&self) -> bool {
        self.checked && self.survivors.is_empty()
    }

    /// Two sweeps of one cancel, read as one answer.
    ///
    /// A job can have left processes under more than one worker (bug AO), and the user is owed a
    /// single statement about the job. `checked` is the conjunction and not the disjunction: one
    /// tree the cancel could not read with is enough to make "every process is dead" a claim the
    /// box cannot support, however well the others went.
    pub fn and(mut self, other: Self) -> Self {
        self.found += other.found;
        self.killed += other.killed;
        self.checked &= other.checked;
        self.survivors.extend(other.survivors);
        self.survivors.sort_unstable();
        self.survivors.dedup();
        self
    }
}

/// One process as it was when the kill started: its pid and the start time that tells it from a
/// later process the kernel gave the same pid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Witness {
    pid: u32,
    start: u64,
}

impl Witness {
    /// `None` once the process is gone or is a zombie waiting to be reaped.
    fn of(pid: u32) -> Option<Self> {
        let st = read_stat(pid)?;
        st.running().then_some(Self {
            pid,
            start: st.start,
        })
    }
}

/// How long the kill waits for the kernel to reap what it signalled before reading back. A
/// SIGKILL is not synchronous, so a re-read with no pause would report live pids that are
/// already dying — the opposite of the honest answer this is here to give.
const REAP_WAIT: std::time::Duration = std::time::Duration::from_millis(100);
const REAP_TRIES: usize = 20; // 2 s, inside the spec's five-second budget

impl ProcessTree {
    pub fn is_empty(&self) -> bool {
        self.pgid.is_none() && self.sid.is_none() && self.cgroup.is_none() && self.mark.is_none()
    }

    /// Every live process of this tree: the cgroup's members, the session's, the process
    /// group's and everything carrying the mark, together.
    ///
    /// All four are read, not whichever looks authoritative: on a box with isolation the cgroup
    /// is the complete set and the rest add nothing — but on one without it the cgroup is
    /// `None` and they are all there is. The mark is what makes the union hold at a depth and
    /// through a `setsid` that no other handle here reaches.
    ///
    /// A handle that names agentd's **own** session or group is dropped rather than swept. The
    /// ids are recorded from a child that leads both, so this cannot happen from a correct
    /// record — and the one outcome it rules out is a cancel that SIGKILLs the box's agentd.
    pub fn members(&self) -> Vec<u32> {
        let mine = Ids::own();
        let sid = self.sid.filter(|s| *s != mine.sid);
        let pgid = self.pgid.filter(|g| *g != mine.pgid);
        let mut pids: Vec<u32> = Vec::new();
        if let Some(dir) = &self.cgroup {
            pids.extend(cgroup_procs(dir));
        }
        if sid.is_some() || pgid.is_some() {
            for (pid, st) in all_stats() {
                if sid == Some(st.session) || pgid == Some(st.pgrp) {
                    pids.push(pid);
                }
            }
        }
        if let Some(mark) = &self.mark {
            pids.extend(marked_procs(mark));
        }
        let me = std::process::id();
        pids.retain(|p| *p > 1 && *p != me && is_running(*p));
        pids.sort_unstable();
        pids.dedup();
        pids
    }

    /// SIGKILL every member of the tree, then read the kernel back and report what is left.
    ///
    /// The read-back is the point, and it is done **by pid**: each process found at the start
    /// is re-read at `/proc/<pid>` and matched on its start time, rather than the handles being
    /// enumerated a second time. Re-enumerating proves only that the handle is empty, which is
    /// what a process that left the handle makes true without dying — bug AE's claim, in one
    /// sentence.
    ///
    /// The kill is re-issued each round and the membership re-read, so a tree that forks while
    /// it is being reaped is caught on the next pass rather than slipping between the
    /// enumeration and the signal.
    pub fn kill(&self) -> Sweep {
        // No handle, no claim. A tree that resolved nothing cannot tell "I read the kernel and
        // found nothing alive" from "I had nothing to read with", and those are the two
        // statements bug AE exists to keep apart — so it makes neither. `Sweep::default` is
        // `checked: false`, which `clean` refuses and the chat reports as "I could not check".
        //
        // This is the mechanism's own guard rather than the caller's. Before bug AI the mark
        // was unconditionally `Some`, so this was never reachable and a sweep that resolved no
        // usable handle at all still returned `checked: true` with an empty survivor list —
        // which `clean` reads as success, and `r17status` printed as "every process it had
        // started is dead" over a loop that was still writing.
        if self.is_empty() {
            return Sweep::default();
        }
        let found = self.members();
        let witnesses: Vec<Witness> = found.iter().copied().filter_map(Witness::of).collect();
        if !found.is_empty() {
            // `cgroup.kill` (cgroup v2, Linux 5.14+) kills the whole subtree in one atomic
            // write, so a process forked while we are enumerating cannot slip out between the
            // read and the signal. Where it is missing, the enumerated kill below is the
            // fallback.
            if let Some(dir) = &self.cgroup {
                let _ = std::fs::write(dir.join("cgroup.kill"), "1");
            }
            for pid in &found {
                signal(*pid);
            }
            // The group and the session as wholes, which cover a child forked since the
            // enumeration.
            let mine = Ids::own();
            if let Some(pgid) = self.pgid.filter(|g| *g != mine.pgid) {
                let _ = nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(pgid as i32),
                    nix::sys::signal::Signal::SIGKILL,
                );
            }
        }
        let mut alive = Vec::new();
        for attempt in 0..REAP_TRIES {
            if found.is_empty() {
                break;
            }
            std::thread::sleep(REAP_WAIT);
            alive = still_alive(&witnesses);
            for pid in self.members() {
                if !alive.contains(&pid) {
                    alive.push(pid);
                }
            }
            if alive.is_empty() {
                break;
            }
            // Something forked after the first pass, or is still dying. Signal again.
            if attempt < REAP_TRIES - 1 {
                for pid in &alive {
                    signal(*pid);
                }
            }
        }
        let mut survivors = alive;
        survivors.sort_unstable();
        Sweep {
            found: found.len(),
            killed: found.iter().filter(|p| !survivors.contains(p)).count(),
            survivors,
            checked: true,
        }
    }
}

/// The witnesses that are still the process they were, read one pid at a time. A pid the kernel
/// has given to something else since is not a survivor, and signalling it again would be a kill
/// aimed at a stranger.
fn still_alive(witnesses: &[Witness]) -> Vec<u32> {
    witnesses
        .iter()
        .filter(|w| Witness::of(w.pid) == Some(**w))
        .map(|w| w.pid)
        .collect()
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
    read_stat(pid).is_some_and(|st| st.running())
}

/// agentd's own session and process group, which no sweep may take as a job's.
struct Ids {
    sid: u32,
    pgid: u32,
}

impl Ids {
    fn own() -> Self {
        // Both calls read ids of the calling process and cannot fail.
        unsafe {
            Self {
                sid: nix::libc::getsid(0).max(0) as u32,
                pgid: nix::libc::getpgrp().max(0) as u32,
            }
        }
    }
}

/// The fields of `/proc/<pid>/stat` a sweep reads.
struct ProcStat {
    state: char,
    pgrp: u32,
    session: u32,
    /// Clock ticks since boot at which the process started: what tells a pid from the same pid
    /// after the kernel has reused it.
    start: u64,
}

impl ProcStat {
    /// `Z` zombie, `X` dead.
    fn running(&self) -> bool {
        !matches!(self.state, 'Z' | 'X')
    }
}

/// `/proc/<pid>/stat` is `pid (comm) state ppid pgrp session … starttime …` and `comm` may hold
/// spaces and parentheses, so the fields are counted from the last `)` rather than split on
/// whitespace.
fn parse_stat(stat: &str) -> Option<ProcStat> {
    let after = stat.rsplit_once(')').map(|(_, rest)| rest)?;
    let fields: Vec<&str> = after.split_whitespace().collect();
    Some(ProcStat {
        state: fields.first()?.chars().next()?,
        pgrp: fields.get(2)?.parse().ok()?,
        session: fields.get(3)?.parse().ok()?,
        // state is field 3 of the file, starttime is field 22, so 19 fields along from here.
        start: fields.get(19)?.parse().ok()?,
    })
}

fn read_stat(pid: u32) -> Option<ProcStat> {
    // Gone between a read and here is the outcome a cancel wants, so it reads as `None`.
    parse_stat(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// Every process on the box with the stat fields a sweep reads, in one pass of `/proc`.
fn all_stats() -> Vec<(u32, ProcStat)> {
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
        if let Some(st) = parse_stat(&stat) {
            out.push((pid, st));
        }
    }
    out
}

/// Pids whose environment holds `ARBOS_WORKER=<mark>`.
///
/// `/proc/<pid>/environ` is the environment the process was exec'd with, readable by its owner
/// — agentd is root on a box and owns its children on a dev machine. A process whose environ
/// cannot be read is skipped: the mark is how a cancel finds what escaped it, and a read that
/// fails is one more reason to report rather than to promise.
fn marked_procs(mark: &str) -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let needle = format!("{WORKER_MARK_ENV}={mark}");
    let mut out = Vec::new();
    for e in entries.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(environ) = std::fs::read(e.path().join("environ")) else {
            continue;
        };
        if environ
            .split(|b| *b == 0)
            .any(|entry| entry == needle.as_bytes())
        {
            out.push(pid);
        }
    }
    out
}

/// Every process carrying a worker mark, with the value it carries.
///
/// [`marked_procs`] answers "where is this worker's tree", which a cancel asks because it has a
/// job row naming the worker. This answers the question a cancel cannot ask: **whose** is this
/// process, for a process nobody named. Bug AK left a loop running under a worker that had died,
/// with no job row to reach it by — the mark was the only thing that still said where it came
/// from, and nothing read marks it had not been given.
///
/// Same `/proc` read and the same skip-what-cannot-be-read rule as [`marked_procs`].
pub fn marked_procs_all() -> Vec<(u32, String)> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let prefix = format!("{WORKER_MARK_ENV}=");
    let mut out = Vec::new();
    for e in entries.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(environ) = std::fs::read(e.path().join("environ")) else {
            continue;
        };
        let found = environ.split(|b| *b == 0).find_map(|entry| {
            let text = std::str::from_utf8(entry).ok()?;
            text.strip_prefix(&prefix).map(str::to_owned)
        });
        if let Some(mark) = found {
            out.push((pid, mark));
        }
    }
    out
}

/// The session id a mark names, for a mark that names one at all.
///
/// Marks are written in exactly one place ([`crate::rpc_child::RpcChild::spawn`]) and come in
/// two kinds: `<harness>:<session id>` for a worker, and a bare name for a child that is not a
/// session — `pi-main` for the box's own agent, `fake-pi` and `fake-pi-<tag>` in tests. The bare
/// names are why this returns an `Option` instead of the caller splitting on `:` and hoping.
pub fn session_of(mark: &str) -> Option<&str> {
    let (_harness, session) = mark.split_once(':')?;
    (!session.is_empty()).then_some(session)
}

/// Processes whose worker mark names a session that is **provably** a worker's and **provably**
/// over: work nobody can ever reach again, because the only handle that could have named it
/// died with its worker.
///
/// `is_dead_session` is the caller's, because only the box knows which sessions it still holds
/// children for. Deriving the keep set this way rather than listing exclusions is what makes it
/// safe to extend: another job's tree is spared because its session is live, not because
/// someone remembered to add it to a list.
///
/// **The polarity is the safety property.** Nothing is collected without positive evidence on
/// both halves — that the mark names a session at all, and that the session is over. Everything
/// else is kept, including a mark this function has never seen. The alternative, exempting
/// known-safe names, is one catastrophe per name anybody forgets: the main pi child is spawned
/// through `RpcChild` exactly like a worker, so it and **everything it runs** carry the mark
/// `pi-main`, and a classifier that looked that up as a worker session, found nothing and called
/// it garbage would SIGKILL the box's own agent mid-turn. `pi-main` is not special here. It is
/// simply one of the marks that never proves itself dead, which is also true of every name a
/// future caller invents.
pub fn orphan_marked_procs(is_dead_session: impl Fn(&str) -> bool) -> Vec<(u32, String)> {
    let me = std::process::id();
    let mut out: Vec<(u32, String)> = marked_procs_all()
        .into_iter()
        .filter(|(pid, mark)| {
            *pid > 1 && *pid != me && session_of(mark).is_some_and(&is_dead_session)
        })
        .collect();
    out.sort_unstable();
    out
}

/// Kill every process left by a worker whose session is over, and read the kernel back to say
/// what is still alive.
///
/// The same discipline as [`ProcessTree::kill`], and for the same reason: the survivors are the
/// output that matters. Bug AE was a cancel reporting a clean kill from a sweep that never
/// looked, so a reaper that cannot account for a process must name it rather than let the box
/// go on claiming everything is accounted for.
///
/// Re-enumerated between rounds, so a process the set forks while it is dying is caught on the
/// next pass instead of slipping between the read and the signal.
pub fn reap_orphan_workers(is_dead_session: impl Fn(&str) -> bool) -> Sweep {
    let found = orphan_pids(&is_dead_session);
    let witnesses: Vec<Witness> = found.iter().copied().filter_map(Witness::of).collect();
    for pid in &found {
        signal(*pid);
    }
    let mut alive = Vec::new();
    for attempt in 0..REAP_TRIES {
        if found.is_empty() {
            break;
        }
        std::thread::sleep(REAP_WAIT);
        alive = still_alive(&witnesses);
        for pid in orphan_pids(&is_dead_session) {
            if !alive.contains(&pid) {
                alive.push(pid);
            }
        }
        if alive.is_empty() {
            break;
        }
        if attempt < REAP_TRIES - 1 {
            for pid in &alive {
                signal(*pid);
            }
        }
    }
    alive.sort_unstable();
    Sweep {
        found: found.len(),
        killed: found.iter().filter(|p| !alive.contains(p)).count(),
        survivors: alive,
        checked: true,
    }
}

fn orphan_pids(is_dead_session: &impl Fn(&str) -> bool) -> Vec<u32> {
    orphan_marked_procs(is_dead_session)
        .into_iter()
        .map(|(pid, _)| pid)
        .collect()
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
