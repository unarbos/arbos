//! One-time migration from the plan engine to Cursor's shape, at kernel
//! start. Kernels before this one kept each agent's intent in
//! `plan.jsonl` (nodes with `when × do`) and the place's GitHub follows in
//! `.arbos/subscriptions.json`. Now:
//!
//! - a recurring or deferred node (`every`, `after`) becomes a subscription
//!   file (`timer`, or `shell` with `deliver_to = user` when it had a
//!   `notify`), keeping its next due;
//! - an open one-shot agent node becomes a line of `notes.md`;
//! - a pending message node (older still) becomes an inbox file;
//! - a done, cancelled, or failed node is dropped (git has it);
//! - every `subscriptions.json` entry becomes a `github_pr` file of its agent.
//!
//! The old files are renamed `*.migrated` so this runs once and nothing is
//! lost. The nodes are read with a reader of their own here: the types
//! they came from are gone.

use std::path::Path;

use arbos_core::{
    Place, inbox, list_agents, notes,
    subscription::{self, Subscription},
};
use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
struct OldWhen {
    #[serde(default)]
    after_ms: Option<i64>,
    #[serde(default)]
    every_ms: Option<u64>,
    #[serde(default)]
    next_due_ms: Option<i64>,
    #[serde(default)]
    condition: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum OldDo {
    Agent,
    Shell {
        cmd: String,
        #[serde(default)]
        report: Option<String>,
    },
    Notify {
        text: String,
    },
    Ask,
}

impl Default for OldDo {
    fn default() -> Self {
        Self::Agent
    }
}

#[derive(Debug, Deserialize)]
struct OldNode {
    id: u64,
    #[serde(default)]
    parent: u64,
    goal: String,
    #[serde(default)]
    when: OldWhen,
    #[serde(rename = "do", default)]
    do_: OldDo,
    status: String,
    #[serde(default)]
    outcome: String,
    #[serde(default)]
    origin: String,
    #[serde(default)]
    hops: u8,
    #[serde(default)]
    attachments: Vec<String>,
    #[serde(default)]
    created_ms: i64,
}

/// Last line per id wins, as the old kernel read it.
fn load_old(path: &Path) -> Vec<OldNode> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut by_id: Vec<OldNode> = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(n) = serde_json::from_str::<OldNode>(line) else {
            continue;
        };
        if let Some(slot) = by_id.iter_mut().find(|x| x.id == n.id) {
            *slot = n;
        } else {
            by_id.push(n);
        }
    }
    by_id.sort_by_key(|n| (n.parent, n.id));
    by_id
}

/// A summary line per agent migrated, for the log.
pub fn run(hooks: &crate::hooks::KernelHooks) -> Vec<String> {
    let place = &hooks.place;
    let mut report = Vec::new();
    for agent in list_agents(place).unwrap_or_default() {
        let id = agent.id.as_str();
        let layout = arbos_core::Layout::new(place, id);
        let plan = layout.plan_jsonl();
        if plan.exists()
            && let Some(line) = migrate_plan(hooks, id, &plan)
        {
            report.push(line);
        }
        let attempts = layout.attempts_jsonl();
        if attempts.exists() {
            let _ = std::fs::rename(&attempts, attempts.with_extension("jsonl.migrated"));
        }
        // The old generated render is not anyone's file now; it goes aside.
        // (Root's checklist lives at .arbos/notes.md, the project page.)
        let old_md = layout.plan_md();
        if old_md.exists() {
            let _ = std::fs::rename(&old_md, old_md.with_extension("md.migrated"));
        }
    }
    let json = place.arbos().join("subscriptions.json");
    if json.exists()
        && let Some(line) = migrate_github(place, &json)
    {
        report.push(line);
    }
    report
}

fn is_inbox_node(n: &OldNode, all: &[OldNode]) -> bool {
    // The old kernel's rule: a root agent node with a message origin and
    // no schedule, whose goal is the message itself.
    n.parent == 0
        && matches!(n.do_, OldDo::Agent)
        && n.when.every_ms.is_none()
        && n.when.after_ms.is_none()
        && n.when.condition.is_empty()
        && (n.origin.is_empty()
            || n.origin == "user"
            || n.origin.starts_with("user:")
            || n.origin.starts_with("agent:")
            || n.origin.starts_with("spawn:")
            || n.origin == "kernel")
        && !all.iter().any(|k| k.parent == n.id)
}

fn migrate_plan(hooks: &crate::hooks::KernelHooks, agent: &str, path: &Path) -> Option<String> {
    let place = &hooks.place;
    let nodes = load_old(path);
    let mut n_asks = 0;
    let mut n_notes = 0;
    let mut n_subs = 0;
    let mut n_inbox = 0;
    let mut n_dropped = 0;
    let mut notes_file = notes::load(place, agent);
    let now = arbos_core::now_ms();
    for n in &nodes {
        let open = matches!(n.status.as_str(), "pending" | "active" | "blocked");
        if !open {
            n_dropped += 1;
            continue;
        }
        // An open question for the user parks as a waiting file with its
        // ask line on the transcript, so the card appears and the answer
        // arrives as a message (#106). A checklist line would never be
        // put to anyone (qa-028).
        if matches!(n.do_, OldDo::Ask) {
            let call_id = format!("migrated-{}", n.id);
            match hooks.ask(&arbos_core::AgentId::new(agent), &n.goal, &[], &call_id) {
                Ok(_) => {
                    let _ = arbos_core::append_event(
                        &hooks.layout(agent).transcript(),
                        &arbos_core::Event::new(arbos_core::EventKind::Notice {
                            text: format!(
                                "A question from the old plan (node #{}) is waiting for your answer above.",
                                n.id
                            ),
                            failed: false,
                        }),
                    );
                    n_asks += 1;
                }
                Err(e) => {
                    crate::klog::warn("migrate_ask", Some(agent), format!("node #{}: {e:#}", n.id));
                    n_dropped += 1;
                }
            }
            continue;
        }
        let scheduled = n.when.every_ms.is_some() || n.when.after_ms.is_some();
        if scheduled || !n.when.condition.is_empty() {
            let every = n
                .when
                .every_ms
                .map(|ms| subscription::human_ms(ms.max(subscription::MIN_EVERY_MS)));
            let due = n
                .when
                .next_due_ms
                .or(n.when.after_ms)
                .unwrap_or_else(|| now + n.when.every_ms.unwrap_or(60_000) as i64);
            let mut sub = Subscription {
                id: 0,
                kind: "timer".into(),
                prompt: n.goal.clone(),
                every: every.clone(),
                at: None,
                once: n.when.every_ms.is_none(),
                cmd: None,
                path: None,
                repo: None,
                pr: None,
                deliver_to: "agent".into(),
                notify: None,
                expires: None,
                paused: n.status == "blocked",
                internal: false,
                created: inbox::rfc3339(if n.created_ms > 0 { n.created_ms } else { now }),
                next_due: Some(inbox::rfc3339(due.max(now))),
                last_fired: None,
                last: if n.outcome.is_empty() {
                    String::new()
                } else {
                    format!("(migrated) {}", n.outcome)
                },
                error: None,
                seen: None,
            };
            match &n.do_ {
                OldDo::Shell { cmd, report } => {
                    sub.kind = "shell".into();
                    sub.cmd = Some(cmd.clone());
                    if let Some(r) = report {
                        sub.deliver_to = "user".into();
                        sub.notify = Some(if r.contains("{output}") {
                            r.clone()
                        } else {
                            format!("{r} {{output}}")
                        });
                    }
                    if every.is_none() {
                        sub.every = Some("1h".into());
                        sub.once = true;
                    }
                }
                OldDo::Notify { text } => {
                    // A notify with no command: a timer whose prompt says
                    // what to tell the user.
                    sub.prompt = format!("Tell the user: {text}");
                }
                OldDo::Agent | OldDo::Ask => {}
            }
            if !n.when.condition.is_empty() {
                // A polled predicate becomes a shell subscription that
                // wakes the agent when the predicate holds (exit 0 →
                // output → the agent is told).
                sub.kind = "shell".into();
                sub.cmd = Some(n.when.condition.clone());
                sub.deliver_to = "agent".into();
                sub.prompt = format!("Condition held: {}", n.goal);
                if sub.every.is_none() {
                    sub.every = Some("5m".into());
                }
                sub.once = false;
            }
            if sub.every.is_none() && sub.next_due.is_none() {
                n_dropped += 1;
                continue;
            }
            match subscription::add(place, agent, sub, None) {
                Ok(_) => n_subs += 1,
                Err(e) => {
                    crate::klog::warn(
                        "migrate_subscription",
                        Some(agent),
                        format!("node #{}: {e:#}", n.id),
                    );
                    n_dropped += 1;
                }
            }
            continue;
        }
        if n.status == "pending" && is_inbox_node(n, &nodes) {
            let (from, kind) = match n.origin.as_str() {
                o if o.starts_with("spawn:") => {
                    (format!("agent:{}", &o["spawn:".len()..]), "brief")
                }
                "" | "user" => ("user".to_string(), "request"),
                o => (o.to_string(), "request"),
            };
            let mut msg = inbox::Message::new(from, kind, n.goal.clone());
            msg.wake = true;
            msg.hops = n.hops;
            msg.attachments = n.attachments.clone();
            msg.sent = inbox::rfc3339(if n.created_ms > 0 { n.created_ms } else { now });
            if inbox::deliver(place, agent, &msg).is_ok() {
                n_inbox += 1;
                continue;
            }
        }
        // A goal with children is a section; its children are the lines.
        if nodes.iter().any(|k| {
            k.parent == n.id && matches!(k.status.as_str(), "pending" | "active" | "blocked")
        }) {
            continue;
        }
        // An open goal: one checklist line, with its last outcome as the
        // readout. Children go under a section named for their parent.
        let section = nodes
            .iter()
            .find(|p| p.id == n.parent)
            .map(|p| arbos_core::text::clip(&p.goal, 60))
            .unwrap_or_default();
        let text = if n.outcome.is_empty() {
            n.goal.clone()
        } else {
            format!("{} — {}", n.goal, arbos_core::text::clip(&n.outcome, 120))
        };
        notes_file.add(&section, &text);
        n_notes += 1;
    }
    if n_notes > 0 {
        let _ = notes::save(place, agent, &notes_file);
    }
    let _ = std::fs::rename(path, path.with_extension("jsonl.migrated"));
    Some(format!(
        "{agent}: {} node(s) → {n_notes} notes line(s), {n_subs} subscription(s), {n_inbox} inbox file(s), {n_asks} parked question(s); {n_dropped} closed node(s) dropped",
        nodes.len()
    ))
}

#[derive(Debug, Deserialize)]
struct OldGithubSub {
    agent: String,
    repo: String,
    pr: u64,
    #[serde(default)]
    note: String,
    #[serde(default)]
    last: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Default)]
struct OldGithubFile {
    #[serde(default)]
    subscriptions: Vec<OldGithubSub>,
}

fn migrate_github(place: &Place, path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let file: OldGithubFile = serde_json::from_str(&text).unwrap_or_default();
    let mut n = 0;
    for old in file.subscriptions {
        if !arbos_core::agent_exists(place, &old.agent) {
            continue;
        }
        let sub = Subscription {
            id: 0,
            kind: "github_pr".into(),
            prompt: old.note.clone(),
            every: None,
            at: None,
            once: false,
            cmd: None,
            path: None,
            repo: Some(old.repo.clone()),
            pr: Some(old.pr),
            deliver_to: "agent".into(),
            notify: None,
            expires: None,
            paused: false,
            internal: false,
            created: String::new(),
            next_due: None,
            last_fired: None,
            last: String::new(),
            error: None,
            seen: old.last.as_ref().map(|v| v.to_string()),
        };
        if subscription::add(place, &old.agent, sub, None).is_ok() {
            n += 1;
        }
    }
    let _ = std::fs::rename(path, path.with_extension("json.migrated"));
    Some(format!(
        "subscriptions.json: {n} pull-request follow(s) → github_pr files"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(tag: &str) -> Place {
        let dir = std::env::temp_dir().join(format!("arbos-migrate-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".arbos/agents/root")).unwrap();
        let p = Place::new(dir);
        arbos_core::Agent::root("root")
            .save(&p.agent_dir("root"))
            .unwrap();
        p
    }

    fn hooks(p: &Place) -> std::sync::Arc<crate::hooks::KernelHooks> {
        let (wake_tx, _wake_rx) = tokio::sync::mpsc::unbounded_channel();
        let (kick_tx, _kick_rx) = tokio::sync::mpsc::unbounded_channel();
        crate::hooks::KernelHooks::new(p.clone(), wake_tx, kick_tx)
    }

    #[test]
    fn nodes_become_notes_subscriptions_and_inbox_files() {
        let p = place("nodes");
        let plan = arbos_core::Layout::new(&p, "root").plan_jsonl();
        let now = arbos_core::now_ms();
        let lines = [
            r#"{"id":1,"goal":"Ship the feature","status":"pending","origin":"user"}"#,
            r#"{"id":2,"parent":1,"seq":1,"goal":"write the code","status":"done","outcome":"merged"}"#,
            r#"{"id":3,"parent":1,"seq":2,"goal":"write the docs","status":"pending","outcome":"half done"}"#,
            &format!(
                r#"{{"id":4,"goal":"check the build","when":{{"every_ms":3600000,"next_due_ms":{}}},"do":{{"kind":"shell","cmd":"make test","report":"build: {{output}}"}},"status":"pending"}}"#,
                now + 1000
            ),
            r#"{"id":5,"goal":"remind me to stretch","when":{"after_ms":9999999999999},"status":"pending"}"#,
            r#"{"id":6,"goal":"hello from the phone","status":"pending","origin":"user","when":{"wake":true}}"#,
            r#"{"id":7,"goal":"Which colour, teal or red?","do":{"kind":"ask"},"status":"pending","origin":"user"}"#,
        ];
        std::fs::write(&plan, lines.join("\n") + "\n").unwrap();
        let h = hooks(&p);
        let report = run(&h);
        assert_eq!(report.len(), 1, "{report:?}");
        // The open ask is parked, not a checklist line (qa-028).
        let asks = arbos_core::waiting::asks(&p, "root");
        assert_eq!(asks.len(), 1, "{asks:?}");
        assert_eq!(asks[0].question, "Which colour, teal or red?");
        assert_eq!(asks[0].id, "migrated-7");
        let transcript =
            std::fs::read_to_string(p.agent_dir("root").join("transcript.jsonl")).unwrap();
        assert!(
            transcript.contains("\"kind\":\"ask\"") && transcript.contains("migrated-7"),
            "{transcript}"
        );
        assert!(!plan.exists() && plan.with_extension("jsonl.migrated").exists());
        let n = notes::load(&p, "root");
        let items = n.items();
        assert_eq!(items.len(), 1, "{}", n.render());
        assert!(
            p.arbos().join("notes.md").exists(),
            "root's checklist is the project page"
        );
        assert!(!p.agent_dir("root").join("notes.md").exists());
        assert_eq!(items[0].section, "Ship the feature");
        assert!(items[0].text.starts_with("write the docs — half done"));
        let subs = subscription::list(&p, "root");
        assert_eq!(subs.len(), 2, "{subs:?}");
        let shell = subs.iter().find(|s| s.kind == "shell").unwrap();
        assert_eq!(shell.deliver_to, "user");
        assert_eq!(shell.notify.as_deref(), Some("build: {output}"));
        assert_eq!(shell.every.as_deref(), Some("1h"));
        let timer = subs.iter().find(|s| s.kind == "timer").unwrap();
        assert!(timer.once && timer.prompt == "remind me to stretch");
        let inbox = inbox::list(&p, "root");
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].msg.body, "hello from the phone");
        assert!(!n.render().contains("teal"), "{}", n.render());
        // Second run: nothing to do.
        assert!(run(&h).is_empty());
    }

    #[test]
    fn github_follows_become_per_agent_files() {
        let p = place("gh");
        std::fs::write(
            p.arbos().join("subscriptions.json"),
            r#"{"next_id":2,"subscriptions":[{"id":1,"agent":"root","repo":"unarbos/arbos","pr":58,"note":"tell me when merged","created_ms":0}]}"#,
        )
        .unwrap();
        let report = run(&hooks(&p));
        assert_eq!(report.len(), 1);
        let subs = subscription::list(&p, "root");
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].kind, "github_pr");
        assert_eq!(subs[0].pr, Some(58));
        assert_eq!(subs[0].prompt, "tell me when merged");
    }
}
