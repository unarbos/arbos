//! The grep index was built once, at start, and never learned of a file
//! written after it: `write src/new.rs "fn zebra() {}"`, `edit a.txt` to
//! hold zebra, `echo zebra > shell.txt` in bash — and `grep zebra` said
//! "(no matches)" (probe on main 249ddb5f). Now a tool that may have
//! changed the tree marks the index stale, grep walks meanwhile, and a
//! rebuild lands in the background; a second grep a moment later comes
//! from the fresh index.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::time::Duration;

const REPLIES: &str = concat!(
    "{\"agent\":\"root\",\"content\":\"before\",\"calls\":[{\"name\":\"grep\",\"arguments\":{\"pattern\":\"zebra\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"three writes\",\"calls\":[{\"name\":\"write\",\"arguments\":{\"path\":\"src/new.rs\",\"content\":\"fn zebra() {}\\n\"}},{\"name\":\"edit\",\"arguments\":{\"path\":\"a.txt\",\"old_string\":\"hello\",\"new_string\":\"hello zebra\"}},{\"name\":\"bash\",\"arguments\":{\"command\":\"echo 'zebra in shell file' > shell.txt\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"after, at once\",\"calls\":[{\"name\":\"grep\",\"arguments\":{\"pattern\":\"zebra\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"after, settled\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 3\"}},{\"name\":\"grep\",\"arguments\":{\"pattern\":\"zebra\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"done\"}\n",
);

#[test]
fn grep_finds_what_the_turn_just_wrote() {
    let mut k = start_kernel_replay_prepared("grep-follows-writes", REPLIES, "", |place| {
        std::fs::write(place.join("a.txt"), "hello\n").unwrap();
        // Root needs bash: an old-style place.
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\nname = \"a\"\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    // Let the first index land so the "before" grep is the indexed path.
    std::thread::sleep(Duration::from_millis(1500));
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "find zebra"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(60)));
    let root: Vec<serde_json::Value> =
        std::fs::read_to_string(k.place.join(".arbos/agents/root/transcript.jsonl"))
            .unwrap()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
    let greps: Vec<&str> = root
        .iter()
        .filter(|e| e["kind"] == "tool" && e["name"] == "grep")
        .map(|e| e["body"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(greps.len(), 3, "{root:#?}");
    assert!(greps[0].contains("(no matches)"), "before: {}", greps[0]);
    // Control on main 249ddb5f: "(no matches)" for both greps after.
    for (i, body) in greps[1..].iter().enumerate() {
        for want in ["src/new.rs", "a.txt", "shell.txt"] {
            assert!(
                body.contains(want),
                "grep #{}: {want} missing: {body}",
                i + 2
            );
        }
    }
    let _ = k.child.kill();
}
