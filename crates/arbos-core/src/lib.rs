//! File kernel types and the place store.
//!
//! Five types: [`Place`], [`Agent`], [`Page`], [`Event`], [`Node`].
//! A [`Wake`] is derived from a node whose moment came.
//! The tree is the directory. The log is JSONL.

mod agent;
pub mod agent_def;
pub mod chattitle;
pub mod cloudsync;
pub mod envsafe;
mod event;
pub mod files;
pub mod host;
pub mod hub;
pub mod inbox;
mod lock;
pub mod machines;
pub mod models;
pub mod notes;
mod page;
mod place;
pub mod project;
pub mod protocol;
pub mod prs;
pub mod skills;
pub mod store;
pub mod subscription;
pub mod text;
pub mod waiting;
mod wake;
pub mod wire;

pub use agent::validate_id;
pub use agent::{ALL_TOOLS, Agent, AgentId, Mode};
pub use agent_def::{AgentDef, find_def, load_defs};
pub use event::{Event, EventKind, ToolRec, Usage};
pub use files::{
    Layout, ROOT_ID, TranscriptTail, agent_exists, append_event, append_events, bootstrap,
    create_chat, list_agents, load_agent, load_transcript, needs_serve, read_focus, validate_focus,
    write_focus,
};
pub use host::{Host, HostConfig, KeySource, ProviderKind};
pub use hub::{HubConfig, HubFrame, MachineInfo, MeshTarget, ProjectInfo, RegistrantKind};
pub use lock::PlaceLock;
pub use machines::{Machine, Machines};
/// A row id in the window's plan frame (inbox file, subscription, notes
/// item): see `wire::PlanNode`.
pub type NodeId = u64;
pub use page::{Page, PageKind};
pub use place::Place;
pub use prs::{PrRec, load_prs, record_pr};
pub use skills::{Skill, load_skills, slash_skill};
pub use wake::{Wake, WakeKind};

/// `~/.config/arbos` (or `$XDG_CONFIG_HOME/arbos`): the host's own files.
pub fn host_dir() -> std::path::PathBuf {
    if let Some(base) = std::env::var_os("XDG_CONFIG_HOME") {
        return std::path::PathBuf::from(base).join("arbos");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return std::path::PathBuf::from(home).join(".config").join("arbos");
    }
    std::path::PathBuf::from(".arbos-host")
}

/// Whether a message is a bare control word — `stop`, `cancel`, `halt`,
/// `wait`, `pause` — the whole message, any case, trailing punctuation
/// allowed. Typed while a turn runs it means "interrupt", never a prompt.
pub fn is_stop_word(text: &str) -> bool {
    let word = text
        .trim()
        .trim_end_matches(['.', '!', '…', ',', ';'])
        .trim()
        .to_ascii_lowercase();
    matches!(
        word.as_str(),
        "stop" | "cancel" | "halt" | "wait" | "pause" | "stop it" | "stop now" | "cancel that"
    )
}

/// Env var that moves the clock: `ARBOS_NOW=2026-09-13T09:00:00Z` (or unix
/// millis). The process's clock reads that instant at start and runs
/// forward from it, so crons and `after` nodes in a fixture fire on cue
/// while timeouts and elapsed times still make sense. Every timestamp the
/// kernel writes comes from [`now_ms`], so they all shift together.
pub const NOW_ENV: &str = "ARBOS_NOW";

/// Unix millis, on the shifted clock when `ARBOS_NOW` is set.
pub fn now_ms() -> i64 {
    real_now_ms() + clock_offset_ms()
}

fn real_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `ARBOS_NOW` minus the real time when it was first read; zero without it.
/// An unreadable value is zero too, but says so once on stderr rather than
/// failing every caller.
pub fn clock_offset_ms() -> i64 {
    static OFFSET: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *OFFSET.get_or_init(|| {
        let Some(raw) = std::env::var(NOW_ENV).ok().filter(|s| !s.trim().is_empty()) else {
            return 0;
        };
        match parse_instant_ms(raw.trim()) {
            Some(target) => target - real_now_ms(),
            None => {
                eprintln!("{NOW_ENV}={raw:?}: not a time (want 2026-09-13T09:00:00Z or unix millis); clock unchanged");
                0
            }
        }
    })
}

/// `2026-09-13T09:00:00Z`, `2026-09-13T09:00:00.250Z`, `2026-09-13 09:00`,
/// `2026-09-13` (midnight), or unix millis/seconds. UTC only.
pub fn parse_instant_ms(s: &str) -> Option<i64> {
    if let Ok(n) = s.parse::<i64>() {
        // Seconds until the year 33658; anything larger is millis.
        return Some(if n.abs() < 100_000_000_000 {
            n * 1000
        } else {
            n
        });
    }
    let s = s.trim_end_matches('Z').trim_end_matches("+00:00");
    let (date, time) = match s.split_once(['T', ' ']) {
        Some((d, t)) => (d, t),
        None => (s, "00:00:00"),
    };
    let mut dp = date.split('-');
    let y: i64 = dp.next()?.parse().ok()?;
    let m: i64 = dp.next()?.parse().ok()?;
    let d: i64 = dp.next()?.parse().ok()?;
    if dp.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let mut tp = time.split(':');
    let hh: i64 = tp.next()?.parse().ok()?;
    let mm: i64 = tp.next().unwrap_or("0").parse().ok()?;
    let sec_part = tp.next().unwrap_or("0");
    let (ss, frac_ms) = match sec_part.split_once('.') {
        Some((a, f)) => {
            let digits: String = f.chars().take(3).collect();
            let ms: i64 = format!("{digits:0<3}").parse().ok()?;
            (a.parse::<i64>().ok()?, ms)
        }
        None => (sec_part.parse::<i64>().ok()?, 0),
    };
    if hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    let days = days_from_civil(y, m, d);
    Some((((days * 24 + hh) * 60 + mm) * 60 + ss) * 1000 + frac_ms)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
