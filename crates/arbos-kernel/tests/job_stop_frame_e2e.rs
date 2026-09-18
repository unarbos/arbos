//! Stop a job from a window (side-panels handover 4).
//!
//! The drawer's job row has a Stop; until now it could only put words in
//! the composer. A `job_stop` frame routes through the kernel's own kill:
//! the whole group ends, the `killed` marker is written once — by the
//! kill, before the signal — and says who pressed it, and the end travels
//! the paths every job end does: the `board` close with the status line,
//! and the agent's wake. A second Stop on a job already over answers with
//! the final `job` frame so the row settles; an unknown id is an error.

mod common;

use common::{Attach, start_kernel_replay};
use std::time::{Duration, Instant};

const LOOP: &str = "while :; do echo tick; sleep 0.05; done";

/// The one job folder under root: `(id, pid)`.
fn the_job(place: &std::path::Path) -> (String, u32) {
    let jobs = place.join(".arbos/agents/root/jobs");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(rd) = std::fs::read_dir(&jobs) {
            for e in rd.flatten() {
                if let Ok(text) = std::fs::read_to_string(e.path().join("meta.json"))
                    && let Ok(v) = serde_json::from_str::<serde_json::Value>(&text)
                    && let Some(pid) = v["pid"].as_u64()
                {
                    return (e.file_name().to_string_lossy().into_owned(), pid as u32);
                }
            }
        }
        assert!(Instant::now() < deadline, "no job folder with a pid");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn group_alive(pgid: u32) -> bool {
    let Ok(out) = std::process::Command::new("pgrep")
        .args(["-g", &pgid.to_string()])
        .output()
    else {
        return false;
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .any(|pid| {
            std::process::Command::new("ps")
                .args(["-o", "stat=", "-p", pid])
                .output()
                .map(|o| {
                    let stat = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    !stat.is_empty() && !stat.starts_with('Z')
                })
                .unwrap_or(false)
        })
}

#[test]
fn a_window_stops_a_job_through_the_kernels_own_kill() {
    let replies = format!(
        "{{\"agent\":\"root\",\"content\":\"Started the ticker.\",\"calls\":[{{\"name\":\"bash\",\"arguments\":{{\"command\":\"{LOOP}\",\"description\":\"Ticks forever\",\"background\":true}}}}]}}\n"
    );
    let k = start_kernel_replay("job-stop-frame", &replies);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(
        serde_json::json!({"type":"user","agent":"root","text":"tick forever in the background","attachments":[]}),
    );
    assert!(a.wait_turn("root", "running", Duration::from_secs(10)));
    let (id, pid) = the_job(&k.place);
    assert!(group_alive(pid), "the ticker runs");
    // The turn ends with the job detached behind it.
    assert!(a.wait_turn("root", "idle", Duration::from_secs(20)));

    // An id nobody has: an error names it, nothing is killed.
    a.send(serde_json::json!({"type":"job_stop","agent":"root","id":"j99"}));
    let err = a
        .wait(Duration::from_secs(5), |f| f["type"] == "error")
        .expect("an error frame");
    assert!(
        err["detail"]
            .as_str()
            .unwrap()
            .contains("has no job \"j99\""),
        "{err}"
    );
    assert!(group_alive(pid), "the ticker still runs");

    // Stop from the window.
    a.send(serde_json::json!({"type":"job_stop","agent":"root","id":id}));
    let deadline = Instant::now() + Duration::from_secs(10);
    while group_alive(pid) {
        assert!(Instant::now() < deadline, "the group is still alive");
        std::thread::sleep(Duration::from_millis(100));
    }
    let marker = k
        .place
        .join(".arbos/agents/root/jobs")
        .join(&id)
        .join("killed");
    let line = std::fs::read_to_string(&marker).expect("the killed marker");
    assert_eq!(
        line.trim(),
        "killed: stopped by the user from the window",
        "one writer, and it says who"
    );
    // The end travels the usual paths: the row's close with the status
    // line, and the agent's wake.
    let close = a
        .wait(Duration::from_secs(15), |f| {
            f["type"] == "board" && f["action"] == "close" && f["panel"] == "process"
        })
        .expect("the board close");
    assert!(
        close["title"]
            .as_str()
            .unwrap()
            .contains("stopped by the user from the window"),
        "{close}"
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    let transcript = k.place.join(".arbos/agents/root/transcript.jsonl");
    loop {
        let text = std::fs::read_to_string(&transcript).unwrap_or_default();
        if text.contains("stopped by the user from the window") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the agent never heard who stopped its job:\n{text}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    // Stop again on a job that is over: the row gets the last word.
    a.send(serde_json::json!({"type":"job_stop","agent":"root","id":id}));
    let last = a
        .wait(Duration::from_secs(5), |f| {
            f["type"] == "job" && f["id"] == id && f["running"] == false
        })
        .expect("the final job frame again");
    assert!(last["exit"].is_null(), "killed: no exit code — {last}");
}
