//! `surfaces`: a client asks a kernel what it holds — jobs, shells, browser
//! pages, with their states — so a window that reattaches after a kernel
//! died and a replacement answered reconciles its rows against the record
//! instead of its memory. Watched live: both tabs read `link lost`, then
//! flipped back to `running` when the new kernel answered, with nothing
//! behind either.

mod common;

use common::{Attach, restart_replay, start_kernel_replay};
use std::time::Duration;

fn ask(a: &mut Attach, agent: Option<&str>) -> serde_json::Value {
    let mut req = serde_json::json!({"type": "surfaces"});
    if let Some(agent) = agent {
        req["agent"] = serde_json::json!(agent);
    }
    a.send(req);
    a.wait(Duration::from_secs(10), |f| f["type"] == "surface_list")
        .expect("a surface_list answer")
}

fn rows<'a>(list: &'a serde_json::Value, panel: &str) -> Vec<&'a serde_json::Value> {
    list["surfaces"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["panel"] == panel)
        .collect()
}

#[test]
fn a_replacement_kernel_says_what_it_holds_and_the_dead_ones_rows_are_not_in_it() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"starting both\",\"calls\":[",
        "{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 300\",\"background\":true,\"description\":\"A long sleep that runs on\"}},",
        "{\"name\":\"terminal\",\"arguments\":{\"action\":\"open\"}}",
        "]}\n",
        "{\"agent\":\"root\",\"content\":\"both open\"}\n",
    );
    let mut k = start_kernel_replay("surfaces", replies);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(
        serde_json::json!({"type": "user", "agent": "root", "text": "open a job and a terminal"}),
    );
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));

    // The living kernel: one job running, one shell alive, both root's.
    let list = ask(&mut a, None);
    assert!(list["at_ms"].as_i64().is_some_and(|t| t > 0), "{list}");
    let jobs = rows(&list, "process");
    assert_eq!(jobs.len(), 1, "{list:#}");
    let job = jobs[0];
    assert_eq!(job["owner"], "root");
    assert_eq!(job["running"], true, "{job}");
    assert_eq!(job["title"], "sleep 300", "{job}");
    assert_eq!(job["journal"], "present", "{job}");
    assert!(job["pid"].as_u64().is_some_and(|p| p > 0), "{job}");
    assert!(job["started_ms"].as_i64().is_some_and(|t| t > 0), "{job}");
    assert!(
        job["status"]
            .as_str()
            .unwrap_or("")
            .starts_with("running for"),
        "{job}"
    );
    assert!(job["cwd"].is_string() && job["url"].is_string(), "{job}");
    let job_id = job["id"].as_str().unwrap().to_string();
    let shells = rows(&list, "terminal");
    assert_eq!(shells.len(), 1, "{list:#}");
    assert_eq!(shells[0]["owner"], "root");
    assert_eq!(shells[0]["running"], true, "{}", shells[0]);
    assert!(
        shells[0]["status"]
            .as_str()
            .unwrap_or("")
            .starts_with("shell alive"),
        "{}",
        shells[0]
    );
    let shell_id = shells[0]["id"].as_str().unwrap().to_string();
    assert!(rows(&list, "browser").is_empty());

    // Scoped to an agent nobody has: an error, no list.
    a.send(serde_json::json!({"type": "surfaces", "agent": "ghost"}));
    let err = a
        .wait(Duration::from_secs(10), |f| f["type"] == "error")
        .expect("an error frame");
    assert!(
        err["detail"]
            .as_str()
            .unwrap_or("")
            .contains("no agent is named ghost"),
        "{err}"
    );
    drop(a);

    // The kernel dies; a replacement answers on the same place. The window
    // still holds a job row and a terminal row from its memory.
    let mut k = restart_replay(&mut k, "{\"agent\":\"root\",\"content\":\"back\"}\n");
    let mut b = Attach::connect(&k.url);
    let snap = b
        .wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
        .expect("a snapshot");
    // The snapshot itself carries the list, so a window that does not
    // know to ask still rebuilds from the kernel's record: the job is
    // there (the disk record), the shell is not (it died with its kernel).
    let snap_rows = |panel: &str| {
        snap["surfaces"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|s| s["panel"] == panel)
            .count()
    };
    assert_eq!(snap_rows("process"), 1, "{snap:#}");
    assert_eq!(snap_rows("terminal"), 0, "{snap:#}");
    // The shell died with its kernel: not in the list at all. The job is
    // the kernel's record on disk, so it is listed — and once the leash has
    // ended it (the kernel that owned it is gone), it is listed as not
    // running, with the kernel's words for why.
    let list = ask(&mut b, Some("root"));
    assert_eq!(list["agent"], "root");
    assert!(
        rows(&list, "terminal").is_empty(),
        "no shell is behind {shell_id} in the new kernel: {list:#}"
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let job = loop {
        let list = ask(&mut b, Some("root"));
        let jobs = rows(&list, "process");
        assert_eq!(jobs.len(), 1, "{list:#}");
        assert_eq!(jobs[0]["id"], job_id.as_str(), "{list:#}");
        if jobs[0]["running"] == false {
            break jobs[0].clone();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the leash ends a job whose kernel died: {list:#}"
        );
        std::thread::sleep(Duration::from_millis(500));
    };
    assert!(
        !job["status"].as_str().unwrap_or("").starts_with("running"),
        "{job}"
    );
    assert_eq!(job["journal"], "present", "{job}");
    let _ = k.child.kill();
}
