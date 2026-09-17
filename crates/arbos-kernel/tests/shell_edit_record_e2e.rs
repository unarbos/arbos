//! An edit made through the shell is an edit (SWE-bench cycle 21: four
//! rollouts edited with `sed -i` and no edit was on the record, so the
//! mechanism line, the coverage hook and the read were all blind). The
//! bash tool compares the repository's tracked changes before and after a
//! command; files the command changed go on the tool event's paths, the
//! result says so, and the coverage hook reads them as it reads an `edit`.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::{path::Path, process::Command, time::Duration};

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "git {args:?}");
}

fn tools(place: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|e| e["kind"] == "tool" && e["name"] == "bash")
        .collect()
}

#[test]
fn a_sed_edit_through_bash_is_on_the_record_with_its_coverage_and_a_no_op_is_not() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"editing\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sed -i 's/return c$/return c or 0/' pkg/cm.py\",\"description\":\"Patch register_cmap with sed\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"no-op\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sed -i 's/nothing-here/x/' pkg/pyplot.py\",\"description\":\"A sed that matches nothing\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"listing\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"ls pkg\",\"description\":\"List the package\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"done\"}\n",
    );
    let mut k = start_kernel_replay_prepared("shell-edit", replies, "", |place| {
        std::fs::create_dir_all(place.join("pkg")).unwrap();
        std::fs::create_dir_all(place.join("tests")).unwrap();
        git(place, &["init", "-q"]);
        git(place, &["config", "user.email", "t@t"]);
        git(place, &["config", "user.name", "t"]);
        std::fs::write(
            place.join("pkg/cm.py"),
            "def register_cmap(c):\n    return c\n",
        )
        .unwrap();
        std::fs::write(
            place.join("pkg/pyplot.py"),
            "def set_cmap(c):\n    return c\n",
        )
        .unwrap();
        std::fs::write(
            place.join("tests/test_cm.py"),
            "from pkg.cm import register_cmap\n\ndef test_it():\n    assert register_cmap(1) == 1\n",
        )
        .unwrap();
        std::fs::write(place.join(".gitignore"), ".arbos/\n").unwrap();
        git(place, &["add", "-A"]);
        git(place, &["commit", "-q", "-m", "init"]);
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "patch it with sed"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(60)));

    let calls = tools(&k.place);
    assert_eq!(calls.len(), 3, "{calls:#?}");

    // The sed that changed a tracked file: recorded as an edit.
    let sed = &calls[0];
    let paths: Vec<&str> = sed["paths"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p.as_str())
        .collect();
    assert!(
        paths.iter().any(|p| p.ends_with("pkg/cm.py")),
        "the changed file is on the record: {paths:?}"
    );
    let body = sed["body"].as_str().unwrap_or("");
    assert!(
        body.contains("[files changed by this command: pkg/cm.py"),
        "{body}"
    );
    assert!(
        body.contains("Tests covering this edit")
            && body.contains("register_cmap named in tests/test_cm.py"),
        "the coverage hook read the shell edit: {body}"
    );
    assert_eq!(
        std::fs::read_to_string(k.place.join("pkg/cm.py")).unwrap(),
        "def register_cmap(c):\n    return c or 0\n"
    );

    // The sed that matched nothing: sed rewrote the file, the bytes are
    // the same — not an edit, and said as a no-op.
    let noop = &calls[1];
    let body = noop["body"].as_str().unwrap_or("");
    assert!(!body.contains("files changed by this command"), "{body}");
    assert!(body.contains("changed nothing"), "{body}");
    assert!(
        !noop["paths"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p.as_str().is_some_and(|p| p.ends_with(".py"))),
        "{noop}"
    );

    // A read-only command: nothing recorded but its journal.
    let ls = &calls[2];
    assert!(!ls["body"].as_str().unwrap_or("").contains("files changed"));
    assert_eq!(ls["paths"].as_array().unwrap().len(), 1, "{ls}");
    let _ = k.child.kill();
}
