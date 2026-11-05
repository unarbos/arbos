//! The transcript's time-divider rule for Swift (`arbos_core::transcript`): the
//! iPhone asks the same function the Mac does, so the two cannot drift
//! ([feedback time-dividers-2026-11-05]).

use arbos_core::transcript as core;

/// Whether the message at `at_ms` opens with a centred time divider, given the
/// clock of the message above it (`previous_ms`; 0 when there is none). Both
/// clocks are `Message.createdAt` — milliseconds, 0 for a row without one; a
/// negative value is read as absent. `utc_offset_seconds` is the reader's
/// offset from UTC (`TimeZone.current.secondsFromGMT(for:)`), which decides
/// the day half of the rule.
#[uniffi::export]
pub fn shows_time_divider(previous_ms: i64, at_ms: i64, utc_offset_seconds: i32) -> bool {
    core::shows_divider(
        previous_ms.max(0) as u64,
        at_ms.max(0) as u64,
        utc_offset_seconds,
    )
}

/// The gap that draws a divider, so the phone states the hour nowhere of its own.
#[uniffi::export]
pub fn time_divider_gap_ms() -> i64 {
    core::DIVIDER_GAP_MS as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-11-05 12:00:00 UTC.
    const NOON: i64 = 1_793_880_000_000;

    #[test]
    fn the_phone_asks_the_same_rule_the_mac_does() {
        let hour = time_divider_gap_ms();
        assert_eq!(hour, 3_600_000);
        assert!(shows_time_divider(0, NOON, 0), "the chat's first message");
        assert!(!shows_time_divider(NOON, NOON + hour - 1, 0));
        assert!(shows_time_divider(NOON, NOON + hour, 0));
        assert!(!shows_time_divider(NOON, 0, 0), "a row with no clock");
        assert!(!shows_time_divider(NOON, -1, 0), "nor a negative one");
    }
}
