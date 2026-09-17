//! qal-j03: a reply that lands while the app is closed showed no badge
//! on reopening, though the kernel wrote the notification. The exact
//! case: nobody attached when the turn ends; then a client attaches — to
//! this kernel, and to a new kernel after a restart — and must get the
//! notification replayed. kernel.log now says so (`notify_replayed`
//! with the count; `seen_marked` when a client clears), so the two
//! sides can be told apart.

mod common;

use common::{Attach, restart_replay, start_kernel_replay};
use std::time::{Duration, Instant};

fn log(place: &std::path::Path) -> String {
    std::fs::read_to_string(place.join(".arbos/runtime/kernel.log")).unwrap_or_default()
}

#[test]
fn a_reply_that_lands_with_nobody_attached_is_replayed_to_the_next_client_even_after_a_restart() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"working\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 2; echo done\",\"description\":\"Slow step\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"All three files are written.\"}\n",
    );
    let mut k = start_kernel_replay("notify-away", replies);
    {
        let mut a = Attach::connect(&k.url);
        let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
        a.send(serde_json::json!({"type":"user","agent":"root","text":"write the files","attachments":[]}));
        assert!(a.wait_turn("root", "running", Duration::from_secs(10)));
        // The app closes mid-turn.
    }
    // The turn ends with nobody attached: the notification is on disk.
    let path = k.place.join(".arbos/notifications.jsonl");
    let deadline = Instant::now() + Duration::from_secs(20);
    while std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .count()
        < 1
    {
        assert!(
            Instant::now() < deadline,
            "the reply is recorded with nobody attached"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(on_disk.contains("\"kind\":\"reply\""), "{on_disk}");

    // Reopen: the client attaches to the same kernel and gets it back.
    let mut b = Attach::connect(&k.url);
    let n = b
        .wait(Duration::from_secs(10), |f| f["type"] == "notify")
        .expect("the missed reply is replayed after hello");
    assert_eq!(n["replayed"], true);
    assert_eq!(n["kind"], "reply");
    assert_eq!(n["id"], 1);
    assert!(n["body"].as_str().unwrap().contains("three files"), "{n}");
    let text = log(&k.place);
    assert!(text.contains("\"event\":\"notify_replayed\""), "{text}");
    assert!(text.contains("count=1 ids=1..1 seen_through=0"), "{text}");
    assert!(
        !text.contains("seen_marked"),
        "no client cleared anything: {text}"
    );
    drop(b);

    // The kernel restarts (the app closed and reopened later): the file
    // outlives it, and the next client gets the same replay.
    let mut k2 = restart_replay(&mut k, "");
    let mut c = Attach::connect(&k2.url);
    let n = c
        .wait(Duration::from_secs(10), |f| f["type"] == "notify")
        .expect("replayed by the new kernel too");
    assert_eq!(n["replayed"], true);
    assert_eq!(n["id"], 1);
    // The client clears it: the log says who moved the mark and to where.
    c.send(serde_json::json!({"type":"seen","through":1}));
    assert!(
        c.wait(Duration::from_secs(5), |f| f["type"] == "seen"
            && f["through"] == 1)
            .is_some()
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !log(&k2.place).contains("seen_marked") {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(50));
    }
    let text = log(&k2.place);
    assert!(
        text.contains("through=1 newest=1 was=0 now=1 unseen_left=0"),
        "{text}"
    );
    drop(c);
    let mut d = Attach::connect(&k2.url);
    assert!(
        d.wait(Duration::from_secs(2), |f| f["type"] == "notify")
            .is_none(),
        "seen: nothing to replay"
    );
    assert!(
        log(&k2.place).contains("count=0 ids=0..0 seen_through=1"),
        "{}",
        log(&k2.place)
    );
    let _ = k2.child.kill();
}
