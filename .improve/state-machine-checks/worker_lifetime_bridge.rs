//! Bounded observations of the actual pure RPC worker registry; no children or external effects.
#[allow(dead_code)]
#[path = "../../crates/arbos-agentd/src/worker_lifetime.rs"]
mod worker_lifetime;
use std::io::{self, BufRead, Write};
use worker_lifetime::{Action, Entry, Event, State, Ticket};

fn snapshot(entry: Option<&Entry<u32>>, tickets: &[Ticket]) -> String {
    let (tag, ticket, resource) = match entry {
        None => ("Missing", None, None),
        Some(Entry::Starting { ticket, resource }) => ("Starting", Some(ticket), *resource),
        Some(Entry::Ready { ticket, resource }) => ("Ready", Some(ticket), Some(*resource)),
        Some(Entry::Stopping { ticket, resource }) => ("Stopping", Some(ticket), *resource),
    };
    let owner = ticket
        .and_then(|t| tickets.iter().position(|saved| saved == t))
        .map(|n| n + 1);
    format!(
        "{{\"tag\":\"{tag}\",\"owner\":{},\"resource\":{}}}",
        owner.map_or("null".into(), |n| n.to_string()),
        resource.map_or("null".into(), |n| n.to_string())
    )
}
fn reply(raw: &[u8]) -> Option<String> {
    let raw = std::str::from_utf8(raw)
        .ok()?
        .trim_end_matches(['\r', '\n']);
    let (id, events) = raw.split_once('|')?;
    let id: u32 = id.parse().ok()?;
    let inputs: Vec<_> = if events.is_empty() {
        vec![]
    } else {
        events.split(',').collect()
    };
    if inputs.len() > 64 {
        return None;
    }
    let mut state = State::<u32>::default();
    let mut tickets = Vec::<Ticket>::new();
    let mut steps = Vec::new();
    for input in inputs {
        let words: Vec<_> = input.split(':').collect();
        let key = |s: &str| {
            if matches!(s, "a" | "b") {
                Some(s.to_owned())
            } else {
                None
            }
        };
        let ticket = |s: &str| -> Option<Ticket> {
            let n: usize = s.parse().ok()?;
            tickets
                .get(if n == 0 {
                    tickets.len().checked_sub(1)?
                } else {
                    n.checked_sub(1)?
                })
                .copied()
        };
        let event = match words.as_slice() {
            ["b", id] => Event::Begin(key(id)?),
            ["c", id, owner, resource] => {
                let resource: u32 = resource.parse().ok()?;
                if resource > 100000 {
                    return None;
                }
                Event::Capture {
                    id: key(id)?,
                    ticket: ticket(owner)?,
                    resource,
                }
            }
            ["p", id, owner] => Event::Publish {
                id: key(id)?,
                ticket: ticket(owner)?,
            },
            ["x", id, owner] => Event::Abort {
                id: key(id)?,
                ticket: ticket(owner)?,
            },
            ["t", id] => Event::Stop(key(id)?),
            ["d", id, owner] => Event::Stopped {
                id: key(id)?,
                ticket: ticket(owner)?,
            },
            ["s"] => Event::Shutdown,
            _ => return None,
        };
        let (action, returned) = match state.transition(event) {
            Action::Begun { ticket, retired } => {
                tickets.push(ticket);
                ("Begun", retired.into_iter().collect::<Vec<_>>())
            }
            Action::Stopping { resource, .. } => ("Stopping", resource.into_iter().collect()),
            Action::Retired(resource) => ("Retired", resource.into_iter().collect()),
            Action::Accepted => ("Accepted", vec![]),
            Action::Missing => ("Missing", vec![]),
            Action::Wait => ("Wait", vec![]),
            Action::Closed(resources) => ("Closed", resources),
            Action::Rejected(resource) => ("Rejected", resource.into_iter().collect()),
        };
        let tag = if matches!(&state, State::Closed) {
            "Closed"
        } else {
            "Accepting"
        };
        let returned = returned
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        steps.push(format!("{{\"tag\":\"{tag}\",\"a\":{},\"b\":{},\"action\":\"{action}\",\"returned\":[{returned}]}}",snapshot(state.entry("a"),&tickets),snapshot(state.entry("b"),&tickets)));
    }
    Some(format!("{{\"id\":{id},\"steps\":[{}]}}", steps.join(",")))
}

fn main() -> io::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut line = Vec::new();
    loop {
        line.clear();
        let mut oversized = false;
        loop {
            let bytes = input.fill_buf()?;
            if bytes.is_empty() {
                break;
            }
            let used = bytes
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len(), |index| index + 1);
            let ended = bytes[used - 1] == b'\n';
            if line.len() + used <= 8192 && !oversized {
                line.extend_from_slice(&bytes[..used]);
            } else {
                oversized = true;
            }
            input.consume(used);
            if ended {
                break;
            }
        }
        if line.is_empty() && !oversized {
            break;
        }
        let response = if oversized { None } else { reply(&line) };
        writeln!(
            output,
            "{}",
            response
                .as_deref()
                .unwrap_or("{\"id\":null,\"error\":\"Invalid trace\"}")
        )?;
        output.flush()?;
    }
    Ok(())
}
