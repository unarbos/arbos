//! A bound on how many finished-job workers stay resident
//! ([`crate::agent::Agent::retire_idle_workers`]).
//!
//! The defect these pin is a circle, not a leak. A job that finishes normally never calls `stop`,
//! and the only collector keys on [`crate::agent::Agent::session_is_over`], which asks every
//! adapter `is_open`, which answers "the child is in my map and alive" — so the worker was spared
//! because it was alive and stayed alive because nothing stopped it. Forest held 25 live `pi:`
//! workers at 3505 MiB against the 3 GB budget `Agent::pi_parked`'s comment names; Test5 on
//! 0.1.198 held 35 after 11.5 hours.
//!
//! ## What a false pass would look like here, and what rules it out
//!
//! The dangerous shape is a case that goes green because the child was never alive, or because
//! the pass evicts indiscriminately. So:
//!
//! * every child comes from [`RpcChild::spawn`], production's own call, which sets `ARBOS_WORKER`
//!   from [`crate::harness::worker_name`] and makes the child a session leader. Nothing is
//!   planted; if production stopped marking its children these cases would not notice, because
//!   they do not read the mark at all — they read the pid.
//! * liveness is asserted **before** the pass runs, by pid in `/proc`, with the zombie rule the
//!   product uses. A child that was dead on arrival cannot pass for a retirement.
//! * the negatives carry the weight. Five cases require a child to **survive** — a young clock, a
//!   run in flight, an open job, a contended map, and the survivors of the population cap. A pass
//!   that simply closed everything would fail all five, so "it evicted something" is never enough
//!   on its own.
//! * the cap case names which sessions must live, not only how many, so trimming the wrong ones
//!   is a failure rather than a pass.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, MutexGuard};

use arbos_core::ids::ulid;
use arbos_core::route::Harness;

use crate::agent::Agent;
use crate::rpc_child::RpcChild;
use crate::status::tests::test_agent;

/// `retire_idle_workers` reads every adapter's map and the store, both of which are this agent's
/// own — but the cases also read `/proc` by pid, and one of them holds an adapter lock. Serialised
/// so a sibling case's child cannot be mistaken for this one's.
static ONE_AT_A_TIME: Mutex<()> = Mutex::const_new(());

async fn exclusive() -> MutexGuard<'static, ()> {
    ONE_AT_A_TIME.lock().await
}

/// Alive as `isolation.rs` counts it: a killed child stays in `/proc` as a zombie until it is
/// waited on, so the directory existing is not liveness. The product's rule, not a second one.
fn is_running(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some(after) = stat.rsplit(')').next() else {
        return false;
    };
    !matches!(after.split_whitespace().next(), Some("Z") | Some("X"))
}

async fn wait_until_gone(pid: u32) {
    for _ in 0..100 {
        if !is_running(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A worker child of the shape the adapters hold: spawned by production's `RpcChild::spawn`, under
/// production's worker name, sitting in `read` on its stdin exactly as a real idle worker does.
///
/// `cat` rather than a sleep: it keeps stdin open, so `RpcChild::shutdown`'s close-then-wait is
/// the thing that ends it, which is the production shutdown this pass relies on.
async fn a_retained_worker(agent: &Arc<Agent>, root: &std::path::Path, session_id: &str) -> u32 {
    let child = RpcChild::spawn(
        &crate::harness::worker_name("pi", session_id),
        "cat",
        &[],
        &[],
        root,
        &root.join(format!("worker-{session_id}.log")),
        None,
    )
    .await
    .expect("the worker child spawns");
    let pid = child.pid().expect("a spawned child has a pid");
    agent
        .store
        .insert_subagent(session_id, "worker", "do the thing", "pi", CHAT)
        .unwrap();
    agent.store.set_subagent_status(session_id, "idle").unwrap();
    agent
        .adapter(Harness::Pi)
        .test_attach_child(session_id, child)
        .await;
    pid
}

/// Age one retained child's idle clock past the bound without waiting ten minutes.
///
/// It moves the clock and nothing else — the process stays as it is — so the case still closes a
/// real child through the real path, and the comparison in `retire_idle_workers` is the code under
/// test rather than a test-only bound.
async fn age_clock(agent: &Arc<Agent>, session_id: &str, ms: u64) {
    agent
        .adapter(Harness::Pi)
        .test_age_idle_clock(session_id, ms)
        .await
        .expect("the child is in the map");
}

const CHAT: &str = "01TESTCHATWORKERRETENTION1";

fn past_the_bound() -> u64 {
    Agent::WORKER_RETIRE_IDLE_MS + 60_000
}

/// An idle worker past the bound gives its process back.
///
/// The case the whole change exists for. Red before it: nothing removed the child, so the pid
/// stayed in `/proc` for as long as the box ran.
#[tokio::test]
async fn an_idle_worker_past_the_bound_is_retired() {
    let _one = exclusive().await;
    let (agent, root) = test_agent().await;
    let sid = ulid();
    let pid = a_retained_worker(&agent, &root, &sid).await;

    // The premise, before anything is asserted about the pass: the child is really alive and the
    // adapter really holds it.
    assert!(is_running(pid), "the worker child is alive to begin with");
    assert_eq!(
        agent.adapter(Harness::Pi).is_open(&sid),
        Some(true),
        "and the adapter holds it, which is what keeps `session_is_over` answering `Some(false)`"
    );

    age_clock(&agent, &sid, past_the_bound()).await;
    assert_eq!(agent.retire_idle_workers().await, 1, "one child is closed");

    wait_until_gone(pid).await;
    assert!(
        !is_running(pid),
        "an idle worker past the bound must give its process back; pid {pid} is still alive"
    );
    assert_eq!(
        agent.adapter(Harness::Pi).is_open(&sid),
        Some(false),
        "and the session must read closed, which is what lets the existing reap reach anything it \
         left behind"
    );
}

/// A worker whose clock is young is kept.
///
/// The negative the age bound rests on. Without it the case above passes just as well for a pass
/// that closes every child it can see, which would take workers seconds after their jobs finish
/// and make every follow-up brief pay a resume.
#[tokio::test]
async fn a_recently_used_worker_is_kept() {
    let _one = exclusive().await;
    let (agent, root) = test_agent().await;
    let sid = ulid();
    let pid = a_retained_worker(&agent, &root, &sid).await;

    // Deliberately just under, not zero: the comparison is `>=`, so a case at zero would also
    // pass against an off-by-one that used `>`.
    age_clock(&agent, &sid, Agent::WORKER_RETIRE_IDLE_MS - 1_000).await;

    assert_eq!(
        agent.retire_idle_workers().await,
        0,
        "a worker used a moment ago is not idle and must be kept"
    );
    assert!(is_running(pid), "so its process is still there");
    assert_eq!(agent.adapter(Harness::Pi).is_open(&sid), Some(true));
}

/// A worker with a run in flight is kept, however old its clock reads.
///
/// Gate 1, and the one that protects a worker mid-job. `subagent_msgs` is written before the start
/// or resume and cleared after the run, so it is the live set; an old clock on top of it is the
/// shape a long job produces when its one prompt was ten minutes ago.
#[tokio::test]
async fn a_worker_with_a_run_in_flight_is_kept() {
    let _one = exclusive().await;
    let (agent, root) = test_agent().await;
    let sid = ulid();
    let pid = a_retained_worker(&agent, &root, &sid).await;
    age_clock(&agent, &sid, past_the_bound()).await;

    agent
        .subagent_msgs
        .lock()
        .await
        .insert(sid.clone(), (CHAT.to_owned(), ulid()));

    assert_eq!(
        agent.retire_idle_workers().await,
        0,
        "a session with a run in flight must be kept whatever its clock says"
    );
    assert!(is_running(pid), "so its process is still there");
}

/// A worker an open job names is kept.
///
/// Gate 3, and it reads `job_worker_ids` rather than the row's current `subagent_id` for bug AO's
/// reason: a job keeps one row across several workers, so the column names only the newest.
#[tokio::test]
async fn a_worker_an_open_job_names_is_kept() {
    let _one = exclusive().await;
    let (agent, root) = test_agent().await;
    let sid = ulid();
    let pid = a_retained_worker(&agent, &root, &sid).await;
    age_clock(&agent, &sid, past_the_bound()).await;

    let job = ulid();
    agent
        .store
        .insert_job(&job, CHAT, &ulid(), "A job", "ops", "work", &[])
        .unwrap();
    agent.store.set_job_subagent(&job, &sid).unwrap();
    // A second worker takes over the same row, so the row's own column stops naming the first.
    let newer = ulid();
    agent
        .store
        .insert_subagent(&newer, "worker", "work", "pi", CHAT)
        .unwrap();
    agent.store.set_job_subagent(&job, &newer).unwrap();
    agent.store.set_job_state(&job, "running", None).unwrap();
    assert_ne!(
        agent.store.job(&job).unwrap().unwrap().subagent_id.unwrap(),
        sid,
        "the premise of this case: the row no longer names the worker under test, so a gate that \
         read the column alone would not see it"
    );

    assert_eq!(
        agent.retire_idle_workers().await,
        0,
        "a worker an open job has had must be kept"
    );
    assert!(is_running(pid), "so its process is still there");
}

/// The population cap takes the longest-idle first, stops at the cap, and leaves the rest.
///
/// This is the bound that answers the measurement: Forest reached 25 live workers against a
/// concurrency gate of 4, so a cap keyed on age alone would not have held. The survivors are named
/// rather than counted — a pass that trimmed to the right number by taking the wrong children
/// would otherwise go green.
#[tokio::test]
async fn the_population_cap_retires_the_longest_idle_first() {
    let _one = exclusive().await;
    let (agent, root) = test_agent().await;
    let over = 3;
    let total = Agent::WORKER_RETAINED_MAX + over;

    // Every clock is well under the age bound, so the age bound cannot be what acts here.
    let mut sessions: Vec<(String, u32)> = Vec::new();
    for i in 0..total {
        let sid = ulid();
        let pid = a_retained_worker(&agent, &root, &sid).await;
        // The oldest clock first, each a second apart and all far short of the bound.
        age_clock(&agent, &sid, (total - i) as u64 * 1_000).await;
        sessions.push((sid, pid));
    }

    assert_eq!(
        agent.retire_idle_workers().await,
        over,
        "exactly the excess over the cap is retired, not everything and not nothing"
    );

    let (going, staying) = sessions.split_at(over);
    for (sid, pid) in going {
        wait_until_gone(*pid).await;
        assert!(
            !is_running(*pid),
            "the longest-idle children go first; {sid} (pid {pid}) is still alive"
        );
    }
    for (sid, pid) in staying {
        assert!(
            is_running(*pid),
            "and the cap stops at the cap: {sid} (pid {pid}) was taken and should have been kept"
        );
        assert_eq!(
            agent.adapter(Harness::Pi).is_open(sid),
            Some(true),
            "{sid} must still be held"
        );
    }
}

/// `waiting_user` is retired, and it is retired **last**.
///
/// The user-visible half of this change, pinned rather than left to the ranking comment. A session
/// a person is expected to reply to can lose its child, and their reply then comes back through a
/// resume. It is included because excluding it would leave the cap unbounded; it is ranked last so
/// a finished worker is always taken before it.
///
/// Both children here are young, so only the cap can act — and with exactly one eviction to make,
/// which of the two goes is the whole assertion.
#[tokio::test]
async fn waiting_user_is_retired_only_after_an_idle_worker() {
    let _one = exclusive().await;
    let (agent, root) = test_agent().await;

    // Fill the cap to one short, with children younger than both of the two below. The two below
    // then bring the set one over, so the pass has exactly one eviction to make and the two
    // candidates below are the oldest two.
    let mut filler = Vec::new();
    for _ in 0..Agent::WORKER_RETAINED_MAX - 1 {
        let sid = ulid();
        a_retained_worker(&agent, &root, &sid).await;
        age_clock(&agent, &sid, 1_000).await;
        filler.push(sid);
    }

    // The `waiting_user` one is the *older* of the two, so age alone would take it first. Only the
    // rank puts the finished worker ahead of it.
    let waiting = ulid();
    let waiting_pid = a_retained_worker(&agent, &root, &waiting).await;
    agent
        .store
        .set_subagent_status(&waiting, "waiting_user")
        .unwrap();
    age_clock(&agent, &waiting, 300_000).await;

    let finished = ulid();
    let finished_pid = a_retained_worker(&agent, &root, &finished).await;
    age_clock(&agent, &finished, 120_000).await;

    assert_eq!(
        agent.retire_idle_workers().await,
        1,
        "one over the cap means one eviction"
    );

    wait_until_gone(finished_pid).await;
    assert!(
        !is_running(finished_pid),
        "the finished worker is the one taken, though it is the younger of the two"
    );
    assert!(
        is_running(waiting_pid),
        "and a session waiting on a person's reply is the last to go"
    );

    // And it *is* reachable, rather than exempt. Put every other child in a state the pass keeps,
    // so `waiting_user` is the only candidate, and go one over the cap again. A design that
    // excluded it would have nothing to take and leave the set over the cap for good — which is
    // the unbounded cap this change exists to close.
    for sid in &filler {
        agent.store.set_subagent_status(sid, "running").unwrap();
    }
    let extra = ulid();
    a_retained_worker(&agent, &root, &extra).await;
    agent.store.set_subagent_status(&extra, "running").unwrap();
    age_clock(&agent, &extra, 1_000).await;
    assert_eq!(agent.retire_idle_workers().await, 1);
    wait_until_gone(waiting_pid).await;
    assert!(
        !is_running(waiting_pid),
        "`waiting_user` is ranked last, not excluded — excluding it would leave the cap unbounded"
    );
}

/// An adapter whose map is held retires nothing.
///
/// The three-state discipline `is_open` is written for, pointed at this pass: a read that could
/// not look knows nothing about any session, and the only thing it may do with that is close
/// nothing. Without it a sibling session's `stop` — which holds the map across a five-second
/// shutdown — would make every other worker on the box look absent and therefore collectable.
#[tokio::test]
async fn a_contended_children_map_retires_nothing() {
    let _one = exclusive().await;
    let (agent, root) = test_agent().await;
    let sid = ulid();
    let pid = a_retained_worker(&agent, &root, &sid).await;
    age_clock(&agent, &sid, past_the_bound()).await;

    // Held on another task, with a ready signal, so the pass below provably runs while the map is
    // taken rather than racing it.
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let adapter = agent.adapter(Harness::Pi);
    let holding =
        tokio::spawn(async move { adapter.test_hold_children_until(ready_tx, release_rx).await });
    ready_rx.await.expect("the map is held");

    let retired = agent.retire_idle_workers().await;
    let _ = release_tx.send(());
    holding.await.unwrap();

    assert_eq!(
        retired, 0,
        "a pass that could not read the map must close nothing"
    );
    assert!(is_running(pid), "so the worker is still there");

    // And the lock being free again is what makes the zero above a statement about contention
    // rather than about this child being ineligible.
    assert_eq!(agent.retire_idle_workers().await, 1);
    wait_until_gone(pid).await;
    assert!(!is_running(pid));
}

/// Retiring leaves everything a resume reads: the pack on disk, the store row, and `is_open`
/// false.
///
/// These are the three inputs `run_subagent`'s resume branch takes, and the case is here as a
/// cheap guard on all of them at once. **It is not proof that resume works** — that is
/// [`a_retired_worker_resumes_its_thread_end_to_end`], which drives the real path through a pi
/// that speaks the protocol.
#[tokio::test]
async fn retiring_leaves_what_a_resume_reads() {
    let _one = exclusive().await;
    let (agent, root) = test_agent().await;
    let sid = ulid();
    let pid = a_retained_worker(&agent, &root, &sid).await;

    let pack = agent.cfg.state_dir.join("packs").join(format!("{sid}.md"));
    std::fs::create_dir_all(pack.parent().unwrap()).unwrap();
    std::fs::write(&pack, "the thread's pack").unwrap();

    age_clock(&agent, &sid, past_the_bound()).await;
    assert_eq!(agent.retire_idle_workers().await, 1);
    wait_until_gone(pid).await;

    assert_eq!(
        std::fs::read_to_string(&pack).unwrap(),
        "the thread's pack",
        "the pack a resume re-reads must survive the retirement"
    );
    assert!(
        agent.store.subagent(&sid).unwrap().is_some(),
        "and so must the store row the resume looks the harness up in"
    );
    assert_eq!(
        agent.adapter(Harness::Pi).is_open(&sid),
        Some(false),
        "and `is_open` must read false, which is the one condition `run_subagent` resumes on"
    );
}

// ---- the resume round trip ----------------------------------------------------------------------

/// A pi that speaks enough of the protocol to be started, resumed and prompted, and that records
/// what it was given.
///
/// Two files, both append-only, both read by the case rather than inferred:
///
/// * `@ARGV@` takes one JSON array per spawn, so the case can tell a **start** (`--session-id`)
///   from a **resume** (`--session`) by exact token rather than by substring — `--session-id`
///   contains `--session`, and a case that matched on text would read every start as a resume.
/// * `@PROMPTS@` takes one JSON string per prompt. This is the decisive record: `run_subagent`
///   words its first prompt differently for a fresh start, a successful resume, and a resume that
///   failed and fell back to a fresh session. The words therefore say which branch ran.
const RESUME_PI: &str = r#"#!/usr/bin/env python3
import json, os, sys

with open("@ARGV@", "a") as f:
    f.write(json.dumps(sys.argv[1:]) + "\n")

def out(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

worker = os.environ.get("ARBOS_SESSION_ROLE") == "subagent"

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    c = json.loads(line)
    kind, cid = c.get("type"), c.get("id")
    if kind == "get_state":
        out({"type": "response", "id": cid, "command": kind, "success": True,
             "data": {"isStreaming": False, "sessionFile": "@SESSIONS@/s.jsonl"}})
        continue
    if kind != "prompt":
        out({"type": "response", "id": cid, "command": kind, "success": True, "data": None})
        continue
    if worker:
        with open("@PROMPTS@", "a") as f:
            f.write(json.dumps(c.get("message", "")) + "\n")
    out({"type": "response", "id": cid, "command": kind, "success": True, "data": None})
    if worker:
        out({"type": "message_update",
             "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": "done"}})
        out({"type": "agent_settled", "aborted": False})
"#;

struct ResumeFixture {
    root: std::path::PathBuf,
    argv: std::path::PathBuf,
    prompts: std::path::PathBuf,
    pi_bin: std::path::PathBuf,
}

impl ResumeFixture {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("arbos-worker-resume-{}", ulid()));
        std::fs::create_dir_all(&root).unwrap();
        let (argv, prompts) = (root.join("argv.jsonl"), root.join("prompts.jsonl"));
        let pi_bin = root.join("resume-pi.py");
        std::fs::write(
            &pi_bin,
            RESUME_PI
                .replace("@ARGV@", &argv.display().to_string())
                .replace("@PROMPTS@", &prompts.display().to_string())
                .replace("@SESSIONS@", &root.display().to_string()),
        )
        .unwrap();
        std::fs::set_permissions(&pi_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            root,
            argv,
            prompts,
            pi_bin,
        }
    }

    fn lines(path: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Every spawn's argv, as exact token lists.
    fn spawns(&self) -> Vec<Vec<String>> {
        Self::lines(&self.argv)
            .iter()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    /// Spawns that carry `flag` with `value` as adjacent, exactly matching tokens.
    fn spawns_with(&self, flag: &str, value: &str) -> usize {
        self.spawns()
            .iter()
            .filter(|argv| argv.windows(2).any(|w| w[0] == flag && w[1] == value))
            .count()
    }

    fn prompts(&self) -> Vec<String> {
        Self::lines(&self.prompts)
            .iter()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }
}

impl Drop for ResumeFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

async fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    for _ in 0..1200 {
        if ready() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{what} did not happen within 60s");
}

/// **A retired worker's thread really does resume.** The end-to-end case, and the one the safety
/// argument for this whole change rests on.
///
/// Nothing is planted and nothing is asserted about preconditions. `run_subagent` mints the
/// session id and calls `PiAdapter::start`, which spawns a real child. The run completes. The
/// retention pass closes that child. Then the **same** subagent id is given a second brief through
/// `run_subagent` again, which is the production reuse path — and the proof that the resume
/// branch ran, rather than a fresh start wearing its clothes, is read two independent ways:
///
/// 1. the second spawn's argv carries `--session <id>` and no spawn ever carries `--session-id
///    <id>` twice, so the harness was told to reopen the thread rather than to begin one;
/// 2. the second prompt's wording is `run_subagent`'s successful-resume wording. That line at
///    `agent.rs:12115` is reachable only with `is_new` false **and** `resumed_fresh` false, so a
///    resume that had failed and fallen back would have written the "could not be resumed" text
///    instead. This is the assertion a passing-for-the-wrong-reason run cannot survive.
#[tokio::test]
async fn a_retired_worker_resumes_its_thread_end_to_end() {
    let _one = exclusive().await;
    let fx = ResumeFixture::new();
    let (agent, _root) =
        crate::status::tests::test_agent_with_pi(&fx.pi_bin.display().to_string()).await;
    agent
        .start_pi(CHAT)
        .await
        .expect("the main pi child starts");
    let host = agent.sync.open_room(CHAT).await.unwrap();
    let ask = ulid();
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(
                &ask,
                "look at the thing",
                "dev-mac",
                &arbos_core::doc::UserMessageOpts::default(),
            )
            .unwrap();
    }

    // First run: a fresh worker, through the ordinary path.
    agent
        .run_subagent(
            CHAT,
            &host,
            &ask,
            None,
            "scout",
            "look at the thing",
            "first brief",
            Some(Harness::Pi),
            false,
            false,
            None,
        )
        .await
        .expect("the first run completes");

    // The worker the agent minted, learned from the store rather than chosen by the test.
    let sid = {
        let subs = agent.store.subagents().unwrap();
        assert_eq!(subs.len(), 1, "exactly one worker was started: {subs:?}");
        subs[0].id.clone()
    };
    assert_eq!(
        fx.spawns_with("--session-id", &sid),
        1,
        "the first run is a fresh start, which is what makes the second one a resume: {:?}",
        fx.spawns()
    );
    assert_eq!(
        agent.store.subagent(&sid).unwrap().unwrap().status,
        "idle",
        "the run ended normally, which is the state nothing used to collect"
    );
    assert_eq!(
        agent.adapter(Harness::Pi).is_open(&sid),
        Some(true),
        "and its child is retained — the defect, reproduced through the production path"
    );

    // Retire it.
    age_clock(&agent, &sid, past_the_bound()).await;
    assert_eq!(
        agent.retire_idle_workers().await,
        1,
        "the finished worker is retired"
    );
    assert_eq!(agent.adapter(Harness::Pi).is_open(&sid), Some(false));

    // Second run on the same thread: the reuse path, as the main agent drives it.
    agent
        .run_subagent(
            CHAT,
            &host,
            &ask,
            Some(sid.clone()),
            "scout",
            "look at the thing",
            "second brief",
            Some(Harness::Pi),
            false,
            false,
            None,
        )
        .await
        .expect("the second run completes on the resumed thread");

    wait_for("the resumed worker is prompted", || fx.prompts().len() >= 2).await;

    assert_eq!(
        fx.spawns_with("--session", &sid),
        1,
        "the second run must reopen the thread with `--session <id>`: {:?}",
        fx.spawns()
    );
    assert_eq!(
        fx.spawns_with("--session-id", &sid),
        1,
        "and must not have started a second fresh session for it: {:?}",
        fx.spawns()
    );

    let prompts = fx.prompts();
    assert!(
        prompts[0].contains("first brief"),
        "the fresh worker got the brief itself: {prompts:?}"
    );
    assert!(
        prompts[1].starts_with("Context update: continue your thread."),
        "the resumed worker must get the continue-your-thread wording, which `run_subagent` \
         produces only when the resume succeeded: {prompts:?}"
    );
    assert!(
        !prompts[1].contains("could not be resumed"),
        "a resume that had failed and fallen back to a fresh session would say so here, and this \
         is the assertion that keeps the case from passing for the wrong reason: {prompts:?}"
    );
}
