//! QA's machine: a job from a scenario outlived its kernel and its scratch
//! folder and wrote 164 GB into a deleted `out.log`. The leash watched
//! only the kernel's pid, and with the job folder gone its log cap read
//! the missing file as 0 bytes. Two answers, both driven here:
//!
//! - The leash ends the job when the place's `.arbos` store is gone,
//!   whether or not the kernel is alive (the kernel may be the leak).
//! - The kernel exits when its own store is gone from under it, so a
//!   leaked kernel does not keep every job's leash satisfied.

mod common;

use common::{Attach, start_kernel_replay};
use std::time::{Duration, Instant};

const LOOP: &str = "while :; do echo tick; sleep 0.05; done";

/// The leash's pid (the job's process-group leader) from the job folder.
fn job_pid(place: &std::path::Path) -> u32 {
    let jobs = place.join(".arbos/agents/root/jobs");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(rd) = std::fs::read_dir(&jobs) {
            for e in rd.flatten() {
                if let Ok(text) = std::fs::read_to_string(e.path().join("meta.json"))
                    && let Ok(v) = serde_json::from_str::<serde_json::Value>(&text)
                    && let Some(pid) = v["pid"].as_u64()
                {
                    return pid as u32;
                }
            }
        }
        assert!(Instant::now() < deadline, "no job folder with a pid");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Any live (not zombie) process in the group. A leash that exited under
/// a stopped kernel stays a zombie in the table until the kernel reaps
/// it; that is a dead job, not a running one.
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

fn wait_group_gone(pgid: u32, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if !group_alive(pgid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

fn start_loop(name: &str) -> (common::Kernel, u32) {
    let replies = format!(
        "{{\"agent\":\"root\",\"content\":\"\",\"calls\":[{{\"name\":\"bash\",\"arguments\":{{\"command\":\"{LOOP}\",\"description\":\"Ticks forever\"}}}}]}}\n"
    );
    let mut k = start_kernel_replay(name, &replies);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(
        serde_json::json!({"type":"user","agent":"root","text":"tick forever","attachments":[]}),
    );
    assert!(a.wait_turn("root", "running", Duration::from_secs(10)));
    let pid = job_pid(&k.place);
    // The whole group is there: leash, wrapper shell, the loop.
    assert!(group_alive(pid));
    (k, pid)
}

#[test]
fn a_leaked_kernel_that_lost_its_store_exits_and_its_jobs_end() {
    let (mut k, pid) = start_loop("store-gone-kernel");
    std::fs::remove_dir_all(&k.place).unwrap();
    // Two five-second looks, then the exit.
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Ok(Some(s)) = k.child.try_wait() {
            break s;
        }
        assert!(
            Instant::now() < deadline,
            "the kernel served on with no store"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(status.code(), Some(4), "{status:?}");
    assert!(
        wait_group_gone(pid, Duration::from_secs(3)),
        "the job's group outlived the kernel"
    );
}

#[test]
fn the_leash_ends_a_job_whose_store_is_gone_even_with_the_kernel_alive() {
    let (mut k, pid) = start_loop("store-gone-leash");
    // A kernel that is alive but does nothing — the leak on QA's machine
    // as the leash saw it: `kill -0` says the parent is there.
    unsafe {
        libc::kill(k.child.id() as libc::pid_t, libc::SIGSTOP);
    }
    std::fs::remove_dir_all(&k.place).unwrap();
    let gone = wait_group_gone(pid, Duration::from_secs(3));
    unsafe {
        libc::kill(k.child.id() as libc::pid_t, libc::SIGCONT);
    }
    let _ = k.child.kill();
    let _ = k.child.wait();
    assert!(
        gone,
        "the job ran on with its store deleted while the kernel stood still"
    );
}
