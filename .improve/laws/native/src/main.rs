//! A bounded JSONL adapter for exercising repository code, without a cloned implementation.

use std::io::{self, BufRead, Write};

use serde_json::{json, Value};

#[path = "../../../../crates/arbos-mac/src/ui/urlwrap.rs"]
mod urlwrap;

const MAX_LINE_BYTES: usize = 65_536;
const MAX_JSON_DEPTH: usize = 32;
const MAX_SAFE_ID: u64 = 9_007_199_254_740_991;

/// Read or drain one line with a bounded allocation. False means the line was too long.
fn read_line(reader: &mut impl BufRead, line: &mut Vec<u8>) -> io::Result<Option<bool>> {
    line.clear();
    let mut oversized = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok((!line.is_empty() || oversized).then_some(!oversized));
        }
        let newline = available.iter().position(|b| *b == b'\n');
        let bytes = newline.unwrap_or(available.len());
        if !oversized {
            if bytes > MAX_LINE_BYTES - line.len() {
                oversized = true;
                line.clear();
            } else {
                line.extend_from_slice(&available[..bytes]);
            }
        }
        reader.consume(bytes + usize::from(newline.is_some()));
        if newline.is_some() {
            return Ok(Some(!oversized));
        }
    }
}

fn invalid(id: Option<u64>) -> Value {
    json!({"id": id, "error": {"tag": "InvalidRequest", "message": "Invalid request"}})
}

fn valid_depth(value: &Value, depth: usize) -> bool {
    if depth > MAX_JSON_DEPTH {
        return false;
    }
    match value {
        Value::Array(items) => items.iter().all(|v| valid_depth(v, depth + 1)),
        Value::Object(items) => items.values().all(|v| valid_depth(v, depth + 1)),
        _ => true,
    }
}

fn respond(line: &[u8]) -> Value {
    let Ok(request) = serde_json::from_slice::<Value>(line) else {
        return invalid(None);
    };
    let id = request["id"].as_u64().filter(|n| *n <= MAX_SAFE_ID);
    let Some(fields) = request.as_object() else {
        return invalid(id);
    };
    let Some(id) = id else {
        return invalid(None);
    };
    let Some(op) = fields.get("op").and_then(Value::as_str) else {
        return invalid(Some(id));
    };
    let payload = match op {
        "text" | "url" => "text",
        "json" => "value",
        _ => return invalid(Some(id)),
    };
    if fields.len() != 3 || !fields.contains_key(payload) {
        return invalid(Some(id));
    }
    if op == "json" {
        let mut value = fields[payload].clone();
        if !valid_depth(&value, 0) {
            return invalid(Some(id));
        }
        laws_redact::scrub_value(&mut value);
        return json!({"id": id, "redact_json": value});
    }
    let Some(text) = fields[payload].as_str() else {
        return invalid(Some(id));
    };
    if op == "url" {
        let hinted = urlwrap::hint_bare_urls(text);
        return json!({
            "id": id,
            "plain": urlwrap::plain(&hinted),
            "hinted": hinted,
            "breakable": urlwrap::breakable(text),
        });
    }
    let patterns = laws_redact::redact_patterns(text);
    let placeholders = laws_redact::redact_placeholders(text);
    let scrub = laws_redact::scrub(text);
    let keys: Vec<_> = laws_redact::find_key_like(text)
        .into_iter()
        .map(|key| {
            json!({
                "start": key.start,
                "end": key.end,
                "value": text.get(key.start..key.end),
                "confident": key.confident,
                "suggested_name": key.suggested_name,
            })
        })
        .collect();
    json!({
        "id": id,
        "patterns_twice": laws_redact::redact_patterns(&patterns),
        "patterns": patterns,
        "placeholders_twice": laws_redact::redact_placeholders(&placeholders),
        "placeholders": placeholders,
        "scrub_twice": laws_redact::scrub(&scrub),
        "scrub": scrub,
        "keys": keys,
    })
}

fn main() -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = stdin.lock();
    let mut writer = io::BufWriter::new(stdout.lock());
    let mut line = Vec::new();
    while let Some(fits) = read_line(&mut reader, &mut line)? {
        let response = if fits { respond(&line) } else { invalid(None) };
        serde_json::to_writer(&mut writer, &response)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
    }
    Ok(())
}
