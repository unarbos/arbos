//! End-to-end checks on the real `arbos-kernel serve` binary: start it on a
//! scratch folder, drive it over the attach socket, and look at what it
//! leaves on disk. These are the QA loop's scenarios, kept where CI runs.

use std::{
    io::{BufRead, BufReader, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

pub struct Kernel {
    pub child: Child,
    pub place: PathBuf,
    pub url: String,
    _scratch: PathBuf,
}

/// Start `serve` on a fresh folder with its own config home. No API key is
/// set, so a prompt fails fast without a model; that is enough to exercise
/// the kernel's own bookkeeping.
pub fn start_kernel(name: &str) -> Kernel {
    let scratch = std::env::temp_dir().join(format!(
        "arbos-kernel-{name}-{}-{}",
        std::process::id(),
        arbos_core::now_ms()
    ));
    let place = scratch.join("place");
    let xdg = scratch.join("xdg");
    std::fs::create_dir_all(&place).unwrap();
    std::fs::create_dir_all(xdg.join("arbos")).unwrap();
    std::fs::write(xdg.join("arbos").join("config.toml"), "trace = false\n").unwrap();
    std::fs::create_dir_all(scratch.join("home")).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_arbos-kernel"))
        .arg("serve")
        .arg(&place)
        .env("XDG_CONFIG_HOME", &xdg)
        .env("HOME", scratch.join("home"))
        .env_remove("OPENAI_API_KEY")
        .env_remove("INCEPTION_API_KEY")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn arbos-kernel");
    let kernel_json = place.join(".arbos").join("kernel.json");
    let deadline = Instant::now() + Duration::from_secs(20);
    let url = loop {
        if let Ok(text) = std::fs::read_to_string(&kernel_json) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                if v["pid"].as_u64() == Some(child.id() as u64) {
                    break v["url"].as_str().unwrap().to_string();
                }
            }
        }
        assert!(Instant::now() < deadline, "kernel never wrote kernel.json");
        std::thread::sleep(Duration::from_millis(50));
    };
    Kernel {
        child,
        place,
        url,
        _scratch: scratch,
    }
}

pub struct Attach {
    writer: TcpStream,
    reader: BufReader<TcpStream>,
}

impl Attach {
    pub fn connect(url: &str) -> Self {
        let addr = url.trim_start_matches("tcp://");
        let stream = TcpStream::connect(addr).expect("attach");
        stream
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        Self {
            writer: stream.try_clone().unwrap(),
            reader: BufReader::new(stream),
        }
    }

    pub fn send(&mut self, frame: serde_json::Value) {
        let mut line = frame.to_string();
        line.push('\n');
        self.writer.write_all(line.as_bytes()).unwrap();
    }

    /// Frames until `pred` matches one, or the timeout. Returns the match.
    pub fn wait(
        &mut self,
        timeout: Duration,
        mut pred: impl FnMut(&serde_json::Value) -> bool,
    ) -> Option<serde_json::Value> {
        let deadline = Instant::now() + timeout;
        let mut line = String::new();
        while Instant::now() < deadline {
            line.clear();
            match self.reader.read_line(&mut line) {
                Ok(0) => return None,
                Ok(_) => {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                        if pred(&v) {
                            return Some(v);
                        }
                    }
                }
                Err(_) => continue,
            }
        }
        None
    }

    pub fn wait_turn(&mut self, agent: &str, state: &str, timeout: Duration) -> bool {
        self.wait(timeout, |f| {
            f["type"] == "turn" && f["agent"] == agent && f["state"] == state
        })
        .is_some()
    }
}

pub fn sigint(child: &Child) {
    let status = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("kill -INT");
    assert!(status.success());
}

/// Exit status within `timeout`, or None if the process is still running.
pub fn wait_exit(child: &mut Child, timeout: Duration) -> Option<i32> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            return status.code();
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

pub fn lock_path(place: &Path) -> PathBuf {
    place.join(".arbos").join("lock")
}

/// qa-003: a 4 MB prompt used to make the serve loop re-parse megabytes
/// of transcript five times a second and the plan several times per turn,
/// so Ctrl-C went unanswered for more than ten seconds and the lock file
/// stayed behind after the kill.
#[test]
fn a_huge_prompt_does_not_stop_ctrl_c_from_ending_the_kernel_cleanly() {
    let mut k = start_kernel("huge");
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );

    let big = format!("Reply OK.\n{}", "lorem ipsum ".repeat(350_000));
    assert!(big.len() > 4_000_000);
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": big}));
    assert!(
        a.wait_turn("root", "idle", Duration::from_secs(60)),
        "the huge turn never reached idle"
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "Reply FINE."}));
    assert!(
        a.wait_turn("root", "idle", Duration::from_secs(60)),
        "the follow-up turn never reached idle"
    );

    let asked = Instant::now();
    sigint(&k.child);
    let code = wait_exit(&mut k.child, Duration::from_secs(4));
    if code.is_none() {
        let _ = k.child.kill();
    }
    assert_eq!(
        code,
        Some(0),
        "kernel must exit 0 within 4s of Ctrl-C (took {:?}, None = still running)",
        asked.elapsed()
    );
    assert!(
        !lock_path(&k.place).exists(),
        "a clean stop removes .arbos/lock"
    );
}
