//! Settings → Bot → Timezone: which zone the agents answer in.
//!
//! The zone travels to the agent on every message the app sends, as the row's `tz` field
//! (`arbos_core::doc::UserMessageOpts`), which agentd turns into the turn's "Now" line. By
//! default it is whatever the Mac itself reports; this module lets one account pin a different
//! zone, and that choice then stands in for the device's everywhere the device's would be used.
//!
//! Kept per account in `<config>/settings.json` under `timezone`, a map of account id to IANA
//! zone name. Per account because two people signing into the same Mac must not inherit each
//! other's zone, and an account with no entry means auto-detect, not "no zone".

use serde_json::{Map, Value};

use arbos_core::doc::device_time_zone;

/// The `settings.json` key holding the account-to-zone map.
const KEY: &str = "timezone";

/// Every IANA zone the app can be pinned to, sorted, each name once. `TZ_VARIANTS` holds the
/// old aliases too and resolves them to the name they were renamed to, so reading it straight
/// lists `Africa/Asmara` twice (once for itself, once for `Africa/Asmera`).
pub fn all_zones() -> &'static [&'static str] {
    static ZONES: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
    ZONES.get_or_init(|| {
        let mut names: Vec<&'static str> =
            chrono_tz::TZ_VARIANTS.iter().map(|z| z.name()).collect();
        names.sort_unstable();
        names.dedup();
        names
    })
}

/// The zone this account pinned by hand, if any. An unknown or empty name is no choice at all,
/// so a zone removed from the database in a later build falls back to auto-detect rather than
/// pinning the agent to a name nothing can parse.
pub fn chosen(account: &str) -> Option<String> {
    chosen_in(&crate::update::read_settings(), account)
}

fn chosen_in(settings: &Map<String, Value>, account: &str) -> Option<String> {
    settings
        .get(KEY)?
        .get(account)?
        .as_str()
        .map(str::trim)
        .filter(|z| known(z))
        .map(str::to_owned)
}

/// Pin `zone` for this account, or clear the pin back to auto-detect with `None`.
pub fn choose(account: &str, zone: Option<&str>) {
    let mut settings = crate::update::read_settings();
    let mut map = match settings.remove(KEY) {
        Some(Value::Object(m)) => m,
        _ => Map::new(),
    };
    match zone.filter(|z| known(z)) {
        Some(z) => {
            map.insert(account.to_owned(), Value::String(z.to_owned()));
        }
        None => {
            map.remove(account);
        }
    }
    crate::update::save_setting(KEY, Value::Object(map));
}

/// The zone the agents are told about: this account's pin when it has one, else the Mac's own.
/// `None` only when the account has no pin and the platform names no zone.
pub fn effective(account: &str) -> Option<String> {
    chosen(account).or_else(device_time_zone)
}

/// What the Timezone row shows on its right: the zone the agents are actually told about,
/// pinned or detected. Only the zone, because the settings pane is 340 pt wide and
/// `Auto-detect (America/Costa_Rica)` does not fit beside the word "Timezone" there — the
/// footnote under the card says which of the two states it is, at no cost in width. The zone
/// list itself has the room, and spells "Auto-detect (…)" out in full.
pub fn row_value(account: &str) -> String {
    effective(account).unwrap_or_else(|| "Not set".to_owned())
}

/// The line under the card. It carries what the row has no room for: whether this zone follows
/// the Mac or was pinned by hand.
pub fn footnote(account: &str) -> &'static str {
    match chosen(account) {
        Some(_) => {
            "Agents answer times, dates and \"today\" in this zone. It stays set when this Mac moves."
        }
        None => {
            "Agents answer times, dates and \"today\" in this zone, detected from this Mac."
        }
    }
}

/// How the zone list names the auto-detect entry, where there is room for both facts:
/// `Auto-detect (America/Costa_Rica)`, or `Auto-detect` alone on a Mac that names no zone.
pub(crate) fn auto_label(device: Option<&str>) -> String {
    match device {
        Some(d) => format!("Auto-detect ({d})"),
        None => "Auto-detect".to_owned(),
    }
}

/// Whether `chrono_tz` — the same database agentd parses the zone with — knows this name. A name
/// it cannot parse would reach the agent and be dropped there, leaving the user's zone silently
/// unknown, so it is refused here instead.
fn known(zone: &str) -> bool {
    !zone.is_empty() && zone.parse::<chrono_tz::Tz>().is_ok()
}

/// The zones a query matches, case-insensitively, on the zone name with `_` read as a space so
/// "costa rica" finds `America/Costa_Rica`. An empty query matches everything.
pub fn matching(query: &str) -> Vec<&'static str> {
    let q = query.trim().to_lowercase().replace('_', " ");
    if q.is_empty() {
        return all_zones().to_vec();
    }
    all_zones()
        .iter()
        .copied()
        .filter(|z| z.to_lowercase().replace('_', " ").contains(&q))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_zone_list_spells_out_what_auto_detect_resolved_to() {
        assert_eq!(
            auto_label(Some("America/Costa_Rica")),
            "Auto-detect (America/Costa_Rica)"
        );
        assert_eq!(auto_label(None), "Auto-detect");
    }

    #[test]
    fn a_pin_is_per_account_and_must_be_a_zone_the_agent_can_parse() {
        let settings: Map<String, Value> = json!({
            "timezone": {
                "gh_1": "Europe/London",
                "gh_2": "Not/AZone",
                "gh_3": "  ",
            }
        })
        .as_object()
        .unwrap()
        .clone();
        assert_eq!(
            chosen_in(&settings, "gh_1").as_deref(),
            Some("Europe/London")
        );
        // A name chrono_tz cannot parse is no pin: the account auto-detects instead.
        assert_eq!(chosen_in(&settings, "gh_2"), None);
        assert_eq!(chosen_in(&settings, "gh_3"), None);
        // An account with no entry of its own never inherits another's.
        assert_eq!(chosen_in(&settings, "gh_4"), None);
        assert_eq!(chosen_in(&Map::new(), "gh_1"), None);
    }

    #[test]
    fn the_search_reads_underscores_as_spaces() {
        assert!(matching("costa rica").contains(&"America/Costa_Rica"));
        assert!(matching("COSTA_RICA").contains(&"America/Costa_Rica"));
        assert!(matching("london").contains(&"Europe/London"));
        assert!(matching("nowhere at all").is_empty());
        assert_eq!(matching("  ").len(), all_zones().len());
    }

    #[test]
    fn a_renamed_zone_is_listed_once_under_its_new_name() {
        // `Africa/Asmera` is an old name for `Africa/Asmara`; both resolve to the same name.
        assert_eq!(
            all_zones()
                .iter()
                .filter(|z| **z == "Africa/Asmara")
                .count(),
            1
        );
        let mut sorted = all_zones().to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, all_zones());
    }
}
