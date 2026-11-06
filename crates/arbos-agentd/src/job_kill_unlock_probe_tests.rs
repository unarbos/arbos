//! Bug AI, Test5 round 15 (`r15cancel`, agentd 0.1.197 `ef90488f8`): after a `stop_job` that
//! left survivors and an explicit user OK to kill exactly those pids, `kill 6300 6308 6317 6326
//! 6335` was still refused by the "a signal is not a cancel" guard, and all five ran to 73/73.
//!
//! The report framed this as a regression in an unlock path that an `ask_user` OK is supposed
//! to open. There is no such path, and there never was: [`Agent::hook_kill_allowed`] reads one
//! thing, `cancel_leftovers`, which [`Agent::stop_jobs`] fills from `sweep.survivors`. The
//! user's answer is not an input to it, and the pi extension refuses the `kill` *before* the
//! approval card the user would answer. So the guard cannot be unlocked by a human saying yes
//! — only by the cancel having reported the pid it could not reap.
//!
//! That makes the whole of round 15 one question, which this probe asks as an invariant rather
//! than as a guess about the guard:
//!
//! > After a cancel, either nothing the job started is alive, or every pid that is alive is one
//! > the guard will let the agent kill.
//!
//! Both halves fail together in round 15 and for one reason. The sweep read handles the tree
//! had left (bug Z/AE), found nothing, recorded no survivor, and reported a clean cancel — so
//! the five processes lived *and* the guard had nothing to unlock on. The guard was right and
//! its input was wrong.
//!
//! The tree here is round 15's shape, not round 10's or 13's: five levels, each ticking into a
//! log of its own, with `setsid` stacked on `nohup` below the third, and the whole thing
//! backgrounded so it is orphaned onto PID 1 and writes to files rather than down a pipe. A
//! foreground tree passes against the broken code and proves nothing.
//!
//! Real processes and `/proc`, because nothing else tells a kill that worked from one that was
//! only issued. Linux-only, skipped elsewhere.

#![cfg(target_os = "linux")]

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use arbos_core::doc::UserMessageOpts;

use crate::agent::Agent;
use crate::status::tests::test_agent;

const CHAT: &str = "01TESTCHATKILLUNLOCK000001";
const JOB: &str = "01TESTJOBKILLUNLOCKR15001";
const ASK: &str = "01TESTASKKILLUNLOCK000001";
/// A worker id per test. The mark is read out of the box's process table, which is one
/// namespace for the whole suite, so two tests sharing a mark would each find the other's tree
/// and count it as a survivor of its own job — a flake that would look exactly like the bug.
const WORKER_R15: &str = "01TESTWORKERKILLUNLOCK001";

/// The env var a worker's descendants carry, by literal name so this file compiles against the
/// tree that has no such constant yet as well as the one that does.
const MARK_ENV: &str = "ARBOS_WORKER";

/// Round 15's tree: PARENT, CHILD, GRANDCHILD, then GREAT through `setsid` and GREAT2 through
/// `nohup`. One entry per link, so the tree is one level longer than this list.
const R15_LAUNCHERS: [&str; 4] = ["", "", "setsid", "nohup"];

/// Fast enough that "the log stopped growing" is measurable, slow enough that a level is
/// reliably mid-`sleep` when the cancel lands.
const TICK: &str = "0.2";

/// Many ticks at [`TICK`], so "it stopped writing" is measured rather than read off a pause.
const SETTLE: Duration = Duration::from_millis(1_500);

/// Round 15's worker: a root shell that leads a session and a group of its own and carries the
/// mark, whose tool call backgrounds a tree several levels deep.
struct DeepWorker {
    /// The root shell's pid, which is its session id and its process group id too.
    leader: u32,
    /// One pid per level, deepest last.
    pids: Vec<u32>,
    dir: PathBuf,
    /// Held only so [`Drop`] can reap it.
    shell: std::process::Child,
}

impl DeepWorker {
    fn start(dir: &Path, mark: &str, launchers: &[&str]) -> Self {
        let depth = launchers.len() + 1;
        for level in (1..=depth).rev() {
            write_level(dir, level, launchers.get(level - 1).copied());
        }
        // The middle shell exits the moment it has forked level 1, which orphans the tree onto
        // PID 1; the root shell stays alive, as a worker does until the cancel reaches it.
        let script = format!(
            "bash -c '( bash {} >/dev/null 2>&1 & )' &\nsleep 600",
            level_script(dir, 1).display()
        );
        let shell = spawn_root(&script, mark);
        let leader = shell.id();
        wait_until(Duration::from_secs(20), || {
            (1..=depth)
                .all(|l| pid_in(&level_pid(dir, l)).is_some() && lines(&level_log(dir, l)) > 0)
        });
        let pids: Vec<u32> = (1..=depth)
            .map(|l| {
                pid_in(&level_pid(dir, l)).unwrap_or_else(|| {
                    panic!("level {l} of the tree never started, so this probe proves nothing")
                })
            })
            .collect();
        Self {
            leader,
            pids,
            dir: dir.to_path_buf(),
            shell,
        }
    }

    /// Round 15's shape, asserted rather than assumed. Without each of these the probe would
    /// pass against the code that has the bug.
    fn assert_is_round_15_shape(&self) {
        assert_eq!(
            self.pids.len(),
            R15_LAUNCHERS.len() + 1,
            "the tree is not five levels deep: {:?}",
            self.pids
        );
        assert_eq!(
            ppid_of(self.pids[0]),
            Some(1),
            "level 1 is not orphaned, so no reparenting is being tested"
        );
        let mine = sid_of(self.leader);
        assert!(
            self.pids.iter().any(|p| sid_of(*p) != mine),
            "no level left the worker's session, so the `setsid` escape round 15 used is \
             absent: sessions {:?} against the worker's {mine:?}",
            self.sessions()
        );
    }

    fn sessions(&self) -> Vec<Option<u32>> {
        self.pids.iter().map(|p| sid_of(*p)).collect()
    }

    fn still_alive(&self) -> Vec<u32> {
        self.pids.iter().copied().filter(|p| alive(*p)).collect()
    }

    fn total_lines(&self) -> usize {
        (1..=self.pids.len())
            .map(|l| lines(&level_log(&self.dir, l)))
            .sum()
    }
}

impl Drop for DeepWorker {
    /// A failed assertion must not leave five tick loops on the box for the rest of the suite
    /// — which is, after all, the bug.
    fn drop(&mut self) {
        let mut all = self.pids.clone();
        all.push(self.leader);
        for pid in &all {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(*pid as i32),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        let _ = self.shell.wait();
    }
}

/// One level: record its pid, start the level below it through `launcher`, then tick.
fn write_level(dir: &Path, level: usize, launcher: Option<&str>) {
    let next = match launcher {
        Some(l) => {
            let cmd = format!("bash {}", level_script(dir, level + 1).display());
            match l {
                "" => format!("{cmd} >/dev/null 2>&1 &\n"),
                "nohup" => format!("nohup {cmd} >/dev/null 2>&1 &\n"),
                other => format!("{other} {cmd} >/dev/null 2>&1 &\n"),
            }
        }
        None => String::new(),
    };
    let body = format!(
        "echo $$ > {}\n{next}for k in $(seq 1 400); do sleep {TICK}; echo tick >> {}; done\n",
        level_pid(dir, level).display(),
        level_log(dir, level).display(),
    );
    std::fs::write(level_script(dir, level), body).expect("the level script is written");
}

/// The root shell as `RpcChild::spawn` leaves one: its own session, its own group, and the mark
/// in its environment that every process it starts inherits.
fn spawn_root(script: &str, mark: &str) -> std::process::Child {
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(script)
        .env(MARK_ENV, mark)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    unsafe {
        cmd.pre_exec(|| nix::unistd::setsid().map(|_| ()).map_err(Into::into));
    }
    cmd.spawn().expect("the worker shell starts")
}

fn level_script(dir: &Path, level: usize) -> PathBuf {
    dir.join(format!("tick15-l{level}.sh"))
}

fn level_pid(dir: &Path, level: usize) -> PathBuf {
    dir.join(format!("tick15-l{level}.pid"))
}

fn level_log(dir: &Path, level: usize) -> PathBuf {
    dir.join(format!("tick15-l{level}.log"))
}

fn lines(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

fn pid_in(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn wait_until(budget: Duration, mut ready: impl FnMut() -> bool) {
    let started = std::time::Instant::now();
    while started.elapsed() < budget {
        if ready() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn stat_field(pid: u32, n: usize) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after = stat.rsplit_once(')')?.1;
    after.split_whitespace().nth(n)?.parse().ok()
}

fn ppid_of(pid: u32) -> Option<u32> {
    stat_field(pid, 1)
}

fn sid_of(pid: u32) -> Option<u32> {
    stat_field(pid, 3)
}

/// Running, not a zombie waiting to be reaped: a zombie is dead work and counting it would make
/// a cancel that worked report that it had failed.
fn alive(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some(after) = stat.rsplit_once(')').map(|(_, rest)| rest) else {
        return false;
    };
    !matches!(
        after.split_whitespace().next(),
        Some("Z") | Some("X") | None
    )
}

/// The job as `run_subagent` leaves one, with the worker's leader recorded.
async fn a_running_job(agent: &std::sync::Arc<Agent>, leader: u32, worker: &str) {
    let host = agent.sync.open_room(CHAT).await.unwrap();
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(
                ASK,
                "start a five-level ticker under out/tick15*.log",
                "dev-mac",
                &UserMessageOpts::default(),
            )
            .unwrap();
    }
    agent
        .store
        .insert_job(
            JOB,
            CHAT,
            ASK,
            "Ticking five logs",
            "code",
            "Start a five-level ticker writing to out/tick15*.log.",
            &[],
        )
        .unwrap();
    agent
        .store
        .insert_subagent(worker, "job-tick15", "tick five logs", "pi", CHAT)
        .unwrap();
    agent.store.set_job_subagent(JOB, worker).unwrap();
    agent.store.set_job_state(JOB, "running", None).unwrap();
    agent.note_worker_pgid(worker, leader).await;
}

/// **Bug AI.** A cancel leaves the user with a way out: either the work is dead, or the guard
/// lets the agent kill what is left.
///
/// Round 15 failed both halves at once. The cancel marked the job `cancelled` and reported that
/// "every process it had started is dead"; the five processes wrote on to 73/73; and because
/// the sweep had recorded no survivor, `hook_kill_allowed` refused `kill 6300 6308 6317 6326
/// 6335` no matter what the user had agreed to.
///
/// So the assertion is not "the OK unlocks the guard" — the OK is not an input to the guard and
/// making it one would hand every runaway `kill` a one-word key. It is that the cancel leaves
/// no process the user cannot get rid of.
#[tokio::test]
async fn a_cancel_leaves_nothing_alive_that_the_guard_will_not_let_the_agent_kill() {
    let (agent, root) = test_agent().await;
    let worker = DeepWorker::start(&root, WORKER_R15, &R15_LAUNCHERS);
    worker.assert_is_round_15_shape();
    a_running_job(&agent, worker.leader, WORKER_R15).await;
    let _turn = agent.test_hold_turn(CHAT).await;

    let stopped = agent.stop_jobs(CHAT, Some(JOB)).await;
    assert_eq!(
        stopped[0].state, "cancelled",
        "#2779's half must still hold"
    );

    // Give what the cancel signalled a moment to die, then read the kernel rather than the
    // cancel's own opinion of what it did.
    std::thread::sleep(SETTLE);
    let alive_now = worker.still_alive();
    if alive_now.is_empty() {
        let at_cancel = worker.total_lines();
        std::thread::sleep(SETTLE);
        assert_eq!(
            worker.total_lines(),
            at_cancel,
            "every level reads dead and the logs are still growing"
        );
        let _ = std::fs::remove_dir_all(&root);
        return;
    }

    // Something outlived the cancel. That is allowed — the sweep is best-effort — but only if
    // the cancel reported it, because the report is the only thing that unlocks the guard.
    let command = format!(
        "kill {}",
        alive_now
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(" ")
    );
    let verdict = agent.hook_kill_allowed(&command).await;
    assert!(
        verdict.is_ok(),
        "the cancel left {alive_now:?} running and the guard refuses `{command}`, so the user \
         has no way to stop them at all. stop_job reported survivors {:?} out of {} found. \
         Refusal: {}",
        stopped[0].sweep.survivors,
        stopped[0].sweep.found,
        verdict.unwrap_err(),
    );

    let _ = std::fs::remove_dir_all(&root);
}
