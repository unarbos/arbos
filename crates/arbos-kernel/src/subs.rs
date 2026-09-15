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

/// The command of the weekly `git gc` of the `.arbos/` repository.
pub const GC_CMD: &str = "git -C .arbos gc --auto --quiet";

/// Root's standing chores, added once: a weekly `git gc` of `.arbos/`
/// (one commit per turn adds up; design: "a weekly gc node"). Quiet on
/// success; a failure wakes root with the log tail like any shell job.
pub fn ensure_chores(place: &arbos_core::Place) {
    let root = arbos_core::ROOT_ID;
    if !arbos_core::agent_exists(place, root) {
        return;
    }
    if let Some(existing) = subscription::list(place, root)
        .into_iter()
        .find(|s| s.cmd.as_deref() == Some(GC_CMD))
    {
        // A chore written by an older kernel showed in every list.
        if !existing.internal {
            let mut fixed = existing;
            fixed.internal = true;
            let _ = subscription::save(place, root, &fixed);
        }
        return;
    }
    let sub = Subscription {
        id: 0,
        kind: "shell".into(),
        prompt: "Weekly git gc of the .arbos repository (kernel chore).".into(),
        every: Some("7d".into()),
        at: None,
        once: false,
        cmd: Some(GC_CMD.into()),
        path: None,
        repo: None,
        pr: None,

        branch: None,

        channel: None,

        thread: None,

        match_text: None,
        deliver_to: "none".into(),
        notify: None,
        expires: None,
        paused: false,
        continuity: false,
        internal: true,
        created: String::new(),
        next_due: None,
        last_fired: None,
        last: String::new(),
        error: None,
        seen: None,
    };
    if let Err(e) = subscription::add(place, root, sub, None) {
        crate::klog::warn("gc_chore", Some(root), format!("{e:#}"));
    }
}

/// Unreadable subscription files already reported, by agent, name, and reason.
fn reported() -> &'static Mutex<HashSet<String>> {
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
        let (subs, errors) = subscription::list_with_errors(&hooks.place, id);
        for (name, why) in errors {
            // Said once per file and reason, so a hand-written file that
            // does not read is seen and does not flood the log.
            let key = format!("{id}/{name}: {why}");
            if reported().lock().unwrap().insert(key) {
                crate::klog::warn(
                    "subscription_unreadable",
                    Some(id),
                    format!("subscriptions/{name}: {why}"),
                );
                hooks.broadcast(arbos_core::wire::Frame::Error {
                    agent: Some(id.to_string()),
                    detail: format!(
                        "subscriptions/{name} does not read and is not scheduled: {why}"
                    ),
                });
            }
        }
        for sub in subs {
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
            // Overdue by whole periods (the kernel was down, the agent was
            // paused): one firing with a note, or none — never one per
            // missed period.
            let missed = sub.missed_periods(now);
            if missed > 0 {
                match hooks.caps.catch_up {
                    crate::hooks::CatchUp::Skip => {
                        crate::klog::info(
                            "subscription_catch_up_skipped",
                            Some(id),
                            format!("#{} missed {missed}; next in one period", sub.id),
                        );
                        settle(
                            hooks,
                            id,
                            sub,
                            now,
                            format!("skipped {missed} missed firing(s)"),
                        );
                        continue;
                    }
                    crate::hooks::CatchUp::Once => {
                        crate::klog::info(
                            "subscription_catch_up_once",
                            Some(id),
                            format!("#{} missed {missed}; firing once", sub.id),
                        );
                        let note = format!(
                            "(missed {missed} earlier firing(s) while the kernel was down or the agent was paused; this is the one catch-up)"
                        );
                        fire_with_note(hooks, &agent, sub, now, Some(note));
                        continue;
                    }
                }
            }
            fire(hooks, &agent, sub, now);
        }
    }
}

/// On resume: every overdue or paused-through subscription of `agent` is
/// due one period from `now`. Returns how many were moved.
pub fn resume_subscriptions(place: &arbos_core::Place, agent: &str, now: i64) -> usize {
    let mut moved = 0;
    for mut sub in subscription::list(place, agent) {
        if sub.next_due_ms().is_some_and(|d| d <= now) {
            sub.resume_at(now);
            if subscription::save(place, agent, &sub).is_ok() {
                moved += 1;
            }
        }
    }
    moved
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

/// A human message landed in a door's channel: every `chat` subscription
/// that names that channel (and thread, and text) fires now, as a wake
/// from `subscription:N` carrying who said what where. Event-driven: no
/// schedule to advance, the file only remembers the last firing.
pub fn fire_chat(
    hooks: &Arc<KernelHooks>,
    channel_tag: &str,
    thread: Option<&str>,
    author: &str,
    text: &str,
) {
    let now = arbos_core::now_ms();
    for agent in list_agents(&hooks.place).unwrap_or_default() {
        let id = agent.id.as_str();
        for mut sub in subscription::list(&hooks.place, id) {
            if !sub.matches_chat(channel_tag, thread, text) {
                continue;
            }
            let where_ = match thread {
                Some(t) => format!("{channel_tag} (thread {t})"),
                None => channel_tag.to_string(),
            };
            let mut body = format!("[chat] {author} in {where_}: {text}");
            if !sub.prompt.trim().is_empty() {
                body = format!("{}\n\n{body}", sub.prompt.trim());
            }
            let outcome = match inbox::deliver(&hooks.place, id, &message(&sub, true, body)) {
                Ok(_) => format!("{author}: {}", text::clip(text, 80)),
                Err(e) => format!("could not deliver: {e:#}"),
            };
            sub.last_fired = Some(inbox::rfc3339(now));
            sub.last = text::clip(&outcome, 200);
            if let Err(e) = subscription::save(&hooks.place, id, &sub) {
                crate::klog::warn(
                    "subscription_save_failed",
                    Some(id),
                    format!("#{}: {e:#}", sub.id),
                );
            }
            hooks.plan_changed(id);
            crate::klog::info(
                "chat_subscription_fired",
                Some(id),
                format!("#{} {channel_tag}", sub.id),
            );
        }
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
    fire_with_note(hooks, agent, sub, now, None);
}

/// `note`, when given, rides at the end of whatever message this firing
/// delivers (the catch-up remark). It is never written into the file.
fn fire_with_note(
    hooks: &Arc<KernelHooks>,
    agent: &Agent,
    sub: Subscription,
    now: i64,
    note: Option<String>,
) {
    let id = agent.id.as_str();
    let noted = move |body: String| match &note {
        Some(n) => format!("{body}\n{n}"),
        None => body,
    };
    match sub.kind.as_str() {
        "timer" => {
            // With continuity, the last words of the turn the previous
            // firing opened ride along (close_turn_folder stores them).
            let body = match (&sub.continuity, sub.seen.as_deref()) {
                (true, Some(prev)) if !prev.trim().is_empty() => format!(
                    "{}\n\nLast time this fired, your turn ended with:\n{}",
                    sub.prompt,
                    text::clip(prev, CONTINUITY_CAP)
                ),
                _ => sub.prompt.clone(),
            };
            let outcome = match inbox::deliver(&hooks.place, id, &message(&sub, true, noted(body)))
            {
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
                match inbox::deliver(&hooks.place, id, &message(&sub, true, noted(body))) {
                    Ok(_) => format!("{} new file(s)", new.len()),
                    Err(e) => format!("could not deliver: {e:#}"),
                }
            } else {
                "no change".to_string()
            };
            settle(hooks, id, sub, now, outcome);
        }
        // A goal: run its check (when it has one); exit 0 closes the goal
        // and tells the agent and the user; anything else wakes the agent
        // with the goal, the check's output, and what changed since last
        // time. Without a check, the agent is woken each period until it
        // removes the goal.
        "goal" => {
            let key = format!("{id}#{}", sub.id);
            {
                let mut set = in_flight().lock().unwrap();
                if set.len() >= MAX_SHELL || set.contains(&key) {
                    return;
                }
                set.insert(key.clone());
            }
            let mut scheduled = sub.clone();
            scheduled.schedule_next(now);
            if scheduled.next_due.is_some() {
                let _ = subscription::save(&hooks.place, id, &scheduled);
            }
            let hooks = Arc::clone(hooks);
            let agent = agent.clone();
            tokio::spawn(async move {
                let id = agent.id.as_str();
                let goal = sub.prompt.trim().to_string();
                let (met, detail) =
                    match sub.cmd.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
                        Some(cmd) => {
                            let (_job, code, tail) = run_job(&hooks, &agent, cmd).await;
                            let tail = tail.trim().to_string();
                            (
                                code == 0,
                                format!(
                                    "The check `{cmd}` exited {code}.{}",
                                    if tail.is_empty() {
                                        String::new()
                                    } else {
                                        format!(" Output tail:\n{tail}")
                                    }
                                ),
                            )
                        }
                        None => (
                            false,
                            "No check is set: you decide when it is met.".to_string(),
                        ),
                    };
                let previous = sub.seen.clone().unwrap_or_default();
                let outcome = if met {
                    let body = noted(format!(
                        "Goal met: {goal}\n{detail}\nGoal #{} is closed. Say so to the user in one line, with what made it true.",
                        sub.id
                    ));
                    let _ = inbox::deliver(&hooks.place, id, &message(&sub, true, body));
                    let _ = hooks.notify_user(id, &format!("goal met: {goal}"));
                    crate::klog::info(
                        "goal_met",
                        Some(id),
                        format!("#{} {}", sub.id, text::clip(&goal, 80)),
                    );
                    format!("met — {}", text::clip(&detail, 120))
                } else {
                    let since = if previous.is_empty() || previous == detail {
                        String::new()
                    } else {
                        format!(
                            "\nLast time the check said:\n{}",
                            text::clip(&previous, 1200)
                        )
                    };
                    let body = noted(format!(
                        "Goal not yet met: {goal}\n{detail}{since}\nWork toward it now. When you believe it is met, end your turn: the check runs again in {} (subscribe remove {} closes the goal without it{}).",
                        subscription::human_ms(
                            sub.every_ms()
                                .unwrap_or(subscription::GOAL_DEFAULT_EVERY_MS)
                        ),
                        sub.id,
                        if sub.cmd.is_none() {
                            "; that is how a goal with no check is closed"
                        } else {
                            ""
                        }
                    ));
                    let _ = inbox::deliver(&hooks.place, id, &message(&sub, true, body));
                    format!("not met — {}", text::clip(&detail, 120))
                };
                if let Some(current) = subscription::get(&hooks.place, id, sub.id) {
                    let mut current = current;
                    current.last = text::clip(&outcome, 200);
                    current.last_fired = Some(inbox::rfc3339(arbos_core::now_ms()));
                    current.seen = Some(text::clip(&detail, 4000));
                    if met || current.once {
                        let _ = subscription::remove(&hooks.place, id, sub.id);
                    } else {
                        let _ = subscription::save(&hooks.place, id, &current);
                    }
                }
                in_flight().lock().unwrap().remove(&key);
                hooks.plan_changed(id);
                hooks.kick();
            });
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
                // What the command printed last time, for a message or a
                // notice that compares instead of starting over.
                let previous = sub
                    .continuity
                    .then(|| sub.seen.clone())
                    .flatten()
                    .filter(|p| !p.trim().is_empty());
                let last_time = previous
                    .as_deref()
                    .map(|p| {
                        format!(
                            "\n\nLast time it printed:\n{}",
                            text::clip(p, CONTINUITY_CAP)
                        )
                    })
                    .unwrap_or_default();
                // A reading with nothing to read is a failure too (`curl |
                // jq` with a dead endpoint exits 0 on some shells).
                let silent = to_user && tail.trim().is_empty();
                let ok = code == 0 && !silent;
                let outcome = if ok {
                    if sub.deliver_to == "none" {
                        format!(
                            "exit 0 — {}",
                            if tail.is_empty() {
                                "quiet".to_string()
                            } else {
                                text::clip(&tail, 120)
                            }
                        )
                    } else if to_user {
                        let template = sub.notify.clone().unwrap_or_else(|| "{output}".into());
                        let line = template.replace("{output}", tail.trim()).replace(
                            "{previous}",
                            previous
                                .as_deref()
                                .map(str::trim)
                                .unwrap_or("(nothing yet)"),
                        );
                        match hooks.notify_user(id, &line) {
                            Ok(()) => format!("exit 0 — told the user: {}", text::clip(&line, 120)),
                            Err(e) => format!("exit 0 — could not tell the user: {e:#}"),
                        }
                    } else {
                        let body = format!(
                            "{}\n\nSubscription #{} ran `{}` (exit 0). Output:\n{}{last_time}",
                            sub.prompt,
                            sub.id,
                            cmd,
                            if tail.is_empty() { "(none)" } else { &tail }
                        );
                        match inbox::deliver(&hooks.place, id, &message(&sub, true, noted(body))) {
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
                        "Subscription #{} (`{}`) failed: {why}. Output tail:\n{}{last_time}\nDiagnose and act: fix the cause, change the command (subscribe remove {} and add a new one), or remove it and say so.",
                        sub.id,
                        cmd,
                        if tail.is_empty() {
                            "(no output captured)"
                        } else {
                            &tail
                        },
                        sub.id
                    );
                    let _ = inbox::deliver(&hooks.place, id, &message(&sub, true, noted(body)));
                    format!("{why} — {}", text::clip(&tail, 120))
                };
                // The file may have moved on (paused, removed) meanwhile.
                if let Some(current) = subscription::get(&hooks.place, id, sub.id) {
                    let mut current = current;
                    current.last = text::clip(&outcome, 200);
                    current.last_fired = Some(inbox::rfc3339(arbos_core::now_ms()));
                    if current.continuity {
                        current.seen = Some(text::clip(tail.trim(), CONTINUITY_CAP));
                    }
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
                    // A merged or closed pull request has nothing more to
                    // watch: the subscription goes with this firing.
                    if outcome.closed {
                        current.once = true;
                        crate::klog::info(
                            "subscription_closed",
                            Some(&agent_id),
                            format!("#{}: the pull request is {}", current.id, outcome.last),
                        );
                    }
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

/// Most of a previous output a continuity firing carries.
const CONTINUITY_CAP: usize = 4000;

struct Polled {
    seen: Option<String>,
    error: Option<String>,
    last: String,
    /// The pull request is merged or closed: stop watching.
    closed: bool,
}

/// One look at the pull request; the diff against `seen` is the message.
/// The first look only remembers. An error is said once.
fn poll_github(hooks: &KernelHooks, agent: &str, sub: &Subscription) -> Polled {
    let repo = sub.repo.clone().unwrap_or_default();
    let pr = sub.pr.unwrap_or(0);
    let branch = sub
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|b| !b.is_empty() && sub.pr.is_none());
    let subject = match branch {
        Some(b) => format!("{repo}@{b}"),
        None => format!("{repo}#{pr}"),
    };
    // `gh` runs with the grants this agent (or one above it) holds.
    let env = arbos_engine::secrets::store().env_for(&arbos_core::lineage(&hooks.place, agent));
    let looked = match branch {
        Some(b) => crate::github::branch_snapshot(&repo, b, &env),
        None => crate::github::snapshot(&repo, pr, &env),
    };
    match looked {
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
                        if branch.is_some() {
                            // Branch runs: checks and the new-commit line;
                            // the overall state line repeats the checks.
                            is_check || l.starts_with("new commits")
                        } else if sub.kind == "github_ci" {
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
                    "{subject} ({}): {}{}",
                    if branch.is_some() {
                        now.state.clone()
                    } else {
                        now.title.clone()
                    },
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
            let closed = branch.is_none()
                && matches!(now.state.to_ascii_uppercase().as_str(), "MERGED" | "CLOSED");
            Polled {
                seen: serde_json::to_string(&now).ok(),
                error: None,
                last: if closed {
                    now.state.to_ascii_lowercase()
                } else {
                    last
                },
                closed,
            }
        }
        Err(e) => {
            let msg = format!("{e:#}");
            if sub.error.as_deref() != Some(&msg) {
                let text = format!(
                    "{subject}: the subscription cannot be checked: {msg}. It stays until you remove it (subscribe remove {}).",
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
                closed: false,
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
    let granted = arbos_engine::secrets::store()
        .env_for(&arbos_core::lineage(&hooks.place, agent.id.as_str()));
    let (job, mut child) = match root.spawn(
        cmd,
        &cwd,
        Some(CMD_TIMEOUT.as_millis() as u64),
        None,
        granted,
    ) {
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
    // What leaves the journal for a message or a notice is redacted like
    // a tool result: a command that echoes a key does not put it in the
    // user's line or the agent's inbox.
    let out = arbos_engine::secrets::store().redact(&journal_tail(&job.journal(), 256 * 1024));
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
