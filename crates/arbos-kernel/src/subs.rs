//! The one watcher. Each tick looks at every agent's `subscriptions/` and
//! fires what is due: a `timer` writes its prompt to the agent's inbox; a
//! `shell` runs its command as a job and delivers the reading to the user
//! or the output to the agent (a failure always wakes the agent); the
//! GitHub kinds poll a pull request and deliver the diff; `inbox` watches a
//! folder for new files. Every firing is one inbox file (or one line to
//! the user); the scan that follows claims it like any other message.

use std::{
    collections::HashSet,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use arbos_core::{
    Agent, inbox, list_agents,
    subscription::{self, Subscription},
    text,
};
use arbos_engine::JobsRoot;

use crate::hooks::KernelHooks;

/// Most shell runs at once across the place.
const MAX_SHELL: usize = 8;
/// One kernel-run command may take this long.
const CMD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// `agent#id` of shell runs and GitHub polls in flight.
fn in_flight() -> &'static Mutex<HashSet<String>> {
    static SET: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Anything of the watcher's still running (for `--until-idle`).
pub fn busy() -> bool {
    !in_flight().lock().unwrap().is_empty()
}

/// One pass over every agent's subscriptions at `now`.
pub fn tick(hooks: &Arc<KernelHooks>, now: i64) {
    for agent in list_agents(&hooks.place).unwrap_or_default() {
        if agent.paused {
            continue;
        }
        let id = agent.id.as_str();
        for sub in subscription::list(&hooks.place, id) {
            if sub.expired(now) {
                let _ = subscription::remove(&hooks.place, id, sub.id);
                let _ = inbox::deliver(
                    &hooks.place,
                    id,
                    &message(
                        &sub,
                        false,
                        format!(
                            "Subscription #{} ({}) expired and was removed.",
                            sub.id,
                            sub.label()
                        ),
                    ),
                );
                hooks.plan_changed(id);
                continue;
            }
            if !sub.is_due(now) {
                continue;
            }
            fire(hooks, &agent, sub, now);
        }
    }
}

/// Fire `sub` now from a window action, whatever its schedule says.
pub fn fire_now(hooks: &Arc<KernelHooks>, agent: &str, id: u32) -> anyhow::Result<()> {
    let agent = arbos_core::load_agent(&hooks.place, &arbos_core::AgentId::new(agent))?;
    let sub = subscription::get(&hooks.place, agent.id.as_str(), id)
        .ok_or_else(|| anyhow::anyhow!("no subscription #{id}"))?;
    fire(hooks, &agent, sub, arbos_core::now_ms());
    Ok(())
}

fn message(sub: &Subscription, wake: bool, body: String) -> inbox::Message {
    inbox::Message {
        from: format!("subscription:{}", sub.id),
        kind: if wake {
            "wake".into()
        } else {
            "message".into()
        },
        wake,
        hops: 0,
        body,
        ..inbox::Message::default()
    }
}

/// Advance the schedule (or drop a one-shot), remember `last`, save.
fn settle(hooks: &KernelHooks, agent: &str, mut sub: Subscription, now: i64, last: String) {
    sub.schedule_next(now);
    sub.last = text::clip(&last, 200);
    if sub.next_due.is_none() {
        let _ = subscription::remove(&hooks.place, agent, sub.id);
    } else if let Err(e) = subscription::save(&hooks.place, agent, &sub) {
        crate::klog::warn(
            "subscription_save_failed",
            Some(agent),
            format!("#{}: {e:#}", sub.id),
        );
    }
    hooks.plan_changed(agent);
}

fn fire(hooks: &Arc<KernelHooks>, agent: &Agent, sub: Subscription, now: i64) {
    let id = agent.id.as_str();
    match sub.kind.as_str() {
        "timer" => {
            let body = sub.prompt.clone();
            let outcome = match inbox::deliver(&hooks.place, id, &message(&sub, true, body)) {
                Ok(_) => "fired".to_string(),
                Err(e) => format!("could not deliver: {e:#}"),
            };
            settle(hooks, id, sub, now, outcome);
        }
        "inbox" => {
            let path = std::path::PathBuf::from(sub.path.clone().unwrap_or_default());
            let path = if path.is_absolute() {
                path
            } else {
                hooks.place.path.join(path)
            };
            let seen: HashSet<String> = sub
                .seen
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            let now_names: Vec<String> = std::fs::read_dir(&path)
                .map(|rd| {
                    let mut v: Vec<String> = rd
                        .flatten()
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .filter(|n| !n.starts_with('.'))
                        .collect();
                    v.sort();
                    v
                })
                .unwrap_or_default();
            let new: Vec<&String> = now_names.iter().filter(|n| !seen.contains(*n)).collect();
            let mut sub = sub;
            let first_look = sub.seen.is_none();
            sub.seen = Some(serde_json::to_string(&now_names).unwrap_or_default());
            let outcome = if !new.is_empty() && !first_look {
                let list: Vec<String> = new
                    .iter()
                    .map(|n| format!("{}/{n}", path.display()))
                    .collect();
                let body = format!(
                    "{}\n\nNew in {}:\n{}",
                    sub.prompt,
                    path.display(),
                    list.join("\n")
                );
                match inbox::deliver(&hooks.place, id, &message(&sub, true, body)) {
                    Ok(_) => format!("{} new file(s)", new.len()),
                    Err(e) => format!("could not deliver: {e:#}"),
                }
            } else {
                "no change".to_string()
            };
            settle(hooks, id, sub, now, outcome);
        }
        "shell" => {
            let key = format!("{id}#{}", sub.id);
            {
                let mut set = in_flight().lock().unwrap();
                if set.len() >= MAX_SHELL || set.contains(&key) {
                    return;
                }
                set.insert(key.clone());
            }
            // Rescheduled before it runs, so a slow command is not fired
            // twice; the outcome lands when it ends.
            let mut scheduled = sub.clone();
            scheduled.schedule_next(now);
            if scheduled.next_due.is_some() {
                let _ = subscription::save(&hooks.place, id, &scheduled);
            }
            let hooks = Arc::clone(hooks);
            let agent = agent.clone();
            tokio::spawn(async move {
                let cmd = sub.cmd.clone().unwrap_or_default();
                let (job, code, tail) = run_job(&hooks, &agent, &cmd).await;
                let id = agent.id.as_str();
                let to_user = sub.deliver_to == "user";
                // A reading with nothing to read is a failure too (`curl |
                // jq` with a dead endpoint exits 0 on some shells).
                let silent = to_user && tail.trim().is_empty();
                let ok = code == 0 && !silent;
                let outcome = if ok {
                    if to_user {
                        let template = sub.notify.clone().unwrap_or_else(|| "{output}".into());
                        let line = template.replace("{output}", tail.trim());
                        match hooks.notify_user(id, &line) {
                            Ok(()) => format!("exit 0 — told the user: {}", text::clip(&line, 120)),
                            Err(e) => format!("exit 0 — could not tell the user: {e:#}"),
                        }
                    } else {
                        let body = format!(
                            "{}\n\nSubscription #{} ran `{}` (exit 0). Output:\n{}",
                            sub.prompt,
                            sub.id,
                            cmd,
                            if tail.is_empty() { "(none)" } else { &tail }
                        );
                        match inbox::deliver(&hooks.place, id, &message(&sub, true, body)) {
                            Ok(_) => format!("exit 0 — {}", text::clip(&tail, 120)),
                            Err(e) => format!("exit 0 — could not deliver: {e:#}"),
                        }
                    }
                } else {
                    let why = if silent && code == 0 {
                        "no output for the reading".to_string()
                    } else {
                        format!("exit {code}")
                    };
                    let body = format!(
                        "Subscription #{} (`{}`) failed: {why}. Output tail:\n{}\nDiagnose and act: fix the cause, change the command (subscribe remove {} and add a new one), or remove it and say so.",
                        sub.id,
                        cmd,
                        if tail.is_empty() {
                            "(no output captured)"
                        } else {
                            &tail
                        },
                        sub.id
                    );
                    let _ = inbox::deliver(&hooks.place, id, &message(&sub, true, body));
                    format!("{why} — {}", text::clip(&tail, 120))
                };
                // The file may have moved on (paused, removed) meanwhile.
                if let Some(current) = subscription::get(&hooks.place, id, sub.id) {
                    let mut current = current;
                    current.last = text::clip(&outcome, 200);
                    current.last_fired = Some(inbox::rfc3339(arbos_core::now_ms()));
                    if current.once || current.every_ms().is_none() {
                        let _ = subscription::remove(&hooks.place, id, sub.id);
                    } else {
                        let _ = subscription::save(&hooks.place, id, &current);
                    }
                }
                in_flight().lock().unwrap().remove(&key);
                crate::snapshot::commit_later(
                    &hooks.place,
                    format!("{id} subscription #{}: {}", sub.id, text::clip(&cmd, 60)),
                );
                let _ = job;
                hooks.plan_changed(id);
                hooks.kick();
            });
        }
        "github_pr" | "github_ci" => {
            let key = format!("{id}#{}", sub.id);
            {
                let mut set = in_flight().lock().unwrap();
                if set.contains(&key) {
                    return;
                }
                set.insert(key.clone());
            }
            let hooks = Arc::clone(hooks);
            let agent_id = id.to_string();
            tokio::task::spawn_blocking(move || {
                let outcome = poll_github(&hooks, &agent_id, &sub);
                in_flight().lock().unwrap().remove(&key);
                if let Some(current) = subscription::get(&hooks.place, &agent_id, sub.id) {
                    let mut current = current;
                    current.seen = outcome.seen.or(current.seen);
                    current.error = outcome.error;
                    settle(
                        &hooks,
                        &agent_id,
                        current,
                        arbos_core::now_ms(),
                        outcome.last,
                    );
                }
                hooks.kick();
            });
        }
        other => {
            crate::klog::warn(
                "subscription_kind",
                Some(id),
                format!("#{}: unknown kind {other}", sub.id),
            );
            let mut sub = sub;
            sub.paused = true;
            let _ = subscription::save(&hooks.place, id, &sub);
        }
    }
}

struct Polled {
    seen: Option<String>,
    error: Option<String>,
    last: String,
}

/// One look at the pull request; the diff against `seen` is the message.
/// The first look only remembers. An error is said once.
fn poll_github(hooks: &KernelHooks, agent: &str, sub: &Subscription) -> Polled {
    let repo = sub.repo.clone().unwrap_or_default();
    let pr = sub.pr.unwrap_or(0);
    match crate::github::snapshot(&repo, pr) {
        Ok(now) => {
            let prev: Option<crate::github::Snapshot> = sub
                .seen
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok());
            let lines: Vec<String> = match &prev {
                Some(p) => crate::github::diff(p, &now)
                    .into_iter()
                    .filter(|l| {
                        let is_check = l.starts_with("check ");
                        if sub.kind == "github_ci" {
                            is_check
                        } else {
                            !is_check
                        }
                    })
                    .collect(),
                None => Vec::new(),
            };
            let last = if lines.is_empty() {
                "no change".to_string()
            } else {
                let text = format!(
                    "{repo}#{pr} ({}): {}{}",
                    now.title,
                    lines.join("; "),
                    if sub.prompt.is_empty() {
                        String::new()
                    } else {
                        format!(". You asked: {}", sub.prompt)
                    }
                );
                let mut msg = message(sub, true, text.clone());
                msg.from = "github".into();
                match inbox::deliver(&hooks.place, agent, &msg) {
                    Ok(_) => text::clip(&lines.join("; "), 200),
                    Err(e) => format!("could not deliver: {e:#}"),
                }
            };
            Polled {
                seen: serde_json::to_string(&now).ok(),
                error: None,
                last,
            }
        }
        Err(e) => {
            let msg = format!("{e:#}");
            if sub.error.as_deref() != Some(&msg) {
                let text = format!(
                    "{repo}#{pr}: the subscription cannot be checked: {msg}. It stays until you remove it (subscribe remove {}).",
                    sub.id
                );
                let mut m = message(sub, true, text);
                m.from = "github".into();
                let _ = inbox::deliver(&hooks.place, agent, &m);
            }
            Polled {
                seen: None,
                error: Some(msg.clone()),
                last: format!("error: {}", text::clip(&msg, 160)),
            }
        }
    }
}

/// Run `cmd` as a job of `agent` and wait for it (with a cap).
async fn run_job(hooks: &KernelHooks, agent: &Agent, cmd: &str) -> (Option<String>, i32, String) {
    let cwd = agent
        .cwd
        .clone()
        .unwrap_or_else(|| hooks.place.path.clone());
    let root = JobsRoot::for_agent(&hooks.place, &agent.id);
    let (job, mut child) = match root.spawn(cmd, &cwd, Some(CMD_TIMEOUT.as_millis() as u64), None) {
        Ok(x) => x,
        Err(e) => return (None, -1, format!("could not start: {e}")),
    };
    let id = job.id.clone();
    let timed_out = tokio::time::timeout(CMD_TIMEOUT, child.wait())
        .await
        .is_err();
    if timed_out && let Ok(j) = root.load(&id) {
        root.kill(&j);
    }
    let code = match root.load(&id) {
        Ok(j) => match j.status {
            arbos_engine::JobStatus::Exited(c) => c,
            _ => -1,
        },
        Err(_) => -1,
    };
    let out = journal_tail(&job.journal(), 256 * 1024);
    let mut tail = text::tail(&out);
    if timed_out {
        tail = format!("timed out after {}s\n{tail}", CMD_TIMEOUT.as_secs());
    }
    (Some(id), code, tail)
}

/// The last `max` bytes of a job's journal. Never the whole file.
fn journal_tail(path: &std::path::Path, max: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return String::new();
    };
    let size = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = size.saturating_sub(max);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut buf = Vec::with_capacity((size - start) as usize);
    let _ = f.take(size - start).read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}
