//! What the model-call trace has to be able to answer (Test5, 2026-11-05).
//!
//! Two pings 28 s apart on one box both wrote their whole prompt to cache and read none of it:
//! 28,591 tokens for a new chat at 14:16:55Z, then 91,982 for an existing one at 14:17:23Z. The
//! prompt's cacheable head — the system prompt and the tool definitions — is per project, not per
//! chat, so the second call should have read the first one's prefix. Two explanations were left
//! and nothing on either side could separate them: the two requests' system or tool blocks really
//! differed, or OpenRouter served them from backends that do not share a cache.
//!
//! So the trace is built to answer exactly that: a hash per block (never the text), the cache
//! markers and the TTL the request asked for, and what the upstream says about which backend
//! answered. Same `prefix_sha` with no cache read puts it on the backend; different `prefix_sha`
//! puts it on us, and names which block moved.

use serde_json::json;

use crate::proxy_trace::{generation_marks, prompt_shape, upstream_marks, HASH_HEX};

/// Anthropic's shape: `system` and `tools` ride beside `messages`.
fn anthropic_body(system: &str, tool: &str, user: &str) -> serde_json::Value {
    json!({
        "model": "anthropic/claude-opus-5.5",
        "system": [{ "type": "text", "text": system }],
        "tools": [{ "name": tool, "input_schema": { "type": "object" } }],
        "messages": [{ "role": "user", "content": [{ "type": "text", "text": user }] }],
    })
}

/// The question itself: one prompt head, two chats. The messages differ and must not move the
/// prefix hash — otherwise every turn looks like a different prefix and the trace says nothing.
#[test]
fn the_same_prompt_head_in_two_chats_hashes_the_same() {
    let a = prompt_shape(&anthropic_body(
        "you are test5",
        "route",
        "Reply with only the word PONG.",
    ));
    let b = prompt_shape(&anthropic_body(
        "you are test5",
        "route",
        "r10pinga1: reply with PONG10",
    ));
    assert_eq!(a.prefix_sha, b.prefix_sha, "the cacheable head is the same");
    assert_eq!(a.system_sha, b.system_sha);
    assert_eq!(a.tools_sha, b.tools_sha);
    assert_eq!(a.messages, 1);
}

/// A system prompt that moved by one byte is the other explanation, and the hashes have to name
/// which block it was: the project memory block sits in the system prompt and is refreshed on a
/// timer, so it is the first thing to suspect.
#[test]
fn a_changed_system_block_moves_only_the_system_and_prefix_hashes() {
    let a = prompt_shape(&anthropic_body("you are test5", "route", "hi"));
    let b = prompt_shape(&anthropic_body("you are test5.", "route", "hi"));
    assert_ne!(a.system_sha, b.system_sha, "the system block moved");
    assert_ne!(a.prefix_sha, b.prefix_sha, "so the cacheable head moved");
    assert_eq!(a.tools_sha, b.tools_sha, "the tools did not");

    let c = prompt_shape(&anthropic_body("you are test5", "route_v2", "hi"));
    assert_eq!(a.system_sha, c.system_sha, "the system block did not move");
    assert_ne!(a.tools_sha, c.tools_sha, "the tools did");
    assert_ne!(a.prefix_sha, c.prefix_sha);
}

/// Where the breakpoints sit decides what can be read back at all: a head that is never marked
/// is never cached, however stable it is.
#[test]
fn cache_markers_and_the_ttl_asked_for_are_reported() {
    let body = json!({
        "system": [{ "type": "text", "text": "head", "cache_control": { "type": "ephemeral" } }],
        "tools": [
            { "name": "a" },
            { "name": "route", "cache_control": { "type": "ephemeral", "ttl": "1h" } },
        ],
        "messages": [
            { "role": "user", "content": [{ "type": "text", "text": "one", "cache_control": { "type": "ephemeral" } }] },
            { "role": "user", "content": "two" },
        ],
    });
    let s = prompt_shape(&body);
    assert_eq!(s.cache_marks, "system:1,tools:1,messages:1");
    assert_eq!(
        s.cache_ttl, "1h",
        "the longest retention the request asked for"
    );
    assert_eq!(s.tools, 2);
    assert_eq!(s.messages, 2);

    let plain = prompt_shape(&anthropic_body("head", "route", "hi"));
    assert_eq!(plain.cache_marks, "", "no markers, nothing cached");
    assert_eq!(plain.cache_ttl, "", "no ttl asked for");
}

/// The trace goes to the relay's journal, so it carries hashes and never prompt text.
#[test]
fn hashes_are_short_hex_and_carry_no_prompt_text() {
    let secret = "the owner's home address is 12 Example Street";
    let s = prompt_shape(&anthropic_body(secret, "route", secret));
    for sha in [&s.system_sha, &s.tools_sha, &s.prefix_sha] {
        let sha = sha.as_deref().expect("hashed");
        assert_eq!(sha.len(), HASH_HEX);
        assert!(sha.chars().all(|c| c.is_ascii_hexdigit()));
    }
    let line = format!("{s:?}");
    assert!(
        !line.contains("Example Street"),
        "prompt text leaked: {line}"
    );
    assert!(!line.contains("route"), "tool names are hashed too: {line}");
}

/// The OpenAI-compatible shape keeps its system prompt in `messages`; the same call serves both
/// routes, because the box's subagents and the memory tree go through that one.
#[test]
fn the_openai_shape_takes_its_system_prompt_from_the_messages() {
    let body = json!({
        "model": "openai/gpt-6-luna",
        "messages": [
            { "role": "system", "content": "head" },
            { "role": "user", "content": "hi" },
        ],
        "tools": [{ "type": "function", "function": { "name": "route" } }],
    });
    let s = prompt_shape(&body);
    let same_head = prompt_shape(&json!({
        "messages": [
            { "role": "system", "content": "head" },
            { "role": "user", "content": "something else entirely" },
        ],
        "tools": [{ "type": "function", "function": { "name": "route" } }],
    }));
    assert_eq!(s.system_sha, same_head.system_sha);
    assert_eq!(s.prefix_sha, same_head.prefix_sha);
    assert_eq!(
        s.messages, 2,
        "the system message is counted as a message too"
    );
}

/// A body the relay cannot parse must still be loggable: the trace never decides whether a call
/// is made, so nothing in it may refuse or panic.
#[test]
fn a_body_without_a_head_is_reported_as_having_none() {
    let s = prompt_shape(&json!({ "model": "x" }));
    assert!(s.system_sha.is_none());
    assert!(s.tools_sha.is_none());
    assert!(s.prefix_sha.is_none(), "nothing cacheable to name");
    assert_eq!(s.tools, 0);
    assert_eq!(s.messages, 0);
    assert_eq!(prompt_shape(&serde_json::Value::Null).tools, 0);
}

/// The backend record, which is the half of the question our own hashes cannot answer.
#[test]
fn the_upstream_backend_and_cache_headers_are_picked_out() {
    let mut h = axum::http::HeaderMap::new();
    h.insert("x-openrouter-provider", "Anthropic".parse().unwrap());
    h.insert("anthropic-cache-status", "miss".parse().unwrap());
    h.insert("x-request-id", "req_abc123".parse().unwrap());
    h.insert("content-type", "text/event-stream".parse().unwrap());
    h.insert("set-cookie", "session=secret".parse().unwrap());
    let m = upstream_marks(&h);
    assert_eq!(m.backend, "x-openrouter-provider=Anthropic");
    assert_eq!(m.cache, "anthropic-cache-status=miss");
    assert_eq!(m.upstream_id, "req_abc123");
    let line = format!("{m:?}");
    assert!(
        !line.contains("secret"),
        "only named headers are traced: {line}"
    );

    let bare = upstream_marks(&axum::http::HeaderMap::new());
    assert_eq!(bare.backend, "");
    assert_eq!(bare.cache, "");
    assert_eq!(bare.upstream_id, "");
}

/// A long or repeated header cannot turn one model call into a wall of journal.
#[test]
fn traced_header_values_are_bounded() {
    let mut h = axum::http::HeaderMap::new();
    h.insert(
        "x-openrouter-provider",
        "b".repeat(4096).parse::<axum::http::HeaderValue>().unwrap(),
    );
    let m = upstream_marks(&h);
    assert!(m.backend.len() <= 128, "bounded, got {}", m.backend.len());
}

/// OpenRouter names the backend in the stream as well as (sometimes) in the headers, and the
/// generation id is what its own dashboard is searched by.
#[test]
fn the_generation_id_and_provider_are_read_off_the_stream() {
    let sse = concat!(
        "data: {\"id\":\"gen-abc\",\"provider\":\"Amazon Bedrock\",\"model\":\"anthropic/claude-opus-5.5\"}\n",
        "data: not json\n",
        "data: [DONE]\n",
    );
    let (id, provider) = generation_marks(sse);
    assert_eq!(id.as_deref(), Some("gen-abc"));
    assert_eq!(provider.as_deref(), Some("Amazon Bedrock"));

    // Anthropic's own shape nests it under `message`.
    let (id, provider) =
        generation_marks("data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_01\"}}\n");
    assert_eq!(id.as_deref(), Some("msg_01"));
    assert_eq!(provider, None);

    let (id, provider) = generation_marks("data: [DONE]\n\n");
    assert_eq!(id, None);
    assert_eq!(provider, None);
}
