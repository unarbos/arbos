//! When a transcript draws a centred time divider ("Today 7:29") — one rule,
//! both clients.
//!
//! Jacob, 2026-11-05 (Arbonites, `01M6P64SJJ1PC20ETWHM20FZBP`): a divider
//! between nearly every message group is noise, in a quiet chat most of all.
//! One lands when the local day changes, or when an hour or more passed since
//! the message before it. Nothing else draws one. Each message's own time is
//! still there to ask for — the Mac's hover rail, the iPhone's drag-to-reveal
//! (`docs/mac-design-rules.md` § Message timestamps) — it is no longer a
//! permanent line through the conversation.
//!
//! The rule lives in core because the two clients each had their own, and the
//! two had already drifted: the Mac measured the hour from the **last divider
//! it drew**, so a chat whose messages are minutes apart grew one every hour
//! mid-conversation; the iPhone measured it from the previous message; and
//! neither looked at the day at all, so 23:58 → 00:04 drew nothing. One
//! question deserves one answer.

/// A divider lands when a message is this far from the one before it.
pub const DIVIDER_GAP_MS: u64 = 60 * 60 * 1000;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// The local day a UTC millisecond stamp falls in, counted in days from the
/// epoch. `utc_offset_seconds` is the reader's offset from UTC, east positive.
fn local_day(at_ms: u64, utc_offset_seconds: i32) -> i64 {
    (at_ms as i64 + utc_offset_seconds as i64 * 1000).div_euclid(DAY_MS)
}

/// Whether the message at `at_ms` opens with a time divider, given the clock of
/// the message before it (`previous_ms`; 0 when there is none).
///
/// - A row with no clock (`at_ms` 0) never draws one: it has no time to name.
/// - The first message of a chat draws one. Its day is new, and a transcript
///   has to say which day the reader is looking at.
/// - After that: a different local day, or [`DIVIDER_GAP_MS`] or more between
///   the two messages — measured in either direction, because a row can arrive
///   with an earlier clock than the one above it.
///
/// `utc_offset_seconds` covers both stamps (Swift
/// `TimeZone.current.secondsFromGMT(for:)`, Rust chrono `Local`). A zone that
/// steps its clock across midnight can therefore cost one divider on that one
/// night; messages a DST step apart are already an hour apart, which draws one
/// on the gap.
pub fn shows_divider(previous_ms: u64, at_ms: u64, utc_offset_seconds: i32) -> bool {
    if at_ms == 0 {
        return false;
    }
    if previous_ms == 0 {
        return true;
    }
    at_ms.abs_diff(previous_ms) >= DIVIDER_GAP_MS
        || local_day(at_ms, utc_offset_seconds) != local_day(previous_ms, utc_offset_seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-11-05 12:00:00 UTC, far from any midnight in any zone.
    const NOON: u64 = 1_793_880_000_000;
    /// 2026-11-05 23:50:00 UTC, ten minutes before UTC midnight.
    const LATE: u64 = 1_793_922_600_000;
    const MINUTE: u64 = 60_000;
    const HOUR: u64 = 60 * MINUTE;

    #[test]
    fn the_first_message_of_a_chat_names_its_day() {
        assert!(shows_divider(0, NOON, 0));
    }

    #[test]
    fn a_row_with_no_clock_draws_nothing() {
        assert!(!shows_divider(NOON, 0, 0));
        assert!(!shows_divider(0, 0, 0));
    }

    #[test]
    fn an_hour_exactly_draws_one_and_anything_under_it_does_not() {
        assert!(
            !shows_divider(NOON, NOON + HOUR - 1, 0),
            "a millisecond under"
        );
        assert!(shows_divider(NOON, NOON + HOUR, 0), "exactly an hour");
        assert!(
            shows_divider(NOON, NOON + HOUR + 1, 0),
            "a millisecond over"
        );
    }

    /// The symptom: Jacob's screenshot had a divider between nearly every
    /// group of a chat nobody was typing in quickly.
    #[test]
    fn a_quiet_chat_whose_messages_are_minutes_apart_draws_nothing() {
        let mut previous = NOON;
        for step in [7 * MINUTE, 11 * MINUTE, 4 * MINUTE, 23 * MINUTE, 9 * MINUTE] {
            let at = previous + step;
            assert!(
                !shows_divider(previous, at, 0),
                "{step} ms apart is one conversation"
            );
            previous = at;
        }
        // …and the hour is measured from the message above, not from the last
        // divider, so three hours of talking still draw none.
        assert!(!shows_divider(previous, previous + 54 * MINUTE, 0));
    }

    #[test]
    fn a_new_day_draws_one_however_short_the_gap() {
        let after_midnight = LATE + 14 * MINUTE;
        assert!(
            shows_divider(LATE, after_midnight, 0),
            "23:50 to 00:04 is a new day"
        );
        assert!(
            !shows_divider(LATE, LATE + 9 * MINUTE, 0),
            "23:50 to 23:59 is the same day"
        );
    }

    #[test]
    fn the_day_is_the_readers_day_and_not_utc() {
        let after_midnight = LATE + 14 * MINUTE;
        // New York (UTC−5) reads the same two stamps as 18:50 and 19:04 on one
        // evening, so it gets no divider where UTC gets one.
        assert!(!shows_divider(LATE, after_midnight, -5 * 3600));
        // Nepal (UTC+5:45) is ten minutes into the next day at 18:20 UTC,
        // where UTC itself is still on the same afternoon.
        let before = NOON + 6 * HOUR + 10 * MINUTE; // 18:10 UTC, 23:55 in Kathmandu
        let after = NOON + 6 * HOUR + 20 * MINUTE; // 18:20 UTC, 00:05 in Kathmandu
        assert!(shows_divider(before, after, 5 * 3600 + 45 * 60));
        assert!(!shows_divider(before, after, 0));
    }

    #[test]
    fn a_row_that_arrives_with_an_earlier_clock_is_still_an_hour_away() {
        assert!(shows_divider(NOON, NOON - 2 * HOUR, 0));
        assert!(!shows_divider(NOON, NOON - 5 * MINUTE, 0));
    }
}
