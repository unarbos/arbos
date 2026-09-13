//! `record`: a screen recording for the user — start, do the work, stop.
//!
//! The video lands in the agent's `recordings/`; the last frame is saved
//! beside it as a PNG and attached, so the model and the transcript show
//! what was recorded without anyone opening the file. The encoder runs as a
//! child of the kernel with its own time cap (`max_secs`), so a recording
//! nobody stops ends on its own.

use anyhow::{Context, Result, bail};
use arbos_core::Layout;
use arbos_engine::{Access, BoxFuture, Plan, PlanCx, RunCx, Tool, ToolOut, typed_schema};

use crate::screenshot::{display_hint, which};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const DEFAULT_MAX_SECS: u64 = 120;
const CAP_MAX_SECS: u64 = 600;
/// How long `stop` waits for the encoder to write the file's index.
const FINALIZE_TIMEOUT: Duration = Duration::from_secs(15);
const FPS: u32 = 15;

#[derive(Default)]
pub struct Record {
    live: Arc<Mutex<HashMap<String, Recording>>>,
}

struct Recording {
    child: Child,
    path: PathBuf,
    log: PathBuf,
    backend: &'static str,
    started: Instant,
    max_secs: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Start,
    Stop,
    Status,
}

impl Op {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "start" | "begin" => Some(Self::Start),
            "stop" | "end" | "finish" => Some(Self::Stop),
            "status" => Some(Self::Status),
            _ => None,
        }
    }
}

impl Tool for Record {
    fn name(&self) -> &'static str {
        "record"
    }
    fn schema(&self) -> Value {
        typed_schema(
            "record",
            "Screen recording for the user: start (returns at once), do the steps, stop (video path + last frame). status. Ends by itself at max_secs.",
            &[
                ("op", "start|stop|status", true, "string"),
                (
                    "max_secs",
                    "start: cap in seconds (120, max 600).",
                    false,
                    "integer",
                ),
                (
                    "display",
                    "start: 1-based display index (macOS).",
                    false,
                    "integer",
                ),
            ],
        )
    }
    fn plan(&self, _cx: &PlanCx, _args: &Value) -> Result<Plan> {
        Ok(Plan::access(Access::none()))
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        let live = Arc::clone(&self.live);
        Box::pin(async move {
            let raw = args.get("op").and_then(Value::as_str).unwrap_or("");
            let op = Op::parse(raw).ok_or_else(|| {
                anyhow::anyhow!("record: op must be start, stop, or status, not {raw:?}")
            })?;
            let agent = cx.agent.id.to_string();
            let dir = Layout::new(&cx.place, cx.agent.id.as_str())
                .dir
                .join("recordings");
            let max_secs = match args.get("max_secs") {
                None | Some(Value::Null) => DEFAULT_MAX_SECS,
                Some(v) => match v.as_u64().or_else(|| v.as_f64().map(|f| f.max(0.0) as u64)) {
                    Some(n) if (1..=CAP_MAX_SECS).contains(&n) => n,
                    _ => bail!(
                        "record: max_secs must be a whole number from 1 to {CAP_MAX_SECS}, not {v}"
                    ),
                },
            };
            let display = args.get("display").and_then(Value::as_u64);
            tokio::task::spawn_blocking(move || match op {
                Op::Start => start(&live, &agent, &dir, max_secs, display),
                Op::Stop => stop(&live, &agent),
                Op::Status => status(&live, &agent),
            })
            .await
            .map_err(|e| anyhow::anyhow!("record task: {e}"))?
        })
    }
}

fn start(
    live: &Mutex<HashMap<String, Recording>>,
    agent: &str,
    dir: &Path,
    max_secs: u64,
    display: Option<u64>,
) -> Result<ToolOut> {
    let mut map = live.lock().unwrap();
    if let Some(rec) = map.get_mut(agent) {
        if rec.child.try_wait()?.is_none() {
            bail!(
                "record: a recording is already running ({}s so far, cap {}s, {}); stop it first",
                rec.started.elapsed().as_secs(),
                rec.max_secs,
                rec.path.display()
            );
        }
        // Ended on its own (hit its cap) and nobody stopped it: finish it
        // now so the file is reported, then start the new one.
        let old = map.remove(agent).expect("just seen");
        let _ = finish(old);
    }
    std::fs::create_dir_all(dir)?;
    let ms = arbos_core::now_ms();
    let stem = format!("record-{ms}");
    let log = dir.join(format!("{stem}.log"));
    let mut tried = Vec::new();
    for (name, cmd, path) in backends(dir, &stem, max_secs, display) {
        if which(cmd.get_program()).is_none() {
            continue;
        }
        tried.push(name);
        let logf = std::fs::File::create(&log)?;
        let child = leashed(&cmd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(logf)
            .spawn()
            .with_context(|| format!("start {name}"))?;
        let mut rec = Recording {
            child,
            path,
            log: log.clone(),
            backend: name,
            started: Instant::now(),
            max_secs,
        };
        // Give it a moment: a wrong display or size fails at once.
        std::thread::sleep(Duration::from_millis(400));
        if let Some(status) = rec.child.try_wait()? {
            let tail = log_tail(&rec.log);
            let _ = std::fs::remove_file(&rec.path);
            bail!("{name} exited at once ({status}): {tail}{}", display_hint());
        }
        let shown = rec.path.display().to_string();
        map.insert(agent.to_string(), rec);
        return Ok(ToolOut::text(format!(
            "recording started via {name}: {shown}. It stops by itself after {max_secs}s; call record stop when the flow you want shown is done."
        )));
    }
    if tried.is_empty() {
        bail!(
            "no screen recorder on this machine: install {}{}",
            if cfg!(target_os = "macos") {
                "screencapture (ships with macOS)"
            } else {
                "ffmpeg (X11, `x11grab`) or wf-recorder (Wayland)"
            },
            display_hint()
        );
    }
    bail!("record: no backend started")
}

fn stop(live: &Mutex<HashMap<String, Recording>>, agent: &str) -> Result<ToolOut> {
    let rec = live.lock().unwrap().remove(agent);
    let Some(rec) = rec else {
        bail!("record: nothing is recording for this agent; call record start first");
    };
    finish(rec)
}

fn status(live: &Mutex<HashMap<String, Recording>>, agent: &str) -> Result<ToolOut> {
    let mut map = live.lock().unwrap();
    let Some(rec) = map.get_mut(agent) else {
        return Ok(ToolOut::text("no recording running for this agent"));
    };
    let elapsed = rec.started.elapsed().as_secs();
    match rec.child.try_wait()? {
        None => Ok(ToolOut::text(format!(
            "recording: {}s so far (cap {}s), via {}, {}",
            elapsed,
            rec.max_secs,
            rec.backend,
            rec.path.display()
        ))),
        Some(_) => {
            let rec = map.remove(agent).expect("just seen");
            let mut out = finish(rec)?;
            out.body = format!(
                "recording had already ended (hit its cap or the encoder quit). {}",
                out.body
            );
            Ok(out)
        }
    }
}

/// Ask the encoder to end, wait for the file to be finalized, and report
/// it with a poster frame.
fn finish(mut rec: Recording) -> Result<ToolOut> {
    let elapsed = rec.started.elapsed();
    if rec.child.try_wait()?.is_none() {
        // ffmpeg, screencapture -v, and wf-recorder all finalize the file
        // on SIGINT. The leash forwards it to the encoder.
        interrupt(&rec.child);
        let deadline = Instant::now() + FINALIZE_TIMEOUT;
        loop {
            if rec.child.try_wait()?.is_some() {
                break;
            }
            if Instant::now() > deadline {
                let _ = rec.child.kill();
                let _ = rec.child.wait();
                bail!(
                    "record: {} did not finish the file within {FINALIZE_TIMEOUT:?}; killed. Partial file: {}. Log tail: {}",
                    rec.backend,
                    rec.path.display(),
                    log_tail(&rec.log)
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let size = std::fs::metadata(&rec.path).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        let _ = std::fs::remove_file(&rec.path);
        bail!(
            "record: {} wrote an empty file after {:.1}s. Log tail: {}",
            rec.backend,
            elapsed.as_secs_f64(),
            log_tail(&rec.log)
        );
    }
    let _ = std::fs::remove_file(&rec.log);
    let secs = probe_duration(&rec.path).unwrap_or(elapsed.as_secs_f64());
    let shown = rec.path.display().to_string();
    let poster = poster_frame(&rec.path);
    let mut paths = vec![shown.clone()];
    let mut images = Vec::new();
    let mut body = format!(
        "recording {shown} ({:.1}s, {} KB, via {})",
        secs,
        size.div_ceil(1024),
        rec.backend
    );
    match poster {
        Some(p) => {
            let p = p.display().to_string();
            body.push_str(&format!("; last frame {p} attached below"));
            paths.push(p.clone());
            images.push(p);
        }
        None => body.push_str("; no poster frame (ffmpeg missing or clip too short)"),
    }
    Ok(ToolOut {
        body,
        paths,
        child: None,
        images,
        diff: None,
        park: None,
    })
}

/// The encoder wrapped in a shell that ends it when the kernel is gone.
/// A recording outliving the kernel would run to its cap with nobody to
/// report it. `$PPID` is the kernel; SIGINT/SIGTERM to the wrapper are
/// forwarded so `stop` still finalizes the file.
fn leashed(cmd: &Command) -> Command {
    const LEASH: &str = r#"K=$PPID
"$@" & F=$!
trap 'kill -INT "$F" 2>/dev/null' INT TERM
while kill -0 "$K" 2>/dev/null && kill -0 "$F" 2>/dev/null; do sleep 0.5; done
kill -INT "$F" 2>/dev/null
wait "$F""#;
    let mut sh = Command::new("sh");
    sh.arg("-c").arg(LEASH).arg("record-leash");
    sh.arg(cmd.get_program());
    sh.args(cmd.get_args());
    sh
}

/// The recorders to try, in order: `(name, command, output path)`.
fn backends(
    dir: &Path,
    stem: &str,
    max_secs: u64,
    display: Option<u64>,
) -> Vec<(&'static str, Command, PathBuf)> {
    let mut list = Vec::new();
    if cfg!(target_os = "macos") {
        let path = dir.join(format!("{stem}.mov"));
        let mut c = Command::new("screencapture");
        c.arg("-x").arg("-v").arg("-V").arg(max_secs.to_string());
        if let Some(d) = display {
            c.arg("-D").arg(d.to_string());
        }
        c.arg(&path);
        list.push(("screencapture", c, path));
        return list;
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        let path = dir.join(format!("{stem}.mp4"));
        let mut c = Command::new("wf-recorder");
        c.arg("-f").arg(&path);
        list.push(("wf-recorder", c, path));
    }
    if let Some(x11) = std::env::var("DISPLAY").ok().filter(|d| !d.is_empty()) {
        let path = dir.join(format!("{stem}.mp4"));
        let (w, h) = x11_screen_size().unwrap_or((1920, 1080));
        let mut c = Command::new("ffmpeg");
        c.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostats",
            "-nostdin",
            "-y",
        ])
        .args(["-f", "x11grab"])
        .args(["-framerate", &FPS.to_string()])
        .args(["-video_size", &format!("{w}x{h}")])
        .args(["-i", &x11])
        .args(["-t", &max_secs.to_string()])
        .args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "28"])
        .args(["-pix_fmt", "yuv420p"])
        .args(["-movflags", "+faststart"])
        .arg(&path);
        list.push(("ffmpeg x11grab", c, path));
    }
    list
}

/// Root window size, so x11grab covers the whole screen and not the
/// 640x480 it defaults to. Even numbers: yuv420p needs them.
fn x11_screen_size() -> Option<(u32, u32)> {
    let text = Command::new("xdpyinfo")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .or_else(|| {
            Command::new("xrandr")
                .arg("--current")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        })?;
    // xdpyinfo: "  dimensions:    1920x1200 pixels (508x317 millimeters)"
    // xrandr:   "Screen 0: minimum 320 x 200, current 1920 x 1200, maximum …"
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("dimensions:") {
            let dims = rest.split_whitespace().next()?;
            return parse_dims(dims, 'x');
        }
        if let Some(i) = line.find("current ") {
            let rest = &line[i + "current ".len()..];
            let end = rest.find(',').unwrap_or(rest.len());
            let dims: String = rest[..end].chars().filter(|c| !c.is_whitespace()).collect();
            return parse_dims(&dims, 'x');
        }
    }
    None
}

fn parse_dims(s: &str, sep: char) -> Option<(u32, u32)> {
    let (w, h) = s.split_once(sep)?;
    let (w, h): (u32, u32) = (w.parse().ok()?, h.parse().ok()?);
    Some((w & !1, h & !1))
}

/// The clip's last frame as a PNG beside it. `None` without ffmpeg or when
/// the clip is too short to seek in.
fn poster_frame(video: &Path) -> Option<PathBuf> {
    which(std::ffi::OsStr::new("ffmpeg"))?;
    let out = video.with_extension("png");
    for seek in [Some("-1"), None] {
        let mut c = Command::new("ffmpeg");
        c.args(["-hide_banner", "-loglevel", "error", "-y"]);
        if let Some(s) = seek {
            c.args(["-sseof", s]);
        }
        c.arg("-i")
            .arg(video)
            .args(["-frames:v", "1", "-update", "1"])
            .arg(&out);
        let ok = c
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok
            && std::fs::metadata(&out)
                .map(|m| m.len() > 0)
                .unwrap_or(false)
        {
            return Some(out);
        }
    }
    let _ = std::fs::remove_file(&out);
    None
}

fn probe_duration(video: &Path) -> Option<f64> {
    which(std::ffi::OsStr::new("ffprobe"))?;
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
        ])
        .arg(video)
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

fn interrupt(child: &Child) {
    // SIGINT to the leash, which forwards it; every backend here treats it
    // as "stop and finalize".
    unsafe {
        libc::kill(child.id() as i32, libc::SIGINT);
    }
}

fn log_tail(log: &Path) -> String {
    std::fs::read_to_string(log)
        .ok()
        .and_then(|t| {
            t.lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "(no output)".into())
}
