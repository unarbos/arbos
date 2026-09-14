//! T3-08: `continuity = true` — a shell subscription's next firing carries
//! what the command printed last time; a timer's carries the last words
//! of the turn it opened.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::path::Path;
use std::time::{Duration, Instant};

fn transcript_text(place: &Path) -> String {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl")).unwrap_or_default()
}

fn wait_for(timeout: Duration, mut ok: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    ok()
}

/// Two hand-written subscriptions, both past due so they fire at start:
/// a shell monitor with continuity and one without.
fn prepare(place: &Path) {
    std::fs::create_dir_all(place.join(".arbos/agents/root/subscriptions")).unwrap();
    std::fs::write(
        place.join(".arbos/project.toml"),
        "schema = 2\nname = \"c\"\n",
    )
    .unwrap();
    // The command prints a counter it bumps each run, so two firings differ.
    let cmd = "n=$(cat count 2>/dev/null || echo 0); n=$((n+1)); echo $n > count; echo reading-$n";
    std::fs::write(
        place.join(".arbos/agents/root/subscriptions/0001-monitor.toml"),
        format!(
            "id = 1\nkind = \"shell\"\nprompt = \"compare the reading with last time\"\ncmd = \"{cmd}\"\nevery = \"30s\"\ndeliver_to = \"agent\"\ncontinuity = true\ncreated = \"2026-09-10T00:00:00Z\"\nnext_due = \"2026-09-10T00:00:00Z\"\n"
        ),
    )
    .unwrap();
}

#[test]
fn a_shell_subscription_with_continuity_carries_its_previous_output() {
    let replies = "{\"agent\":\"root\",\"content\":\"noted\"}\n{\"agent\":\"root\",\"content\":\"noted again\"}\n";
    let mut k = start_kernel_replay_prepared("continuity", replies, "", prepare);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    // First firing: output reading-1, nothing to compare with.
    assert!(
        wait_for(Duration::from_secs(30), || transcript_text(&k.place)
            .contains("reading-1")),
        "{}",
        transcript_text(&k.place)
    );
    let first = transcript_text(&k.place);
    assert!(!first.contains("Last time it printed"), "{first}");
    // Second firing (30 s): output reading-2, and last time's reading-1.
    assert!(
        wait_for(Duration::from_secs(70), || transcript_text(&k.place)
            .contains("reading-2")),
        "{}",
        transcript_text(&k.place)
    );
    let second = transcript_text(&k.place);
    assert!(
        second.contains("Last time it printed:\\nreading-1"),
        "the previous output rides along: {second}"
    );
    let file = std::fs::read_to_string(
        k.place
            .join(".arbos/agents/root/subscriptions/0001-monitor.toml"),
    )
    .unwrap();
    assert!(file.contains("seen = \"reading-2\""), "{file}");
    let _ = k.child.kill();
}

#[test]
fn a_timer_with_continuity_carries_the_last_words_of_its_previous_turn() {
    let replies = "{\"agent\":\"root\",\"content\":\"the count was 41\"}\n{\"agent\":\"root\",\"content\":\"now 42\"}\n";
    let mut k = start_kernel_replay_prepared("continuity-timer", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos/agents/root/subscriptions")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\nname = \"c\"\n",
        )
        .unwrap();
        std::fs::write(
            place.join(".arbos/agents/root/subscriptions/0002-count.toml"),
            "id = 2\nkind = \"timer\"\nprompt = \"report the count\"\nevery = \"30s\"\ndeliver_to = \"agent\"\ncontinuity = true\ncreated = \"2026-09-10T00:00:00Z\"\nnext_due = \"2026-09-10T00:00:00Z\"\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    assert!(wait_for(Duration::from_secs(30), || transcript_text(
        &k.place
    )
    .contains("the count was 41")));
    assert!(
        wait_for(Duration::from_secs(70), || {
            transcript_text(&k.place)
                .contains("Last time this fired, your turn ended with:\\nthe count was 41")
        }),
        "{}",
        transcript_text(&k.place)
    );
    let _ = k.child.kill();
}
