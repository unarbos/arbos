//! qal-j40: `rm -rf .arbos/runtime .arbos/lock` under a running kernel
//! left its flocks on unlinked inodes, and a second kernel served the same
//! place beside it in silence. The holder now notices on its tick, writes
//! its lock files again and says so; a second kernel is refused as before.
//! And when a second kernel did get in first, the holder stops rather than
//! be one of two writers on one store.

mod common;

use common::{Attach, start_kernel_replay, wait_for};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

fn transcript_text(place: &Path) -> String {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl")).unwrap_or_default()
}

fn kernel_log(place: &Path) -> String {
    std::fs::read_to_string(place.join(".arbos/runtime/kernel.log")).unwrap_or_default()
}

fn relaunch(k: &common::Kernel) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_arbos-kernel"))
        .arg("serve")
        .arg(&k.place)
        .env("XDG_CONFIG_HOME", k.scratch.join("xdg"))
        .env("HOME", k.scratch.join("home"))
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn pid_in(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[test]
fn lock_files_removed_under_a_kernel_are_written_again_and_a_second_kernel_is_still_refused() {
    let mut k = start_kernel_replay("lock-removed", "");
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    let legacy = k.place.join(".arbos/lock");
    let runtime_lock = k.place.join(".arbos/runtime/lock");
    assert_eq!(pid_in(&legacy), Some(k.child.id()));
    assert_eq!(pid_in(&runtime_lock), Some(k.child.id()));

    // The thorough cleanup a person reaches for.
    std::fs::remove_dir_all(k.place.join(".arbos/runtime")).unwrap();
    std::fs::remove_file(&legacy).unwrap();
    assert!(!legacy.exists() && !runtime_lock.exists());

    // Within a tick or two the holder has them back, with its pid, and
    // the record windows find it by.
    assert!(
        wait_for(Duration::from_secs(15), || pid_in(&legacy)
            == Some(k.child.id())
            && pid_in(&runtime_lock) == Some(k.child.id())),
        "the lock files are written again by the holder: legacy={:?} runtime={:?}",
        pid_in(&legacy),
        pid_in(&runtime_lock)
    );
    assert!(
        wait_for(Duration::from_secs(5), || k
            .place
            .join(".arbos/runtime/kernel.json")
            .exists()),
        "kernel.json is written again"
    );
    assert!(
        wait_for(Duration::from_secs(5), || transcript_text(&k.place)
            .contains("were removed while it ran; it has written them again")),
        "said on root's transcript: {}",
        transcript_text(&k.place)
    );
    assert!(
        kernel_log(&k.place).contains("lock_retaken"),
        "{}",
        kernel_log(&k.place)
    );

    // A second kernel finds the place held, as before the removal.
    let (code, err) = relaunch(&k);
    assert_eq!(code, 3, "{err}");
    assert!(err.contains("place already served"), "{err}");
    assert!(err.contains(&format!("pid {}", k.child.id())), "{err}");
    // And the first is still serving.
    assert!(k.child.try_wait().unwrap().is_none(), "the holder is alive");
    let _ = k.child.kill();
}

/// The race the fix cannot win from behind: a second kernel took fresh
/// lock files before the holder's tick. Then the holder stops, says why,
/// and the second kernel keeps the place — one writer, not two.
#[test]
fn a_holder_whose_place_another_kernel_took_stops_and_says_so() {
    let mut k = start_kernel_replay("lock-taken", "");
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    let legacy = k.place.join(".arbos/lock");
    let runtime_lock = k.place.join(".arbos/runtime/lock");
    // Remove both, then hold fresh ones ourselves before the holder's
    // tick — standing in for the second kernel that got in first.
    std::fs::remove_dir_all(k.place.join(".arbos/runtime")).unwrap();
    std::fs::remove_file(&legacy).unwrap();
    let place = arbos_core::Place::new(k.place.clone());
    let intruder = arbos_core::PlaceLock::acquire(&place).expect("fresh files lock freely");
    assert_eq!(pid_in(&legacy), Some(std::process::id()));

    let code = common::wait_exit(&mut k.child, Duration::from_secs(20));
    assert_eq!(code, Some(4), "the holder stops with the store-lost code");
    assert!(
        transcript_text(&k.place).contains("another kernel has taken the place since"),
        "{}",
        transcript_text(&k.place)
    );
    // The intruder's files were not touched by the holder's exit.
    assert_eq!(pid_in(&legacy), Some(std::process::id()));
    assert_eq!(pid_in(&runtime_lock), Some(std::process::id()));
    drop(intruder);
}
