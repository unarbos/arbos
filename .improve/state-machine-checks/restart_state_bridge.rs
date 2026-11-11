//! Bounded std-only oracle for the actual pure restart reducer. No runtime/effect imports.
#[path = "../../crates/arbos-relay/src/restart_lifetime.rs"]
mod restart_lifetime;
use restart_lifetime::{transition, Event, Phase, State};
use std::io::{self, BufRead, Write};

fn snapshot(state: State) -> String {
    match state {
        State::Serving(generation) => {
            format!("{{\"tag\":\"Serving\",\"generation\":{generation}}}")
        }
        State::Planned { generation, phase } => {
            format!("{{\"tag\":\"Planned\",\"generation\":{generation},\"phase\":\"{phase:?}\"}}")
        }
        State::Shutdown => "{\"tag\":\"Shutdown\"}".into(),
    }
}

fn event(raw: &str) -> Option<Event> {
    let pieces: Vec<_> = raw.split(':').collect();
    match pieces.as_slice() {
        ["p"] => Some(Event::Plan),
        ["s"] => Some(Event::Shutdown),
        ["r", generation] => Some(Event::Release(generation.parse::<u32>().ok()?.into())),
        ["a", generation, phase] => Some(Event::Advance {
            generation: generation.parse::<u32>().ok()?.into(),
            phase: match *phase {
                "n" => Phase::Notifying,
                "d" => Phase::Draining,
                "r" => Phase::Ready,
                _ => return None,
            },
        }),
        _ => None,
    }
}

fn reply(raw: &[u8]) -> Option<String> {
    let raw = std::str::from_utf8(raw)
        .ok()?
        .trim_end_matches(['\r', '\n']);
    let (id, events) = raw.split_once('|')?;
    let id: u32 = id.parse().ok()?;
    let events: Vec<_> = if events.is_empty() {
        Vec::new()
    } else {
        events.split(',').map(event).collect::<Option<Vec<_>>>()?
    };
    if events.len() > 64 {
        return None;
    }
    let mut state = State::Serving(0);
    let mut steps = Vec::with_capacity(events.len());
    for event in events {
        state = transition(state, event);
        steps.push(snapshot(state));
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
