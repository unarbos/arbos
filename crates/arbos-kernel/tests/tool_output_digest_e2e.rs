//! F14 (the phone): the `tool` record carried name, args, paths, error and
//! timings — the full `body` too, but a small screen wants a glance. Each
//! record now has `output`: the first ~400 characters, or head and tail
//! of a long run, the error first. Live, and filled in on replay for
//! records written before the field existed.

mod common;

use common::{Attach, start_kernel_replay};
use std::time::Duration;

#[test]
fn a_tool_record_carries_a_glance_at_its_output_live_and_on_replay() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"counting\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"seq 1 300\",\"description\":\"Count\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"Counted.\"}\n",
    );
    let mut k = start_kernel_replay("tool-output-digest", replies);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(
        serde_json::json!({"type":"user","agent":"root","text":"count to 300","attachments":[]}),
    );
    let live = a
        .wait(Duration::from_secs(15), |f| {
            f["type"] == "event" && f["event"]["kind"] == "tool" && f["event"]["ended"].is_number()
        })
        .expect("the finished bash record");
    let output = live["event"]["output"]
        .as_str()
        .expect("output on the live record");
    assert!(output.starts_with("1\n2\n3"), "{output}");
    assert!(output.trim_end().ends_with("299\n300"), "{output}");
    assert!(output.contains("\n…\n"), "{output}");
    assert!(output.chars().count() <= 420, "{}", output.chars().count());
    let body = live["event"]["body"].as_str().unwrap();
    assert!(body.contains("150\n"), "the full body is still there");

    // A record from before the field: strip `output` on disk, reattach,
    // and the replay fills it in.
    let path = k.place.join(".arbos/agents/root/transcript.jsonl");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !std::fs::read_to_string(&path)
        .unwrap()
        .contains("turn_complete")
    {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(50));
    }
    let stripped: Vec<String> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|l| {
            let mut v: serde_json::Value = serde_json::from_str(l).unwrap();
            if v["kind"] == "tool" {
                v.as_object_mut().unwrap().remove("output");
            }
            v.to_string()
        })
        .collect();
    std::fs::write(&path, stripped.join("\n") + "\n").unwrap();
    let mut b = Attach::connect(&k.url);
    let replayed = b
        .wait(Duration::from_secs(10), |f| {
            f["type"] == "replayed" && f["event"]["kind"] == "tool"
        })
        .expect("the replayed record");
    let output = replayed["event"]["output"]
        .as_str()
        .expect("output filled on replay");
    assert!(
        output.starts_with("1\n2\n3") && output.contains("\n…\n"),
        "{output}"
    );
    let _ = k.child.kill();
}
