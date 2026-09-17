//! A project folder renamed under a running kernel (desktop gate, cycle
//! 35): the kernel's late writes — a turn's tail, through `create_dir_all`
//! on the absolute path — recreated the place at its old path as a ghost
//! the next open read as a project. Now the kernel compares the store's
//! identity (device, inode) before the turn's tail and on every tick, and
//! stops rather than write one byte where its store is not.

mod common;

use common::{Attach, start_kernel_replay_in_place_cwd, wait_exit};
use std::time::Duration;

#[test]
fn a_place_renamed_mid_turn_is_not_recreated_at_its_old_path_and_the_kernel_stops() {
    // After the slow step, the model writes the project page — the write
    // that recreated the moved place at its old path on main.
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"working\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 3; echo done\",\"description\":\"A slow step\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"noting\",\"calls\":[{\"name\":\"write\",\"arguments\":{\"path\":\".arbos/notes.md\",\"content\":\"# Notes\\n\\n- [ ] the slow step ran\\n\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"finished the step\"}\n",
    );
    let mut k = start_kernel_replay_in_place_cwd("store-moved", replies);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "do the slow step"}));
    assert!(
        a.wait(Duration::from_secs(20), |f| {
            f["type"] == "event"
                && f["agent"] == "root"
                && f["event"]["kind"] == "tool"
                && f["event"]["name"] == "bash"
        })
        .is_some(),
        "the bash started"
    );

    // The person renames the project folder while the turn runs.
    let old = k.place.clone();
    let moved = k.scratch.join("place-moved");
    std::fs::rename(&old, &moved).unwrap();

    // The turn's tail lands; the kernel sees its store is not at the path
    // and stops (exit 4) instead of writing there.
    let code = wait_exit(&mut k.child, Duration::from_secs(30));
    assert_eq!(code, Some(4), "the kernel stopped on the moved store");
    assert!(
        !old.exists(),
        "the old path was not recreated: {:?}",
        std::fs::read_dir(&old).map(|rd| rd.flatten().map(|e| e.file_name()).collect::<Vec<_>>())
    );
    // The store is whole where it went, and says why its kernel stopped.
    assert!(moved.join(".arbos/agents/root/transcript.jsonl").exists());
    let transcript =
        std::fs::read_to_string(moved.join(".arbos/agents/root/transcript.jsonl")).unwrap();
    assert!(
        transcript.contains("folder was moved from") && transcript.contains("kernel stopped"),
        "the moved store carries the notice: {transcript}"
    );
    // No lock left behind where the store went (QA's `state:lock-leftover`
    // on every run): the stopping kernel takes its own files with it.
    for p in arbos_core::Place::new(moved.clone()).lock_paths() {
        assert!(
            !p.exists(),
            "lock file left in the moved store: {}",
            p.display()
        );
    }
    let _ = k.child.kill();
}

/// An idle kernel: the rename lands between turns, and the five-second
/// look catches it in one — a different inode does not change back.
#[test]
fn a_place_renamed_while_idle_stops_the_kernel_on_the_next_look() {
    let mut k = start_kernel_replay_in_place_cwd(
        "store-moved-idle",
        "{\"agent\":\"root\",\"content\":\"hi\"}\n",
    );
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "hello"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(20)));
    let old = k.place.clone();
    let moved = k.scratch.join("place-moved");
    std::fs::rename(&old, &moved).unwrap();
    // A stand-in folder appears at the old path, as a late write would
    // make: a different inode, so `Moved`, not `Gone`.
    std::fs::create_dir_all(old.join(".arbos")).unwrap();
    let code = wait_exit(&mut k.child, Duration::from_secs(20));
    assert_eq!(
        code,
        Some(4),
        "stopped on the first look at a foreign store"
    );
    // Nothing of ours landed in the stand-in.
    let ghost: Vec<_> = std::fs::read_dir(old.join(".arbos"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        ghost.is_empty(),
        "the kernel wrote into the stand-in: {ghost:?}"
    );
    let _ = k.child.kill();
}

/// The bash whose tool event went out before the rename and whose job
/// spawned after it: the job folder is the first thing made at the old
/// path — `.arbos/agents/root/jobs/j1`, a ghost with nothing in it. The
/// spawn now checks the store first and the call fails with the reason.
#[test]
fn a_bash_spawned_after_the_rename_fails_with_the_reason_and_makes_no_job_folder_at_the_old_path() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"working\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"echo hi\",\"description\":\"A quick step\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"done\"}\n",
    );
    // The job's folder is made two seconds after the tool's record goes
    // out; the rename lands in between.
    let scratch = common::scratch_dir("store-moved-spawn");
    let file = scratch.join("replies.jsonl");
    std::fs::write(&file, replies).unwrap();
    let mut k = common::spawn_with_opts(
        scratch,
        &[
            "--provider",
            "replay",
            "--replies",
            &file.display().to_string(),
        ],
        &[("ARBOS_TEST_SPAWN_DELAY_MS", "2000")],
        true,
    );
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "run it"}));
    assert!(
        a.wait(Duration::from_secs(20), |f| {
            f["type"] == "event"
                && f["agent"] == "root"
                && f["event"]["kind"] == "tool"
                && f["event"]["name"] == "bash"
        })
        .is_some(),
        "the tool's record went out"
    );
    let old = k.place.clone();
    let moved = k.scratch.join("place-moved");
    std::fs::rename(&old, &moved).unwrap();
    let code = wait_exit(&mut k.child, Duration::from_secs(30));
    assert_eq!(code, Some(4), "the kernel stopped on the moved store");
    assert!(
        !old.exists(),
        "no job folder at the old path: {:?}",
        std::fs::read_dir(&old).map(|rd| rd.flatten().map(|e| e.path()).collect::<Vec<_>>())
    );
    let _ = k.child.kill();
}
