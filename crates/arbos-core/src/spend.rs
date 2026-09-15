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
    match cap_usd(place) {
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
