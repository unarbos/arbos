//! qa-004: the focus file is written from the attach socket and read back
//! by every client and every prompt, so it must only ever name an agent
//! folder of the place.

mod common;

use common::{Attach, start_kernel_replay};
use std::time::Duration;

#[test]
fn an_attach_client_cannot_point_the_focus_outside_the_agents_folder() {
    let mut k = start_kernel_replay("focus", "");
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    let focus = k.place.join(".arbos").join("runtime").join("focus");
    assert_eq!(
        std::fs::read_to_string(&focus).unwrap().trim(),
        ".arbos/agents/root"
    );

    for bad in [
        "../../../../etc/passwd",
        "/etc/passwd",
        ".arbos/agents/../../x",
        ".arbos/agents/does-not-exist",
    ] {
        a.send(serde_json::json!({"type": "focus", "path": bad}));
    }
    // Something the kernel does answer, so the rejected frames are behind us.
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "hi"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert_eq!(
        std::fs::read_to_string(&focus).unwrap().trim(),
        ".arbos/agents/root",
        "rejected focus frames must not touch the file"
    );

    // A fresh client's snapshot carries the real focus, never a bad one
    // planted on disk.
    std::fs::write(&focus, ".arbos/agents/gone\n").unwrap();
    let mut b = Attach::connect(&k.url);
    let snap = b
        .wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
        .unwrap();
    assert_eq!(snap["focus"], ".arbos/agents/root");
    assert_eq!(
        std::fs::read_to_string(&focus).unwrap().trim(),
        ".arbos/agents/root"
    );
    let _ = k.child.kill();
}
