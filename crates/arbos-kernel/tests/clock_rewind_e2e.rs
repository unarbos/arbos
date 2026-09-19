//! qal-j38: a clock set backwards leaves every subscription's `next_due`
//! in the future by the size of the jump, and a scheduler that only pulls
//! in the overdue never touches them — the kernel's own weekly `git gc`
//! chore included. A due time further ahead than the schedule can produce
//! now fires once, with the reason, and keeps its cadence from there.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::time::Duration;

fn sub_text(place: &std::path::Path, name: &str) -> String {
    std::fs::read_to_string(place.join(".arbos/agents/root/subscriptions").join(name))
        .unwrap_or_default()
}

fn next_due_ms(text: &str) -> i64 {
    let line = text
        .lines()
        .find(|l| l.starts_with("next_due"))
        .unwrap_or_default();
    let stamp = line.split('"').nth(1).unwrap_or_default();
    arbos_core::parse_instant_ms(stamp).unwrap_or(0)
}

#[test]
fn a_subscription_ten_days_ahead_on_a_thirty_second_period_fires_once_and_is_pulled_back() {
    let day = 86_400_000i64;
    let now = arbos_core::now_ms();
    let k = start_kernel_replay_prepared("clock-rewind", "", "", |place| {
        let dir = place.join(".arbos/agents/root");
        std::fs::create_dir_all(dir.join("subscriptions")).unwrap();
        arbos_core::Agent::root("root").save(&dir).unwrap();
        // The clock moved back ten days after these were scheduled: one is
        // ten days ahead (the stranded one), the other ten days overdue
        // (the coalescing path, unchanged). Each command leaves a mark.
        for (id, name, due) in [
            (1, "0001-ahead.toml", now + 10 * day),
            (2, "0002-behind.toml", now - 10 * day),
        ] {
            let mark = place.join(format!("fired-{id}"));
            std::fs::write(
                dir.join("subscriptions").join(name),
                format!(
                    "id = {id}\nkind = \"shell\"\nprompt = \"mark\"\ncmd = \"echo x >> {}\"\nevery = \"30s\"\ndeliver_to = \"none\"\ncreated = \"2026-09-01T00:00:00Z\"\nnext_due = \"{}\"\n",
                    mark.display(),
                    arbos_core::inbox::rfc3339(due)
                ),
            )
            .unwrap();
        }
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    // Both fire within the first sweeps: the overdue one as before, the
    // one ten days ahead because its due time was read as a rewound clock.
    for id in [1, 2] {
        assert!(
            common::wait_for(Duration::from_secs(20), || k
                .place
                .join(format!("fired-{id}"))
                .exists()),
            "#{id} fired"
        );
    }
    // One firing each, not a storm: give a second sweep time to pass.
    std::thread::sleep(Duration::from_secs(3));
    for id in [1, 2] {
        let marks = std::fs::read_to_string(k.place.join(format!("fired-{id}")))
            .unwrap_or_default()
            .lines()
            .count();
        assert_eq!(marks, 1, "#{id} fired once");
    }
    // Both due times are now one period out from a real `now`, not ten
    // days away in either direction.
    let after = arbos_core::now_ms();
    for name in ["0001-ahead.toml", "0002-behind.toml"] {
        let text = sub_text(&k.place, name);
        let due = next_due_ms(&text);
        assert!(
            due > after - 60_000 && due <= after + 31_000,
            "{name}: next_due pulled to one period out, got {} (now {}):\n{text}",
            arbos_core::inbox::rfc3339(due),
            arbos_core::inbox::rfc3339(after)
        );
    }
    // The reason is in the log.
    let log =
        std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.log")).unwrap_or_default();
    assert!(log.contains("subscription_rewound"), "{log}");
    assert!(
        log.contains("the clock moved back"),
        "the log says why: {log}"
    );
}
