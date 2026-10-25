//! The rollout view's document for one job (`GET /w/{project}/jobs/{job_id}?after=<seq>`, served
//! to the relay, which passes it through as `GET /projects/{id}/jobs/{jobId}`): the brief, the
//! job's own exchange — prompt, text, tool steps with plain-words labels and durations, steers,
//! the result — its state and step, built from the `jobs` row and the worker's T3 thread.
//!
//! Why the box and not a relay mirror: the transcript already lives here (T3 rows of the job's
//! subagent id, written as the worker runs); the relay holds no copy of any box's T3 and would
//! have to receive, store and age a second one. The relay reaches the box the way `inspect`
//! does (`/w/{project}/…` with the MCP token); a finished job stays readable while the box is
//! awake, which is the chat's lifetime for v1. The same builder serves a named worker or an
//! agent-to-agent thread later (`document_for_thread` keyed by subagent id).

use serde_json::{json, Value};

use crate::store::{JobRow, Store, T3Row};

/// Most T3 rows read for one document; a job's thread is a few hundred rows at most.
const MAX_ROWS: i64 = 5_000;
/// Longest text carried per entry (the raw result is already clipped to 7,000 chars upstream).
const TEXT_MAX: usize = 7_000;

/// `None` when there is no such job.
pub fn document(
    store: &Store,
    project_id: &str,
    project_name: &str,
    job_id: &str,
    after: i64,
) -> Option<Value> {
    let job = store.job(job_id).ok().flatten()?;
    let (entries, total, tool_calls) = match &job.subagent_id {
        Some(sid) => entries_for_thread(store, sid, &job, after),
        None => (Vec::new(), 0, 0),
    };
    Some(json!({
        "id": job.id,
        "title": job.title,
        "kind": job.kind,
        "state": job.state,
        "step": job.last_step,
        "elapsed_s": job.elapsed_s(),
        "started_at": job.started_at,
        "ended_at": job.ended_at,
        "chat_id": job.chat_id,
        "ask_message_id": job.ask_message_id,
        "parent": { "project_id": project_id, "name": project_name },
        "brief": job.brief,
        "error": job.error,
        "entries": entries,
        "tool_calls": tool_calls,
        "next_seq": total + 1,
    }))
}

/// The entries of one worker thread, numbered 1.. in T3 order; only those with `seq > after`
/// are returned, with the total count and the number of tool steps.
pub fn entries_for_thread(
    store: &Store,
    thread_id: &str,
    job: &JobRow,
    after: i64,
) -> (Vec<Value>, i64, i64) {
    let rows = store
        .t3_thread_in_order(thread_id, MAX_ROWS)
        .unwrap_or_default();
    let final_state = !job.is_open();
    let last_assistant = rows
        .iter()
        .rposition(|(r, _)| r.kind == "assistant" && r.role.as_deref() == Some("assistant"));
    let mut entries: Vec<Value> = Vec::new();
    let mut seq: i64 = 0;
    let mut tool_calls: i64 = 0;
    for (i, (row, source)) in rows.iter().enumerate() {
        let entry = match row.kind.as_str() {
            // the brief as sent to the worker; a restart after a box reboot sends it again
            "user" => Some(json!({ "kind": "prompt", "text": clip(&row.text) })),
            "steer" => Some(json!({ "kind": "steer", "text": clip(&row.text) })),
            "tool_call" => {
                tool_calls += 1;
                Some(tool_entry(row, source.as_ref(), &rows[i + 1..]))
            }
            "assistant" if row.role.as_deref() == Some("assistant") => {
                if Some(i) == last_assistant && final_state {
                    let duration_ms = match (job.started_at, job.ended_at) {
                        (Some(s), Some(e)) => Some(e - s),
                        _ => None,
                    };
                    Some(
                        json!({ "kind": "result", "text": clip(&row.text), "status": job.state, "duration_ms": duration_ms }),
                    )
                } else {
                    Some(json!({ "kind": "text", "text": clip(&row.text) }))
                }
            }
            _ => None, // tool_output (folded into its call), compaction, interim
        };
        if let Some(mut e) = entry {
            seq += 1;
            e["seq"] = json!(seq);
            e["at"] = json!(row.at);
            if seq > after {
                entries.push(e);
            }
        }
    }
    (entries, seq, tool_calls)
}

/// A tool step from its `tool_call` row and the `tool_output` row that answers it (matched by
/// `call_id` when the rows carry one, else the next output row), with the plain-words label the
/// status line uses and the elapsed time between the two rows.
fn tool_entry(call: &T3Row, source: Option<&Value>, later: &[(T3Row, Option<Value>)]) -> Value {
    let (name, input) = split_call(&call.text);
    let label = crate::activity::label(&name, &input)
        .unwrap_or_else(|| crate::agent::step_label(&name, &input));
    let call_id = source
        .and_then(|s| s.get("call_id"))
        .and_then(Value::as_str);
    let output = later.iter().find(|(r, s)| {
        r.kind == "tool_output"
            && match (
                call_id,
                s.as_ref()
                    .and_then(|s| s.get("call_id"))
                    .and_then(Value::as_str),
            ) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            }
    });
    match output {
        Some((out, out_src)) => {
            let is_error = out_src
                .as_ref()
                .and_then(|s| s.get("is_error"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            json!({
                "kind": "tool", "name": name, "label": label,
                "status": if is_error { "error" } else { "done" },
                "duration_ms": (out.at - call.at).max(0),
            })
        }
        None => json!({ "kind": "tool", "name": name, "label": label, "status": "running" }),
    }
}

/// `"{name} {json input}"` as `run_subagent` writes it → the name and the parsed input.
fn split_call(text: &str) -> (String, Value) {
    let t = text.trim();
    let (name, rest) = match t.find(' ') {
        Some(i) => (&t[..i], t[i + 1..].trim()),
        None => (t, ""),
    };
    let input = serde_json::from_str::<Value>(rest).unwrap_or(Value::Null);
    (name.to_owned(), input)
}

fn clip(text: &str) -> String {
    let t = text.trim();
    if t.chars().count() <= TEXT_MAX {
        t.to_owned()
    } else {
        let mut s: String = t.chars().take(TEXT_MAX).collect();
        s.push('…');
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The document the rollout view draws: prompt, a finished and a running tool step with
    /// labels and durations, a steer, the result; `?after` returns only newer entries.
    #[test]
    fn job_document_from_t3_rows() {
        let dir = std::env::temp_dir().join(format!("arbos-jobs-view-{}", arbos_core::ids::ulid()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("state.sqlite")).unwrap();
        store
            .insert_job(
                "job-1",
                "chat-1",
                "ask-1",
                "Searching for surf schools",
                "research",
                "Find the 3 best-reviewed surf schools near Santa Teresa and compare prices.",
                &[],
            )
            .unwrap();
        store.set_job_subagent("job-1", "sub-1").unwrap();
        store.set_job_state("job-1", "running", None).unwrap();
        store
            .set_job_step("job-1", "Reading delsoulsurf.com")
            .unwrap();
        let t = |kind: &str, role: &str, text: &str, src: Option<Value>| {
            store
                .t3_append(
                    "sub-1",
                    Some("sub-1"),
                    Some("sub-1"),
                    kind,
                    Some(role),
                    text,
                    src.as_ref(),
                )
                .unwrap()
        };
        t(
            "user",
            "user",
            "Find the 3 best-reviewed surf schools near Santa Teresa and compare prices.",
            None,
        );
        t(
            "tool_call",
            "assistant",
            r#"web_search {"query":"surf schools santa teresa"}"#,
            Some(json!({ "call_id": "c1" })),
        );
        t(
            "tool_output",
            "tool",
            "web_search: 10 results",
            Some(json!({ "call_id": "c1", "is_error": false })),
        );
        t(
            "steer",
            "user",
            "only schools with group lessons under $70",
            None,
        );
        t(
            "tool_call",
            "assistant",
            r#"web_fetch {"url":"https://delsoulsurf.com/lessons"}"#,
            Some(json!({ "call_id": "c2" })),
        );
        let doc = document(&store, "proj", "Affine", "job-1", 0).unwrap();
        assert_eq!(doc["state"], "running");
        assert_eq!(doc["step"], "Reading delsoulsurf.com");
        assert!(doc["brief"].as_str().unwrap().len() > 20);
        let entries = doc["entries"].as_array().unwrap();
        let kinds: Vec<&str> = entries
            .iter()
            .map(|e| e["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, vec!["prompt", "tool", "steer", "tool"]);
        assert_eq!(
            entries[1]["label"],
            "Searching for surf schools santa teresa"
        );
        assert_eq!(entries[1]["status"], "done");
        assert!(entries[1]["duration_ms"].as_i64().unwrap() >= 0);
        assert_eq!(entries[3]["label"], "Reading delsoulsurf.com");
        assert_eq!(entries[3]["status"], "running");
        assert_eq!(doc["tool_calls"], 2);
        assert_eq!(doc["next_seq"], 5);
        // the job finishes: the worker's final text is the result
        t(
            "tool_output",
            "tool",
            "web_fetch: <html>…",
            Some(json!({ "call_id": "c2", "is_error": false })),
        );
        t(
            "assistant",
            "assistant",
            "Blue Mystic is the best value …",
            None,
        );
        store.set_job_state("job-1", "done", None).unwrap();
        let doc = document(&store, "proj", "Affine", "job-1", 4).unwrap();
        let entries = doc["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1, "only entries after seq 4: {entries:?}");
        assert_eq!(entries[0]["kind"], "result");
        assert_eq!(entries[0]["status"], "done");
        assert_eq!(entries[0]["seq"], 5);
        assert_eq!(doc["next_seq"], 6);
        assert!(document(&store, "proj", "Affine", "job-nope", 0).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
