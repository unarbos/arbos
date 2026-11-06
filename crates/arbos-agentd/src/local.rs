//! User-computer tools (F9): `local_devices`, `local_run`, `local_read`, `local_list`. Every op
//! is gated: it reaches the relay (`POST /projects/{id}/local/{device}/exec`) only with an
//! approved outcome from the approval shape (risk `computer`, the exact command or path in the
//! card), or — for reads only — when the user switched that device to "allow reads without
//! asking". Every call is audited; big outputs become handles.

use std::sync::Arc;

use base64::Engine;
use serde_json::{json, Value};

use arbos_core::local::{LocalOp, LocalResult};

use crate::agent::Agent;
use crate::connectors::{request_approval, ApprovalOutcome, ApprovalRequest};
use crate::mcp::{err_result, materialize_handle, text_result};

const INLINE_MAX: usize = 16_000;

pub fn tool_specs() -> Vec<Value> {
    vec![
        json!({ "name": "local_devices", "description": "The user's own computers that run the arbos-local helper (name, os, home, online). Needed before local_run / local_read / local_list; pass the device id.",
                "inputSchema": { "type": "object", "properties": {} } }),
        json!({ "name": "local_run", "description": "Run a shell command on the user's computer. The user sees the exact command in an approval card and must approve it first; the call fails when refused. Output is capped; big outputs come back as a handle for peek.",
                "inputSchema": { "type": "object", "properties": { "device": { "type": "string", "description": "device id from local_devices" }, "cmd": { "type": "string" }, "cwd": { "type": "string" }, "timeout_s": { "type": "integer", "minimum": 1, "maximum": 600 } }, "required": ["device", "cmd"] } }),
        json!({ "name": "local_read", "description": "Read one file from the user's computer (≤ 1 MiB inline; bigger files come as a handle). Asks the user first unless they allowed reads for that device.",
                "inputSchema": { "type": "object", "properties": { "device": { "type": "string" }, "path": { "type": "string" }, "max_bytes": { "type": "integer" } }, "required": ["device", "path"] } }),
        json!({ "name": "local_list", "description": "List a directory on the user's computer. Asks the user first unless they allowed reads for that device.",
                "inputSchema": { "type": "object", "properties": { "device": { "type": "string" }, "path": { "type": "string" } }, "required": ["device", "path"] } }),
    ]
}

/// Why an op may proceed. Produced only by the gate below, so an exec request cannot be built
/// without going through it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Clearance {
    Approved,
    ReadAllowedByUser,
}

/// The one rule (invariant, unit-tested): no approved outcome ⇒ refused, except a read on a
/// device whose owner switched on "allow reads without asking".
pub fn gate(
    op: &LocalOp,
    outcome: Option<&ApprovalOutcome>,
    device_allows_reads: bool,
) -> Result<Clearance, String> {
    if op.is_read() && device_allows_reads && outcome.is_none() {
        return Ok(Clearance::ReadAllowedByUser);
    }
    match outcome {
        Some(ApprovalOutcome::Approved) => Ok(Clearance::Approved),
        Some(ApprovalOutcome::Rejected { reason }) => {
            Err(format!("{} not approved: {reason}", op.name()))
        }
        Some(ApprovalOutcome::Undecided) => Err(format!(
            "{} did not run: it needs the user's approval and the card is still waiting for \
             them. Do not retry it",
            op.name()
        )),
        Some(ApprovalOutcome::Cancelled) => Err(format!(
            "{} did not run: the work was cancelled, so its approval card was closed",
            op.name()
        )),
        None => Err(format!(
            "{} refused: the user's approval is required before anything runs on their computer",
            op.name()
        )),
    }
}

async fn relay_devices(agent: &Agent) -> Result<Vec<Value>, String> {
    let r = agent
        .http_client()
        .get(format!("{}/local/devices", agent.cfg.relay_url))
        .bearer_auth(&agent.cfg.box_token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = r.status();
    let text = r.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "relay /local/devices: HTTP {status} {}",
            crate::relay_error::message(&text)
        ));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok(v["items"].as_array().cloned().unwrap_or_default())
}

pub async fn local_devices(agent: &Agent) -> Value {
    match relay_devices(agent).await {
        Ok(items) => text_result(
            json!({
                "devices": items,
                "note": "Every command needs the user's approval in the chat; reads too unless the device allows them. Use the device_id with local_run / local_read / local_list.",
            })
            .to_string(),
        ),
        Err(e) => err_result(e),
    }
}

fn op_from_args(name: &str, args: &Value) -> Result<LocalOp, String> {
    let s = |k: &str| {
        args.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    match name {
        "local_run" => Ok(LocalOp::Run {
            cmd: s("cmd").ok_or("cmd is required")?,
            cwd: s("cwd"),
            timeout_s: args.get("timeout_s").and_then(Value::as_u64),
        }),
        "local_read" => Ok(LocalOp::ReadFile {
            path: s("path").ok_or("path is required")?,
            max_bytes: args
                .get("max_bytes")
                .and_then(Value::as_u64)
                .map(|n| n as usize),
        }),
        "local_list" => Ok(LocalOp::ListDir {
            path: s("path").ok_or("path is required")?,
        }),
        _ => Err(format!("unknown local tool {name}")),
    }
}

/// `local_run` / `local_read` / `local_list`.
pub async fn local_call(agent: &Arc<Agent>, name: &str, args: &Value, chat_hint: &str) -> Value {
    let device = args["device"].as_str().unwrap_or("").trim().to_owned();
    if device.is_empty() {
        return err_result("device is required (see local_devices)");
    }
    let op = match op_from_args(name, args) {
        Ok(op) => op,
        Err(e) => return err_result(e),
    };
    let devices = match relay_devices(agent).await {
        Ok(d) => d,
        Err(e) => return err_result(e),
    };
    let Some(dev) = devices
        .iter()
        .find(|d| d["device_id"].as_str() == Some(device.as_str()))
    else {
        return err_result(format!(
            "unknown device {device}; call local_devices for the list"
        ));
    };
    let dev_name = dev["name"].as_str().unwrap_or("computer").to_owned();
    let allows_reads = dev["allow_reads"].as_bool().unwrap_or(false);
    if !dev["connected"].as_bool().unwrap_or(false) {
        return err_result(format!(
            "{dev_name} is offline (arbos-local is not connected); ask the user to start it"
        ));
    }

    let chat_id = agent
        .approval_chat(chat_hint)
        .await
        .unwrap_or_else(|| chat_hint.to_owned());
    let chat_id = chat_id.as_str();
    let outcome = if op.is_read() && allows_reads {
        None
    } else if let Some(granted) = agent.take_grant(name, args).await {
        // A late approval of this exact call (tool + args) re-ran it.
        tracing::info!(
            tool = name,
            approval_id = granted,
            "pre-approved local call"
        );
        Some(ApprovalOutcome::Approved)
    } else {
        Some(
            request_approval(
                agent,
                chat_id,
                ApprovalRequest {
                    title: match &op {
                        LocalOp::Run { .. } => format!("Run on {dev_name}"),
                        LocalOp::ReadFile { .. } => format!("Read a file on {dev_name}"),
                        LocalOp::ListDir { .. } => format!("List a folder on {dev_name}"),
                    },
                    summary: op.subject(),
                    risk: "computer",
                    payload: json!({ "kind": "tool_call", "tool": name, "input": args, "class": "local-computer",
                                     "why": "touches the user's own computer", "device": device, "deviceName": dev_name, "op": op }),
                },
            )
            .await,
        )
    };
    let clearance = match gate(&op, outcome.as_ref(), allows_reads) {
        Ok(c) => c,
        Err(e) => {
            agent.append_audit(
                "local_refused",
                json!({ "chatId": chat_id, "device": device, "op": op.name(), "subject": op.subject(), "reason": e }),
            );
            return err_result(e);
        }
    };
    agent.append_audit(
        "local_exec",
        json!({ "chatId": chat_id, "device": device, "deviceName": dev_name, "op": op.name(), "subject": op.subject(),
                "clearance": match &clearance { Clearance::Approved => "approved", Clearance::ReadAllowedByUser => "reads_allowed" } }),
    );
    let _ = agent.store.t3_append(
        chat_id,
        None,
        Some(chat_id),
        "local_exec",
        Some("assistant"),
        &format!("{} on {dev_name}: {}", op.name(), op.subject()),
        None,
    );

    let r = agent
        .http_client()
        .post(format!(
            "{}/projects/{}/local/{}/exec",
            agent.cfg.relay_url, agent.cfg.project_id, device
        ))
        .bearer_auth(&agent.cfg.box_token)
        .json(&op)
        .send()
        .await;
    let (status, text) = match r {
        Ok(r) => {
            let st = r.status();
            (st, r.text().await.unwrap_or_default())
        }
        Err(e) => return err_result(format!("relay unreachable: {e}")),
    };
    let result: LocalResult = match serde_json::from_str(&text) {
        Ok(r) => r,
        Err(_) if !status.is_success() => {
            return err_result(format!(
                "{} on {dev_name}: HTTP {status} {}",
                op.name(),
                crate::relay_error::message(&text)
            ))
        }
        Err(e) => return err_result(format!("bad result from relay: {e}")),
    };
    agent.append_audit(
        "local_result",
        json!({ "chatId": chat_id, "device": device, "op": op.name(), "subject": op.subject(), "summary": summarize(&result) }),
    );
    render(agent, chat_id, &dev_name, &op, result).await
}

fn summarize(r: &LocalResult) -> Value {
    match r {
        LocalResult::Run {
            exit,
            stdout,
            stderr,
            timed_out,
            duration_ms,
            ..
        } => {
            json!({ "exit": exit, "stdoutBytes": stdout.len(), "stderrBytes": stderr.len(), "timedOut": timed_out, "ms": duration_ms })
        }
        LocalResult::File {
            size,
            blob,
            truncated,
            ..
        } => json!({ "size": size, "blob": blob, "truncated": truncated }),
        LocalResult::Dir {
            entries, truncated, ..
        } => json!({ "entries": entries.len(), "truncated": truncated }),
        LocalResult::Error { message } => json!({ "error": message }),
    }
}

async fn render(
    agent: &Agent,
    chat_id: &str,
    dev_name: &str,
    op: &LocalOp,
    result: LocalResult,
) -> Value {
    match result {
        LocalResult::Error { message } => {
            err_result(format!("{} on {dev_name}: {message}", op.name()))
        }
        LocalResult::Run {
            exit,
            stdout,
            stderr,
            stdout_truncated,
            stderr_truncated,
            timed_out,
            duration_ms,
        } => {
            let mut out = json!({ "device": dev_name, "exit": exit, "timed_out": timed_out, "duration_ms": duration_ms,
                                  "stdout_truncated": stdout_truncated, "stderr_truncated": stderr_truncated });
            offload(
                agent,
                chat_id,
                &mut out,
                "stdout",
                stdout,
                &format!("{} stdout", op.subject()),
            );
            offload(
                agent,
                chat_id,
                &mut out,
                "stderr",
                stderr,
                &format!("{} stderr", op.subject()),
            );
            text_result(out.to_string())
        }
        LocalResult::File {
            path,
            size,
            text,
            bytes_b64,
            blob,
            truncated,
        } => {
            let mut out =
                json!({ "device": dev_name, "path": path, "size": size, "truncated": truncated });
            if let Some(t) = text {
                offload(agent, chat_id, &mut out, "text", t, &path);
            } else if let Some(b64) = bytes_b64 {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(b64.as_bytes())
                    .unwrap_or_default();
                let handle =
                    materialize_handle(agent, chat_id, &path, &String::from_utf8_lossy(&bytes));
                out["binary"] = json!(true);
                out["handle"] = json!(handle);
            }
            if let Some(key) = blob {
                // The helper put the whole file in the relay blob store; pull it into a handle.
                match fetch_blob(agent, &key).await {
                    Ok(full) => {
                        let handle = materialize_handle(agent, chat_id, &path, &full);
                        out["handle"] = json!(handle);
                        out["note"] = json!("full file by handle (peek)");
                    }
                    Err(e) => out["blob_error"] = json!(e),
                }
            }
            text_result(out.to_string())
        }
        LocalResult::Dir {
            path,
            entries,
            truncated,
        } => {
            let listing = entries
                .iter()
                .map(|e| {
                    let size = e.size.map(|s| format!("\t{s}")).unwrap_or_default();
                    format!("{}\t{}{}", e.kind, e.name, size)
                })
                .collect::<Vec<_>>()
                .join("\n");
            let mut out = json!({ "device": dev_name, "path": path, "count": entries.len(), "truncated": truncated });
            offload(agent, chat_id, &mut out, "entries", listing, &path);
            text_result(out.to_string())
        }
    }
}

/// Put `text` inline under `key`, or as a handle plus preview when it is big.
fn offload(agent: &Agent, chat_id: &str, out: &mut Value, key: &str, text: String, note: &str) {
    if text.len() > INLINE_MAX {
        let handle = materialize_handle(agent, chat_id, note, &text);
        out[key] = json!(clip(&text, 4000));
        out[format!("{key}_handle")] = json!(handle);
    } else {
        out[key] = json!(text);
    }
}

async fn fetch_blob(agent: &Agent, key: &str) -> Result<String, String> {
    let r = agent
        .http_client()
        .get(format!(
            "{}/blob/{}/{}",
            agent.cfg.relay_url, agent.cfg.project_id, key
        ))
        .bearer_auth(&agent.cfg.box_token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("blob {key}: HTTP {}", r.status()));
    }
    let bytes = r.bytes().await.map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run() -> LocalOp {
        LocalOp::Run {
            cmd: "uname -a".into(),
            cwd: None,
            timeout_s: None,
        }
    }

    fn read() -> LocalOp {
        LocalOp::ReadFile {
            path: "~/notes.txt".into(),
            max_bytes: None,
        }
    }

    /// Invariant (F9): an exec with no approved approval is refused by agentd.
    #[test]
    fn exec_without_approval_is_refused() {
        assert!(gate(&run(), None, false).is_err());
        assert!(
            gate(&run(), None, true).is_err(),
            "allow_reads never covers run"
        );
        assert!(gate(
            &run(),
            Some(&ApprovalOutcome::Rejected {
                reason: "no".into()
            }),
            true
        )
        .is_err());
        // An undecided card — the wait ended, the card is still open — refuses exactly as a
        // refusal does. This is the case that used to arrive as "expired".
        assert!(gate(&run(), Some(&ApprovalOutcome::Undecided), true).is_err());
        assert!(gate(&run(), Some(&ApprovalOutcome::Cancelled), true).is_err());
        assert!(gate(&read(), None, false).is_err());
        assert!(gate(
            &read(),
            Some(&ApprovalOutcome::Rejected {
                reason: "no".into()
            }),
            false
        )
        .is_err());
    }

    #[test]
    fn approved_or_user_allowed_reads_pass() {
        assert_eq!(
            gate(&run(), Some(&ApprovalOutcome::Approved), false),
            Ok(Clearance::Approved)
        );
        assert_eq!(gate(&read(), None, true), Ok(Clearance::ReadAllowedByUser));
        assert_eq!(
            gate(&LocalOp::ListDir { path: "~".into() }, None, true),
            Ok(Clearance::ReadAllowedByUser)
        );
    }
}
