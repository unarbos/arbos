//! `submit_feedback`: the agent files the user's bug report or feedback itself. It asks the
//! user's signed-in apps for a picture of their screen (relay push `feedback_capture`; the app in
//! the foreground captures its own window and uploads it), or attaches a PNG/JPEG the user
//! already provided (`image_path`, usually a chat attachment under `in/`), gathers the rollout of
//! this chat (recent turns, tool calls, errors and timings from the T3 store), scrubs secrets and
//! secret placeholders out of it, adds platform / version / agent / box facts, and posts
//! everything to the relay's `POST /feedback` as the user's agent (`submitted_by: "agent"`). The
//! relay-side feedback loop turns the item into a tracked issue; when it exists, the relay pushes
//! `feedback_issue` back and the agent leaves one line in the chat with the number.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};

use arbos_core::ids::{now_ms, ulid};

use crate::agent::Agent;
use crate::mcp::{err_result, text_result};

/// How long the tool waits for a device to answer the capture request. The apps poll
/// `/me/pushes` every 10–15 s, then capture and upload.
const CAPTURE_WAIT: Duration = Duration::from_secs(32);
/// Once some other device answered, how much longer to wait for the preferred one (the apps
/// poll every 10–15 s, so the device the request came from needs up to ~20 s to answer).
const PREFERRED_GRACE: Duration = Duration::from_secs(16);
/// A capture taken for this chat is reused by further `submit_feedback` calls within this long
/// (a list of nine complaints becomes nine reports with one screenshot, not nine 30 s waits).
const CAPTURE_REUSE: Duration = Duration::from_secs(150);
const POLL_EVERY: Duration = Duration::from_secs(1);
/// T3 rows of the chat that go into the rollout.
const ROLLOUT_ROWS: i64 = 60;
const ROW_CHARS: usize = 1_200;
const TOOL_OUTPUT_CHARS: usize = 700;
const ROLLOUT_CHARS: usize = 40_000;
/// Same ceiling as the relay's `POST /feedback` image field.
const MAX_ATTACH_BYTES: usize = 15 * 1024 * 1024;

pub fn tool_specs() -> Vec<Value> {
    vec![json!({
        "name": "submit_feedback",
        "description": "File the user's feedback or bug report for them. Filing is irreversible from this chat (you cannot withdraw a report once filed). Use it when the user asks to submit, send, file or report feedback, a bug, a problem, a crash or a suggestion about this app or about you, and they have said what to report. When they only ask to file with no content (for example \"file a feedback report\"), do not call yet — ask what to include, or propose a one-line description and wait for their yes. By default it captures a picture of the user's current screen from their app; when they already attached or saved a PNG/JPEG for the report, pass image_path instead. It also attaches the recent conversation and your tool calls, errors and timings (scrubbed of secrets), adds platform and version details, and files the report. Returns a reference id to give the user. Call it once per report; do not ask the user to use a menu, settings screen or another channel instead.",
        "inputSchema": { "type": "object", "properties": {
            "description": { "type": "string", "description": "What the user reported, in their words plus what you observed (what happened, what they expected). Required. When the user has not said what to report, ask them first; do not invent a report from a bare file request." },
            "expected": { "type": "string", "description": "Optional: what should have happened instead." },
            "include_screenshot": { "type": "boolean", "description": "Default true. false when the user says the screen is not relevant. Ignored when image_path is set (the file replaces the live capture)." },
            "image_path": { "type": "string", "description": "Optional path to a PNG or JPEG the user attached or saved for this report (usually under in/). Attaches that file instead of capturing the live screen. Pass it whenever the user provides their own image." }
        }, "required": ["description"] }
    })]
}

/// A PNG/JPEG the user (or the agent) already has on disk for this report.
struct UserImage {
    bytes: Vec<u8>,
    mime: &'static str,
    label: String,
}

/// Read a project-folder image for `image_path`. Rejects paths outside the workspace and
/// anything that is not a PNG or JPEG within the relay's size limit.
fn read_user_image(a: &Agent, path: &str) -> Result<UserImage, String> {
    let raw = Path::new(path.trim());
    if path.trim().is_empty() {
        return Err("path is empty".into());
    }
    let abs: PathBuf = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        a.cfg.work_dir.join(raw)
    };
    let root = a
        .cfg
        .work_dir
        .canonicalize()
        .unwrap_or_else(|_| a.cfg.work_dir.clone());
    let canon = abs
        .canonicalize()
        .map_err(|e| format!("{}: {e}", abs.display()))?;
    if !canon.starts_with(&root) {
        return Err("path must be inside the project folder".into());
    }
    let meta = std::fs::metadata(&canon).map_err(|e| format!("{}: {e}", canon.display()))?;
    if !meta.is_file() {
        return Err(format!("{} is not a file", canon.display()));
    }
    if meta.len() as usize > MAX_ATTACH_BYTES {
        return Err(format!(
            "{} is {} MB; the limit is 15 MB",
            canon.display(),
            meta.len() / (1024 * 1024)
        ));
    }
    let bytes = std::fs::read(&canon).map_err(|e| format!("{}: {e}", canon.display()))?;
    let mime = match bytes.get(..4) {
        Some([0x89, b'P', b'N', b'G']) => "image/png",
        Some([0xFF, 0xD8, ..]) => "image/jpeg",
        _ => return Err("not a PNG or JPEG".into()),
    };
    let label = canon
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "image".into());
    Ok(UserImage { bytes, mime, label })
}

/// Secret values (D19 redaction) and secret placeholders / vault references, so a report never
/// carries anything the owner's secrets system handed the agent.
pub(crate) fn scrub(text: &str) -> String {
    arbos_core::redact::redact_placeholders(&crate::store::redact(text))
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}… [{} more chars]", s.chars().count() - max)
    }
}

fn iso(ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .map(|t| t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .unwrap_or_else(|| ms.to_string())
}

/// The rollout: this chat's recent T3 events (user, assistant, tool calls and outputs, errors)
/// with absolute times and the gap from the previous event, then the chat's turn timings.
fn rollout(a: &Agent, chat_id: &str) -> String {
    let mut out = String::new();
    let rows = a
        .store
        .t3_thread(chat_id, None, None, ROLLOUT_ROWS)
        .unwrap_or_default();
    out.push_str(&format!(
        "## Rollout — chat `{chat_id}` ({} most recent events, oldest first)\n\n",
        rows.len()
    ));
    let mut prev: Option<i64> = None;
    for r in &rows {
        let gap = match prev {
            Some(p) => format!(" (+{:.1}s)", (r.at - p).max(0) as f64 / 1000.0),
            None => String::new(),
        };
        prev = Some(r.at);
        let who = match (r.kind.as_str(), r.role.as_deref()) {
            ("user", _) => "user".to_owned(),
            ("assistant", _) => "agent".to_owned(),
            ("tool_call", _) => "tool call".to_owned(),
            ("tool_output", _) => "tool result".to_owned(),
            (k, Some(role)) => format!("{k} ({role})"),
            (k, None) => k.to_owned(),
        };
        let max = if r.kind == "tool_output" {
            TOOL_OUTPUT_CHARS
        } else {
            ROW_CHARS
        };
        let text = clip(r.text.trim(), max).replace('\n', "\n  ");
        let flag = if r.kind == "tool_output"
            && (r.text.starts_with("error") || r.text.contains(": error:"))
        {
            " ⚠"
        } else {
            ""
        };
        out.push_str(&format!("- {}{gap} · {who}{flag}:\n  {text}\n", iso(r.at)));
    }
    let metrics: Vec<Value> = a
        .store
        .turn_metrics(200)
        .unwrap_or_default()
        .into_iter()
        .filter(|m| m["chat_id"].as_str() == Some(chat_id))
        .take(6)
        .collect();
    if !metrics.is_empty() {
        out.push_str("\n## Turn timings (newest first)\n\n| when | route | first token | route | total | tool calls |\n|---|---|---|---|---|---|\n");
        for m in &metrics {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} ms | {} |\n",
                iso(m["created_at"].as_i64().unwrap_or(0)),
                m["route"].as_str().unwrap_or("-"),
                m["ttft_ms"]
                    .as_i64()
                    .map(|v| format!("{v} ms"))
                    .unwrap_or_else(|| "-".into()),
                m["route_ms"]
                    .as_i64()
                    .map(|v| format!("{v} ms"))
                    .unwrap_or_else(|| "-".into()),
                m["total_ms"].as_i64().unwrap_or(0),
                m["tool_calls"].as_i64().unwrap_or(0),
            ));
        }
    }
    clip(&scrub(&out), ROLLOUT_CHARS)
}

/// The device whose message started this turn, so its screen is the one asked for first: the
/// `deviceId` on the user row (iPhone), else the `issuedBy` of the `send` command that named the
/// message (Mac).
async fn preferred_device(a: &Agent, chat_id: &str) -> Option<String> {
    let user_msg_id = a
        .turn
        .lock()
        .await
        .as_ref()
        .map(|t| t.user_msg_id.clone())?;
    let host = a.sync.host(chat_id).await?;
    let h = host.lock().await;
    let on_row = h.doc.message(&user_msg_id).and_then(|m| {
        m.rest
            .get("deviceId")
            .and_then(Value::as_str)
            .filter(|d| !d.is_empty())
            .map(str::to_owned)
    });
    on_row.or_else(|| {
        h.doc
            .commands()
            .into_iter()
            .rev()
            .find(|c| {
                c.kind == "send"
                    && c.payload
                        .get("messageId")
                        .or_else(|| c.payload.get("message_id"))
                        .and_then(Value::as_str)
                        == Some(user_msg_id.as_str())
            })
            .map(|c| c.issued_by)
            .filter(|d| !d.is_empty())
    })
}

/// chat id → the last capture and when it was taken (see `CAPTURE_REUSE`).
fn recent_captures() -> &'static std::sync::Mutex<HashMap<String, (Capture, Instant)>> {
    static RECENT: std::sync::OnceLock<std::sync::Mutex<HashMap<String, (Capture, Instant)>>> =
        std::sync::OnceLock::new();
    RECENT.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

#[derive(Clone)]
struct Capture {
    request_id: String,
    device_id: String,
    has_image: bool,
    platform: String,
    app_version: String,
    os: String,
    device: String,
    screen: String,
    skipped: Option<String>,
}

fn capture_from(request_id: &str, v: &Value) -> Capture {
    let s = |k: &str| v[k].as_str().unwrap_or("").to_owned();
    Capture {
        request_id: request_id.to_owned(),
        device_id: s("device_id"),
        has_image: v["has_image"].as_bool().unwrap_or(false),
        platform: s("platform"),
        app_version: s("app_version"),
        os: s("os"),
        device: s("device"),
        screen: s("screen"),
        skipped: v["skipped"].as_str().map(str::to_owned),
    }
}

/// Ask the relay to push a capture request and wait for the devices' answers. `Ok(None)` when no
/// device answered in time; `Err` when the relay refused.
async fn request_capture(
    a: &Arc<Agent>,
    chat_id: &str,
    preferred: Option<&str>,
) -> Result<Option<Capture>, String> {
    let resp = a
        .http_client()
        .post(format!("{}/box/feedback/capture", a.cfg.relay_url))
        .bearer_auth(&a.cfg.box_token)
        .header("x-arbos-project", &a.cfg.project_id)
        .json(&json!({ "project_id": a.cfg.project_id, "chat_id": chat_id, "preferred_device_id": preferred }))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("relay unreachable: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "relay refused the capture request: HTTP {}",
            resp.status()
        ));
    }
    let v: Value = resp.json().await.unwrap_or_default();
    let Some(request_id) = v["request_id"].as_str().map(str::to_owned) else {
        return Err("relay answered without a request id".into());
    };
    if v["devices"].as_u64() == Some(0) {
        return Ok(None);
    }
    let started = Instant::now();
    let mut first_other: Option<Instant> = None;
    while started.elapsed() < CAPTURE_WAIT {
        tokio::time::sleep(POLL_EVERY).await;
        let Ok(r) = a
            .http_client()
            .get(format!(
                "{}/box/feedback/capture/{request_id}",
                a.cfg.relay_url
            ))
            .bearer_auth(&a.cfg.box_token)
            .timeout(Duration::from_secs(10))
            .send()
            .await
        else {
            continue;
        };
        let Ok(st) = r.json::<Value>().await else {
            continue;
        };
        let captures: Vec<Capture> = st["captures"]
            .as_array()
            .map(|c| c.iter().map(|v| capture_from(&request_id, v)).collect())
            .unwrap_or_default();
        if captures.is_empty() {
            continue;
        }
        if let Some(p) = preferred {
            if let Some(c) = captures.iter().find(|c| c.device_id == p && c.has_image) {
                return Ok(Some(c.clone()));
            }
        }
        match captures.iter().find(|c| c.has_image) {
            Some(c) => {
                // Another device answered first: give the preferred one a few more seconds.
                let waited = *first_other.get_or_insert_with(Instant::now);
                if preferred.is_none() || waited.elapsed() >= PREFERRED_GRACE {
                    return Ok(Some(c.clone()));
                }
            }
            None => {
                // Every answer so far is a skip (background / denied). Keep the first for its
                // device facts; if the preferred device skipped, nothing better is coming.
                if preferred
                    .map(|p| captures.iter().any(|c| c.device_id == p))
                    .unwrap_or(false)
                {
                    return Ok(captures.into_iter().next());
                }
            }
        }
    }
    // Deadline: the best we have (an image if any, else the first skip for its device facts).
    let Ok(r) = a
        .http_client()
        .get(format!(
            "{}/box/feedback/capture/{request_id}",
            a.cfg.relay_url
        ))
        .bearer_auth(&a.cfg.box_token)
        .timeout(Duration::from_secs(10))
        .send()
        .await
    else {
        return Ok(None);
    };
    let st: Value = r.json().await.unwrap_or_default();
    let arr = st["captures"].as_array().cloned().unwrap_or_default();
    let pick = arr
        .iter()
        .find(|c| c["has_image"].as_bool().unwrap_or(false))
        .or_else(|| arr.first());
    Ok(pick.map(|v| capture_from(&request_id, v)))
}

/// True when `description` only restates that the user asked to file, with no problem or
/// suggestion named — the agent must ask what to include before filing (irreversible).
fn description_lacks_report_content(description: &str) -> bool {
    let d = description.to_lowercase();
    if d.contains("no specific details")
        || d.contains("no details given")
        || d.contains("nothing specific")
        || d.contains("no details provided")
    {
        return true;
    }
    let asks_to_file = (d.contains("asked") || d.contains("wants") || d.contains("asked me"))
        && (d.contains("file") || d.contains("submit") || d.contains("send"))
        && (d.contains("feedback") || d.contains("bug report") || d.contains("bug"));
    if !asks_to_file {
        return false;
    }
    // Meta-only: they asked to file, but the text never names a problem, change, or topic.
    let has_substance = d.contains(" because ")
        || d.contains(" about ")
        || d.contains("expected")
        || d.contains("should have")
        || d.contains("instead")
        || d.contains("broken")
        || d.contains("crash")
        || d.contains("bug:")
        || d.contains("issue:")
        || d.contains("problem:")
        || d.contains("analysis")
        || d.contains("weakness")
        || d.contains("hole");
    !has_substance
}

pub async fn submit_feedback(a: &Arc<Agent>, args: &Value, chat_id: &str) -> Value {
    let description = args
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_owned();
    if description.is_empty() {
        return err_result(
            "description is empty: ask the user one short question about what went wrong or what they want changed (ask_user), then call submit_feedback again with their answer",
        );
    }
    if description_lacks_report_content(&description) {
        return err_result(
            "description has no report content: the user asked to file but did not say what to include. Ask one short question about what to report (or propose a one-line description and wait for their yes), then call submit_feedback again with their answer",
        );
    }
    let expected = args
        .get("expected")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let user_image = match args
        .get("image_path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(path) => match read_user_image(a, path) {
            Ok(img) => Some(img),
            Err(e) => {
                return err_result(format!(
                    "image_path: {e}. Pass a PNG or JPEG under the project folder (chat attachments land in in/), or omit image_path to capture the live screen."
                ));
            }
        },
        None => None,
    };
    // A user-provided file replaces the live capture; include_screenshot is ignored then.
    let want_shot = user_image.is_none()
        && args
            .get("include_screenshot")
            .and_then(Value::as_bool)
            .unwrap_or(true);

    let preferred = preferred_device(a, chat_id).await;
    let capture = if let Some(ref img) = user_image {
        a.post_activity(chat_id, Some(format!("Attaching {}", img.label)));
        None
    } else if !want_shot {
        a.post_activity(chat_id, Some("Filing your feedback".into()));
        None
    } else {
        a.post_activity(chat_id, Some("Capturing your screen".into()));
        let reused = recent_captures()
            .lock()
            .ok()
            .and_then(|m| m.get(chat_id).cloned())
            .filter(|(c, at)| c.has_image && at.elapsed() < CAPTURE_REUSE)
            .map(|(c, _)| c);
        if reused.is_some() {
            reused
        } else {
            match request_capture(a, chat_id, preferred.as_deref()).await {
                Ok(c) => {
                    if let Some(cap) = c.as_ref().filter(|c| c.has_image) {
                        if let Ok(mut m) = recent_captures().lock() {
                            m.insert(chat_id.to_owned(), (cap.clone(), Instant::now()));
                        }
                    }
                    c
                }
                Err(e) => {
                    tracing::warn!(error = %e, "feedback: capture request failed");
                    None
                }
            }
        }
    };
    a.post_activity(chat_id, Some("Filing your feedback".into()));

    let image_version = std::env::var("ARBOS_IMAGE_VERSION")
        .ok()
        .filter(|v| !v.is_empty() && v != "unknown");
    let agentd_sha = std::env::var("ARBOS_AGENTD_SHA")
        .ok()
        .filter(|v| !v.is_empty() && v != "unknown");
    let (talking_model, subagent_model) = a.live_models().await;
    let turn = {
        let t = a.turn.lock().await;
        t.as_ref().map(|t| {
            json!({
                "user_message_id": t.user_msg_id, "assistant_message_id": t.assistant_msg_id,
                "elapsed_ms": t.started.elapsed().as_millis() as u64, "tool_calls": t.metrics.tool_calls,
                "route": t.route.as_ref().map(|r| format!("{r:?}")),
            })
        })
    };
    let mut context = json!({
        "agent_id": a.cfg.project_id, "agent_name": a.name(),
        "chat_id": chat_id, "box_device_id": a.cfg.device_id,
        "box_image_version": image_version, "agentd_sha": agentd_sha, "agentd_version": env!("CARGO_PKG_VERSION"),
        "talking_model": talking_model, "subagent_model": subagent_model,
        "preferred_device_id": preferred, "filed_at": iso(now_ms()),
        "turn": turn, "screenshot_requested": want_shot,
    });
    let mut platform = "agent".to_owned();
    let mut app_version = String::new();
    let mut os = String::new();
    let (capture_id, capture_device, shot_line) = if let Some(img) = user_image.as_ref() {
        if let Some(map) = context.as_object_mut() {
            map.insert("screenshot".into(), json!(true));
            map.insert("user_image".into(), json!(true));
            map.insert("user_image_name".into(), json!(img.label));
            map.insert("screenshot_skipped".into(), json!("user-provided image"));
        }
        (None, None, format!("user image attached ({})", img.label))
    } else {
        match &capture {
            Some(c) => {
                if let Some(map) = context.as_object_mut() {
                    map.insert("platform".into(), json!(c.platform));
                    map.insert("app_version".into(), json!(c.app_version));
                    map.insert("os".into(), json!(c.os));
                    map.insert("device".into(), json!(c.device));
                    map.insert("screen".into(), json!(c.screen));
                    map.insert("capture_device_id".into(), json!(c.device_id));
                    map.insert("screenshot".into(), json!(c.has_image));
                    map.insert("screenshot_skipped".into(), json!(c.skipped));
                }
                if !c.platform.is_empty() {
                    platform = c.platform.clone();
                }
                app_version = c.app_version.clone();
                os = c.os.clone();
                let line = if c.has_image {
                    format!(
                        "screenshot attached from the {} app",
                        platform_name(&c.platform)
                    )
                } else {
                    format!(
                        "no screenshot ({}); device facts attached",
                        c.skipped.as_deref().unwrap_or("the app could not capture")
                    )
                };
                (Some(c.request_id.clone()), Some(c.device_id.clone()), line)
            }
            None => {
                if let Some(map) = context.as_object_mut() {
                    map.insert("screenshot".into(), json!(false));
                    map.insert(
                        "screenshot_skipped".into(),
                        json!(if want_shot {
                            "no app answered the capture request (none in the foreground, or capture denied)"
                        } else {
                            "not requested"
                        }),
                    );
                }
                (
                    None,
                    None,
                    if want_shot {
                        "no screenshot (no app was in the foreground to capture one)".to_owned()
                    } else {
                        "no screenshot (not requested)".to_owned()
                    },
                )
            }
        }
    };

    let mut note = scrub(&description);
    if let Some(exp) = expected {
        note.push_str("\n\nExpected: ");
        note.push_str(&scrub(exp));
    }
    note.push_str(&format!(
        "\n\n— filed by the user's agent {} from the chat",
        a.name()
    ));
    let note: String = note.chars().take(3_900).collect();
    let rollout_text = rollout(a, chat_id);

    let mut body = json!({
        "note": note, "platform": platform, "app_version": app_version, "os": os,
        "project_id": a.cfg.project_id, "chat_id": chat_id,
        "submitted_by": "agent", "agent_name": a.name(),
        "rollout": rollout_text, "context": context,
        "capture_id": capture_id, "capture_device_id": capture_device,
    });
    if let Some(img) = user_image.as_ref() {
        body["image"] = json!(base64::engine::general_purpose::STANDARD.encode(&img.bytes));
        body["mime"] = json!(img.mime);
    }
    let resp = a
        .http_client()
        .post(format!("{}/feedback", a.cfg.relay_url))
        .bearer_auth(&a.cfg.box_token)
        .header("x-arbos-project", &a.cfg.project_id)
        .json(&body)
        .timeout(Duration::from_secs(60))
        .send()
        .await;
    let v: Value = match resp {
        Ok(r) if r.status().is_success() => r.json().await.unwrap_or_default(),
        Ok(r) => {
            let status = r.status();
            let text = r.text().await.unwrap_or_default();
            a.post_activity(chat_id, None);
            return err_result(format!(
                "the relay refused the report (HTTP {status}: {}). Tell the user it could not be filed right now and that you will try again when they ask.",
                clip(&crate::relay_error::message(&text), 200)
            ));
        }
        Err(e) => {
            a.post_activity(chat_id, None);
            return err_result(format!(
                "could not reach the relay ({e}). Tell the user the report could not be filed right now."
            ));
        }
    };
    a.post_activity(chat_id, None);
    let id = v["id"].as_str().unwrap_or("").to_owned();
    let has_image = v["has_image"].as_bool().unwrap_or(false);
    a.append_audit(
        "feedback_submitted",
        json!({ "feedback_id": id, "chat_id": chat_id, "screenshot": has_image, "capture_id": capture_id,
            "capture_device_id": capture_device, "at": now_ms() }),
    );
    let _ = a.store.t3_append(
        chat_id,
        None,
        Some(chat_id),
        "feedback",
        Some("assistant"),
        &format!("submit_feedback {id}: {}", clip(&description, 300)),
        Some(&json!({ "feedback_id": id, "screenshot": has_image })),
    );
    text_result(
        json!({
            "status": "filed", "reference": id, "screenshot": has_image, "attached": shot_line,
            "rollout_chars": rollout_text.chars().count(),
            "next": format!("Tell the user in one or two sentences: the report is filed, the reference is {id}, {shot_line}, and the tracking number will appear in this chat once it exists. Do not describe the tool or the steps."),
        })
        .to_string(),
    )
}

fn platform_name(p: &str) -> &str {
    match p {
        "ios" | "iphone" => "iPhone",
        "mac" | "macos" => "Mac",
        other => {
            if other.is_empty() {
                "user's"
            } else {
                other
            }
        }
    }
}

/// `feedback_issue` push: the loop opened the issue for a report this agent filed; leave one
/// line in the chat it came from.
pub async fn on_issue_push(agent: &Arc<Agent>, payload: &Value) {
    let number = payload["number"].as_i64().unwrap_or(0);
    let url = payload["url"].as_str().unwrap_or("");
    let feedback_id = payload["feedback_id"].as_str().unwrap_or("");
    let chat_hint = payload["chat_id"].as_str().unwrap_or("").to_owned();
    if number <= 0 || chat_hint.is_empty() {
        return;
    }
    let chat_id = agent
        .approval_chat(&chat_hint)
        .await
        .unwrap_or(chat_hint.clone());
    let text = if url.is_empty() {
        format!("Your feedback is now tracked as #{number} (reference {feedback_id}).")
    } else {
        format!("Your feedback is now tracked as #{number}: {url}")
    };
    // A `system` row with a text part: both apps draw it as one centred muted line (the
    // `note` status part of `append_system_note` is not shown by either transcript).
    // The room may be closed for being idle (#2367): the line opens it, rather than being lost.
    if let Ok(host) = agent.room(&chat_id).await {
        let h = host.lock().await;
        h.doc.append_voice_line(
            "system",
            &ulid(),
            &text,
            &agent.cfg.device_id,
            &format!("agent:{}", agent.cfg.project_id),
            &[("feedbackIssue", json!(number))],
        );
    }
    agent.append_audit(
        "feedback_issue",
        json!({ "feedback_id": feedback_id, "number": number, "url": url, "chat_id": chat_id }),
    );
}
