//! Driven with the transcript made unwritable mid-turn (the store
//! read-only, the disk full): what the kernel did with a person's words
//! and what it said about it.
//!
//! Before: the steer's inbox file was deleted before the transcript held
//! its words; a turn that ended on the failed append was only a log line
//! (no notice, no `turn_complete`), and its folder closed as
//! `verdict = "success"`, `outcome = "(no reply)"` — a turn that never
//! ran, recorded as a success, and the words gone with nothing said.
//!
//! Now: the words are still on disk (in the inbox, or as the cause of the
//! turn that could not write them); the turn folder says failed and why;
//! a failed notice reaches the attached window live even though the
//! transcript cannot take it, and tells the person to send again.

mod common;

use common::{Attach, start_kernel_replay};
use std::time::Duration;

fn transcript(place: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Every file under the agent's folder holding the words, by path.
fn files_holding(place: &std::path::Path, words: &str) -> Vec<String> {
    fn walk(dir: &std::path::Path, words: &str, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, words, out);
            } else if std::fs::read_to_string(&p).is_ok_and(|t| t.contains(words)) {
                out.push(p.display().to_string());
            }
        }
    }
    let mut out = Vec::new();
    walk(&place.join(".arbos/agents/root"), words, &mut out);
    out.sort();
    out
}

fn turn_metas(place: &std::path::Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(place.join(".arbos/agents/root/turns"))
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| std::fs::read_to_string(e.path().join("meta.toml")).ok())
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

#[cfg(unix)]
fn chmod(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(unix)]
#[test]
fn words_the_transcript_cannot_take_are_kept_the_turn_says_failed_and_the_window_hears_it() {
    if unsafe { libc::geteuid() } == 0 {
        // root writes through 0444; the fault cannot be staged.
        return;
    }
    const WORDS: &str = "ALSO-RENAME-THE-FILE";
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"running it\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"for i in $(seq 1 30); do echo tick $i; sleep 0.5; done\",\"description\":\"Slow script\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"Read it: ALSO-RENAME-THE-FILE.\"}\n",
        "{\"agent\":\"root\",\"content\":\"Read it: ALSO-RENAME-THE-FILE.\"}\n",
    );
    let k = start_kernel_replay("steer-kept", replies);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(serde_json::json!({"type":"user","agent":"root","text":"start it","attachments":[]}));
    assert!(a.wait_turn("root", "running", Duration::from_secs(10)));
    a.wait(Duration::from_secs(10), |f| {
        f["type"] == "job"
            && f["agent"] == "root"
            && f["delta"].as_str().is_some_and(|d| d.contains("tick"))
    })
    .expect("the command runs");

    // The store stops taking writes, and the person types.
    let transcript_path = k.place.join(".arbos/agents/root/transcript.jsonl");
    chmod(&transcript_path, 0o444);
    a.send(serde_json::json!({"type":"user","agent":"root","text":WORDS,"steer":true,"attachments":[]}));

    // The window hears, live, that the words did not land — the
    // transcript cannot carry the notice, so it comes as a frame.
    let notice = a
        .wait(Duration::from_secs(30), |f| {
            f["type"] == "event"
                && f["agent"] == "root"
                && f["event"]["kind"] == "notice"
                && f["event"]["failed"] == true
                && f["event"]["text"]
                    .as_str()
                    .is_some_and(|t| t.contains("did not reach the record") && t.contains(WORDS))
        })
        .expect("a failed notice names the words and says to send again");
    let text = notice["event"]["text"].as_str().unwrap();
    assert!(text.contains("send it again"), "{text}");

    // The words are still on disk, and nothing was written through the
    // read-only file.
    let holding = files_holding(&k.place, WORDS);
    assert!(
        holding
            .iter()
            .any(|p| p.contains("/inbox/") || p.ends_with("cause.md")),
        "the words are kept in the inbox or as a turn's cause: {holding:?}"
    );
    assert!(
        !transcript(&k.place)
            .iter()
            .any(|e| e["kind"] == "user" && e["text"] == WORDS),
        "nothing went through a read-only transcript"
    );

    // The turn that could not write is closed as failed, with the reason
    // — never "success (no reply)".
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let metas = loop {
        let metas = turn_metas(&k.place);
        let steer_turn = metas.iter().find(|m| m.contains("kind = \"steer\""));
        if steer_turn.is_some_and(|m| m.contains("\nended = "))
            || std::time::Instant::now() > deadline
        {
            break metas;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let steer_turn = metas
        .iter()
        .find(|m| m.contains("kind = \"steer\""))
        .expect("the steer started a turn folder");
    assert!(
        steer_turn.contains("verdict = \"failed\""),
        "a turn that wrote nothing is not a success:\n{steer_turn}"
    );
    assert!(
        steer_turn.contains("Permission denied")
            || steer_turn.contains("nothing reached the transcript"),
        "and says why:\n{steer_turn}"
    );
    assert!(
        !metas
            .iter()
            .any(|m| m.contains("verdict = \"success\"") && m.contains("outcome = \"(no reply)\"")),
        "no turn of this run is 'success (no reply)':\n{}",
        metas.join("\n---\n")
    );

    // The store takes writes again; the person sends again, as told.
    chmod(&transcript_path, 0o644);
    a.send(serde_json::json!({"type":"user","agent":"root","text":WORDS,"attachments":[]}));
    a.wait(Duration::from_secs(30), |f| {
        f["type"] == "event"
            && f["agent"] == "root"
            && f["event"]["kind"] == "assistant"
            && f["event"]["text"]
                .as_str()
                .is_some_and(|t| t.contains(WORDS))
    })
    .expect("the words reach the transcript once it can be written");
    assert!(
        transcript(&k.place)
            .iter()
            .any(|e| e["kind"] == "user" && e["text"] == WORDS)
    );
    drop(a);
    drop(k);
}
