//! Polls in chats (`docs/polls.md`): the shape, its bounds, and its text form.
//!
//! One validator for every boundary: the composers build a poll through [`Poll::new`], the
//! relay's push guard and every reader take one through [`Poll::parse`], and a vote is read
//! through [`Poll::selections`]. Anything that does not pass is treated as not a poll.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const QUESTION_MAX: usize = 300;
pub const LABEL_MAX: usize = 100;
pub const OPTIONS_MIN: usize = 2;
pub const OPTIONS_MAX: usize = 10;
const OPTION_ID_MAX: usize = 8;
/// The latest time a poll field may name, in ms since the epoch (2100-01-01): the sanity bound
/// on a closing time, a manual close and a vote's time.
pub const TIME_MAX: i64 = 4_102_444_800_000;
/// The longest a poll may be posted to run: one week.
pub const DURATION_MAX_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// How far the relay's clock is ahead of this device's, in ms. One relay per process: a
/// second account on another relay would need this per relay.
static CLOCK_AHEAD_MS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// Tell the poll rules what the relay's clock read just now (`MeDto::now_ms`, on receipt).
/// Every time on a poll is the relay's (it stamps the opening, each vote and the close), so
/// a device judges "closed" and counts down against the relay's clock, not its own: a phone
/// half an hour slow would otherwise show a closed poll as open and a countdown half an
/// hour long.
pub fn set_relay_now(relay_now_ms: i64) {
    let ahead = relay_now_ms - crate::ids::now_ms();
    CLOCK_AHEAD_MS.store(ahead, std::sync::atomic::Ordering::Relaxed);
}

/// The time by the relay's clock, as closely as this device knows it ([`set_relay_now`]);
/// the device's own until then. The `now` for [`closed`] and for every time a device writes
/// on a poll.
pub fn now() -> i64 {
    crate::ids::now_ms() + CLOCK_AHEAD_MS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Whether a poll is closed at `now`: the one rule, for the writers, the relay and both apps.
/// `deadline` is [`Poll::deadline`] (the `deadline` of the projected part); `None` never closes.
pub fn closed(deadline: Option<i64>, now: i64) -> bool {
    deadline.is_some_and(|deadline| now >= deadline)
}

/// The line under a poll's options, the same words on both apps: "Ana's poll · 3 voted · pick
/// one · names shown · closes in 2 h", or "… · closed". `by` is whose poll when it is not the
/// reader's; `left_ms` is the time to the deadline, when it has one.
pub fn foot(
    by: Option<&str>,
    voted: u32,
    multiple: bool,
    show_voters: bool,
    closed: bool,
    left_ms: Option<i64>,
) -> String {
    let whose = by.map_or("Your poll".to_owned(), |name| format!("{name}'s poll"));
    let rule = match (closed, multiple) {
        (true, _) => "closed",
        (false, true) => "pick any",
        (false, false) => "pick one",
    };
    let mut foot = format!("{whose} · {voted} voted · {rule}");
    if show_voters {
        foot.push_str(" · names shown");
    }
    if let Some(left) = left_ms.filter(|_| !closed) {
        let minutes = (left / 60_000).max(1);
        let left = match minutes {
            m if m >= 2 * 24 * 60 => format!("{} days", m / (24 * 60)),
            m if m >= 60 => format!("{} h", m / 60),
            m => format!("{m} min"),
        };
        foot.push_str(&format!(" · closes in {left}"));
    }
    foot
}

/// A time stored on a poll row (its closing time, the manual close, a vote's time): an integer in
/// `1..=TIME_MAX`. Anything else is not a time.
pub fn time(value: &Value) -> Option<i64> {
    value.as_i64().filter(|t| (1..=TIME_MAX).contains(t))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PollOption {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Poll {
    pub question: String,
    pub options: Vec<PollOption>,
    /// How many options one person may hold: `1`, or `options.len()` for "multiple answers".
    pub max: usize,
    /// "Show who voted", chosen by the author and fixed with the poll. `None` is a poll from
    /// before this field: its votes carry no time, so it cannot be closed, and its voters
    /// are not shown.
    #[serde(
        rename = "showVoters",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub show_voters: Option<bool>,
    /// How long the poll runs before it closes by itself, in ms: what its author picked.
    /// `None` runs until closed by hand. When it closes is not here but on the row
    /// ([`crate::doc::POLL_CLOSES_FIELD`]), where the relay writes it from its own clock.
    #[serde(rename = "closesIn", default, skip_serializing_if = "Option::is_none")]
    pub closes_in: Option<i64>,
}

/// `1..=max` characters once trimmed.
fn sized(s: &str, max: usize) -> bool {
    (1..=max).contains(&s.trim().chars().count())
}

impl Poll {
    /// A poll from what a composer collected, or `None` when it is out of bounds. Blank option
    /// rows are dropped; ids are the option's place in the list. `closes_in_ms` is how long it
    /// runs, at most [`DURATION_MAX_MS`]; `None` runs until its author closes it.
    pub fn new(
        question: &str,
        labels: &[String],
        multiple: bool,
        show_voters: bool,
        closes_in_ms: Option<i64>,
    ) -> Option<Self> {
        Self::build(question, labels, multiple, Some(show_voters), closes_in_ms)
    }

    /// A poll of the older shape — no visibility flag, no duration, cannot be closed — for a
    /// relay that does not take the newer fields (`features.poll_options` absent).
    pub fn older(question: &str, labels: &[String], multiple: bool) -> Option<Self> {
        Self::build(question, labels, multiple, None, None)
    }

    fn build(
        question: &str,
        labels: &[String],
        multiple: bool,
        show_voters: Option<bool>,
        closes_in: Option<i64>,
    ) -> Option<Self> {
        let options: Vec<PollOption> = labels
            .iter()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .enumerate()
            .map(|(i, label)| PollOption {
                id: (i + 1).to_string(),
                label: label.to_owned(),
            })
            .collect();
        let poll = Self {
            question: question.trim().to_owned(),
            max: if multiple { options.len() } else { 1 },
            options,
            show_voters,
            closes_in,
        };
        poll.is_valid().then_some(poll)
    }

    /// A poll read from a doc or a push: strict on types, unknown fields and bounds.
    pub fn parse(value: &Value) -> Option<Self> {
        let poll: Self = serde_json::from_value(value.clone()).ok()?;
        poll.is_valid().then_some(poll)
    }

    fn is_valid(&self) -> bool {
        let id_ok = |id: &str| {
            (1..=OPTION_ID_MAX).contains(&id.len())
                && id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        };
        sized(&self.question, QUESTION_MAX)
            && (OPTIONS_MIN..=OPTIONS_MAX).contains(&self.options.len())
            && (1..=self.options.len()).contains(&self.max)
            // A duration only on a timed poll: without `showVoters` its votes carry no time
            // and nothing could be held against the close.
            && self
                .closes_in
                .is_none_or(|d| self.timed() && (1..=DURATION_MAX_MS).contains(&d))
            && self.options.iter().enumerate().all(|(i, o)| {
                id_ok(&o.id)
                    && sized(&o.label, LABEL_MAX)
                    && self.options[..i].iter().all(|p| p.id != o.id)
            })
    }

    /// Votes on this poll carry their time and it can be closed: every poll written since
    /// `showVoters` exists. An older one has neither.
    pub fn timed(&self) -> bool {
        self.show_voters.is_some()
    }

    /// The time after which a vote does not count: the earlier of the row's closing time
    /// (`closes`, [`crate::doc::POLL_CLOSES_FIELD`]; read only on a poll with a duration) and
    /// the author's manual close (`closed`, [`crate::doc::POLL_CLOSED_FIELD`]). `None` is
    /// open. An untimed poll never closes.
    pub fn deadline(&self, closes: Option<&Value>, closed: Option<&Value>) -> Option<i64> {
        if !self.timed() {
            return None;
        }
        let closes = closes.filter(|_| self.closes_in.is_some());
        [closes, closed]
            .into_iter()
            .flatten()
            .filter_map(time)
            .min()
    }

    /// One person's vote as stored: a list of known option ids, no repeats, at most `max`.
    /// Empty is a retracted vote. `None` is not a vote in this poll.
    pub fn selections(&self, value: &Value) -> Option<Vec<String>> {
        let picked: Vec<String> = serde_json::from_value(value.clone()).ok()?;
        let ok = picked.len() <= self.max
            && picked.iter().enumerate().all(|(i, id)| {
                self.options.iter().any(|o| o.id == *id) && !picked[..i].contains(id)
            });
        ok.then_some(picked)
    }

    /// The plain-text form written as the message's text part: what a build without polls,
    /// a notification and an agent read.
    pub fn fallback_text(&self) -> String {
        let mut out = format!("Poll: {}", self.question);
        for (i, o) in self.options.iter().enumerate() {
            out.push_str(&format!("\n{}. {}", i + 1, o.label));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn labels(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("Option {i}")).collect()
    }

    #[test]
    fn a_composed_poll_is_trimmed_and_bounded() {
        let poll = Poll::new(
            "  Where do we meet? ",
            &[" Lisbon ".into(), "".into(), "Berlin".into()],
            false,
            false,
            None,
        )
        .unwrap();
        assert_eq!(poll.question, "Where do we meet?");
        assert_eq!(poll.max, 1);
        assert_eq!(
            poll.options
                .iter()
                .map(|o| (o.id.as_str(), o.label.as_str()))
                .collect::<Vec<_>>(),
            [("1", "Lisbon"), ("2", "Berlin")]
        );
        assert_eq!(
            poll.fallback_text(),
            "Poll: Where do we meet?\n1. Lisbon\n2. Berlin"
        );
        assert_eq!(
            Poll::new("Q", &labels(3), true, false, None).unwrap().max,
            3
        );

        assert!(Poll::new(" ", &labels(2), false, false, None).is_none());
        assert!(Poll::new(
            &"q".repeat(QUESTION_MAX + 1),
            &labels(2),
            false,
            false,
            None
        )
        .is_none());
        assert!(Poll::new("Q", &labels(1), false, false, None).is_none());
        assert!(Poll::new("Q", &labels(OPTIONS_MAX + 1), false, false, None).is_none());
        assert!(Poll::new(
            "Q",
            &["a".repeat(LABEL_MAX + 1), "b".into()],
            false,
            false,
            None
        )
        .is_none());
        assert!(Poll::new("Q", &labels(OPTIONS_MAX), false, false, None).is_some());
    }

    #[test]
    fn a_parsed_poll_is_strict() {
        let good = json!({ "question": "Q", "max": 1,
            "options": [{ "id": "a", "label": "A" }, { "id": "b", "label": "B" }] });
        assert!(Poll::parse(&good).is_some());

        let broken = |edit: fn(&mut Value)| {
            let mut v = json!({ "question": "Q", "max": 1,
                "options": [{ "id": "a", "label": "A" }, { "id": "b", "label": "B" }] });
            edit(&mut v);
            Poll::parse(&v).is_none()
        };
        assert!(broken(|v| v["extra"] = json!(1)), "unknown field");
        assert!(
            broken(|v| v["options"][0]["extra"] = json!(1)),
            "unknown option field"
        );
        assert!(broken(|v| v["question"] = json!(7)), "wrong type");
        assert!(broken(|v| v["question"] = json!("  ")), "blank question");
        assert!(broken(|v| v["max"] = json!(0)), "max below one");
        assert!(broken(|v| v["max"] = json!(3)), "max above the options");
        assert!(broken(|v| v["max"] = json!(-1)), "negative max");
        assert!(
            broken(|v| v["options"][1]["id"] = json!("a")),
            "repeated id"
        );
        assert!(broken(|v| v["options"][0]["id"] = json!("A")), "id case");
        assert!(
            broken(|v| v["options"][0]["id"] = json!("abcdefghi")),
            "id length"
        );
        assert!(
            broken(|v| v["options"][0]["label"] = json!("")),
            "blank label"
        );
        assert!(
            broken(|v| v["options"] = json!([{ "id": "a", "label": "A" }])),
            "one option"
        );
    }

    /// A poll stored before `showVoters` and `closesIn` existed still reads, as an untimed
    /// poll that never closes; the new fields are typed and bounded like the old ones.
    #[test]
    fn the_new_fields_are_optional_typed_and_bounded() {
        let old = json!({ "question": "Q", "max": 1,
            "options": [{ "id": "a", "label": "A" }, { "id": "b", "label": "B" }] });
        let poll = Poll::parse(&old).unwrap();
        assert_eq!((poll.show_voters, poll.closes_in), (None, None));
        assert!(!poll.timed());
        assert_eq!(
            poll.deadline(Some(&json!(4)), Some(&json!(5))),
            None,
            "it cannot be closed"
        );
        assert_eq!(
            serde_json::to_value(&poll).unwrap(),
            old,
            "and is rewritten as it was"
        );

        let with = |key: &str, value: Value| {
            let mut v = old.clone();
            v[key] = value;
            Poll::parse(&v)
        };
        let poll = with("showVoters", json!(true)).unwrap();
        assert_eq!(poll.show_voters, Some(true));
        assert!(
            with("showVoters", json!("yes")).is_none(),
            "a typed boolean"
        );
        assert!(with("showVoters", json!(1)).is_none());
        assert!(
            with("closesIn", json!(3_600_000)).is_none(),
            "a duration needs a timed poll"
        );
        let timed = |closes_in: Value| {
            let mut v = old.clone();
            v["showVoters"] = json!(false);
            v["closesIn"] = closes_in;
            Poll::parse(&v)
        };
        assert!(timed(json!(3_600_000)).is_some());
        assert!(timed(json!(DURATION_MAX_MS)).is_some());
        assert!(timed(json!(0)).is_none(), "not a duration");
        assert!(timed(json!(-5)).is_none());
        assert!(timed(json!(DURATION_MAX_MS + 1)).is_none(), "over a week");
        assert!(timed(json!("soon")).is_none());
        assert!(timed(json!(1.5)).is_none());
        assert!(with("closesAt", json!(5)).is_none(), "an unknown field");

        // The deadline is the earlier of the row's closing time and the manual close; one
        // that is not a time is none.
        let timed = timed(json!(3_600_000)).unwrap();
        let at =
            |closes: Value, closed: Option<Value>| timed.deadline(Some(&closes), closed.as_ref());
        assert_eq!(at(json!(100), None), Some(100));
        assert_eq!(at(json!(100), Some(json!(40))), Some(40));
        assert_eq!(at(json!(100), Some(json!(400))), Some(100));
        assert_eq!(at(json!(100), Some(json!("now"))), Some(100));
        assert_eq!(at(json!("soon"), None), None);
        let open = Poll::new("Q", &labels(2), false, true, None).unwrap();
        assert_eq!(open.deadline(None, None), None);
        assert_eq!(open.deadline(None, Some(&json!(7))), Some(7));
        assert_eq!(
            open.deadline(Some(&json!(3)), None),
            None,
            "no duration, no closing time"
        );
        // One rule for "closed": at the deadline and after it, never without one.
        assert!(!closed(Some(100), 99) && closed(Some(100), 100) && closed(Some(100), 101));
        assert!(!closed(None, i64::MAX));
    }

    /// A composed poll's duration is bounded where the poll is made, for every composer.
    #[test]
    fn a_composed_poll_runs_at_most_a_week() {
        let hour = Poll::new("Q", &labels(2), false, false, Some(3_600_000)).unwrap();
        assert_eq!(
            hour.closes_in,
            Some(3_600_000),
            "what was picked, not a time"
        );
        assert!(Poll::new("Q", &labels(2), false, false, Some(DURATION_MAX_MS)).is_some());
        assert!(Poll::new("Q", &labels(2), false, false, Some(DURATION_MAX_MS + 1)).is_none());
        assert!(Poll::new("Q", &labels(2), false, false, Some(0)).is_none());
        assert!(Poll::new("Q", &labels(2), false, false, Some(-1)).is_none());
        // The older shape, for a relay that does not take the new fields: as it was stored
        // before them, field for field.
        let older = Poll::older(" Q ", &labels(2), true).unwrap();
        assert!(!older.timed() && older.max == 2);
        let stored = serde_json::to_value(&older).unwrap();
        assert!(stored.get("showVoters").is_none() && stored.get("closesIn").is_none());
        assert!(Poll::older(" ", &labels(2), false).is_none());
    }

    #[test]
    fn the_foot_says_whose_poll_how_many_voted_and_whether_it_is_open() {
        assert_eq!(
            foot(None, 0, false, false, false, None),
            "Your poll · 0 voted · pick one"
        );
        assert_eq!(
            foot(Some("Ana"), 3, true, true, false, Some(2 * 3_600_000 + 5)),
            "Ana's poll · 3 voted · pick any · names shown · closes in 2 h"
        );
        assert_eq!(
            foot(None, 1, false, false, false, Some(30_000)),
            "Your poll · 1 voted · pick one · closes in 1 min"
        );
        assert_eq!(
            foot(None, 1, false, false, false, Some(7 * 24 * 3_600_000)),
            "Your poll · 1 voted · pick one · closes in 7 days"
        );
        assert_eq!(
            foot(Some("Ana"), 3, true, false, true, Some(-5)),
            "Ana's poll · 3 voted · closed"
        );
    }

    #[test]
    fn a_vote_names_known_options_once_and_at_most_max() {
        let poll = Poll::new("Q", &labels(3), false, false, None).unwrap();
        assert_eq!(poll.selections(&json!(["2"])), Some(vec!["2".to_owned()]));
        assert_eq!(poll.selections(&json!([])), Some(vec![]), "a retraction");
        assert!(poll.selections(&json!(["1", "2"])).is_none(), "over max");
        assert!(poll.selections(&json!(["9"])).is_none(), "unknown option");
        assert!(poll.selections(&json!("1")).is_none(), "not a list");
        assert!(poll.selections(&json!([1])).is_none(), "not strings");

        let many = Poll::new("Q", &labels(3), true, false, None).unwrap();
        assert!(many.selections(&json!(["1", "3"])).is_some());
        assert!(many.selections(&json!(["1", "1"])).is_none(), "a repeat");
    }
}
