//! A JSONL RPC child (pi or prime-agent `--mode rpc`): spawn, send commands,
//! correlate responses by id, fan out session events.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{broadcast, oneshot, Mutex};

use arbos_core::rpc::{self, Command as RpcCommand, Event, Record, Response};

pub struct RpcChild {
    pub name: String,
    child: Mutex<Option<Child>>,
    stdin: Mutex<Option<ChildStdin>>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<Response>>>>,
    events: broadcast::Sender<Event>,
    seq: AtomicU64,
    pub exited: Arc<std::sync::atomic::AtomicBool>,
}

impl RpcChild {
    /// Spawn `program args…` with `envs` in `cwd`; stderr goes to `log_path`.
    pub async fn spawn(
        name: &str,
        program: &str,
        args: &[String],
        envs: &[(String, String)],
        cwd: &std::path::Path,
        log_path: &std::path::Path,
        run_as: Option<&crate::isolation::RunAs>,
    ) -> Result<Arc<Self>> {
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
            .with_context(|| format!("open {}", log_path.display()))?;
        if let Some(ra) = run_as {
            let _ = std::os::unix::fs::chown(log_path, Some(ra.uid), Some(ra.gid));
        }
        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(log))
            .kill_on_drop(true);
        for (k, v) in envs {
            cmd.env(k, v);
        }
        if let Some(ra) = run_as {
            // The workspace's own user: no sudo, no other workspace's files (D70 hardening).
            cmd.uid(ra.uid).gid(ra.gid);
            for (k, v) in crate::isolation::Isolation::env(ra) {
                cmd.env(k, v);
            }
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawn {program} ({name})"))?;
        if let (Some(ra), Some(pid)) = (run_as, child.id()) {
            crate::isolation::Isolation::attach(ra, pid);
        }
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        let (events, _) = broadcast::channel(4096);
        let me = Arc::new(Self {
            name: name.to_owned(),
            child: Mutex::new(Some(child)),
            stdin: Mutex::new(Some(stdin)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            events,
            seq: AtomicU64::new(1),
            exited: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        let reader_me = me.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => match rpc::parse_line(&line) {
                        Some(Record::Response(r)) => {
                            if let Some(id) = &r.id {
                                if let Some(tx) = reader_me.pending.lock().await.remove(id) {
                                    let _ = tx.send(r);
                                    continue;
                                }
                            }
                            tracing::debug!(child = %reader_me.name, cmd = %r.command, success = r.success, "uncorrelated response");
                        }
                        Some(Record::Event(e)) => {
                            let _ = reader_me.events.send(e);
                        }
                        Some(Record::Raw(v)) => {
                            tracing::trace!(child = %reader_me.name, raw = %v, "raw record");
                        }
                        None => {}
                    },
                    Ok(None) => break,
                    Err(e) => {
                        tracing::warn!(child = %reader_me.name, error = %e, "stdout read error");
                        break;
                    }
                }
            }
            reader_me.exited.store(true, Ordering::SeqCst);
            // Whoever is waiting on this child hears now, not at their 60 s timeout: a
            // `switch_session` sent to a child that a restart replaced (#2007).
            reader_me.pending.lock().await.clear();
            let _ = reader_me.events.send(Event::Other);
            tracing::info!(child = %reader_me.name, "rpc child stdout closed");
        });
        Ok(me)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    pub fn next_id(&self, prefix: &str) -> String {
        format!("{prefix}-{}", self.seq.fetch_add(1, Ordering::Relaxed))
    }

    /// Send a command and wait for its response (default 60 s).
    pub async fn call(&self, cmd: RpcCommand) -> Result<Response> {
        self.call_timeout(cmd, Duration::from_secs(60)).await
    }

    pub async fn call_timeout(&self, cmd: RpcCommand, timeout: Duration) -> Result<Response> {
        let id = cmd.id().to_owned();
        let id = id.as_str();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id.to_owned(), tx);
        let line = rpc::encode_line(&cmd);
        {
            let mut g = self.stdin.lock().await;
            let stdin = g
                .as_mut()
                .ok_or_else(|| anyhow!("child {} stdin closed", self.name))?;
            stdin.write_all(line.as_bytes()).await?;
            stdin.flush().await?;
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(r)) => Ok(r),
            Ok(Err(_)) if !self.is_alive() => {
                Err(anyhow!("child {} exited before answering {id}", self.name))
            }
            Ok(Err(_)) => Err(anyhow!("child {} dropped response {id}", self.name)),
            Err(_) => {
                self.pending.lock().await.remove(id);
                Err(anyhow!("child {} timed out on {id}", self.name))
            }
        }
    }

    /// Call and fail on `success: false`.
    pub async fn call_ok(&self, cmd: RpcCommand) -> Result<serde_json::Value> {
        let r = self.call(cmd).await?;
        if !r.success {
            return Err(anyhow!("{}: {}", r.command, r.error.unwrap_or_default()));
        }
        Ok(r.data.unwrap_or(serde_json::Value::Null))
    }

    /// Orderly shutdown: close stdin, wait up to 5 s, then kill.
    pub async fn shutdown(&self) {
        self.stdin.lock().await.take();
        if let Some(mut child) = self.child.lock().await.take() {
            if tokio::time::timeout(Duration::from_secs(5), child.wait())
                .await
                .is_err()
            {
                let _ = child.kill().await;
            }
        }
        self.exited.store(true, Ordering::SeqCst);
    }

    pub fn is_alive(&self) -> bool {
        !self.exited.load(Ordering::SeqCst)
    }
}
