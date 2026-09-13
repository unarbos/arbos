//! `screenshot`: a picture of the machine's screen, for the model and the
//! window. Not a browser page — `browser screenshot` does that.
//!
//! The file lands in the agent's `images/` like a browser shot, so the
//! transcript cite survives and the desktop can open it. The model gets it
//! as pixels through `ToolOut::images`.

use anyhow::{Context, Result, bail};
use arbos_core::Layout;
use arbos_engine::{Access, BoxFuture, Plan, PlanCx, RunCx, Tool, ToolOut, typed_schema};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Longest a capture may take. macOS can sit on a permission dialog.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(15);

pub struct Screenshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Screen,
    Window,
}

impl Target {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "screen" | "display" | "full" => Some(Self::Screen),
            "window" | "active" | "frontmost" => Some(Self::Window),
            _ => None,
        }
    }
}

impl Tool for Screenshot {
    fn name(&self) -> &'static str {
        "screenshot"
    }
    fn schema(&self) -> Value {
        typed_schema(
            "screenshot",
            "Capture the machine's screen (or the frontmost window) as a PNG. The image is attached below for you to look at and saved under this agent's images/ for the user. For a web page use browser screenshot instead.",
            &[
                (
                    "target",
                    "screen (default) or window (the frontmost / focused window).",
                    false,
                    "string",
                ),
                (
                    "display",
                    "Display index, 1-based, when there are several (macOS). Default: the main one.",
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
        Box::pin(async move {
            let raw = args
                .get("target")
                .and_then(Value::as_str)
                .unwrap_or("screen");
            let target = Target::parse(raw).ok_or_else(|| {
                anyhow::anyhow!("screenshot: target must be screen or window, not {raw:?}")
            })?;
            let display = args.get("display").and_then(Value::as_u64);
            let dir = Layout::new(&cx.place, cx.agent.id.as_str()).images();
            let out = tokio::task::spawn_blocking(move || capture(&dir, target, display))
                .await
                .map_err(|e| anyhow::anyhow!("screenshot task: {e}"))??;
            let shown = out.path.display().to_string();
            let dims = arbos_engine::image::dimensions(&out.png)
                .map(|(w, h)| format!(", {w}x{h}"))
                .unwrap_or_default();
            Ok(ToolOut {
                body: format!(
                    "screenshot {shown} (image/png{dims}, {} KB, via {}) — attached below",
                    out.png.len().div_ceil(1024),
                    out.backend
                ),
                paths: vec![shown.clone()],
                child: None,
                images: vec![shown],
                diff: None,
            })
        })
    }
}

struct Captured {
    path: PathBuf,
    png: Vec<u8>,
    backend: &'static str,
}

/// A fresh file name under `dir`. Two calls in one step can share a
/// millisecond; the suffix keeps them apart.
fn fresh_path(dir: &Path) -> PathBuf {
    let ms = arbos_core::now_ms();
    let mut path = dir.join(format!("screen-{ms}.png"));
    let mut n = 1;
    while path.exists() {
        n += 1;
        path = dir.join(format!("screen-{ms}-{n}.png"));
    }
    path
}

fn capture(dir: &Path, target: Target, display: Option<u64>) -> Result<Captured> {
    std::fs::create_dir_all(dir)?;
    let path = fresh_path(dir);
    let backends = backends(target, display, &path);
    let mut tried: Vec<&'static str> = Vec::new();
    for (name, mut cmd) in backends {
        if which(cmd.get_program()).is_none() {
            continue;
        }
        tried.push(name);
        match run_capped(&mut cmd) {
            Ok(()) => {
                let png = std::fs::read(&path)
                    .with_context(|| format!("{name} wrote nothing to {}", path.display()))?;
                if png.is_empty() {
                    let _ = std::fs::remove_file(&path);
                    bail!("{name} wrote an empty file; is a display available?");
                }
                return Ok(Captured {
                    path,
                    png,
                    backend: name,
                });
            }
            Err(e) => {
                let _ = std::fs::remove_file(&path);
                bail!("{name} failed: {e:#}{}", display_hint());
            }
        }
    }
    if tried.is_empty() {
        bail!(
            "no screenshot backend on this machine: install one of {} {}",
            if cfg!(target_os = "macos") {
                "screencapture (ships with macOS)"
            } else {
                "grim (Wayland), imagemagick (`import`, X11), scrot, gnome-screenshot"
            },
            display_hint()
        );
    }
    bail!("screenshot: no backend succeeded")
}

/// The commands to try, in order, for this platform and target.
fn backends(target: Target, display: Option<u64>, path: &Path) -> Vec<(&'static str, Command)> {
    let out = path.to_string_lossy().into_owned();
    let mut list = Vec::new();
    if cfg!(target_os = "macos") {
        let mut c = Command::new("screencapture");
        c.arg("-x"); // no sound
        if let Some(d) = display {
            c.arg("-D").arg(d.to_string());
        }
        if target == Target::Window {
            // The frontmost window, by its CGWindow id, without the shadow.
            if let Some(id) = frontmost_window_id() {
                c.arg("-o").arg("-l").arg(id.to_string());
            }
        }
        c.arg(&out);
        list.push(("screencapture", c));
        return list;
    }
    // Wayland first when the session says so; X11 tools otherwise.
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        let mut c = Command::new("grim");
        c.arg(&out);
        list.push(("grim", c));
    }
    {
        let mut c = Command::new("import");
        c.arg("-window");
        match target {
            Target::Screen => c.arg("root"),
            Target::Window => c.arg(focused_x11_window().unwrap_or_else(|| "root".into())),
        };
        c.arg(&out);
        list.push(("import", c));
    }
    {
        let mut c = Command::new("scrot");
        if target == Target::Window {
            c.arg("--focused");
        }
        c.arg(&out);
        list.push(("scrot", c));
    }
    {
        let mut c = Command::new("gnome-screenshot");
        if target == Target::Window {
            c.arg("--window");
        }
        c.arg("-f").arg(&out);
        list.push(("gnome-screenshot", c));
    }
    list
}

/// Run with stdout/stderr captured and a time cap; the cap kills it.
fn run_capped(cmd: &mut Command) -> Result<()> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("start {}", cmd.get_program().to_string_lossy()))?;
    let started = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            if status.success() {
                return Ok(());
            }
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                use std::io::Read;
                let _ = e.read_to_string(&mut err);
            }
            bail!("{status}: {}", err.trim().lines().last().unwrap_or(""));
        }
        if started.elapsed() > CAPTURE_TIMEOUT {
            let _ = child.kill();
            bail!("took longer than {CAPTURE_TIMEOUT:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn display_hint() -> String {
    if cfg!(target_os = "macos") {
        " (macOS: allow Screen Recording for the app in System Settings › Privacy)".into()
    } else {
        match (
            std::env::var_os("DISPLAY"),
            std::env::var_os("WAYLAND_DISPLAY"),
        ) {
            (None, None) => " (no DISPLAY or WAYLAND_DISPLAY in this kernel's environment: no screen to capture)".into(),
            _ => String::new(),
        }
    }
}

fn which(program: &std::ffi::OsStr) -> Option<PathBuf> {
    let p = Path::new(program);
    if p.is_absolute() {
        return p.is_file().then(|| p.to_path_buf());
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(program))
            .find(|f| f.is_file())
    })
}

/// X11: the window that has focus, as the hex id `import -window` takes.
fn focused_x11_window() -> Option<String> {
    let out = Command::new("xdotool")
        .arg("getactivewindow")
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!id.is_empty()).then_some(id)
}

/// macOS: the frontmost window's CGWindow id, via a one-line Swift-less
/// route: `osascript` cannot give it, so ask `screencapture` itself to
/// pick interactively is not an option either. `GetWindowID`-style tools
/// are not standard; return None and fall back to the full screen.
fn frontmost_window_id() -> Option<u64> {
    None
}
