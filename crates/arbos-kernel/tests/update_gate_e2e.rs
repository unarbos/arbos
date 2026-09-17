//! The self-update design's "watched happening" items 2 and 3, made
//! readable and driven:
//!
//! - A detached job across an `execv`: a helper starts a job exactly as
//!   the kernel does, then execs into `arbos-kernel serve` — same pid, the
//!   updater's shape. The kernel must find the job leashed to itself,
//!   leave it running (`job_inherited`, `jobs_alive` in the log), and the
//!   job must keep writing. Driving this found the boot reap killed it.
//! - The gate from outside: `GET /healthz` says what the self-updater
//!   would decide this second, with the reason.

mod common;

use common::Attach;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
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

/// The helper: start a detached job under the place, then become the
/// kernel. Runs only when the outer test asks for it.
#[test]
fn helper_starts_a_job_then_execs_into_the_kernel() {
    let Ok(place) = std::env::var("ARBOS_EXEC_INTO_KERNEL_PLACE") else {
        return;
    };
    use std::os::unix::process::CommandExt;
    let place = PathBuf::from(place);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let id = rt.block_on(async {
        let root = arbos_engine::JobsRoot::new(place.join(".arbos/agents/root/jobs"));
        let (job, child) = root
            .spawn(
                "for i in $(seq 1 600); do echo beat $i; sleep 0.1; done",
                &place,
                None,
                None,
                vec![],
            )
            .expect("job starts");
        std::mem::forget(child);
        job.id
    });
    std::mem::forget(rt);
    std::fs::write(place.join("job-id"), &id).unwrap();
    let replies = std::env::var("ARBOS_EXEC_INTO_KERNEL_REPLIES").unwrap();
    let err = Command::new(env!("CARGO_BIN_EXE_arbos-kernel"))
        .args([
            "serve",
            place.to_str().unwrap(),
            "--provider",
            "replay",
            "--replies",
            &replies,
        ])
        .exec();
    panic!("exec failed: {err}");
}

#[test]
fn a_job_survives_an_exec_into_the_kernel_and_healthz_shows_the_gate() {
    let scratch = common::scratch_dir("exec-into-kernel");
    let place = scratch.join("place");
    std::fs::create_dir_all(place.join(".arbos/agents/root")).unwrap();
    arbos_core::Agent::root("root")
        .save(&place.join(".arbos/agents/root"))
        .unwrap();
    let replies = scratch.join("replies.jsonl");
    std::fs::write(
        &replies,
        concat!(
            "{\"agent\":\"root\",\"content\":\"working\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 4; echo done\",\"description\":\"Slow step\"}}]}\n",
            "{\"agent\":\"root\",\"content\":\"Done.\"}\n",
        ),
    )
    .unwrap();
    std::fs::write(scratch.join("xdg/arbos/config.toml"), "trace = false\n").unwrap();

    let mut helper = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "helper_starts_a_job_then_execs_into_the_kernel",
            "--nocapture",
        ])
        .env("ARBOS_EXEC_INTO_KERNEL_PLACE", &place)
        .env("ARBOS_EXEC_INTO_KERNEL_REPLIES", &replies)
        .env("XDG_CONFIG_HOME", scratch.join("xdg"))
        .env("HOME", scratch.join("home"))
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("helper");
    let kernel_pid = helper.id();

    // The kernel is up — under the helper's pid.
    let kernel_json = place.join(".arbos/runtime/kernel.json");
    let deadline = Instant::now() + Duration::from_secs(20);
    let url = loop {
        if let Ok(text) = std::fs::read_to_string(&kernel_json)
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(&text)
            && v["pid"].as_u64() == Some(kernel_pid as u64)
        {
            break v["url"].as_str().unwrap().to_string();
        }
        assert!(
            Instant::now() < deadline,
            "the kernel came up under the exec'd pid"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    let id = std::fs::read_to_string(place.join("job-id")).unwrap();
    let jobs = arbos_engine::JobsRoot::new(place.join(".arbos/agents/root/jobs"));
    let job = jobs
        .list()
        .into_iter()
        .find(|j| j.id == id)
        .expect("the job is listed");
    assert!(job.running(), "the job outlived the exec: {job:?}");
    assert_eq!(
        arbos_engine::parent_pid(job.meta.pid),
        Some(kernel_pid),
        "the leash's parent is the kernel's pid"
    );

    // Boot said so, and did not reap it.
    let log_path = place.join(".arbos/runtime/kernel.log");
    let deadline = Instant::now() + Duration::from_secs(10);
    let log = loop {
        let log = std::fs::read_to_string(&log_path).unwrap_or_default();
        if log.contains("\"event\":\"jobs_alive\"") {
            break log;
        }
        assert!(Instant::now() < deadline, "jobs_alive at boot: {log}");
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(log.contains("\"event\":\"job_inherited\""), "{log}");
    assert!(!log.contains("job_reaped"), "not reaped: {log}");
    assert!(
        log.contains(&format!("count=1 {id}:pid={}", job.meta.pid)),
        "{log}"
    );
    // And it keeps writing.
    let out = place
        .join(".arbos/agents/root/jobs")
        .join(&id)
        .join("out.log");
    let before = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    std::thread::sleep(Duration::from_millis(1200));
    let after = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    assert!(
        after > before,
        "output kept coming under the new image: {before} → {after}"
    );

    // The gate from outside: idle, busy with the reason mid-turn, idle
    // after — the running job does not hold it.
    let idle = healthz(&url);
    assert_eq!(idle["update_gate"]["verdict"], "idle", "{idle}");
    assert!(idle["update_gate"].get("reason").is_none());
    let mut a = Attach::connect(&url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(serde_json::json!({"type":"user","agent":"root","text":"do the slow step","attachments":[]}));
    assert!(a.wait_turn("root", "running", Duration::from_secs(10)));
    let busy = healthz(&url);
    assert_eq!(busy["update_gate"]["verdict"], "busy", "{busy}");
    assert!(
        busy["update_gate"]["reason"]
            .as_str()
            .unwrap()
            .contains("turns running: root"),
        "{busy}"
    );
    assert!(a.wait_turn("root", "idle", Duration::from_secs(20)));
    let deadline = Instant::now() + Duration::from_secs(5);
    while healthz(&url)["update_gate"]["verdict"] != "idle" {
        assert!(
            Instant::now() < deadline,
            "idle after the turn: {}",
            healthz(&url)
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        jobs.list()
            .into_iter()
            .find(|j| j.id == id)
            .unwrap()
            .running(),
        "still running after a turn"
    );

    // The kernel dies for real: the leash ends the job with it.
    let _ = helper.kill();
    let _ = helper.wait();
    let deadline = Instant::now() + Duration::from_secs(8);
    while jobs
        .list()
        .into_iter()
        .find(|j| j.id == id)
        .is_some_and(|j| j.running())
    {
        assert!(
            Instant::now() < deadline,
            "the job dies with the kernel's pid"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = std::fs::remove_dir_all(&scratch);
}
