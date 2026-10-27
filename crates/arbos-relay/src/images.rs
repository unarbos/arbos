//! Image generation through the relay (F10, D2): `POST /proxy/v1/images/generations` goes to
//! OpenAI with the relay's key and is metered per project like a model call. With
//! `ARBOS_IMAGE_MOCK=1` the relay answers itself with a labelled placeholder PNG so the whole
//! flow runs without a key.

use std::io::Cursor;
use std::time::Instant;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine;
use image::{ImageBuffer, Rgb};
use serde_json::{json, Value};

use crate::auth::Principal;
use crate::state::AppState;

const DEFAULT_MODEL: &str = "gpt-image-1";
const MAX_SIDE: u32 = 2048;

/// USD per image for the sizes and qualities OpenAI publishes; the model's `usage` block, when
/// present, is recorded as tokens beside it.
pub fn price(model: &str, size: &str, quality: &str) -> f64 {
    let m = model.to_ascii_lowercase();
    let tall_or_wide = size != "1024x1024" && size != "auto";
    if m.contains("dall-e-2") {
        return 0.02;
    }
    if m.contains("dall-e-3") {
        return match (quality, tall_or_wide) {
            ("hd", false) => 0.08,
            ("hd", true) => 0.12,
            (_, false) => 0.04,
            (_, true) => 0.08,
        };
    }
    if m.contains("gpt-image-1-mini") {
        return match (quality, tall_or_wide) {
            ("low", false) => 0.005,
            ("low", true) => 0.006,
            ("high", false) => 0.036,
            ("high", true) => 0.052,
            (_, false) => 0.011,
            (_, true) => 0.015,
        };
    }
    match (quality, tall_or_wide) {
        ("low", false) => 0.011,
        ("low", true) => 0.016,
        ("high", false) => 0.167,
        ("high", true) => 0.25,
        // `auto` and `medium`
        (_, false) => 0.042,
        (_, true) => 0.063,
    }
}

fn parse_size(size: &str) -> (u32, u32) {
    let mut it = size.split('x');
    let w = it.next().and_then(|s| s.trim().parse::<u32>().ok());
    let h = it.next().and_then(|s| s.trim().parse::<u32>().ok());
    match (w, h) {
        (Some(w), Some(h)) if (64..=MAX_SIDE).contains(&w) && (64..=MAX_SIDE).contains(&h) => {
            (w, h)
        }
        _ => (1024, 1024),
    }
}

// ---- mock renderer -------------------------------------------------------------------------

/// 5×7 glyphs, one byte per row, low five bits used. Uppercase letters, digits, a few marks.
fn glyph(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0A],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x11, 0x0A, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        '-' => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C],
        ',' => [0x00, 0x00, 0x00, 0x00, 0x0C, 0x04, 0x08],
        ':' => [0x00, 0x0C, 0x0C, 0x00, 0x0C, 0x0C, 0x00],
        '?' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04],
        '!' => [0x04, 0x04, 0x04, 0x04, 0x04, 0x00, 0x04],
        '/' => [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10],
        _ => [0; 7],
    }
}

fn draw_text(img: &mut ImageBuffer<Rgb<u8>, Vec<u8>>, x0: u32, y0: u32, scale: u32, text: &str) {
    let (w, h) = img.dimensions();
    for (i, c) in text.chars().enumerate() {
        let g = glyph(c);
        let cx = x0 + i as u32 * 6 * scale;
        for (row, bits) in g.iter().enumerate() {
            for col in 0..5u32 {
                if bits & (0x10 >> col) == 0 {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let x = cx + col * scale + dx;
                        let y = y0 + row as u32 * scale + dy;
                        if x < w && y < h {
                            img.put_pixel(x, y, Rgb([250, 250, 250]));
                        }
                    }
                }
            }
        }
    }
}

fn wrap(text: &str, cols: usize, max_lines: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let word: String = word.chars().take(cols).collect();
        if !cur.is_empty() && cur.chars().count() + 1 + word.chars().count() > cols {
            lines.push(std::mem::take(&mut cur));
            if lines.len() == max_lines {
                break;
            }
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(&word);
    }
    if !cur.is_empty() && lines.len() < max_lines {
        lines.push(cur);
    }
    lines
}

/// A gradient tinted from the prompt's hash, a frame, "MOCK IMAGE", the prompt, and the size.
pub fn mock_png(prompt: &str, size: &str) -> Vec<u8> {
    let (w, h) = parse_size(size);
    let seed = prompt.bytes().fold(0x811c_9dc5u32, |a, b| {
        (a ^ b as u32).wrapping_mul(0x0100_0193)
    });
    let base = [
        40 + (seed & 0x3F) as u8,
        60 + ((seed >> 8) & 0x3F) as u8,
        90 + ((seed >> 16) & 0x3F) as u8,
    ];
    let mut img = ImageBuffer::from_fn(w, h, |x, y| {
        let fx = x as f32 / w as f32;
        let fy = y as f32 / h as f32;
        Rgb([
            (base[0] as f32 + 90.0 * fx) as u8,
            (base[1] as f32 + 70.0 * fy) as u8,
            (base[2] as f32 + 60.0 * (1.0 - fx)) as u8,
        ])
    });
    let margin = (w.min(h) / 24).max(8);
    for x in margin..w.saturating_sub(margin) {
        for t in 0..3 {
            img.put_pixel(x, margin + t, Rgb([250, 250, 250]));
            img.put_pixel(x, h - margin - 1 - t, Rgb([250, 250, 250]));
        }
    }
    for y in margin..h.saturating_sub(margin) {
        for t in 0..3 {
            img.put_pixel(margin + t, y, Rgb([250, 250, 250]));
            img.put_pixel(w - margin - 1 - t, y, Rgb([250, 250, 250]));
        }
    }
    let scale = (w / 160).clamp(2, 8);
    let inner = w.saturating_sub(4 * margin);
    let cols = (inner / (6 * scale)).max(8) as usize;
    let x0 = 2 * margin;
    draw_text(&mut img, x0, 2 * margin, scale + 1, "MOCK IMAGE");
    let mut y = 2 * margin + 7 * (scale + 1) + 3 * scale;
    for line in wrap(prompt, cols, 8) {
        draw_text(&mut img, x0, y, scale, &line);
        y += 9 * scale;
    }
    draw_text(
        &mut img,
        x0,
        h.saturating_sub(2 * margin + 7 * scale),
        scale,
        &format!("{w}X{h} - ARBOS MOCK"),
    );
    let mut out = Cursor::new(Vec::new());
    let _ = img.write_to(&mut out, image::ImageFormat::Png);
    out.into_inner()
}

// ---- handler -------------------------------------------------------------------------------

/// `POST /proxy/v1/images/generations` (OpenAI Images API shape; `b64_json` responses).
pub async fn generations(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
    Json(mut body): Json<Value>,
) -> Response {
    // A box serves every project of its user (D70): the caller names the project in
    // `X-Arbos-Project`, like the per-workspace model stub does.
    let project_id = crate::proxy::project_of(&state, &principal, &headers)
        .unwrap_or_else(|| "relay-admin".to_owned());
    let prompt = body
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
    if prompt.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": { "message": "prompt is required" } })),
        )
            .into_response();
    }
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_MODEL)
        .to_owned();
    let size = body
        .get("size")
        .and_then(Value::as_str)
        .unwrap_or("1024x1024")
        .to_owned();
    let quality = body
        .get("quality")
        .and_then(Value::as_str)
        .unwrap_or("auto")
        .to_owned();
    let n = body
        .get("n")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .clamp(1, 4);
    body["n"] = json!(n);
    body["model"] = json!(model);

    let t0 = Instant::now();
    if state.cfg.image_mock {
        let data: Vec<Value> = (0..n)
            .map(|_| {
                json!({
                    "b64_json": base64::engine::general_purpose::STANDARD.encode(mock_png(&prompt, &size)),
                    "revised_prompt": prompt,
                })
            })
            .collect();
        let _ = state.db.record_usage(
            &project_id,
            "openai-images",
            &format!("mock:{model}"),
            0,
            0,
            0,
            0,
            0.0,
            t0.elapsed().as_millis() as i64,
            None,
        );
        return Json(json!({
            "created": arbos_core::ids::now_ms() / 1000,
            "data": data,
            "size": size,
            "arbos_mock": true,
        }))
        .into_response();
    }

    let Some(key) = state.cfg.openai_key.clone() else {
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": { "message": "relay has no OPENAI_API_KEY for image generation (set ARBOS_IMAGE_MOCK=1 for placeholders)" } })),
        )
            .into_response();
    };
    if let Err(r) = crate::credits::check_call(&state, &principal, &project_id) {
        return crate::credits::refused_response(&r);
    }
    if let Ok(Some(p)) = state.db.project(&project_id) {
        let today = state
            .db
            .usage_since(&project_id, crate::proxy::start_of_utc_day_ms())
            .map(|u| u.usd)
            .unwrap_or(0.0);
        if today >= p.spend_cap_daily {
            return (
                StatusCode::PAYMENT_REQUIRED,
                Json(json!({ "error": { "type": "spend_cap", "message": format!("daily spend cap ${:.2} reached", p.spend_cap_daily) } })),
            )
                .into_response();
        }
    }
    // gpt-image-* models only return base64; DALL·E needs to be asked for it.
    if model.starts_with("dall-e") {
        body["response_format"] = json!("b64_json");
    }
    let url = format!(
        "{}/images/generations",
        state.cfg.openai_images_base.trim_end_matches('/')
    );
    let upstream = match state
        .http
        .post(&url)
        .bearer_auth(&key)
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": { "message": format!("upstream: {e}") } })),
            )
                .into_response()
        }
    };
    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let bytes = upstream.bytes().await.unwrap_or_default();
    if status.is_success() {
        let parsed: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        let (input, output) = parsed
            .get("usage")
            .map(|u| {
                (
                    u.get("input_tokens").and_then(Value::as_i64).unwrap_or(0),
                    u.get("output_tokens").and_then(Value::as_i64).unwrap_or(0),
                )
            })
            .unwrap_or((0, 0));
        let images = parsed
            .get("data")
            .and_then(Value::as_array)
            .map(|a| a.len() as f64)
            .unwrap_or(n as f64);
        let _ = state.db.record_usage(
            &project_id,
            "openai-images",
            &model,
            input,
            output,
            0,
            0,
            price(&model, &size, &quality) * images,
            t0.elapsed().as_millis() as i64,
            None,
        );
        crate::credits::charge_call(
            &state,
            &principal,
            &project_id,
            price(&model, &size, &quality) * images,
            "inference",
            Some(&model),
        );
    }
    (
        status,
        [
            ("content-type", "application/json"),
            ("x-arbos-proxied", "1"),
        ],
        bytes,
    )
        .into_response()
}

// ---- avatar generation (D92 amendment: the avatar picker's Generate tab) --------------------

/// Pictures the picker may generate per project per UTC day (`ARBOS_AVATAR_GEN_DAILY`, default 20):
/// one `gpt-image-1` square at low quality is $0.011, so a day of taps costs cents.
fn avatar_gen_daily_cap() -> i64 {
    std::env::var("ARBOS_AVATAR_GEN_DAILY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20)
}

/// Models tried in order for an avatar (`ARBOS_AVATAR_GEN_MODELS`, comma-separated, overrides).
/// Production hits api.openai.com directly with the relay's `OPENAI_API_KEY`; on 2026-10-24
/// that key listed `gpt-image-1` in `/models` but `/images/generations` answered 400 "The model
/// 'gpt-image-1' does not exist" (#1456 follow-up), while `gpt-image-1-mini` (≈ $0.005 a square at
/// low) and `gpt-image-2` both generated. A model the provider rejects as unknown falls through to
/// the next one; any other failure stops with the friendly error.
const AVATAR_GEN_MODELS: &str = "gpt-image-1-mini,gpt-image-2";
const AVATAR_GEN_SIZE: &str = "1024x1024";
const AVATAR_GEN_QUALITY: &str = "low";
/// What the apps show when generation fails for any reason (the provider's text goes to the log
/// and the JSON `detail`, never to the user).
pub const AVATAR_GEN_FRIENDLY: &str =
    "Couldn't generate an avatar right now — try again or upload one.";

fn avatar_gen_models() -> Vec<String> {
    let spec =
        std::env::var("ARBOS_AVATAR_GEN_MODELS").unwrap_or_else(|_| AVATAR_GEN_MODELS.into());
    let list: Vec<String> = spec
        .split(',')
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_owned)
        .collect();
    if list.is_empty() {
        AVATAR_GEN_MODELS.split(',').map(str::to_owned).collect()
    } else {
        list
    }
}

/// The OpenAI Images request for one avatar: one square at low quality, the prompt framed as a
/// profile picture.
pub fn avatar_request(model: &str, prompt: &str) -> Value {
    json!({
        "model": model,
        "prompt": format!("A profile picture for an assistant: {prompt}. Centred subject, simple background, no text."),
        "size": AVATAR_GEN_SIZE,
        "quality": AVATAR_GEN_QUALITY,
        "n": 1,
    })
}

/// Did the provider reject the *model* (unknown / retired / not enabled for this key)? Those
/// answers are worth trying the next model on; anything else is not.
pub fn is_unknown_model_error(status: u16, body: &[u8]) -> bool {
    if !(400..500).contains(&status) {
        return false;
    }
    let v: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let param = v
        .pointer("/error/param")
        .and_then(Value::as_str)
        .unwrap_or("");
    let code = v
        .pointer("/error/code")
        .and_then(Value::as_str)
        .unwrap_or("");
    let message = v
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    param == "model"
        || code == "model_not_found"
        || (message.contains("model")
            && (message.contains("does not exist") || message.contains("not found")))
}

/// Did the provider refuse the *prompt* (its safety / content-policy system)? Jacob's
/// "Snowwhite and the transformers" (2026-10-25 23:36 UTC) came back from gpt-image-1-mini as
/// 400 "Your request was rejected by the safety system…" and surfaced as a blanket 502. Another
/// model will refuse the same words, so this is a clear 422 for the person, not a fall-through.
pub fn is_prompt_refusal(status: u16, body: &[u8]) -> bool {
    if !(400..500).contains(&status) {
        return false;
    }
    let v: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let code = v
        .pointer("/error/code")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let kind = v
        .pointer("/error/type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let message = v
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    code.contains("moderation")
        || code.contains("content_policy")
        || code.contains("safety")
        || kind.contains("moderation")
        || kind.contains("content_policy")
        || message.contains("safety system")
        || message.contains("content policy")
        || message.contains("rejected as a result of")
        || (message.contains("rejected") && message.contains("safety"))
}

/// A provider answer worth one more try before giving up on this model: rate limited or a
/// server-side failure (the request itself was fine).
pub fn is_transient_upstream(status: u16) -> bool {
    status == 429 || (500..600).contains(&status)
}

/// What the person reads when the provider refused their words.
pub const AVATAR_REFUSED_FRIENDLY: &str = "That prompt was refused — try a different one.";

/// What the person reads when the provider did not answer within the budget.
pub const AVATAR_TIMEOUT_FRIENDLY: &str = "Couldn't generate the image. The server took too long.";

/// The budget ran out: a 504 with the clear line, the provider's words under `detail`.
fn avatar_timed_out(project_id: &str, detail: impl Into<String>) -> Response {
    let detail = detail.into();
    tracing::warn!(project = %project_id, %detail, "avatar generation timed out");
    (
        StatusCode::GATEWAY_TIMEOUT,
        Json(json!({ "error": { "type": "avatar_timeout", "message": AVATAR_TIMEOUT_FRIENDLY, "detail": detail } })),
    )
        .into_response()
}

/// The refusal: a 422 with the clear line, the provider's words under `detail`, one log line.
fn avatar_refused(project_id: &str, detail: impl Into<String>) -> Response {
    let detail = detail.into();
    tracing::info!(project = %project_id, %detail, "avatar prompt refused by the provider");
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "error": { "type": "avatar_refused", "message": AVATAR_REFUSED_FRIENDLY, "detail": detail } })),
    )
        .into_response()
}

/// The friendly failure: the user-facing line, the provider's words under `detail`, one log line.
fn avatar_unavailable(project_id: &str, detail: impl Into<String>) -> Response {
    let detail = detail.into();
    tracing::warn!(project = %project_id, %detail, "avatar generation failed");
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({ "error": { "type": "avatar_unavailable", "message": AVATAR_GEN_FRIENDLY, "detail": detail } })),
    )
        .into_response()
}
fn avatar_error(status: StatusCode, kind: &str, message: impl Into<String>) -> Response {
    (
        status,
        Json(json!({ "error": { "type": kind, "message": message.into() } })),
    )
        .into_response()
}

/// The provider's PNG (or anything `image` decodes) → the stored master: ≤ 1024 px, Lanczos,
/// JPEG q92 for the photographic pictures the models paint, PNG when flat (`avatar_image`).
pub fn avatar_master(bytes: &[u8]) -> Result<crate::avatar_image::Master, String> {
    crate::avatar_image::master(bytes)
}

#[derive(serde::Deserialize)]
pub struct AvatarGenerateBody {
    pub prompt: String,
}

/// Whose picture is being generated: the blob slot it lands in, who pays, and the per-day key.
pub struct AvatarTarget {
    /// Blob slot (`{project}`, `people:{chat}`, `servers:{server}`) — also the daily-count key.
    pub slot: String,
    /// The user whose credits pay (the agent's owner, the member / admin asking for a group /
    /// server picture).
    pub payer: String,
    /// The agent's daily spend cap, when the target is an agent.
    pub spend_cap_daily: Option<f64>,
    /// The project id to meter against for an agent; the slot for the others.
    pub meter_id: String,
}

/// `POST /projects/{id}/avatar/generate {prompt}` — one square picture for the agent from a short
/// prompt, through the relay's OpenAI key, stored in the project's blob slot and **not yet set**:
/// the answer is `{image_key}` for the picker's preview, and "Set avatar" makes it the picture via
/// `PATCH /projects/{id} {image_key}`. Metered like `/proxy/v1/images/generations` (usage row,
/// credits, the project's daily spend cap) plus a per-target daily count cap. `ARBOS_IMAGE_MOCK=1`
/// answers with the labelled placeholder.
pub async fn avatar_generate(
    State(state): State<AppState>,
    principal: Principal,
    axum::extract::Path(project_id): axum::extract::Path<String>,
    Json(body): Json<AvatarGenerateBody>,
) -> Response {
    let Ok(Some(project)) = state.db.project(&project_id) else {
        return avatar_error(StatusCode::NOT_FOUND, "no_such_project", "no such project");
    };
    if !state.owns(&principal, &project.id) {
        return avatar_error(StatusCode::FORBIDDEN, "forbidden", "not your project");
    }
    let target = AvatarTarget {
        slot: project.id.clone(),
        payer: project.user_id.clone(),
        spend_cap_daily: Some(project.spend_cap_daily),
        meter_id: project.id.clone(),
    };
    generate_into(&state, target, &body.prompt).await
}

/// `POST /chats/{id}/avatar/generate {prompt}` — the same for a people group's picture (any human
/// member, like `PATCH /chats/{id}`); the picture lands in `people:{id}` and "Set avatar" sends
/// `PATCH /chats/{id} {image_key}`. The member asking pays.
pub async fn chat_avatar_generate(
    State(state): State<AppState>,
    principal: Principal,
    axum::extract::Path(chat_id): axum::extract::Path<String>,
    Json(body): Json<AvatarGenerateBody>,
) -> Response {
    let (me, cm) = match crate::people::member_chat(&state, &principal, &chat_id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let slot = format!("{}{}", crate::people::PEOPLE_ACL_PREFIX, cm.chat.id);
    let target = AvatarTarget {
        slot: slot.clone(),
        payer: me,
        spend_cap_daily: None,
        meter_id: slot,
    };
    generate_into(&state, target, &body.prompt).await
}

/// `POST /servers/{id}/avatar/generate {prompt}` — the same for a server's picture (owner /
/// admins, like `PATCH /servers/{id}`); the picture lands in `servers:{id}`. The admin asking pays.
pub async fn server_avatar_generate(
    State(state): State<AppState>,
    principal: Principal,
    axum::extract::Path(server_id): axum::extract::Path<String>,
    Json(body): Json<AvatarGenerateBody>,
) -> Response {
    let (me, server) = match crate::matrix::servers::admin_of(&state, &principal, &server_id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let slot = format!(
        "{}{}",
        crate::matrix::servers::SERVER_BLOB_PREFIX,
        server.id
    );
    let target = AvatarTarget {
        slot: slot.clone(),
        payer: me,
        spend_cap_daily: None,
        meter_id: slot,
    };
    generate_into(&state, target, &body.prompt).await
}

/// Blob slot prefix for a person's own generated previews: `users:{user_id}`.
pub const USER_BLOB_PREFIX: &str = "users:";

/// `GET /projects/{id}/avatar.jpg` — the agent's current picture (the blob its `image_key`
/// names), public like `GET /avatars/{user}.jpg` (the same role as a GitHub CDN avatar URL).
/// Why not `/blob/{project}/{key}`: that slot is the owner's and the project's guests' (it holds
/// the agent's working files too), so a viewer who only shares a group with the agent got 403
/// and drew the automatic leaf instead of the owner's picture (Jacob, 2026-10-25: "Hezbola,
/// run by @unarbos, purple leaf"). Clients append `?v={avatar_version}`, so a change is a new
/// URL and the day-long cache never serves the old bytes. 404 while the agent has no picture.
/// `?w=<px>` serves the picture at that bucket (`avatar_image`).
pub async fn project_avatar(
    State(state): State<AppState>,
    Path(file): Path<String>,
    axum::extract::Query(size): axum::extract::Query<crate::avatar_image::SizeQuery>,
) -> Response {
    let project_id = file.strip_suffix(".jpg").unwrap_or(file.as_str());
    let Ok(Some(p)) = state.db.project(project_id) else {
        return crate::signin::api_error(StatusCode::NOT_FOUND, "not_found", "no such agent");
    };
    let Some(key) = p
        .image_key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
    else {
        return crate::signin::api_error(StatusCode::NOT_FOUND, "not_found", "no picture");
    };
    let Some(bytes) = crate::blobs::read(&state, &p.id, key).await else {
        return crate::signin::api_error(StatusCode::NOT_FOUND, "not_found", "no picture");
    };
    let kind = match crate::blobs::stored_image(&state, &p.id, key).await {
        crate::blobs::StoredImage::Image(kind) => kind,
        crate::blobs::StoredImage::Missing | crate::blobs::StoredImage::NotAnImage(_) => {
            return crate::signin::api_error(StatusCode::NOT_FOUND, "not_found", "no picture")
        }
    };
    crate::avatar_image::respond(
        &state,
        crate::avatar_image::Master { kind, bytes },
        size.w,
        Some("public, max-age=86400"),
    )
    .await
}

/// The image provider's last health probe: `GET {images_base}/models/{first avatar model}` with
/// the relay's key — no generation, no spend — so an outage, a revoked key or a retired model
/// shows in `healthz.images` and `GET /admin/images/health` before a person hits it.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ImageHealth {
    pub ok: bool,
    pub checked_at: i64,
    pub model: String,
    pub status: u16,
    pub detail: String,
    pub took_ms: u64,
}

/// How often the background probe runs.
const IMAGE_HEALTH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(300);

/// One probe; stores and returns the record. With `ARBOS_IMAGE_MOCK=1` or no key the answer
/// says so instead of calling out.
pub async fn probe_provider(state: &AppState) -> ImageHealth {
    let model = avatar_gen_models()[0].clone();
    let t0 = Instant::now();
    let record = if state.cfg.image_mock {
        ImageHealth {
            ok: true,
            checked_at: arbos_core::ids::now_ms(),
            model,
            status: 200,
            detail: "mock".into(),
            took_ms: 0,
        }
    } else if let Some(key) = state.cfg.openai_key.clone() {
        let url = format!(
            "{}/models/{model}",
            state.cfg.openai_images_base.trim_end_matches('/')
        );
        match state
            .http
            .get(&url)
            .bearer_auth(&key)
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await
        {
            Ok(r) => {
                let status = r.status().as_u16();
                let body = r.text().await.unwrap_or_default();
                let detail = if (200..300).contains(&status) {
                    "ok".to_owned()
                } else {
                    serde_json::from_str::<Value>(&body)
                        .ok()
                        .and_then(|v| {
                            v.pointer("/error/message")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        })
                        .unwrap_or_else(|| format!("HTTP {status}"))
                };
                ImageHealth {
                    ok: (200..300).contains(&status),
                    checked_at: arbos_core::ids::now_ms(),
                    model,
                    status,
                    detail,
                    took_ms: t0.elapsed().as_millis() as u64,
                }
            }
            Err(e) => ImageHealth {
                ok: false,
                checked_at: arbos_core::ids::now_ms(),
                model,
                status: 0,
                detail: e.to_string(),
                took_ms: t0.elapsed().as_millis() as u64,
            },
        }
    } else {
        ImageHealth {
            ok: false,
            checked_at: arbos_core::ids::now_ms(),
            model,
            status: 0,
            detail: "no OPENAI_API_KEY".into(),
            took_ms: 0,
        }
    };
    if !record.ok {
        tracing::warn!(model = %record.model, status = record.status, detail = %record.detail, "image provider probe failed");
    }
    *state.image_health.lock().await = Some(record.clone());
    record
}

/// Every 5 minutes, from start-up (the first probe runs at once).
pub fn spawn_health_probe(state: AppState) {
    tokio::spawn(async move {
        loop {
            probe_provider(&state).await;
            tokio::time::sleep(IMAGE_HEALTH_INTERVAL).await;
        }
    });
}

/// `GET /admin/images/health[?probe=1]` (admin token): the last probe, or a fresh one.
pub async fn admin_image_health(
    State(state): State<AppState>,
    principal: Principal,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    if !matches!(principal, Principal::Admin) {
        return avatar_error(StatusCode::FORBIDDEN, "forbidden", "admin token required");
    }
    let record = if q.get("probe").is_some_and(|v| v == "1") {
        Some(probe_provider(&state).await)
    } else {
        state.image_health.lock().await.clone()
    };
    Json(json!({ "images": record })).into_response()
}

/// `POST /me/avatar/generate {prompt}` — the same for the signed-in person's own profile picture
/// (Jacob, 2026-10-25: the own-profile avatar opens the agents' picker). The picture lands in
/// `users:{id}`, which only that person reads, and "Set avatar" sends
/// `PUT /me/avatar {image_key}`. The person asking pays; 20 a day like every other target.
pub async fn me_avatar_generate(
    State(state): State<AppState>,
    principal: Principal,
    Json(body): Json<AvatarGenerateBody>,
) -> Response {
    let Principal::Device { user_id, .. } = &principal else {
        return avatar_error(StatusCode::FORBIDDEN, "forbidden", "device token required");
    };
    let slot = format!("{USER_BLOB_PREFIX}{user_id}");
    let target = AvatarTarget {
        slot: slot.clone(),
        payer: user_id.clone(),
        spend_cap_daily: None,
        meter_id: slot,
    };
    generate_into(&state, target, &body.prompt).await
}

async fn generate_into(state: &AppState, target: AvatarTarget, prompt: &str) -> Response {
    let prompt: String = prompt.trim().chars().take(400).collect();
    if prompt.is_empty() {
        return avatar_error(StatusCode::BAD_REQUEST, "bad_prompt", "prompt is required");
    }
    let since = crate::proxy::start_of_utc_day_ms();
    let made_today = state
        .db
        .usage_count_since(&target.meter_id, "openai-images", "avatar:", since)
        .unwrap_or(0);
    if made_today >= avatar_gen_daily_cap() {
        return avatar_error(
            StatusCode::TOO_MANY_REQUESTS,
            "avatar_gen_cap",
            format!(
                "{} generated pictures today is the limit; try again tomorrow or upload one",
                avatar_gen_daily_cap()
            ),
        );
    }
    let models = avatar_gen_models();
    let mut model = format!("avatar:{}", models[0]);
    let t0 = Instant::now();
    let png: Vec<u8> = if state.cfg.image_mock {
        let _ = state.db.record_usage(
            &target.meter_id,
            "openai-images",
            &format!("mock:{model}"),
            0,
            0,
            0,
            0,
            0.0,
            t0.elapsed().as_millis() as i64,
            None,
        );
        mock_png(&prompt, AVATAR_GEN_SIZE)
    } else {
        let Some(key) = state.cfg.openai_key.clone() else {
            return avatar_error(
                StatusCode::BAD_GATEWAY,
                "no_image_key",
                "the relay has no OPENAI_API_KEY for picture generation",
            );
        };
        if let Err(r) = crate::credits::check(state, &target.payer) {
            return crate::credits::refused_response(&r);
        }
        if let Some(cap) = target.spend_cap_daily {
            let today = state
                .db
                .usage_since(&target.meter_id, since)
                .map(|u| u.usd)
                .unwrap_or(0.0);
            if today >= cap {
                return avatar_error(
                    StatusCode::PAYMENT_REQUIRED,
                    "spend_cap",
                    format!("daily spend cap ${cap:.2} reached"),
                );
            }
        }
        let url = format!(
            "{}/images/generations",
            state.cfg.openai_images_base.trim_end_matches('/')
        );
        let mut picked: Option<(String, Value)> = None;
        let mut last_detail = String::from("no model answered");
        let backoff = &state.cfg.avatar_retry_backoff_ms;
        // Every attempt gets `avatar_attempt_timeout`, the whole generate `avatar_total_timeout`
        // (less than the apps' request timeout, so the person always gets the relay's answer
        // and never their own client's "operation timed out").
        let deadline = t0 + state.cfg.avatar_total_timeout;
        let mut timed_out = false;
        'models: for candidate in &models {
            // Transient trouble (timeout, 429, 5xx) gets `backoff.len()` more tries on the same
            // model, then the next model; a refused prompt or any other 4xx stops here.
            let mut attempt = 0usize;
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    timed_out = true;
                    break 'models;
                }
                // The verdict follows the last attempt: a timeout, or whatever it answered.
                timed_out = false;
                let sent = state
                    .http
                    .post(&url)
                    .bearer_auth(&key)
                    .timeout(state.cfg.avatar_attempt_timeout.min(left))
                    .json(&avatar_request(candidate, &prompt))
                    .send()
                    .await;
                let (status, bytes) = match sent {
                    Ok(r) => {
                        let status = r.status();
                        let body = tokio::time::timeout(
                            deadline.saturating_duration_since(Instant::now()),
                            r.bytes(),
                        )
                        .await;
                        match body {
                            Ok(b) => (status, b.unwrap_or_default()),
                            Err(_) => {
                                last_detail = format!("{candidate}: timed out reading the answer");
                                timed_out = true;
                                break 'models;
                            }
                        }
                    }
                    Err(e) => {
                        last_detail = format!("{candidate}: {e}");
                        if e.is_timeout() {
                            timed_out = true;
                        }
                        if deadline.saturating_duration_since(Instant::now()).is_zero() {
                            break 'models;
                        }
                        if let Some(wait) = backoff.get(attempt) {
                            tracing::warn!(target = %target.slot, model = %candidate, error = %e, attempt, "avatar provider unreachable; retrying");
                            tokio::time::sleep(std::time::Duration::from_millis(*wait)).await;
                            attempt += 1;
                            continue;
                        }
                        continue 'models;
                    }
                };
                if status.is_success() {
                    let parsed: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                    picked = Some((candidate.clone(), parsed));
                    break 'models;
                }
                let message = serde_json::from_slice::<Value>(&bytes)
                    .ok()
                    .and_then(|v| {
                        v.pointer("/error/message")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| format!("HTTP {status}"));
                last_detail = format!("{candidate}: {message}");
                if is_prompt_refusal(status.as_u16(), &bytes) {
                    return avatar_refused(&target.slot, last_detail);
                }
                if is_unknown_model_error(status.as_u16(), &bytes) {
                    tracing::info!(target = %target.slot, model = %candidate, %message, "avatar model rejected; trying the next");
                    continue 'models;
                }
                if is_transient_upstream(status.as_u16()) {
                    if let Some(wait) = backoff.get(attempt) {
                        tracing::warn!(target = %target.slot, model = %candidate, %status, attempt, "avatar provider transient failure; retrying");
                        tokio::time::sleep(std::time::Duration::from_millis(*wait)).await;
                        attempt += 1;
                        continue;
                    }
                    tracing::warn!(target = %target.slot, model = %candidate, %status, "avatar provider still failing; trying the next model");
                    continue 'models;
                }
                return avatar_unavailable(&target.slot, last_detail);
            }
        }
        let Some((used, parsed)) = picked else {
            if timed_out {
                return avatar_timed_out(&target.slot, last_detail);
            }
            return avatar_unavailable(&target.slot, last_detail);
        };
        model = format!("avatar:{used}");
        let Some(b64) = parsed.pointer("/data/0/b64_json").and_then(Value::as_str) else {
            return avatar_unavailable(&target.slot, format!("{used}: no picture in the answer"));
        };
        let Ok(png) = base64::engine::general_purpose::STANDARD.decode(b64) else {
            return avatar_unavailable(&target.slot, format!("{used}: picture was not base64"));
        };
        let (input, output) = parsed
            .get("usage")
            .map(|u| {
                (
                    u.get("input_tokens").and_then(Value::as_i64).unwrap_or(0),
                    u.get("output_tokens").and_then(Value::as_i64).unwrap_or(0),
                )
            })
            .unwrap_or((0, 0));
        let usd = price(&used, AVATAR_GEN_SIZE, AVATAR_GEN_QUALITY);
        let _ = state.db.record_usage(
            &target.meter_id,
            "openai-images",
            &model,
            input,
            output,
            0,
            0,
            usd,
            t0.elapsed().as_millis() as i64,
            None,
        );
        crate::credits::charge(state, &target.payer, usd, "inference", Some(&model));
        png
    };
    let stored = match tokio::task::spawn_blocking(move || avatar_master(&png)).await {
        Ok(Ok(m)) => m,
        Ok(Err(e)) => return avatar_unavailable(&target.slot, e),
        Err(e) => return avatar_unavailable(&target.slot, e.to_string()),
    };
    let image_key = format!(
        "avatar-gen-{}.{}",
        arbos_core::ids::ulid().to_lowercase(),
        stored.ext()
    );
    if let Err(e) = crate::blobs::store(
        state,
        &target.slot,
        &image_key,
        stored.mime(),
        &stored.bytes,
    )
    .await
    {
        return avatar_error(StatusCode::INTERNAL_SERVER_ERROR, "store", e);
    }
    Json(json!({
        "image_key": image_key,
        "model": model.trim_start_matches("avatar:"),
        "mock": state.cfg.image_mock,
        "made_today": made_today + 1,
        "daily_cap": avatar_gen_daily_cap(),
    }))
    .into_response()
}

#[cfg(test)]
mod avatar_generate_tests {
    //! The avatar picker's Generate tab against a stub OpenAI: the request shape, the fall-through
    //! from a model the provider rejects to the next one, and the friendly error.

    use std::sync::{Arc, Mutex};

    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use axum::routing::post;
    use axum::{Json, Router};

    use super::*;
    use crate::people::tests_support::{person, project_of};
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    /// A stub `/images/generations`: records every request, rejects models in `unknown` the way
    /// OpenAI does, refuses a prompt containing "forbidden" the way the safety system did on
    /// 2026-10-25, answers 503 to the first `flaky_503` calls, a 1 × 1 PNG for the rest. Also
    /// serves `GET /models/{m}` (200, or 404 for `unknown`) for the health probe.
    async fn stub(unknown: &'static [&'static str]) -> (String, Arc<Mutex<Vec<Value>>>) {
        stub_with(unknown, 0).await
    }

    async fn stub_with(
        unknown: &'static [&'static str],
        flaky_503: usize,
    ) -> (String, Arc<Mutex<Vec<Value>>>) {
        use axum::routing::get;
        let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
        let seen_in = seen.clone();
        let flaky_left = Arc::new(Mutex::new(flaky_503));
        let app = Router::new().route(
            "/models/{model}",
            get(move |Path(model): Path<String>| async move {
                if unknown.contains(&model.as_str()) {
                    (
                        StatusCode::NOT_FOUND,
                        Json(json!({ "error": { "message": format!("The model '{model}' does not exist"), "code": "model_not_found" } })),
                    )
                } else {
                    (StatusCode::OK, Json(json!({ "id": model, "object": "model" })))
                }
            }),
        ).route(
            "/images/generations",
            post(move |Json(body): Json<Value>| {
                let seen = seen_in.clone();
                let flaky_left = flaky_left.clone();
                async move {
                    seen.lock().unwrap().push(body.clone());
                    let model = body["model"].as_str().unwrap_or("");
                    let prompt = body["prompt"].as_str().unwrap_or("");
                    if unknown.contains(&model) {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({ "error": {
                                "message": format!("The model '{model}' does not exist."),
                                "type": "image_generation_user_error", "param": "model", "code": "invalid_value" } })),
                        );
                    }
                    // A stalled provider: the first `flaky_503` calls hang 30 s (the caller's
                    // attempt timeout must cut them), the rest answer.
                    if prompt.contains("slow") {
                        let stall = {
                            let mut left = flaky_left.lock().unwrap();
                            if *left > 0 {
                                *left -= 1;
                                true
                            } else {
                                false
                            }
                        };
                        if stall {
                            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                        }
                    }
                    if prompt.contains("forbidden") {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({ "error": {
                                "message": "Your request was rejected by the safety system. If you believe this is an error, contact us at help.openai.com and include the request ID req_stub.",
                                "type": "image_generation_user_error", "param": Value::Null, "code": "moderation_blocked" } })),
                        );
                    }
                    {
                        let mut left = flaky_left.lock().unwrap();
                        if *left > 0 {
                            *left -= 1;
                            return (
                                StatusCode::SERVICE_UNAVAILABLE,
                                Json(json!({ "error": { "message": "The server is overloaded", "type": "server_error", "param": Value::Null, "code": Value::Null } })),
                            );
                        }
                    }
                    let png = mock_png("stub", "64x64");
                    (
                        StatusCode::OK,
                        Json(json!({ "created": 1, "data": [{ "b64_json": base64::engine::general_purpose::STANDARD.encode(png) }],
                            "usage": { "input_tokens": 8, "output_tokens": 272 } })),
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), seen)
    }

    fn state_against(base: &str) -> AppState {
        let mut state = test_state(AuthMode::Github);
        let cfg = Arc::get_mut(&mut state.cfg).unwrap();
        cfg.openai_key = Some("sk-test".into());
        cfg.openai_images_base = base.to_owned();
        state
    }

    /// A funded owner and one of their agents (GitHub mode meters credits).
    fn funded_agent(state: &AppState, n: i64) -> (Principal, String) {
        let (alice, alice_dev) = person(state, n);
        state.db.grant_credits(&alice, 5.0, "test", "test").unwrap();
        (alice_dev, project_of(state, &alice, "Bob"))
    }

    async fn generate(state: &AppState, who: &Principal, pid: &str) -> (StatusCode, Value) {
        let r = avatar_generate(
            State(state.clone()),
            who.clone(),
            Path(pid.to_owned()),
            Json(AvatarGenerateBody {
                prompt: "a cat".into(),
            }),
        )
        .await;
        let status = r.status();
        let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[test]
    fn request_is_one_low_square_with_the_framed_prompt() {
        let r = avatar_request("gpt-image-1-mini", "a cat");
        assert_eq!(r["model"], "gpt-image-1-mini");
        assert_eq!(r["size"], "1024x1024");
        assert_eq!(r["quality"], "low");
        assert_eq!(r["n"], 1);
        let prompt = r["prompt"].as_str().unwrap();
        assert!(prompt.contains("a cat") && prompt.starts_with("A profile picture"));
        assert!(
            r.get("response_format").is_none(),
            "gpt-image-* return b64 without it"
        );
        // Jacob's report, verbatim from api.openai.com: a model rejection, not a transient.
        let body = br#"{"error":{"message":"The model 'gpt-image-1' does not exist.","type":"image_generation_user_error","param":"model","code":"invalid_value"}}"#;
        assert!(is_unknown_model_error(400, body));
        assert!(!is_unknown_model_error(500, body));
        assert!(!is_unknown_model_error(
            429,
            br#"{"error":{"message":"Rate limit reached"}}"#
        ));
    }

    #[tokio::test]
    async fn falls_through_to_the_next_model_and_stores_a_master() {
        // The default list is tried in order; the stub rejects the first the way OpenAI rejected
        // gpt-image-1 (no env override: these tests run in parallel threads).
        let (base, seen) = stub(&["gpt-image-1-mini"]).await;
        let state = state_against(&base);
        let (alice_dev, pid) = funded_agent(&state, 81);
        let (status, v) = generate(&state, &alice_dev, &pid).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        assert_eq!(v["model"], "gpt-image-2");
        let key = v["image_key"].as_str().unwrap();
        // The stub paints a flat placeholder, so the master is PNG; a real painting is JPEG q92.
        assert!(matches!(
            crate::blobs::stored_image(&state, &pid, key).await,
            crate::blobs::StoredImage::Image(
                crate::image_sniff::ImageKind::Jpeg | crate::image_sniff::ImageKind::Png
            )
        ));
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0]["model"], "gpt-image-1-mini");
        assert_eq!(seen[1]["model"], "gpt-image-2");
        assert_eq!(seen[1]["quality"], "low");
        // Metered under the model that answered.
        assert_eq!(
            state
                .db
                .usage_count_since(&pid, "openai-images", "avatar:gpt-image-2", 0)
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn every_model_unknown_is_the_friendly_error_with_the_detail_kept() {
        let (base, _) = stub(&["gpt-image-1-mini", "gpt-image-2"]).await;
        let state = state_against(&base);
        let (alice_dev, pid) = funded_agent(&state, 82);
        let (status, v) = generate(&state, &alice_dev, &pid).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(v["error"]["type"], "avatar_unavailable");
        assert_eq!(v["error"]["message"], AVATAR_GEN_FRIENDLY);
        assert!(v["error"]["detail"]
            .as_str()
            .unwrap()
            .contains("does not exist"));
    }

    #[tokio::test]
    async fn groups_and_servers_generate_into_their_own_slots() {
        use crate::people::tests_support::dm;
        let (base, seen) = stub(&[]).await;
        let state = state_against(&base);
        let (alice, alice_dev) = person(&state, 84);
        let (bob, bob_dev) = person(&state, 85);
        let (_, carol_dev) = person(&state, 86);
        state.db.grant_credits(&alice, 5.0, "test", "test").unwrap();
        state.db.grant_credits(&bob, 5.0, "test", "test").unwrap();

        // A people group: any member generates into `people:{chat}`; a stranger may not.
        let chat = dm(&state, &alice, &bob);
        let call = |who: &Principal, id: &str| {
            let state = state.clone();
            let who = who.clone();
            let id = id.to_owned();
            async move {
                let r = chat_avatar_generate(
                    State(state),
                    who,
                    Path(id),
                    Json(AvatarGenerateBody {
                        prompt: "a cat".into(),
                    }),
                )
                .await;
                let status = r.status();
                let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
                (
                    status,
                    serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
                )
            }
        };
        let (status, _) = call(&carol_dev, &chat).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, v) = call(&bob_dev, &chat).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        let key = v["image_key"].as_str().unwrap();
        let slot = format!("{}{chat}", crate::people::PEOPLE_ACL_PREFIX);
        // The stub paints a flat placeholder, so the master is PNG; a real painting is JPEG q92.
        assert!(matches!(
            crate::blobs::stored_image(&state, &slot, key).await,
            crate::blobs::StoredImage::Image(
                crate::image_sniff::ImageKind::Jpeg | crate::image_sniff::ImageKind::Png
            )
        ));
        // Set avatar on the group accepts it (the same gate as an upload).
        let r = crate::people::chat_patch(
            State(state.clone()),
            bob_dev.clone(),
            Path(chat.clone()),
            Json(crate::people::PatchChatBody {
                image_key: Some(key.to_owned()),
                // Every other field at its wire default, so a sibling adding one does not break
                // this test in a shared gate tree.
                ..serde_json::from_str("{}").unwrap()
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        // Metered per group, the asking member pays.
        assert_eq!(
            state
                .db
                .usage_count_since(&slot, "openai-images", "avatar:", 0)
                .unwrap(),
            1
        );

        // A server: owner / admins generate into `servers:{id}`; a plain member may not.
        let server = crate::matrix::servers_db::Server {
            id: arbos_core::ids::ulid(),
            name: "Launch Crew".into(),
            owner_user_id: alice.clone(),
            image_key: None,
            invite_code: "crew8484".into(),
            space_room_id: None,
            created_at: arbos_core::ids::now_ms(),
            is_open: false,
            avatar_leaf: None,
            avatar_leaf_color: None,
        };
        state.db.insert_server(&server).unwrap();
        state
            .db
            .add_server_member(&server.id, &bob, &alice)
            .unwrap();
        let scall = |who: &Principal| {
            let state = state.clone();
            let who = who.clone();
            let id = server.id.clone();
            async move {
                let r = server_avatar_generate(
                    State(state),
                    who,
                    Path(id),
                    Json(AvatarGenerateBody {
                        prompt: "a crew".into(),
                    }),
                )
                .await;
                let status = r.status();
                let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
                (
                    status,
                    serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
                )
            }
        };
        let (status, _) = scall(&bob_dev).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "a member is not an admin");
        let (status, v) = scall(&alice_dev).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        let key = v["image_key"].as_str().unwrap();
        let slot = format!(
            "{}{}",
            crate::matrix::servers::SERVER_BLOB_PREFIX,
            server.id
        );
        // The stub paints a flat placeholder, so the master is PNG; a real painting is JPEG q92.
        assert!(matches!(
            crate::blobs::stored_image(&state, &slot, key).await,
            crate::blobs::StoredImage::Image(
                crate::image_sniff::ImageKind::Jpeg | crate::image_sniff::ImageKind::Png
            )
        ));
        // Members read the slot (the apps fetch the picture); strangers do not.
        assert!(state.allows(&bob_dev, &slot));
        assert!(!state.allows(&carol_dev, &slot));
        // Set avatar on the server accepts it, and the leaf PATCH works beside it.
        let r = crate::matrix::servers::patch(
            State(state.clone()),
            alice_dev.clone(),
            Path(server.id.clone()),
            Json(crate::matrix::servers::PatchBody {
                image_key: Some(json!(key)),
                avatar_leaf: Some(None),
                avatar_leaf_color: Some(None),
                ..serde_json::from_str("{}").unwrap()
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        let saved = state.db.server(&server.id).unwrap().unwrap();
        assert_eq!(saved.image_key.as_deref(), Some(key));
        assert_eq!(saved.avatar_leaf, None);
        assert_eq!(
            seen.lock().unwrap().len(),
            2,
            "one provider call per picture"
        );
    }

    /// The own-profile picker (2026-10-25): Generate lands in `users:{me}` (mine to read, nobody
    /// else's), `PUT /me/avatar {image_key}` makes it the profile picture, and the change fans out
    /// as `profile_updated` to my other devices and to everyone who shares a chat or a server with
    /// me — never to the device that made the change, never to a stranger.
    #[tokio::test]
    async fn own_profile_generates_into_users_slot_sets_and_fans_out() {
        use crate::people::tests_support::dm;
        use axum::http::{header, HeaderMap, HeaderValue};
        let (base, _seen) = stub(&[]).await;
        let state = state_against(&base);
        let (alice, alice_dev) = person(&state, 87);
        let (bob, bob_dev) = person(&state, 88);
        let (_carol, carol_dev) = person(&state, 89);
        let (dave, dave_dev) = person(&state, 90);
        state.db.grant_credits(&alice, 5.0, "test", "test").unwrap();
        // Alice's second device (her phone), a DM with Bob, a server with Dave.
        let alice_phone = arbos_core::ids::ulid();
        state
            .db
            .insert_device(
                &alice_phone,
                "phone",
                &arbos_core::ids::ulid(),
                &alice,
                "github",
            )
            .unwrap();
        let _dm = dm(&state, &alice, &bob);
        let server = crate::matrix::servers_db::Server {
            id: arbos_core::ids::ulid(),
            name: "Crew".into(),
            owner_user_id: alice.clone(),
            image_key: None,
            invite_code: "crew8790".into(),
            space_room_id: None,
            created_at: arbos_core::ids::now_ms(),
            is_open: false,
            avatar_leaf: None,
            avatar_leaf_color: None,
        };
        state.db.insert_server(&server).unwrap();
        state
            .db
            .add_server_member(&server.id, &dave, &alice)
            .unwrap();
        let device_id = |p: &Principal| match p {
            Principal::Device { id, .. } => id.clone(),
            _ => unreachable!(),
        };
        let drain = |dev: &str| {
            state
                .db
                .take_user_pushes(dev)
                .unwrap()
                .into_iter()
                .filter(|(_, kind, _, _)| kind == "profile_updated")
                .map(|(_, _, payload, _)| payload)
                .collect::<Vec<Value>>()
        };

        // Generate: a JPEG in `users:{alice}`, which only Alice's devices read.
        let r = me_avatar_generate(
            State(state.clone()),
            alice_dev.clone(),
            Json(AvatarGenerateBody {
                prompt: "a fox".into(),
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        let key = v["image_key"].as_str().unwrap().to_owned();
        let slot = format!("{USER_BLOB_PREFIX}{alice}");
        // The stub paints a flat placeholder, so the master is PNG; a real painting is JPEG q92.
        assert!(matches!(
            crate::blobs::stored_image(&state, &slot, &key).await,
            crate::blobs::StoredImage::Image(
                crate::image_sniff::ImageKind::Jpeg | crate::image_sniff::ImageKind::Png
            )
        ));
        assert!(state.allows(&alice_dev, &slot));
        assert!(!state.allows(&bob_dev, &slot));
        assert!(!state.allows(&carol_dev, &slot));
        assert_eq!(
            state
                .db
                .usage_count_since(&slot, "openai-images", "avatar:", 0)
                .unwrap(),
            1
        );
        // A box token may not generate for a person.
        let r = me_avatar_generate(
            State(state.clone()),
            Principal::Box {
                user_id: alice.clone(),
            },
            Json(AvatarGenerateBody {
                prompt: "a fox".into(),
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);

        // Set avatar: `PUT /me/avatar {image_key}` copies the preview into the profile picture.
        let mut json_headers = HeaderMap::new();
        json_headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        let put = |key: &str| {
            let state = state.clone();
            let who = alice_dev.clone();
            let headers = json_headers.clone();
            let body = json!({ "image_key": key }).to_string();
            async move { crate::signin::put_avatar(State(state), who, headers, body.into()).await }
        };
        let r = put("avatar-gen-nothing.jpg").await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST, "unknown key");
        assert!(
            drain(&alice_phone).is_empty(),
            "a refused set fans out nothing"
        );
        let r = put(&key).await;
        assert_eq!(r.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        let url = v["avatar_url"].as_str().unwrap().to_owned();
        assert!(url.contains(&format!("/avatars/{alice}.jpg")), "{url}");
        let stored = state.db.user(&alice).unwrap().unwrap().avatar_url;
        assert_eq!(stored.as_deref(), Some(url.as_str()));

        // Fan-out: her phone, Bob (DM) and Dave (server) hear it; her Mac and Carol do not.
        let phone = drain(&alice_phone);
        assert_eq!(phone.len(), 1, "{phone:?}");
        assert_eq!(phone[0]["user_id"], alice);
        assert_eq!(phone[0]["avatar_url"], url);
        assert!(
            drain(&device_id(&alice_dev)).is_empty(),
            "the caller already knows"
        );
        assert_eq!(drain(&device_id(&bob_dev)).len(), 1);
        assert_eq!(drain(&device_id(&dave_dev)).len(), 1);
        assert!(
            drain(&device_id(&carol_dev)).is_empty(),
            "a stranger hears nothing"
        );
        let mut audience = crate::signin::profile_audience(&state, &alice);
        audience.sort();
        let mut expected = vec![bob.clone(), dave.clone()];
        expected.sort();
        assert_eq!(audience, expected);

        // Reset: back to the GitHub picture, and the same people hear about it.
        let r = crate::signin::delete_avatar(State(state.clone()), alice_dev.clone()).await;
        assert_eq!(r.status(), StatusCode::OK);
        let phone = drain(&alice_phone);
        assert_eq!(phone.len(), 1);
        assert!(phone[0]["avatar_url"]
            .as_str()
            .unwrap()
            .contains("avatars.githubusercontent.com"));
        assert_eq!(drain(&device_id(&bob_dev)).len(), 1);
        assert!(drain(&device_id(&alice_dev)).is_empty());
    }

    /// Jacob's 2026-10-25 23:36 UTC failure: the provider's safety system refused "Snowwhite and
    /// the transformers" and the relay answered a blanket 502. A refusal is now a 422 with a clear
    /// line, on the agent route and the own-profile route alike; nothing is retried or metered.
    #[tokio::test]
    async fn a_refused_prompt_is_a_clear_422_on_every_route() {
        let (base, seen) = stub(&[]).await;
        let state = state_against(&base);
        let (dev, project) = funded_agent(&state, 93);
        let r = avatar_generate(
            State(state.clone()),
            dev.clone(),
            Path(project.clone()),
            Json(AvatarGenerateBody {
                prompt: "a forbidden picture".into(),
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"]["type"], "avatar_refused");
        assert_eq!(v["error"]["message"], AVATAR_REFUSED_FRIENDLY);
        assert!(v["error"]["detail"]
            .as_str()
            .unwrap()
            .contains("rejected by the safety system"));
        assert_eq!(seen.lock().unwrap().len(), 1, "no retry, no next model");
        assert_eq!(
            state
                .db
                .usage_count_since(&project, "openai-images", "avatar:", 0)
                .unwrap(),
            0,
            "a refusal is not metered"
        );
        // The own-profile route: the same answer.
        let r = me_avatar_generate(
            State(state.clone()),
            dev.clone(),
            Json(AvatarGenerateBody {
                prompt: "another forbidden one".into(),
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
        // Jacob's exact provider body is a refusal, not a model rejection.
        let body = br#"{"error":{"message":"Your request was rejected by the safety system. If you believe this is an error, contact us at help.openai.com and include the request ID req_e26ae2ab492d48b7bafa7e6b19440e8f.","type":"image_generation_user_error","param":null,"code":"moderation_blocked"}}"#;
        assert!(is_prompt_refusal(400, body));
        assert!(!is_unknown_model_error(400, body));
        assert!(!is_prompt_refusal(500, body), "a 5xx is never a refusal");
    }

    /// Jacob's "nuke vampire" (2026-10-26 17:4x UTC): the provider stalled past the apps' 30 s,
    /// the relay had no timeout, the client gave up and the dropped handler logged nothing. Now
    /// every attempt has a timeout: one stall then an answer → the picture (one retry); stalls
    /// past the whole budget → a clean 504 `avatar_timeout` with the plain line, well inside the
    /// apps' 90 s.
    #[tokio::test]
    async fn a_stalled_provider_is_cut_and_retried_then_a_clean_504() {
        // One stall, then the picture.
        let (base, seen) = stub_with(&[], 1).await;
        let mut state = state_against(&base);
        {
            let cfg = Arc::get_mut(&mut state.cfg).unwrap();
            cfg.avatar_retry_backoff_ms = vec![0, 0];
            cfg.avatar_attempt_timeout = std::time::Duration::from_millis(300);
            cfg.avatar_total_timeout = std::time::Duration::from_secs(5);
        }
        let (dev, project) = funded_agent(&state, 95);
        let t0 = Instant::now();
        let r = avatar_generate(
            State(state.clone()),
            dev,
            Path(project.clone()),
            Json(AvatarGenerateBody {
                prompt: "a slow sloth".into(),
            }),
        )
        .await;
        assert_eq!(
            r.status(),
            StatusCode::OK,
            "the retry after the stall lands the picture"
        );
        assert_eq!(seen.lock().unwrap().len(), 2);
        assert!(
            t0.elapsed() < std::time::Duration::from_secs(3),
            "{:?}",
            t0.elapsed()
        );

        // Every call stalls: the budget ends it with a 504 and the plain sentence.
        let (base, seen) = stub_with(&[], 99).await;
        let mut state = state_against(&base);
        {
            let cfg = Arc::get_mut(&mut state.cfg).unwrap();
            cfg.avatar_retry_backoff_ms = vec![0, 0];
            cfg.avatar_attempt_timeout = std::time::Duration::from_millis(300);
            cfg.avatar_total_timeout = std::time::Duration::from_millis(1_000);
        }
        let (dev, project) = funded_agent(&state, 96);
        let t0 = Instant::now();
        let r = avatar_generate(
            State(state.clone()),
            dev,
            Path(project.clone()),
            Json(AvatarGenerateBody {
                prompt: "a slow sloth".into(),
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::GATEWAY_TIMEOUT);
        assert!(
            t0.elapsed() < std::time::Duration::from_secs(3),
            "{:?}",
            t0.elapsed()
        );
        let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"]["type"], "avatar_timeout");
        assert_eq!(v["error"]["message"], AVATAR_TIMEOUT_FRIENDLY);
        assert!(v["error"]["detail"].as_str().unwrap().contains("gpt-image"));
        assert!(
            seen.lock().unwrap().len() >= 2,
            "the budget allowed more than one attempt"
        );
        assert_eq!(
            state
                .db
                .usage_count_since(&project, "openai-images", "avatar:", 0)
                .unwrap(),
            0,
            "nothing metered"
        );
    }

    /// A transient provider failure (503 / 429 / a dropped connection) gets the backoff retries
    /// on the same model before the fall-through; the picture still arrives.
    #[tokio::test]
    async fn a_transient_provider_failure_is_retried_then_succeeds() {
        let (base, seen) = stub_with(&[], 2).await;
        let mut state = state_against(&base);
        Arc::get_mut(&mut state.cfg)
            .unwrap()
            .avatar_retry_backoff_ms = vec![0, 0];
        let (dev, project) = funded_agent(&state, 94);
        let r = avatar_generate(
            State(state.clone()),
            dev,
            Path(project.clone()),
            Json(AvatarGenerateBody {
                prompt: "a patient otter".into(),
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        let calls = seen.lock().unwrap();
        assert_eq!(calls.len(), 3, "two 503s, then the picture");
        assert!(
            calls.iter().all(|c| c["model"] == "gpt-image-1-mini"),
            "retries stay on the same model"
        );
        assert!(
            is_transient_upstream(503) && is_transient_upstream(429) && !is_transient_upstream(400)
        );
    }

    /// The provider probe: `GET /models/{first model}` — ok against a live stub, not ok when the
    /// model is gone; the record lands in state for `healthz.images` and the admin route.
    #[tokio::test]
    async fn the_provider_probe_records_health() {
        let (base, _) = stub(&[]).await;
        let state = state_against(&base);
        let h = probe_provider(&state).await;
        assert!(h.ok, "{h:?}");
        assert_eq!(h.model, avatar_gen_models()[0]);
        assert!(state
            .image_health
            .lock()
            .await
            .as_ref()
            .is_some_and(|r| r.ok));
        let (base, _) = stub(&["gpt-image-1-mini"]).await;
        let state = state_against(&base);
        let h = probe_provider(&state).await;
        assert!(!h.ok);
        assert_eq!(h.status, 404);
        assert!(h.detail.contains("does not exist"));
        // Admin only.
        let r = admin_image_health(
            State(state.clone()),
            Principal::Box {
                user_id: "u".into(),
            },
            axum::extract::Query(Default::default()),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        let r = admin_image_health(
            State(state.clone()),
            Principal::Admin,
            axum::extract::Query(Default::default()),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["images"]["ok"], false);
    }

    /// One REAL generation against api.openai.com with `OPENAI_API_KEY` (≈ $0.005). Off unless
    /// `ARBOS_AVATAR_SMOKE=1`, so CI stays offline; `scripts/avatars/smoke-avatar-generate.sh`
    /// runs it (or the production relay) end to end.
    #[tokio::test]
    async fn smoke_real_generation_when_asked() {
        if std::env::var("ARBOS_AVATAR_SMOKE").ok().as_deref() != Some("1") {
            return;
        }
        let key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY for the smoke test");
        let mut state = test_state(AuthMode::Github);
        {
            let cfg = Arc::get_mut(&mut state.cfg).unwrap();
            cfg.openai_key = Some(key);
            cfg.openai_images_base = std::env::var("ARBOS_OPENAI_IMAGES_BASE")
                .unwrap_or_else(|_| "https://api.openai.com/v1".into());
        }
        let (alice_dev, pid) = funded_agent(&state, 83);
        let (status, v) = generate(&state, &alice_dev, &pid).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        let key = v["image_key"].as_str().unwrap();
        // The stub paints a flat placeholder, so the master is PNG; a real painting is JPEG q92.
        assert!(matches!(
            crate::blobs::stored_image(&state, &pid, key).await,
            crate::blobs::StoredImage::Image(
                crate::image_sniff::ImageKind::Jpeg | crate::image_sniff::ImageKind::Png
            )
        ));
        eprintln!("smoke: {} generated {key}", v["model"]);
    }
}
