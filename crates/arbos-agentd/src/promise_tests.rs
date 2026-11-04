//! AF-782, the half that is not about charts: an agent that states a next step and then loses it
//! will reproduce this bug for any promised work. Jacob's turn said "I'll post the table and the
//! price chart now", was cut short before the chart, and 12 s later the window was compacted. The
//! promise existed nowhere but in the words compaction had just rewritten, so it was gone.
//!
//! A promise is therefore kept in the store and read back into the working set, which is rebuilt
//! from the store on every turn — the same reason a running job is still there after a
//! compaction.

use arbos_core::rpc::Event;

use crate::agent::{promised_next_step, TurnState};
use crate::status::tests::test_agent;

/// A cut-short turn whose words promised a next step.
fn turn_that_promised(chat: &str, words: &str) -> TurnState {
    let mut t = TurnState::claim(chat, "m-ask", false);
    t.progress_texts.push(words.to_owned());
    t.stopped = true;
    t
}

#[test]
fn a_stated_next_step_is_told_apart_from_a_plan_a_refusal_and_a_claim() {
    // Jacob's line, and the shapes around it.
    assert_eq!(
        promised_next_step("I'll post the table and the price chart now.").as_deref(),
        Some("I'll post the table and the price chart now.")
    );
    for promise in [
        "Let me pull the prices first.",
        "I'm going to send the comparison next.",
        "Posting the summary now.",
        "Got the four listings. I will attach the spreadsheet in a moment.",
    ] {
        assert!(
            promised_next_step(promise).is_some(),
            "missed a promise: {promise}"
        );
    }
    // Only the promising sentence comes back, not the whole message.
    assert_eq!(
        promised_next_step(
            "Four listings came back. I'll post the chart now. They are all under $500."
        )
        .as_deref(),
        Some("I'll post the chart now.")
    );

    for not_a_promise in [
        // Done, not owed — that is `claims_posted`' business.
        "I've posted the chart above.",
        "The table is in the chat.",
        // Declined.
        "I won't post the chart now; the data is too sparse.",
        "I cannot send it right away without the key.",
        // Asked, not promised.
        "Do you want me to post the chart now?",
        "Would you like the table next?",
        // A plan with no clock on it: the turn is not leaving work behind.
        "I'll keep an eye on the prices.",
        "The approach I would take is to chart the prices.",
        // Someone else's next step.
        "The job will post its result when it finishes.",
    ] {
        assert_eq!(
            promised_next_step(not_a_promise),
            None,
            "false promise: {not_a_promise}"
        );
    }
}

/// The trace Jacob hit: the turn is cut short owing a chart, the window is compacted, and the
/// next turn still knows. Then a turn that runs to completion hands the promise back.
#[tokio::test]
async fn a_promise_outlives_the_turn_and_the_compaction_that_follows_it() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATPROMISE000000001";
    let host = agent.sync.open_room(chat).await.unwrap();

    assert!(
        !agent.working_set_text(chat).contains("ended before"),
        "nothing is owed yet"
    );

    let cut_short = turn_that_promised(chat, "I'll post the table and the price chart now.");
    agent.carry_promise_past_this_turn(chat, &cut_short, false);
    assert_eq!(
        agent.outstanding_promise(chat).as_deref(),
        Some("I'll post the table and the price chart now.")
    );
    let ws = agent.working_set_text(chat);
    assert!(
        ws.contains("I'll post the table and the price chart now."),
        "{ws}"
    );
    assert!(
        ws.contains("Do it now"),
        "the next turn is told what to do: {ws}"
    );

    // What killed it before: pi compacts the window, rewriting every word of that turn.
    let _busy = agent.test_hold_turn(chat).await;
    agent
        .test_fold(
            chat,
            &host,
            &Event::CompactionEnd {
                result: None,
                aborted: false,
                error_message: None,
            },
        )
        .await;
    assert!(
        agent
            .working_set_text(chat)
            .contains("I'll post the table and the price chart now."),
        "the promise is read from the store, so compaction cannot reach it"
    );

    // One turn carries it. A turn that runs to completion has been shown it and hands it back.
    let mut done = TurnState::claim(chat, "m-next", false);
    done.progress_texts.push("Here is the chart.".to_owned());
    agent.carry_promise_past_this_turn(chat, &done, true);
    assert_eq!(agent.outstanding_promise(chat), None);
    assert!(!agent.working_set_text(chat).contains("ended before"));

    let _ = std::fs::remove_dir_all(root);
}

/// A turn that finished normally leaves nothing behind, promise words or not: the carry-forward
/// is for work an interrupted turn still owes, not a running commentary.
#[tokio::test]
async fn a_turn_that_finished_leaves_no_promise() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATPROMISE000000002";
    let mut t = TurnState::claim(chat, "m-ask", false);
    t.progress_texts.push("I'll post the chart now.".to_owned());
    agent.carry_promise_past_this_turn(chat, &t, true);
    assert_eq!(agent.outstanding_promise(chat), None);
    let _ = std::fs::remove_dir_all(root);
}
