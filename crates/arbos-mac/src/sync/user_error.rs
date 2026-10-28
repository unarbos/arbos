//! One classifier for every failure the user sees (Jacob, 2026-10-26, [feedback
//! error-messages-mac-2026-10-26]): a raw transport / relay / IO error becomes a [`UserError`] —
//! a **verb** (what the user tried: "generate the image"), a **class** (why it failed), the
//! **call** it was (for the Details disclosure) and the **raw** words (for Copy). The UI draws
//! every one of them through `ui::error_note` and nothing else: no raw URLs, HTTP verbs, ids or
//! error chains ever reach a sentence.
//!
//! Sentence shape: "Couldn't <verb>. <why>." — the why-clause is one of the ten below.
//!
//! | class | why | action |
//! |---|---|---|
//! | Timeout | The server took too long. | Try again |
//! | Offline | You're offline. / Arbos can't reach the server right now. | Try again |
//! | Refused (4xx with the relay's words) | the relay's sentence, else "That wasn't accepted." | — |
//! | NotFound | It isn't there any more. | — |
//! | Server (5xx) | The server had a problem. | Try again |
//! | RateLimited (429) | Too many requests — wait a moment. | Try again |
//! | ContentRefused | That prompt was refused — try different words. | — |
//! | Upload (read / size / type) | The file couldn't be read. / … too large … / … type isn't supported. | Try again (read only) |
//! | AuthExpired (401 / 403) | You were signed out. | Sign in |
//! | Unknown | Something went wrong. | Try again |

use std::time::{SystemTime, UNIX_EPOCH};

use super::relay::RelayError;

/// Why it failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorClass {
    Timeout,
    /// `device_offline`: no route at all (DNS, unreachable); else the relay does not answer.
    Offline {
        device_offline: bool,
    },
    /// A 4xx the relay explained (`message` is its sentence when it has one).
    Refused {
        message: Option<String>,
    },
    NotFound,
    Server,
    RateLimited,
    ContentRefused,
    Upload(UploadFault),
    AuthExpired,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadFault {
    Unreadable,
    TooLarge,
    Unsupported,
}

/// What the note offers after the sentence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    TryAgain,
    SignIn,
    None,
}

/// Where the classifier found its answer (for the Details block's first line).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserError {
    /// What the user tried, as an infinitive phrase: "send your message", "save the secret".
    pub verb: String,
    pub class: ErrorClass,
    /// The request, when known: "POST /projects/01M5N1…ESK7X/avatar/generate".
    pub call: Option<String>,
    /// The untrimmed raw words (an `anyhow` chain, the relay's body, the OS error).
    pub raw: String,
    /// Unix ms when it happened.
    pub at_ms: u64,
}

/// What a call site has in hand when something failed: the error, the HTTP status of a
/// response it did not turn into an error, or the words alone.
pub enum Fault<'a> {
    Any(&'a anyhow::Error),
    Req(&'a reqwest::Error),
    Status(u16),
    Text(String),
}

impl<'a> From<&'a anyhow::Error> for Fault<'a> {
    fn from(e: &'a anyhow::Error) -> Self {
        Fault::Any(e)
    }
}
impl<'a> From<&'a reqwest::Error> for Fault<'a> {
    fn from(e: &'a reqwest::Error) -> Self {
        Fault::Req(e)
    }
}
impl From<reqwest::StatusCode> for Fault<'_> {
    fn from(s: reqwest::StatusCode) -> Self {
        Fault::Status(s.as_u16())
    }
}
impl From<&String> for Fault<'_> {
    fn from(s: &String) -> Self {
        Fault::Text(s.clone())
    }
}
impl From<String> for Fault<'_> {
    fn from(s: String) -> Self {
        Fault::Text(s)
    }
}
impl From<&str> for Fault<'_> {
    fn from(s: &str) -> Self {
        Fault::Text(s.to_owned())
    }
}
impl<'a> From<&'a std::io::Error> for Fault<'a> {
    fn from(e: &'a std::io::Error) -> Self {
        Fault::Text(e.to_string())
    }
}

impl Fault<'_> {
    pub fn classify(self, verb: &str) -> UserError {
        match self {
            Fault::Any(e) => UserError::classify(verb, e),
            Fault::Req(e) => UserError::classify_reqwest(verb, e),
            Fault::Status(s) => UserError::from_status(verb, s, None),
            Fault::Text(t) => UserError::from_text(verb, t),
        }
    }
}

/// A sentence the relay sent that is already human (its `error.message`): used verbatim.
fn relay_sentence(e: &RelayError) -> Option<String> {
    let m = e.message.trim();
    if m.is_empty() || m.starts_with("HTTP ") || m.len() > 160 || m.contains("://") {
        return None;
    }
    Some(m.to_owned())
}

impl UserError {
    pub fn new(verb: impl Into<String>, class: ErrorClass, raw: impl Into<String>) -> Self {
        let raw = raw.into();
        Self {
            verb: verb.into(),
            class,
            call: call_in(&raw),
            raw,
            at_ms: now_ms(),
        }
    }

    /// Classify an `anyhow` error from the relay layer: a typed [`RelayError`] first, then the
    /// reqwest / io causes, then the words of the chain.
    pub fn classify(verb: impl Into<String>, err: &anyhow::Error) -> Self {
        if let Some(e) = err.downcast_ref::<RelayError>() {
            return Self::from_relay(verb, e);
        }
        let raw = format!("{err:#}");
        let mut class = None;
        for cause in err.chain() {
            if let Some(req) = cause.downcast_ref::<reqwest::Error>() {
                if req.is_timeout() {
                    class = Some(ErrorClass::Timeout);
                } else if req.is_connect() {
                    class = Some(ErrorClass::Offline {
                        device_offline: raw_says_offline(&raw),
                    });
                } else if let Some(status) = req.status() {
                    class = Some(class_for_status(status.as_u16(), None, None));
                }
                break;
            }
            if let Some(io) = cause.downcast_ref::<std::io::Error>() {
                use std::io::ErrorKind as K;
                class = Some(match io.kind() {
                    K::TimedOut => ErrorClass::Timeout,
                    K::NotFound => ErrorClass::NotFound,
                    K::ConnectionRefused | K::ConnectionReset | K::ConnectionAborted => {
                        ErrorClass::Offline {
                            device_offline: false,
                        }
                    }
                    K::PermissionDenied | K::InvalidData | K::UnexpectedEof => {
                        ErrorClass::Upload(UploadFault::Unreadable)
                    }
                    _ => ErrorClass::Unknown,
                });
                break;
            }
        }
        let class = class.unwrap_or_else(|| class_for_words(&raw));
        Self::new(verb, class, raw)
    }

    /// A bare `reqwest::Error` (a response the caller sent itself).
    pub fn classify_reqwest(verb: impl Into<String>, req: &reqwest::Error) -> Self {
        let raw = format!("{req:#}");
        let class = if req.is_timeout() {
            ErrorClass::Timeout
        } else if req.is_connect() {
            ErrorClass::Offline {
                device_offline: raw_says_offline(&raw),
            }
        } else if let Some(status) = req.status() {
            class_for_status(status.as_u16(), None, None)
        } else {
            class_for_words(&raw)
        };
        Self::new(verb, class, raw)
    }

    /// Classify a typed relay answer.
    pub fn from_relay(verb: impl Into<String>, e: &RelayError) -> Self {
        let class = class_for_status(e.status, Some(e.kind.as_str()), relay_sentence(e));
        let mut raw = format!("HTTP {} {}", e.status, e.kind)
            .trim_end()
            .to_owned();
        if !e.message.is_empty() {
            raw.push_str(" — ");
            raw.push_str(&e.message);
        }
        if let Some(d) = &e.detail {
            raw.push_str(" — ");
            raw.push_str(d);
        }
        Self::new(verb, class, raw)
    }

    /// Classify an HTTP status alone (a response the caller did not turn into an error).
    pub fn from_status(verb: impl Into<String>, status: u16, body: Option<&str>) -> Self {
        let e = RelayError::from_body(
            reqwest::StatusCode::from_u16(status)
                .unwrap_or(reqwest::StatusCode::INTERNAL_SERVER_ERROR),
            body.unwrap_or_default(),
        );
        Self::from_relay(verb, &e)
    }

    /// Classify a failure that only exists as text (a relay frame's `message`, an OS error
    /// already formatted, a subprocess's stderr).
    pub fn from_text(verb: impl Into<String>, raw: impl Into<String>) -> Self {
        let raw = raw.into();
        Self::new(verb, class_for_words(&raw), raw)
    }

    /// A failure the code itself diagnosed — the class is known, the raw words are a note.
    pub fn of(verb: impl Into<String>, class: ErrorClass, note: impl Into<String>) -> Self {
        Self::new(verb, class, note)
    }

    /// The one sentence the user reads. Chat-level notes pass their own words as the verb
    /// with an empty class note; everything else is "Couldn't <verb>. <why>."
    pub fn sentence(&self) -> String {
        let why = self.why();
        if self.verb.is_empty() {
            return why;
        }
        if why.is_empty() {
            format!("Couldn't {}.", self.verb)
        } else {
            format!("Couldn't {}. {why}", self.verb)
        }
    }

    /// The why-clause alone.
    pub fn why(&self) -> String {
        match &self.class {
            ErrorClass::Timeout => "The server took too long.".into(),
            ErrorClass::Offline {
                device_offline: true,
            } => "You're offline.".into(),
            ErrorClass::Offline {
                device_offline: false,
            } => "Arbos can't reach the server right now.".into(),
            ErrorClass::Refused { message: Some(m) } => ensure_period(m),
            ErrorClass::Refused { message: None } => "That wasn't accepted.".into(),
            ErrorClass::NotFound => "It isn't there any more.".into(),
            ErrorClass::Server => "The server had a problem.".into(),
            ErrorClass::RateLimited => "Too many requests — wait a moment.".into(),
            ErrorClass::ContentRefused => "That prompt was refused — try different words.".into(),
            ErrorClass::Upload(UploadFault::Unreadable) => "The file couldn't be read.".into(),
            ErrorClass::Upload(UploadFault::TooLarge) => {
                "That file is too large (25 MB max).".into()
            }
            ErrorClass::Upload(UploadFault::Unsupported) => {
                "That file type isn't supported.".into()
            }
            ErrorClass::AuthExpired => "You were signed out.".into(),
            ErrorClass::Unknown => "Something went wrong.".into(),
        }
    }

    /// Try again where a retry can help; Sign in on 401 / 403; nothing where the input must
    /// change (content refused, 404, size / type, a 4xx with the relay's reason).
    pub fn action(&self) -> Action {
        match &self.class {
            ErrorClass::Timeout
            | ErrorClass::Offline { .. }
            | ErrorClass::Server
            | ErrorClass::RateLimited
            | ErrorClass::Unknown
            | ErrorClass::Upload(UploadFault::Unreadable) => Action::TryAgain,
            ErrorClass::AuthExpired => Action::SignIn,
            ErrorClass::Refused { .. }
            | ErrorClass::NotFound
            | ErrorClass::ContentRefused
            | ErrorClass::Upload(UploadFault::TooLarge)
            | ErrorClass::Upload(UploadFault::Unsupported) => Action::None,
        }
    }

    /// The Details block: what · call · where / when / build — three short lines, ids
    /// shortened in the middle, never the whole chain (that is [`Self::copy_text`]).
    pub fn details(&self) -> Vec<String> {
        let mut out = vec![self.what_line()];
        if let Some(call) = &self.call {
            out.push(shorten_ids(call));
        }
        out.push(format!(
            "{} · {} · Mac {}",
            host_in(&self.raw).unwrap_or_else(|| "this Mac".into()),
            stamp(self.at_ms),
            env!("CARGO_PKG_VERSION")
        ));
        out
    }

    /// What goes on the pasteboard for a feedback report: the three lines plus the raw words.
    pub fn copy_text(&self) -> String {
        let mut text = self.sentence();
        text.push('\n');
        text.push_str(&self.details().join("\n"));
        if !self.raw.is_empty() {
            text.push_str("\n\n");
            text.push_str(&self.raw);
        }
        text
    }

    fn what_line(&self) -> String {
        let measured = timeout_seconds(&self.raw);
        let status = status_in(&self.raw);
        match (&self.class, status, measured) {
            (ErrorClass::Timeout, _, Some(s)) => format!("Timed out after {s} s"),
            (ErrorClass::Timeout, _, None) => "Timed out".into(),
            (ErrorClass::Offline { device_offline }, _, _) => {
                if *device_offline {
                    "No network route".into()
                } else {
                    "Connection refused or reset".into()
                }
            }
            (_, Some((code, reason)), _) if !reason.is_empty() => {
                format!("HTTP {code} {reason} from the relay")
            }
            (_, Some((code, _)), _) => format!("HTTP {code} from the relay"),
            (ErrorClass::Upload(_), _, _) => "The file could not be read or encoded".into(),
            _ => first_line(&self.raw).unwrap_or_else(|| "Unexpected failure".into()),
        }
    }
}

fn ensure_period(s: &str) -> String {
    let s = s.trim();
    if s.ends_with(['.', '!', '?']) {
        s.to_owned()
    } else {
        format!("{s}.")
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Classes by HTTP status; `kind` is the relay's `error.type`.
fn class_for_status(status: u16, kind: Option<&str>, sentence: Option<String>) -> ErrorClass {
    let kind = kind.unwrap_or_default();
    if kind.contains("refused") && (kind.contains("prompt") || kind.contains("content"))
        || kind == "moderation"
        || kind == "content_policy"
    {
        return ErrorClass::ContentRefused;
    }
    if kind.contains("timeout") {
        return ErrorClass::Timeout;
    }
    if kind == "too_large" || kind == "payload_too_large" {
        return ErrorClass::Upload(UploadFault::TooLarge);
    }
    if kind == "unsupported_media" || kind == "unsupported_type" {
        return ErrorClass::Upload(UploadFault::Unsupported);
    }
    // A 403 that is a permission answer, not a dead session: the relay names the rule
    // (`admin_only`, `owner_only`, `not_member`, …) and says it in one sentence. The bare
    // `forbidden` / `token_expired` kinds are the session.
    if status == 403
        && !kind.is_empty()
        && !matches!(kind, "forbidden" | "token_expired" | "unauthorized")
    {
        return ErrorClass::Refused { message: sentence };
    }
    match status {
        401 | 403 => ErrorClass::AuthExpired,
        404 | 410 => ErrorClass::NotFound,
        408 | 504 => ErrorClass::Timeout,
        413 => ErrorClass::Upload(UploadFault::TooLarge),
        415 => ErrorClass::Upload(UploadFault::Unsupported),
        429 => ErrorClass::RateLimited,
        400..=499 => ErrorClass::Refused { message: sentence },
        500..=599 => ErrorClass::Server,
        _ => ErrorClass::Unknown,
    }
}

fn raw_says_offline(raw: &str) -> bool {
    let d = raw.to_ascii_lowercase();
    d.contains("dns error")
        || d.contains("failed to lookup address")
        || d.contains("name or service not known")
        || d.contains("nodename nor servname")
        || d.contains("no address associated")
        || d.contains("network is unreachable")
        || d.contains("host is down")
        || d.contains("network down")
}

/// The words alone (no typed cause): the same signals `diagnostics::is_transient` reads.
fn class_for_words(raw: &str) -> ErrorClass {
    let d = raw.to_ascii_lowercase();
    if let Some((code, _)) = status_in(raw) {
        return class_for_status(code, None, None);
    }
    if d.contains("timed out")
        || d.contains("timeout")
        || d.contains("took too long")
        || d.contains("did not answer in time")
    {
        return ErrorClass::Timeout;
    }
    if raw_says_offline(raw) {
        return ErrorClass::Offline {
            device_offline: true,
        };
    }
    if d.contains("error sending request")
        || d.contains("connection refused")
        || d.contains("connection reset")
        || d.contains("broken pipe")
        || d.contains("tls handshake")
        || d.contains("connection closed")
        || d.contains("can't reach")
        || d.contains("cannot reach")
        || d.contains("could not reach")
        || d.contains("not reachable")
    {
        return ErrorClass::Offline {
            device_offline: false,
        };
    }
    if d.contains("rate limit") || d.contains("too many requests") {
        return ErrorClass::RateLimited;
    }
    if d.contains("prompt was refused") || d.contains("content policy") || d.contains("moderation")
    {
        return ErrorClass::ContentRefused;
    }
    if d.contains("too large") {
        return ErrorClass::Upload(UploadFault::TooLarge);
    }
    if d.contains("unsupported") && (d.contains("type") || d.contains("format")) {
        return ErrorClass::Upload(UploadFault::Unsupported);
    }
    if d.contains("signed out") || d.contains("unauthorized") || d.contains("token expired") {
        return ErrorClass::AuthExpired;
    }
    if d.contains("not found") || d.contains("no such file") {
        return ErrorClass::NotFound;
    }
    if d.contains("bad gateway") || d.contains("gateway timeout") || d.contains("server error") {
        return ErrorClass::Server;
    }
    ErrorClass::Unknown
}

/// `HTTP 504 avatar_timeout …` / `HTTP 404` → (504, "avatar_timeout").
fn status_in(raw: &str) -> Option<(u16, String)> {
    let idx = raw.find("HTTP ")?;
    let rest = &raw[idx + 5..];
    let code: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let code: u16 = code.parse().ok()?;
    if !(100..=599).contains(&code) {
        return None;
    }
    let reason = rest[code.to_string().len()..]
        .trim_start()
        .split([' ', '—', ':', '\n'])
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned();
    // The relay's `error.type` is snake_case ("avatar_timeout"); prose ("Bad Gateway") is not
    // a reason token.
    let reason = if !reason.is_empty()
        && reason
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        reason
    } else {
        String::new()
    };
    Some((code, reason))
}

/// "… operation timed out after 40 s …" → 40.
fn timeout_seconds(raw: &str) -> Option<u64> {
    let d = raw.to_ascii_lowercase();
    let idx = d.find("after ")?;
    let n: String = d[idx + 6..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    n.parse().ok()
}

/// "POST /projects/x/avatar/generate" (an anyhow context) → the call line.
fn call_in(raw: &str) -> Option<String> {
    for verb in ["POST ", "GET ", "PUT ", "DELETE ", "PATCH "] {
        if let Some(idx) = raw.find(verb) {
            let rest = &raw[idx..];
            let line = rest
                .split([':', '\n', ')'])
                .next()
                .unwrap_or_default()
                .trim();
            if line.len() > verb.len() {
                return Some(line.to_owned());
            }
        }
    }
    None
}

/// The host out of a URL in the raw words ("https://relay.arbos.life/…" → "relay.arbos.life").
fn host_in(raw: &str) -> Option<String> {
    let idx = raw.find("://")?;
    let host: String = raw[idx + 3..]
        .chars()
        .take_while(|c| !matches!(c, '/' | ')' | ' ' | ':' | '\n'))
        .collect();
    (!host.is_empty()).then_some(host)
}

/// ULIDs and long tokens shortened in the middle: `01M5N1WSR91G8CJKNYXKJESK7X` → `01M5N1…ESK7X`.
pub fn shorten_ids(s: &str) -> String {
    s.split('/')
        .map(|seg| {
            let alnum = seg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
            if alnum && seg.len() >= 20 && seg.chars().any(|c| c.is_ascii_digit()) {
                format!("{}…{}", &seg[..6], &seg[seg.len() - 5..])
            } else {
                seg.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn first_line(raw: &str) -> Option<String> {
    let line = raw.lines().next()?.trim();
    if line.is_empty() {
        return None;
    }
    let mut out: String = line.chars().take(120).collect();
    if out.len() < line.len() {
        out.push('…');
    }
    Some(out)
}

/// "17:41:05 UTC" from unix ms (no chrono: a plain civil-time split).
fn stamp(ms: u64) -> String {
    let secs = ms / 1000;
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    format!("{h:02}:{m:02}:{s:02} UTC")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relay(status: u16, body: &str) -> anyhow::Error {
        anyhow::Error::from(RelayError::from_body(
            reqwest::StatusCode::from_u16(status).unwrap(),
            body,
        ))
    }

    #[test]
    fn the_ten_classes_make_the_agreed_sentences() {
        let raw = anyhow::anyhow!("error sending request for url (https://relay.arbos.life/projects/01M5N1WSR91G8CJKNYXKJESK7X/avatar/generate): operation timed out")
            .context("POST /projects/01M5N1WSR91G8CJKNYXKJESK7X/avatar/generate");
        let e = UserError::classify("generate the image", &raw);
        assert_eq!(e.class, ErrorClass::Timeout);
        assert_eq!(
            e.sentence(),
            "Couldn't generate the image. The server took too long."
        );
        assert_eq!(e.action(), Action::TryAgain);
        let d = e.details();
        assert_eq!(d[0], "Timed out");
        assert_eq!(d[1], "POST /projects/01M5N1…ESK7X/avatar/generate");
        assert!(d[2].starts_with("relay.arbos.life · "), "{}", d[2]);
        assert!(e.copy_text().contains("operation timed out"));

        let dns = anyhow::anyhow!("dns error: failed to lookup address information")
            .context("POST /chats/x/messages");
        let e = UserError::classify("send your message", &dns);
        assert_eq!(e.sentence(), "Couldn't send your message. You're offline.");
        assert_eq!(e.details()[0], "No network route");

        let e = UserError::classify(
            "save the secret",
            &relay(
                409,
                r#"{"error":{"type":"exists","message":"A secret with that name already exists"}}"#,
            ),
        );
        assert_eq!(
            e.sentence(),
            "Couldn't save the secret. A secret with that name already exists."
        );
        assert_eq!(e.action(), Action::None);

        let e = UserError::classify("open the file", &relay(404, ""));
        assert_eq!(
            e.sentence(),
            "Couldn't open the file. It isn't there any more."
        );
        assert_eq!(e.action(), Action::None);

        let e = UserError::classify("save the secret", &relay(502, "Bad Gateway"));
        assert_eq!(
            e.sentence(),
            "Couldn't save the secret. The server had a problem."
        );
        assert_eq!(e.details()[0], "HTTP 502 from the relay");

        let e = UserError::classify("send your message", &relay(429, ""));
        assert_eq!(e.class, ErrorClass::RateLimited);
        assert_eq!(e.action(), Action::TryAgain);

        let e = UserError::classify(
            "generate the image",
            &relay(
                422,
                r#"{"error":{"type":"prompt_refused","message":"That prompt was refused — try a different one."}}"#,
            ),
        );
        assert_eq!(
            e.sentence(),
            "Couldn't generate the image. That prompt was refused — try different words."
        );
        assert_eq!(e.action(), Action::None);

        let e = UserError::classify("upload the picture", &relay(413, ""));
        assert_eq!(
            e.sentence(),
            "Couldn't upload the picture. That file is too large (25 MB max)."
        );
        assert_eq!(e.action(), Action::None);

        let e = UserError::classify("send your message", &relay(401, ""));
        assert_eq!(
            e.sentence(),
            "Couldn't send your message. You were signed out."
        );
        assert_eq!(e.action(), Action::SignIn);

        let e = UserError::from_text("save the secret", "boom");
        assert_eq!(
            e.sentence(),
            "Couldn't save the secret. Something went wrong."
        );
        assert_eq!(e.action(), Action::TryAgain);
    }

    /// A permission 403 (`admin_only`) is the relay's sentence, not "You were signed out".
    #[test]
    fn a_permission_403_reads_as_the_relays_sentence() {
        let e = UserError::classify(
            "rename the group",
            &relay(
                403,
                r#"{"error":{"type":"admin_only","message":"Only the group's owner and admins can change its name, picture and description."}}"#,
            ),
        );
        assert_eq!(
            e.sentence(),
            "Couldn't rename the group. Only the group's owner and admins can change its name, picture and description."
        );
        assert_eq!(e.action(), Action::None);
        // A session 403 still signs out.
        let e = UserError::classify(
            "send your message",
            &relay(
                403,
                r#"{"error":{"type":"forbidden","message":"forbidden"}}"#,
            ),
        );
        assert_eq!(e.class, ErrorClass::AuthExpired);
    }

    #[test]
    fn a_relay_timeout_with_detail_keeps_the_measured_seconds_for_details() {
        let e = UserError::classify(
            "generate the image",
            &relay(
                504,
                r#"{"error":{"type":"avatar_timeout","message":"Couldn't generate the image. The server took too long.","detail":"gpt-image-1-mini: operation timed out after 40 s (attempt 2 of 2)"}}"#,
            ),
        );
        assert_eq!(e.class, ErrorClass::Timeout);
        assert_eq!(e.details()[0], "Timed out after 40 s");
        assert!(e.raw.contains("gpt-image-1-mini"));
    }

    #[test]
    fn sentences_never_carry_urls_verbs_ids_or_chains() {
        let raw = anyhow::anyhow!("error sending request for url (https://relay.arbos.life/projects/01M5N1WSR91G8CJKNYXKJESK7X/avatar/generate): connection refused")
            .context("POST /projects/01M5N1WSR91G8CJKNYXKJESK7X/avatar/generate");
        for verb in ["generate the image", "send your message", ""] {
            let s = UserError::classify(verb, &raw).sentence();
            assert!(
                !s.contains("://") && !s.contains("POST") && !s.contains("01M5N1"),
                "{s}"
            );
        }
        let e = UserError::classify("", &raw);
        assert_eq!(e.sentence(), "Arbos can't reach the server right now.");
    }

    #[test]
    fn words_alone_are_classified() {
        assert_eq!(
            UserError::from_text("x", "HTTP 503 Service Unavailable").class,
            ErrorClass::Server
        );
        assert_eq!(
            UserError::from_text("x", "The relay did not answer in time").class,
            ErrorClass::Timeout
        );
        assert_eq!(
            UserError::from_text("x", "token expired").class,
            ErrorClass::AuthExpired
        );
        assert_eq!(
            UserError::from_text("x", "image too large").class,
            ErrorClass::Upload(UploadFault::TooLarge)
        );
        assert_eq!(
            shorten_ids("/projects/01M5N1WSR91G8CJKNYXKJESK7X/x"),
            "/projects/01M5N1…ESK7X/x"
        );
    }
}
