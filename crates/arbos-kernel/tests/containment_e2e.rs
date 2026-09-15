//! T3-06: a command that reaches past this machine — the cloud metadata
//! service, the container runtime, credential files — asks the user in
//! ask mode (the default, auto, asks nothing since 2026-09-15); `fetch`
//! refuses the metadata service outright in every mode; `check` names
//! jobs that reached; `allow_metadata = true` quiets the metadata question
//! for a place that really runs on a cloud instance.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::path::Path;
use std::time::Duration;

fn tools(place: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|e| e["kind"] == "tool")
        .collect()
}

const REPLIES: &str = concat!(
    "{\"agent\":\"root\",\"content\":\"creds\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"curl -s --connect-timeout 2 --max-time 3 http://169.254.169.254/latest/meta-data/ || echo no-metadata\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"ssh key\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"cat ~/.ssh/id_ed25519 2>/dev/null || echo no-key\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"page\",\"calls\":[{\"name\":\"fetch\",\"arguments\":{\"url\":\"http://169.254.169.254/latest/api/token\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"plain\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"echo hello\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"done\"}\n",
);

#[test]
fn reaches_past_the_machine_ask_in_ask_mode_and_fetch_refuses_metadata() {
    let mut k = start_kernel_replay_prepared("containment", REPLIES, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\nname = \"c\"\n\n[root]\npermission = \"ask\"\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "go"}));

    // Metadata reach: asks; deny.
    let ask = a
        .wait(Duration::from_secs(30), |f| {
            f["type"] == "ask" && f["agent"] == "root"
        })
        .expect("the metadata reach asks");
    let q = ask["question"].as_str().unwrap_or("");
    assert!(q.contains("cloud metadata service"), "{q}");
    let id = ask["id"].as_str().unwrap_or("").to_string();
    a.send(serde_json::json!({"type": "approve", "agent": "root", "call_id": id, "allow": false}));
    // Credential file: asks; allow (the file does not exist here).
    let ask2 = a
        .wait(Duration::from_secs(30), |f| {
            f["type"] == "ask" && f["agent"] == "root"
        })
        .expect("the credential read asks");
    assert!(
        ask2["question"]
            .as_str()
            .unwrap_or("")
            .contains("credential files"),
        "{ask2}"
    );
    let id2 = ask2["id"].as_str().unwrap_or("").to_string();
    a.send(serde_json::json!({"type": "approve", "agent": "root", "call_id": id2, "allow": true}));
    // Ask mode may ask for the plain command too (every write-class
    // call): allow whatever else asks until the turn ends — the
    // containment questions above were the point.
    let mut idle = false;
    for _ in 0..4 {
        let next = a.wait(Duration::from_secs(40), |f| {
            (f["type"] == "ask" && f["agent"] == "root" && f["id"] != id && f["id"] != id2)
                || (f["type"] == "turn" && f["agent"] == "root" && f["state"] == "idle")
        });
        match next {
            Some(f) if f["type"] == "ask" => {
                let id3 = f["id"].as_str().unwrap_or("").to_string();
                a.send(serde_json::json!({"type": "approve", "agent": "root", "call_id": id3, "allow": true}));
            }
            Some(_) => {
                idle = true;
                break;
            }
            None => break,
        }
    }
    assert!(idle, "the turn ends");

    let t = tools(&k.place);
    assert_eq!(t.len(), 4, "{t:#?}");
    assert!(
        t[0]["error"]
            .as_str()
            .unwrap_or("")
            .contains("did not allow bash"),
        "{:#?}",
        t[0]
    );
    assert!(
        t[1].get("error").is_none() && t[1]["body"].as_str().unwrap_or("").contains("no-key"),
        "{:#?}",
        t[1]
    );
    assert!(
        t[2]["error"]
            .as_str()
            .unwrap_or("")
            .contains("refuses the cloud metadata service"),
        "{:#?}",
        t[2]
    );
    // The ordinary command ran.
    assert!(
        t[3].get("error").is_none() && t[3]["body"].as_str().unwrap_or("").contains("hello"),
        "{:#?}",
        t[3]
    );

    // `check` names the job that reached for credentials.
    let place = arbos_core::Place::new(&k.place);
    let report = arbos_kernel::check::check(&place).unwrap();
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.what.contains("reached for credential files")),
        "{:#?}",
        report.findings
    );
    let _ = k.child.kill();
}

#[test]
fn allow_metadata_quiets_the_metadata_question_only() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"creds\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"curl -s --max-time 1 http://169.254.169.254/ || echo no-metadata\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"done\"}\n",
    );
    let mut k = start_kernel_replay_prepared("containment-allowed", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\nname = \"c\"\n",
        )
        .unwrap();
        std::fs::write(
            place.join(".arbos/sandbox.toml"),
            "enabled = false\nallow_metadata = true\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "go"}));
    // Had it asked, the turn would sit on the question and never go idle.
    assert!(
        a.wait_turn("root", "idle", Duration::from_secs(40)),
        "no question was asked"
    );
    let t = tools(&k.place);
    assert_eq!(t.len(), 1, "{t:#?}");
    assert!(
        t[0].get("error").is_none(),
        "no question, it ran: {:#?}",
        t[0]
    );
    let _ = k.child.kill();
}

/// The default mode: no card for the reaches; the metadata `fetch` is
/// still refused (a hard refusal, not an ask); `check` still names what
/// reached for credentials.
#[test]
fn in_auto_mode_reaches_run_without_a_card_and_fetch_still_refuses_metadata() {
    let mut k = start_kernel_replay_prepared("containment-auto", REPLIES, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\nname = \"c\"\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "go"}));
    let mut asked = false;
    let idle = a.wait(Duration::from_secs(60), |f| {
        if f["type"] == "ask" && f["agent"] == "root" {
            asked = true;
        }
        f["type"] == "turn" && f["agent"] == "root" && f["state"] == "idle"
    });
    assert!(idle.is_some(), "the turn ends");
    assert!(!asked, "no approval card in auto mode");
    let t = tools(&k.place);
    assert_eq!(t.len(), 4, "{t:#?}");
    assert!(
        t[0].get("error").is_none(),
        "the metadata curl ran (and failed on its own): {:#?}",
        t[0]
    );
    assert!(t[1].get("error").is_none() && t[1]["body"].as_str().unwrap_or("").contains("no-key"));
    assert!(
        t[2]["error"]
            .as_str()
            .unwrap_or("")
            .contains("refuses the cloud metadata service"),
        "{:#?}",
        t[2]
    );
    assert!(t[3]["body"].as_str().unwrap_or("").contains("hello"));
    let place = arbos_core::Place::new(&k.place);
    let report = arbos_kernel::check::check(&place).unwrap();
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.what.contains("reached for credential files")),
        "{:#?}",
        report.findings
    );
    let _ = k.child.kill();
}
