//! Secret scrubbing shared by the box and the relay (D19). The box runs it at T3 write time
//! (`arbos-agentd::store::redact`, plus the live vault values only that process knows) and
//! before anything leaves in a feedback rollout; the relay runs it over every row it hands to a
//! debugging reader (`GET /projects/{id}/inspect`), so a secret that reached a relay table never
//! reaches a bot's context window.

use serde_json::Value;

/// Known key shapes, `Authorization: Bearer …` and `KEY=value` lines for well-known secret names.
pub fn redact_patterns(text: &str) -> String {
    let mut out = text.to_owned();
    let patterns: [(&str, &str); 9] = [
        (r"sk-or-v1-[A-Za-z0-9]{20,}", "[REDACTED:openrouter-key]"),
        (r"\bops_[A-Za-z0-9]{20,}", "[REDACTED:1password-token]"),
        (r"\bdp\.st\.[A-Za-z0-9_\-]{10,}", "[REDACTED:doppler-token]"),
        (r"sk-ant-[A-Za-z0-9_\-]{20,}", "[REDACTED:anthropic-key]"),
        (r"sk-[A-Za-z0-9]{32,}", "[REDACTED:openai-key]"),
        (r"dtn_[A-Za-z0-9]{20,}", "[REDACTED:daytona-key]"),
        (
            r"(?i)(authorization:\s*bearer\s+)[A-Za-z0-9._\-]+",
            "${1}[REDACTED:bearer]",
        ),
        (
            r"(?i)\b(gh[pousr]_[A-Za-z0-9]{20,}|xox[baprs]-[A-Za-z0-9\-]{10,}|AKIA[0-9A-Z]{16})\b",
            "[REDACTED:token]",
        ),
        // Arbos's own bearer tokens: `auth::mint` hex-encodes 32 random bytes behind a kind
        // prefix (adv_ device, abx_ box, apr_ project, amc_ MCP), signin adds arf_ for refresh
        // and the relay adm_ for admin. The full 64 hex digits are required so an ordinary
        // identifier that happens to start `adv_` is left alone.
        (
            r"\ba(?:dv|bx|pr|mc|rf|dm)_[0-9a-f]{64}\b",
            "[REDACTED:arbos-token]",
        ),
    ];
    for (pat, rep) in patterns {
        if let Ok(re) = regex_lite::Regex::new(pat) {
            out = re.replace_all(&out, rep).into_owned();
        }
    }
    if let Ok(re) = regex_lite::Regex::new(
        r"(?im)^(\s*(?:export\s+)?[A-Z0-9_]*(?:KEY|TOKEN|SECRET|PASSWORD)[A-Z0-9_]*\s*=\s*)\S+",
    ) {
        out = re.replace_all(&out, "${1}[REDACTED:env]").into_owned();
    }
    out
}

/// Secret placeholders and vault references the secrets system hands an agent
/// (`arbos-secret://…`, `op://…`, `$ARBOS_SECRET_*`): never useful to a reader, always scrubbed.
pub fn redact_placeholders(text: &str) -> String {
    let mut out = text.to_owned();
    let patterns: [(&str, &str); 3] = [
        (r"arbos-secret://[^\s\x22'`<>]+", "[secret placeholder]"),
        (r"\bop://[^\s\x22'`<>]+", "[vault reference]"),
        (r"\$\{?ARBOS_SECRET_[A-Z0-9_]+\}?", "[secret env]"),
    ];
    for (pat, rep) in patterns {
        if let Ok(re) = regex_lite::Regex::new(pat) {
            out = re.replace_all(&out, rep).into_owned();
        }
    }
    out
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

/// Known credential prefixes → the name the vault should suggest. Order matters: the first
/// pattern that matches a token wins.
const KEY_PREFIXES: &[(&str, &str)] = &[
    (r"^sk_(live|test)_[A-Za-z0-9]{16,}$", "STRIPE_KEY"),
    (r"^rk_(live|test)_[A-Za-z0-9]{16,}$", "STRIPE_KEY"),
    (r"^sk-or-v1-[A-Za-z0-9]{20,}$", "OPENROUTER_KEY"),
    (r"^sk-ant-[A-Za-z0-9_\-]{20,}$", "ANTHROPIC_KEY"),
    (r"^sk-proj-[A-Za-z0-9_\-]{20,}$", "OPENAI_KEY"),
    (r"^sk-[A-Za-z0-9]{32,}$", "OPENAI_KEY"),
    (r"^gh[pousr]_[A-Za-z0-9]{20,}$", "GITHUB_TOKEN"),
    (r"^github_pat_[A-Za-z0-9_]{20,}$", "GITHUB_TOKEN"),
    (r"^xox[baprs]-[A-Za-z0-9\-]{10,}$", "SLACK_TOKEN"),
    (r"^AKIA[0-9A-Z]{16}$", "AWS_ACCESS_KEY"),
    (r"^ops_[A-Za-z0-9]{20,}$", "ONEPASSWORD_TOKEN"),
    (r"^dp\.st\.[A-Za-z0-9_\-]{10,}$", "DOPPLER_TOKEN"),
    (r"^dtn_[A-Za-z0-9]{20,}$", "DAYTONA_KEY"),
    (r"^AIza[0-9A-Za-z_\-]{30,}$", "GOOGLE_API_KEY"),
    (
        r"^SG\.[A-Za-z0-9_\-]{16,}\.[A-Za-z0-9_\-]{16,}$",
        "SENDGRID_KEY",
    ),
    (r"^key-[a-f0-9]{32}$", "MAILGUN_API_KEY"),
    (
        r"^eyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}$",
        "JWT_TOKEN",
    ),
];

/// Characters a credential token may contain. A token is a maximal run of these.
fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '+' | '=')
}

/// Find tokens in `text` that look like credentials: a known prefix (`sk_live_…`, `ghp_…`,
/// `AKIA…`) or, failing that, one long opaque run (≥ 32 chars, letters **and** digits, no
/// spaces) that is not a URL or a path. Shared by the relay (the read-once "save this" path)
/// and the box (the offer card): both must agree on what a key looks like.
pub fn find_key_like(text: &str) -> Vec<KeyLike> {
    let compiled: Vec<(regex_lite::Regex, &'static str)> = KEY_PREFIXES
        .iter()
        .filter_map(|(p, n)| regex_lite::Regex::new(p).ok().map(|r| (r, *n)))
        .collect();
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
        if let Some((_, name)) = compiled.iter().find(|(r, _)| r.is_match(tok)) {
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

    /// `auth::TokenKind::prefix` says the prefixes exist so "a leaked string is identifiable in
    /// logs and greps". The scrubber has to know them, or the thing that makes an Arbos token
    /// recognisable is only useful to whoever finds it.
    #[test]
    fn arbos_own_bearer_tokens_go() {
        let hex = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        for prefix in ["adv_", "abx_", "apr_", "amc_", "arf_", "adm_"] {
            let text = format!("the agent printed {prefix}{hex} into the session");
            let out = scrub(&text);
            assert!(out.contains("[REDACTED:arbos-token]"), "{prefix}: {out}");
            assert!(!out.contains(hex), "{prefix}: the value survived: {out}");
        }
    }

    /// The prefixes are four ordinary lowercase letters and an underscore. They must not eat an
    /// identifier that merely starts the same way.
    #[test]
    fn arbos_token_shape_does_not_eat_prose() {
        for t in [
            "adv_mode is a column name",
            "set abx_count = 3",
            "apr_2026 is a partition",
            "adv_0123456789abcdef",
        ] {
            assert_eq!(scrub(t), t, "{t}");
        }
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

    #[test]
    fn values_are_walked() {
        let mut v = json!({ "a": ["ghp_abcdefghijklmnopqrstuvwxyz", { "b": "op://x/y" }], "n": 1 });
        scrub_value(&mut v);
        assert_eq!(v["a"][0], "[REDACTED:token]");
        assert_eq!(v["a"][1]["b"], "[vault reference]");
        assert_eq!(v["n"], 1);
    }
}
