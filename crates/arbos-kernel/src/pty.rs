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
    /// The agent the row docks under, and who asked (`user` | `agent`);
    /// empty for a page a client wrote to before any shell was opened.
    owner: String,
    by: String,
    cwd: std::path::PathBuf,
    started_ms: i64,
}

/// One live shell, as `surfaces` reports it.
pub struct PtyRow {
    pub agent: String,
    pub page: String,
    pub pid: u32,
    pub alive: bool,
    pub owner: String,
    pub by: String,
    pub cwd: std::path::PathBuf,
    pub started_ms: i64,
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
    /// `owner` is the agent the row docks under; it gets the close when the
    /// shell ends. `by` is who asked for it — `user` or `agent` — and rides
    /// on that close, so the window can pair it with the open.
    pub fn spawn_shell(
        &self,
        page: &str,
        cwd: &std::path::Path,
        owner: &str,
        by: &str,
    ) -> Result<u32> {
        let frames = self
            .out
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow!("pty hub not bound"))?;
        self.open("root", page, cwd, frames, Some((owner, by)))
    }

    pub fn open(
        &self,
        agent: &str,
        page: &str,
        cwd: &std::path::Path,
        frames: mpsc::UnboundedSender<Frame>,
        owner: Option<(&str, &str)>,
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
                // #461 carries who asked beside the owner; #468 keeps both on
                // the page so `surfaces` can report them. The tuple is
                // (owner, by), and an absent one is a page with no owner
                // recorded rather than a page owned by nobody in particular.
                owner: owner.map(|(o, _)| o.to_string()).unwrap_or_default(),
                by: owner.map(|(_, by)| by.to_string()).unwrap_or_default(),
                cwd: cwd.to_path_buf(),
                started_ms: arbos_core::now_ms(),
            },
        );
        let agent = agent.to_string();
        let page = page.to_string();
        let owner = owner.map(|(o, by)| (o.to_string(), by.to_string()));
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
            if let Some((owner, by)) = owner {
                let _ = frames.send(Frame::Board {
                    owner,
                    action: "close".into(),
                    panel: "terminal".into(),
                    terminal_ids: vec![page],
                    cwd: None,
                    title: None,
                    url: None,
                    by,
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

    /// Every shell this kernel has opened and not yet seen end. `alive` is
    /// the process, checked now: a shell whose reader thread has not yet
    /// noticed the EOF still lists, as gone.
    pub fn list(&self) -> Vec<PtyRow> {
        let guard = self.inner.lock().unwrap();
        let mut rows: Vec<PtyRow> = guard
            .iter()
            .map(|(key, p)| {
                let (agent, page) = key.split_once(':').unwrap_or((key.as_str(), ""));
                PtyRow {
                    agent: agent.to_string(),
                    page: page.to_string(),
                    pid: p.pid,
                    alive: p.pid != 0 && unsafe { libc::kill(p.pid as i32, 0) == 0 },
                    owner: p.owner.clone(),
                    by: p.by.clone(),
                    cwd: p.cwd.clone(),
                    started_ms: p.started_ms,
                }
            })
            .collect();
        rows.sort_by(|a, b| a.started_ms.cmp(&b.started_ms).then(a.page.cmp(&b.page)));
        rows
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
