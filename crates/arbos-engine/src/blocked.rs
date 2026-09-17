//! Provider families this key cannot call, learned from the provider's
//! own refusals and remembered across turns and kernels.
//!
//! Jacob's first turn on a brand-new project (Code2, 2026-09-16) opened
//! with a provider's policy text and two model ids: his key is blocked
//! for `openai/*`, the configured model was one, and the fallback was
//! another. A first turn is the worst moment to learn that. Now a 403 on
//! a model marks its family blocked for that host (`~/.config/arbos/
//! runtime/blocked-models.json`); the next turn picks a model the key can
//! use, the fallbacks skip the family, and a success from the family
//! clears the mark.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Blocked {
    /// `<api host>` → family → what the provider said, when.
    #[serde(default)]
    pub hosts: BTreeMap<String, BTreeMap<String, Mark>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mark {
    pub since_ms: i64,
    pub message: String,
    pub model: String,
}

fn path() -> PathBuf {
    arbos_core::host::dirs_config()
        .join("runtime")
        .join("blocked-models.json")
}

/// The host part of an API base (`openrouter.ai` of
/// `https://openrouter.ai/api/v1`), so one key's marks stay with its
/// provider.
pub fn host_of(base: &str) -> String {
    let b = base.trim();
    let b = b.split("://").nth(1).unwrap_or(b);
    b.split('/').next().unwrap_or(b).to_ascii_lowercase()
}

/// The marks as they stand: empty when absent or unreadable (the block
/// is then simply not known this run — the fallback path handles a 403
/// live).
pub fn load() -> Blocked {
    load_for_rewrite().unwrap_or_default()
}

/// For `mark`/`clear`, which write the file back: an unreadable file is
/// not rewritten from empty (the qal-j08 family — `arbos_core::record`).
fn load_for_rewrite() -> Option<Blocked> {
    match arbos_core::record::read_json::<Blocked>(&path()) {
        arbos_core::record::Read::Present(b) => Some(b),
        arbos_core::record::Read::Absent => Some(Blocked::default()),
        arbos_core::record::Read::Unknown(why) => {
            eprintln!("blocked models: {why}; not rewritten");
            None
        }
    }
}

fn save(b: &Blocked) {
    let p = path();
    if let Ok(text) = serde_json::to_string_pretty(b)
        && let Err(e) = arbos_core::record::write_atomic(&p, text.as_bytes())
    {
        eprintln!("blocked models: {e:#}");
    }
}

/// Whether `model`'s family is marked blocked for `base`.
pub fn is_blocked(base: &str, model: &str) -> bool {
    let fam = crate::retry::family(model);
    if fam.is_empty() {
        return false;
    }
    load()
        .hosts
        .get(&host_of(base))
        .is_some_and(|m| m.contains_key(fam))
}

/// The provider refused `model` for this key (403): its family is marked.
pub fn mark(base: &str, model: &str, message: &str) {
    let fam = crate::retry::family(model);
    if fam.is_empty() {
        return;
    }
    let Some(mut b) = load_for_rewrite() else {
        return;
    };
    b.hosts.entry(host_of(base)).or_default().insert(
        fam.to_string(),
        Mark {
            since_ms: arbos_core::now_ms(),
            message: arbos_core::text::clip(message, 300),
            model: model.to_string(),
        },
    );
    save(&b);
}

/// `model` answered: its family is not blocked after all (the block was
/// lifted, or it was one model's). Cheap when nothing is marked.
pub fn clear(base: &str, model: &str) {
    let fam = crate::retry::family(model);
    if fam.is_empty() {
        return;
    }
    let Some(mut b) = load_for_rewrite() else {
        return;
    };
    let host = host_of(base);
    let Some(m) = b.hosts.get_mut(&host) else {
        return;
    };
    if m.remove(fam).is_some() {
        if m.is_empty() {
            b.hosts.remove(&host);
        }
        save(&b);
    }
}

/// A model the key can use in place of `blocked`: the first of
/// `candidates` whose family is not marked (and is not `blocked`'s).
pub fn alternative<'a>(base: &str, blocked: &str, candidates: &[&'a str]) -> Option<&'a str> {
    let marks = load();
    let host = host_of(base);
    let bad = crate::retry::family(blocked);
    candidates.iter().copied().find(|c| {
        let fam = crate::retry::family(c);
        !fam.is_empty()
            && fam != bad
            && !marks.hosts.get(&host).is_some_and(|m| m.contains_key(fam))
    })
}

/// The families marked for `base`, for a plain sentence.
pub fn families(base: &str) -> Vec<String> {
    load()
        .hosts
        .get(&host_of(base))
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_marks_the_family_and_a_success_clears_it() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: test-local; the module reads XDG_CONFIG_HOME at each call.
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let base = "https://openrouter.ai/api/v1";
        assert!(!is_blocked(base, "openai/gpt-4.1-mini"));
        mark(
            base,
            "openai/gpt-4.1-mini",
            "Policy Violation: this user has been blocked",
        );
        assert!(is_blocked(base, "openai/gpt-4.1-mini"));
        assert!(
            is_blocked(base, "openai/gpt-5.6-terra"),
            "the family, not the model"
        );
        assert!(!is_blocked(base, "anthropic/claude-opus-5"));
        assert!(
            !is_blocked("https://api.openai.com/v1", "openai/gpt-4.1-mini"),
            "another host"
        );
        assert_eq!(
            alternative(
                base,
                "openai/gpt-4.1-mini",
                &[
                    "openai/gpt-5.6-terra",
                    "anthropic/claude-opus-5",
                    "google/gemini-3.8-flash"
                ]
            ),
            Some("anthropic/claude-opus-5")
        );
        assert_eq!(families(base), vec!["openai".to_string()]);
        clear(base, "openai/gpt-5.6-terra");
        assert!(!is_blocked(base, "openai/gpt-4.1-mini"));
        assert!(families(base).is_empty());
        // A hand-written id with no vendor prefix is its own family: a
        // block on it is remembered too (qal-040's scenarios).
        let direct = "https://api.openai.com/v1";
        mark(direct, "gpt-4.1-mini", "blocked");
        assert!(is_blocked(direct, "gpt-4.1-mini"));
        assert!(
            !is_blocked(direct, "gpt-5.6-terra"),
            "another unprefixed id is another family"
        );
        assert_eq!(
            alternative(direct, "gpt-4.1-mini", &["gpt-4.1-mini", "gpt-5.6-terra"]),
            Some("gpt-5.6-terra")
        );
        unsafe { std::env::remove_var("XDG_CONFIG_HOME") };
    }
}
