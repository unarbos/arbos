//! A fresh machine without git (a Mac before the command line tools): the
//! kernel serves, but checkpoints, rewind and undo have nothing to stand
//! on. Said once on root's transcript, carried in kernel.json, and said
//! again — the other way — when git appears.

mod common;

use common::{Attach, scratch_dir, spawn_with_env};
use std::time::Duration;

fn notices(place: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|e| e["kind"] == "notice")
        .filter_map(|e| e["text"].as_str().map(str::to_string))
        .collect()
}

/// A PATH with a shell and no git: the leash's wrapper needs `sh`, the
/// kernel needs nothing else to start.
fn path_without_git(scratch: &std::path::Path) -> String {
    let bin = scratch.join("nogit-bin");
    std::fs::create_dir_all(&bin).unwrap();
    for tool in [
        "sh", "bash", "cut", "ps", "kill", "sleep", "cat", "tr", "head",
    ] {
        for dir in ["/bin", "/usr/bin"] {
            let src = std::path::Path::new(dir).join(tool);
            if src.exists() {
                let _ = std::os::unix::fs::symlink(&src, bin.join(tool));
                break;
            }
        }
    }
    bin.display().to_string()
}

#[test]
fn a_kernel_without_git_says_so_once_and_says_when_git_is_back() {
    let scratch = scratch_dir("git-missing");
    let file = scratch.join("replies.jsonl");
    std::fs::write(&file, "{\"agent\":\"root\",\"content\":\"hi\"}\n").unwrap();
    let nogit = path_without_git(&scratch);
    let replay: [&str; 4] = [
        "--provider",
        "replay",
        "--replies",
        &file.display().to_string(),
    ];

    let mut k = spawn_with_env(scratch.clone(), &replay, &[("PATH", nogit.as_str())]);
    let kernel_json: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(kernel_json["git_missing"], true, "{kernel_json}");
    // The hello frame says it too, for a client that cannot read
    // kernel.json (a phone over the hub).
    let mut a = Attach::connect(&k.url);
    let hello = a
        .wait(Duration::from_secs(5), |f| f["type"] == "hello")
        .expect("hello");
    assert_eq!(hello["git_missing"], true, "{hello}");
    drop(a);
    assert!(
        common::wait_for(Duration::from_secs(5), || {
            notices(&k.place)
                .iter()
                .any(|t| t.starts_with("git is not installed"))
        }),
        "said on root's transcript: {:?}",
        notices(&k.place)
    );
    let said = notices(&k.place)
        .iter()
        .filter(|t| t.starts_with("git is not installed"))
        .count();
    assert_eq!(said, 1);
    assert!(k.place.join(".arbos/runtime/git-missing.said").exists());
    let _ = k.child.kill();
    let _ = k.child.wait();

    // A second start without git: not said again.
    let _ = std::fs::remove_file(k.place.join(".arbos/runtime/kernel.json"));
    let mut k2 = spawn_with_env(scratch.clone(), &replay, &[("PATH", nogit.as_str())]);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        notices(&k2.place)
            .iter()
            .filter(|t| t.starts_with("git is not installed"))
            .count(),
        1,
        "once, not once per start"
    );
    let _ = k2.child.kill();
    let _ = k2.child.wait();

    // git installed (the normal PATH): said the other way, once; the
    // marker goes; kernel.json no longer carries the flag.
    let _ = std::fs::remove_file(k2.place.join(".arbos/runtime/kernel.json"));
    let mut k3 = spawn_with_env(scratch.clone(), &replay, &[]);
    assert!(
        common::wait_for(Duration::from_secs(5), || {
            notices(&k3.place)
                .iter()
                .any(|t| t.starts_with("git is installed now"))
        }),
        "{:?}",
        notices(&k3.place)
    );
    assert!(!k3.place.join(".arbos/runtime/git-missing.said").exists());
    let kernel_json: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(k3.place.join(".arbos/runtime/kernel.json")).unwrap(),
    )
    .unwrap();
    assert!(kernel_json.get("git_missing").is_none(), "{kernel_json}");
    let mut c = Attach::connect(&k3.url);
    let hello = c
        .wait(Duration::from_secs(5), |f| f["type"] == "hello")
        .expect("hello");
    assert!(hello.get("git_missing").is_none(), "{hello}");
    let _ = k3.child.kill();
}
