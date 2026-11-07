//! The polarity of the pi crash-loop escalation's leftover sweep: nothing is collected without
//! positive evidence that it is abandoned.
//!
//! `escalate_pi_crash_loop` calls `kill_workspace_leftovers` as its **first** action, before it
//! rotates the session and before it reports the box degraded. What that sweep collects is
//! therefore decided at the worst possible moment — five children lost in a row, jobs still
//! running beside the dead main agent — so the rule it collects by is the whole of its safety.
//!
//! ## The two halves that did not match
//!
//! This is the defect, not the present design, and neither half of it survives.
//!
//! The old sweep read the workspace cgroup **recursively**: since bug Z every harness child sits
//! in a leaf of its own, so a read of the workspace's own `cgroup.procs` would find nothing. Its
//! keep list came from `live_child_pids`, a `HarnessAdapter` method returning the adapters'
//! **session children and no descendant of theirs**. A live job worker's `bash`, `python` and
//! `curl` were therefore in the kill list, absent from the keep list, and SIGKILLed on that
//! absence — killed because nobody had put them on a list, not because anything said they were
//! abandoned.
//!
//! **Neither name resolves in the tree today.** `live_child_pids` went with the keep list it
//! fed; the trait now carries `child_pgid`, which tells a cancel which process group to reap
//! (bug Z) and is not a keep list. And the sweep no longer subtracts the living from everything
//! in the cgroup at all: `reap_unreachable_workers` selects from marked processes in `/proc` and
//! collects only on positive evidence, which is the rule the cases below pin.
//!
//! Test5 round 16 logged "workspace leftovers killed… killed: 36" in the window 18:36:30–18:37:21
//! while eight compute jobs were mid-calculation, and five of those jobs died of
//! `Broken pipe (os error 32)` — what a worker sees when the child it is writing to is killed
//! underneath it. These cases do not claim that sequence as proven: see "What is not measured
//! here".
//!
//! ## The rule these cases pin
//!
//! `isolation::orphan_marked_procs` states it: "**The polarity is the safety property.** Nothing
//! is collected without positive evidence on both halves — that the mark names a session at all,
//! and that the session is over. Everything else is kept… The alternative, exempting known-safe
//! names, is one catastrophe per name anybody forgets."
//!
//! So each case below stands up a process the old keep list would not have named, and asserts
//! the sweep leaves it alone unless the box can say *why* it is garbage.
//!
//! ## What is not measured here
//!
//! Cgroups are not available in this environment, so none of this reproduces round 16's kill or
//! proves the fix against a real cgroup. **Nor do these two cases declare one any more.** They
//! once handed the agent a temp directory standing in for the workspace's, which read as
//! coverage of a cgroup-driven sweep; the sweep they drive selects from the marks in `/proc` and
//! never looked at it. A declaration nothing reads is worse than none, so it is gone, and what
//! these cases measure is the mark walk and the evidence it yields. Nothing in this file is
//! evidence about bug AJ's root cause.
//!
//! **`test_agent_in_cgroup` itself stays**, and is not to be read as dead because these two
//! stopped calling it. `worker_retention_tests` declares a cgroup and then reads `memory.max`
//! out of it through `workspace_mem_max`, which is a real dependency on the declaration. The
//! difference is the whole point: a fixture is worth having where something reads it, and these
//! cases were not that place.
//!
//! Real processes with real marks, read back out of `/proc`, for the same reason
//! `job_orphan_tests` uses them: nothing else tells a kill that worked from one that was only
//! issued. Linux-only, and skipped elsewhere.

#![cfg(target_os = "linux")]

use std::os::unix::process::CommandExt;
use std::process::Command;
use std::time::{Duration, Instant};

use arbos_core::ids::ulid;

use crate::isolation::{marked_procs_all, Evidence, WORKER_MARK_ENV};
use crate::status::tests::test_agent;

const CHAT: &str = "01TESTCHATCRASHLOOPLEFT001";

/// A marked tree as `RpcChild::spawn` leaves one: a worker root in a session of its own, and a
/// backgrounded `setsid` child it started. Both `exec` a plain sleep and fork nothing further,
/// so the marked set is exactly two and stays that way — a tree that respawns children makes
/// every exact-count assertion race its next fork.
///
/// The backgrounded child is the process that matters. It is a session of its own, so the old
/// keep list — session children only — never held it, and it is what a worker's tool call looks
/// like to the sweep. The sweep reaches it here through the mark it inherited, not through the
/// cgroup.
fn marked_tree(mark: &str) -> Tree {
    let before = pids_for(mark);
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg("setsid sleep 300 & exec sleep 300")
        .env(WORKER_MARK_ENV, mark)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            nix::libc::setsid();
            Ok(())
        });
    }
    let child = cmd.spawn().expect("the marked tree starts");
    let mut mine = Vec::new();
    wait_until(&format!("the `{mark}` tree is up"), || {
        mine = pids_for(mark)
            .into_iter()
            .filter(|p| !before.contains(p))
            .collect();
        mine.len() >= 2
    });
    Tree { pids: mine, child }
}

/// A process in the workspace that carries no mark at all, as an `env -i` exec leaves one. The
/// mark is removed explicitly because this test binary may itself have been started under one,
/// and an inherited mark would make this the opposite of the case it is here to measure.
fn unmarked_process() -> Tree {
    let child = Command::new("sleep")
        .arg("300")
        .env_remove(WORKER_MARK_ENV)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the unmarked process starts");
    Tree {
        pids: vec![child.id()],
        child,
    }
}

/// Kills its own pids on the way out, including when a case panics part-way. Without this a
/// failed assertion leaves real `sleep` processes carrying marks on the machine, and the next
/// run reads them as part of its own measurement.
struct Tree {
    pids: Vec<u32>,
    /// Held so [`Drop`] can reap it: a killed child stays a zombie until its parent waits, and
    /// this test process is that parent.
    child: std::process::Child,
}

impl Tree {
    fn pids(&self) -> &[u32] {
        &self.pids
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        for pid in &self.pids {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(*pid as i32),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The pids carrying `mark`, or a panic saying `/proc` could not be read.
///
/// `marked_procs_all` answers `None` for a `/proc` it could not walk, which is **not** an empty
/// process list — that conflation is the whole of bug AP, and a fixture that made it here would
/// measure an empty box and call the result a pass. The panic is the point: these cases have no
/// meaning without the walk, so they must fail loudly rather than quietly measure nothing.
fn pids_for(mark: &str) -> Vec<u32> {
    marked_procs_all()
        .expect("/proc must be readable, or these cases measure an empty box and prove nothing")
        .into_iter()
        .filter(|(_, m)| m == mark)
        .map(|(pid, _)| pid)
        .collect()
}

/// Running in the sense `isolation.rs` uses: a zombie is not. A SIGKILLed child of this process
/// keeps its `/proc` entry until it is waited on, so the directory existing says nothing.
fn is_running(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some((_, after_comm)) = stat.rsplit_once(')') else {
        return false;
    };
    !matches!(after_comm.split_whitespace().next(), Some("Z") | Some("X"))
}

fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        if ready() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{what} did not happen within 10s");
}

/// The measurement. Four processes stand on the box, none of them a session child an adapter
/// holds a handle for, and the escalation's sweep must take exactly the one the box can prove
/// is abandoned. The sweep finds them the way it finds anything: by walking `/proc` for marks.
///
/// The three survivors are each a different way of having no evidence against you, and the old
/// sweep killed all three:
///
/// 1. a mark naming a session no row in this box's store attests to — nothing says it was ever
///    a worker's, so nothing says its work is over;
/// 2. a mark naming no session at all, which is the shape `pi-main` has: the box's own agent and
///    every bash call it makes carry one of these;
/// 3. no mark at all, which is the case this fix knowingly gives up on. An unmarked process is
///    invisible to a mark walk, so nothing here names it abandoned and it is kept. A kept
///    leftover costs one more OOM kill; a killed tool call costs the job.
#[tokio::test]
async fn the_escalation_collects_only_what_the_box_can_prove_is_abandoned() {
    let abandoned_session = ulid();
    let unattested_session = ulid();
    let abandoned_mark = format!("pi:{abandoned_session}");
    let unattested_mark = format!("pi:{unattested_session}");
    // No colon, so it names no session: `session_of` returns `None`, exactly as for `pi-main`.
    // A unique string rather than the real `pi-main`, because `/proc` is the whole machine and
    // `job_orphan_tests` runs its own `pi-main` trees beside this one.
    let no_session_mark = format!("fake-pi-{}", ulid());

    let abandoned = marked_tree(&abandoned_mark);
    let unattested = marked_tree(&unattested_mark);
    let no_session = marked_tree(&no_session_mark);
    let unmarked = unmarked_process();

    let doomed = abandoned.pids().to_vec();
    let spared: Vec<u32> = unattested
        .pids()
        .iter()
        .chain(no_session.pids())
        .chain(unmarked.pids())
        .copied()
        .collect();

    assert!(
        doomed.len() >= 2,
        "the fixture must have a backgrounded descendant, or it measures nothing: {doomed:?}"
    );

    let (agent, _root) = test_agent().await;
    // The only evidence offered about any of these processes: this one session really was a
    // worker's, and the box holds no child for it.
    agent
        .store
        .insert_subagent(
            &abandoned_session,
            "job-abandoned",
            "a worker that died with its loop still running",
            "pi",
            CHAT,
        )
        .unwrap();

    let sweep = agent.kill_workspace_leftovers().await;

    assert_eq!(
        sweep.evidence,
        Evidence::Enumerated,
        "this sweep enumerated a tree and read each pid back, which is the only outcome a kill \
         may be claimed from (bugs AE and AP); nothing weaker earns the assertions below"
    );
    // The survivors come first: a sweep that collected the wrong thing has already done the
    // damage, and this is the assertion that says which process it was.
    for pid in &spared {
        assert!(
            is_running(*pid),
            "the escalation killed {pid}, for which it had no evidence of abandonment at all; \
             round 16 killed 36 processes this way while eight jobs were mid-calculation"
        );
    }
    assert_eq!(
        sweep.found,
        doomed.len(),
        "nothing but the abandoned worker's tree should have been considered at all"
    );
    assert!(
        sweep.clean(),
        "nothing should have survived this sweep: {:?}",
        sweep.survivors
    );
    wait_until("the abandoned worker's tree is gone", || {
        doomed.iter().all(|p| !is_running(*p))
    });
}

/// The same rule where it is most dangerous to get wrong: a box holding nothing but live work.
/// The sweep runs with no job row to reach any of it by, and must take none of it — and must say
/// so rather than report a kill it did not make.
#[tokio::test]
async fn a_workspace_of_live_work_loses_nothing_to_the_escalation() {
    let no_session_mark = format!("fake-pi-{}", ulid());
    let live = marked_tree(&no_session_mark);
    let tool_call = unmarked_process();
    let alive: Vec<u32> = live
        .pids()
        .iter()
        .chain(tool_call.pids())
        .copied()
        .collect();

    let (agent, _root) = test_agent().await;

    let sweep = agent.kill_workspace_leftovers().await;

    for pid in &alive {
        assert!(
            is_running(*pid),
            "every process here is live work: killing {pid} is the bug"
        );
    }
    // `found == 0` on its own is the assertion that decayed: it stays true whether the walk ran
    // and condemned nothing or never ran at all, and those are the two answers bug AP exists to
    // keep apart. The evidence value is what still says the interesting half.
    assert_eq!(
        sweep.evidence,
        Evidence::Nothing,
        "the walk read /proc and no process on this box was a candidate — which is neither \
         `Unconfirmed` (it could not look) nor `Enumerated` (it killed something)"
    );
    assert_eq!(
        sweep.found, 0,
        "no evidence was offered about anything, so nothing is a candidate"
    );
    assert_eq!(sweep.killed, 0, "and nothing is killed");
}
