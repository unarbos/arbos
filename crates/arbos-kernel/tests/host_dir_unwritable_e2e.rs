//! The host folder (`~/.config/arbos`) the person cannot write — a managed
//! machine, a home on a read-only mount, a folder another account made.
//! The config in it reads fine; the kernel used to die at `Host::load`
//! writing the empty `places` marker, with a bare "Permission denied (os
//! error 13)" — no path, no what. Now it serves, says once on the main
//! chat what is not kept and where, and says when the folder takes writes
//! again.

mod common;

use std::time::Duration;

use common::{Attach, scratch_dir, spawn_with};

#[cfg(unix)]
#[test]
fn a_host_folder_the_user_cannot_write_serves_and_is_said_once() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let scratch = scratch_dir("host-dir-unwritable");
    let arbos = scratch.join("xdg").join("arbos");
    std::fs::write(arbos.join("config.toml"), "trace = false\n").unwrap();
    let replies = scratch.join("replies.jsonl");
    std::fs::write(&replies, "").unwrap();
    std::fs::set_permissions(&arbos, std::fs::Permissions::from_mode(0o555)).unwrap();
    let start = || {
        spawn_with(
            scratch.clone(),
            &[
                "--provider",
                "replay",
                "--replies",
                replies.to_str().unwrap(),
            ],
        )
    };
    let notices = |place: &std::path::Path| -> Vec<String> {
        std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(|v| v["kind"] == "notice")
            .map(|v| v["text"].as_str().unwrap_or("").to_string())
            .collect()
    };

    // First start: it serves (a snapshot arrives), the log names the
    // folder, and the main chat carries one notice with the folder and
    // what is not kept.
    let mut k = start();
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    let log = std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.log")).unwrap();
    assert!(log.contains("host_dir_unwritable"), "{log}");
    assert!(log.contains(&arbos.display().to_string()), "{log}");
    let said: Vec<String> = notices(&k.place)
        .into_iter()
        .filter(|t| t.contains("cannot be written"))
        .collect();
    assert_eq!(said.len(), 1, "{:?}", notices(&k.place));
    assert!(said[0].contains(&arbos.display().to_string()), "{said:?}");
    assert!(said[0].contains("XDG_CONFIG_HOME"), "what to do: {said:?}");
    let _ = k.child.kill();
    let _ = k.child.wait();

    // Second start, still read-only: the log says it again, the chat does
    // not.
    let mut k = start();
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    assert_eq!(
        notices(&k.place)
            .iter()
            .filter(|t| t.contains("cannot be written"))
            .count(),
        1
    );
    let _ = k.child.kill();
    let _ = k.child.wait();

    // Writable again: said once, and the marker is gone so a later
    // relapse is said afresh.
    std::fs::set_permissions(&arbos, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut k = start();
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    let back: Vec<String> = notices(&k.place)
        .into_iter()
        .filter(|t| t.contains("can be written again"))
        .collect();
    assert_eq!(back.len(), 1, "{:?}", notices(&k.place));
    assert!(
        !k.place
            .join(".arbos/runtime/host-dir-unwritable.said")
            .exists()
    );
    assert!(
        arbos.join("places").exists(),
        "the marker file lands once it can"
    );
    let _ = k.child.kill();
    let _ = k.child.wait();
}
