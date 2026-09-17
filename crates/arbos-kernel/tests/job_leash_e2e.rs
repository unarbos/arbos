//! The job leash, driven from every side that has failed or could.
//!
//! QA's machine, 2026-09-16: job `j1` of the `inbox:jobs-leash` run wrote
//! 164 GB into a deleted `out.log` in 4.7 hours. The trace: the model
//! started a `bash background:true` flood, then ran `kill <pid>` on the
//! pid `jobs` displayed — the leash's. The leash forwarded TERM to the
//! wrapper shell only; the loop under it lived on with PPID 1 and nothing
//! polling anything; the kernel stopped cleanly; the scratch folder was
//! removed; `wc -c` on the missing path read 0 and the cap went blind.
//! We displayed a pid whose killing disarmed the safety and orphaned the
//! work.
//!
//! The rules now held, each with a test here:
//! - The pid `jobs` shows is safe to kill: a signal at it ends the whole
//!   group with a `killed` line; the tool routes a literal `kill <pid>`
//!   on a job to the kernel's own kill (engine `kill_by_pid_tests`).
//! - The wrapper polls its parent: a leash killed with `-9` still takes
//!   the job with it.
//! - Leash and wrapper both gone: the next kernel on the place finds the
//!   group under its dead leader and ends it.
//! - A command that backgrounds something leaves it leashed and capped,
//!   not orphaned behind an `exit 0`.
//! - An archived worker's job follows its folder: the kernel tells the
//!   leash the new path before the move; the cap holds there; the job
//!   goes on.
//! - The store gone (the place deleted): the leash ends the job whether
//!   or not the kernel is alive, and the kernel exits when its own store
//!   is gone, so a leaked kernel does not keep every leash satisfied.

mod common;

use common::{Attach, restart_replay, scratch_dir, spawn_with_env, start_kernel_replay};
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

/// The kernel writes into the place while it goes; a delete that races a
/// write is retried.
fn remove_place(place: &std::path::Path) {
    for _ in 0..20 {
        if std::fs::remove_dir_all(place).is_ok() || !place.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    std::fs::remove_dir_all(place).unwrap();
}

fn start_loop(name: &str) -> (common::Kernel, u32) {
    let replies = format!(
        "{{\"agent\":\"root\",\"content\":\"\",\"calls\":[{{\"name\":\"bash\",\"arguments\":{{\"command\":\"{LOOP}\",\"description\":\"Ticks forever\"}}}}]}}\n"
    );
    let k = start_kernel_replay(name, &replies);
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
    remove_place(&k.place);
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
    remove_place(&k.place);
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

/// Live members of the group other than `pgid` itself.
fn members(pgid: u32) -> Vec<u32> {
    std::process::Command::new("pgrep")
        .args(["-g", &pgid.to_string()])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter_map(|l| l.trim().parse::<u32>().ok())
                .filter(|&p| p != pgid)
                .collect()
        })
        .unwrap_or_default()
}

fn children_of(pid: u32) -> Vec<u32> {
    std::process::Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter_map(|l| l.trim().parse::<u32>().ok())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn the_leash_killed_alone_takes_the_job_with_it() {
    let (mut k, pid) = start_loop("leash-killed-alone");
    // Before the wrapper polled its parent: leash dead, wrapper and loop
    // ran on with nothing watching the cap.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
    let gone = wait_group_gone(pid, Duration::from_secs(3));
    let _ = k.child.kill();
    let _ = k.child.wait();
    assert!(gone, "the wrapper and the loop outlived their leash");
}

#[test]
fn shells_both_killed_the_next_kernel_ends_the_orphaned_group() {
    let (mut k, pid) = start_loop("shells-both-killed");
    // The cleanup that matched our shells and not the command: leash and
    // wrapper die in the same instant.
    let wrapper: Vec<u32> = children_of(pid);
    assert!(!wrapper.is_empty(), "the wrapper shell under the leash");
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
        for w in &wrapper {
            libc::kill(*w as libc::pid_t, libc::SIGKILL);
        }
    }
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        !members(pid).is_empty(),
        "the command's own processes survive the shells (the shape under test)"
    );
    // The kernel goes too, unnoticed by anyone.
    let mut k2 = restart_replay(&mut k, "{\"agent\":\"root\",\"content\":\"back\"}\n");
    assert!(
        wait_group_gone(pid, Duration::from_secs(10)),
        "the next kernel found the group under its dead leader and ended it"
    );
    let killed = std::fs::read_dir(k2.place.join(".arbos/agents/root/jobs"))
        .unwrap()
        .flatten()
        .find_map(|e| std::fs::read_to_string(e.path().join("killed")).ok())
        .expect("the job folder says what happened");
    assert!(killed.contains("of its group still ran"), "{killed}");
    let log = std::fs::read_to_string(k2.place.join(".arbos/runtime/kernel.log")).unwrap();
    assert!(log.contains("orphan(s) of its group"), "{log}");
    let _ = k2.child.kill();
}

#[test]
fn an_archived_workers_job_follows_its_folder_and_keeps_its_cap() {
    // ~3 KB/s: past a 2000-byte cap within a second, but under it between
    // two of the leash's looks, so the log is cut back rather than the job
    // ended as runaway.
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"one worker\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"w1\",\"task\":\"start the ticker\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"started\"}\n",
        "{\"content\":\"starting\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"while :; do head -c 300 /dev/zero | tr '\\\\0' x; echo; sleep 0.1; done\",\"background\":true,\"description\":\"A ticker that runs on\"}}]}\n",
        "{\"content\":\"ticker running\"}\n",
        "{\"agent\":\"root\",\"content\":\"noted\"}\n",
    );
    let dir = scratch_dir("archive-follows");
    let file = dir.join("replies.jsonl");
    std::fs::write(&file, replies).unwrap();
    std::fs::write(dir.join("xdg/arbos/config.toml"), "trace = false\n").unwrap();
    let replies_path = file.display().to_string();
    let mut k = spawn_with_env(
        dir,
        &["--provider", "replay", "--replies", &replies_path],
        &[("ARBOS_JOB_LOG_CAP", "2000")],
    );
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(serde_json::json!({"type":"user","agent":"root","text":"start it","attachments":[]}));
    let archived = k.place.join(".arbos/archive/agents/w1");
    let deadline = Instant::now() + Duration::from_secs(40);
    while !archived.join("transcript.jsonl").exists() {
        assert!(Instant::now() < deadline, "w1 was never archived");
        std::thread::sleep(Duration::from_millis(150));
    }
    // The job moved with the folder, and its leash was told.
    let job_dir = std::fs::read_dir(archived.join("jobs"))
        .expect("the job folder moved with the worker")
        .flatten()
        .map(|e| e.path())
        .find(|p| p.join("meta.json").exists())
        .expect("a job under the archived worker");
    let meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(job_dir.join("meta.json")).unwrap()).unwrap();
    let pid = meta["pid"].as_u64().unwrap() as u32;
    let pointer = k.place.join(".arbos/runtime/leash").join(pid.to_string());
    assert_eq!(
        std::fs::read_to_string(&pointer)
            .map(|s| s.trim().to_string())
            .ok(),
        Some(job_dir.display().to_string()),
        "the pointer names the new folder"
    );
    let log = std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.log")).unwrap();
    assert!(log.contains("\"event\":\"jobs_repointed\""), "{log}");

    // The job runs on after the move, and the cap holds at the new path.
    let deadline = Instant::now() + Duration::from_secs(15);
    let out = job_dir.join("out.log");
    let cut = loop {
        let text = std::fs::read_to_string(&out).unwrap_or_default();
        if text.contains("[arbos: out.log passed 2000 bytes") {
            break true;
        }
        if Instant::now() > deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    assert!(
        cut,
        "the log at the archived path was never cut back to the cap"
    );
    assert!(group_alive(pid), "the move must not end the job");
    assert!(
        std::fs::read_to_string(&out).unwrap_or_default().len() < 8000,
        "the cap holds"
    );
    // The kernel ends: the leash follows it, pointer and all.
    let _ = k.child.kill();
    let _ = k.child.wait();
    assert!(wait_group_gone(pid, Duration::from_secs(3)));
    assert!(
        !pointer.exists(),
        "the leash removed its pointer on the way out"
    );
}

#[test]
fn a_command_that_backgrounds_something_leaves_it_leashed_and_capped() {
    // Not a server by the tool's reading (it does not end in `&`), so it
    // runs attached; the command returns at once and leaves a writer
    // behind in its group. Before: the leash left with the wrapper, and
    // the writer ran on with no cap, no kernel tie, and a folder that
    // read "exit 0".
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"(while :; do head -c 300 /dev/zero | tr '\\\\0' x; echo; sleep 0.1; done) & echo started\",\"description\":\"Start a writer and return\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"Started.\"}\n",
    );
    let dir = scratch_dir("survivors-leashed");
    let file = dir.join("replies.jsonl");
    std::fs::write(&file, replies).unwrap();
    std::fs::write(dir.join("xdg/arbos/config.toml"), "trace = false\n").unwrap();
    let replies_path = file.display().to_string();
    let mut k = spawn_with_env(
        dir,
        &["--provider", "replay", "--replies", &replies_path],
        &[("ARBOS_JOB_LOG_CAP", "2000")],
    );
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    let started = Instant::now();
    a.send(serde_json::json!({"type":"user","agent":"root","text":"start it","attachments":[]}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "the call returned with the command, not with its survivors: {:?}",
        started.elapsed()
    );
    let pid = job_pid(&k.place);
    let job_dir = std::fs::read_dir(k.place.join(".arbos/agents/root/jobs"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.join("meta.json").exists())
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(job_dir.join("exit"))
            .map(|s| s.trim().to_string())
            .ok(),
        Some("0".into()),
        "the command's own exit is on record"
    );
    // The writer runs on — leashed: the cap holds on its output.
    assert!(group_alive(pid), "the survivor is still there");
    let out = job_dir.join("out.log");
    let deadline = Instant::now() + Duration::from_secs(15);
    while !std::fs::read_to_string(&out)
        .unwrap_or_default()
        .contains("[arbos: out.log passed 2000 bytes")
    {
        assert!(
            Instant::now() < deadline,
            "the survivor's output was never capped"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    // And tied to the kernel.
    let _ = k.child.kill();
    let _ = k.child.wait();
    assert!(
        wait_group_gone(pid, Duration::from_secs(3)),
        "the survivor outlived the kernel"
    );
}

#[test]
fn a_signal_at_the_pid_jobs_shows_ends_the_whole_job() {
    // QA's trace: `kill <pid>` on the pid `jobs` displayed. The leash used
    // to forward TERM to the wrapper shell only; the loop lived on with
    // PPID 1 and nothing polling anything.
    let (mut k, pid) = start_loop("kill-displayed-pid");
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }
    let gone = wait_group_gone(pid, Duration::from_secs(4));
    let killed = std::fs::read_dir(k.place.join(".arbos/agents/root/jobs"))
        .unwrap()
        .flatten()
        .find_map(|e| std::fs::read_to_string(e.path().join("killed")).ok());
    let _ = k.child.kill();
    let _ = k.child.wait();
    assert!(gone, "the loop outlived a TERM at the job's pid");
    let killed = killed.expect("the folder says what ended it");
    assert!(
        killed.contains("the whole job, not only its shell"),
        "{killed}"
    );
}

#[test]
fn the_models_kill_through_its_own_shell_still_ends_the_whole_job() {
    // A `kill` the tool does not intercept (the pid comes from a
    // subshell, so the command is compound): it reaches the leash as a
    // plain TERM, the way QA's model's did — and the leash now ends its
    // group. The intercepted form, `kill <pid>` with the literal pid, is
    // driven in the engine's `kill_by_pid_tests` (a replay script cannot
    // carry a pid it does not know).
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"while :; do echo tick; sleep 0.05; done\",\"background\":true,\"description\":\"Ticks forever\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"Ticking.\"}\n",
        "{\"agent\":\"root\",\"content\":\"\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"kill $(sed -n 's/.*\\\"pid\\\":\\\\([0-9]*\\\\).*/\\\\1/p' .arbos/agents/root/jobs/j1/meta.json)\",\"description\":\"Stop the ticker\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"Stopped.\"}\n",
    );
    let mut k = start_kernel_replay("model-kills-job-pid", replies);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(serde_json::json!({"type":"user","agent":"root","text":"start the ticker","attachments":[]}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let pid = job_pid(&k.place);
    assert!(group_alive(pid));
    a.send(serde_json::json!({"type":"user","agent":"root","text":"stop it","attachments":[]}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(
        wait_group_gone(pid, Duration::from_secs(4)),
        "the ticker outlived the model's kill"
    );
    let killed = std::fs::read_to_string(k.place.join(".arbos/agents/root/jobs/j1/killed"))
        .expect("the folder says what ended it");
    assert!(
        killed.contains("the whole job, not only its shell"),
        "{killed}"
    );
    let _ = k.child.kill();
}

#[test]
fn a_subscription_command_that_backgrounds_a_child_still_delivers_at_once() {
    // qal-j07 (QA's `sb-01`): on this branch before it took #371's
    // exit-file wait in `subs::run_job`, a shell subscription whose
    // command backgrounded a child was held for the child's life and its
    // reading never arrived. The run must end with the command; the
    // child stays leashed and capped behind it.
    let now = arbos_core::now_ms() - 1_000;
    let due = arbos_core::inbox::rfc3339(now);
    let mut k = common::start_kernel_replay_prepared(
        "sub-backgrounds",
        "{\"agent\":\"root\",\"content\":\"hi\"}\n",
        "",
        |place| {
            let dir = place.join(".arbos/agents/root/subscriptions");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("0001-bg.toml"),
                format!(
                    "kind = \"shell\"\nevery = \"30s\"\ncmd = \"nohup sleep 300 >/dev/null 2>&1 & echo started-bg\"\ndeliver_to = \"user\"\nnotify = \"bg: {{output}}\"\nnext_due = \"{due}\"\n"
                ),
            )
            .unwrap();
        },
    );
    let mut a = Attach::connect(&k.url);
    let asked = Instant::now();
    let _ = a
        .wait(Duration::from_secs(20), |f| {
            f["type"] != "snapshot" && f.to_string().contains("bg: started-bg")
        })
        .expect("the reading reached the window");
    assert!(asked.elapsed() < Duration::from_secs(20));
    let pid = job_pid(&k.place);
    assert!(
        group_alive(pid),
        "the backgrounded child is still there, leashed, after the reading was delivered"
    );
    let _ = k.child.kill();
    let _ = k.child.wait();
    assert!(
        wait_group_gone(pid, Duration::from_secs(3)),
        "and dies with the kernel"
    );
}
