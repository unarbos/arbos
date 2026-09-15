//! Process parity, slice 11: the tool parameters Cursor's coordinator has
//! and Arbos lacked. `agents` (GetAgentStatus) and `transcript`
//! (ReadAgentTranscript) on a coordinator's workers; `edit replace_all`,
//! `delete`, `grep` flags, `find sort=mtime`.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::path::Path;
use std::time::Duration;

fn transcript(place: &Path, agent: &str) -> Vec<serde_json::Value> {
    std::fs::read_to_string(
        place
            .join(".arbos/agents")
            .join(agent)
            .join("transcript.jsonl"),
    )
    .unwrap_or_default()
    .lines()
    .filter_map(|l| serde_json::from_str(l).ok())
    .collect()
}

fn tool_bodies(t: &[serde_json::Value], name: &str) -> Vec<String> {
    t.iter()
        .filter(|e| e["kind"] == "tool" && e["name"] == name)
        .map(|e| {
            e["body"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| e["error"].as_str().unwrap_or("").to_string())
        })
        .collect()
}

#[test]
fn agents_and_transcript_show_a_workers_state_without_waiting() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"two workers\",\"calls\":[",
        "{\"name\":\"spawn\",\"arguments\":{\"name\":\"Count slowly\",\"task\":\"count\"}},",
        "{\"name\":\"spawn\",\"arguments\":{\"name\":\"Say the word\",\"task\":\"say it\"}}",
        "]}\n",
        "{\"agent\":\"root\",\"content\":\"started\"}\n",
        "{\"agent\":\"count-slowly\",\"content\":\"counting\",\"calls\":[{\"name\":\"status\",\"arguments\":{\"step\":\"Counting to ten\"}},{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 15\",\"wait_ms\":30000}}]}\n",
        "{\"agent\":\"count-slowly\",\"content\":\"ten\"}\n",
        "{\"agent\":\"say-the-word\",\"content\":\"the word is xylophone\"}\n",
        // Root hears say-the-word's done, then looks at both.
        "{\"agent\":\"root\",\"content\":\"looking\",\"calls\":[{\"name\":\"agents\",\"arguments\":{}},{\"name\":\"transcript\",\"arguments\":{\"agent\":\"say-the-word\"}},{\"name\":\"transcript\",\"arguments\":{\"agent\":\"say-the-word\",\"mode\":\"full\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"noted\"}\n",
        "{\"agent\":\"root\",\"content\":\"noted the other\"}\n",
    );
    let mut k = start_kernel_replay_prepared("tool-parity-agents", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"coordinator\"\narchive_children = false\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "count and say the word"}));
    assert!(
        common::wait_for(Duration::from_secs(40), || {
            !tool_bodies(&transcript(&k.place, "root"), "agents").is_empty()
        }),
        "root looked at its workers: {:?}",
        transcript(&k.place, "root")
    );
    let root = transcript(&k.place, "root");
    let agents = &tool_bodies(&root, "agents")[0];
    assert!(
        agents.contains("count-slowly (Count slowly) — running, now: Counting to ten"),
        "{agents}"
    );
    assert!(
        agents.contains("say-the-word (Say the word) — idle; last turn success"),
        "{agents}"
    );
    assert!(agents.contains("the word is xylophone"), "{agents}");
    let ts = tool_bodies(&root, "transcript");
    assert_eq!(ts.len(), 2, "{ts:?}");
    let tail = &ts[0];
    assert!(
        tail.starts_with("say-the-word: last 1 turn(s) of 1"),
        "{tail}"
    );
    assert!(
        tail.contains("== turn (") && tail.contains("agent: the word is xylophone"),
        "{tail}"
    );
    assert!(tail.contains("-- turn complete"), "{tail}");
    let full = &ts[1];
    let path = k
        .place
        .join(".arbos/agents/root/results/transcript-say-the-word.txt");
    assert!(
        full.starts_with(&path.display().to_string()),
        "the file's path leads: {full}"
    );
    assert!(path.is_file());
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("the word is xylophone")
    );
    let _ = k.child.kill();
}

#[test]
fn a_peer_transcript_is_refused_and_a_worker_may_read_its_own() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"two workers\",\"calls\":[",
        "{\"name\":\"spawn\",\"arguments\":{\"name\":\"Worker a\",\"task\":\"a\"}},",
        "{\"name\":\"spawn\",\"arguments\":{\"name\":\"Worker b\",\"task\":\"b\"}}",
        "]}\n",
        "{\"agent\":\"root\",\"content\":\"started\"}\n",
        "{\"agent\":\"worker-a\",\"content\":\"peeking\",\"calls\":[{\"name\":\"transcript\",\"arguments\":{\"agent\":\"worker-b\"}},{\"name\":\"transcript\",\"arguments\":{\"agent\":\"worker-a\"}}]}\n",
        "{\"agent\":\"worker-a\",\"content\":\"done a\"}\n",
        "{\"agent\":\"worker-b\",\"content\":\"done b\"}\n",
        "{\"agent\":\"root\",\"content\":\"noted\"}\n",
        "{\"agent\":\"root\",\"content\":\"noted\"}\n",
    );
    let mut k = start_kernel_replay_prepared("tool-parity-peer", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"coordinator\"\narchive_children = false\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "go"}));
    assert!(a.wait_turn("worker-a", "idle", Duration::from_secs(40)));
    let wa = transcript(&k.place, "worker-a");
    let calls: Vec<&serde_json::Value> = wa
        .iter()
        .filter(|e| e["kind"] == "tool" && e["name"] == "transcript")
        .collect();
    assert_eq!(calls.len(), 2, "{wa:?}");
    assert!(
        calls[0]["error"]
            .as_str()
            .unwrap_or("")
            .contains("not a worker of yours"),
        "a peer's transcript is theirs: {:#?}",
        calls[0]
    );
    assert!(
        calls[1].get("error").is_none(),
        "one's own is fine: {:#?}",
        calls[1]
    );
    let _ = k.child.kill();
}

#[test]
fn edit_replace_all_delete_grep_flags_and_find_by_mtime() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"editing\",\"calls\":[",
        "{\"name\":\"edit\",\"arguments\":{\"path\":\"colours.txt\",\"old_string\":\"colour\",\"new_string\":\"color\",\"replace_all\":true}}",
        "]}\n",
        "{\"agent\":\"root\",\"content\":\"searching\",\"calls\":[",
        "{\"name\":\"grep\",\"arguments\":{\"pattern\":\"COLOR\",\"ignore_case\":true,\"mode\":\"count\"}},",
        "{\"name\":\"grep\",\"arguments\":{\"pattern\":\"middle\",\"context\":1}},",
        "{\"name\":\"grep\",\"arguments\":{\"pattern\":\"middle\",\"mode\":\"files\"}},",
        "{\"name\":\"find\",\"arguments\":{\"pattern\":\"*.txt\",\"sort\":\"mtime\"}},",
        "{\"name\":\"delete\",\"arguments\":{\"path\":\"old.txt\"}}",
        "]}\n",
        "{\"agent\":\"root\",\"content\":\"Done.\"}\n",
    );
    // A plain root (no coordinator role): the coordinator's writes reach
    // only the store, and this is about the file tools themselves.
    let mut k = start_kernel_replay_prepared("tool-parity-files", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(place.join(".arbos/project.toml"), "schema = 2\n").unwrap();
        std::fs::write(place.join("old.txt"), "old\n").unwrap();
        std::thread::sleep(Duration::from_millis(1100));
        std::fs::write(
            place.join("colours.txt"),
            "one colour\ntwo colour\nmiddle line\nthree colour\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "americanise and tidy"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(40)));
    let text = std::fs::read_to_string(k.place.join("colours.txt")).unwrap();
    assert_eq!(text, "one color\ntwo color\nmiddle line\nthree color\n");
    let root = transcript(&k.place, "root");
    let edit = &tool_bodies(&root, "edit")[0];
    assert!(edit.contains("3 occurrence(s) replaced"), "{edit}");
    let greps = tool_bodies(&root, "grep");
    assert!(
        greps[0].contains("colours.txt:3"),
        "count, case-insensitive: {:?}",
        greps
    );
    assert!(
        greps[1].contains("colours.txt-2-two color")
            && greps[1].contains("colours.txt:3:middle line")
            && greps[1].contains("colours.txt-4-three color"),
        "context lines: {:?}",
        greps
    );
    assert_eq!(greps[2].trim(), "colours.txt", "files mode: {:?}", greps);
    let find = &tool_bodies(&root, "find")[0];
    let lines: Vec<&str> = find.lines().collect();
    assert_eq!(lines[0], "colours.txt", "newest first: {find}");
    assert_eq!(lines[1], "old.txt", "{find}");
    assert!(!k.place.join("old.txt").exists(), "deleted");
    let del = &tool_bodies(&root, "delete")[0];
    assert!(del.starts_with("deleted "), "{del}");
    let _ = k.child.kill();
}
