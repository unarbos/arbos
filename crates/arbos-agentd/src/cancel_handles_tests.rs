//! Bug AI: the two handles a cancel needs most, and the string that switched them both off.
//!
//! `RpcChild::spawn` starts a worker under a name — `prime:<session id>` or `pi:<session id>` —
//! and that name becomes two things: the `ARBOS_WORKER` value every descendant inherits, and
//! the cgroup leaf the worker is attached to. `Agent::worker_tree` rebuilt both from the bare
//! session id instead, so it looked for `ARBOS_WORKER=<session id>` against an environment
//! holding `prime:<session id>`, and for a leaf `w-<session id>` against one created as
//! `w-prime-<session id>`. Both missed, silently, on every cancel the box has ever run.
//!
//! What was left is the recorded process group and session — **precisely the two handles a
//! `setsid` descendant escapes**, which is why `setsid` and grandchild shapes are the ones that
//! survived cancel in Test5 rounds 13, 14, 16 and 17 while a plain `&` child died as intended.
//!
//! The code's own comments say what was being lost. The mark "is what makes the union hold at a
//! depth and through a `setsid` that no other handle here reaches"; the cgroup "is the one
//! handle that finds every descendant". Both were reasoned about carefully and neither ran.
//!
//! ## Why the tests did not catch it
//!
//! `RpcChild` records the tree correctly at spawn, and `job_leftover_tests` kills through
//! `worker.tree()` — the correct object. Production never does: the only non-test caller of
//! `RpcChild::tree()` takes `.pgid` and discards the rest, and every production kill goes
//! through `worker_tree`'s reconstruction. **The suite validated an object production does not
//! use.** That is the part worth remembering, because the next handle added here will be wired
//! into the correct tree and will watch green tests while the box keeps leaking.
//!
//! So these cases go through `Agent::worker_tree` by session id — the way a cancel reaches a
//! worker — and never through `RpcChild::tree()`.

use std::os::unix::process::CommandExt;
use std::process::Command;
use std::time::{Duration, Instant};

use arbos_core::ids::ulid;

use crate::isolation::{ProcessTree, WORKER_MARK_ENV};
use crate::status::tests::test_agent;

/// A worker as `RpcChild::spawn` leaves one: a session of its own, and the mark its descendants
/// inherit. The root backgrounds a `setsid` child which in turn starts the writer, so the
/// writer is a **grandchild in a session of its own** — `g` and `e`'s shapes from `r17status`,
/// and the one shape no recorded pgid or sid can reach.
fn worker_with_setsid_grandchild(mark: &str, flag: &std::path::Path) -> std::process::Child {
    let script = format!(
        "setsid bash -c 'exec sleep 300' & exec sleep 300; : {}",
        flag.display()
    );
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(script)
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
    cmd.spawn().expect("the worker shell starts")
}

fn marked_pids(mark: &str) -> Vec<u32> {
    ProcessTree {
        pgid: None,
        sid: None,
        cgroup: None,
        mark: Some(mark.to_owned()),
    }
    .members()
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

fn reap(pids: &[u32]) {
    for pid in pids {
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(*pid as i32),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
}

/// The cancel must find a worker's `setsid` descendant, reached the way a cancel reaches one:
/// by session id, through `Agent::worker_tree`.
///
/// Before the fix this found nothing — not "failed to kill", but an empty enumeration, which is
/// worse, because an empty enumeration is what a clean sweep looks like.
#[tokio::test]
async fn the_cancel_finds_a_workers_setsid_descendant_by_session_id() {
    let (agent, _dir) = test_agent().await;
    let sid = ulid();
    agent
        .store
        .insert_subagent(
            &sid,
            "worker",
            "a backgrounded loop",
            "prime",
            "01TESTCHATCANCELAI000001",
        )
        .expect("the subagent row is written before the worker spawns");

    // Exactly what `RpcChild::spawn` would have put in the environment for this session.
    let mark = crate::harness::worker_name("prime", &sid);
    let flag = std::env::temp_dir().join(format!("ai-{sid}"));
    let mut root = worker_with_setsid_grandchild(&mark, &flag);
    wait_until("the worker and its setsid descendant are up", || {
        marked_pids(&mark).len() >= 2
    });
    let planted = marked_pids(&mark);

    let tree = agent.worker_tree(&sid).await;
    let found = tree.members();

    for pid in &planted {
        assert!(
            found.contains(pid),
            "the cancel must reach everything the worker left, including the descendant that \
             called setsid: {pid} missing from {found:?} (tree mark {:?})",
            tree.mark
        );
    }

    reap(&planted);
    let _ = root.kill();
    let _ = root.wait();
    let _ = std::fs::remove_file(&flag);
}

/// The mark is built from the name the worker was spawned under, not from its session id. This
/// is the whole of bug AI in one assertion, and it is kept separate from the case above so the
/// cause is named even if the behavioural case is ever rewritten.
#[tokio::test]
async fn the_cancels_mark_is_the_name_the_worker_was_spawned_under() {
    let (agent, _dir) = test_agent().await;
    let sid = ulid();
    agent
        .store
        .insert_subagent(
            &sid,
            "worker",
            "a backgrounded loop",
            "prime",
            "01TESTCHATCANCELAI000002",
        )
        .expect("the subagent row is written before the worker spawns");

    let tree = agent.worker_tree(&sid).await;
    assert_eq!(
        tree.mark.as_deref(),
        Some(crate::harness::worker_name("prime", &sid).as_str()),
        "the environment holds the spawn name; a mark spelled any other way matches nothing"
    );
    assert_ne!(
        tree.mark.as_deref(),
        Some(sid.as_str()),
        "the bare session id is the spelling that never matched"
    );
}

/// A worker the box knows nothing about offers no handle, and a sweep with no handle must not
/// report success. "I read the kernel and found nothing" and "I had nothing to read with" are
/// the two claims bug AE exists to keep apart.
///
/// Before the fix the mark was unconditionally `Some(sid)`, so this tree was never empty, the
/// kill ran, found nothing, and returned `checked: true` with no survivors — which `clean`
/// reads as success. That is what `r17status` printed as "every process it had started is
/// dead" over a loop that was still writing.
#[tokio::test]
async fn a_sweep_with_no_handle_says_it_could_not_check_rather_than_that_all_is_well() {
    let (agent, _dir) = test_agent().await;
    let tree = agent.worker_tree(&ulid()).await;

    assert!(
        tree.is_empty(),
        "nothing is known about this worker, so there is no handle to sweep with: {tree:?}"
    );
    let sweep = tree.kill();
    assert!(
        !sweep.checked,
        "a sweep that resolved no handle did not check anything: {sweep:?}"
    );
    assert!(
        !sweep.clean(),
        "and must not report the work stopped: {sweep:?}"
    );
}

/// Claude Code spawns its own `Command` rather than going through `RpcChild`, so its children
/// carry no mark and own no cgroup leaf. Constructing a name for them would be a handle that
/// cannot match — which reads as a clean sweep over work that is still running.
#[tokio::test]
async fn a_harness_that_marks_nothing_is_given_no_mark_to_match() {
    let (agent, _dir) = test_agent().await;
    let sid = ulid();
    agent
        .store
        .insert_subagent(
            &sid,
            "worker",
            "a claude job",
            "claude_code",
            "01TESTCHATCANCELAI000003",
        )
        .expect("the subagent row is written before the worker spawns");

    let tree = agent.worker_tree(&sid).await;
    assert_eq!(
        tree.mark, None,
        "a mark that cannot match is not a handle, and must not be counted as one"
    );
    assert!(
        !tree.kill().checked,
        "so a cancel of one of these reports that it could not check, not that all is well"
    );
}
