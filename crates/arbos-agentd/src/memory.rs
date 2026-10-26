//! Memory as decision models (design §6.3, memory-decision-models §5): the write
//! pass (gate → candidates → IGNORE/ADD/UPDATE/MERGE/INVALIDATE) and chat
//! compaction with its write pass (§6.6). Both run off the reply path.

use std::sync::Arc;

use anyhow::Result;
use serde_json::{json, Value};

use arbos_core::ids::now_ms;

use crate::config::Config;
use crate::llm::Llm;
use crate::store::{MemoryRow, Store};

pub const COMPACTION_TURNS: i64 = 20;
pub const COMPACTION_TOKENS: usize = 24_000;
pub const VERBATIM_TURNS: usize = 12;
pub const TOOL_OUTPUT_CAP_FOR_SUMMARISER: usize = 2_000;

const WRITE_SYSTEM: &str = r#"You are the memory write decision for a project agent. Decide what, if anything, the project should remember from the event below.
First a gate: is anything here worth storing beyond the raw transcript (a decision, a durable fact, a user preference or correction, a project reference such as a path or URL, or a status that others need)? Transient chatter, tool noise, and things already obvious from the files are not store-worthy. Never store the user's name, handle or how to address them: the name the agent used in a reply and the sender label on a message are presentation that comes live from the user's profile at every turn, not facts (a name can differ per device and change at any time). Candidates like "The user's name is X" / "address the user as X" are IGNORE. Refer to the project's owner as "the owner" (or "the user"), never by name: the name is resolved live when the record is read.
Then extract at most 8 candidate records. For each candidate compare with the SIMILAR existing records and choose one action:
- IGNORE: not worth storing or already stored.
- ADD: new.
- UPDATE: refines one existing record (give its id in target_ids).
- MERGE: replaces two or more existing records with one (give all ids).
- INVALIDATE: contradicts an existing record (give its id); the new text is stored as a new record.
Fields per candidate: text (one or two self-contained sentences), kind (user|feedback|project|reference|decision|status), scope (user|project|thread), tier (T1 for project-level facts and decisions, T2 for thread-level status and facts), action, target_ids, reason (one line), confidence (0-1), importance (1-10).
Output: {"gate":{"store_worthy":bool,"confidence":0-1},"decisions":[...]}"#;

const COMPACTION_SYSTEM: &str = r#"You write the rolling summary of one chat between a user and a project agent. You receive the previous summary and the turns that are leaving the verbatim window. Write a new summary (at most ~1500 words) that carries the old one forward.
Preserve, in this priority: decisions and their reasons; open items and questions; user corrections; artifacts (file paths, URLs, commit ids, names); who did what (thread ids). Drop pleasantries, tool chatter, superseded drafts.
Handles listed in the working set stay available; record which ones matter, do not restate their content.
Output: {"summary":"...","candidates":[{"text":"...","kind":"decision|project|reference|status|user|feedback","scope":"project|thread"}]}"#;

pub struct Memory {
    pub cfg: Arc<Config>,
    pub store: Store,
    pub llm: Arc<Llm>,
    /// The owner's current and former display names as the relay reports them, for
    /// `owner_by_role`: memory refers to the owner as "the owner", never by a name that may
    /// change (Hezbola 2026-10-26: old records said Arbos, the profile said Jacob).
    pub owner_names: std::sync::Mutex<Vec<String>>,
}

/// `"Jacob asked for …"` → `"the owner asked for …"` for every known owner name, whole words
/// only, case-sensitive. Names that are also the product's or the agent's name are left alone
/// (replacing "Arbos" would rewrite "Arbos is the app").
pub(crate) fn owner_by_role(text: &str, owner_names: &[String], skip: &[&str]) -> String {
    let mut out = text.to_owned();
    for name in owner_names {
        let n = name.trim();
        if n.len() < 2 || skip.iter().any(|s| s.eq_ignore_ascii_case(n)) {
            continue;
        }
        let mut res = String::with_capacity(out.len());
        let mut rest = out.as_str();
        while let Some(i) = rest.find(n) {
            let before = rest[..i].chars().next_back();
            let after = rest[i + n.len()..].chars().next();
            let boundary = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric());
            res.push_str(&rest[..i]);
            if boundary(before) && boundary(after) {
                res.push_str("the owner");
            } else {
                res.push_str(n);
            }
            rest = &rest[i + n.len()..];
        }
        res.push_str(rest);
        out = res;
    }
    out
}

/// A candidate that would fix the user's name or form of address in memory (P0 San José,
/// 2026-10-25: the write pass learned "The user's name is Const" from a reply that used the
/// iPhone's `senderName`, and the agent kept saying Const after the display name changed and on
/// devices that send another name). The address comes live from the row/profile every turn;
/// such a record is never stored.
pub(crate) fn is_user_name_fact(text: &str) -> bool {
    let t: String = text
        .to_lowercase()
        .chars()
        .map(|c| match c {
            '\u{2019}' | '\u{2018}' | '`' => '\'',
            c if c.is_alphanumeric() || c == ' ' || c == '\'' => c,
            _ => ' ',
        })
        .collect();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    const PATTERNS: &[&str] = &[
        "the user's name is",
        "the users name is",
        "user's name is",
        "the user is called",
        "the user is named",
        "the user goes by",
        "the user prefers to be called",
        "the user wants to be called",
        "the user should be addressed as",
        "address the user as",
        "address them as",
        "call the user",
        "the owner's name is",
        "the owner is called",
        "user's display name is",
        "the user's handle is",
        "user's handle is",
        "user's first name is",
        "name of the user is",
        "my name is",
        "i am called",
        "call me ",
    ];
    PATTERNS.iter().any(|p| t.contains(p))
}

impl Memory {
    pub fn new(cfg: Arc<Config>, store: Store, llm: Arc<Llm>) -> Self {
        Self {
            cfg,
            store,
            llm,
            owner_names: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// T0 + T1 whole, as Markdown lines for the prompt (design §6.4 item 2).
    pub fn core_memory_text(&self) -> String {
        let mut out = String::new();
        let t0 = self.store.memory_live("T0", None, 40).unwrap_or_default();
        let t1 = self.store.memory_live("T1", None, 80).unwrap_or_default();
        if !t0.is_empty() {
            out.push_str("## User core\n");
            for m in t0 {
                out.push_str(&format!("- {}\n", m.text));
            }
        }
        if !t1.is_empty() {
            out.push_str("## Project core\n");
            for m in t1 {
                out.push_str(&format!("- [{}] {}\n", m.kind, m.text));
            }
        }
        let threads = self.store.subagents().unwrap_or_default();
        if !threads.is_empty() {
            out.push_str("## Threads\n");
            for t in threads.iter().take(30) {
                out.push_str(&format!(
                    "- {} · {} · {} · {} · {}\n",
                    t.id,
                    t.name,
                    t.purpose,
                    t.status,
                    t.summary.chars().take(120).collect::<String>()
                ));
            }
        }
        out
    }

    /// Name/address records written before the rule existed are superseded on the next pass:
    /// invalidated (the row stays for the audit, D7) so the prompt's User core and the walks stop
    /// offering them. Idempotent and cheap (T0/T1 live rows only).
    pub fn purge_user_name_memories(&self) -> usize {
        let mut n = 0;
        for tier in ["T0", "T1"] {
            for m in self.store.memory_live(tier, None, 200).unwrap_or_default() {
                if is_user_name_fact(&m.text)
                    && self.store.memory_invalidate(&m.id, Some(now_ms())).is_ok()
                {
                    n += 1;
                    tracing::info!(id = %m.id, text = %m.text.chars().take(120).collect::<String>(), "memory: user-name record superseded (address comes live from the profile)");
                    continue;
                }
                // A record that names the owner is rewritten to the role, in place: the text
                // stays true when the owner's display name changes.
                let by_role = self.owner_by_role_text(&m.text);
                if by_role != m.text && self.store.memory_set_text(&m.id, &by_role).is_ok() {
                    n += 1;
                    tracing::info!(id = %m.id, "memory: owner named by role instead of by name");
                }
            }
        }
        n
    }

    /// The owner's names the relay reports (current and former), from the box listing.
    pub fn set_owner_names(&self, names: Vec<String>) {
        *self.owner_names.lock().expect("owner names") = names;
    }

    fn owner_by_role_text(&self, text: &str) -> String {
        let names = self.owner_names.lock().expect("owner names").clone();
        owner_by_role(text, &names, &["Arbos", self.cfg.project_name.as_str()])
    }

    /// Write-side decision over one event (user message, assistant reply, subagent turn end).
    pub async fn write_pass(
        &self,
        event_id: &str,
        thread_id: &str,
        source_role: &str,
        text: &str,
        proposals: &[Value],
    ) -> Result<()> {
        let text = crate::truncate::clip_marked(text, 12_000, "", None);
        if text.trim().len() < 20 && proposals.is_empty() {
            return Ok(());
        }
        self.purge_user_name_memories();
        // Similar records: a coarse FTS pass over the event text (top-5 per tier).
        let mut similar: Vec<MemoryRow> = Vec::new();
        for tier in ["T1", "T2"] {
            if let Ok(hits) = self.store.memory_search(
                &text.chars().take(400).collect::<String>(),
                &[tier.to_owned()],
                false,
                5,
            ) {
                similar.extend(hits.into_iter().map(|(m, _)| m));
            }
        }
        let similar_json: Vec<Value> = similar.iter().map(|m| json!({ "id": m.id, "tier": m.tier, "kind": m.kind, "text": m.text, "created_at": m.created_at })).collect();
        let user = json!({
            "event": { "id": event_id, "thread_id": thread_id, "role": source_role, "text": text },
            "proposed_by_subagent": proposals,
            "similar": similar_json,
        })
        .to_string();
        let (v, ms) = match self
            .llm
            .complete_json(&self.cfg.decision_model, WRITE_SYSTEM, &user, 1500)
            .await
        {
            Ok(x) => x,
            Err(e) => {
                // Fail open and count it (design §6.3 logging).
                let _ = self.store.log_decision(
                    "write",
                    Some(event_id),
                    Some(&json!({ "error": e.to_string() })),
                    None,
                    &[],
                    Some("decision model failed"),
                    None,
                    Some(&self.cfg.decision_model),
                    None,
                );
                return Ok(());
            }
        };
        let gate = v.get("gate").cloned().unwrap_or(json!({}));
        let store_worthy = gate
            .get("store_worthy")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !store_worthy {
            self.store.log_decision(
                "write",
                Some(event_id),
                Some(&gate),
                Some("IGNORE"),
                &[],
                Some("gate: not store-worthy"),
                gate.get("confidence").and_then(Value::as_f64),
                Some(&self.cfg.decision_model),
                Some(ms),
            )?;
            return Ok(());
        }
        for d in v
            .get("decisions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .take(8)
        {
            let action = d
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("IGNORE")
                .to_ascii_uppercase();
            let targets: Vec<String> = d
                .get("target_ids")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let text = d
                .get("text")
                .or(d.get("new_text"))
                .or(d.get("candidate"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_owned();
            let kind = d.get("kind").and_then(Value::as_str).unwrap_or("project");
            let scope = d.get("scope").and_then(Value::as_str).unwrap_or("project");
            let tier = d
                .get("tier")
                .and_then(Value::as_str)
                .unwrap_or(if scope == "thread" { "T2" } else { "T1" });
            let conf = d.get("confidence").and_then(Value::as_f64).unwrap_or(0.5);
            let imp = d
                .get("importance")
                .and_then(Value::as_i64)
                .unwrap_or(5)
                .clamp(1, 10);
            let reason = d.get("reason").and_then(Value::as_str);
            if is_user_name_fact(&text) {
                tracing::info!(event_id, text = %text.chars().take(120).collect::<String>(), "memory write: user-name candidate dropped (address comes live from the profile)");
                continue;
            }
            let text = self.owner_by_role_text(&text);
            let thread = if tier == "T2" { Some(thread_id) } else { None };
            let source =
                json!({ "event_id": event_id, "thread_id": thread_id, "role": source_role });
            let mut touched = targets.clone();
            match action.as_str() {
                "ADD" if !text.is_empty() => {
                    let id = self.store.memory_add(
                        tier,
                        thread,
                        kind,
                        scope,
                        &text,
                        Some(&source),
                        Some(now_ms()),
                        conf,
                        imp,
                        &[],
                    )?;
                    touched.push(id);
                }
                "UPDATE" | "MERGE" if !text.is_empty() && !targets.is_empty() => {
                    for t in &targets {
                        self.store.memory_invalidate(t, None)?;
                    }
                    let id = self.store.memory_add(
                        tier,
                        thread,
                        kind,
                        scope,
                        &text,
                        Some(&source),
                        Some(now_ms()),
                        conf,
                        imp,
                        &targets,
                    )?;
                    touched.push(id);
                }
                "INVALIDATE" if !targets.is_empty() => {
                    for t in &targets {
                        self.store.memory_invalidate(t, Some(now_ms()))?;
                    }
                    if !text.is_empty() {
                        let id = self.store.memory_add(
                            tier,
                            thread,
                            kind,
                            scope,
                            &text,
                            Some(&source),
                            Some(now_ms()),
                            conf,
                            imp,
                            &[],
                        )?;
                        touched.push(id);
                    }
                }
                _ => {}
            }
            self.store.log_decision(
                "write",
                Some(event_id),
                Some(&gate),
                Some(&action),
                &touched,
                reason,
                Some(conf),
                Some(&self.cfg.decision_model),
                Some(ms),
            )?;
        }
        Ok(())
    }

    /// Compaction trigger (design §6.6): 20 turns past `compacted_through` or > 24k tokens in summary + window.
    pub async fn maybe_compact(&self, chat_id: &str) -> Result<()> {
        let Some(chat) = self.store.chat(chat_id)? else {
            return Ok(());
        };
        let turns = self
            .store
            .t3_thread(chat_id, chat.compacted_through.as_deref(), None, 400)?;
        let turns: Vec<_> = turns
            .into_iter()
            .filter(|t| t.kind == "user" || t.kind == "assistant")
            .collect();
        let approx_tokens = (chat
            .compaction_summary
            .as_deref()
            .map(str::len)
            .unwrap_or(0)
            + turns.iter().map(|t| t.text.len()).sum::<usize>())
            / 4;
        if (turns.len() as i64) < COMPACTION_TURNS && approx_tokens < COMPACTION_TOKENS {
            return Ok(());
        }
        if turns.len() <= VERBATIM_TURNS {
            return Ok(());
        }
        let leaving = &turns[..turns.len() - VERBATIM_TURNS];
        let through = leaving.last().map(|t| t.id.clone()).unwrap_or_default();
        let mut body = String::new();
        body.push_str("## Previous summary\n");
        body.push_str(chat.compaction_summary.as_deref().unwrap_or("(none)"));
        body.push_str("\n\n## Turns leaving the window\n");
        for t in leaving {
            let text: String = if t.kind == "tool_output" {
                let s = &t.text;
                if s.len() > TOOL_OUTPUT_CAP_FOR_SUMMARISER {
                    format!(
                        "{}…{}",
                        &s[..1500.min(s.len())],
                        &s[s.len().saturating_sub(500)..]
                    )
                } else {
                    s.clone()
                }
            } else {
                t.text.clone()
            };
            body.push_str(&format!(
                "[{}] {}\n",
                t.role.as_deref().unwrap_or(&t.kind),
                text
            ));
        }
        let (v, _ms) = self
            .llm
            .complete_json(&self.cfg.talking_model, COMPACTION_SYSTEM, &body, 2500)
            .await?;
        let summary = v
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if summary.is_empty() {
            return Ok(());
        }
        self.store
            .set_chat_compaction(chat_id, &summary, &through)?;
        let _ = self.store.t3_append(
            chat_id,
            None,
            None,
            "compaction",
            Some("system"),
            &format!("chat compacted through {through}: {} chars", summary.len()),
            None,
        );
        let candidates = v
            .get("candidates")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if !candidates.is_empty() {
            self.write_pass(
                &format!("compaction:{through}"),
                chat_id,
                "compaction",
                &summary,
                &candidates,
            )
            .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P0 San José (2026-10-25): the write pass learned "The user's name is Const" from a reply
    /// that used the iPhone's `senderName`. Name and address records are presentation, never
    /// memory: new candidates are dropped, old rows are superseded on the next pass.
    #[test]
    fn user_name_facts_are_recognised_and_nothing_else_is() {
        for t in [
            "The user's name is Const.",
            "The user’s name is Const",
            "Address the user as Const, never as @unarbos.",
            "The user prefers to be called Jake.",
            "The owner's name is Jacob Steeves.",
            "The user's display name is Const on the iPhone.",
            "User goes by Const in this chat — the user goes by that name.",
            "My name is Jacob; call me Jake.",
        ] {
            assert!(is_user_name_fact(t), "should be a name fact: {t}");
        }
        for t in [
            "The user wants any outgoing email drafted and shown for review first.",
            "Santa Teresa surf school shortlist: Del Soul (5.0 from 593 reviews).",
            "This agent was renamed from \"Home\" to \"GigChadAgent\"; it introduces itself as \"GigChadAgent\".",
            "The slugkit fix is commit a2a9d29 and was not pushed.",
            "Deleting old build target directories requires the owner's explicit approval.",
            "The project is named tally and its CI gate is ./check.sh.",
        ] {
            assert!(!is_user_name_fact(t), "not a name fact: {t}");
        }
    }

    /// Hezbola 2026-10-26: memory refers to the owner by role, so a display-name change never
    /// leaves two names in the records. The app's and the agent's own names are never touched.
    #[test]
    fn owner_is_named_by_role_in_memory() {
        let names = vec!["Jacob".to_owned(), "Const".to_owned(), "Arbos".to_owned()];
        let skip = ["Arbos", "Bubble"];
        assert_eq!(
            owner_by_role(
                "Jacob asked Const to keep the roleplay going.",
                &names,
                &skip
            ),
            "the owner asked the owner to keep the roleplay going."
        );
        assert_eq!(
            owner_by_role("Arbos is the app; Jacobson is a colleague.", &names, &skip),
            "Arbos is the app; Jacobson is a colleague.",
            "the app name and partial matches are left alone"
        );
        assert_eq!(owner_by_role("no owner here", &[], &skip), "no owner here");
        assert!(WRITE_SYSTEM.contains("Refer to the project's owner as \"the owner\""));
    }

    #[test]
    fn existing_name_memories_are_superseded_on_the_next_pass() {
        let dir = std::env::temp_dir().join(format!("arbos-mem-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("agentd.db")).unwrap();
        let bx = crate::config::BoxConfig {
            user_id: Some("u".into()),
            relay_url: "http://127.0.0.1:1".into(),
            box_token: String::new(),
            mcp_token: String::new(),
            work_root: dir.clone(),
            bind: "127.0.0.1".into(),
            port: 0,
            local_box: true,
            pi_bin: String::new(),
            prime_bin: String::new(),
            claude_bin: String::new(),
            pi_ext_override: None,
            device_id: "box:test".into(),
            default_talking_model: "m".into(),
            default_subagent_model: "m".into(),
            legacy_project_id: None,
        };
        let cfg = Arc::new(bx.workspace(&json!({ "id": "01MPROJ", "name": "Doug" }), 0));
        let llm = Arc::new(Llm::new(&cfg));
        let memory = Memory::new(cfg, store.clone(), llm);
        let name_id = store
            .memory_add(
                "T1",
                None,
                "user",
                "user",
                "The user's name is Const.",
                None,
                Some(1),
                0.9,
                8,
                &[],
            )
            .unwrap();
        let keep_id = store
            .memory_add(
                "T1",
                None,
                "feedback",
                "user",
                "The user wants emails drafted and shown for review.",
                None,
                Some(1),
                0.9,
                8,
                &[],
            )
            .unwrap();
        let named_id = store
            .memory_add(
                "T1",
                None,
                "decision",
                "project",
                "Const wants the roleplay kept up.",
                None,
                Some(1),
                0.9,
                7,
                &[],
            )
            .unwrap();
        memory.set_owner_names(vec!["Jacob".into(), "Const".into()]);
        assert_eq!(memory.purge_user_name_memories(), 2);
        assert_eq!(
            store
                .memory_by_ids(std::slice::from_ref(&named_id))
                .unwrap()[0]
                .text,
            "the owner wants the roleplay kept up."
        );
        let live: Vec<String> = store
            .memory_live("T1", None, 50)
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert!(!live.contains(&name_id), "the name record is superseded");
        assert!(live.contains(&keep_id), "a real preference stays");
        assert!(live.contains(&named_id), "a rewritten record stays live");
        assert_eq!(memory.purge_user_name_memories(), 0, "idempotent");
        assert!(!memory.core_memory_text().contains("Const"));
        assert!(WRITE_SYSTEM.contains("Never store the user's name, handle or how to address them"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
