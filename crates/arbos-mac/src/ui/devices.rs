//! Settings → Devices (Jacob, 2026-10-23): the account's signed-in devices
//! — one card, a row per device (glyph, name, kind tag, added + last seen),
//! this Mac marked, and for every *other* device a Sign out that asks first.
//! The current device's row offers the app's own Sign out instead, so the
//! user is never stranded. List + revoke only (relay `GET /auth/devices`,
//! `DELETE /auth/devices/{id}`); second-device approval stays deferred.
//!
//! This module is the pure half (what the rows say); `profiles/me.rs` draws
//! them with the pane kit.

use crate::model::DeviceView;

/// What one device row shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceRow {
    pub id: String,
    pub name: String,
    /// The tile glyph: `laptop`, `smartphone`, `bot`, `desktop`.
    pub glyph: &'static str,
    /// Small tag after the name ("read-only", "pair code"), when the kind is
    /// not a plain sign-in.
    pub tag: Option<&'static str>,
    /// "Active now · added Oct 11" / "2 h ago · added Oct 14".
    pub subtitle: String,
    /// The device making the request: marked "This Mac", no Sign out here.
    pub current: bool,
}

impl DeviceRow {
    /// Only another device can be signed out from this list.
    pub fn revokable(&self) -> bool {
        !self.current
    }
}

/// The rows, this Mac first, then the others as the relay orders them
/// (oldest sign-in first).
pub fn rows(devices: &[DeviceView], now_ms: i64) -> Vec<DeviceRow> {
    let mut out: Vec<DeviceRow> = devices.iter().map(|d| row(d, now_ms)).collect();
    out.sort_by_key(|r| !r.current);
    out
}

fn row(d: &DeviceView, now_ms: i64) -> DeviceRow {
    DeviceRow {
        id: d.id.clone(),
        name: if d.name.trim().is_empty() {
            "Unnamed device".to_string()
        } else {
            d.name.trim().to_string()
        },
        glyph: glyph_for(&d.name, &d.kind),
        tag: kind_tag(&d.kind),
        subtitle: format!(
            "{} · added {}",
            seen_label(now_ms, d.last_seen_at),
            date_label(d.created_at)
        ),
        current: d.current,
    }
}

/// The tile glyph from the kind, then the name: a reader is a bot, a phone
/// or iPad is a phone, anything with Mac in it is a laptop.
pub fn glyph_for(name: &str, kind: &str) -> &'static str {
    let n = name.to_lowercase();
    if kind == "reader" || n.contains("bot") {
        "server.rack"
    } else if n.contains("iphone") || n.contains("ipad") || n.contains("phone") {
        "iphone"
    } else if n.contains("imac") || n.contains("mac mini") || n.contains("studio") {
        "desktopcomputer"
    } else {
        "laptopcomputer"
    }
}

/// `kind` as a small tag: readers (Grok Bot's read access) say "read-only",
/// the old pairing-code devices "pair code"; a GitHub sign-in needs no tag.
pub fn kind_tag(kind: &str) -> Option<&'static str> {
    match kind {
        "reader" => Some("read-only"),
        "paircode" => Some("pair code"),
        _ => None,
    }
}

/// "Active now" within two minutes, then minutes / hours / days, then the date.
pub fn seen_label(now_ms: i64, last_seen_at: Option<i64>) -> String {
    let Some(seen) = last_seen_at else {
        return "Never seen".to_string();
    };
    let ago = now_ms.saturating_sub(seen).max(0);
    let min = ago / 60_000;
    if min < 2 {
        "Active now".to_string()
    } else if min < 60 {
        format!("{min} min ago")
    } else if min < 24 * 60 {
        format!("{} h ago", min / 60)
    } else if min < 14 * 24 * 60 {
        format!("{} d ago", min / (24 * 60))
    } else {
        date_label(seen)
    }
}

/// "Oct 11" (this year) or "Oct 11, 2025".
pub fn date_label(ms: i64) -> String {
    use chrono::{Datelike, TimeZone, Utc};
    let Some(t) = Utc.timestamp_millis_opt(ms).single() else {
        return "—".to_string();
    };
    let now = Utc::now();
    if t.year() == now.year() {
        t.format("%b %-d").to_string()
    } else {
        t.format("%b %-d, %Y").to_string()
    }
}

/// The confirm line under a Sign out: who, and what it means.
pub fn confirm_question(name: &str) -> String {
    format!("Sign out {name}? It will need to sign in again.")
}

/// Drop `id` from the list unless it is the current device — the list never
/// loses this Mac through a revoke (the fixture and the optimistic update
/// share the rule).
pub fn remove_other(devices: &mut Vec<DeviceView>, id: &str) -> bool {
    let before = devices.len();
    devices.retain(|d| d.id != id || d.current);
    devices.len() != before
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(
        id: &str,
        name: &str,
        kind: &str,
        current: bool,
        seen_ago_ms: Option<i64>,
    ) -> DeviceView {
        let now = 1_761_200_000_000; // 2025-10-23-ish
        DeviceView {
            id: id.into(),
            name: name.into(),
            kind: kind.into(),
            created_at: now - 9 * 24 * 3_600_000,
            last_seen_at: seen_ago_ms.map(|a| now - a),
            current,
        }
    }

    const NOW: i64 = 1_761_200_000_000;

    #[test]
    fn the_list_renders_this_mac_first_and_marks_it() {
        let rows = rows(
            &[
                dev("b", "iPhone", "github", false, Some(2 * 3_600_000)),
                dev("a", "Jacob's MacBook Pro", "github", true, Some(10_000)),
                dev(
                    "c",
                    "GrokBot-Arbos",
                    "reader",
                    false,
                    Some(3 * 24 * 3_600_000),
                ),
            ],
            NOW,
        );
        assert_eq!(rows.len(), 3);
        assert!(rows[0].current && rows[0].id == "a", "this Mac first");
        assert_eq!(rows[0].glyph, "laptopcomputer");
        assert_eq!(rows[0].tag, None);
        assert!(rows[0].subtitle.starts_with("Active now · added "));
        assert_eq!(rows[1].glyph, "iphone");
        assert!(rows[1].subtitle.starts_with("2 h ago · added "));
        assert_eq!(rows[2].glyph, "server.rack");
        assert_eq!(rows[2].tag, Some("read-only"));
        assert!(rows[2].subtitle.starts_with("3 d ago · added "));
    }

    #[test]
    fn only_other_devices_can_be_signed_out() {
        let rows = rows(
            &[
                dev("a", "Jacob's MacBook Pro", "github", true, Some(0)),
                dev("c", "GrokBot-Arbos", "reader", false, Some(0)),
            ],
            NOW,
        );
        assert!(!rows[0].revokable(), "the current row has no Sign out");
        assert!(rows[1].revokable());
        assert_eq!(
            confirm_question(&rows[1].name),
            "Sign out GrokBot-Arbos? It will need to sign in again."
        );
    }

    #[test]
    fn revoking_removes_the_row_but_never_this_mac() {
        let mut list = vec![
            dev("a", "Jacob's MacBook Pro", "github", true, Some(0)),
            dev("b", "iPhone", "github", false, Some(0)),
            dev("c", "GrokBot-Arbos", "reader", false, Some(0)),
        ];
        assert!(remove_other(&mut list, "c"));
        assert_eq!(
            list.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert!(!remove_other(&mut list, "a"), "the current device stays");
        assert_eq!(list.len(), 2);
        assert!(!remove_other(&mut list, "zzz"));
    }

    #[test]
    fn seen_labels_step_from_now_to_dates() {
        assert_eq!(seen_label(NOW, None), "Never seen");
        assert_eq!(seen_label(NOW, Some(NOW - 30_000)), "Active now");
        assert_eq!(seen_label(NOW, Some(NOW - 5 * 60_000)), "5 min ago");
        assert_eq!(seen_label(NOW, Some(NOW - 2 * 3_600_000)), "2 h ago");
        assert_eq!(seen_label(NOW, Some(NOW - 3 * 24 * 3_600_000)), "3 d ago");
        assert!(seen_label(NOW, Some(NOW - 40 * 24 * 3_600_000)).contains(' '));
    }

    #[test]
    fn kinds_and_names_pick_glyphs_and_tags() {
        assert_eq!(glyph_for("Jacob's iPhone", "github"), "iphone");
        assert_eq!(glyph_for("iPad", "github"), "iphone");
        assert_eq!(glyph_for("Jacob's iMac", "github"), "desktopcomputer");
        assert_eq!(glyph_for("GrokBot-Arbos", "reader"), "server.rack");
        assert_eq!(glyph_for("anything", "reader"), "server.rack");
        assert_eq!(glyph_for("MacBook Air", "paircode"), "laptopcomputer");
        assert_eq!(kind_tag("github"), None);
        assert_eq!(kind_tag("reader"), Some("read-only"));
        assert_eq!(kind_tag("paircode"), Some("pair code"));
    }
}
