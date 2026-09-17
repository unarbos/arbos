//! A place inside iCloud sync: `check` warns, `store nosync` moves the
//! store beside the project with `.arbos` a symlink, and `check` is then
//! quiet about it.

use std::process::Command;

fn kernel(home: &std::path::Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_arbos-kernel"))
        .args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("xdg"))
        .output()
        .unwrap();
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn check_warns_inside_icloud_and_store_nosync_settles_it() {
    let home_dir = std::env::temp_dir().join(format!(
        "arbos-cloudsync-{}-{}",
        std::process::id(),
        arbos_core::now_ms()
    ));
    std::fs::create_dir_all(&home_dir).unwrap();
    struct Home(std::path::PathBuf);
    impl Home {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let home = Home(home_dir);
    let place = home
        .path()
        .join("Library/Mobile Documents/com~apple~CloudDocs/Documents/Misc/arbos");
    std::fs::create_dir_all(place.join(".arbos/agents/root")).unwrap();
    std::fs::write(
        place.join(".arbos/agents/root/agent.md"),
        "---\nname: root\nparent: null\npaused: false\nmodel: inherit\n---\n",
    )
    .unwrap();
    std::fs::write(place.join(".arbos/agents/root/transcript.jsonl"), "").unwrap();
    let place_s = place.to_str().unwrap();

    let before = kernel(home.path(), &["check", place_s]);
    assert!(before.contains("iCloud Drive sync"), "{before}");
    assert!(before.contains("store nosync"), "{before}");

    let status = kernel(home.path(), &["store", "status", place_s]);
    assert!(status.contains("inside iCloud Drive sync"), "{status}");

    let moved = kernel(home.path(), &["store", "nosync", place_s]);
    assert!(moved.contains(".arbos.nosync"), "{moved}");
    assert!(
        std::fs::symlink_metadata(place.join(".arbos"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(place.join(".arbos.nosync/agents/root/agent.md").exists());
    // The link still reads through.
    assert!(place.join(".arbos/agents/root/agent.md").exists());

    let after = kernel(home.path(), &["check", place_s]);
    // The store is a symlink, but its target is still inside the synced
    // folder — iCloud skips `.nosync` names, so this is the settled state
    // and check says nothing more about it... unless it does: the rule
    // is "silent when the store's real path is out of the sync or ends in
    // .nosync".
    assert!(!after.contains("store nosync"), "{after}");
}
