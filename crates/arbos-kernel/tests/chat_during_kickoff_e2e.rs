//! qal-j43: a chat minted while the place's kickoff turn runs, and a line
//! typed into it. The desktop mints the chat on disk (`create_chat`) and
//! sends a plain `user` frame; the kernel must file it and run it.

mod common;

use common::{Attach, start_kernel_replay_prepared, wait_for};
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

#[test]
fn a_line_typed_into_a_chat_minted_during_kickoff_is_not_lost() {
    let replies = concat!(
        // Root's kickoff: one slow step so the chat is minted mid-turn.
        "{\"agent\":\"root\",\"content\":\"looking around\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 6\",\"description\":\"look\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"Hey — ready.\\nTell me what to work on.\"}\n",
        // The new chat's own turn.
        "{\"content\":\"hello from the new chat\"}\n",
    );
    let k = start_kernel_replay_prepared("chat-during-kickoff", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(place.join(".arbos/user.md"), "name: Jacob\n").unwrap();
        std::fs::write(place.join("README.md"), "# Toy\n").unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "kickoff"}));
    assert!(a.wait_turn("root", "running", Duration::from_secs(20)));

    // ⌘N: the desktop mints the chat on disk, as `kernel::mint_chat` does.
    let place = arbos_core::Place::new(k.place.clone());
    let chat = arbos_core::create_chat(&place).expect("mint");
    let id = chat.id.to_string();
    // A second attach for the new chat, then the typed line.
    let mut b = Attach::connect(&k.url);
    assert!(
        b.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    b.send(serde_json::json!({"type": "user", "agent": id, "text": "TYPED-DURING-KICKOFF"}));

    // The line is filed (inbox or transcript) within a moment, and the
    // chat's turn runs — during root's kickoff or right after it.
    let inbox = k.place.join(".arbos/agents").join(&id).join("inbox");
    assert!(
        wait_for(Duration::from_secs(10), || {
            let filed = std::fs::read_dir(&inbox)
                .map(|rd| rd.flatten().count() > 0)
                .unwrap_or(false);
            filed
                || transcript(&k.place, &id)
                    .iter()
                    .any(|e| e["kind"] == "user")
        }),
        "the typed line reaches the kernel: inbox {:?}, transcript {:?}",
        std::fs::read_dir(&inbox).map(|rd| rd.flatten().count()),
        transcript(&k.place, &id)
    );
    assert!(
        wait_for(Duration::from_secs(40), || transcript(&k.place, &id)
            .iter()
            .any(|e| e["kind"] == "turn_complete")),
        "the chat's turn ran: {:#?}",
        transcript(&k.place, &id)
    );
    let t = transcript(&k.place, &id);
    assert!(
        t.iter()
            .any(|e| e["kind"] == "user" && e["text"] == "TYPED-DURING-KICKOFF"),
        "{t:#?}"
    );
    assert!(a.wait_turn("root", "idle", Duration::from_secs(40)));
}
