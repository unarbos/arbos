use crate::model::{
    attachment::{MessageImage, UserMessage},
    session::{ChatItem, ToolStatus},
};
use anyhow::{Result, anyhow};
use cacp::schema::ToolKind;
use serde_json::Value;
use std::time::Duration;

#[derive(Clone, Default)]
pub struct Replay {
    pub items: Vec<ChatItem>,
    pub model: Option<String>,
}

pub fn load(base: &str, session: &str) -> Result<Replay> {
    let client: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .into();
    let mut response = client
        .get(&format!(
            "{}/api/sessions/{session}/events",
            base.trim_end_matches('/')
        ))
        .call()?;
    let body = response
        .body_mut()
        .with_config()
        .limit(128 * 1024 * 1024)
        .read_to_string()?;
    let value: Value = serde_json::from_str(&body)?;
    let model = value
        .pointer("/session/model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_owned);
    Ok(Replay {
        items: decode(&value)?,
        model,
    })
}

pub fn decode(value: &Value) -> Result<Vec<ChatItem>> {
    let events = value
        .get("events")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("History response has no events array"))?;
    let mut items = Vec::new();
    for event in events {
        let text = event.get("text").and_then(Value::as_str).unwrap_or("");
        match event.get("type").and_then(Value::as_str) {
            Some("user") => {
                let mut message = UserMessage::from(text.to_owned());
                if let Some(parts) = event.get("parts").and_then(Value::as_array) {
                    for part in parts.iter().filter(|part| part["type"] == "image") {
                        match MessageImage::from_part(part) {
                            Ok(image) => message.images.push(image),
                            Err(_) => {
                                if !message.text.is_empty() {
                                    message.text.push_str("\n\n");
                                }
                                message.text.push_str("[Image unavailable]");
                            }
                        }
                    }
                }
                items.push(
                    match event
                        .get("author")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                    {
                        Some(who) => ChatItem::From {
                            who: who.into(),
                            text: message.text,
                            images: message.images,
                        },
                        None => ChatItem::User(message),
                    },
                );
            }
            Some("assistant") => {
                if !text.is_empty() {
                    items.push(ChatItem::Agent(text.into()));
                }
                if let Some(calls) = event.get("tool_calls").and_then(Value::as_array) {
                    for call in calls {
                        let id = call
                            .get("ID")
                            .or_else(|| call.get("id"))
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        let name = call
                            .get("Name")
                            .or_else(|| call.get("name"))
                            .and_then(Value::as_str)
                            .unwrap_or("tool");
                        items.push(ChatItem::Tool {
                            id: id.into(),
                            kind: crate::agent::acp::tool_kind(name),
                            label: crate::agent::acp::tool_title(
                                name,
                                crate::agent::acp::tool_path_hint(call).as_deref(),
                            ),
                            status: ToolStatus::Running,
                            output: String::new(),
                            diff: crate::agent::acp::display_diff(
                                name,
                                call.get("Args").or_else(|| call.get("args")),
                                None,
                            ),
                            child_session: None,
                            secs: None,
                            desc: None,
                        });
                    }
                }
            }
            Some("tool_result") => {
                let id = event.get("call_id").and_then(Value::as_str).unwrap_or("");
                let output = event
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let status = if event
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    ToolStatus::Failure
                } else {
                    ToolStatus::Success
                };
                let child = crate::agent::acp::child_session(&event["details"]);
                let diff = crate::agent::acp::result_diff(event);
                if let Some(ChatItem::Tool {
                    status: held,
                    output: body,
                    diff: held_diff,
                    child_session,
                    ..
                }) = items
                    .iter_mut()
                    .rev()
                    .find(|i| matches!(i, ChatItem::Tool { id: held, .. } if held == id))
                {
                    *held = status;
                    *body = output;
                    if diff.is_some() {
                        *held_diff = diff;
                    }
                    *child_session = child;
                } else {
                    items.push(ChatItem::Tool {
                        id: id.into(),
                        kind: ToolKind::Other,
                        label: "tool".into(),
                        status,
                        output,
                        diff,
                        child_session: child,
                        secs: None,
                        desc: None,
                    });
                }
            }
            Some("interrupted") => {
                for item in &mut items {
                    if let ChatItem::Tool { status, .. } = item
                        && *status == ToolStatus::Running
                    {
                        *status = ToolStatus::Failure;
                    }
                }
                items.push(ChatItem::Notice {
                    text: "cancelled".into(),
                    failed: false,
                });
            }
            _ => {}
        }
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn restores_delegate_links_for_completed_calls_and_orphan_results() {
        for calls in [
            json!([]),
            json!([{"ID":"a","Name":"delegate"},{"ID":"b","Name":"delegate"}]),
        ] {
            let items = decode(&json!({"events":[
                {"type":"assistant","tool_calls":calls},
                {"type":"tool_result","call_id":"b","content":"B","details":{"child_session":"child-b"}},
                {"type":"tool_result","call_id":"a","content":"A","details":{"childSession":"child-a"}}
            ]})).unwrap();
            assert_eq!(items.len(), 2);
            let restored: Vec<ChatItem> =
                serde_json::from_value(serde_json::to_value(&items).unwrap()).unwrap();
            for item in restored {
                let ChatItem::Tool {
                    id, child_session, ..
                } = item
                else {
                    panic!("expected tool");
                };
                assert_eq!(child_session, Some(format!("child-{id}")));
            }
        }
    }

    #[test]
    fn restores_complete_turn_and_tool_results() {
        let items = decode(&json!({"events":[
            {"type":"user","text":"fix this"},
            {"type":"assistant","text":"Checking","tool_calls":[{"ID":"one","Name":"read"}]},
            {"type":"tool_result","call_id":"one","content":"file contents"},
            {"type":"assistant","text":"Finished"}
        ]}))
        .unwrap();
        assert_eq!(items.len(), 4);
        assert!(
            matches!(&items[2], ChatItem::Tool { status: ToolStatus::Success, output, .. } if output == "file contents")
        );
        assert!(matches!(&items[3], ChatItem::Agent(text) if text == "Finished"));
    }
    #[test]
    fn restores_inline_images_with_or_without_text_and_author() {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(8, 4)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let part = json!({"type":"image","image":{"data":STANDARD.encode(bytes.into_inner()),"mimeType":"image/png"}});
        for text in ["look", ""] {
            for author in ["", "Sam"] {
                let items = decode(&json!({"events":[{"type":"user","text":text,"author":author,"parts":[part.clone(),part.clone()]}]})).unwrap();
                let (body, images) = match &items[0] {
                    ChatItem::User(message) => (&message.text, &message.images),
                    ChatItem::From { text, images, .. } => (text, images),
                    _ => panic!("expected user message"),
                };
                assert_eq!(body, text);
                assert_eq!(images.len(), 2);
                assert!(images.iter().all(|image| image.preview().is_some()));
                assert_eq!(images[0].display_size(), (8., 4.));
            }
        }
    }

    #[test]
    fn malformed_history_cannot_clear_local_history() {
        assert!(decode(&json!({"error":"offline"})).is_err());
    }
    #[test]
    fn restores_images_authorship_and_interruptions() {
        let items = decode(&json!({"events":[
            {"type":"user","text":"look","author":"Sam","parts":[{"type":"image"}]},
            {"type":"interrupted"}
        ]}))
        .unwrap();
        assert!(
            matches!(&items[0], ChatItem::From { who, text, images } if who == "Sam" && text.contains("Image unavailable") && images.is_empty())
        );
        assert!(matches!(&items[1], ChatItem::Notice { .. }));
    }
}
