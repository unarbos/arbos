//! Bug AG, Test5 round 22, agentd 0.1.200 (`1b2d967c3`), scenario `r22steer`.
//!
//! One ask — twelve changes to a table, "exactly ONE CSV" — and four CSVs reached the user, at
//! 16:40:05, 16:40:06, 16:41:10 and 16:42:39. The agent said of its own output that the second
//! was "the second wrong CSV", so these are not four deliverables that happened to arrive
//! together: each was an answer that the next one replaced.
//!
//! # What the code says the four posts are
//!
//! Every one of them is a finished `attachment` row written by a deliberate `deliver_file` tool
//! call, through the one post site there is, [`crate::media::deliver`]. There is no progress
//! channel for a file: `deliver` writes the row with `turnRole: attachment`, finishes it `Done`
//! in the same lock, and its duplicate guard tells the model in as many words "do not post it
//! again". So suppressing intermediate frames is not the change this needs — there are no
//! frames. The change is that the latest answer must take the earlier card's place.
//!
//! # Where the rule was already written, and where it was missing
//!
//! "One ask, one file" is `correction_reached_worker`'s own phrase, and three paths keep it:
//! the steer gate suppresses a delivery that a correction beat, `supersede_posted_file_of_running_job`
//! supersedes the card of a *running* job the correction reached, and `replace_just_posted_job`
//! does it for a job that finished moments ago. All three hang off the **steer** arriving, and
//! all three are keyed on one job id.
//!
//! Round 22 went round them in the one shape none of them covers, and the report names it: the
//! steers stacked up, the first job finished before the late ones landed, and a recount was
//! started. A recount is a *second job*. Its file is posted by `deliver`, which asks about the
//! bytes and about the job that is posting them and never about the card the work it took over
//! already put in the chat. So the new CSV is appended and the old one stands beside it, with
//! nothing to say which is live.
//!
//! The box already knows the two jobs are the same work: `steers_to_inherit` hands the recount
//! every correction the user had accepted on the first job, which is bug AG's second half. What
//! was missing is that the *answer* did not inherit the same way the corrections did.

use std::sync::Arc;
use std::time::Duration;

use arbos_core::doc::UserMessageOpts;
use serde_json::{json, Value};

use crate::agent::Agent;
use crate::status::tests::test_agent;
use crate::sync::SharedHost;

const CHAT: &str = "01TESTCHATONEFILEPERASK001";
const ASK: &str = "01TESTASKONEFILEPERASK0001";

const JOB1: &str = "01TESTJOBONEFILEPERASK0001";
const WORKER1: &str = "01TESTWRKONEFILEPERASK0001";
const ROW1: &str = "01TESTROWONEFILEPERASK0001";

const JOB2: &str = "01TESTJOBONEFILEPERASK0002";
const WORKER2: &str = "01TESTWRKONEFILEPERASK0002";
const ROW2: &str = "01TESTROWONEFILEPERASK0002";

/// The user's ask, once, in the chat.
async fn an_ask(host: &SharedHost) {
    let h = host.lock().await;
    h.doc
        .append_user_message(
            ASK,
            "Twelve changes to that table, and give me exactly ONE CSV at the end.",
            "dev-mac",
            &UserMessageOpts::default(),
        )
        .unwrap();
}

/// A running job of this chat with a worker whose session `deliver_file` can post through.
async fn a_running_job(
    agent: &Arc<Agent>,
    host: &SharedHost,
    job: &str,
    worker: &str,
    row: &str,
    title: &str,
) {
    agent
        .store
        .insert_job(job, CHAT, ASK, title, "other", "Write one CSV.", &[])
        .unwrap();
    agent
        .store
        .insert_subagent(worker, "job-a-table", "build a table", "pi", CHAT)
        .unwrap();
    agent.store.set_job_subagent(job, worker).unwrap();
    agent.store.set_job_state(job, "running", None).unwrap();
    {
        let h = host.lock().await;
        h.doc.start_assistant_message(row, ASK, "box:test");
        h.doc.set_message_fields(
            row,
            &[
                (
                    "job",
                    json!({ "id": job, "title": title, "kind": "other", "touches": [] }),
                ),
                ("worker", json!(true)),
                ("hidden", json!(true)),
            ],
        );
        h.doc.add_status_part(row, "working", "Working…");
    }
    agent
        .subagent_msgs
        .lock()
        .await
        .insert(worker.to_owned(), (CHAT.to_owned(), row.to_owned()));
}

/// Shorten the steer hold for one agent, through the same knob file production reads, so the
/// deliveries below do not sit out the real one.
fn hold_for(agent: &Arc<Agent>, ms: u64) {
    let f = agent.cfg.state_dir.join(crate::config::STEER_HOLD_MS_FILE);
    std::fs::write(&f, ms.to_string()).unwrap();
    assert_eq!(agent.cfg.steer_hold(), Duration::from_millis(ms));
}

/// Write `name` under `out/` and hand back the path `deliver_file` takes.
fn a_file(agent: &Arc<Agent>, name: &str, body: &str) -> String {
    let dir = crate::media::out_dir(&agent.cfg);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), body).unwrap();
    format!("out/{name}")
}

async fn worker_delivers(agent: &Arc<Agent>, worker: &str, path: &str) -> Value {
    crate::media::deliver_file(agent, &json!({ "path": path, "session_id": worker })).await
}

/// Every file card in the chat, as `(message id, name, superseded)`, oldest first.
fn cards(host: &SharedHost) -> Vec<(String, String, bool)> {
    let h = host.try_lock().expect("host free");
    h.doc
        .messages()
        .into_iter()
        .flat_map(|m| {
            let id = m.id.clone();
            let gone = m
                .rest
                .get("superseded")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            m.parts
                .clone()
                .into_iter()
                .filter(|p| p.get("kind").and_then(Value::as_str) == Some("attachment"))
                .filter_map(move |p| {
                    p.pointer("/attachment/name")
                        .and_then(Value::as_str)
                        .map(|n| (id.clone(), n.to_owned(), gone))
                })
        })
        .collect()
}

/// The cards the user is actually left with as live: the ones not marked superseded.
fn standing(host: &SharedHost) -> Vec<(String, String, bool)> {
    cards(host)
        .into_iter()
        .filter(|(_, _, gone)| !*gone)
        .collect()
}

/// What is left of a card: whether its row is still in the chat, whether it was retired, and the
/// blob its attachment still names.
///
/// A superseded answer has to stay *readable*, so this reads the three things that would say it
/// does not: a missing row, a hidden one, or an attachment that no longer points at its bytes.
fn recoverable(host: &SharedHost, card_id: &str) -> (bool, bool, Option<String>) {
    let h = host.try_lock().expect("host free");
    match h.doc.message(card_id) {
        None => (false, false, None),
        Some(m) => (
            true,
            m.rest.get("hidden").and_then(Value::as_bool) == Some(true),
            m.parts
                .iter()
                .filter(|p| p.get("kind").and_then(Value::as_str) == Some("attachment"))
                .find_map(|p| {
                    p.pointer("/attachment/ref")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                }),
        ),
    }
}

/// **The red case.** Round 22's own shape: the first job answers, late corrections arrive after
/// it has finished, a recount job takes the work over, and the recount's CSV is appended beside
/// the first one instead of replacing it.
///
/// On today's `main` the chat ends with **two** standing cards for one ask, which is the symptom
/// the report records four times over. With the fix the first card is marked superseded and the
/// recount's is the one the user is left with.
///
/// The first job is put outside [`crate::config::STEER_REPLACE_WINDOW_MS`] on purpose: inside it
/// a correction would have been `replace_just_posted_job`'s case and would have reopened the same
/// job rather than starting a recount. Round 22's steers stacked up for over a minute, so the
/// window had long gone — and that is exactly why a second job existed at all.
#[tokio::test]
async fn the_recount_jobs_file_replaces_the_first_jobs_card() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    hold_for(&agent, 10);
    an_ask(&host).await;

    // 16:40:05 — the first job's answer.
    a_running_job(&agent, &host, JOB1, WORKER1, ROW1, "Counting the files").await;
    let first = a_file(&agent, "steer22.csv", "name,count\na,2\n");
    let posted = worker_delivers(&agent, WORKER1, &first).await;
    assert!(
        posted.get("isError").is_none(),
        "the first file posts: {posted}"
    );
    assert_eq!(standing(&host).len(), 1, "one card so far");
    // Read from the chat rather than from the delivery's reply: the row is what the user has,
    // and it is the row the assertions at the end are about.
    let first_card = cards(&host)[0].0.clone();
    let first_blob = recoverable(&host, &first_card)
        .2
        .expect("the first card names the bytes behind it");

    // The corrections the user had accepted on it, and the job ending before the late ones land.
    agent
        .store
        .append_job_steer(JOB1, "files first, then count, then alphabetical")
        .unwrap();
    agent.store.set_job_state(JOB1, "done", None).unwrap();
    const {
        assert!(
            crate::config::STEER_REPLACE_WINDOW_MS < crate::config::STEER_INHERIT_WINDOW_MS,
            "this case needs a gap between the two windows to sit in"
        )
    };

    // 16:41:10 — the recount. A second job, for the same work: it inherits the correction the
    // first job was given, which is the box's own record that these are one deliverable.
    assert!(
        !agent.steers_to_inherit(CHAT, JOB2).is_empty(),
        "the recount must inherit the first job's correction, or it is not the same work"
    );
    a_running_job(&agent, &host, JOB2, WORKER2, ROW2, "Recounting the files").await;
    let recount = a_file(&agent, "steer22.csv", "files,count,name\n3,2,a\n");
    let posted = worker_delivers(&agent, WORKER2, &recount).await;
    assert!(
        posted.get("isError").is_none(),
        "the recount's file posts: {posted}"
    );

    // The shape, not the count. Two rows, one of them live, and the superseded one still fully
    // there — because the correction that caused the recount can itself be wrong, and the first
    // card is the only copy of the first answer. A case that asserted "one card" alone would
    // pass just as well for a fix that deleted the row, and then we could not tell from the
    // test which of the two had shipped.
    let all = cards(&host);
    assert_eq!(all.len(), 2, "both deliveries wrote a card: {all:?}");
    let left = standing(&host);
    assert_eq!(
        left.len(),
        1,
        "one ask, and the user is left with more than one live CSV to choose between: {all:?}"
    );
    assert_eq!(
        left[0].0, all[1].0,
        "and the live one must be the recount's, not the one it replaced: {all:?}"
    );
    assert!(
        all[0].2,
        "the first answer must be marked superseded rather than left looking current: {all:?}"
    );
    let (present, hidden, blob) = recoverable(&host, &first_card);
    assert!(
        present,
        "the superseded card's row was removed from the chat"
    );
    assert!(
        !hidden,
        "the superseded card was retired, so the user cannot see it any more"
    );
    assert_eq!(
        blob.as_deref(),
        Some(first_blob.as_str()),
        "the superseded card no longer names its own bytes, so the first answer is unrecoverable"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The same job posting a better file replaces its own card, not only another job's.
///
/// Round 22's first two CSVs were one second apart, at 16:40:05 and 16:40:06, which is one worker
/// delivering twice rather than a handover. The duplicate guard above this only catches the
/// *identical* bytes; two revisions are two different files and both used to stand.
#[tokio::test]
async fn a_job_posting_a_second_revision_replaces_its_own_card() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    hold_for(&agent, 10);
    an_ask(&host).await;
    a_running_job(&agent, &host, JOB1, WORKER1, ROW1, "Counting the files").await;

    let v1 = a_file(&agent, "steer22.csv", "name,count\na,2\n");
    worker_delivers(&agent, WORKER1, &v1).await;
    let v2 = a_file(&agent, "steer22.csv", "files,count,name\n3,2,a\n");
    worker_delivers(&agent, WORKER1, &v2).await;

    let all = cards(&host);
    assert_eq!(all.len(), 2, "two revisions, two rows: {all:?}");
    assert_eq!(
        standing(&host).len(),
        1,
        "and one of them live, or the user picks between two CSVs again: {all:?}"
    );
    assert!(
        all[0].2 && !all[1].2,
        "the newer one is the live one: {all:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// **The guard on the key.** Two files a job was asked for separately both stand.
///
/// This is the cost side of keying on the file name, and the case that fails if the rule is ever
/// widened to the file *kind* or to "the last card of this work" — both of which would catch a
/// renamed revision and would also throw away one of the two deliverables here. A wrongly
/// superseded card costs the user something they asked for; a missed one leaves them where they
/// already were, so the key errs this way on purpose.
#[tokio::test]
async fn two_files_one_job_was_asked_for_separately_both_stand() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    hold_for(&agent, 10);
    an_ask(&host).await;
    a_running_job(&agent, &host, JOB1, WORKER1, ROW1, "Building the table").await;

    let table = a_file(&agent, "counts.csv", "name,count\na,2\n");
    worker_delivers(&agent, WORKER1, &table).await;
    let notes = a_file(&agent, "sources.csv", "url,checked\nhttp://x,yes\n");
    worker_delivers(&agent, WORKER1, &notes).await;

    let left = standing(&host);
    assert_eq!(
        left.len(),
        2,
        "a table and its sources are two deliverables and both must stay live: {left:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// **The other guard on the key.** A file of the same name from work this chat has moved on from
/// is left alone.
///
/// `report.csv` from an hour ago is not an earlier draft of today's `report.csv`. The bound is
/// [`crate::config::STEER_INHERIT_WINDOW_MS`], the same two minutes that decide which
/// corrections a follow-up job inherits, so "the work this chat is still on" means one thing in
/// both places rather than two.
#[tokio::test]
async fn a_same_named_file_from_work_the_chat_has_moved_on_from_is_left_alone() {
    let (agent, root) = test_agent().await;
    let host = agent.sync.open_room(CHAT).await.unwrap();
    hold_for(&agent, 10);
    an_ask(&host).await;

    a_running_job(&agent, &host, JOB1, WORKER1, ROW1, "Last hour's report").await;
    let old = a_file(&agent, "report.csv", "name,count\na,2\n");
    worker_delivers(&agent, WORKER1, &old).await;
    // Ended, and long enough ago that the chat is no longer on it.
    agent.store.set_job_state(JOB1, "done", None).unwrap();
    agent
        .store
        .test_backdate_job_end(JOB1, crate::config::STEER_INHERIT_WINDOW_MS + 1_000)
        .unwrap();

    a_running_job(&agent, &host, JOB2, WORKER2, ROW2, "This hour's report").await;
    let new = a_file(&agent, "report.csv", "files,count,name\n3,2,a\n");
    worker_delivers(&agent, WORKER2, &new).await;

    let left = standing(&host);
    assert_eq!(
        left.len(),
        2,
        "an unrelated file that happens to share a name was replaced: {left:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
