//! Exercise attribution at the actual HTTP stub: one reusable main credential, concurrent
//! workers, and the legacy upgrade path all share a proxy without borrowing each other's chat.

use axum::{extract::State, http::HeaderMap, routing::post, Json, Router};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

async fn serve(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    (
        base,
        tokio::spawn(async move {
            axum::serve(l, app).await.unwrap();
        }),
    )
}

#[tokio::test]
async fn model_usage_follows_main_switches_and_concurrent_workers() {
    let received = Arc::new(Mutex::new(Vec::<HeaderMap>::new()));
    let app = Router::new().route("/proxy/v1/chat/completions", post(
        |State(received): State<Arc<Mutex<Vec<HeaderMap>>>>, headers: HeaderMap| async move {
            received.lock().await.push(headers);
            Json(json!({"choices": [{"message": {"content":"ok"}}]}))
        }
    )).with_state(received.clone());
    let (relay, relay_task) = serve(app).await;
    let (agent, root) = crate::status::tests::test_agent_on_relay(&relay).await;
    for chat in ["chat-a", "chat-b"] {
        agent.store.upsert_chat(chat, chat, false, 1).unwrap();
        agent
            .store
            .set_chat_session_path(chat, &format!("/sessions/first-{chat}.jsonl"))
            .unwrap();
    }
    agent
        .store
        .insert_subagent("worker-a", "A", "test", "pi", "chat-a")
        .unwrap();
    agent
        .store
        .insert_subagent("worker-b", "B", "test", "prime", "chat-b")
        .unwrap();
    let (stub, stub_task) = serve(crate::http::stub_router(agent.clone())).await;
    let client = reqwest::Client::new();
    let send = |token: String| {
        client
            .post(format!("{stub}/v1/chat/completions"))
            .bearer_auth(token)
            .json(&json!({"model":"m"}))
            .send()
    };
    let main_token = agent.cfg.model_proxy_token(None);
    agent.test_set_pi(None, "chat-a").await;
    assert!(send(main_token.clone())
        .await
        .unwrap()
        .status()
        .is_success());
    let first_context = agent.model_usage_context(None).await.unwrap();
    agent.test_set_pi(None, "chat-b").await;
    agent
        .store
        .set_chat_session_path("chat-b", "/sessions/rotated-chat-b.jsonl")
        .unwrap();
    assert!(send(main_token).await.unwrap().status().is_success());
    assert_eq!(first_context.session_id.as_deref(), Some("chat-a"));
    let (a, b) = tokio::join!(
        send(agent.cfg.model_proxy_token(Some("worker-a"))),
        send(agent.cfg.model_proxy_token(Some("worker-b")))
    );
    assert!(a.unwrap().status().is_success());
    assert!(b.unwrap().status().is_success());
    let old_context = agent.model_usage_context(Some("worker-a")).await.unwrap();
    agent
        .store
        .insert_job(
            "old-job",
            "chat-a",
            "old-turn",
            "Old work",
            "ops",
            "old brief",
            &[],
        )
        .unwrap();
    agent.store.set_job_subagent("old-job", "worker-a").unwrap();
    agent.store.set_job_state("old-job", "done", None).unwrap();
    // A wake keeps the logical worker but its costs now belong beside a different chat.
    agent
        .store
        .t3_append(
            "worker-a",
            Some("worker-a"),
            Some("worker-a"),
            "user",
            Some("user"),
            "New work",
            Some(&json!({"chat_id":"chat-b", "turn_id":"new-turn", "job_id":null})),
        )
        .unwrap();
    assert!(send(agent.cfg.model_proxy_token(Some("worker-a")))
        .await
        .unwrap()
        .status()
        .is_success());
    assert_eq!(old_context.parent_session_id.as_deref(), Some("chat-a"));
    // An old child keeps working but cannot be attributed with its shared box credential.
    assert!(send(agent.cfg.box_token.clone())
        .await
        .unwrap()
        .status()
        .is_success());
    assert_eq!(
        send("arbos-model:worker:worker-a:forged".into())
            .await
            .unwrap()
            .status(),
        401
    );
    let rows = received.lock().await;
    assert_eq!(rows.len(), 6);
    assert_eq!(rows[0]["x-arbos-session"], "chat-a");
    assert_eq!(rows[1]["x-arbos-session"], "chat-b");
    assert_eq!(rows[1]["x-arbos-session-run"], "rotated-chat-b");
    for (worker, parent) in [("worker-a", "chat-a"), ("worker-b", "chat-b")] {
        let row = rows
            .iter()
            .find(|r| r.get("x-arbos-session").is_some_and(|v| v == worker))
            .unwrap();
        assert_eq!(row["x-arbos-parent-session"], parent);
        assert_eq!(
            row["authorization"],
            format!("Bearer {}", agent.cfg.box_token)
        );
        assert_eq!(row["x-arbos-project"], agent.cfg.project_id);
    }
    assert_eq!(rows[4]["x-arbos-session"], "worker-a");
    assert_eq!(rows[4]["x-arbos-parent-session"], "chat-b");
    assert_eq!(rows[4]["x-arbos-turn"], "new-turn");
    assert!(
        rows[4].get("x-arbos-job").is_none(),
        "a plain wake must not inherit an old job"
    );
    assert!(rows[5].get("x-arbos-session").is_none());
    drop(rows);
    stub_task.abort();
    relay_task.abort();
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn model_usage_context_is_sent_by_internal_llm_calls() {
    let received = Arc::new(Mutex::new(Vec::<HeaderMap>::new()));
    let app = Router::new().route("/proxy/v1/chat/completions", post(
        |State(received): State<Arc<Mutex<Vec<HeaderMap>>>>, headers: HeaderMap| async move {
            received.lock().await.push(headers);
            Json(json!({"choices":[{"message":{"content":"{}"}}], "usage": {"prompt_tokens":1,"completion_tokens":1}}))
        }
    )).with_state(received.clone());
    let (relay, relay_task) = serve(app).await;
    let (agent, root) = crate::status::tests::test_agent_on_relay(&relay).await;
    let llm = agent
        .llm
        .with_context(arbos_core::usage::UsageContext::for_chat("chat"))
        .with_purpose("memory-write");
    llm.complete("m", "system", "text", 10).await.unwrap();
    agent
        .store
        .insert_subagent("worker", "W", "test", "pi", "chat-a")
        .unwrap();
    let original_context = agent.store.usage_context("worker").unwrap();
    agent
        .store
        .t3_append(
            "worker",
            Some("worker"),
            Some("worker"),
            "user",
            Some("user"),
            "Wake",
            Some(&json!({"chat_id":"chat-b", "job_id":null})),
        )
        .unwrap();
    agent
        .memory
        .write_pass(
            "old-event",
            "worker",
            original_context,
            "subagent",
            "The old task finished in chat A and is being indexed later.",
            &[],
        )
        .await
        .unwrap();
    let rows = received.lock().await;
    assert_eq!(rows[0]["x-arbos-session"], "chat");
    assert_eq!(rows[0]["x-arbos-project"], agent.cfg.project_id);
    assert_eq!(rows[0]["x-arbos-purpose"], "memory-write");
    assert_eq!(rows[1]["x-arbos-session"], "worker");
    assert_eq!(
        rows[1]["x-arbos-parent-session"], "chat-a",
        "background indexing keeps the completed run's origin"
    );
    drop(rows);
    relay_task.abort();
    let _ = std::fs::remove_dir_all(root);
}
