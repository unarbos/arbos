//! `post_chart`: a chart the agent sends as structured data (`arbos_core::chart`) and the apps
//! draw natively — interactive on a current Mac / iPhone, a picture elsewhere. One row in the
//! target chat (the same `Target` rules as `deliver_file`: its own visible message, never a
//! worker's hidden row): `text` (the caption) → `attachment` (the PNG agentd renders here, what
//! an old client shows) → `chart` (the data, `specVersion`, the markdown table, and the id of
//! the attachment a new client hides). The data lives in the part, under the 32 KB cap; only the
//! PNG is a blob. Honesty rule as for files (#1900): `posted: true` + the message id only after
//! the row read back; any failure says NOT posted.

use std::sync::Arc;

use serde_json::{json, Value};

use arbos_core::chart::{self, Chart};
use arbos_core::chart_render::{picture_png, PictureCredit, PNG_HEIGHT, PNG_WIDTH};
use arbos_core::ids::ulid;

use crate::agent::Agent;
use crate::mcp::{err_result, text_result};
use crate::media::{self, Target};

pub const TOOL_NAME: &str = "post_chart";

pub fn tool_specs() -> Vec<Value> {
    let chart_schema: Value = serde_json::from_str(chart::SCHEMA_JSON).expect("chart schema");
    vec![json!({
        "name": TOOL_NAME,
        "description": "Post a chart into THIS chat as a real message (1:1, group, channel or DM alike): line, area, bar, scatter or pie, drawn natively on Mac and iPhone — the user can hover or touch for values, expand it, save it as a PNG and copy the data; older apps and notifications get a picture. Send the data, not an image: series with points ([x, y]; x a number or an ISO date) or categories + values, axis labels and units. Keep it small: at most 8 series, 500 points per series, 2 000 in total, 100 categories (pie: 12 slices, one series) and 32 KB — bigger data is a CSV through deliver_file. Returns posted: true with the message id; an error means the chart was NOT posted — then say so plainly instead of claiming it was.",
        "inputSchema": {
            "type": "object",
            "required": ["chart", "caption"],
            "properties": {
                "chart": chart_schema,
                "caption": { "type": "string", "maxLength": chart::MAX_CAPTION, "description": "One or two sentences that state the finding (the prose above the chart; also the notification text). Do not repeat the numbers." },
                "chat_id": { "type": "string", "description": "Where to post (default: the chat of this turn or job)" },
                "reply_to": { "type": "string", "description": "Message id this answers (default: the message that started this turn)" },
                "session_id": { "type": "string", "description": "Subagents: your ARBOS_SESSION_ID" }
            }
        }
    })]
}

/// "## Charts" for the main-agent prompt and the subagent pack.
pub fn prompt_section() -> String {
    "## Charts\n\nWhen the user asks for a chart, or a trend, comparison or share reads better drawn than listed, call `post_chart(chart, caption)`: send the data as series and the apps draw a native, interactive chart in THIS chat (a picture where they cannot). Line/area over time or runs, bar for categories, scatter for two measures, pie only for ≤ 6 parts of a whole. Always `y.unit` (e.g. \"%\"), axis labels, a short title, the source in `subtitle`, ≤ 4 series unless asked. `caption` = one or two sentences stating the finding, not the numbers. Never plot with matplotlib and post a picture when `post_chart` can carry the data. Caps: 8 series, 500 points each, 2 000 total, 100 categories, 32 KB — beyond that a CSV via `deliver_file` (the tool says so). Posted only when it returns `posted: true` with a message id; on an error say the chart could not be posted and why.\n\n".to_owned()
}

/// What a successful post left in the chat.
#[derive(Debug, Clone)]
pub struct PostedChart {
    pub chat_id: String,
    pub message_id: String,
    pub chart: Chart,
    pub fallback_ref: String,
    pub fallback_bytes: usize,
}

fn today() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

/// Validate, render the fallback, upload it, write the row, read it back.
pub async fn post(
    a: &Arc<Agent>,
    chart_value: &Value,
    caption: &str,
    chat_id: Option<&str>,
    reply_to: Option<&str>,
    session_id: Option<&str>,
) -> anyhow::Result<PostedChart> {
    let chart = Chart::parse(chart_value).map_err(|e| anyhow::anyhow!("{e}"))?;
    let caption = caption.trim();
    if caption.is_empty() {
        anyhow::bail!("caption is required: one or two sentences that state the finding");
    }
    if caption.chars().count() > chart::MAX_CAPTION {
        anyhow::bail!(
            "caption is longer than {} characters; say the finding in one or two sentences",
            chart::MAX_CAPTION
        );
    }
    let mut target = match media::target(a, session_id).await {
        Some(t) => t,
        None => anyhow::bail!("no running turn or job to post the chart from"),
    };
    if let Some(c) = chat_id.map(str::trim).filter(|c| !c.is_empty()) {
        if c != target.chat_id {
            target = Target {
                chat_id: c.to_owned(),
                ask_id: None,
                main_turn: false,
            };
        }
    }
    let credit = PictureCredit {
        left: format!("{} · Arbos · {}", a.name(), today()),
    };
    let png = picture_png(&chart, &credit).map_err(|e| anyhow::anyhow!("fallback picture: {e}"))?;
    let png_len = png.len();
    let key = media::upload(a, &target.chat_id, png, "image/png").await?;
    let Some(host) = a.sync.host(&target.chat_id).await else {
        anyhow::bail!("chat {} is not open on this box", target.chat_id);
    };
    let msg_id = ulid();
    let att_id = ulid();
    let chart_id = ulid();
    let title = chart
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| "Chart".to_owned());
    let file_name = format!(
        "{}.png",
        title
            .to_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .trim_matches('-')
            .chars()
            .take(48)
            .collect::<String>()
    );
    let file_name = if file_name == ".png" {
        "chart.png".to_owned()
    } else {
        file_name
    };
    let attachment = json!({
        "name": file_name, "mime": "image/png", "ref": key, "bytes": png_len, "kind": "image",
        "title": title, "width": PNG_WIDTH, "height": PNG_HEIGHT, "chartPart": chart_id,
    });
    let chart_part = json!({
        "id": chart_id,
        "kind": "chart",
        "specVersion": chart::SPEC_VERSION,
        "chart": serde_json::to_value(&chart)?,
        "table": chart.table_markdown(),
        "fallback": att_id,
    });
    {
        let h = host.lock().await;
        let ask = reply_to
            .map(str::trim)
            .filter(|r| !r.is_empty() && h.doc.message(r).is_some())
            .map(str::to_owned)
            .or_else(|| target.ask_id.clone())
            .unwrap_or_default();
        let text_part = h
            .doc
            .start_assistant_message(&msg_id, &ask, &a.cfg.device_id);
        h.doc
            .set_message_fields(&msg_id, &[("turnRole", json!("attachment"))]);
        if let Some(attr) = a.people_reply_fields(&target.chat_id) {
            h.doc.set_message_fields(&msg_id, &attr.fields);
        }
        if !ask.is_empty() && a.reply_to_wanted_for(&h.doc, &target.chat_id, &ask) {
            h.doc
                .set_message_fields(&msg_id, &[("replyTo", json!(ask))]);
        }
        h.doc.append_text(&msg_id, &text_part, caption);
        let att_ok = h
            .doc
            .add_part_from_json(
                &msg_id,
                &json!({ "id": att_id, "kind": "attachment", "attachment": attachment }),
            )
            .is_some();
        let chart_ok = h.doc.add_part_from_json(&msg_id, &chart_part).is_some();
        if !att_ok || !chart_ok {
            let _ = h.doc.delete_message(&msg_id);
            anyhow::bail!(
                "message {msg_id} could not take the chart parts in chat {}",
                target.chat_id
            );
        }
        h.doc
            .finish_message(&msg_id, arbos_core::doc::MessageStatus::Done);
        // Read it back: the row must hold both the picture and the data, or this was not a post.
        let ok = h.doc.message(&msg_id).is_some_and(|m| {
            let has_png = m.parts.iter().any(|p| {
                p.get("kind").and_then(Value::as_str) == Some("attachment")
                    && p.pointer("/attachment/ref").and_then(Value::as_str) == Some(key.as_str())
            });
            let has_chart = m.parts.iter().any(|p| {
                p.get("kind").and_then(Value::as_str) == Some("chart")
                    && p.get("specVersion").and_then(Value::as_u64)
                        == Some(chart::SPEC_VERSION as u64)
                    && p.pointer("/chart/kind").and_then(Value::as_str) == Some(chart.kind.as_str())
            });
            has_png && has_chart && m.text() == caption
        });
        if !ok {
            let _ = h.doc.delete_message(&msg_id);
            anyhow::bail!("the chart row did not read back from the chat doc");
        }
    }
    a.note_posted(&target.chat_id, &msg_id).await;
    if target.main_turn {
        if let Some(t) = a.turn.lock().await.as_mut() {
            t.attachments_posted.push(msg_id.clone());
        }
    }
    let _ = a.store.t3_append(
        &target.chat_id,
        None,
        session_id,
        "chart",
        Some("assistant"),
        &format!(
            "posted a {} chart \"{}\" ({} series, {} points; fallback {} bytes as blob {})",
            chart.kind.as_str(),
            title,
            chart.series.len(),
            chart.point_count(),
            png_len,
            key
        ),
        Some(&json!({ "message_id": msg_id, "caption": caption, "chart": chart_value, "fallback": attachment })),
    );
    media::audit(
        a,
        "chart_posted",
        json!({ "chatId": target.chat_id, "messageId": msg_id, "kind": chart.kind.as_str(), "series": chart.series.len(), "points": chart.point_count(), "bytes": png_len, "ref": key, "worker": !target.main_turn }),
    );
    Ok(PostedChart {
        chat_id: target.chat_id,
        message_id: msg_id,
        chart,
        fallback_ref: key,
        fallback_bytes: png_len,
    })
}

pub async fn post_chart(a: &Arc<Agent>, args: &Value) -> Value {
    let s = |k: &str| args.get(k).and_then(Value::as_str);
    let Some(chart_value) = args.get("chart").filter(|c| c.is_object()) else {
        return err_result(
            "NOT posted — `chart` (an object: kind, series, …) is required; nothing was posted.",
        );
    };
    let caption = s("caption").unwrap_or_default();
    match post(a, chart_value, caption, s("chat_id"), s("reply_to"), s("session_id")).await {
        Ok(p) => text_result(
            json!({
                "posted": true,
                "message_id": p.message_id,
                "chat_id": p.chat_id,
                "kind": p.chart.kind.as_str(),
                "series": p.chart.series.len(),
                "points": p.chart.point_count(),
                "fallback": { "ref": p.fallback_ref, "bytes": p.fallback_bytes, "size": format!("{PNG_WIDTH}x{PNG_HEIGHT}") },
                "note": "The chart is in the chat as its own message: interactive on a current Mac or iPhone, a picture elsewhere. Say in one line what it shows; do not list the numbers again.",
            })
            .to_string(),
        ),
        Err(e) => err_result(format!(
            "NOT posted — the chart is not in the chat: {e}. Fix the chart and call post_chart again, or tell the user plainly that it could not be posted; do not say it was."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::tests::test_agent;

    fn eval_chart() -> Value {
        json!({
            "kind": "line",
            "title": "Eval score over runs",
            "subtitle": "memory-pack eval · 240 tasks per run",
            "series": [
                { "name": "Affine 2", "points": [[1, 66.2], [2, 70.4], [3, 79.6], [4, 83.4]] },
                { "name": "Baseline", "points": [[1, 70.1], [2, 70.6], [3, 71.1], [4, 71.2]] }
            ],
            "x": { "label": "Run" },
            "y": { "label": "Pass rate", "unit": "%", "min": 60, "max": 90 },
            "annotations": [{ "type": "vline", "x": 3, "label": "prompt v3" }]
        })
    }

    fn parts_of_kind(m: &arbos_core::doc::MessageView, kind: &str) -> Vec<Value> {
        m.parts
            .iter()
            .filter(|p| p.get("kind").and_then(Value::as_str) == Some(kind))
            .cloned()
            .collect()
    }

    /// The main turn posts one row: caption text → PNG attachment (what an old client shows) →
    /// chart part (what a new client draws, naming the attachment it hides); the tool answers
    /// posted:true + the message id read back from the doc, and the turn counts it as a result.
    #[tokio::test]
    async fn a_chart_is_one_row_with_caption_picture_and_data() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATCHART00000000001";
        let host = agent.sync.open_room(chat).await.unwrap();
        {
            let h = host.lock().await;
            h.doc
                .append_user_message(
                    "m-busy",
                    "how is the eval going?",
                    "dev-mac",
                    &arbos_core::doc::UserMessageOpts::default(),
                )
                .unwrap();
        }
        let _busy = agent.test_hold_turn(chat).await;
        let v = post_chart(
            &agent,
            &json!({ "chart": eval_chart(), "caption": "Affine 2 passed the baseline at run 2 and holds above 80 %." }),
        )
        .await;
        assert!(v.get("isError").is_none(), "{v}");
        let r: Value = serde_json::from_str(v["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(r["posted"], json!(true));
        assert_eq!(r["kind"], json!("line"));
        assert_eq!(r["series"], json!(2));
        assert_eq!(r["points"], json!(8));
        assert_eq!(r["fallback"]["size"], json!("1200x675"));
        let id = r["message_id"].as_str().unwrap().to_owned();
        {
            let tl = agent.turn.lock().await;
            assert_eq!(tl.as_ref().unwrap().attachments_posted, vec![id.clone()]);
        }
        let h = host.lock().await;
        let row = h.doc.message(&id).unwrap();
        assert_eq!(row.role, "assistant");
        assert_eq!(row.status, "done");
        assert_eq!(row.rest["inReplyTo"], json!("m-busy"));
        assert_eq!(row.rest["turnRole"], json!("attachment"));
        assert_eq!(
            row.text(),
            "Affine 2 passed the baseline at run 2 and holds above 80 %."
        );
        let kinds: Vec<&str> = row
            .parts
            .iter()
            .filter_map(|p| p["kind"].as_str())
            .collect();
        assert_eq!(
            kinds,
            ["text", "attachment", "chart"],
            "caption → picture → data"
        );
        let att = &parts_of_kind(&row, "attachment")[0];
        assert_eq!(att["attachment"]["kind"], json!("image"));
        assert_eq!(att["attachment"]["mime"], json!("image/png"));
        assert_eq!(att["attachment"]["name"], json!("eval-score-over-runs.png"));
        assert_eq!(att["attachment"]["width"], json!(1200));
        assert!(att["attachment"]["bytes"].as_u64().unwrap() > 10_000);
        let ch = &parts_of_kind(&row, "chart")[0];
        assert_eq!(ch["specVersion"], json!(1));
        assert_eq!(ch["chart"]["kind"], json!("line"));
        assert_eq!(ch["chart"]["series"][0]["points"][3], json!([4.0, 83.4]));
        assert_eq!(ch["chart"]["y"]["unit"], json!("%"));
        assert_eq!(
            ch["fallback"], att["id"],
            "the chart names the picture a new client hides"
        );
        assert_eq!(att["attachment"]["chartPart"], ch["id"]);
        assert!(ch["table"]
            .as_str()
            .unwrap()
            .starts_with("| Run | Affine 2 (%) | Baseline (%) |"));
        let _ = std::fs::remove_dir_all(root);
    }

    /// Honesty: over a cap, malformed, or without a caption → an error that says NOT posted with
    /// the reason (and the downsample hint for sizes), no row, no message id, nothing counted.
    #[tokio::test]
    async fn a_refused_chart_says_not_posted_and_leaves_no_row() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATCHART00000000002";
        let host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        let before = host.lock().await.doc.messages().len();

        let mut big = eval_chart();
        big["series"][0]["points"] = json!((0..501).map(|i| json!([i, i])).collect::<Vec<_>>());
        let v = post_chart(&agent, &json!({ "chart": big, "caption": "too big" })).await;
        assert_eq!(v["isError"], json!(true));
        let text = v["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("NOT posted")
                && text.contains("501 points")
                && text.contains("deliver_file"),
            "{text}"
        );
        assert!(!text.contains("message_id"));

        let v = post_chart(
            &agent,
            &json!({ "chart": { "kind": "bar", "series": [] }, "caption": "x" }),
        )
        .await;
        assert!(v["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("NOT posted"));
        let v = post_chart(&agent, &json!({ "chart": eval_chart() })).await;
        assert!(v["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("caption is required"));
        let v = post_chart(&agent, &json!({ "caption": "no chart" })).await;
        assert!(v["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("NOT posted"));
        // A misspelt key (`serie` for `series`): serde reports whichever it meets first — the
        // unknown `serie` or the missing `series` (the order depends on the map the JSON is
        // read into: `serde_json`'s `preserve_order` is a workspace-unified feature) — and
        // both name the problem; the tool answer is NOT posted either way.
        let mut typo = eval_chart();
        let series = typo.as_object_mut().unwrap().remove("series").unwrap();
        typo["serie"] = series;
        let v = post_chart(&agent, &json!({ "chart": typo, "caption": "x" })).await;
        let text = v["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("NOT posted")
                && (text.contains("unknown field `serie`")
                    || text.contains("missing field `series`")),
            "{text}"
        );
        // A typo next to a complete chart is always "unknown field".
        let mut extra = eval_chart();
        extra["colour"] = json!("#ff0000");
        let v = post_chart(&agent, &json!({ "chart": extra, "caption": "x" })).await;
        let text = v["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("unknown field `colour`"), "{text}");

        assert_eq!(
            host.lock().await.doc.messages().len(),
            before,
            "no row for a refused chart"
        );
        assert!(agent
            .turn
            .lock()
            .await
            .as_ref()
            .unwrap()
            .attachments_posted
            .is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    /// A job worker's chart lands as its own visible row under the ask, never on the hidden
    /// worker row (the #1900 rule), and does not count for the main turn.
    #[tokio::test]
    async fn a_worker_chart_is_its_own_visible_row_under_the_ask() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATCHART00000000003";
        let host = agent.sync.open_room(chat).await.unwrap();
        let hidden_row = ulid();
        {
            let h = host.lock().await;
            h.doc
                .append_user_message(
                    "m-ask",
                    "chart the eval",
                    "dev-mac",
                    &arbos_core::doc::UserMessageOpts::default(),
                )
                .unwrap();
            h.doc
                .start_assistant_message(&hidden_row, "m-ask", &agent.cfg.device_id);
            h.doc.set_message_fields(
                &hidden_row,
                &[("job", json!({ "id": "j1", "title": "Charting", "kind": "research", "touches": [] })), ("worker", json!(true)), ("hidden", json!(true))],
            );
        }
        let sid = "01TESTWORKERSESSION0000002";
        agent
            .subagent_msgs
            .lock()
            .await
            .insert(sid.to_owned(), (chat.to_owned(), hidden_row.clone()));
        let p = post(
            &agent,
            &eval_chart(),
            "The eval is up.",
            None,
            None,
            Some(sid),
        )
        .await
        .unwrap();
        assert_ne!(p.message_id, hidden_row);
        let h = host.lock().await;
        let row = h.doc.message(&p.message_id).unwrap();
        assert!(row.rest.get("hidden").is_none() && row.rest.get("job").is_none());
        assert_eq!(row.rest["inReplyTo"], json!("m-ask"));
        assert_eq!(parts_of_kind(&row, "chart").len(), 1);
        assert!(parts_of_kind(&h.doc.message(&hidden_row).unwrap(), "chart").is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_tool_spec_carries_the_schema_and_the_prompt_names_the_caps() {
        let specs = tool_specs();
        assert_eq!(specs[0]["name"], json!("post_chart"));
        assert_eq!(
            specs[0]["inputSchema"]["required"],
            json!(["chart", "caption"])
        );
        assert_eq!(
            specs[0]["inputSchema"]["properties"]["chart"]["properties"]["series"]["maxItems"],
            json!(8)
        );
        let p = prompt_section();
        assert!(p.starts_with("## Charts"));
        assert!(
            p.contains("post_chart")
                && p.contains("2 000")
                && p.contains("deliver_file")
                && p.contains("posted: true")
        );
    }
}
