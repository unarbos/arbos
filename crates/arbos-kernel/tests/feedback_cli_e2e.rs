//! `arbos-kernel feedback <place>`: the report's material through a second
//! door, for a window whose attach never connected. The same bundle the
//! `feedback` frame carries, as JSON on stdout; "nothing to say" and "the
//! command failed" told apart by the exit code.

mod common;

use common::{Attach, start_kernel_replay};
use std::{process::Command, time::Duration};

fn feedback(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_arbos-kernel"))
        .arg("feedback")
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// The kernel is gone (the shape the door exists for): the bundle still
/// comes, from disk, as the frame the window already parses, and it says
/// which kernel was serving and that it is not live.
#[test]
fn a_dead_kernels_place_still_yields_the_bundle_as_the_frame_on_stdout() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"looking\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"echo marker-7731\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"the marker is printed\"}\n",
    );
    let mut k = start_kernel_replay("feedback-cli", replies);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "print the marker"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    drop(a);
    let _ = k.child.kill();
    let _ = k.child.wait();

    let (code, out, err) = feedback(&[
        k.place.to_str().unwrap(),
        "--tail",
        "50",
        "--note",
        "it never connected",
    ]);
    assert_eq!(code, 0, "stderr: {err}");
    let frame: serde_json::Value = serde_json::from_str(out.trim())
        .unwrap_or_else(|e| panic!("stdout is one JSON frame: {e}\n{out}"));
    assert_eq!(frame["type"], "feedback_bundle", "{frame}");
    assert_eq!(frame["agent"], "root");
    let all: Vec<&serde_json::Value> = frame["events"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(frame["tail"].as_array().into_iter().flatten())
        .collect();
    assert!(
        all.iter()
            .any(|e| e["kind"] == "assistant" && e["text"] == "the marker is printed"),
        "the turn's lines are in the bundle: {frame:#}"
    );
    assert_eq!(frame["note"], "it never connected");
    assert_eq!(frame["kernel"]["door"], "cli", "{}", frame["kernel"]);
    let serving = &frame["kernel"]["serving"];
    assert_eq!(serving["known"], true, "{serving}");
    assert_eq!(
        serving["live"], false,
        "the killed kernel is not live: {serving}"
    );
    assert!(serving["pid"].as_u64().is_some_and(|p| p > 0), "{serving}");
    assert!(frame["place"].is_object(), "{frame}");
    assert!(frame["bytes"].as_u64().is_some_and(|b| b > 0));
}

/// "Nothing to say" is a bundle (exit 0); "cannot read the place" is not
/// (exit 2, one plain line on stderr, nothing on stdout).
#[test]
fn a_place_that_cannot_be_read_exits_non_zero_with_a_plain_reason() {
    let dir = std::env::temp_dir().join(format!("arbos-feedback-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // Not a place at all.
    let (code, out, err) = feedback(&[dir.to_str().unwrap()]);
    assert_eq!(code, 2, "stdout: {out}\nstderr: {err}");
    assert!(out.trim().is_empty(), "nothing on stdout: {out}");
    assert!(
        err.contains("is not an Arbos place") && err.contains("no .arbos/"),
        "{err}"
    );

    // A place, but no such agent.
    let k = start_kernel_replay(
        "feedback-cli-agent",
        "{\"agent\":\"root\",\"content\":\"hi\"}\n",
    );
    let (code, out, err) = feedback(&[k.place.to_str().unwrap(), "--agent", "ghost"]);
    assert_eq!(code, 2, "stdout: {out}\nstderr: {err}");
    assert!(out.trim().is_empty());
    assert!(err.contains("no agent is named \"ghost\""), "{err}");

    // A place with a root that has never spoken: a bundle with no lines,
    // and exit 0 — the far side has nothing to say, and says so in shape.
    let (code, out, err) = feedback(&[k.place.to_str().unwrap()]);
    assert_eq!(code, 0, "stderr: {err}");
    let frame: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(frame["type"], "feedback_bundle");
    assert_eq!(
        frame["kernel"]["serving"]["live"], true,
        "{}",
        frame["kernel"]
    );

    // No place given at all: usage, non-zero.
    let (code, _, err) = feedback(&[]);
    assert_ne!(code, 0);
    assert!(err.contains("feedback needs a place"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}
