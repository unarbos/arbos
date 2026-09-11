use anyhow::{Result, anyhow};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use std::{
    collections::HashMap,
    io::{Read, Write},
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc;

use crate::attach::Frame;

pub struct PtyHub {
    inner: Arc<Mutex<HashMap<String, PtyPage>>>,
    cwd: Mutex<Option<std::path::PathBuf>>,
    out: Mutex<Option<mpsc::UnboundedSender<Frame>>>,
    seq: Mutex<u32>,
}

struct PtyPage {
    writer: Mutex<Box<dyn Write + Send>>,
    pid: u32,
}

impl PtyHub {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            cwd: Mutex::new(None),
            out: Mutex::new(None),
            seq: Mutex::new(0),
        }
    }

    pub fn bind(&self, cwd: std::path::PathBuf, out: mpsc::UnboundedSender<Frame>) {
        *self.cwd.lock().unwrap() = Some(cwd);
        *self.out.lock().unwrap() = Some(out);
    }

    /// Next public id (`t1`, `t2`, …). The desktop attaches with this page
    /// and agent `root`.
    pub fn next_id(&self) -> String {
        let mut n = self.seq.lock().unwrap();
        *n += 1;
        format!("t{n}")
    }

    /// Start a shell the Mac pane can attach to (`PtyIn` agent `root`).
    /// `owner` is the agent that asked; it gets the close when the shell ends.
    pub fn spawn_shell(&self, page: &str, cwd: &std::path::Path, owner: &str) -> Result<u32> {
        let frames = self
            .out
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow!("pty hub not bound"))?;
        self.open("root", page, cwd, frames, Some(owner))
    }

    pub fn open(
        &self,
        agent: &str,
        page: &str,
        cwd: &std::path::Path,
        frames: mpsc::UnboundedSender<Frame>,
        owner: Option<&str>,
    ) -> Result<u32> {
        let pair = NativePtySystem::default().openpty(PtySize {
            rows: 32,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        // The user's own shell, interactive and login, so a prompt is drawn
        // at once and their rc files apply; a bare `sh` shows nothing until
        // the first keystroke.
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "sh".to_string());
        let mut cmd = CommandBuilder::new(&shell);
        cmd.arg("-il");
        cmd.env("TERM", "xterm-256color");
        cmd.cwd(cwd);
        let child = pair.slave.spawn_command(cmd)?;
        let pid = child.process_id().unwrap_or(0);
        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let key = format!("{agent}:{page}");
        self.inner.lock().unwrap().insert(
            key.clone(),
            PtyPage {
                writer: Mutex::new(writer),
                pid,
            },
        );
        let agent = agent.to_string();
        let page = page.to_string();
        let owner = owner.map(str::to_string);
        let pages = Arc::clone(&self.inner);
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let data =
                    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &buf[..n]);
                if frames
                    .send(Frame::Pty {
                        agent: agent.clone(),
                        page: page.clone(),
                        data,
                    })
                    .is_err()
                {
                    break;
                }
            }
            // EOF: the shell is gone. Forget the page so a later write
            // starts a fresh one, and tell the desktop to drop the row.
            pages.lock().unwrap().remove(&key);
            if let Some(owner) = owner {
                let _ = frames.send(Frame::Board {
                    owner,
                    action: "close".into(),
                    panel: "terminal".into(),
                    terminal_ids: vec![page],
                    cwd: None,
                    title: None,
                    url: None,
                });
            }
        });
        Ok(pid)
    }

    pub fn write(&self, agent: &str, page: &str, bytes: &[u8]) -> Result<()> {
        if self.pid(agent, page).is_none() {
            if let (Some(cwd), Some(out)) = (
                self.cwd.lock().unwrap().clone(),
                self.out.lock().unwrap().clone(),
            ) {
                let _ = self.open(agent, page, &cwd, out, None);
            }
        }
        let key = format!("{agent}:{page}");
        let guard = self.inner.lock().unwrap();
        if let Some(p) = guard.get(&key) {
            p.writer.lock().unwrap().write_all(bytes)?;
        }
        Ok(())
    }

    pub fn pid(&self, agent: &str, page: &str) -> Option<u32> {
        self.inner
            .lock()
            .unwrap()
            .get(&format!("{agent}:{page}"))
            .map(|p| p.pid)
    }
}

impl Default for PtyHub {
    fn default() -> Self {
        Self::new()
    }
}
