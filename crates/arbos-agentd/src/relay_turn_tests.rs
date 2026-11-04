//! AF-782, "why is the bar chart not posted". A result turn (`TurnKind::SubagentResult`, so
//! `reply_only`) posted its summary and its table, then reached for a tool to post the chart it
//! had promised. The relay-turn guard aborted pi on that call: the call was never answered, the
//! chart was never posted, the promise was never carried forward, and the turn recorded no tool
//! calls at all.
//!
//! The guard itself is right — Doug, 2026-10-22: a result turn routed `act`, ran tools for 13
//! minutes and held every chat. What was wrong is its reach. A turn that hands the user the
//! thing it promised is delivering, not working, so the **delivery tools** run in a relay turn as
//! `ask_user` does. Everything else is still refused, and now recorded when it is.

use std::sync::Arc;

use serde_json::{json, Value};

use arbos_core::doc::UserMessageOpts;
use arbos_core::rpc::Event;

use crate::agent::{relay_turn_tool, Agent, TurnKind};
use crate::status::tests::test_agent;
use crate::sync::SharedHost;

/// The turn as the agent-link log shows it (`GET /agent-link/agents/{agent}/log`): one line per
/// T3 row, with the `message_id` / `call_id` the log carries. Printed by the tests so the trace
/// is read off a run, not described.
fn trace(agent: &Agent, chat: &str) -> String {
    let mut out = String::new();
    for (row, source) in agent
        .store
        .t3_thread_in_order(chat, 200)
        .unwrap_or_default()
    {
        let s = source.unwrap_or(Value::Null);
        let tag = |k: &str| s.get(k).and_then(Value::as_str).unwrap_or("-").to_owned();
        let detail = match row.kind.as_str() {
            "tool_call" | "tool_output" => format!("call_id {}", tag("call_id")),
            _ => format!("message_id {}", tag("message_id")),
        };
        let text: String = row.text.chars().take(60).collect();
        out.push_str(&format!(
            "{:<11} {:<34} {}\n",
            row.kind,
            detail,
            text.replace('\n', " ⏎ ")
        ));
    }
    out
}

/// The progress rows of `user_msg_id`, oldest first, with the text each one actually carries.
fn progress_rows(host: &SharedHost, user_msg_id: &str) -> Vec<(String, String)> {
    let h = host.try_lock().expect("host free");
    h.doc
        .messages()
        .into_iter()
        .filter(|m| {
            m.role == "assistant"
                && m.rest.get("inReplyTo").and_then(Value::as_str) == Some(user_msg_id)
                && m.rest.get("turnRole").and_then(Value::as_str) == Some("progress")
        })
        .map(|m| (m.id.clone(), m.text()))
        .collect()
}

/// Jacob's turn up to the moment it reaches for a tool: a result turn posts its summary and its
/// table as progress bubbles, having said it would post a chart too.
async fn jacobs_turn_up_to_the_chart(agent: &Arc<Agent>, chat: &str, host: &SharedHost) {
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(
                "m-ask",
                "give me the listings as a table plus a bar chart of the prices",
                "dev-mac",
                &UserMessageOpts::default(),
            )
            .unwrap();
    }
    // The guard frees `turn_lock` when it drops; the turn state it claimed stays.
    drop(agent.test_hold_turn(chat).await);
    {
        let mut tl = agent.turn.lock().await;
        let t = tl.as_mut().unwrap();
        t.user_msg_id = "m-ask".to_owned();
        t.ack_text.clear();
        t.activity = None;
        // What `TurnState` carries for `TurnKind::SubagentResult`.
        t.reply_only = true;
        t.relay_fallback =
            "The background work I started has finished. Ask me for the details whenever you like."
                .to_owned();
        agent.test_open_row(host, t).await;
    }
    agent
        .hook_send_message("Four listings came back; here they are.", None, None)
        .await
        .unwrap();
    agent
        .hook_send_message(
            "| Listing | Price |\n| --- | --- |\n| Casa Azul | $420 |",
            None,
            None,
        )
        .await
        .unwrap();
    // No pi child in a test: `pi()` would spend its full 20 s wait before failing, and the Abort
    // it carries has no child to reach. Shutting down makes that lookup fail at once; everything
    // the guard does to the turn still runs.
    agent
        .shutting_down
        .store(true, std::sync::atomic::Ordering::SeqCst);
}

fn call(tool_name: &str) -> Event {
    Event::ToolExecutionStart {
        tool_call_id: "c-chart".into(),
        tool_name: tool_name.into(),
        args: json!({ "caption": "Prices across the four listings." }),
        parent_tool_call_id: None,
    }
}

/// The fix, against Jacob's trace: the turn promised a chart, and `post_chart` reaches the chat.
/// The turn stays open, the call is taken up so something can answer it, and it is counted.
#[tokio::test]
async fn a_result_turn_may_post_the_chart_it_promised() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATRELAYTURN0000001";
    let host = agent.sync.open_room(chat).await.unwrap();
    jacobs_turn_up_to_the_chart(&agent, chat, &host).await;

    agent.test_fold(chat, &host, &call("post_chart")).await;

    let tl = agent.turn.lock().await;
    let t = tl.as_ref().unwrap();
    assert!(!t.stopped, "delivering what the turn promised is not work");
    assert_eq!(t.metrics.tool_calls, 1, "the call is counted");
    assert!(
        t.tool_inputs.contains_key("c-chart"),
        "the call is taken up, so its result has somewhere to land"
    );
    drop(tl);

    println!(
        "--- after: a result turn posting the chart it promised ---\n{}",
        trace(&agent, chat)
    );
    let _ = std::fs::remove_dir_all(root);
}

/// Work is still refused — a relay turn does not work — but the refusal is now a recorded,
/// answered call instead of a call killed in flight. Jacob's `codemode` script is the case.
#[tokio::test]
async fn a_result_turn_still_refuses_work_and_records_the_call() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATRELAYTURN0000002";
    let host = agent.sync.open_room(chat).await.unwrap();
    jacobs_turn_up_to_the_chart(&agent, chat, &host).await;

    agent.test_fold(chat, &host, &call("codemode")).await;

    {
        let tl = agent.turn.lock().await;
        let t = tl.as_ref().unwrap();
        assert!(t.stopped, "a relay turn that reaches for work ends");
        assert_eq!(
            t.metrics.tool_calls, 1,
            "the turn made a call and must say so"
        );
    }
    let trace = trace(&agent, chat);
    println!("--- after: a result turn reaching for a script ---\n{trace}");
    assert!(
        trace.contains("tool_call") && trace.contains("tool_output"),
        "a refused call is a call with an answer, not a gap in the trace:\n{trace}"
    );
    assert!(
        trace.contains("refused"),
        "the record says why the call did not run:\n{trace}"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// Which turn kinds may hand the user what they promised. No wildcard arm: a new `TurnKind` does
/// not compile until someone decides, which is the point — two guards in six days have each
/// independently blocked a working `post_chart` (the `post_` risky-name pattern on 2026-10-27,
/// and this one), and both were written without a thought for delivery.
fn may_deliver(kind: &TurnKind) -> bool {
    match kind {
        // A user turn does everything.
        TurnKind::User => true,
        // A result turn's whole job is handing over what the work produced.
        TurnKind::SubagentResult { .. } => true,
        // An approval outcome dispatches the thing that was approved.
        TurnKind::ApprovalOutcome { .. } => true,
        // A job fallback does the user's ask inline and delivers it.
        TurnKind::JobFallback { .. } => true,
    }
}

/// Dense index per kind, so the sample list below cannot quietly miss one.
fn kind_index(kind: &TurnKind) -> usize {
    match kind {
        TurnKind::User => 0,
        TurnKind::SubagentResult { .. } => 1,
        TurnKind::ApprovalOutcome { .. } => 2,
        TurnKind::JobFallback { .. } => 3,
    }
}

const TURN_KIND_COUNT: usize = 4;

/// Every tool that declares itself a delivery tool is reachable from every turn kind. This is
/// the test that is meant to stop the third occurrence: a new delivery tool is covered the
/// moment it declares `"delivery": true`, and a new turn kind fails to compile until it is
/// listed above.
#[test]
fn every_turn_kind_can_deliver_every_delivery_tool() {
    let kinds = [
        TurnKind::User,
        TurnKind::SubagentResult {
            subagent_id: "s1".into(),
            name: "Searching".into(),
            status: "done".into(),
        },
        TurnKind::ApprovalOutcome {
            approval_id: "a1".into(),
            title: "Send the email".into(),
        },
        TurnKind::JobFallback {
            job_id: "j1".into(),
            title: "Searching".into(),
        },
    ];
    let mut seen = [false; TURN_KIND_COUNT];
    for kind in &kinds {
        seen[kind_index(kind)] = true;
    }
    assert!(
        seen.iter().all(|s| *s),
        "a turn kind has no sample here: {seen:?}"
    );

    let delivery = crate::mcp::delivery_tools();
    assert!(
        delivery.contains("post_chart") && delivery.contains("deliver_file"),
        "the declared delivery tools went missing: {delivery:?}"
    );
    for kind in &kinds {
        assert!(may_deliver(kind), "{}", kind_index(kind));
        for name in delivery {
            assert!(
                relay_turn_tool(name),
                "{name} is not reachable from turn kind {}",
                kind_index(kind)
            );
            assert!(
                relay_turn_tool(&format!("mcp__arbos__{name}")),
                "{name} is not reachable under the name pi uses for it"
            );
        }
    }
    // The guard still has an edge: work is work in a relay turn.
    for name in ["codemode", "bash", "web_search", "mcp__arbos__web_fetch"] {
        assert!(!relay_turn_tool(name), "{name} is not delivery");
    }
    // The marker is Arbos's own; the model never sees it on the tool list.
    let listed = crate::mcp::tools_for_test();
    let chart = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == json!("post_chart"))
        .expect("post_chart is on the tool list");
    assert!(
        chart.get("delivery").is_none(),
        "the delivery marker leaked to the model: {chart}"
    );
}

/// The id half of the report: the log said the final assistant event reused the table's message
/// id. It did not. `send_message` logs `t.assistant_msg_id` after it has already opened the next
/// row, so the table's record names a row the table is not in — and the row it names is the one
/// the turn ends on, which is what read as reuse. The table's own row is never touched.
#[tokio::test]
async fn a_progress_record_names_the_row_opened_after_it() {
    let (agent, root) = test_agent().await;
    let chat = "01TESTCHATRELAYTURN0000003";
    let host = agent.sync.open_room(chat).await.unwrap();
    jacobs_turn_up_to_the_chart(&agent, chat, &host).await;

    let rows = progress_rows(&host, "m-ask");
    assert_eq!(rows.len(), 2, "summary and table are two bubbles: {rows:?}");
    let (table_row_id, table_text) = rows[1].clone();
    assert!(table_text.contains("Casa Azul"), "{table_text}");

    let logged: Vec<String> = agent
        .store
        .t3_thread_in_order(chat, 200)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(_, source)| {
            let s = source?;
            (s.get("progress").and_then(Value::as_bool) == Some(true))
                .then(|| s.get("message_id")?.as_str().map(str::to_owned))
                .flatten()
        })
        .collect();
    assert_eq!(logged.len(), 2, "one record per bubble: {logged:?}");
    assert_ne!(
        logged[1], table_row_id,
        "the table's record names a row the table is not in"
    );
    assert_eq!(
        logged[1],
        agent
            .turn
            .lock()
            .await
            .as_ref()
            .unwrap()
            .assistant_msg_id
            .clone(),
        "it names the row the turn will end on — the 'reuse' in the log"
    );
    assert!(
        progress_rows(&host, "m-ask")[1].1.contains("Casa Azul"),
        "nothing overwrote the table"
    );
    let _ = std::fs::remove_dir_all(root);
}
