//! `.arbos/spend.toml`: what the place's turns have cost so far, and the
//! cap the user set (`project.toml [spend] cap_usd`). Cursor's coordinator
//! checks spend caps mid-run and stops at the cap; the kernel does it for
//! every agent here: the cost of each turn (`turn_complete.usage.cost`)
//! is added when the turn ends, and once the cap is reached only the
//! user's own words to a top-level agent still open a turn — so the cap
//! can be raised — while workers, subscriptions, and `spawn` are refused
//! with a plain notice. The user is told once at 80 % and once at the cap.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::Place;

pub const FILE: &str = "spend.toml";
/// The one-time warning comes at this share of the cap.
pub const WARN_AT: f64 = 0.8;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Spend {
    /// US dollars, summed over every turn of every agent since `since`.
    #[serde(default)]
    pub spent_usd: f64,
    #[serde(default)]
    pub turns: u64,
    /// RFC 3339: when the count began (the first turn, or the last reset).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub since: String,
    /// The user has been told the 80 % mark was passed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub warned: bool,
    /// The user has been told the cap was reached.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub capped: bool,
}

pub fn path(place: &Place) -> PathBuf {
    place.arbos().join(FILE)
}

pub fn load(place: &Place) -> Spend {
    std::fs::read_to_string(path(place))
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save(place: &Place, spend: &Spend) -> Result<()> {
    let p = path(place);
    let text = toml::to_string(spend).context("serialise spend")?;
    let tmp = p.with_extension("toml.tmp");
    std::fs::write(&tmp, text).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &p).with_context(|| format!("replace {}", p.display()))?;
    Ok(())
}

/// The cap from `project.toml`, if the user set one.
pub fn cap_usd(place: &Place) -> Option<f64> {
    crate::project::load(place)
        .spend
        .cap_usd
        .filter(|c| *c > 0.0)
}

/// The per-turn cap from `project.toml`, if the user set one.
pub fn turn_cap_usd(place: &Place) -> Option<f64> {
    crate::project::load(place)
        .spend
        .turn_cap_usd
        .filter(|c| *c > 0.0)
}

/// The cap one turn runs against: the place's `turn_cap_usd`, the host's
/// `max_turn_cost_usd` (a harness or machine-wide knob), or the smaller
/// when both are set. `None` when neither is.
pub fn effective_turn_cap(place: &Place, host_cap: f64) -> Option<(f64, &'static str)> {
    let host = (host_cap > 0.0).then_some((
        host_cap,
        "max_turn_cost_usd in config.toml (or ARBOS_MAX_TURN_COST)",
    ));
    let place_cap =
        turn_cap_usd(place).map(|c| (c, "turn_cap_usd under [spend] in .arbos/project.toml"));
    match (place_cap, host) {
        (Some(p), Some(h)) => Some(if p.0 <= h.0 { p } else { h }),
        (Some(p), None) => Some(p),
        (None, Some(h)) => Some(h),
        (None, None) => None,
    }
}

/// How a turn that stopped at the per-turn cap opens its closing line.
/// A rule the user set working, not a failure: the line is a plain
/// notice (`failed: false`), the turn's one closing line — no
/// `interrupted` beside it — and the parent's report and the user's
/// notification read it by this prefix.
pub const TURN_CAP_PREFIX: &str = "Stopped at the per-turn cap";

/// What the user reads when a turn ends on its cost cap: the rule
/// working, the numbers as they are, what stays, and the fix in the same
/// breath.
pub fn turn_cap_notice(spent: f64, cap: f64, where_set: &str) -> String {
    format!(
        "{TURN_CAP_PREFIX}: this turn spent {} on model calls, over the {} you allow for one turn. What is in the working tree stays. A new message starts a fresh budget; to allow more per turn, raise {where_set}.",
        money(spent),
        money(cap)
    )
}

/// Dollars with the precision the amount needs: cents when there are
/// cents, more digits below that, so $0.0038 over a $0.0001 cap never
/// reads "$0.00 over $0.00".
pub fn money(usd: f64) -> String {
    let v = usd.abs();
    if v == 0.0 || !v.is_finite() {
        return "$0.00".into();
    }
    if v >= 0.01 {
        return format!("${v:.2}");
    }
    // Below a cent: two significant figures, trailing zeros trimmed.
    let places = ((-v.log10().floor()) as usize + 1).clamp(3, 8);
    let s = format!("{v:.places$}");
    let s = s.trim_end_matches('0');
    let s = if s.ends_with('.') {
        format!("{s}00")
    } else {
        s.to_string()
    };
    format!("${s}")
}

/// Add one turn's cost. Returns the spend after, and which threshold this
/// turn crossed for the first time: `Some(true)` the cap, `Some(false)`
/// the warning mark, `None` neither.
pub fn add_turn(place: &Place, cost_usd: f64) -> Result<(Spend, Option<bool>)> {
    let mut s = load(place);
    if s.since.is_empty() {
        s.since = crate::inbox::rfc3339(crate::now_ms());
    }
    s.spent_usd += cost_usd.max(0.0);
    s.turns += 1;
    let mut crossed = None;
    if let Some(cap) = cap_usd(place) {
        if s.spent_usd >= cap && !s.capped {
            s.capped = true;
            s.warned = true;
            crossed = Some(true);
        } else if s.spent_usd >= cap * WARN_AT && !s.warned {
            s.warned = true;
            crossed = Some(false);
        }
    }
    save(place, &s)?;
    Ok((s, crossed))
}

/// Is the place at or over its cap?
pub fn over_cap(place: &Place) -> bool {
    match cap_usd(place) {
        Some(cap) => load(place).spent_usd >= cap,
        None => false,
    }
}

/// The prompt's `Spend:` line, or empty when nothing has been spent and
/// no cap is set.
pub fn prompt_line(place: &Place) -> String {
    let s = load(place);
    let line = match cap_usd(place) {
        Some(cap) => format!(
            "Spend: ${:.2} of the ${cap:.2} cap ({} turns){}\n",
            s.spent_usd,
            s.turns,
            if s.spent_usd >= cap {
                " — cap reached: workers and subscriptions are refused until the user raises cap_usd in .arbos/project.toml"
            } else {
                ""
            }
        ),
        None if s.spent_usd > 0.0 => format!(
            "Spend: ${:.2} so far ({} turns); no cap set\n",
            s.spent_usd, s.turns
        ),
        None => String::new(),
    };
    match turn_cap_usd(place) {
        Some(t) => {
            format!("{line}Each turn may spend up to ${t:.2}; past that it ends with a notice.\n")
        }
        None => line,
    }
}

pub fn refusal(place: &Place) -> String {
    let s = load(place);
    let cap = cap_usd(place).unwrap_or(0.0);
    format!(
        "spend cap reached: ${:.2} of ${cap:.2} spent. Raise cap_usd under [spend] in .arbos/project.toml (or remove it) to go on.",
        s.spent_usd
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_shows_small_amounts_and_the_notice_reads_as_the_rule_working() {
        assert_eq!(money(1.2), "$1.20");
        assert_eq!(money(0.05), "$0.05");
        assert_eq!(money(0.0038), "$0.0038");
        assert_eq!(money(0.0001), "$0.0001");
        assert_eq!(money(0.00001), "$0.00001");
        assert_eq!(money(0.0), "$0.00");
        let n = turn_cap_notice(
            0.00377,
            0.0001,
            "turn_cap_usd under [spend] in .arbos/project.toml",
        );
        assert!(n.starts_with("Stopped at the per-turn cap: this turn spent $0.0038 on model calls, over the $0.0001 you allow"), "{n}");
        assert!(n.contains("raise turn_cap_usd under [spend]"), "{n}");
        assert!(
            !n.to_ascii_lowercase().contains("error") && !n.to_ascii_lowercase().contains("fail"),
            "{n}"
        );
    }

    fn place(tag: &str) -> Place {
        let dir = std::env::temp_dir().join(format!("arbos-spend-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".arbos")).unwrap();
        Place::new(dir)
    }

    #[test]
    fn turns_add_up_and_the_marks_are_crossed_once() {
        let p = place("marks");
        std::fs::write(
            crate::project::path(&p),
            "schema = 2\n[spend]\ncap_usd = 1.0\n",
        )
        .unwrap();
        assert_eq!(cap_usd(&p), Some(1.0));
        let (s, crossed) = add_turn(&p, 0.5).unwrap();
        assert_eq!(s.turns, 1);
        assert_eq!(crossed, None);
        let (_, crossed) = add_turn(&p, 0.35).unwrap();
        assert_eq!(crossed, Some(false), "80 % passed once");
        let (_, crossed) = add_turn(&p, 0.05).unwrap();
        assert_eq!(crossed, None, "not said twice");
        assert!(!over_cap(&p));
        let (s, crossed) = add_turn(&p, 0.2).unwrap();
        assert_eq!(crossed, Some(true));
        assert!(over_cap(&p) && s.capped);
        let (_, crossed) = add_turn(&p, 0.2).unwrap();
        assert_eq!(crossed, None);
        assert!(
            prompt_line(&p).contains("cap reached"),
            "{}",
            prompt_line(&p)
        );
        assert!(refusal(&p).starts_with("spend cap reached: $1.30 of $1.00"));
        // Raising the cap lifts it.
        std::fs::write(
            crate::project::path(&p),
            "schema = 2\n[spend]\ncap_usd = 5.0\n",
        )
        .unwrap();
        assert!(!over_cap(&p));
    }

    #[test]
    fn without_a_cap_spend_is_only_counted() {
        let p = place("nocap");
        assert!(prompt_line(&p).is_empty());
        let (_, crossed) = add_turn(&p, 2.0).unwrap();
        assert_eq!(crossed, None);
        assert!(!over_cap(&p));
        assert!(prompt_line(&p).starts_with("Spend: $2.00 so far (1 turns); no cap set"));
    }
}
