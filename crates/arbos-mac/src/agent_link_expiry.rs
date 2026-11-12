/// How long a new link lives. The spec's own set, with no "Never": an
/// unbounded grant is not a thing this screen can make
/// (`spec` §5.3 — 1 hour minimum, 30 days maximum, 7 days by default).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Expiry {
    pub hours: i64,
    pub label: &'static str,
}

pub const EXPIRIES: [Expiry; 4] = [
    Expiry {
        hours: 1,
        label: "1 hour",
    },
    Expiry {
        hours: 24,
        label: "24 hours",
    },
    Expiry {
        hours: 7 * 24,
        label: "7 days",
    },
    Expiry {
        hours: 30 * 24,
        label: "30 days",
    },
];

/// The default: the job this feature was asked for is a session, not a
/// standing integration (that is what outside agents already are).
pub const EXPIRY_DEFAULT_HOURS: i64 = 7 * 24;

/// "7 days" for the chosen hours, for the copy sheet's one-line restatement.
pub fn expiry_label(hours: i64) -> &'static str {
    EXPIRIES
        .iter()
        .find(|e| e.hours == hours)
        .map(|e| e.label)
        .unwrap_or("7 days")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_and_default_keep_the_shipped_contract() {
        assert_eq!(
            EXPIRIES.map(|e| (e.hours, e.label)),
            [
                (1, "1 hour"),
                (24, "24 hours"),
                (168, "7 days"),
                (720, "30 days")
            ]
        );
        assert_eq!(EXPIRY_DEFAULT_HOURS, 168);
        for choice in EXPIRIES {
            assert_eq!(expiry_label(choice.hours), choice.label);
        }
    }

    #[test]
    fn unknown_hours_keep_the_existing_default_label() {
        for hours in [i64::MIN, -1, 0, 2, 169, 721, i64::MAX] {
            assert_eq!(expiry_label(hours), "7 days");
        }
    }
}
