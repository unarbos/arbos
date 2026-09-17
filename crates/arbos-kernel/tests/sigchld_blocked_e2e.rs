//! Jacob's Mac, 2026-09-17 (`inbox/hung-five-workers`): five workers each
//! ran a one-second python, each job exited 0 with its number in
//! `out.log` — and no tool result ever came; the pythons sat as zombies
//! under the kernel. Root's earlier `bubble_sort.py` had the same shape:
//! `exit` written, the tool returning only at the 600 s floor as "still
//! running". The kernel never saw its children end. One way that happens:
//! the kernel inherits a signal mask with SIGCHLD blocked from the app
//! that launched it, and tokio's reaper — signal-driven — never wakes.
//! Driven here: a helper blocks SIGCHLD and execs the kernel.

mod common;

use common::Attach;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn helper_blocks_sigchld_then_execs_the_kernel() {
    let Ok(place) = std::env::var("ARBOS_SIGCHLD_HELPER_PLACE") else {
        return;
    };
    use std::os::unix::process::CommandExt;
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGCHLD);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
    }
    let replies = std::env::var("ARBOS_SIGCHLD_HELPER_REPLIES").unwrap();
    let err = Command::new(env!("CARGO_BIN_EXE_arbos-kernel"))
        .args([
            "serve",
            &place,
            "--provider",
            "replay",
            "--replies",
            &replies,
        ])
        .exec();
    panic!("exec failed: {err}");
}

#[test]
fn a_job_that_exits_produces_its_tool_result_even_when_sigchld_is_blocked() {
    let scratch = common::scratch_dir("sigchld-blocked");
    let place = scratch.join("place");
    std::fs::create_dir_all(place.join(".arbos/agents/root")).unwrap();
    arbos_core::Agent::root("root")
        .save(&place.join(".arbos/agents/root"))
        .unwrap();
    let replies = scratch.join("replies.jsonl");
    std::fs::write(
        &replies,
        concat!(
            "{\"agent\":\"root\",\"content\":\"rolling\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"python3 -c 'import random; print(random.randint(0,100))' || echo 42\",\"description\":\"Random number\"}}]}\n",
            "{\"agent\":\"root\",\"content\":\"Here is the number.\"}\n",
        ),
    )
    .unwrap();
    std::fs::write(scratch.join("xdg/arbos/config.toml"), "trace = false\n").unwrap();
    let mut helper = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "helper_blocks_sigchld_then_execs_the_kernel",
            "--nocapture",
        ])
        .env("ARBOS_SIGCHLD_HELPER_PLACE", &place)
        .env("ARBOS_SIGCHLD_HELPER_REPLIES", &replies)
        .env("XDG_CONFIG_HOME", scratch.join("xdg"))
        .env("HOME", scratch.join("home"))
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let kernel_json = place.join(".arbos/runtime/kernel.json");
    let deadline = Instant::now() + Duration::from_secs(20);
    let url = loop {
        if let Ok(text) = std::fs::read_to_string(&kernel_json)
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(&text)
            && v["pid"].as_u64() == Some(helper.id() as u64)
        {
            break v["url"].as_str().unwrap().to_string();
        }
        assert!(Instant::now() < deadline, "the kernel came up");
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut a = Attach::connect(&url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    let asked = Instant::now();
    a.send(
        serde_json::json!({"type":"user","agent":"root","text":"roll a number","attachments":[]}),
    );
    // The tool result must come in seconds, not at the 120 s floor.
    let tool = a
        .wait(Duration::from_secs(30), |f| {
            f["type"] == "event"
                && f["event"]["kind"] == "tool"
                && f["event"]["name"] == "bash"
                && f["event"]["ended"].is_number()
        })
        .expect("the finished job produces its tool result");
    let took = asked.elapsed();
    assert!(
        took < Duration::from_secs(20),
        "the result came in {took:?}, not at the wait floor"
    );
    let body = tool["event"]["body"].as_str().unwrap();
    assert!(
        !body.contains("Still running"),
        "the job had exited; the result says so: {body}"
    );
    assert!(tool["event"]["error"].is_null(), "{tool}");
    assert!(
        a.wait(Duration::from_secs(10), |f| f["type"] == "event"
            && f["event"]["kind"] == "assistant"
            && f["event"]["text"] == "Here is the number.")
            .is_some()
    );
    // No zombie left under the kernel.
    std::thread::sleep(Duration::from_millis(500));
    let ps = Command::new("ps")
        .args(["-o", "pid=,ppid=,stat=", "-ax"])
        .output()
        .unwrap();
    let ps = String::from_utf8_lossy(&ps.stdout);
    let zombies: Vec<&str> = ps
        .lines()
        .filter(|l| {
            let cols: Vec<&str> = l.split_whitespace().collect();
            cols.len() >= 3 && cols[1] == helper.id().to_string() && cols[2].starts_with('Z')
        })
        .collect();
    assert!(zombies.is_empty(), "children reaped: {zombies:?}");
    let _ = helper.kill();
    let _ = helper.wait();
    let _ = std::fs::remove_dir_all(&scratch);
}

/// The runtime's reaper never wakes (fault injection, `ARBOS_TEST_NO_CHILD_WAIT`):
/// the wrapper's `exit` file still ends the wait within a second or two,
/// the result carries the output, and the wrapper is reaped by pid.
#[test]
fn a_reaper_that_never_wakes_still_yields_the_result_from_the_exit_file() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"rolling\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"echo 26\",\"description\":\"Random number\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"The number is 26.\"}\n",
    );
    let scratch = common::scratch_dir("no-child-wait");
    let file = scratch.join("replies.jsonl");
    std::fs::write(&file, replies).unwrap();
    std::fs::write(scratch.join("xdg/arbos/config.toml"), "trace = false\n").unwrap();
    let mut k = common::spawn_with_env(
        scratch,
        &["--provider", "replay", "--replies", file.to_str().unwrap()],
        &[("ARBOS_TEST_NO_CHILD_WAIT", "1")],
    );
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    let asked = Instant::now();
    a.send(serde_json::json!({"type":"user","agent":"root","text":"roll","attachments":[]}));
    let tool = a
        .wait(Duration::from_secs(30), |f| {
            f["type"] == "event"
                && f["event"]["kind"] == "tool"
                && f["event"]["name"] == "bash"
                && f["event"]["ended"].is_number()
        })
        .expect("the exit file ends the wait");
    let took = asked.elapsed();
    assert!(took < Duration::from_secs(10), "{took:?}");
    let body = tool["event"]["body"].as_str().unwrap();
    assert!(
        body.contains("26") && !body.contains("Still running"),
        "{body}"
    );
    assert!(
        a.wait(Duration::from_secs(10), |f| f["type"] == "event"
            && f["event"]["kind"] == "assistant"
            && f["event"]["text"] == "The number is 26.")
            .is_some()
    );
    std::thread::sleep(Duration::from_millis(500));
    let ps = Command::new("ps")
        .args(["-o", "pid=,ppid=,stat=", "-ax"])
        .output()
        .unwrap();
    let ps = String::from_utf8_lossy(&ps.stdout);
    let zombies: Vec<&str> = ps
        .lines()
        .filter(|l| {
            let cols: Vec<&str> = l.split_whitespace().collect();
            cols.len() >= 3 && cols[1] == k.child.id().to_string() && cols[2].starts_with('Z')
        })
        .collect();
    assert!(zombies.is_empty(), "reaped by pid: {zombies:?}");
    let _ = k.child.kill();
}
