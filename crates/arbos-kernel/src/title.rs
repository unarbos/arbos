//! A chat's title from the model after its first turn (F-156). A chat
//! nobody named shows its first prompt's opening words until this lands
//! — Cursor does the same — and a `name` the person or the parent gave is
//! never touched. One call per chat, ever, marked on disk before it is
//! made; none when no key is in reach (the fallback stands).

use std::sync::Arc;

use arbos_core::{Agent, EventKind, Place, chattitle, load_transcript};

use crate::hooks::KernelHooks;
use crate::klog;

/// The file that says the one call was made (or tried). In the agent's
/// folder, beside `agent.md`; never in `runtime/`, since it must outlive
/// a kernel.
const ASKED: &str = "title-asked";

/// The label is nobody's choice: empty, or the opening words of the first
/// prompt as a client's fallback cut them (`chattitle::from_prompt`, or
/// the desktop's first-clause cut). Compared on words, case-blind.
fn is_fallback(title: &str, first_user: &str) -> bool {
    let t: Vec<String> = title
        .split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect();
    if t.is_empty() {
        return true;
    }
    let p: Vec<String> = first_user
        .split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect();
    p.len() >= t.len() && p[..t.len()] == t[..]
}

/// The first exchange: the first user (or kickoff) line and the first
/// non-empty assistant line after it.
fn first_exchange(place: &Place, id: &str) -> Option<(String, String)> {
    let events = load_transcript(&arbos_core::Layout::new(place, id).transcript()).ok()?;
    let mut user: Option<String> = None;
    for e in &events {
        match &e.kind {
            EventKind::User { text, .. } if user.is_none() && !text.trim().is_empty() => {
                user = Some(text.clone());
            }
            EventKind::Wake {
                text: Some(text), ..
            } if user.is_none() && !text.trim().is_empty() => {
                user = Some(text.clone());
            }
            EventKind::Assistant { text, .. } if user.is_some() && !text.trim().is_empty() => {
                return Some((user.unwrap_or_default(), text.clone()));
            }
            _ => {}
        }
    }
    None
}

/// Called when `id`'s turn ends. Decides on disk facts, then makes the
/// one call off the serve loop.
pub fn after_turn(hooks: &Arc<KernelHooks>, id: &str) {
    let place = hooks.place.clone();
    let dir = place.agent_dir(id);
    let Ok(agent) = Agent::load(&dir) else {
        return;
    };
    // A real name is the person's or the parent's: theirs to keep.
    if !chattitle::is_generic(&agent.name, Some(id)) {
        return;
    }
    if dir.join(ASKED).exists() {
        return;
    }
    // No key: the first turn was refused; the fallback stands and the
    // question is asked again after a turn that ran.
    if crate::serve::keyless(&place).is_some() {
        return;
    }
    let Some((first_user, first_reply)) = first_exchange(&place, id) else {
        return;
    };
    if !is_fallback(&agent.title, &first_user) {
        // Someone set a title of their own; the call is not owed.
        let _ = std::fs::write(dir.join(ASKED), "kept: a title was already set\n");
        return;
    }
    // Marked before the call: one, ever, whatever the call does.
    if let Err(e) = std::fs::write(dir.join(ASKED), "asked\n") {
        klog::warn("chat_title_mark_failed", Some(id), format!("{e:#}"));
        return;
    }
    let hooks = Arc::clone(hooks);
    let id = id.to_string();
    tokio::spawn(async move {
        let host = match arbos_engine::Host::load().or_else(|_| arbos_engine::Host::peek()) {
            Ok(h) => h,
            Err(e) => {
                klog::warn(
                    "chat_title_skipped",
                    Some(&id),
                    format!("host config: {e:#}"),
                );
                return;
            }
        };
        match arbos_engine::title::title_for(&place, &host, &id, &first_user, &first_reply).await {
            Ok(Some(title)) => {
                // Re-read: the person may have named it meanwhile.
                let Ok(mut agent) = Agent::load(&dir) else {
                    return;
                };
                if !chattitle::is_generic(&agent.name, Some(&id))
                    || !is_fallback(&agent.title, &first_user)
                {
                    klog::info("chat_title_kept", Some(&id), "named meanwhile");
                    return;
                }
                agent.title = title.clone();
                if let Err(e) = agent.save(&dir) {
                    klog::warn("chat_title_unsaved", Some(&id), format!("{e:#}"));
                    return;
                }
                klog::info("chat_titled", Some(&id), &title);
                hooks.broadcast_tree();
            }
            Ok(None) => klog::info("chat_title_skipped", Some(&id), "no key or no script line"),
            Err(e) => klog::warn("chat_title_failed", Some(&id), format!("{e:#}")),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::is_fallback;

    #[test]
    fn the_prompts_opening_words_are_a_fallback_and_a_real_title_is_not() {
        let prompt = "This project is a research notebook about container image formats (OCI, Docker v2, singularity)…";
        assert!(is_fallback("", prompt));
        assert!(is_fallback("This project is a", prompt));
        assert!(is_fallback("This project is a research notebook", prompt));
        assert!(is_fallback("this project is a research notebook…", prompt));
        assert!(!is_fallback("Container image formats notebook", prompt));
        assert!(!is_fallback("Notes restructure", prompt));
    }
}
