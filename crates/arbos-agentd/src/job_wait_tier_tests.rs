//! Bug AL: capped work was done as unmanaged background processes, so no stop could reach it.
//!
//! Test5 round 22, `r22status`. Four `start_job` calls were refused on the per-chat cap with
//! `Job not started (6 jobs already run in this chat (cap 3))`. The agent then did the work by
//! other means: three as inline `bash` (PIDs 1855, 1875, 1893, 1909) and one as a once-routine.
//! The three inline ones had never been jobs, so nothing named them — the user's stop took the
//! four real jobs, `b`, `d`, `e` and `f`, which went flat across both 60-second snapshots, and
//! left `h` (165 to 264 bytes), `i` (`nohup`, 360 to 540) and `j` (`setsid`, 468 to 702) growing.
//! The agent said it had no way to cancel them, which was true.
//!
//! So the defect is the refusal, not the cancel. `job_cancel_window_tests` holds the other half
//! of that: the two windows AL was first read as living in are both closed on `main`.
//!
//! # What changed
//!
//! Over the cap a job now joins a **waiting tier** instead of being refused. It gets a row, and a
//! row is visible, cancellable, and started in order when capacity frees. The tier is bounded —
//! 12 waiting per chat, 24 per agent — and at its limit there is a refusal again, which is the
//! one place where refusing is honest, because no amount of waiting will help there.
//!
//! **Nothing about how many jobs run at once changes.** That is `SUBAGENT_MAX_CONCURRENT`, which
//! this does not touch. The per-chat cap was never a concurrency limit in the first place: it
//! only ever refused jobs the *model* asked for, and a job the user asked for in words went past
//! it (bug U), so a single chat could already occupy every slot.
//!
//! # Why better wording was not an option
//!
//! The refusal already said "fold this into one of them or wait for one to finish, and tell the
//! user". That instruction was ignored twice in one round, in two different ways, by a model that
//! had just been given it. A fix that rests on saying it more firmly has already been tried.

use arbos_core::doc::UserMessageOpts;
use arbos_core::ids::ulid;

use crate::status::tests::test_agent;
use crate::sync::SharedHost;

const CHAT: &str = "01TESTCHATJOBWAITTIER0001";
const ASK: &str = "01TESTASKJOBWAITTIER00001";

/// An agent with every worker slot held, so nothing an admitted job does can start a real worker
/// and the rows below cannot move under the assertions.
///
/// It means a job here would report `queued` even with the caps wide open, so the cases that are
/// about the cap check the capacity note rather than the state alone. The slots are taken before
/// any job exists, so there is no window to slip through.
async fn a_box() -> (
    std::sync::Arc<crate::agent::Agent>,
    SharedHost,
    tokio::sync::OwnedSemaphorePermit,
) {
    let (agent, _root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(
                ASK,
                "run these as background jobs",
                "dev-mac",
                &UserMessageOpts::default(),
            )
            .unwrap();
    }
    let held = agent
        .subagent_slots
        .clone()
        .acquire_many_owned(agent.subagent_slots.available_permits() as u32)
        .await
        .expect("the slots are open");
    assert_eq!(agent.subagent_slots.available_permits(), 0);
    (agent, host, held)
}

/// `n` jobs already open in `chat`, planted straight into the store so no worker is involved.
fn plant_open_jobs(agent: &crate::agent::Agent, chat: &str, n: usize, state: &str) {
    for i in 0..n {
        let id = ulid();
        agent
            .store
            .insert_job(
                &id,
                chat,
                &format!("{ASK}{i}"),
                &format!("Planted job {i}"),
                "ops",
                "do the planted thing",
                &[],
            )
            .unwrap();
        if state != "queued" {
            agent.store.set_job_state(&id, state, None).unwrap();
        }
    }
}

async fn enqueue(
    agent: &std::sync::Arc<crate::agent::Agent>,
    host: &SharedHost,
    title: &str,
) -> Result<serde_json::Value, String> {
    agent
        .start_job(
            CHAT,
            host,
            ASK,
            title,
            "Count the licence lines and report the number.",
            "ops",
            vec![],
            false,
            &mut None,
        )
        .await
}

/// **The behavioural case.** A job over the per-chat cap is enqueued, not refused.
///
/// Red on today's `main` with round 22's own sentence: `3 jobs already run in this chat (cap 3);
/// fold this into one of them or wait for one to finish, and tell the user`. That refusal is
/// where the agent went and launched the work as inline `bash` instead.
#[tokio::test]
async fn a_job_over_the_chat_cap_waits_instead_of_being_refused() {
    let (agent, host, _held) = a_box().await;
    let cap = crate::config::job_max_per_chat();
    plant_open_jobs(&agent, CHAT, cap, "running");

    let out = enqueue(&agent, &host, "Counting the licence lines")
        .await
        .expect("over the cap, a job waits for capacity rather than being refused");

    let job_id = out["job_id"].as_str().expect("an enqueued job has an id");
    assert_eq!(
        out["state"], "queued",
        "and it must say so, so the model is not told work has begun: {out}"
    );
    // Named, because `queued` alone would also be what a job with no free worker slot says, and
    // the slots `a_box` holds mean that is true here too. This note is only written on the
    // capacity branch, so it is the part that distinguishes waiting for the cap from waiting
    // for a slot — and the sentence it carries is the one that has to keep the work in the job
    // system.
    let note = out["note"].as_str().unwrap_or_default();
    assert!(
        note.contains("queued for capacity"),
        "the reply has to say which wait this is: {out}"
    );
    assert!(
        note.contains("do not do the work yourself instead")
            && note.contains("do not schedule anything to start it"),
        "with round 22's two exits closed, since this reply replaces the refusal that took them: {out}"
    );
    let row = agent.store.job(job_id).unwrap().expect("the row exists");
    assert_eq!(
        row.state, "queued",
        "the row is the thing a stop reads, and it has to exist for the stop to reach the work"
    );
    assert!(
        row.started_at.is_none(),
        "a waiting job holds no running permit and has not begun"
    );
}

/// **Waiting means waiting.** The admitted job does not run while the cap is full.
///
/// This is the invariant the change is only acceptable with: the cap decides when a job waits,
/// never how many run at once, so admitting a job over it must not put a fourth worker to work.
/// Every worker slot is deliberately *free* here, so the cap is the only thing that can be
/// holding the job — and the wait is long enough for several of the gate's own poll intervals,
/// so a gate that merely slowed the start down would be caught.
#[tokio::test]
async fn an_admitted_job_does_not_run_while_the_cap_is_full() {
    let (agent, _root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    assert!(
        agent.subagent_slots.available_permits() > 0,
        "the slots must be free, or this case proves nothing about the cap"
    );
    let cap = crate::config::job_max_per_chat();
    plant_open_jobs(&agent, CHAT, cap, "running");

    let out = enqueue(&agent, &host, "Counting the licence lines")
        .await
        .expect("the job is admitted");
    let job_id = out["job_id"].as_str().unwrap().to_owned();

    tokio::time::sleep(std::time::Duration::from_millis(3_500)).await;
    let row = agent.store.job(&job_id).unwrap().expect("the row exists");
    assert_eq!(
        row.state, "queued",
        "a job admitted over the cap started anyway, which would make the cap mean nothing"
    );
    assert_eq!(
        agent
            .store
            .jobs_for_chat(CHAT)
            .unwrap()
            .iter()
            .filter(|j| j.state == "running")
            .count(),
        cap,
        "and the number running stayed at the cap"
    );
    // Also the proof that the gate is what held the job: `queued` is where a worker that failed
    // to start leaves the row too, so without this the case above would pass for the wrong
    // reason. The error and restart count are in the message because that is the difference, and
    // it is the first thing worth knowing when this breaks.
    assert_eq!(
        row.last_step, "Waiting for capacity",
        "the job is queued but not waiting on capacity, so something else stopped it \
         (error {:?}, restarts {})",
        row.error, row.restarts
    );
}

/// Waiting jobs are released in the order they were asked for, one per freed place.
///
/// The gate is asked directly here rather than through a worker, because what is being checked is
/// which job it lets go and which it holds — and a worker would add a pi spawn, a retry and a
/// race to a question that has an exact answer.
///
/// Two places are taken by running jobs and the cap is three, so there is room for exactly one of
/// the two waiting jobs. The older one goes. The younger one is held even though a place is free,
/// which is the whole of "in order": a job that arrived second does not overtake the one that
/// arrived first just because the clock reached it at the right moment.
#[tokio::test]
async fn waiting_jobs_are_released_oldest_first() {
    let (agent, _root) = test_agent().await;
    let cap = crate::config::job_max_per_chat();
    assert!(cap >= 2, "this case needs room for one of the two waiters");
    plant_open_jobs(&agent, CHAT, cap - 1, "running");

    let mut waiting = Vec::new();
    for i in 0..2 {
        let id = ulid();
        agent
            .store
            .insert_job(
                &id,
                CHAT,
                &format!("{ASK}w{i}"),
                &format!("Waiting job {i}"),
                "ops",
                "count the licence lines",
                &[],
            )
            .unwrap();
        waiting.push(id);
        // Distinct milliseconds, so the order is the one the asks arrived in and not the id
        // tie-break. The tie-break is only there so two jobs of the same millisecond still get
        // some order; it is not what this case is about.
        tokio::time::sleep(std::time::Duration::from_millis(3)).await;
    }

    let released = |job: String| {
        let agent = agent.clone();
        async move {
            tokio::time::timeout(
                std::time::Duration::from_millis(2_500),
                agent.await_job_capacity(CHAT, &job),
            )
            .await
            .is_ok()
        }
    };
    assert!(
        released(waiting[0].clone()).await,
        "the older waiting job takes the one free place"
    );
    assert!(
        !released(waiting[1].clone()).await,
        "and the younger one waits, rather than overtaking it into the same place"
    );
}

/// A job whose worker failed and was sent back to `queued` re-enters the gate and keeps its own
/// place, behind a job that has been waiting longer. **A retry does not outrank a longer wait.**
///
/// Ruled in by the coordinator as a decision rather than left as something the code happens to
/// do. The retry path hands the fresh worker the *same job row* with `attempt + 1`, so that
/// worker runs [`Agent::run_subagent`] again and meets the capacity gate again — and the row it
/// carries still holds its original `created_at`, because `set_job_state` does not touch that
/// column. The place a requeued job takes is therefore the place it was asked for, not the place
/// it was retried in. Nothing here is a special case for retries; the gate reads the row, and the
/// row does not record why it went back.
///
/// # On the refusal wording
///
/// This is deliberately *not* a model refusal, because after #2921 a refusal does not requeue at
/// all: `run_subagent` marks a permanently-failed worker's job `failed` and sends the work to the
/// inline fallback, since the same bytes would be declined again. `#2921`'s own
/// `a_refused_worker_fails_with_the_providers_reason_and_is_not_retried` pins that, asserting
/// `failed` and `restarts == 0`. What does come back to `queued` is a *retryable* failure — a
/// failed start, a crashed worker, a transient model error on an empty run — and that is what is
/// driven here, through the real path, by a worker that cannot spawn.
///
/// # Why this is not flaky
///
/// The worker fails at its spawn, which is certain: the test agent's pi path does not exist. The
/// requeue is written immediately after that failure, and only *then* does the retry branch wait
/// up to `Agent::PI_RESPAWN_WAIT` — 45 seconds — for a live main pi child that never arrives.
/// Every assertion below happens inside that wait, in well under a second, so the fresh worker
/// has not reached the gate and cannot move the row under the case.
#[tokio::test]
async fn a_requeued_job_does_not_outrank_a_job_that_waited_longer() {
    let (agent, _root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    let cap = crate::config::job_max_per_chat();
    assert!(cap >= 2, "this case needs room for one of the two waiters");

    // The longer wait: a queued row, asked for first, which no worker is started for.
    let waited_longer = ulid();
    agent
        .store
        .insert_job(
            &waited_longer,
            CHAT,
            &format!("{ASK}older"),
            "Counting the test files",
            "ops",
            "count the test files",
            &[],
        )
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(3)).await;

    // The retried job, asked for second. There is capacity for it, so its first worker is
    // released by the gate at once — and then fails to spawn, which is what sends it back.
    let retried = enqueue(&agent, &host, "Counting the licence lines")
        .await
        .expect("there is capacity for this one")["job_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let mut row = agent.store.job(&retried).unwrap().unwrap();
    for _ in 0..200 {
        if row.state == "queued" && row.restarts >= 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        row = agent.store.job(&retried).unwrap().unwrap();
    }
    assert!(
        row.state == "queued" && row.restarts >= 1,
        "the retry path never sent this job back, so there is nothing to check a place for \
         (state {:?}, restarts {}, error {:?})",
        row.state,
        row.restarts,
        row.error
    );
    let older = agent.store.job(&waited_longer).unwrap().unwrap();
    assert!(
        older.created_at < row.created_at,
        "the longer-waiting job must be the older one, or this case proves nothing"
    );

    // One place, two waiters.
    plant_open_jobs(&agent, CHAT, cap - 1, "running");
    let released = |job: String| {
        let agent = agent.clone();
        async move {
            tokio::time::timeout(
                std::time::Duration::from_millis(2_500),
                agent.await_job_capacity(CHAT, &job),
            )
            .await
            .is_ok()
        }
    };
    assert!(
        released(waited_longer.clone()).await,
        "the job that has waited longer takes the free place"
    );
    assert!(
        !released(retried.clone()).await,
        "and the retried job waits its turn rather than taking the place with its second worker"
    );
}

/// The per-agent half of the same thing, which is a separate branch and had a separate refusal.
#[tokio::test]
async fn a_job_over_the_agent_cap_waits_instead_of_being_refused() {
    let (agent, host, _held) = a_box().await;
    // Spread across other chats, so this chat is nowhere near its own cap and the per-agent
    // branch is the only one that can refuse.
    let cap = crate::config::job_max_per_agent();
    for i in 0..cap {
        plant_open_jobs(
            &agent,
            &format!("01TESTOTHERCHATWAITTIER{i:02}"),
            1,
            "running",
        );
    }
    assert!(agent.store.open_jobs().unwrap().len() >= cap);

    let out = enqueue(&agent, &host, "Counting the licence lines")
        .await
        .expect("over the per-agent cap, a job waits rather than being refused");
    assert_eq!(out["state"], "queued", "{out}");
}

/// At the top of the waiting tier the refusal comes back — and this is the one place it belongs,
/// because a job added here would wait behind twelve others with no end in sight.
///
/// The assertions are about what the refusal *is*: a capacity statement that started nothing, and
/// one that names the two exits round 22 took so neither reads as available.
#[tokio::test]
async fn a_full_waiting_tier_is_refused_honestly_and_starts_nothing() {
    let (agent, host, _held) = a_box().await;
    let wait = crate::config::job_wait_max_per_chat();
    plant_open_jobs(&agent, CHAT, wait, "queued");

    let rows_before = agent.store.jobs_for_chat(CHAT).unwrap().len();
    let msgs_before = host.lock().await.doc.messages().len();

    let err = enqueue(&agent, &host, "Counting the licence lines")
        .await
        .expect_err("a full waiting tier is the one honest refusal");

    assert!(
        err.contains("not started") && err.contains("waiting for capacity"),
        "the refusal has to say what it is — a capacity limit, with nothing started: {err}"
    );
    assert!(
        err.contains(&wait.to_string()),
        "and name the limit it hit, so the user can be told something true: {err}"
    );
    assert!(
        err.contains("Do not run the work yourself"),
        "the inline exit is the whole of bug AL and must be closed in words here: {err}"
    );
    assert!(
        err.contains("do not schedule anything to retry it"),
        "and the routine exit is bug AT, taken from this same refusal: {err}"
    );

    // No side effects before admission: a refused enqueue leaves the chat and the store as they
    // were. In round 22 the agent was left holding work it believed it had been told to do.
    assert_eq!(
        agent.store.jobs_for_chat(CHAT).unwrap().len(),
        rows_before,
        "a refused enqueue wrote a job row"
    );
    assert_eq!(
        host.lock().await.doc.messages().len(),
        msgs_before,
        "a refused enqueue posted to the chat"
    );
}

/// The per-agent waiting bound, which is what bites first when the waiting jobs are spread out.
///
/// Its assertions are the same honest-capacity shape as the per-chat case, and deliberately more
/// than "it refused with a number in it". Today's `main` refuses here too, with `24 jobs already
/// run across your chats (cap 6)`, so a case that only looked for the scope and the count passed
/// against the bug it was written to catch — which it did, until this was tightened.
#[tokio::test]
async fn a_full_waiting_tier_across_an_agents_chats_is_refused() {
    let (agent, host, _held) = a_box().await;
    let wait_agent = crate::config::job_wait_max_per_agent();
    // One per chat, so no chat is near its own waiting bound and only the per-agent one can bite.
    for i in 0..wait_agent {
        plant_open_jobs(
            &agent,
            &format!("01TESTSPREADCHATWAITTI{i:03}"),
            1,
            "queued",
        );
    }

    let err = enqueue(&agent, &host, "Counting the licence lines")
        .await
        .expect_err("the per-agent waiting bound refuses too");
    assert!(
        err.contains("not started") && err.contains("waiting for capacity"),
        "a capacity statement, not an instruction to go and find another way: {err}"
    );
    assert!(
        err.contains("across your chats") && err.contains(&wait_agent.to_string()),
        "naming which of the two limits it was: {err}"
    );
    assert!(
        err.contains("Do not run the work yourself")
            && err.contains("do not schedule anything to retry it"),
        "with both of round 22's exits closed on this branch too: {err}"
    );
}

/// Enqueue idempotency: the same ask enqueued twice is one job, not two places in the queue.
///
/// A retried or duplicated `start_job` is the ordinary case — a tool call re-sent after a
/// timeout, a model that repeats itself. Without this, a retry silently doubles the work and
/// doubles what a stop has to find.
#[tokio::test]
async fn a_duplicate_enqueue_of_the_same_ask_takes_one_place() {
    let (agent, host, _held) = a_box().await;

    let first = enqueue(&agent, &host, "Counting the licence lines")
        .await
        .expect("the first enqueue is accepted");
    let second = enqueue(&agent, &host, "Counting the licence lines")
        .await
        .expect("and the second is answered rather than refused");

    assert_eq!(
        first["job_id"], second["job_id"],
        "the retry must be told about the job it already has: {first} vs {second}"
    );
    assert_eq!(
        second["already_open"],
        serde_json::json!(true),
        "and told that is what happened, so it does not read as a second job: {second}"
    );
    assert!(
        second["note"]
            .as_str()
            .unwrap_or_default()
            .contains("Do not start it again by other means"),
        "and told not to take round 22's exit out of it either: {second}"
    );
    assert_eq!(
        agent
            .store
            .jobs_for_chat(CHAT)
            .unwrap()
            .iter()
            .filter(|j| j.is_open())
            .count(),
        1,
        "two enqueues of one ask took two places in the queue"
    );
}

/// Two different jobs asked for in one message stay two jobs: the idempotency key is the ask
/// **and** the title, because one message often asks for several.
///
/// **Green on `main` too, and that is the point of it.** It is not evidence for the change; it is
/// the guard on the change, because the cheap version of the dedupe above — keyed on the ask
/// alone — would silently drop work the user had asked for. A test that can only fail when my own
/// fix is too eager is worth having precisely because nothing else here would catch that.
#[tokio::test]
async fn two_different_jobs_from_one_message_are_not_merged() {
    let (agent, host, _held) = a_box().await;

    let a = enqueue(&agent, &host, "Counting the licence lines")
        .await
        .expect("the first job");
    let b = enqueue(&agent, &host, "Counting the test files")
        .await
        .expect("the second job of the same message");

    assert_ne!(
        a["job_id"], b["job_id"],
        "one message may ask for several jobs: {a} vs {b}"
    );
    assert_eq!(
        agent
            .store
            .jobs_for_chat(CHAT)
            .unwrap()
            .iter()
            .filter(|j| j.is_open())
            .count(),
        2
    );
}

/// A waiting job is cancellable before it starts, which is the point of giving it a row at all.
///
/// This is the property round 22 did not have: the inline work could not be stopped because
/// nothing named it. Here the stop reaches the job through the ordinary path, and bug AN's guard
/// then keeps it from starting when capacity frees.
#[tokio::test]
async fn a_waiting_job_can_be_cancelled_before_it_starts() {
    let (agent, host, _held) = a_box().await;
    plant_open_jobs(&agent, CHAT, crate::config::job_max_per_chat(), "running");

    let out = enqueue(&agent, &host, "Counting the licence lines")
        .await
        .expect("the job waits for capacity");
    let job_id = out["job_id"].as_str().unwrap().to_owned();

    let stopped = agent.stop_jobs(CHAT, Some(&job_id)).await;
    assert_eq!(
        stopped.len(),
        1,
        "a waiting job is open, so a stop reaches it: {stopped:?}"
    );
    assert_eq!(
        agent.store.job(&job_id).unwrap().unwrap().state,
        "cancelled",
        "and the row records it, which is what the inline work of round 22 could never do"
    );
}
