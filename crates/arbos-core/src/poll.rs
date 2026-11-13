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
}

/// `1..=max` characters once trimmed.
fn sized(s: &str, max: usize) -> bool {
    (1..=max).contains(&s.trim().chars().count())
}

impl Poll {
    /// A poll from what a composer collected, or `None` when it is out of bounds. Blank option
    /// rows are dropped; ids are the option's place in the list.
    pub fn new(question: &str, labels: &[String], multiple: bool) -> Option<Self> {
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
            && self.options.iter().enumerate().all(|(i, o)| {
                id_ok(&o.id)
                    && sized(&o.label, LABEL_MAX)
                    && self.options[..i].iter().all(|p| p.id != o.id)
            })
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
        assert_eq!(Poll::new("Q", &labels(3), true).unwrap().max, 3);

        assert!(Poll::new(" ", &labels(2), false).is_none());
        assert!(Poll::new(&"q".repeat(QUESTION_MAX + 1), &labels(2), false).is_none());
        assert!(Poll::new("Q", &labels(1), false).is_none());
        assert!(Poll::new("Q", &labels(OPTIONS_MAX + 1), false).is_none());
        assert!(Poll::new("Q", &["a".repeat(LABEL_MAX + 1), "b".into()], false).is_none());
        assert!(Poll::new("Q", &labels(OPTIONS_MAX), false).is_some());
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

    #[test]
    fn a_vote_names_known_options_once_and_at_most_max() {
        let poll = Poll::new("Q", &labels(3), false).unwrap();
        assert_eq!(poll.selections(&json!(["2"])), Some(vec!["2".to_owned()]));
        assert_eq!(poll.selections(&json!([])), Some(vec![]), "a retraction");
        assert!(poll.selections(&json!(["1", "2"])).is_none(), "over max");
        assert!(poll.selections(&json!(["9"])).is_none(), "unknown option");
        assert!(poll.selections(&json!("1")).is_none(), "not a list");
        assert!(poll.selections(&json!([1])).is_none(), "not strings");

        let many = Poll::new("Q", &labels(3), true).unwrap();
        assert!(many.selections(&json!(["1", "3"])).is_some());
        assert!(many.selections(&json!(["1", "1"])).is_none(), "a repeat");
    }
}
