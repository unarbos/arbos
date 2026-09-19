//! The self-update design's sixth "watched happening" item: a subscription
//! run in flight holds the update gate — driven, and readable. A shell
//! subscription's command runs for seconds; `/healthz` says busy and
//! names the run; after it settles, idle. A kernel killed mid-run says so
//! at the next boot (`subscription_run_cut`) and on the subscription's
//! row, rather than losing the run in silence.

mod common;

use common::{Attach, restart_replay, start_kernel_replay};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

fn healthz(url: &str) -> serde_json::Value {
    let addr = url.trim_start_matches("tcp://");
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        s,
        "GET /healthz HTTP/1.1\r\nHost: x\r\nUser-Agent: curl/8.0\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    let body = out.split("\r\n\r\n").nth(1).unwrap_or("");
    serde_json::from_str(body.trim()).unwrap()
}

#[test]
fn a_subscription_run_in_flight_holds_the_gate_by_name_and_a_cut_run_is_said_at_boot() {
    // The model sets up a shell subscription firing at once (every 1h,
    // first run now), whose command takes six seconds.
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"setting up\",\"calls\":[{\"name\":\"subscribe\",\"arguments\":{\"op\":\"add\",\"kind\":\"shell\",\"cmd\":\"sleep 6; echo checked\",\"every\":\"1h\",\"deliver_to\":\"none\",\"prompt\":\"a check\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"Set up.\"}\n",
    );
    let mut k = start_kernel_replay("subs-in-flight", replies);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(serde_json::json!({"type":"user","agent":"root","text":"set up the check","attachments":[]}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(20)));
    // #1 is the kernel's weekly gc chore; the check is #2. Its first fire
    // is an hour out: make it due now, as the watcher would find it.
    let place = arbos_core::Place::new(&k.place);
    let due_now = |k: &common::Kernel| {
        let place = arbos_core::Place::new(&k.place);
        let mut sub = arbos_core::subscription::get(&place, "root", 2).unwrap();
        sub.next_due = Some(arbos_core::inbox::rfc3339(arbos_core::now_ms()));
        arbos_core::subscription::save(&place, "root", &sub).unwrap();
    };
    due_now(&k);
    // The run starts: busy, and the reason names it.
    let deadline = Instant::now() + Duration::from_secs(15);
    let busy = loop {
        let h = healthz(&k.url);
        if h["update_gate"]["verdict"] == "busy"
            && h["update_gate"]["reason"]
                .as_str()
                .is_some_and(|r| r.contains("subscription runs in flight"))
        {
            break h;
        }
        assert!(Instant::now() < deadline, "the run holds the gate: {h}");
        std::thread::sleep(Duration::from_millis(200));
    };
    let reason = busy["update_gate"]["reason"].as_str().unwrap();
    assert!(
        reason.contains("root#2 shell `sleep 6; echo checked`"),
        "{reason}"
    );
    assert!(reason.contains("s)"), "with how long it has run: {reason}");
    // The gate says "in flight" the moment the run is claimed; the job
    // folder and its marker land a beat later (CI, 2026-09-19: the marker
    // asked for before it was written). Give them the beat.
    let jobs = k.place.join(".arbos/agents/root/jobs");
    let find_marked = || {
        std::fs::read_dir(&jobs)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.join("subscription").exists())
    };
    assert!(
        common::wait_for(Duration::from_secs(5), || find_marked().is_some()),
        "the run's job carries the subscription marker"
    );
    let job_dir = find_marked().unwrap();
    assert_eq!(
        std::fs::read_to_string(job_dir.join("subscription"))
            .unwrap()
            .trim(),
        "2"
    );
    // It settles: idle, the row shows the outcome, the job marked settled.
    let deadline = Instant::now() + Duration::from_secs(20);
    while healthz(&k.url)["update_gate"]["verdict"] != "idle" {
        assert!(
            Instant::now() < deadline,
            "idle after the run: {}",
            healthz(&k.url)
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while !job_dir.join("settled").exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(100));
    }
    let sub = arbos_core::subscription::get(&place, "root", 2).unwrap();
    assert!(sub.last.contains("exit 0"), "{}", sub.last);

    // A second firing, cut by a kernel death mid-run.
    due_now(&k);
    let deadline = Instant::now() + Duration::from_secs(15);
    let second = loop {
        let found = std::fs::read_dir(&jobs)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|p| p.join("subscription").exists() && !p.join("settled").exists());
        if let Some(p) = found {
            break p;
        }
        assert!(Instant::now() < deadline, "the second run starts");
        std::thread::sleep(Duration::from_millis(100));
    };
    // Mid-run: the gate says so by name.
    let h = healthz(&k.url);
    assert!(
        h["update_gate"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("root#2 shell")),
        "{h}"
    );
    let mut k2 = restart_replay(&mut k, "");
    let log_path = k2.place.join(".arbos/runtime/kernel.log");
    let deadline = Instant::now() + Duration::from_secs(10);
    let line = loop {
        let log = std::fs::read_to_string(&log_path).unwrap_or_default();
        if let Some(l) = log
            .lines()
            .find(|l| l.contains("\"event\":\"subscription_run_cut\""))
        {
            break l.to_string();
        }
        assert!(
            Instant::now() < deadline,
            "the cut run is named at boot: {log}"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    let id = second.file_name().unwrap().to_string_lossy().to_string();
    assert!(line.contains(&format!("#2 job {id}")), "{line}");
    assert!(second.join("settled").exists(), "marked so it is said once");
    let sub = arbos_core::subscription::get(&arbos_core::Place::new(&k2.place), "root", 2).unwrap();
    assert!(
        sub.last.starts_with("run cut by a kernel restart"),
        "{}",
        sub.last
    );
    assert!(sub.next_due.is_some(), "it fires again");
    let _ = k2.child.kill();
}
