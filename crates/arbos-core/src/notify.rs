//! Things the user should hear about even when no window is open: a reply
//! finished, a question or an approval waits, a turn failed, a notice
//! was posted. A worker finishing or asking while every client was away
//! used to be silent (features backlog, 2026-09-16). The kernel records
//! each one here (`.arbos/notifications.jsonl`), sends it live as a
//! `notify` frame, and replays the unseen ones to a client that attaches;
//! `seen` (a `seen` frame) marks them read for every client.

use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::Place;

/// The most kept on disk; older ones are dropped when the file is trimmed.
pub const KEEP: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Notification {
    /// 1-based, increasing; `seen` names the last id read.
    pub id: u64,
    pub ts: i64,
    pub agent: String,
    /// `reply` (a top-level agent's turn ended with an answer), `ask` (a
    /// question or an approval waits), `error` (a turn failed), `notice`
    /// (a line posted to the user: a subscription, the spend cap).
    pub kind: String,
    /// One line: "root replied", "root asks", "root: turn failed".
    pub title: String,
    /// The first words of the reply, the question, the failure.
    pub body: String,
}

pub fn path(place: &Place) -> PathBuf {
    place.arbos().join("notifications.jsonl")
}

fn seen_path(place: &Place) -> PathBuf {
    place.arbos().join("runtime").join("notifications-seen")
}

pub fn load(place: &Place) -> Vec<Notification> {
    std::fs::read_to_string(path(place))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Append one; returns it with its id. The file is trimmed to `KEEP` when
/// it has grown past twice that.
pub fn record(
    place: &Place,
    agent: &str,
    kind: &str,
    title: &str,
    body: &str,
) -> Result<Notification> {
    crate::check_store(&place.arbos())?;
    std::fs::create_dir_all(place.arbos())?;
    let mut all = load(place);
    let id = all.last().map(|n| n.id).unwrap_or(0) + 1;
    let n = Notification {
        id,
        ts: crate::now_ms(),
        agent: agent.to_string(),
        kind: kind.to_string(),
        title: title.to_string(),
        body: crate::text::clip(body.trim(), 400),
    };
    if all.len() >= KEEP * 2 {
        all.push(n.clone());
        let keep: Vec<&Notification> = all
            .iter()
            .rev()
            .take(KEEP)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let text: String = keep
            .iter()
            .map(|x| serde_json::to_string(x).unwrap_or_default() + "\n")
            .collect();
        let tmp = path(place).with_extension("jsonl.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path(place))?;
    } else {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path(place))?;
        writeln!(f, "{}", serde_json::to_string(&n)?)?;
    }
    Ok(n)
}

/// The last id the user has seen (0: none).
pub fn seen_through(place: &Place) -> u64 {
    std::fs::read_to_string(seen_path(place))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// Mark every notification with id ≤ `through` as seen. Never moves back.
pub fn mark_seen(place: &Place, through: u64) -> Result<u64> {
    let now = seen_through(place).max(through);
    std::fs::create_dir_all(place.arbos().join("runtime"))?;
    std::fs::write(seen_path(place), now.to_string())?;
    Ok(now)
}

/// Notifications the user has not seen, oldest first.
pub fn unseen(place: &Place) -> Vec<Notification> {
    let through = seen_through(place);
    load(place).into_iter().filter(|n| n.id > through).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notifications_are_numbered_kept_and_marked_seen() {
        let dir = tempfile::tempdir().unwrap();
        let place = Place::new(dir.path());
        let a = record(
            &place,
            "root",
            "reply",
            "root replied",
            "Done: three files.",
        )
        .unwrap();
        let b = record(&place, "root", "ask", "root asks", "Which branch?").unwrap();
        assert_eq!((a.id, b.id), (1, 2));
        assert_eq!(unseen(&place).len(), 2);
        assert_eq!(mark_seen(&place, 1).unwrap(), 1);
        assert_eq!(
            unseen(&place).iter().map(|n| n.id).collect::<Vec<_>>(),
            vec![2]
        );
        // Seen never moves back.
        assert_eq!(mark_seen(&place, 0).unwrap(), 1);
        // A long body is clipped; the file is trimmed past twice KEEP.
        for _ in 0..(KEEP * 2) {
            record(&place, "root", "notice", "n", &"x".repeat(1000)).unwrap();
        }
        let all = load(&place);
        assert!(all.len() < KEEP * 2, "trimmed on the way: {}", all.len());
        assert!(all.last().unwrap().body.chars().count() <= 401);
        assert_eq!(all.last().unwrap().id, 2 + KEEP as u64 * 2);
    }
}
