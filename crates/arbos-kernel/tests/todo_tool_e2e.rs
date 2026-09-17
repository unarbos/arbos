//! Process parity, slice 3: Cursor's TodoWrite. A coordinator keeps its own
//! steps for the thread in `agents/root/todo.md` through the `todo` tool;
//! the project page is untouched; every write is announced as a `changed`
//! frame so a window draws the card.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::time::Duration;

#[test]
fn a_coordinator_keeps_its_thread_steps_in_todo_md_not_on_the_page() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"planning my steps\",\"calls\":[{\"name\":\"todo\",\"arguments\":{\"op\":\"set\",\"items\":[\"Read the two requests\",\"Spawn one worker per goal\",\"Fold the dones into the page\"]}}]}\n",
        "{\"agent\":\"root\",\"content\":\"first step done\",\"calls\":[{\"name\":\"todo\",\"arguments\":{\"op\":\"check\",\"n\":1,\"readout\":\"two goals, independent\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"Two goals; starting on them.\"}\n",
    );
    let mut k = start_kernel_replay_prepared("todo-tool", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"coordinator\"\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    let page_before = std::fs::read_to_string(k.place.join(".arbos/notes.md")).unwrap();
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "two goals: a poem and a colour table"}));
    let changed = a
        .wait(Duration::from_secs(30), |f| {
            f["type"] == "changed" && f["path"] == "agents/root/todo.md"
        })
        .expect("the todo write is announced as a changed frame");
    assert_eq!(changed["kind"], "modified");
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));

    let todo = std::fs::read_to_string(k.place.join(".arbos/agents/root/todo.md")).unwrap();
    assert!(
        todo.contains("- [x] Read the two requests — two goals, independent"),
        "{todo}"
    );
    assert!(todo.contains("- [ ] Spawn one worker per goal"), "{todo}");
    assert!(
        todo.contains("- [ ] Fold the dones into the page"),
        "{todo}"
    );
    // Checked steps sink below the open ones, as on any checklist.
    let open = todo.find("- [ ] Spawn").unwrap();
    let done = todo.find("- [x] Read").unwrap();
    assert!(open < done, "checked item sinks: {todo}");
    // The project page is not where a coordinator's own steps go.
    let page_after = std::fs::read_to_string(k.place.join(".arbos/notes.md")).unwrap();
    assert_eq!(page_before, page_after, "the page is untouched");
    assert!(!page_after.contains("Spawn one worker"));
    // The tool result shows the list with numbers, like plan does.
    let root: Vec<serde_json::Value> =
        std::fs::read_to_string(k.place.join(".arbos/agents/root/transcript.jsonl"))
            .unwrap()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
    let calls: Vec<&serde_json::Value> = root
        .iter()
        .filter(|e| e["kind"] == "tool" && e["name"] == "todo")
        .collect();
    assert_eq!(calls.len(), 2, "{root:?}");
    assert!(
        calls[0]["body"]
            .as_str()
            .unwrap_or("")
            .starts_with("Set 3 item(s)."),
        "{:?}",
        calls[0]["body"]
    );
    assert!(
        calls[1]["body"]
            .as_str()
            .unwrap_or("")
            .starts_with("Checked 1"),
        "{:?}",
        calls[1]["body"]
    );
    let _ = k.child.kill();
}
