//! Secret scrubbing shared by the box and the relay (D19). The box runs it at T3 write time
//! (`arbos-agentd::store::redact`, plus the live vault values only that process knows) and
//! before anything leaves in a feedback rollout; the relay runs it over every row it hands to a
//! debugging reader (`GET /projects/{id}/inspect`), so a secret that reached a relay table never
//! reaches a bot's context window.

use std::sync::LazyLock;

use regex_lite::Regex;
use serde_json::Value;

/// Known key shapes, `Authorization: Bearer …` and `KEY=value` lines for well-known secret names.
///
/// The shapes come from [`KEY_PREFIXES`], with word boundaries or a nonword tail: keys ending
/// in `-` must also disappear. A consumed nonword delimiter is kept in the output.
pub fn redact_patterns(text: &str) -> String {
    let mut out = text.to_owned();
    for (re, replacement) in REDACTION_PATTERNS.iter() {
        out = re.replace_all(&out, replacement.as_str()).into_owned();
    }
    out
}

fn key_redaction(body: &str, marker: &str) -> Option<(regex_lite::Regex, String)> {
    // Keep the existing word-boundary matches. At a nonword-ending key, consume and
    // restore one delimiter (regex-lite has no lookaround), or accept end of input.
    regex_lite::Regex::new(&format!(r"\b{body}(?:\b|(?P<redact_tail>\W)|$)"))
        .ok()
        .map(|re| (re, format!("{marker}${{redact_tail}}")))
}

/// Secret placeholders and vault references the secrets system hands an agent
/// (`arbos-secret://…`, `op://…`, `$ARBOS_SECRET_*`): never useful to a reader, always scrubbed.
pub fn redact_placeholders(text: &str) -> String {
    let mut out = text.to_owned();
    for (re, replacement) in PLACEHOLDER_PATTERNS.iter() {
        out = re.replace_all(&out, *replacement).into_owned();
    }
    out
}

/// Flags whose next argument is `NAME=value` with a secret on the value side.
const ENV_FLAGS: &[&str] = &["-e", "--env", "--build-arg"];

/// Flags whose next argument is a bare secret.
const VALUE_FLAGS: &[&str] = &["--password", "--token", "--secret"];

/// `NAME=value` with the value removed, keeping the name. A bare `NAME` (the pass-through form
/// of `-e`, whose value comes from the parent's environment) carries nothing and is kept whole.
fn redact_assignment(arg: &str) -> String {
    match arg.split_once('=') {
        Some((name, _)) => format!("{name}=[REDACTED:env]"),
        None => arg.to_owned(),
    }
}

/// Render a child process's argument list for a log line with every secret value removed.
///
/// A `docker run` that fails is worth logging with the command that failed, but that command
/// carries the box's whole environment as `-e NAME=value` pairs: printed verbatim it writes live
/// API keys into the journal. Flag names, variable names, the image and every other argument
/// survive — only the values go — so the line still says what was run and with which variables.
///
/// [`scrub`] runs over the arguments this pass does not know about, so a credential that arrives
/// in some other position (a URL query, a positional token) is removed as well.
pub fn redact_argv<S: AsRef<str>>(args: &[S]) -> String {
    /// Longer than this and an argument is a script or a prompt, not a flag worth reading whole.
    const MAX_ARG: usize = 160;

    let mut out: Vec<String> = Vec::with_capacity(args.len());
    let mut pending: Option<bool> = None;
    for arg in args {
        let arg = arg.as_ref();
        let rendered = match pending.take() {
            Some(true) => redact_assignment(arg),
            Some(false) => "[REDACTED:arg]".to_owned(),
            None if ENV_FLAGS.contains(&arg) => {
                pending = Some(true);
                arg.to_owned()
            }
            None if VALUE_FLAGS.contains(&arg) => {
                pending = Some(false);
                arg.to_owned()
            }
            // The `--flag=NAME=value` spelling of the same thing.
            None => match ENV_FLAGS
                .iter()
                .find_map(|f| arg.strip_prefix(&format!("{f}=")).map(|rest| (f, rest)))
            {
                Some((flag, rest)) => format!("{flag}={}", redact_assignment(rest)),
                None => scrub(arg),
            },
        };
        out.push(clip(&rendered, MAX_ARG));
    }
    out.join(" ")
}

/// `text` cut to `max` characters, with an ellipsis when it was cut. Counts characters rather
/// than bytes so a cut never lands inside one.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let head: String = text.chars().take(max).collect();
    format!("{head}…")
}

/// A token in a message that looks like a credential: where it sits and what it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyLike {
    /// Byte range inside the text.
    pub start: usize,
    pub end: usize,
    /// Suggested vault name (`STRIPE_KEY`, `GITHUB_TOKEN`, `API_KEY`), derived from the prefix.
    pub suggested_name: &'static str,
    /// A known prefix matched (`sk_live_…`). A long opaque run without one is a guess: the
    /// box offers a card only for confident finds, the explicit "save this" path takes both.
    pub confident: bool,
}

impl KeyLike {
    pub fn value<'a>(&self, text: &'a str) -> &'a str {
        &text[self.start..self.end]
    }
}

/// Known credential shapes: the pattern body, the name the vault should suggest, and the marker
/// that replaces the value when it is removed. Order matters: the first entry that matches a
/// token wins, in both directions.
///
/// One table, two anchorings. [`find_key_like`] wraps each body in `^…$` because it classifies a
/// whole token; [`redact_patterns`] adds boundaries because it finds values inside running
/// text. Both are derived from these bodies so the two cannot disagree about what a key is — a
/// hand-maintained second list had already drifted to covering nine of these seventeen shapes,
/// which left a pasted Stripe key readable in storage.
///
/// Every body carries a charset and a length floor, and a test holds them to it. Unanchored, a
/// bare prefix would redact ordinary prose and code — `key-value`, `key-binding` — and a pass
/// that mangles the logs we debug from is one that gets switched off.
const KEY_PREFIXES: &[(&str, &str, &str)] = &[
    (
        r"sk_(live|test)_[A-Za-z0-9]{16,}",
        "STRIPE_KEY",
        "[REDACTED:stripe-key]",
    ),
    (
        r"rk_(live|test)_[A-Za-z0-9]{16,}",
        "STRIPE_KEY",
        "[REDACTED:stripe-key]",
    ),
    (
        r"sk-or-v1-[A-Za-z0-9]{20,}",
        "OPENROUTER_KEY",
        "[REDACTED:openrouter-key]",
    ),
    (
        r"sk-ant-[A-Za-z0-9_\-]{20,}",
        "ANTHROPIC_KEY",
        "[REDACTED:anthropic-key]",
    ),
    (
        r"sk-proj-[A-Za-z0-9_\-]{20,}",
        "OPENAI_KEY",
        "[REDACTED:openai-key]",
    ),
    (
        r"sk-[A-Za-z0-9]{32,}",
        "OPENAI_KEY",
        "[REDACTED:openai-key]",
    ),
    (
        r"gh[pousr]_[A-Za-z0-9]{20,}",
        "GITHUB_TOKEN",
        "[REDACTED:token]",
    ),
    (
        r"github_pat_[A-Za-z0-9_]{20,}",
        "GITHUB_TOKEN",
        "[REDACTED:token]",
    ),
    (
        r"xox[baprs]-[A-Za-z0-9\-]{10,}",
        "SLACK_TOKEN",
        "[REDACTED:token]",
    ),
    (r"AKIA[0-9A-Z]{16}", "AWS_ACCESS_KEY", "[REDACTED:token]"),
    (
        r"ops_[A-Za-z0-9]{20,}",
        "ONEPASSWORD_TOKEN",
        "[REDACTED:1password-token]",
    ),
    (
        r"dp\.st\.[A-Za-z0-9_\-]{10,}",
        "DOPPLER_TOKEN",
        "[REDACTED:doppler-token]",
    ),
    (
        r"dtn_[A-Za-z0-9]{20,}",
        "DAYTONA_KEY",
        "[REDACTED:daytona-key]",
    ),
    (
        r"AIza[0-9A-Za-z_\-]{30,}",
        "GOOGLE_API_KEY",
        "[REDACTED:google-key]",
    ),
    (
        r"SG\.[A-Za-z0-9_\-]{16,}\.[A-Za-z0-9_\-]{16,}",
        "SENDGRID_KEY",
        "[REDACTED:sendgrid-key]",
    ),
    (
        r"key-[a-f0-9]{32}",
        "MAILGUN_API_KEY",
        "[REDACTED:mailgun-key]",
    ),
    (
        r"eyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}",
        "JWT_TOKEN",
        "[REDACTED:jwt]",
    ),
];

static REDACTION_PATTERNS: LazyLock<Vec<(Regex, String)>> = LazyLock::new(|| {
    let keys = KEY_PREFIXES
        .iter()
        .filter_map(|(body, _, marker)| key_redaction(body, marker));
    // These match headers and assignments rather than a token shape in KEY_PREFIXES.
    let other = [
        (
            r"(?i)(authorization:\s*bearer\s+)[A-Za-z0-9._\-]+",
            "${1}[REDACTED:bearer]",
        ),
        (
            r"(?im)^(\s*(?:export\s+)?[A-Z0-9_]*(?:KEY|TOKEN|SECRET|PASSWORD)[A-Z0-9_]*\s*=\s*)\S+",
            "${1}[REDACTED:env]",
        ),
    ]
    .into_iter()
    .filter_map(|(pattern, replacement)| {
        Regex::new(pattern)
            .ok()
            .map(|re| (re, replacement.to_owned()))
    });
    keys.chain(other).collect()
});

static PLACEHOLDER_PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        (r"arbos-secret://[^\s\x22'`<>]+", "[secret placeholder]"),
        (r"\bop://[^\s\x22'`<>]+", "[vault reference]"),
        (r"\$\{?ARBOS_SECRET_[A-Z0-9_]+\}?", "[secret env]"),
    ]
    .into_iter()
    .filter_map(|(pattern, replacement)| Regex::new(pattern).ok().map(|re| (re, replacement)))
    .collect()
});

static KEY_CLASSIFIERS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    KEY_PREFIXES
        .iter()
        .filter_map(|(body, name, _)| Regex::new(&format!("^{body}$")).ok().map(|re| (re, *name)))
        .collect()
});

/// Characters a credential token may contain. A token is a maximal run of these.
fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '+' | '=')
}

/// Find tokens in `text` that look like credentials: a known prefix (`sk_live_…`, `ghp_…`,
/// `AKIA…`) or, failing that, one long opaque run (≥ 32 chars, letters **and** digits, no
/// spaces) that is not a URL or a path. Shared by the relay (the read-once "save this" path)
/// and the box (the offer card): both must agree on what a key looks like.
pub fn find_key_like(text: &str) -> Vec<KeyLike> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let punct = |c: char| matches!(c, '.' | ',' | ':' | ';' | '=');
    let push = |s: usize, e: usize, out: &mut Vec<KeyLike>| {
        let raw = &text[s..e];
        let lead = raw.len() - raw.trim_start_matches(punct).len();
        let tok = raw.trim_matches(punct);
        if tok.is_empty() {
            return;
        }
        let s = s + lead;
        let e = s + tok.len();
        if let Some((_, name)) = KEY_CLASSIFIERS.iter().find(|(r, _)| r.is_match(tok)) {
            out.push(KeyLike {
                start: s,
                end: e,
                suggested_name: name,
                confident: true,
            });
            return;
        }
        let in_url = text[..s]
            .chars()
            .next_back()
            .is_some_and(|c| matches!(c, '?' | '&' | '/' | '#'));
        let long_opaque = tok.len() >= 32
            && !in_url
            && !tok.contains('=')
            && tok.chars().any(|c| c.is_ascii_digit())
            && tok.chars().any(|c| c.is_ascii_alphabetic())
            && !tok.contains("://")
            && !tok.starts_with('/')
            && tok.matches('/').count() <= 1
            && tok.matches('.').count() <= 2;
        if long_opaque {
            out.push(KeyLike {
                start: s,
                end: e,
                suggested_name: "API_KEY",
                confident: false,
            });
        }
    };
    for (i, c) in text.char_indices() {
        match (is_token_char(c), start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                push(s, i, &mut out);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        push(s, text.len(), &mut out);
    }
    out
}

/// The words a user says to hand a pasted key over: "save this", "save that", "save it",
/// "store this", "put this in the vault", "add this to the vault".
pub fn asks_to_save(text: &str) -> bool {
    let t = text.to_lowercase();
    [
        "save this",
        "save that",
        "save it",
        "store this",
        "store that",
        "store it",
        "in the vault",
        "to the vault",
        "to my vault",
        "in my vault",
        "save as ",
    ]
    .iter()
    .any(|w| t.contains(w))
}

/// The display hint a tool may carry instead of the value: the first and last four
/// characters, `sk_l…xA1`. Enough to pick one token in a message, useless to an attacker.
pub fn key_hint(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 10 {
        return "…".to_owned();
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}

/// Does `value` match a hint made by [`key_hint`]?
pub fn matches_hint(value: &str, hint: &str) -> bool {
    let Some((head, tail)) = hint.split_once('…') else {
        return false;
    };
    value.chars().count() > 10 && value.starts_with(head) && value.ends_with(tail)
}

/// Both passes: what a debugging reader may see.
pub fn scrub(text: &str) -> String {
    redact_placeholders(&redact_patterns(text))
}

/// `scrub` over every string inside a JSON value (keys are left alone).
pub fn scrub_value(v: &mut Value) {
    match v {
        Value::String(s) => {
            let clean = scrub(s);
            if clean != *s {
                *s = clean;
            }
        }
        Value::Array(items) => items.iter_mut().for_each(scrub_value),
        Value::Object(map) => map.values_mut().for_each(scrub_value),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// One sample per shape in [`KEY_PREFIXES`], keyed by the pattern body. A shape added to the
    /// table without a sample fails `every_classified_shape_is_redacted` rather than quietly
    /// going unredacted, which is how the two lists drifted apart in the first place.
    ///
    /// A sample convincing enough to test with is convincing enough for a secret scanner to
    /// block the push, so the ones that trip it are split with `concat!`.
    const SAMPLES: &[(&str, &str)] = &[
        (
            r"sk_(live|test)_[A-Za-z0-9]{16,}",
            concat!("sk_", "live_51HxQ9mT3vK2pLnR8wZ4eYc7bD1fG6hJ0kM9nP5qS2tU8vW3xA1"),
        ),
        (
            r"rk_(live|test)_[A-Za-z0-9]{16,}",
            concat!("rk_", "live_51HxQ9mT3vK2pLnR8wZ4e"),
        ),
        (
            r"sk-or-v1-[A-Za-z0-9]{20,}",
            "sk-or-v1-abcdefghijklmnopqrstuvwxyz0123456789",
        ),
        (
            r"sk-ant-[A-Za-z0-9_\-]{20,}",
            "sk-ant-api03-abcdefghijklmnopqrstuvwxyz",
        ),
        (
            r"sk-proj-[A-Za-z0-9_\-]{20,}",
            "sk-proj-abcdefghijklmnopqrstuvwxyz",
        ),
        (
            r"sk-[A-Za-z0-9]{32,}",
            "sk-abcdefghijklmnopqrstuvwxyz0123456789",
        ),
        (
            r"gh[pousr]_[A-Za-z0-9]{20,}",
            "ghp_abcdefghijklmnopqrstuvwxyz012345",
        ),
        (
            r"github_pat_[A-Za-z0-9_]{20,}",
            "github_pat_11ABCDEFG0abcdefghijklmnopqrstuvwxyz",
        ),
        (
            r"xox[baprs]-[A-Za-z0-9\-]{10,}",
            "xoxb-123456789012-abcdefghijkl",
        ),
        (r"AKIA[0-9A-Z]{16}", "AKIAIOSFODNN7EXAMPLE"),
        (r"ops_[A-Za-z0-9]{20,}", "ops_abcdefghijklmnopqrstuvwxyz"),
        (
            r"dp\.st\.[A-Za-z0-9_\-]{10,}",
            "dp.st.dev_abcdefghijklmnop",
        ),
        (r"dtn_[A-Za-z0-9]{20,}", "dtn_abcdefghijklmnopqrstuvwxyz"),
        (
            r"AIza[0-9A-Za-z_\-]{30,}",
            "AIzaSyA1bC2dE3fG4hI5jK6lM7nO8pQ9rS0tU1v",
        ),
        (
            r"SG\.[A-Za-z0-9_\-]{16,}\.[A-Za-z0-9_\-]{16,}",
            "SG.abcdefghijklmnop.qrstuvwxyz0123456789",
        ),
        (
            r"key-[a-f0-9]{32}",
            concat!("key-", "0123456789abcdef0123456789abcdef"),
        ),
        (
            r"eyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}",
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVPmB92K27uhbUJU1p1r",
        ),
    ];

    /// Naming a secret and removing one must stay the same judgement. Every shape the vault can
    /// name has to disappear from running text, whole — not clipped to a prefix.
    #[test]
    fn every_classified_shape_is_redacted() {
        for (body, name, marker) in KEY_PREFIXES {
            let sample = SAMPLES
                .iter()
                .find(|(b, _)| b == body)
                .map(|(_, s)| *s)
                .unwrap_or_else(|| panic!("{name} ({body}) has no sample: add one to SAMPLES"));
            let found = find_key_like(sample);
            assert_eq!(found.len(), 1, "{name}: sample is not one token: {found:?}");
            assert_eq!(
                found[0].suggested_name, *name,
                "{name}: sample misclassifies"
            );

            let out = redact_patterns(&format!("here is the key {sample} save this"));
            assert!(out.contains(marker), "{name} is not marked {marker}: {out}");
            assert!(!out.contains(sample), "{name} survives whole: {out}");
            let head = &sample[..sample.len().min(12)];
            assert!(!out.contains(head), "{name} leaves a prefix behind: {out}");
        }
    }

    /// The other half, and the one that makes the first safe. Unanchored shapes without a length
    /// floor would eat `key-value` and half the prose around it; a pass that corrupts the logs we
    /// debug from is a pass someone turns off.
    #[test]
    fn ordinary_text_and_code_are_left_alone() {
        for t in [
            "let key_value = map.get(&key).unwrap_or_default();",
            "we use a key-value store for the cache",
            "the key-binding is ctrl-k and the primary-key-lookup goes by id",
            "vendors differ: some keys start with sk- and some with sk_",
            "2026-11-07T04:12:33Z INFO relay GET /projects/abc123/inspect 200 in 14ms",
            "eyJ is what a base64-encoded JSON object starts with",
            "SG. is SendGrid, AKIA is AWS, dp.st. is Doppler",
            "see crates/arbos-core/src/redact.rs for the shape table",
        ] {
            assert_eq!(redact_patterns(t), t, "over-redacted");
        }
    }

    fn nonword_ending_keys() -> Vec<(String, &'static str)> {
        vec![
            (
                format!("sk-ant-{}-", "a".repeat(19)),
                "[REDACTED:anthropic-key]",
            ),
            (
                format!("sk-proj-{}-", "_".repeat(19)),
                "[REDACTED:openai-key]",
            ),
            (format!("xoxb-{}", "-".repeat(10)), "[REDACTED:token]"),
            (
                format!("dp.st.{}-", "a".repeat(9)),
                "[REDACTED:doppler-token]",
            ),
            (format!("AIza{}-", "a".repeat(29)), "[REDACTED:google-key]"),
            (
                format!("SG.{}.{}-", "a".repeat(16), "_".repeat(15)),
                "[REDACTED:sendgrid-key]",
            ),
            (
                format!(
                    "eyJ{}.{}.{}-",
                    "a".repeat(10),
                    "_".repeat(10),
                    "-".repeat(9)
                ),
                "[REDACTED:jwt]",
            ),
        ]
    }

    #[test]
    fn nonword_ending_confident_keys_are_redacted_and_keep_the_tail() {
        for (sample, marker) in nonword_ending_keys() {
            for tail in ["", " ", "\n", "'", "\"", ")", "—", "🙂", ":."] {
                let text = format!("前 ({sample}{tail}");
                let found = find_key_like(&text);
                assert_eq!(found.len(), 1, "one known key before {tail:?}");
                assert!(found[0].confident);
                assert_eq!(found[0].value(&text), sample);
                assert_eq!(
                    redact_patterns(&text),
                    format!("前 ({marker}{tail}"),
                    "a confident key must disappear whole and keep {tail:?}"
                );
            }
        }
    }

    #[test]
    fn adjacent_nonword_ending_keys_are_each_redacted() {
        let samples = nonword_ending_keys();
        for (sample, marker) in &samples {
            for (next, next_marker) in &samples {
                for separator in [" ", "\n", "'", "\"", "—", "🙂", "::"] {
                    assert_eq!(
                        redact_patterns(&format!("{sample}{separator}{next}")),
                        format!("{marker}{separator}{next_marker}"),
                        "both keys and their separator must survive replacement correctly"
                    );
                }
            }
        }
    }

    #[test]
    fn legacy_word_boundaries_still_leave_embedded_or_overlong_words_alone() {
        for text in [
            "AKIAIOSFODNN7EXAMPLEZ",
            "xAKIAIOSFODNN7EXAMPLE",
            "xghp_abcdefghijklmnopqrst",
            "prefix_sk-ant-aaaaaaaaaaaaaaaaaaa-",
        ] {
            assert_eq!(redact_patterns(text), text);
        }
    }

    /// The floor the half above depends on, held as a property of the table rather than of review.
    #[test]
    fn every_shape_bounds_its_length() {
        for (body, name, _) in KEY_PREFIXES {
            assert!(
                body.contains('{'),
                "{name} ({body}) has no length bound: unanchored it would match prose"
            );
        }
    }

    #[test]
    fn keys_and_placeholders_go() {
        let text = "key sk-or-v1-abcdefghijklmnopqrstuvwxyz0123456789 and op://Arbos/item/field and \
                    arbos-secret://abc123 and $ARBOS_SECRET_GITHUB and Authorization: Bearer abc.def-ghi";
        let out = scrub(text);
        assert!(out.contains("[REDACTED:openrouter-key]"), "{out}");
        assert!(out.contains("[vault reference]"), "{out}");
        assert!(out.contains("[secret placeholder]"), "{out}");
        assert!(out.contains("[secret env]"), "{out}");
        assert!(out.contains("Bearer [REDACTED:bearer]"), "{out}");
        assert!(!out.contains("abc.def-ghi"), "{out}");
    }

    #[test]
    fn env_lines_keep_the_name() {
        let out = scrub("OPENAI_API_KEY=sk-live-123\nNAME=fine");
        assert_eq!(out, "OPENAI_API_KEY=[REDACTED:env]\nNAME=fine");
    }

    #[test]
    fn key_like_tokens_are_found_with_a_suggested_name() {
        let text = concat!(
            "here is the stripe key sk_",
            "live_51HxQ9mT3vK2pLnR8wZ4eYc7bD1fG6hJ0kM9nP5qS2tU8vW3xA1 save this"
        );
        let found = find_key_like(text);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].suggested_name, "STRIPE_KEY");
        assert!(found[0]
            .value(text)
            .starts_with(concat!("sk_", "live_51Hx")));
        assert!(found[0].value(text).ends_with("W3xA1"));
        assert!(asks_to_save(text));

        let gh = find_key_like("token: ghp_abcdefghijklmnopqrstuvwxyz012345.");
        assert_eq!(gh.len(), 1);
        assert_eq!(gh[0].suggested_name, "GITHUB_TOKEN");
        assert_eq!(
            gh[0].value("token: ghp_abcdefghijklmnopqrstuvwxyz012345."),
            "ghp_abcdefghijklmnopqrstuvwxyz012345"
        );

        let opaque = find_key_like("use a8f3k2j9d0s7f6g5h4j3k2l1z0x9c8v7b6n5m4 for it");
        assert_eq!(opaque.len(), 1);
        assert_eq!(opaque[0].suggested_name, "API_KEY");
        assert!(!opaque[0].confident && found[0].confident);
    }

    #[test]
    fn prose_urls_and_paths_are_not_keys() {
        for t in [
            "Let's refine the agent workspace.",
            "see https://api.stripe.com/v1/refunds?limit=100&starting_after=re_1234567890abcdef",
            "the file is /Users/jacob/Projects/arbos/crates/arbos-relay/src/secrets.rs",
            "ULID 01M5MWS2TNABCDEFGHJKMNPQRS",
        ] {
            assert!(find_key_like(t).is_empty(), "{t}: {:?}", find_key_like(t));
        }
        assert!(!asks_to_save("can you save me a seat"));
        // a git sha is a guess, never a confident find (no offer card for it)
        let sha = find_key_like("deadbeef1234deadbeef1234deadbeef1234abcd");
        assert!(sha.iter().all(|k| !k.confident), "{sha:?}");
    }

    #[test]
    fn hints_pick_one_token_without_the_value() {
        let v = concat!(
            "sk_",
            "live_51HxQ9mT3vK2pLnR8wZ4eYc7bD1fG6hJ0kM9nP5qS2tU8vW3xA1"
        );
        let h = key_hint(v);
        assert_eq!(h, "sk_l…W3xA1".replace("W3xA1", "3xA1"));
        assert!(matches_hint(v, &h));
        assert!(!matches_hint(concat!("sk_", "live_other"), &h));
        assert_eq!(key_hint("short"), "…");
    }

    /// The journald leak of 2026-11-09: a failed `docker run` for an `arbos-agent-*` box was
    /// logged with the command that failed, and the command carries the box's environment as
    /// `-e NAME=value`, so an OpenRouter key reached the system journal in plaintext. Every
    /// value goes; the names, the flags and the image stay, so the line is still debuggable.
    #[test]
    fn docker_run_argv_logs_the_names_and_never_the_values() {
        let key = "sk-or-v1-abcdefghijklmnopqrstuvwxyz0123456789";
        let argv = vec![
            "run".to_owned(),
            "-d".to_owned(),
            "--name".to_owned(),
            "arbos-box-u_default".to_owned(),
            "-p".to_owned(),
            "127.0.0.1::7788".to_owned(),
            "-e".to_owned(),
            format!("OPENROUTER_API_KEY={key}"),
            "-e".to_owned(),
            "ARBOS_BOX_TOKEN=abx_0123456789abcdef0123456789abcdef".to_owned(),
            "-e".to_owned(),
            "PI_TELEMETRY=0".to_owned(),
            "-e".to_owned(),
            "ARBOS_USER_ID".to_owned(),
            "--label".to_owned(),
            "arbos.fleet=gvisor".to_owned(),
            "arbos-agent-0.1.203".to_owned(),
        ];
        let line = redact_argv(&argv);

        assert!(!line.contains(key), "the openrouter key survives: {line}");
        assert!(
            !line.contains("abx_0123456789abcdef0123456789abcdef"),
            "the box token survives: {line}"
        );
        assert!(
            line.contains("OPENROUTER_API_KEY=[REDACTED:env]"),
            "the variable name is gone too, so the line cannot be debugged: {line}"
        );
        assert!(line.contains("ARBOS_BOX_TOKEN=[REDACTED:env]"), "{line}");
        // Structure a reader needs: the subcommand, the container, the port, the image, and a
        // non-secret value such as a label or a pass-through `-e NAME` with no value of its own.
        assert!(
            line.starts_with("run -d --name arbos-box-u_default"),
            "{line}"
        );
        assert!(line.contains("-p 127.0.0.1::7788"), "{line}");
        assert!(line.contains("arbos-agent-0.1.203"), "{line}");
        assert!(line.contains("--label arbos.fleet=gvisor"), "{line}");
        assert!(line.contains("-e ARBOS_USER_ID "), "{line}");
        assert!(line.contains("-e PI_TELEMETRY=[REDACTED:env]"), "{line}");
    }

    /// The other half of the same log line: docker's own stderr, and any argument the `-e` rule
    /// does not cover, go through [`scrub`], so a credential in a flag value or an error message
    /// does not reach the journal either.
    #[test]
    fn argv_values_outside_the_env_flags_are_scrubbed() {
        let key = "sk-or-v1-abcdefghijklmnopqrstuvwxyz0123456789";
        let line = redact_argv(&[
            "login".to_owned(),
            "--password".to_owned(),
            key.to_owned(),
            "--build-arg".to_owned(),
            format!("OPENROUTER_API_KEY={key}"),
            format!("--env=ANTHROPIC_API_KEY={key}"),
            format!("https://openrouter.ai/api/v1?key={key}"),
        ]);
        assert!(!line.contains(key), "{line}");
        assert!(line.contains("--password [REDACTED:arg]"), "{line}");
        assert!(
            line.contains("--build-arg OPENROUTER_API_KEY=[REDACTED:env]"),
            "{line}"
        );
        assert!(
            line.contains("--env=ANTHROPIC_API_KEY=[REDACTED:env]"),
            "{line}"
        );
        assert!(line.contains("[REDACTED:openrouter-key]"), "{line}");

        let stderr = scrub(&format!(
            "docker: Error response from daemon: invalid env OPENROUTER_API_KEY={key}"
        ));
        assert!(!stderr.contains(key), "{stderr}");
        assert!(stderr.contains("Error response from daemon"), "{stderr}");
    }

    /// A helper script passed as one argument is pages long; a log line must not carry all of it.
    #[test]
    fn long_arguments_are_cut_rather_than_logged_whole() {
        let script = "echo hi; ".repeat(100);
        let line = redact_argv(&["run".to_owned(), "-c".to_owned(), script]);
        assert!(line.chars().count() < 200, "{}", line.chars().count());
        assert!(line.ends_with('…'), "{line}");
    }

    #[test]
    fn values_are_walked() {
        let mut v = json!({ "a": ["ghp_abcdefghijklmnopqrstuvwxyz", { "b": "op://x/y" }], "n": 1 });
        scrub_value(&mut v);
        assert_eq!(v["a"][0], "[REDACTED:token]");
        assert_eq!(v["a"][1]["b"], "[vault reference]");
        assert_eq!(v["n"], 1);
    }

    #[test]
    fn concurrent_readers_preserve_token_ranges_and_nested_values() {
        let sample = format!("ghp_{}", "ab12".repeat(6));
        let text = format!("λ ({sample}). op://Synthetic/item");
        let start = text.find(&sample).unwrap();
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    for _ in 0..32 {
                        let found = find_key_like(&text);
                        assert_eq!(found.len(), 1);
                        assert_eq!(found[0].start, start);
                        assert_eq!(found[0].end, start + sample.len());
                        assert_eq!(found[0].value(&text), sample);
                        assert_eq!(found[0].suggested_name, "GITHUB_TOKEN");
                        assert!(found[0].confident);
                        let clean = "λ ([REDACTED:token]). [vault reference]";
                        assert_eq!(scrub(&text), clean);

                        let mut nested = json!({ "op://key/is/unchanged": [text, { "v": text }], "n": 7, "flag": true });
                        scrub_value(&mut nested);
                        assert_eq!(nested, json!({ "op://key/is/unchanged": [clean, { "v": clean }], "n": 7, "flag": true }));
                    }
                });
            }
        });
    }
}
