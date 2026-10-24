//! Image generation through the relay (F10, D2): `POST /proxy/v1/images/generations` goes to
//! OpenAI with the relay's key and is metered per project like a model call. With
//! `ARBOS_IMAGE_MOCK=1` the relay answers itself with a labelled placeholder PNG so the whole
//! flow runs without a key.

use std::io::Cursor;
use std::time::Instant;

use axum::extract::State;
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
/// The stored picture: a 512 px JPEG, like the pictures the apps upload themselves.
const AVATAR_SIDE: u32 = 512;

fn avatar_error(status: StatusCode, kind: &str, message: impl Into<String>) -> Response {
    (
        status,
        Json(json!({ "error": { "type": kind, "message": message.into() } })),
    )
        .into_response()
}

/// PNG (or anything `image` decodes) → 512 px JPEG q85 for the blob store.
pub fn avatar_jpeg(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let img = image::load_from_memory(bytes).map_err(|e| format!("decode: {e}"))?;
    let img = img.resize(
        AVATAR_SIDE,
        AVATAR_SIDE,
        image::imageops::FilterType::Triangle,
    );
    let rgb = image::DynamicImage::ImageRgb8(img.to_rgb8());
    let mut out = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85);
    enc.encode_image(&rgb).map_err(|e| format!("encode: {e}"))?;
    Ok(out)
}

#[derive(serde::Deserialize)]
pub struct AvatarGenerateBody {
    pub prompt: String,
}

/// `POST /projects/{id}/avatar/generate {prompt}` — one square picture for the agent from a short
/// prompt, through the relay's OpenAI key, stored in the project's blob slot and **not yet set**:
/// the answer is `{image_key}` for the picker's preview, and "Set avatar" makes it the picture via
/// `PATCH /projects/{id} {image_key}`. Metered like `/proxy/v1/images/generations` (usage row,
/// credits, the project's daily spend cap) plus a per-project daily count cap. `ARBOS_IMAGE_MOCK=1`
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
    let prompt: String = body.prompt.trim().chars().take(400).collect();
    if prompt.is_empty() {
        return avatar_error(StatusCode::BAD_REQUEST, "bad_prompt", "prompt is required");
    }
    let since = crate::proxy::start_of_utc_day_ms();
    let made_today = state
        .db
        .usage_count_since(&project.id, "openai-images", "avatar:", since)
        .unwrap_or(0);
    if made_today >= avatar_gen_daily_cap() {
        return avatar_error(
            StatusCode::TOO_MANY_REQUESTS,
            "avatar_gen_cap",
            format!(
                "{} generated pictures today is the limit for one agent; try again tomorrow or upload one",
                avatar_gen_daily_cap()
            ),
        );
    }
    let models = avatar_gen_models();
    let mut model = format!("avatar:{}", models[0]);
    let t0 = Instant::now();
    let png: Vec<u8> = if state.cfg.image_mock {
        let _ = state.db.record_usage(
            &project.id,
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
        if let Err(r) = crate::credits::check_call(&state, &principal, &project.id) {
            return crate::credits::refused_response(&r);
        }
        let today = state
            .db
            .usage_since(&project.id, since)
            .map(|u| u.usd)
            .unwrap_or(0.0);
        if today >= project.spend_cap_daily {
            return avatar_error(
                StatusCode::PAYMENT_REQUIRED,
                "spend_cap",
                format!("daily spend cap ${:.2} reached", project.spend_cap_daily),
            );
        }
        let url = format!(
            "{}/images/generations",
            state.cfg.openai_images_base.trim_end_matches('/')
        );
        let mut picked: Option<(String, Value)> = None;
        let mut last_detail = String::from("no model answered");
        for candidate in &models {
            let upstream = match state
                .http
                .post(&url)
                .bearer_auth(&key)
                .json(&avatar_request(candidate, &prompt))
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => return avatar_unavailable(&project.id, format!("{candidate}: {e}")),
            };
            let status = upstream.status();
            let bytes = upstream.bytes().await.unwrap_or_default();
            if status.is_success() {
                let parsed: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                picked = Some((candidate.clone(), parsed));
                break;
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
            if is_unknown_model_error(status.as_u16(), &bytes) {
                tracing::info!(project = %project.id, model = %candidate, %message, "avatar model rejected; trying the next");
                continue;
            }
            return avatar_unavailable(&project.id, last_detail);
        }
        let Some((used, parsed)) = picked else {
            return avatar_unavailable(&project.id, last_detail);
        };
        model = format!("avatar:{used}");
        let Some(b64) = parsed.pointer("/data/0/b64_json").and_then(Value::as_str) else {
            return avatar_unavailable(&project.id, format!("{used}: no picture in the answer"));
        };
        let Ok(png) = base64::engine::general_purpose::STANDARD.decode(b64) else {
            return avatar_unavailable(&project.id, format!("{used}: picture was not base64"));
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
            &project.id,
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
        crate::credits::charge_call(
            &state,
            &principal,
            &project.id,
            usd,
            "inference",
            Some(&model),
        );
        png
    };
    let jpeg = match avatar_jpeg(&png) {
        Ok(j) => j,
        Err(e) => return avatar_unavailable(&project.id, e),
    };
    let image_key = format!("avatar-gen-{}.jpg", arbos_core::ids::ulid().to_lowercase());
    if let Err(e) = crate::blobs::store(&state, &project.id, &image_key, "image/jpeg", &jpeg).await
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
    /// OpenAI does, answers a 1 × 1 PNG for the rest.
    async fn stub(unknown: &'static [&'static str]) -> (String, Arc<Mutex<Vec<Value>>>) {
        let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
        let seen_in = seen.clone();
        let app = Router::new().route(
            "/images/generations",
            post(move |Json(body): Json<Value>| {
                let seen = seen_in.clone();
                async move {
                    seen.lock().unwrap().push(body.clone());
                    let model = body["model"].as_str().unwrap_or("");
                    if unknown.contains(&model) {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({ "error": {
                                "message": format!("The model '{model}' does not exist."),
                                "type": "image_generation_user_error", "param": "model", "code": "invalid_value" } })),
                        );
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
    async fn falls_through_to_the_next_model_and_stores_a_jpeg() {
        // The default list is tried in order; the stub rejects the first the way OpenAI rejected
        // gpt-image-1 (no env override: these tests run in parallel threads).
        let (base, seen) = stub(&["gpt-image-1-mini"]).await;
        let state = state_against(&base);
        let (alice_dev, pid) = funded_agent(&state, 81);
        let (status, v) = generate(&state, &alice_dev, &pid).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        assert_eq!(v["model"], "gpt-image-2");
        let key = v["image_key"].as_str().unwrap();
        assert!(matches!(
            crate::blobs::stored_image(&state, &pid, key).await,
            crate::blobs::StoredImage::Image(crate::image_sniff::ImageKind::Jpeg)
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
        assert!(matches!(
            crate::blobs::stored_image(&state, &pid, key).await,
            crate::blobs::StoredImage::Image(crate::image_sniff::ImageKind::Jpeg)
        ));
        eprintln!("smoke: {} generated {key}", v["model"]);
    }
}
