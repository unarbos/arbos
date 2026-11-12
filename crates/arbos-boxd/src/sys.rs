//! The one seam between the box state machine and the host: run a command, and ask a box's
//! agentd to prepare for sleep. Everything `boxes.rs` does to the system goes through here, so
//! the tests drive the same code against [`fake::Fake`] with no root and no Linux.

use std::net::Ipv4Addr;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};

/// agentd's box port inside every sandbox (`ARBOS_PORT` in the image).
pub const AGENTD_PORT: u16 = 7788;

/// No host command here runs longer: the slowest are a rootfs unpack and a 16 GiB checkpoint.
/// Past it the child is killed, so a hung `umount` cannot hold a box's lock for ever.
const CMD_TIMEOUT: Duration = Duration::from_secs(600);

/// prepare-sleep walks the box's disk and may vacuum its stores.
const PREPARE_SLEEP_TIMEOUT: Duration = Duration::from_secs(120);

pub struct Out {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Out {
    /// The command's stdout, or its stderr as the error.
    pub fn ok(self, what: &str) -> Result<String> {
        if self.ok {
            Ok(self.stdout.trim().to_owned())
        } else {
            Err(anyhow!("{what} failed: {}", self.stderr.trim()))
        }
    }
}

#[async_trait::async_trait]
pub trait Sys: Send + Sync {
    /// Run `argv` to completion; never through a shell. With `log`, stdout and stderr are
    /// appended to that file instead of captured: a detached sandbox keeps both open for its
    /// whole life, and a pipe would never close.
    async fn cmd(&self, argv: &[&str], log: Option<&Path>) -> Result<Out>;

    /// `POST /box/prepare-sleep` on the agentd at `ip`: the status and the body.
    async fn prepare_sleep(&self, ip: Ipv4Addr, token: &str) -> Result<(u16, String)>;
}

pub struct Host {
    http: reqwest::Client,
}

impl Host {
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .no_proxy()
            .timeout(PREPARE_SLEEP_TIMEOUT)
            .build()?;
        Ok(Self { http })
    }
}

#[async_trait::async_trait]
impl Sys for Host {
    async fn cmd(&self, argv: &[&str], log: Option<&Path>) -> Result<Out> {
        let mut c = tokio::process::Command::new(argv[0]);
        c.args(&argv[1..]).stdin(Stdio::null()).kill_on_drop(true);
        let run = async {
            if let Some(log) = log {
                let f = std::fs::File::options()
                    .create(true)
                    .append(true)
                    .open(log)?;
                let status = c.stdout(f.try_clone()?).stderr(f).status().await?;
                return Ok(Out {
                    ok: status.success(),
                    stdout: String::new(),
                    stderr: format!("see {}", log.display()),
                });
            }
            let o = c.output().await?;
            Ok::<_, std::io::Error>(Out {
                ok: o.status.success(),
                stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
            })
        };
        tokio::time::timeout(CMD_TIMEOUT, run)
            .await
            .map_err(|_| anyhow!("{} timed out", argv[0]))?
            .with_context(|| format!("run {}", argv[0]))
    }

    async fn prepare_sleep(&self, ip: Ipv4Addr, token: &str) -> Result<(u16, String)> {
        let r = self
            .http
            .post(format!("http://{ip}:{AGENTD_PORT}/box/prepare-sleep"))
            .bearer_auth(token)
            .send()
            .await?;
        Ok((r.status().as_u16(), r.text().await?))
    }
}

#[cfg(test)]
pub mod fake {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::sync::Mutex;

    /// A host that remembers what a real one would: which images are attached, what is mounted,
    /// which sandboxes run. Every command is logged as one line.
    pub struct Fake {
        pub log: Mutex<Vec<String>>,
        /// What agentd answers to prepare-sleep.
        pub agentd: Mutex<(u16, String)>,
        /// `runsc delete` removes nothing while this is set.
        pub stuck: std::sync::atomic::AtomicBool,
        loops: Mutex<HashMap<String, String>>,
        mounted: Mutex<HashSet<String>>,
        running: Mutex<HashSet<String>>,
    }

    impl Default for Fake {
        fn default() -> Self {
            Self {
                log: Mutex::default(),
                agentd: Mutex::new((200, r#"{"ok":true,"active_user_pids":0}"#.into())),
                stuck: Default::default(),
                loops: Mutex::default(),
                mounted: Mutex::default(),
                running: Mutex::default(),
            }
        }
    }

    impl Fake {
        /// The logged commands that contain `needle`.
        pub fn ran(&self, needle: &str) -> usize {
            let log = self.log.lock().unwrap();
            log.iter().filter(|l| l.contains(needle)).count()
        }

        pub fn answer(&self, status: u16, body: &str) {
            *self.agentd.lock().unwrap() = (status, body.to_owned());
        }
    }

    #[async_trait::async_trait]
    impl Sys for Fake {
        async fn cmd(&self, argv: &[&str], _log: Option<&Path>) -> Result<Out> {
            self.log.lock().unwrap().push(argv.join(" "));
            let last = argv[argv.len() - 1].to_owned();
            let has = |flag: &str| argv.contains(&flag);
            let (mut ok, mut stdout) = (true, String::new());
            match argv[0] {
                "losetup" if has("-j") => {
                    if let Some(dev) = self.loops.lock().unwrap().get(&last) {
                        stdout = format!("{dev}: [2049]:12 ({last})\n");
                    }
                }
                "losetup" if has("--find") => {
                    let mut loops = self.loops.lock().unwrap();
                    stdout = format!("/dev/loop{}\n", loops.len());
                    loops.insert(last, stdout.trim().to_owned());
                }
                "losetup" if has("-d") => self.loops.lock().unwrap().retain(|_, d| *d != last),
                "mountpoint" => ok = self.mounted.lock().unwrap().contains(&last),
                "mount" => drop(self.mounted.lock().unwrap().insert(last)),
                "umount" => drop(self.mounted.lock().unwrap().remove(&last)),
                "runsc" => {
                    let mut running = self.running.lock().unwrap();
                    if has("state") {
                        ok = running.contains(&last);
                        stdout = r#"{"status":"running"}"#.into();
                    } else if has("list") {
                        stdout = running.iter().cloned().collect::<Vec<_>>().join("\n");
                    } else if has("--detach") {
                        running.insert(last);
                    } else if has("checkpoint") {
                        let image = argv.iter().find_map(|a| a.strip_prefix("--image-path="));
                        std::fs::create_dir_all(image.unwrap())?;
                        running.remove(&last);
                    } else if has("delete")
                        && !self.stuck.load(std::sync::atomic::Ordering::Relaxed)
                    {
                        running.remove(&last);
                    }
                }
                _ => {}
            }
            Ok(Out {
                ok,
                stdout,
                stderr: String::new(),
            })
        }

        async fn prepare_sleep(&self, _ip: Ipv4Addr, _token: &str) -> Result<(u16, String)> {
            Ok(self.agentd.lock().unwrap().clone())
        }
    }
}
