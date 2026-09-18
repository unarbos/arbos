//! M-03's follow-up: cost on interrupted turns. A turn the user stopped
//! closed with no usage at all, so the dollars its model calls had
//! already cost never reached `spend.toml`, the card, or the cap. Now the
//! `turn_complete` of a stopped turn carries what the completed steps
//! cost, the spend total takes it, and the day's total stands beside the
//! running one.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::path::Path;
use std::time::Duration;

fn transcript(place: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[test]
fn a_stopped_turn_still_books_what_its_steps_cost() {
    let replies = concat!(
        // Step 1 costs $0.30 and starts a long command; the user stops it
        // before a second step is asked for.
        "{\"agent\":\"root\",\"content\":\"working\",\"cost\":0.30,\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 30\",\"description\":\"Long work\"}}]}\n",
        // A second, whole turn: $0.10.
        "{\"agent\":\"root\",\"content\":\"quick\",\"cost\":0.10}\n",
    );
    let k = start_kernel_replay_prepared("interrupted-cost", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"worker\"\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(serde_json::json!({"type":"user","agent":"root","text":"do the long thing","attachments":[]}));
    assert!(a.wait_turn("root", "running", Duration::from_secs(10)));
    // The step has been paid for once its tool is running.
    assert!(
        a.wait(Duration::from_secs(20), |f| f["type"] == "event"
            && f["event"]["kind"] == "tool"
            && f["event"]["name"] == "bash")
            .is_some(),
        "the bash call started"
    );
    a.send(serde_json::json!({"type":"stop","agent":"root"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(20)));

    let events = transcript(&k.place);
    assert!(
        events.iter().any(|e| e["kind"] == "interrupted"),
        "the turn was interrupted: {events:?}"
    );
    let done = events
        .iter()
        .rev()
        .find(|e| e["kind"] == "turn_complete")
        .expect("turn_complete");
    let cost = done["usage"]["cost"].as_f64();
    assert_eq!(
        cost,
        Some(0.30),
        "the stopped turn says what it spent: {done}"
    );

    let spend = std::fs::read_to_string(k.place.join(".arbos/spend.toml")).unwrap_or_default();
    assert!(
        spend.contains("spent_usd = 0.3"),
        "booked in the total: {spend}"
    );
    assert!(spend.contains("today_usd = 0.3"), "and in today's: {spend}");
    assert!(spend.contains("turns = 1"), "{spend}");

    // A whole turn adds on.
    a.send(
        serde_json::json!({"type":"user","agent":"root","text":"and a quick one","attachments":[]}),
    );
    assert!(a.wait_turn("root", "idle", Duration::from_secs(20)));
    assert!(
        common::wait_for(Duration::from_secs(10), || {
            std::fs::read_to_string(k.place.join(".arbos/spend.toml"))
                .unwrap_or_default()
                .contains("turns = 2")
        }),
        "the second turn is booked"
    );
    let spend = std::fs::read_to_string(k.place.join(".arbos/spend.toml")).unwrap();
    assert!(
        spend.contains("spent_usd = 0.4") && spend.contains("today_usd = 0.4"),
        "{spend}"
    );
}
