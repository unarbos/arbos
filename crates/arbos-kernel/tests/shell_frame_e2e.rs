//! Side panels, kernel handover 2: a `board` frame says who asked for the
//! panel (`by: user | agent`), and a client can ask for a shell of its own
//! with a `shell` frame. The drawer opens when the person asked, however
//! they asked, and never because the agent did something.

mod common;

use common::{Attach, start_kernel_replay};
use std::time::Duration;

/// A `shell` frame mints a terminal in the asked-for directory, announced
/// with `by: user`; a directory that is not there is an `error` frame, not
/// a shell somewhere else.
#[test]
fn a_client_asks_for_a_shell_and_the_board_says_the_user_asked() {
    let mut k = start_kernel_replay("shell-frame", "{\"agent\":\"root\",\"content\":\"hi\"}\n");
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    let sub = k.place.join("sub");
    std::fs::create_dir_all(&sub).unwrap();

    a.send(serde_json::json!({"type": "shell", "cwd": "sub"}));
    let board = a
        .wait(Duration::from_secs(10), |f| {
            f["type"] == "board" && f["panel"] == "terminal" && f["action"] == "open"
        })
        .expect("a board frame for the shell");
    assert_eq!(board["by"], "user", "{board}");
    assert_eq!(board["owner"], "root", "{board}");
    assert_eq!(
        board["cwd"].as_str().map(std::path::PathBuf::from),
        Some(sub.clone()),
        "relative to the place: {board}"
    );
    let id = board["terminal_ids"][0].as_str().unwrap().to_string();
    assert!(id.starts_with('t'), "{id}");
    // The shell is live: its prompt arrives as pty output on that page.
    assert!(
        a.wait(Duration::from_secs(10), |f| {
            f["type"] == "pty" && f["page"] == id.as_str()
        })
        .is_some(),
        "pty output for {id}"
    );

    // A directory that is not there: an error, and no board.
    a.send(serde_json::json!({"type": "shell", "cwd": "nowhere/at/all"}));
    let err = a
        .wait(Duration::from_secs(10), |f| f["type"] == "error")
        .expect("an error frame");
    let detail = err["detail"].as_str().unwrap_or("");
    assert!(
        detail.starts_with("shell:") && detail.contains("is not a directory"),
        "{err}"
    );
    assert!(
        a.wait(Duration::from_millis(500), |f| {
            f["type"] == "board" && f["terminal_ids"][0] != id.as_str()
        })
        .is_none(),
        "no second shell for a missing directory"
    );

    // An owner nobody has is refused by name.
    a.send(serde_json::json!({"type": "shell", "owner": "ghost"}));
    let err = a
        .wait(Duration::from_secs(10), |f| {
            f["type"] == "error" && f["agent"] == "ghost"
        })
        .expect("an error frame naming the owner");
    assert!(
        err["detail"]
            .as_str()
            .unwrap_or("")
            .contains("no agent \"ghost\""),
        "{err}"
    );
    let _ = k.child.kill();
}

/// The agent's `terminal` tool: `by: user` when it says the person asked,
/// `agent` otherwise — and anything that is not the word `user` reads as
/// the agent's own, so a drawer never opens on a guess.
#[test]
fn the_terminal_tool_marks_who_asked_and_defaults_to_the_agent() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"opening for you\",\"calls\":[{\"name\":\"terminal\",\"arguments\":{\"action\":\"open\",\"by\":\"user\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"opened\"}\n",
        "{\"agent\":\"root\",\"content\":\"opening for me\",\"calls\":[{\"name\":\"terminal\",\"arguments\":{\"action\":\"open\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"opened\"}\n",
        "{\"agent\":\"root\",\"content\":\"opening oddly\",\"calls\":[{\"name\":\"terminal\",\"arguments\":{\"action\":\"open\",\"by\":\"the person, I think\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"opened\"}\n",
    );
    let mut k = start_kernel_replay("terminal-by", replies);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    let mut seen = Vec::new();
    for (ask, want) in [
        ("open the terminal", "user"),
        ("do your thing", "agent"),
        ("again", "agent"),
    ] {
        a.send(serde_json::json!({"type": "user", "agent": "root", "text": ask}));
        let board = a
            .wait(Duration::from_secs(20), |f| {
                f["type"] == "board"
                    && f["panel"] == "terminal"
                    && f["action"] == "open"
                    && !seen.contains(&f["terminal_ids"][0])
            })
            .expect("a board frame from the terminal tool");
        seen.push(board["terminal_ids"][0].clone());
        assert_eq!(board["by"], want, "{ask:?}: {board}");
        assert!(a.wait_turn("root", "idle", Duration::from_secs(20)));
    }
    let _ = k.child.kill();
}
