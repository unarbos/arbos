#[path = "../../../../../crates/arbos-mac/src/sync/work_lifetime.rs"]
mod work_lifetime;
#[path = "../../../../../crates/arbos-mac/src/sync/session_scope.rs"]
mod session_scope;
#[path = "../../../../../crates/arbos-mac/src/sync/account_tasks.rs"]
mod account_tasks;
use serde_json::{json, Value};
use std::io::{self, BufRead};
use work_lifetime::{MatrixLifetime, MatrixTicket, WorkLifetime};

fn observation(accepted: Vec<bool>, issued: Option<usize>, current: bool, other: Option<bool>) -> Value {
    let mut out = json!({"accepted": accepted, "issued": issued, "hasCurrent": current});
    if let Some(other) = other { out["other"] = json!(other); }
    out
}

fn run(value: &Value) -> Option<Value> {
    let id = value["id"].as_u64()?;
    let actions = value["actions"].as_array()?;
    if value["machine"] == "ui" { return observe_ui(id, actions); }
    if value["machine"] == "tasks" {
        return tokio::runtime::Builder::new_current_thread().enable_time().build().ok()?.block_on(observe_tasks(id, actions));
    }
    let mut steps = Vec::new();
    match value["machine"].as_str()? {
        "work" => {
            let mut owner = WorkLifetime::default();
            let other = WorkLifetime::default().ticket()?;
            let mut tickets = vec![owner.ticket()?];
            let observe = |owner: &WorkLifetime, tickets: &Vec<_>, issued| observation(
                tickets.iter().map(|ticket| owner.accepts(*ticket)).collect(), issued,
                owner.ticket().is_some(), Some(owner.accepts(other)));
            let initial = observe(&owner, &tickets, None);
            for action in actions {
                let mut issued = None;
                match action["kind"].as_str()? {
                    "close" => owner.close(),
                    "reopen" => { tickets.push(owner.reopen()); issued = Some(tickets.len() - 1); },
                    "capture" => if let Some(ticket) = owner.ticket() { tickets.push(ticket); issued = Some(tickets.len() - 1); },
                    _ => return None,
                }
                steps.push(observe(&owner, &tickets, issued));
            }
            Some(json!({"id": id, "initial": initial, "steps": steps}))
        }
        "matrix" => {
            let account = WorkLifetime::default().ticket()?;
            let mut owner = MatrixLifetime::default();
            let mut tickets: Vec<MatrixTicket> = Vec::new();
            let observe = |owner: &MatrixLifetime, tickets: &Vec<MatrixTicket>, issued| {
                let accepted: Vec<_> = tickets.iter().map(|ticket| owner.accepts(*ticket)).collect();
                let current = accepted.contains(&true);
                observation(accepted, issued, current, None)
            };
            let initial = observe(&owner, &tickets, None);
            for action in actions {
                let mut issued = None;
                match action["kind"].as_str()? {
                    "start" => if let Some(ticket) = owner.start(account) { tickets.push(ticket); issued = Some(tickets.len() - 1); },
                    "stop" => owner.stop(),
                    "ready" | "failed" => {
                        let index = usize::try_from(action["index"].as_u64()?).ok()?;
                        let ticket = *tickets.get(index)?;
                        if action["kind"] == "ready" { owner.ready(ticket); } else { owner.failed(ticket); }
                    },
                    _ => return None,
                }
                steps.push(observe(&owner, &tickets, issued));
            }
            Some(json!({"id": id, "initial": initial, "steps": steps}))
        }
        _ => None,
    }
}

fn main() {
    for line in io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let out = serde_json::from_str::<Value>(&line).ok().and_then(|input| run(&input))
            .unwrap_or_else(|| json!({"id":null,"error":"Invalid trace"}));
        println!("{out}");
    }
}

fn observe_ui(id: u64, actions: &[Value]) -> Option<Value> {
    let mut ui = session_scope::UiSession::default();
    let original = ui.control.ticket()?;
    let foreign = WorkLifetime::default().ticket()?;
    let a = WorkLifetime::default().ticket()?;
    let b = WorkLifetime::default().ticket()?;
    let observe = |ui: &session_scope::UiSession, result: Option<bool>| {
        let current = ui.control.ticket();
        observation(vec![ui.accepts_account(a), ui.accepts_account(b),
            ui.accepts_account_work(current, Some(a)), ui.accepts_account_work(current, Some(b))],
            None, ui.control.admit(original).is_some(), result)
    };
    let initial = observe(&ui, None);
    let mut steps = Vec::new();
    for action in actions {
        let result = match action["kind"].as_str()? {
            "adopt_a" => Some(ui.adopt(ui.control.ticket()?, a)),
            "adopt_b" => Some(ui.adopt(ui.control.ticket()?, b)),
            "adopt_old" => Some(ui.adopt(original, a)),
            "adopt_foreign" => Some(ui.adopt(foreign, a)),
            "renew" => { ui.begin_control()?; None },
            "clear" => { ui.invalidate()?; None },
            "unbind" => { ui.clear_account(); None },
            _ => return None,
        };
        steps.push(observe(&ui, result));
    }
    Some(json!({"id": id, "initial": initial, "steps": steps}))
}

async fn observe_tasks(id: u64, actions: &[Value]) -> Option<Value> {
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
    let owner = account_tasks::TaskOwner::default();
    let ran = Arc::new(AtomicUsize::new(0));
    let mut steps = Vec::new();
    for action in actions {
        match action["kind"].as_str()? {
            "capture" => {
                let before = ran.load(Ordering::SeqCst);
                let count = ran.clone();
                let began = Arc::new(tokio::sync::Notify::new());
                let signal = began.clone();
                let accepted = owner.spawn(async move { count.fetch_add(1, Ordering::SeqCst); signal.notify_one(); }).is_some();
                if accepted { began.notified().await; }
                steps.push(observation(vec![accepted, ran.load(Ordering::SeqCst) > before], None, accepted, None));
            }
            "close" => {
                let mut tasks = owner.retire();
                let count = tasks.len();
                while tasks.join_next().await.is_some() {}
                steps.push(observation(Vec::new(), Some(count), false, None));
            }
            _ => return None,
        }
    }
    Some(json!({"id": id, "steps": steps}))
}
