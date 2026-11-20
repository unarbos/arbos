//! A JSONL RPC child (pi or prime-agent `--mode rpc`): spawn, send commands,
//! correlate responses by id, fan out session events.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{broadcast, oneshot, Mutex};

use arbos_core::rpc::{self, Command as RpcCommand, Event, Record, Response};

/// The longest line a child's stdout may hand agentd. A record is one line of JSON, and a line
/// was read whole however long it was, so one runaway record was held in memory in full, twice
/// over once parsed. Far above any real record (an image part is a few MiB).
pub(crate) const CHILD_LINE_MAX: usize = 32 * 1024 * 1024;

/// How much of each end of an over-long line is kept to tell what it was.
const OVER_CAP_KEEP: usize = 4096;

/// The next line of a child's stdout, `None` at its end.
///
/// A line over `max` bytes is read past without being held, and what comes back in its place
/// depends on what it was (see [`over_cap_line`]): the record a run ends on is kept as a record,
/// anything else comes back empty, which no reader parses, and the stream goes on.
pub(crate) async fn read_capped_line<R: AsyncBufRead + Unpin>(
    stdout: &mut R,
    max: usize,
) -> std::io::Result<Option<String>> {
    let mut buf = Vec::new();
    let mut part = (&mut *stdout).take(max as u64 + 1);
    if part.read_until(b'\n', &mut buf).await? == 0 {
        return Ok(None);
    }
    if buf.len() > max && buf.last() != Some(&b'\n') {
        let head = buf[..OVER_CAP_KEEP.min(buf.len())].to_vec();
        let mut tail = buf.split_off(buf.len().saturating_sub(OVER_CAP_KEEP));
        drop(buf);
        let mut chunk = Vec::new();
        loop {
            chunk.clear();
            let mut part = (&mut *stdout).take(64 * 1024);
            let read = part.read_until(b'\n', &mut chunk).await?;
            tail.extend_from_slice(&chunk);
            let excess = tail.len().saturating_sub(OVER_CAP_KEEP);
            tail.drain(..excess);
            if read == 0 || chunk.last() == Some(&b'\n') {
                return Ok(Some(over_cap_line(&head, &tail, max)));
            }
        }
    }
    while matches!(buf.last(), Some(b'\n' | b'\r')) {
        buf.pop();
    }
    Ok(Some(String::from_utf8(buf).unwrap_or_else(|e| {
        String::from_utf8_lossy(e.as_bytes()).into_owned()
    })))
}

/// What stands in for a line that was over the cap, from its first and last few KiB.
///
/// `agent_end` is the record a Prime worker's run ends on, and it carries the run's whole
/// message list, so it is the record most likely to be over the cap (a run with forty
/// screenshots is). agentd reads one thing from it, `willRetry`, and nothing from `messages`. So
/// it is handed on with that flag and an empty list: the run settles as it would have. Reading
/// it as a lost child instead would give the finished job's brief to a fresh worker, which would
/// do the work again and end on the same line.
///
/// Any other record is dropped: none of them ends a run, and the stream carries on after it.
///
/// The type and the flag are looked for as the bytes a top-level key is written with. Inside a
/// message they would be inside a JSON string, with their quotes escaped, and would not match.
fn over_cap_line(head: &[u8], tail: &[u8], max: usize) -> String {
    let has = |needle: &[u8]| {
        [head, tail]
            .iter()
            .any(|end| end.windows(needle.len()).any(|w| w == needle))
    };
    if has(br#""type":"agent_end""#) {
        let will_retry = has(br#""willRetry":true"#);
        tracing::warn!(
            max,
            will_retry,
            "an agent_end over the line cap was settled without its messages"
        );
        return format!(r#"{{"type":"agent_end","messages":[],"willRetry":{will_retry}}}"#);
    }
    tracing::warn!(max, "a line from a child was over the cap and was dropped");
    String::new()
}

pub struct RpcChild {
    pub name: String,
    child: Mutex<Option<Child>>,
    stdin: Mutex<Option<ChildStdin>>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<Response>>>>,
    events: broadcast::Sender<Event>,
    seq: AtomicU64,
    pub exited: Arc<std::sync::atomic::AtomicBool>,
    /// When this child last carried traffic in either direction, for
    /// [`crate::agent::Agent::retire_idle_workers`].
    ///
    /// Both directions, and that is the whole reason it is here rather than derived from the
    /// store. A job's worker is prompted **once** and then streams for as long as the job runs,
    /// so every store-derived clock reads a busy worker as long idle: `last_seen_at` is written
    /// by `set_subagent_status`, which a running job does not call again. A retention pass that
    /// trusted that would evict workers mid-job.
    ///
    /// Written at spawn, in [`Self::call_timeout`] (the one funnel every command out passes
    /// through), and in the reader loop for every record in — so a child that is only receiving
    /// still reads as busy.
    last_used_ms: AtomicU64,
    /// Kept from spawn: `Child::id` is `None` once the process has been reaped, and the pid is
    /// what tells a workspace cgroup's leftovers apart from the children the box still owns.
    pid: Option<u32>,
    /// Where this child's descendants can be found after it is gone (bug Z). Killing the pid
    /// above reaches nothing it spawned, so a cancel needs a handle on the set instead.
    tree: crate::isolation::ProcessTree,
}

/// How a child that is gone went: its exit code, or the signal that took it.
///
/// Test5 round 8 (bug Y) spawned about four thousand main pi children in two and a half hours
/// and recorded this for none of them, because nothing ever waited on one. `RpcChild` notices a
/// child through its stdout reaching EOF and then drops the handle, so the status the kernel
/// held was thrown away — including the `SIGKILL` an OOM kill would have left.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ChildExit {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

impl std::fmt::Display for ChildExit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.code, self.signal) {
            (_, Some(s)) => write!(f, "signal {s}"),
            (Some(c), _) => write!(f, "exit {c}"),
            _ => write!(f, "unknown"),
        }
    }
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
        // A session and a process group of the child's own. Both are inherited on fork, so
        // everything the child starts can be signalled as a set without the signal reaching
        // agentd (bug Z) — and the session, unlike the group, is still the child's after a tool
        // runner has put its command in a group of its own to time it out (bug Z, round 13).
        // `setsid` is called here rather than through `process_group`, which would make the
        // child a group leader first and so make `setsid` fail with EPERM.
        unsafe {
            cmd.pre_exec(|| {
                // Fails only when the child already leads a group, which a forked child cannot.
                nix::libc::setsid();
                Ok(())
            });
        }
        for (k, v) in envs {
            cmd.env(k, v);
        }
        // Inherited through fork and exec by everything the child starts, which is what lets a
        // cancel find a process that has left every set the kernel keeps for it.
        cmd.env(crate::isolation::WORKER_MARK_ENV, name);
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
        let pid = child.id();
        // A cgroup of this child's own under the workspace's. Membership is inherited on fork
        // and survives `setsid`, so it is the one handle that finds every descendant; the
        // process group below misses a child that deliberately leaves the session.
        let cgroup = match (run_as, pid) {
            (Some(ra), Some(pid)) => crate::isolation::Isolation::attach_leaf(ra, name, pid),
            _ => None,
        };
        let tree = crate::isolation::ProcessTree {
            // `setsid` makes the child the leader of both, so each id is its pid.
            pgid: pid,
            sid: pid,
            cgroup,
            mark: Some(name.to_owned()),
        };
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
            pid,
            tree,
            last_used_ms: AtomicU64::new(arbos_core::ids::now_ms() as u64),
        });
        let reader_me = me.clone();
        tokio::spawn(async move {
            let mut stdout = BufReader::new(stdout);
            loop {
                match read_capped_line(&mut stdout, CHILD_LINE_MAX).await {
                    Ok(Some(line)) => {
                        // Before the parse, so a line this build cannot read still counts as the
                        // child doing something. An idle clock that only unparseable traffic
                        // could not move is a clock that ages a working child.
                        reader_me.touch();
                        match rpc::parse_line(&line) {
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
                        }
                    }
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
        self.touch();
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

    /// Give the whole EOF cleanup five seconds, then kill the captured process.
    pub async fn shutdown(&self) {
        // Keep the handle outside the timed future: cancelling the grace must not
        // discard the authority needed to kill and reap a backpressured child.
        let mut process = None;
        let graceful = async {
            process = self.child.lock().await.take();
            self.stdin.lock().await.take();
            if let Some(child) = process.as_mut() {
                let _ = child.wait().await;
            }
        };
        if tokio::time::timeout(Duration::from_secs(5), graceful)
            .await
            .is_err()
        {
            // A process-handle owner may have delayed capture. Never wait for the
            // blocked stdin writer before terminating this exact process.
            if process.is_none() {
                process = self.child.lock().await.take();
            }
            if let Some(child) = process.as_mut() {
                // Tokio's kill also waits/reaps, retaining the existing best-effort error policy.
                let _ = child.kill().await;
            }
            self.stdin.lock().await.take();
        }
        self.exited.store(true, Ordering::SeqCst);
    }

    /// Retire only this captured child. If the runtime drops the cleanup task, use its owned
    /// Child handle as a last resort; never reconstruct authority from a pid or process name.
    pub(crate) fn retire_owned(self: Arc<Self>) {
        struct Cleanup(Option<Arc<RpcChild>>);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                if let Some(child) = &self.0 {
                    if let Ok(mut input) = child.stdin.try_lock() {
                        input.take();
                    }
                    if let Ok(mut held) = child.child.try_lock() {
                        if let Some(process) = held.as_mut() {
                            let _ = process.start_kill();
                            let _ = process.try_wait();
                        }
                    }
                }
            }
        }
        let mut cleanup = Cleanup(Some(self));
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if let Some(child) = &cleanup.0 {
                    child.shutdown().await;
                }
                cleanup.0.take();
            });
        }
        // Without a runtime, Cleanup drops here. If a shutdown future held either mutex,
        // its own Child kill_on_drop and eventual RpcChild drop remain the final fallback.
    }

    pub fn is_alive(&self) -> bool {
        !self.exited.load(Ordering::SeqCst)
    }

    /// Pause only this captured process handle while a test observes owned cleanup.
    /// RPC stdin, sibling processes and the session registry remain independent.
    #[cfg(test)]
    pub(crate) async fn test_hold_process_until(
        &self,
        ready: oneshot::Sender<()>,
        release: oneshot::Receiver<()>,
    ) {
        let _process = self.child.lock().await;
        let _ = ready.send(());
        let _ = release.await;
    }

    #[cfg(test)]
    pub(crate) fn test_process_is_captured(&self) -> bool {
        self.child.try_lock().is_ok_and(|process| process.is_none())
    }

    #[cfg(test)]
    pub(crate) fn test_stdin_is_locked(&self) -> bool {
        self.stdin.try_lock().is_err()
    }

    /// This child carried traffic just now.
    fn touch(&self) {
        self.last_used_ms
            .store(arbos_core::ids::now_ms() as u64, Ordering::SeqCst);
    }

    /// How long since any traffic in either direction. Zero while traffic is flowing.
    pub fn idle_ms(&self) -> u64 {
        (arbos_core::ids::now_ms() as u64).saturating_sub(self.last_used_ms.load(Ordering::SeqCst))
    }

    /// Age this child's idle clock by `ms`, for a test that needs a long-idle worker without
    /// waiting out the bound.
    ///
    /// It moves the clock and nothing else: the process stays exactly as it is, so a case using
    /// this still kills a real child through the real path. The alternative — a test-only
    /// override of the bound — would leave the comparison itself unexercised.
    #[cfg(test)]
    pub fn test_age_idle_clock(&self, ms: u64) {
        let now = arbos_core::ids::now_ms() as u64;
        self.last_used_ms
            .store(now.saturating_sub(ms), Ordering::SeqCst);
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Where this child's descendants can be reached, for a cancel that has to reap them.
    pub fn tree(&self) -> crate::isolation::ProcessTree {
        self.tree.clone()
    }

    /// Wait on a child that is gone and report how it went.
    ///
    /// Call this only once [`Self::is_alive`] is false. EOF on stdout can beat the process's own
    /// exit by a moment, so a child that has not quite finished is given a short wait rather
    /// than reported as unknown; the handle stays in place, so a later [`Self::shutdown`] is
    /// still safe. A status that cannot be read at all — a child the runtime already reaped —
    /// reports `unknown` instead of inventing a code.
    pub async fn exit_report(&self) -> ChildExit {
        use std::os::unix::process::ExitStatusExt;
        let mut held = self.child.lock().await;
        let Some(child) = held.as_mut() else {
            return ChildExit::default();
        };
        let status = match child.try_wait() {
            Ok(Some(st)) => Some(st),
            Ok(None) => tokio::time::timeout(Duration::from_secs(2), child.wait())
                .await
                .ok()
                .and_then(Result::ok),
            Err(_) => None,
        };
        match status {
            Some(st) => ChildExit {
                code: st.code(),
                signal: st.signal(),
            },
            None => ChildExit::default(),
        }
    }
}

#[cfg(test)]
mod capped_line_tests {
    use super::*;

    async fn lines(input: &str, max: usize) -> Vec<String> {
        let mut stdout = BufReader::new(input.as_bytes());
        let mut out = Vec::new();
        while let Some(line) = read_capped_line(&mut stdout, max).await.unwrap() {
            out.push(line);
        }
        out
    }

    #[tokio::test]
    async fn a_line_over_the_cap_is_dropped_and_the_stream_goes_on() {
        let input = format!(
            "first\n{{\"type\":\"message_end\",\"x\":\"{}\"}}\n{{\"type\":\"agent_start\"}}\r\nlast",
            "x".repeat(200_000)
        );
        assert_eq!(
            lines(&input, 64).await,
            ["first", "", "{\"type\":\"agent_start\"}", "last"]
        );
    }

    /// The record a run ends on, over the cap, still ends the run: it parses as an `agent_end`
    /// with no messages and the flag it was sent with, whichever end of the line the keys are at.
    #[tokio::test]
    async fn an_agent_end_over_the_cap_is_still_an_agent_end() {
        let big = "x".repeat(200_000);
        // A message that quotes the words themselves must not be taken for the record's own keys.
        let quoted = r#"{\"type\":\"agent_end\",\"willRetry\":true}"#;
        for (line, retry) in [
            (
                format!(r#"{{"type":"agent_end","messages":["{big}"]}}"#),
                false,
            ),
            (
                format!(r#"{{"messages":["{big}"],"willRetry":true,"type":"agent_end"}}"#),
                true,
            ),
            (
                format!(r#"{{"type":"agent_end","messages":["{quoted}{big}"]}}"#),
                false,
            ),
        ] {
            let read = lines(&format!("{line}\nnext\n"), 64).await;
            assert_eq!(read.len(), 2, "{read:?}");
            assert_eq!(read[1], "next");
            match rpc::parse_line(&read[0]) {
                Some(Record::Event(Event::AgentEnd {
                    messages,
                    will_retry,
                })) => {
                    assert!(messages.is_empty());
                    assert_eq!(will_retry, retry, "{}", &line[..60]);
                }
                _ => panic!("not an agent_end: {:?}", read[0]),
            }
        }
        // Another record that merely quotes those words is dropped, not turned into an end.
        let other = format!(r#"{{"type":"message_end","text":"{quoted}{big}"}}"#);
        assert_eq!(lines(&format!("{other}\n"), 64).await, [""]);
    }
}
