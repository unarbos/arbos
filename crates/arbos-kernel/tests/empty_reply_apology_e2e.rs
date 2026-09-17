//! Jacob's phone, 2026-09-16: a reply began "Sorry — empty reply on my
//! side, nothing blocking." — the model apologising for the empty step
//! the kernel had nudged it about. The nudge was for the model; the user
//! saw neither the blank nor the note. The apology is cut; the answer is
//! what the user reads.

mod common;

use common::{Attach, start_kernel_replay};
use std::time::Duration;

#[test]
fn an_apology_for_the_nudged_empty_reply_is_not_the_first_thing_the_user_reads() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"\"}\n",
        "{\"agent\":\"root\",\"content\":\"Sorry — empty reply on my side, nothing blocking. The build passes on both targets.\"}\n",
    );
    let mut k = start_kernel_replay("empty-reply-apology", replies);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(serde_json::json!({"type":"user","agent":"root","text":"Does the build pass?","attachments":[]}));
    let nudge = a
        .wait(Duration::from_secs(10), |f| {
            f["type"] == "event" && f["event"]["kind"] == "nudge"
        })
        .expect("the empty reply is nudged");
    assert_eq!(nudge["event"]["reason"], "empty reply");
    assert!(
        nudge["event"]["text"]
            .as_str()
            .unwrap()
            .contains("do not mention the empty reply"),
        "{nudge}"
    );
    let reply = a
        .wait(Duration::from_secs(10), |f| {
            f["type"] == "event"
                && f["event"]["kind"] == "assistant"
                && !f["event"]["text"].as_str().unwrap_or("").is_empty()
        })
        .expect("the answer");
    assert_eq!(reply["event"]["text"], "The build passes on both targets.");
    let transcript =
        std::fs::read_to_string(k.place.join(".arbos/agents/root/transcript.jsonl")).unwrap();
    assert!(!transcript.contains("Sorry — empty reply"), "{transcript}");
    let _ = k.child.kill();
}
