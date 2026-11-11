//! Bounded std-only observations of the actual main-Pi resource slot; no child processes.
#[allow(dead_code)]
#[path = "../../crates/arbos-agentd/src/pi_slot.rs"]
mod pi_slot;
use pi_slot::{Action, BootTicket, Event, PiSlot};
use std::io::{self, BufRead, Write};

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
    let mut slot = PiSlot::<u32>::default();
    let mut tickets: Vec<BootTicket> = vec![];
    let mut steps = Vec::new();
    for input in inputs {
        let words: Vec<_> = input.split(':').collect();
        let ticket = |s: &str| -> Option<BootTicket> {
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
            ["b"] => Event::Begin,
            ["s"] => Event::Shutdown,
            ["a", owner, child] => {
                let child: u32 = child.parse().ok()?;
                if child > 100000 {
                    return None;
                }
                Event::Attach {
                    ticket: ticket(owner)?,
                    child,
                }
            }
            ["r", owner] => Event::Ready(ticket(owner)?),
            ["e", owner] => Event::Exited(ticket(owner)?),
            ["t", owner] => Event::Retire(ticket(owner)?),
            ["p", owner] => Event::Park(ticket(owner)?),
            ["x", owner] => Event::Abort(ticket(owner)?),
            _ => return None,
        };
        let (action, released) = match slot.transition(event) {
            Action::Started { ticket, retired } => {
                tickets.push(ticket);
                ("Started", retired)
            }
            Action::Accepted => ("Accepted", None),
            Action::Retired(child) => ("Retired", child),
            Action::Rejected(child) => ("Rejected", child),
        };
        let (tag, child) = match &slot {
            PiSlot::Vacant { .. } => ("Vacant", None),
            PiSlot::Starting { .. } => ("Starting", None),
            PiSlot::Checking { child, .. } => ("Checking", Some(*child)),
            PiSlot::Ready { child, .. } => ("Ready", Some(*child)),
            PiSlot::Exited { child, .. } => ("Exited", Some(*child)),
            PiSlot::Parked { .. } => ("Parked", None),
            PiSlot::Stopped { .. } => ("Stopped", None),
        };
        let owner = slot
            .ticket()
            .and_then(|t| tickets.iter().position(|saved| *saved == t))
            .map(|n| n + 1);
        let optional = |n: Option<u32>| n.map(|n| n.to_string()).unwrap_or_else(|| "null".into());
        steps.push(format!("{{\"tag\":\"{tag}\",\"child\":{},\"owner\":{},\"action\":\"{action}\",\"released\":{}}}",optional(child),optional(owner.map(|n|n as u32)),optional(released)));
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
