//! The three answers a cancel may give about processes, and the two it used to merge.
//!
//! **Bug AP.** `stop_job` had one sentence for every cancel that ended with an empty survivor
//! list: "every process it had started was checked and is dead". Three different things reach
//! that state. The scan enumerated processes and killed them. The scan ran and there was
//! nothing of the kind to look at. Or the scan could not read what it needed — a `/proc` it
//! cannot walk returns no process, which is byte-for-byte what a quiet box returns — and the
//! box then reported the strongest of the three.
//!
//! The reach of the third is what makes it worth a file. `reap_unreachable_workers` runs on
//! every chat-wide stop, and when its mark scan came back empty it returned
//! [`crate::isolation::Sweep::nothing_to_check`], whose `checked: true` is a confident "all
//! accounted for" over a tree it had never enumerated.
//!
//! # What a false pass would look like here
//!
//! A case that builds a [`crate::isolation::Sweep`] and asserts on its fields passes whether or
//! not any production path can produce that sweep, and a case that calls
//! `reap_unreachable_workers` directly passes against the unfixed code too — the reaper was
//! never wrong, its answer was. So both cases below go through
//! [`crate::agent::Agent::hook_stop_job`], the entry point the agent actually calls, and assert
//! on the answer the model is handed. Neither plants a process, a mark or a worker id.

use arbos_core::ids::ulid;

use crate::agent::StoppedJob;
use crate::isolation::{Evidence, Sweep};
use crate::status::tests::test_agent;

const CHAT: &str = "01TESTCHATCANCELUNCONF0001";

/// An open job that has never been given a worker.
///
/// Deliberately workerless: it makes the per-job sweep say "there was nothing to check" on its
/// own terms, so the only thing left that can speak about processes in the answer is the
/// box-wide reap. Without that isolation a blinded `/proc` would also blind the job's own
/// sweep, and the case would pass for a reason it is not testing.
async fn a_workerless_open_job(agent: &crate::agent::Agent) -> String {
    let job = ulid();
    agent
        .store
        .insert_job(
            &job,
            CHAT,
            &ulid(),
            "A job with no worker",
            "code",
            "do the thing",
            &[],
        )
        .expect("the job row is written");
    agent
        .store
        .set_job_state(&job, "running", None)
        .expect("the job is open");
    job
}

/// **The guard.** A cancel whose scan could not read `/proc` must not report that the box is
/// accounted for.
///
/// Red against the unfixed code: the mark walk returns an empty vector for a `/proc` it cannot
/// read, `dead.is_empty()` is then true, `reap_unreachable_workers` returns
/// `Sweep::nothing_to_check()` with `checked: true`, and the note is the one that tells the
/// model to say every process was checked and is dead. Nothing in the answer contradicts it,
/// because the reap's own verdict never reached the sentence at all.
///
/// The blinding is the only way to reach the branch: a box whose `/proc` works cannot be made
/// to fail this read, and the failure is the case. It is thread-local, so the cases running
/// beside this one keep the real `/proc`.
#[tokio::test]
async fn a_cancel_whose_scan_could_not_read_the_kernel_says_it_could_not_confirm() {
    let (agent, root) = test_agent().await;
    let job = a_workerless_open_job(&agent).await;
    let _turn = agent.test_hold_turn(CHAT).await;

    let empty = root.join("a-proc-holding-no-process");
    std::fs::create_dir_all(&empty).expect("a directory the walk can open and learn nothing from");
    crate::isolation::blind_proc_walks_to(&empty);
    let answer = agent
        .hook_stop_job(&job)
        .await
        .expect("the job is stoppable");

    let note = answer["note"].as_str().unwrap_or_default();
    assert!(
        note.contains("could not look for them"),
        "a scan that read nothing knows nothing, and must say so: {answer}"
    );
    assert!(
        !note.contains("checked and is dead"),
        "and must not make the claim it could not support: {answer}"
    );
    // The reap is box-wide and mark-keyed, so its verdict is not the named job's. A sentence
    // that merged the two would read perfectly well and would attribute work belonging to no
    // row to the job the user just cancelled.
    assert!(
        note.contains("belongs to no job") && note.contains("about no job in particular"),
        "and must not say it about the job above, which is not what it looked at: {answer}"
    );
    assert_eq!(
        answer["unnamed_processes_checked"],
        serde_json::json!(false),
        "and says it structurally too, so the answer cannot read clean while the note does \
         not: {answer}"
    );
}

/// **The true negative, and it must stay a different sentence from the one above.** A scan that
/// ran, read `/proc` and found no unreachable work has not failed — it has nothing to report.
///
/// Pinned because the cheap way to pass the case above is to say "could not confirm" whenever a
/// sweep ends empty, which is most cancels, and a guard that fires on every ordinary stop is a
/// guard nobody reads by the second week.
///
/// The third thing this holds is the convention: a job with no worker reports **no**
/// `processes_checked` field at all rather than a vacuously true one, which is what the
/// row-less cancel path does and is why this does it too.
#[tokio::test]
async fn a_cancel_whose_scan_ran_and_found_nothing_says_there_was_nothing_to_check() {
    let (agent, _root) = test_agent().await;
    let job = a_workerless_open_job(&agent).await;
    let _turn = agent.test_hold_turn(CHAT).await;

    let answer = agent
        .hook_stop_job(&job)
        .await
        .expect("the job is stoppable");

    let note = answer["note"].as_str().unwrap_or_default();
    assert!(
        !note.contains("could NOT be checked"),
        "a scan that read the kernel and found nothing has not failed to look: {answer}"
    );
    assert!(
        note.contains("no processes to account for"),
        "and the sentence says that, rather than claiming a kill that never happened: {answer}"
    );
    assert!(
        answer["stopped"][0].get("processes_checked").is_none(),
        "no vacuous `checked` claim belongs in the answer for a job that had no worker: {answer}"
    );
    assert!(
        answer.get("unnamed_processes_checked").is_none(),
        "nor in the box-wide half, which also found nothing of the kind to look at: {answer}"
    );
    assert_eq!(
        answer["all_cancelled"],
        serde_json::json!(true),
        "nothing was running and nothing is unaccounted for, so this is a confirmed cancel: {answer}"
    );
}

/// The chat line keeps the same discipline as the note: a reap that could not look does not get
/// to end the sentence with a promise.
///
/// `stop_line` reached its closing clause through `unreachable.survivors.is_empty()`, and an
/// empty survivor list is what a quiet box and a `/proc` that cannot be walked both produce. So
/// the sentence the **user** reads — not the note the model is handed — was the last place the
/// unchecked claim survived.
///
/// Built from `Sweep` values rather than from a cancel, and that is sound here only because
/// this is the whole input of a pure function: the production path that makes a reap
/// unconfirmed is driven end to end by the first case in this file, and this pins what the
/// chat says once it is.
#[test]
fn the_chat_line_promises_nothing_more_arrives_only_when_the_reap_could_look() {
    let cancelled = vec![StoppedJob {
        id: ulid(),
        title: "Ticking a log".to_owned(),
        state: "cancelled".to_owned(),
        sweep: Sweep {
            found: 1,
            killed: 1,
            survivors: Vec::new(),
            evidence: Evidence::Enumerated,
            unread: None,
        },
    }];

    let looked = Sweep::nothing_to_check();
    assert!(
        crate::agent::stop_line(&cancelled, &[], &looked).contains("nothing more arrives"),
        "a reap that read the kernel and found nothing has accounted for the box, and the \
         promise is the user's to have: {}",
        crate::agent::stop_line(&cancelled, &[], &looked)
    );

    let blind = Sweep::default();
    let line = crate::agent::stop_line(&cancelled, &[], &blind);
    assert!(
        !line.contains("nothing more arrives"),
        "but a reap that could not look has not earned it, however empty its survivor list \
         is: {line}"
    );
}
