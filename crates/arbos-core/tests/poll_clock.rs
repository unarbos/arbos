//! The poll clock is one value for the process, so its check runs in a process of its own.

use arbos_core::{ids::now_ms, poll};

/// A device judges a poll by the relay's clock once it has heard it: half an hour slow or a
/// day fast, `poll::now()` is the relay's time.
#[test]
fn the_poll_clock_follows_the_relay() {
    for ahead in [30 * 60_000, -24 * 3_600_000, 0] {
        poll::set_relay_now(now_ms() + ahead);
        let off = poll::now() - now_ms() - ahead;
        assert!(off.abs() < 1_000, "{ahead}: {off}");
    }
}
