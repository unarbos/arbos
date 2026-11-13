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
//!
//! ## And why the first of them counted nothing
//!
//! The same lesson arrived a second time, from the fixture rather than the object. The
//! behavioural case below waited for `marked_pids(&mark).len() >= 2` and took that for "the
//! worker and its `setsid` descendant are up". A count is not a shape: **the case passed green
//! against a planted tree with no `setsid` in it at all**, measured by deleting the word. What
//! it is about was optional, and a weak premise let a real race through on top — see
//! [`planted_shape`], which is the premise it should always have had.

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
///
/// Both processes end as a plain `sleep` and fork nothing further, which is what lets
/// [`planted_shape`] recognise the finished tree rather than count members of a half-built one.
fn worker_with_setsid_grandchild(mark: &str) -> std::process::Child {
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg("setsid bash -c 'exec sleep 300' & exec sleep 300")
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

/// A marked process as the fixture needs to see it: what it is running now, and whose session
/// it is in.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Marked {
    pid: u32,
    session: u32,
    comm: String,
}

/// Every process carrying `mark`, read straight from `/proc`.
///
/// Deliberately not [`ProcessTree::members`]. That is the call under test, and a premise
/// established with it would be no premise at all: a discovery that found nothing would report
/// an unplanted fixture instead of a broken discovery, which is the one confusion this file
/// cannot afford.
fn marked_now(mark: &str) -> Vec<Marked> {
    let needle = format!("{WORKER_MARK_ENV}={mark}");
    let mut out = Vec::new();
    for entry in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(environ) = std::fs::read(entry.path().join("environ")) else {
            continue;
        };
        if !environ.split(|b| *b == 0).any(|v| v == needle.as_bytes()) {
            continue;
        }
        let (Some(session), Ok(comm)) = (
            session_of(pid),
            std::fs::read_to_string(entry.path().join("comm")),
        ) else {
            continue;
        };
        out.push(Marked {
            pid,
            session,
            comm: comm.trim().to_owned(),
        });
    }
    out.sort_by_key(|m| m.pid);
    out
}

/// The session of `pid`, field 6 of `/proc/<pid>/stat`. Counted from after the last `)` because
/// a process name may hold spaces and parentheses of its own.
fn session_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, after) = stat.rsplit_once(')')?;
    after.split_whitespace().nth(3)?.parse().ok()
}

/// The two processes this case is about, pinned by pid.
#[derive(Debug, Clone, Copy)]
struct Planted {
    /// The worker root, which leads the session it was spawned into.
    root: u32,
    /// Its descendant, which called `setsid` and so leads a session of its own — the one shape
    /// no recorded pgid or session of the worker can reach.
    escapee: u32,
}

/// The planted tree once it has stopped changing, or everything carrying the mark instead.
///
/// **A count cannot tell the finished shape from a half-built one, and that is what failed on
/// `ecb04054b`.** The old premise was `marked_pids(&mark).len() >= 2`, which the root plus
/// whatever is mid-`setsid` satisfies exactly as well as the root plus the descendant. The
/// case then compared two separate walks of `/proc` and required the first to be a subset of
/// the second — and `/proc/<pid>/environ` is not readable atomically against `execve`, so a pid
/// counted into the premise can be legitimately missing from any later read. The run that
/// blocked the Mac release asserted `4177 missing from [4178]` for exactly that reason.
///
/// No wait makes that invariant true; a longer one only makes it rare, which is worse, because
/// it comes back on someone else's release. So the premise is a statement about the **shape**,
/// and three facts carry it, none of them a count:
///
/// - **every marked process is a `sleep`**, so every `exec` in the planted tree has already
///   happened and no member can drop out of a later read for being mid-`exec`;
/// - **the root is present and leads its own session**, pinned by the pid the fixture spawned
///   rather than inferred from the set;
/// - **exactly one other member leads a session of its own**, which is the escape this case
///   exists to measure and the thing `>= 2` never asked about.
///
/// Once all three hold nothing moves until `reap`: a `sleep 300` neither execs nor exits inside
/// a case that finishes in milliseconds, so the pids pinned here are the pids read back below.
fn planted_shape(mark: &str, root: u32) -> Result<Planted, Vec<Marked>> {
    let marked = marked_now(mark);
    let rooted = marked
        .iter()
        .any(|m| m.pid == root && m.session == root && m.comm == "sleep");
    let escapees: Vec<u32> = marked
        .iter()
        .filter(|m| m.pid != root && m.session == m.pid && m.comm == "sleep")
        .map(|m| m.pid)
        .collect();
    match (rooted, escapees.as_slice(), marked.len()) {
        (true, [escapee], 2) => Ok(Planted {
            root,
            escapee: *escapee,
        }),
        _ => Err(marked),
    }
}

/// Block until the planted tree reaches [`planted_shape`], and fail naming what it reached
/// instead.
///
/// The panic is the point of this helper, not a formality. A fixture whose processes never
/// arrive must say so in the words of the thing it failed to build, because the alternative is
/// a case that runs against a shape which cannot prove what its name claims and passes.
fn planted(mark: &str, root: u32) -> Planted {
    let started = Instant::now();
    let mut last = Vec::new();
    while started.elapsed() < Duration::from_secs(10) {
        match planted_shape(mark, root) {
            Ok(shape) => return shape,
            Err(seen) => last = seen,
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "the worker never settled into the shape this case is about: a sleeping root (pid \
         {root}, leading its own session) and exactly one sleeping descendant leading a session \
         of its own. What carried the mark after 10s: {last:?}. This is the fixture's premise \
         and not the discovery under test — with no escaped descendant there is nothing here \
         for a recorded pgid or session to miss, so finding it would prove nothing."
    );
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
    let mut root = worker_with_setsid_grandchild(&mark);
    let shape = planted(&mark, root.id());

    // The escape is real, asserted rather than assumed. A worker's recorded session and group
    // are the handles a cancel holds without the mark, so reconstructing them here says in one
    // place that they reach the worker and **not** its descendant. Both halves earn their
    // keep: the first stops the second from being the vacuous kind of negative that also
    // passes when the enumeration is broken and finds nothing at all.
    let by_recorded_ids = ProcessTree {
        pgid: Some(shape.root),
        sid: Some(shape.root),
        cgroup: None,
        mark: None,
    }
    .members();
    assert!(
        by_recorded_ids.contains(&shape.root),
        "premise: the worker's recorded session must reach the worker itself, or the assertion \
         below says nothing: {by_recorded_ids:?}"
    );
    assert!(
        !by_recorded_ids.contains(&shape.escapee),
        "premise: pid {} called setsid, so the worker's recorded session and group must not \
         reach it — if they do, this case is measuring a descendant that never escaped: \
         {by_recorded_ids:?}",
        shape.escapee
    );

    // And now the discovery under test, resolved from the session id alone — the way a cancel
    // reaches a worker it holds nothing but a row for.
    let tree = agent.worker_tree(&sid).await;
    let found = tree.members();
    assert!(
        found.contains(&shape.escapee),
        "the cancel must reach the descendant that called setsid: {} missing from {found:?} \
         (tree mark {:?})",
        shape.escapee,
        tree.mark
    );
    assert!(
        found.contains(&shape.root),
        "and the worker itself: {} missing from {found:?} (tree mark {:?})",
        shape.root,
        tree.mark
    );

    reap(&[shape.root, shape.escapee]);
    let _ = root.kill();
    let _ = root.wait();
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
        !sweep.evidence.proven(),
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
        !tree.kill().evidence.proven(),
        "so a cancel of one of these reports that it could not check, not that all is well"
    );
}
