//! The `subscribe` and `plan` plumbing through `KernelHooks`: what the
//! tools call. Firing itself is covered by the `cron-fires-and-reports`
//! fixture; here: validation, the window's rows, and the window's ops.

use arbos_core::{Place, bootstrap, notes, subscription::Subscription};
use arbos_kernel::hooks::KernelHooks;
use arbos_kernel::plan::{NOTE_ID_BIT, SUB_ID_BIT};
use std::sync::Arc;

fn hooks(name: &str) -> Arc<KernelHooks> {
    let dir = std::env::temp_dir().join(format!(
        "arbos-subs-{name}-{}-{}",
        std::process::id(),
        arbos_core::now_ms()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let place = Place::new(&dir);
    bootstrap(&place).unwrap();
    let (wake_tx, _wake_rx) = tokio::sync::mpsc::unbounded_channel();
    let (kick_tx, _kick_rx) = tokio::sync::mpsc::unbounded_channel();
    KernelHooks::new(place, wake_tx, kick_tx)
}

fn timer(prompt: &str, every: Option<&str>) -> Subscription {
    Subscription {
        id: 0,
        kind: "timer".into(),
        prompt: prompt.into(),
        every: every.map(str::to_string),
        at: None,
        once: false,
        cmd: None,
        path: None,
        repo: None,
        pr: None,
        author: None,
        branch: None,

        channel: None,

        thread: None,

        match_text: None,
        deliver_to: "agent".into(),
        notify: None,
        expires: None,
        paused: false,
        continuity: false,
        internal: false,
        created: String::new(),
        next_due: None,
        last_fired: None,
        last: String::new(),
        error: None,
        seen: None,
    }
}

#[test]
fn subscriptions_are_validated_numbered_and_shown_as_standing_rows() {
    let h = hooks("rows");
    let a = h
        .subscribe("root", timer("check the build", Some("1h")), None)
        .unwrap();
    let b = h
        .subscribe("root", timer("stretch", None), Some("30m"))
        .unwrap();
    assert_eq!((a.id, b.id), (1, 2));
    assert!(b.once);
    let err = h
        .subscribe("root", timer("too fast", Some("5s")), None)
        .unwrap_err();
    assert!(format!("{err:#}").contains("minimum"), "{err:#}");
    let frame = h.plan_frame("root");
    let arbos_core::wire::Frame::Plan { nodes, .. } = frame else {
        panic!("plan frame");
    };
    let standing: Vec<_> = nodes.iter().filter(|n| n.standing).collect();
    assert_eq!(standing.len(), 2);
    assert_eq!(standing[0].id, SUB_ID_BIT | 1);
    assert!(
        standing[0].when.starts_with("every 1h"),
        "{}",
        standing[0].when
    );
    assert!(
        standing[1].when.starts_with("once at"),
        "{}",
        standing[1].when
    );
    // The window's ops: pause, run (due now), cancel.
    h.plan_op("root", SUB_ID_BIT | 1, "pause", "").unwrap();
    assert!(
        arbos_core::subscription::get(&h.place, "root", 1)
            .unwrap()
            .paused
    );
    h.plan_op("root", SUB_ID_BIT | 1, "run", "").unwrap();
    let s = arbos_core::subscription::get(&h.place, "root", 1).unwrap();
    assert!(!s.paused && s.is_due(arbos_core::now_ms() + 1));
    h.plan_op("root", SUB_ID_BIT | 2, "cancel", "").unwrap();
    assert!(arbos_core::subscription::get(&h.place, "root", 2).is_none());
}

#[test]
fn notes_items_are_rows_the_window_can_check_and_drop() {
    let h = hooks("notes");
    let mut n = h.notes("root");
    n.set(&[
        ("Kernel".into(), "[#96](x) — running".into()),
        ("Kernel".into(), "[#97](y) — review".into()),
    ]);
    h.save_notes("root", &n).unwrap();
    let arbos_core::wire::Frame::Plan { nodes, .. } = h.plan_frame("root") else {
        panic!("plan frame");
    };
    let open: Vec<_> = nodes.iter().filter(|n| !n.standing && !n.inbox).collect();
    assert_eq!(open.len(), 2);
    assert_eq!(open[0].id, NOTE_ID_BIT | 1);
    assert_eq!(open[0].origin, "Kernel");
    h.plan_op("root", NOTE_ID_BIT | 1, "check", "solved")
        .unwrap();
    let items = notes::load(&h.place, "root").items();
    assert!(
        items.iter().any(|i| i.done && i.text.ends_with("— solved")),
        "{items:?}"
    );
    let arbos_core::wire::Frame::Plan { nodes, .. } = h.plan_frame("root") else {
        panic!("plan frame");
    };
    assert_eq!(nodes.iter().filter(|n| !n.standing && !n.inbox).count(), 1);
    h.plan_op("root", NOTE_ID_BIT | 1, "cancel", "").unwrap();
    assert_eq!(notes::load(&h.place, "root").open().len(), 0);
}

/// Stop pauses standing work and keeps what the user queued: their
/// follow-up is held (wake off — the Send now / Remove row), never
/// dropped (F-105); a worker's queued brief, the machine's words, goes
/// with the stop as before.
#[test]
fn stop_pauses_subscriptions_holds_the_users_follow_up_and_drops_machine_wakes() {
    let h = hooks("stop");
    h.subscribe("root", timer("tick", Some("1h")), None)
        .unwrap();
    h.inbox("root", "later", "user", Vec::new()).unwrap();
    h.inbox("root", "a brief for you", "spawn:parent", Vec::new())
        .unwrap();
    let before = arbos_core::inbox::list(&h.place, "root");
    assert_eq!(before.len(), 2);
    assert!(before.iter().all(|f| f.msg.wake));
    h.stop_work("root");
    let after = arbos_core::inbox::list(&h.place, "root");
    assert_eq!(
        after.len(),
        1,
        "the user's words stay; the brief goes: {after:?}"
    );
    let held = &after[0];
    assert_eq!(held.msg.body.trim(), "later");
    assert_eq!(held.msg.from, "user");
    assert!(!held.msg.wake, "held, not queued to run: {held:?}");
    // Send now: it runs as its own turn again.
    h.plan_op("root", arbos_kernel::hooks::inbox_id(&held.name), "run", "")
        .unwrap();
    assert!(arbos_core::inbox::list(&h.place, "root")[0].msg.wake);
    let s = arbos_core::subscription::get(&h.place, "root", 1).unwrap();
    assert!(s.paused && s.last.contains("stopped by the user"));
}
