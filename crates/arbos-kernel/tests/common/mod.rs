//! Harness for end-to-end tests on the real `arbos-kernel serve` binary:
//! start it on a scratch folder, drive it over the attach socket, and look
//! at what it leaves on disk. Shared by every `*_e2e.rs` test file.
#![allow(dead_code)]

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
    pub scratch: PathBuf,
}

/// Start `serve` on a fresh folder with its own config home. No API key is
/// set (every key variable is scrubbed from the environment), so a prompt
/// fails fast without a model; that is enough to exercise the kernel's own
/// bookkeeping.
pub fn start_kernel(name: &str) -> Kernel {
    start_kernel_with(name, "trace = false\n")
}

/// Start `serve` with the replay provider: every model call answers with
/// the next line of `replies` (one JSON object per line, as `rollout
/// export` writes), so turns run for real and the same way twice with no
/// key and no network.
pub fn start_kernel_replay(name: &str, replies: &str) -> Kernel {
    start_kernel_replay_with(name, replies, "")
}

/// Stop `k` and start a fresh kernel on the same place with a new script:
/// what survives a restart is what is on disk.
pub fn restart_replay(k: &mut Kernel, replies: &str) -> Kernel {
    let _ = k.child.kill();
    let _ = k.child.wait();
    let file = k.scratch.join("replies.jsonl");
    std::fs::write(&file, replies).unwrap();
    let _ = std::fs::remove_file(k.place.join(".arbos").join("runtime").join("kernel.json"));
    spawn_with(
        k.scratch.clone(),
        &[
            "--provider",
            "replay",
            "--replies",
            &file.display().to_string(),
        ],
    )
}

/// `start_kernel_replay` plus extra `config.toml` lines (caps, windows).
pub fn start_kernel_replay_with(name: &str, replies: &str, config: &str) -> Kernel {
    let scratch = scratch_dir(name);
    let file = scratch.join("replies.jsonl");
    std::fs::write(&file, replies).unwrap();
    std::fs::write(
        scratch.join("xdg").join("arbos").join("config.toml"),
        format!("trace = false\n{config}"),
    )
    .unwrap();
    spawn_with(
        scratch,
        &[
            "--provider",
            "replay",
            "--replies",
            &file.display().to_string(),
        ],
    )
}

/// Same, with the given `config.toml` body (e.g. an `api_base` that points
/// at a fake model server).
pub fn start_kernel_with(name: &str, config: &str) -> Kernel {
    let scratch = scratch_dir(name);
    std::fs::write(
        scratch.join("xdg").join("arbos").join("config.toml"),
        config,
    )
    .unwrap();
    spawn(scratch)
}

fn scratch_dir(name: &str) -> PathBuf {
    let scratch = std::env::temp_dir().join(format!(
        "arbos-kernel-{name}-{}-{}",
        std::process::id(),
        arbos_core::now_ms()
    ));
    std::fs::create_dir_all(scratch.join("place")).unwrap();
    std::fs::create_dir_all(scratch.join("xdg").join("arbos")).unwrap();
    std::fs::create_dir_all(scratch.join("home")).unwrap();
    scratch
}

/// A second kernel on the same place and config, after the first one is
/// gone: what a restart looks like.
pub fn restart_kernel(k: &Kernel, config: &str) -> Kernel {
    let xdg = k.scratch.join("xdg");
    std::fs::write(xdg.join("arbos").join("config.toml"), config).unwrap();
    spawn(k.scratch.clone())
}

fn spawn(scratch: PathBuf) -> Kernel {
    spawn_with(scratch, &[])
}

/// Key variables a developer's shell may carry. Scrubbed so a test never
/// reaches a real model by accident: with OpenRouter the default provider,
/// a stray `OPENROUTER_API_KEY` turned "first" into a live turn that ran
/// past the test's 30 s (recreate_e2e on the integration head).
const KEY_VARS: &[&str] = &[
    "OPENAI_API_KEY",
    "OPENROUTER_API_KEY",
    "INCEPTION_API_KEY",
    "ANTHROPIC_API_KEY",
    "ARBOS_API_KEY",
    "ARBOS_MODEL",
    "ARBOS_PROVIDER",
    "ARBOS_REPLIES",
];

pub fn spawn_with(scratch: PathBuf, extra: &[&str]) -> Kernel {
    let place = scratch.join("place");
    let xdg = scratch.join("xdg");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_arbos-kernel"));
    cmd.arg("serve").arg(&place).args(extra);
    for var in KEY_VARS {
        cmd.env_remove(var);
    }
    let child = cmd
        .env("XDG_CONFIG_HOME", &xdg)
        .env("HOME", scratch.join("home"))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn arbos-kernel");
    let kernel_json = place.join(".arbos").join("runtime").join("kernel.json");
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
        scratch,
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

    /// A raw line, for what is not a frame.
    pub fn send_raw(&mut self, line: &str) {
        self.writer.write_all(line.as_bytes()).unwrap();
        self.writer.write_all(b"\n").unwrap();
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
    place.join(".arbos").join("runtime").join("lock")
}
