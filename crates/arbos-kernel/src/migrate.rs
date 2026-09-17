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
        match claim(&plan) {
            Claim::Taken(source) => {
                if let Some(c) = migrate_plan(hooks, id, &source, false) {
                    // The person hears what now runs or waits from the
                    // old plan; the log keeps the count.
                    if c.subs + c.inbox + c.asks + c.notes > 0 {
                        let _ = arbos_core::append_event(
                            &layout.transcript(),
                            &arbos_core::Event::new(arbos_core::EventKind::Notice {
                                text: format!(
                                    "Carried over from the old plan (plan.jsonl): {}. The old file is kept as plan.jsonl.migrated.",
                                    c.said()
                                ),
                                failed: false,
                            }),
                        );
                    }
                    report.push(c.line(id));
                }
                finish(&source);
            }
            Claim::Cut(source) => {
                // An earlier start moved the file aside and died before it
                // finished. Finishing it blind would double what it had
                // already written (qal-j12); leaving it would lose the
                // rest and say so only in the log (qal-j13). So it is
                // finished with the records it already wrote recognised
                // and not written again, and the person hears both counts.
                if let Some(c) = migrate_plan(hooks, id, &source, true) {
                    let _ = arbos_core::append_event(
                        &layout.transcript(),
                        &arbos_core::Event::new(arbos_core::EventKind::Notice {
                            text: format!(
                                "An earlier start began carrying over the old plan (plan.jsonl) and was cut before it finished. Finished now: {} carried over this time; {} were already in place and were not written again. The old file is kept as plan.jsonl.migrated.",
                                c.said(),
                                c.already
                            ),
                            failed: false,
                        }),
                    );
                    crate::klog::warn(
                        "migrate_cut",
                        Some(id),
                        format!("finished a cut migration: {}", c.line(id)),
                    );
                    report.push(c.line(id));
                }
                finish(&source);
            }
            Claim::Blocked(why) => {
                // The completion record could not be written, so nothing
                // is written on its account: a start that migrated and
                // could not say so migrated again the next time, and the
                // standing cron fired twice for ever (qal-j12).
                crate::klog::warn("migrate_blocked", Some(id), &why);
                let _ = arbos_core::append_event(
                    &layout.transcript(),
                    &arbos_core::Event::new(arbos_core::EventKind::Notice {
                        text: format!(
                            "The one-time migration of the old plan (plan.jsonl) could not start: {why}. Nothing was migrated and the old plan stays as it is; fix what blocks writes in this agent's folder and start the kernel again."
                        ),
                        failed: true,
                    }),
                );
            }
            Claim::Absent => {}
        }
        let attempts = layout.attempts_jsonl();
        if attempts.exists()
            && let Err(e) = std::fs::rename(&attempts, attempts.with_extension("jsonl.migrated"))
        {
            crate::klog::warn("migrate_move", Some(id), format!("attempts.jsonl: {e}"));
        }
        // The old generated render is not anyone's file now; it goes aside.
        // (Root's checklist lives at .arbos/notes.md, the project page.)
        let old_md = layout.plan_md();
        if old_md.exists()
            && let Err(e) = std::fs::rename(&old_md, old_md.with_extension("md.migrated"))
        {
            crate::klog::warn("migrate_move", Some(id), format!("plan.md: {e}"));
        }
    }
    let json = place.arbos().join("subscriptions.json");
    match claim(&json) {
        Claim::Taken(source) => {
            if let Some(line) = migrate_github(place, &source) {
                report.push(line);
            }
            finish(&source);
        }
        Claim::Cut(source) => crate::klog::warn(
            "migrate_cut",
            None,
            format!(
                "an earlier migration of subscriptions.json was cut mid-way; its source is kept at {}; not run again",
                source.display()
            ),
        ),
        Claim::Blocked(why) => crate::klog::warn("migrate_blocked", None, &why),
        Claim::Absent => {}
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

/// Whether this start may migrate `old`.
enum Claim {
    /// The old file is moved aside as `*.migrating`: this start owns the
    /// migration and reads from the moved file.
    Taken(std::path::PathBuf),
    /// A `*.migrating` file stands and no old file: an earlier start was
    /// cut between moving the file aside and finishing. Not migrated
    /// again.
    Cut(std::path::PathBuf),
    /// The old file is there and could not be moved aside: nothing is
    /// migrated, and the reason.
    Blocked(String),
    /// Nothing to migrate.
    Absent,
}

/// The completion record comes first (the rule in `arbos_core::record`):
/// the old file is renamed to `*.migrating` *before* a single new record
/// is written, so a start that cannot record "done" writes nothing, and
/// a start that dies mid-way leaves a file that says so rather than one
/// that invites a second pass. `let _ = rename` after the writes was the
/// only record that the migration happened; when it failed once, the
/// next start migrated again, and the standing cron fired twice for ever
/// (qal-j12).
fn claim(old: &Path) -> Claim {
    let ext = old
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_default();
    let migrating = old.with_extension(format!("{ext}.migrating"));
    if migrating.exists() && !old.exists() {
        return Claim::Cut(migrating);
    }
    if !old.exists() {
        return Claim::Absent;
    }
    match std::fs::rename(old, &migrating) {
        Ok(()) => Claim::Taken(migrating),
        Err(e) => Claim::Blocked(format!(
            "could not move {} aside as {}: {e}",
            old.display(),
            migrating.file_name().unwrap_or_default().to_string_lossy()
        )),
    }
}

/// Migration done: `*.migrating` becomes `*.migrated`. A failure here is
/// logged and harmless — a `*.migrating` file is never migrated again.
fn finish(source: &Path) {
    let done = source.with_extension("migrated");
    if let Err(e) = std::fs::rename(source, &done) {
        crate::klog::warn(
            "migrate_move",
            None,
            format!("{} → {}: {e}", source.display(), done.display()),
        );
    }
}

/// What a migration carried over, for the log line and the transcript.
struct Carried {
    asks: usize,
    notes: usize,
    subs: usize,
    inbox: usize,
    dropped: usize,
    /// Records found already in place and not written again (a resumed
    /// migration, qal-j13).
    already: usize,
    nodes: usize,
}

impl Carried {
    fn line(&self, agent: &str) -> String {
        format!(
            "{agent}: {} node(s) → {} notes line(s), {} subscription(s), {} inbox file(s), {} parked question(s); {} closed node(s) dropped{}",
            self.nodes,
            self.notes,
            self.subs,
            self.inbox,
            self.asks,
            self.dropped,
            if self.already > 0 {
                format!(
                    "; {} already carried over by an earlier, cut migration",
                    self.already
                )
            } else {
                String::new()
            }
        )
    }

    /// The person's sentence: what is now running or waiting from the old
    /// plan.
    fn said(&self) -> String {
        let mut parts = Vec::new();
        if self.subs > 0 {
            parts.push(format!("{} standing subscription(s)", self.subs));
        }
        if self.inbox > 0 {
            parts.push(format!("{} pending task(s)", self.inbox));
        }
        if self.asks > 0 {
            parts.push(format!("{} open question(s)", self.asks));
        }
        if self.notes > 0 {
            parts.push(format!("{} checklist line(s)", self.notes));
        }
        if parts.is_empty() {
            "nothing open".to_string()
        } else {
            parts.join(", ")
        }
    }
}

/// Migrate the old plan at `path`. With `resume`, records an earlier
/// (cut) migration already wrote are recognised and not written again:
/// a subscription with the same kind, period and command (or prompt); an
/// inbox file with the same sender and body; a parked question with the
/// same id; a checklist line with the same text (`Notes::add` de-dups on
/// its own).
fn migrate_plan(
    hooks: &crate::hooks::KernelHooks,
    agent: &str,
    path: &Path,
    resume: bool,
) -> Option<Carried> {
    let place = &hooks.place;
    let nodes = load_old(path);
    let mut c = Carried {
        asks: 0,
        notes: 0,
        subs: 0,
        inbox: 0,
        dropped: 0,
        already: 0,
        nodes: nodes.len(),
    };
    let have_subs = if resume {
        subscription::list(place, agent)
    } else {
        Vec::new()
    };
    let have_inbox = if resume {
        inbox::list(place, agent)
    } else {
        Vec::new()
    };
    let have_asks: Vec<String> = if resume {
        arbos_core::waiting::asks(place, agent)
            .into_iter()
            .map(|w| w.id)
            .collect()
    } else {
        Vec::new()
    };
    let mut notes_file = notes::load(place, agent);
    let now = arbos_core::now_ms();
    for n in &nodes {
        let open = matches!(n.status.as_str(), "pending" | "active" | "blocked");
        if !open {
            c.dropped += 1;
            continue;
        }
        // An open question for the user parks as a waiting file with its
        // ask line on the transcript, so the card appears and the answer
        // arrives as a message (#106). A checklist line would never be
        // put to anyone (qa-028).
        if matches!(n.do_, OldDo::Ask) {
            let call_id = format!("migrated-{}", n.id);
            if have_asks.contains(&call_id) {
                c.already += 1;
                continue;
            }
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
                    c.asks += 1;
                }
                Err(e) => {
                    crate::klog::warn("migrate_ask", Some(agent), format!("node #{}: {e:#}", n.id));
                    c.dropped += 1;
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
                author: None,

                branch: None,

                channel: None,

                thread: None,

                match_text: None,
                deliver_to: "agent".into(),
                notify: None,
                expires: None,
                paused: n.status == "blocked",
                continuity: false,
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
                c.dropped += 1;
                continue;
            }
            // The same standing subscription: kind and period, and the
            // command when there is one (its prompt is a label), else the
            // prompt (a timer's whole content).
            if have_subs.iter().any(|s| {
                s.kind == sub.kind
                    && s.every == sub.every
                    && if sub.cmd.is_some() {
                        s.cmd == sub.cmd
                    } else {
                        s.prompt == sub.prompt
                    }
            }) {
                c.already += 1;
                continue;
            }
            match subscription::add(place, agent, sub, None) {
                Ok(_) => c.subs += 1,
                Err(e) => {
                    crate::klog::warn(
                        "migrate_subscription",
                        Some(agent),
                        format!("node #{}: {e:#}", n.id),
                    );
                    c.dropped += 1;
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
            if have_inbox
                .iter()
                .any(|f| f.msg.from == msg.from && f.msg.body == msg.body)
            {
                c.already += 1;
                continue;
            }
            if inbox::deliver(place, agent, &msg).is_ok() {
                c.inbox += 1;
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
        if notes_file.add(&section, &text).replaced {
            c.already += 1;
        } else {
            c.notes += 1;
        }
    }
    if c.notes > 0
        && let Err(e) = notes::save(place, agent, &notes_file)
    {
        crate::klog::warn("migrate_notes", Some(agent), format!("{e:#}"));
    }
    Some(c)
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
            author: None,

            branch: None,

            channel: None,

            thread: None,

            match_text: None,
            deliver_to: "agent".into(),
            notify: None,
            expires: None,
            paused: false,
            continuity: false,
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
    let _ = path;
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

    /// qal-j12: the completion record comes first. A folder where the old
    /// plan cannot be moved aside migrates nothing (and says so); a second
    /// start of a completed migration writes nothing; a start cut between
    /// the move and the finish is not run again.
    #[cfg(unix)]
    #[test]
    fn a_migration_that_cannot_record_itself_writes_nothing_and_never_doubles() {
        use std::os::unix::fs::PermissionsExt;
        let p = place("claim");
        let agent_dir = p.agent_dir("root");
        let plan = arbos_core::Layout::new(&p, "root").plan_jsonl();
        let now = arbos_core::now_ms();
        let lines = format!(
            "{{\"id\":1,\"goal\":\"tick\",\"when\":{{\"every_ms\":30000,\"next_due_ms\":{}}},\"do\":{{\"kind\":\"shell\",\"cmd\":\"echo legacy-tick >> ticks.txt\",\"report\":\"tick: {{output}}\"}},\"status\":\"pending\"}}\n\
             {{\"id\":2,\"goal\":\"Reply with the single word MIGRATED.\",\"status\":\"pending\",\"origin\":\"user\",\"when\":{{\"wake\":true}}}}\n",
            now + 1000
        );
        std::fs::write(&plan, &lines).unwrap();
        // The subfolders the migration writes into exist and are writable;
        // the folder holding plan.jsonl is not (QA's injector).
        std::fs::create_dir_all(agent_dir.join("subscriptions")).unwrap();
        std::fs::create_dir_all(agent_dir.join("inbox")).unwrap();
        std::fs::write(agent_dir.join("transcript.jsonl"), "").unwrap();
        let count = |sub: &str| {
            std::fs::read_dir(agent_dir.join(sub))
                .map(|d| d.flatten().count())
                .unwrap_or(0)
        };
        std::fs::set_permissions(&agent_dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let h = hooks(&p);
        for _ in 0..2 {
            let report = run(&h);
            assert!(report.is_empty(), "nothing migrated: {report:?}");
        }
        std::fs::set_permissions(&agent_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(plan.exists(), "the old plan stays");
        assert_eq!(
            count("subscriptions"),
            0,
            "no cron written on an unrecorded migration"
        );
        assert_eq!(count("inbox"), 0);
        let transcript = std::fs::read_to_string(agent_dir.join("transcript.jsonl")).unwrap();
        assert!(
            transcript.contains("could not start") && transcript.contains("Nothing was migrated"),
            "{transcript}"
        );

        // Writable again: one migration, then a second start writes nothing.
        for _ in 0..2 {
            run(&h);
        }
        assert!(!plan.exists() && plan.with_extension("jsonl.migrated").exists());
        assert_eq!(count("subscriptions"), 1, "one cron, not two");
        assert_eq!(count("inbox"), 1, "one task, not two");

        // Cut mid-way: a `.migrating` file and no plan, with the cron
        // already written and the task not (its inbox file removed, as if
        // the start died between the two). Finished: the cron is not
        // written again, the task is, and the transcript says both.
        std::fs::rename(
            plan.with_extension("jsonl.migrated"),
            plan.with_extension("jsonl.migrating"),
        )
        .unwrap();
        for e in std::fs::read_dir(agent_dir.join("inbox"))
            .unwrap()
            .flatten()
        {
            std::fs::remove_file(e.path()).unwrap();
        }
        let before = std::fs::read_to_string(agent_dir.join("transcript.jsonl")).unwrap();
        let report = run(&h);
        assert_eq!(report.len(), 1, "{report:?}");
        assert!(report[0].contains("already carried over"), "{report:?}");
        assert_eq!(
            count("subscriptions"),
            1,
            "the cron was recognised, not doubled"
        );
        assert_eq!(count("inbox"), 1, "the task was carried over this time");
        assert!(
            !plan.with_extension("jsonl.migrating").exists()
                && plan.with_extension("jsonl.migrated").exists(),
            "finished"
        );
        let after = std::fs::read_to_string(agent_dir.join("transcript.jsonl")).unwrap();
        let said = &after[before.len()..];
        assert!(
            said.contains("cut before it finished")
                && said.contains("1 pending task(s) carried over this time")
                && said.contains("1 were already in place"),
            "{said}"
        );
    }
}
