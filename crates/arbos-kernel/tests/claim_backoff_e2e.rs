//! qa-018: a message whose turn cannot be started (the claim — the rename
//! of the inbox file into turns/tNNNN/ — fails: disk full, a read-only
//! folder) used to be retried every tick with no error anywhere. Now the
//! client is told once, the kernel logs once, and the message waits a
//! minute before the next try.

mod common;

use common::{Attach, start_kernel_replay};
use std::time::Duration;

#[test]
fn an_unclaimable_message_is_reported_once_and_backs_off() {
    // A keyed kernel (replay): without a key the words are held before
    // any claim is tried (keyless_first_line_e2e), and this test is about
    // the claim itself.
    let mut k = start_kernel_replay("claim", "");
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );

    // turns/ exists and cannot be written to: the claim's mkdir fails.
    let root = k.place.join(".arbos").join("agents").join("root");
    let turns = root.join("turns");
    std::fs::create_dir_all(&turns).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&turns, std::fs::Permissions::from_mode(0o555)).unwrap();
    }

    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "Reply OK."}));
    let err = a.wait(Duration::from_secs(20), |f| {
        f["type"] == "error" && f["agent"] == "root"
    });
    assert!(
        err.is_some(),
        "the client must be told the turn could not start"
    );
    std::thread::sleep(Duration::from_secs(12));

    // The message is still there, unclaimed, and nothing was written for it.
    let inbox: Vec<_> = std::fs::read_dir(root.join("inbox"))
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(inbox.len(), 1, "the message waits in the inbox");
    let log = std::fs::read_to_string(k.place.join(".arbos").join("runtime").join("kernel.log"))
        .or_else(|_| std::fs::read_to_string(k.place.join(".arbos").join("kernel.log")))
        .unwrap_or_default();
    let failures = log.matches("inbox_claim_failed").count();
    assert!(
        (1..=2).contains(&failures),
        "claims kept retrying: {failures} log lines in ~15s"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&turns, std::fs::Permissions::from_mode(0o755));
    }
    let _ = k.child.kill();
}
