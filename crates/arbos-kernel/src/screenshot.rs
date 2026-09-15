//! `screenshot`: a picture of the machine's screen, for the model and the
//! window. Not a browser page — `browser screenshot` does that. `target:
//! text` renders words (a command's output) as a terminal-styled image
//! with headless Chrome: what "show me the output" needs on a machine
//! with no screen, and what a worker improvised with ImageMagick and lost
//! to its security policy (kickoff item 3).
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
/// Longest a text render may take: a cold Chrome on a small CI runner
/// needs most of 15 s on its own, and two at once went past it (#219).
const RENDER_TIMEOUT: Duration = Duration::from_secs(60);
/// One Chrome at a time for text renders: they are quick, and two cold
/// starts side by side on a small machine are slower than one after the
/// other.
static RENDER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub struct Screenshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Screen,
    Window,
    /// Words rendered as an image; no display involved.
    Text,
}

impl Target {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "screen" | "display" | "full" => Some(Self::Screen),
            "window" | "active" | "frontmost" => Some(Self::Window),
            "text" | "output" | "render" => Some(Self::Text),
            _ => None,
        }
    }
}

/// Widest rendering of a text image, in CSS pixels.
const TEXT_IMAGE_WIDTH: u32 = 960;
/// Longest text rendered; more is cut with a note in the image.
const TEXT_IMAGE_MAX_LINES: usize = 200;

impl Tool for Screenshot {
    fn name(&self) -> &'static str {
        "screenshot"
    }
    fn schema(&self) -> Value {
        typed_schema(
            "screenshot",
            "Capture the screen or frontmost window, or render text (a command's output) as an image; the image is shown to you and the user. Web pages: browser screenshot.",
            &[
                (
                    "target",
                    "screen (default), window, or text (render `text` as a terminal-styled image; needs no display).",
                    false,
                    "string",
                ),
                (
                    "text",
                    "With target text: the words to render, e.g. a command and its output.",
                    false,
                    "string",
                ),
                (
                    "title",
                    "With target text: a title line (the command, a file name).",
                    false,
                    "string",
                ),
                (
                    "display",
                    "1-based display index (macOS).",
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
            let out = if target == Target::Text {
                let text = args
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::trim_end)
                    .filter(|t| !t.trim().is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "screenshot: target text needs `text` (the words to render)"
                        )
                    })?
                    .to_string();
                let title = args
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                tokio::task::spawn_blocking(move || render_text(&dir, &title, &text))
                    .await
                    .map_err(|e| anyhow::anyhow!("screenshot task: {e}"))??
            } else {
                tokio::task::spawn_blocking(move || capture(&dir, target, display))
                    .await
                    .map_err(|e| anyhow::anyhow!("screenshot task: {e}"))??
            };
            let shown = out.path.display().to_string();
            let dims = arbos_engine::image::dimensions(&out.png)
                .map(|(w, h)| format!(", {w}x{h}"))
                .unwrap_or_default();
            let what = if target == Target::Text {
                "rendered"
            } else {
                "screenshot"
            };
            Ok(ToolOut {
                body: format!(
                    "{what} {shown} (image/png{dims}, {} KB, via {}) — attached below; put this path in your reply",
                    out.png.len().div_ceil(1024),
                    out.backend
                ),
                paths: vec![shown.clone()],
                child: None,
                images: vec![shown],
                diff: None,
                park: None,
            })
        })
    }
}

struct Captured {
    path: PathBuf,
    png: Vec<u8>,
    backend: &'static str,
}

/// One frame of the main screen for Try Live: captured into the place's
/// runtime folder and removed at once (the bytes are the answer). Scaled
/// to 1280 wide and JPEG-encoded when ImageMagick or sips is on the
/// machine — a 4 MB PNG every two seconds is too much for a tunnel; else
/// the PNG as captured. Returns (bytes, mime, width, height) of the
/// original screen.
pub fn grab_screen(place: &arbos_core::Place) -> Result<(Vec<u8>, &'static str, u32, u32)> {
    let dir = place.runtime_dir().join("live");
    let out = capture(&dir, Target::Screen, None)?;
    let (w, h) = arbos_engine::image::dimensions(&out.png).unwrap_or((0, 0));
    let small = out.path.with_extension("jpg");
    let scaled = if which(std::ffi::OsStr::new("convert")).is_some() {
        std::process::Command::new("convert")
            .arg(&out.path)
            .args(["-resize", "1280x>", "-quality", "72"])
            .arg(&small)
            .stdin(std::process::Stdio::null())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    } else if which(std::ffi::OsStr::new("sips")).is_some() {
        std::process::Command::new("sips")
            .args([
                "-Z",
                "1280",
                "-s",
                "format",
                "jpeg",
                "-s",
                "formatOptions",
                "72",
            ])
            .arg(&out.path)
            .arg("--out")
            .arg(&small)
            .stdin(std::process::Stdio::null())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    } else {
        false
    };
    let result = if scaled && let Ok(bytes) = std::fs::read(&small) {
        (bytes, "image/jpeg", w, h)
    } else {
        (out.png, "image/png", w, h)
    };
    let _ = std::fs::remove_file(&out.path);
    let _ = std::fs::remove_file(&small);
    Ok(result)
}

/// A fresh file name under `dir`. Two calls in one step can share a
/// millisecond; the suffix keeps them apart.
fn fresh_path(dir: &Path) -> PathBuf {
    fresh_named(dir, "screen")
}

fn fresh_named(dir: &Path, stem: &str) -> PathBuf {
    let ms = arbos_core::now_ms();
    let mut path = dir.join(format!("{stem}-{ms}.png"));
    let mut n = 1;
    while path.exists() {
        n += 1;
        path = dir.join(format!("{stem}-{ms}-{n}.png"));
    }
    path
}

/// `text` as a terminal-styled PNG under `dir`, drawn by headless Chrome
/// from a one-page HTML file (removed afterwards). Chrome is what the
/// browser tool already needs, so no new dependency; a machine without it
/// gets told what to install and what to do instead.
fn render_text(dir: &Path, title: &str, text: &str) -> Result<Captured> {
    let Some(chrome) = chrome_binary() else {
        bail!(
            "screenshot target text needs chromium or google-chrome on this machine (none found); save the output to a file under .arbos/media/<topic>/ and name it in your reply instead"
        );
    };
    std::fs::create_dir_all(dir)?;
    let path = fresh_named(dir, "text");
    let html_path = path.with_extension("html");
    let (html, lines) = text_page(title, text);
    std::fs::write(&html_path, html)?;
    // 22 px a line at 14 px monospace, a title band, padding; capped so a
    // long log does not make a 40 000-pixel image.
    let height = (lines as u32 * 22 + if title.is_empty() { 40 } else { 76 }).clamp(120, 4600);
    // Its own profile: the browser tool's Chrome holds the default one,
    // and two Chromes on one profile wait on each other's lock. One per
    // kernel process, kept between renders — a fresh profile is the slow
    // part of a cold start, and renders run one at a time (RENDER_LOCK).
    let profile = std::env::temp_dir().join(format!("arbos-textshot-{}", std::process::id()));
    std::fs::create_dir_all(&profile)?;
    sweep_stale_profiles();
    let mut cmd = Command::new(chrome);
    cmd.args([
        "--headless=new",
        "--disable-gpu",
        "--no-sandbox",
        "--disable-dev-shm-usage",
        "--hide-scrollbars",
        "--no-first-run",
        "--no-default-browser-check",
        "--disable-extensions",
        "--disable-background-networking",
        "--disable-component-update",
        "--disable-sync",
        "--disable-crash-reporter",
        "--disable-breakpad",
        "--force-device-scale-factor=1",
        // A local page has nothing to wait for; on a CI runner headless
        // Chrome still sat on the load for a minute before writing the
        // shot. Virtual time runs the page's timers out at once, and
        // --timeout stops the load and takes the shot regardless.
        "--virtual-time-budget=3000",
        "--timeout=8000",
    ])
    .arg(format!("--user-data-dir={}", profile.display()))
    .arg(format!("--window-size={TEXT_IMAGE_WIDTH},{height}"))
    .arg(format!("--screenshot={}", path.display()))
    .arg(format!("file://{}", html_path.display()));
    let result = {
        let _one_at_a_time = RENDER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        run_until_file(&mut cmd, &path, RENDER_TIMEOUT)
    };
    let _ = std::fs::remove_file(&html_path);
    result.map_err(|e| anyhow::anyhow!("chrome could not render the text: {e:#}"))?;
    let png = std::fs::read(&path)
        .with_context(|| format!("chrome wrote nothing to {}", path.display()))?;
    if png.is_empty() {
        let _ = std::fs::remove_file(&path);
        bail!("chrome wrote an empty file");
    }
    Ok(Captured {
        path,
        png,
        backend: "chrome (text)",
    })
}

/// Profiles left by kernels that are gone (no Drop runs for a killed
/// process): removed on the next render, by pid.
fn sweep_stale_profiles() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    let me = std::process::id();
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|n| n.strip_prefix("arbos-textshot-"))
            // `<pid>`; an earlier build wrote `<pid>-<ms>`.
            .and_then(|p| p.split('-').next()?.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == me || pid_alive(pid) {
            continue;
        }
        let _ = std::fs::remove_dir_all(e.path());
    }
}

fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // kill -0: "may I signal it" is "does it exist" for our own user.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Run `cmd` until it exits or `file` is written whole, whichever first.
/// Headless Chrome writes the screenshot and then may sit on its exit;
/// the file is the result, so a written file ends the wait and the
/// process is killed. The time cap kills it too.
fn run_until_file(cmd: &mut Command, file: &Path, cap: Duration) -> Result<()> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("start {}", cmd.get_program().to_string_lossy()))?;
    let started = std::time::Instant::now();
    let mut last_len: Option<u64> = None;
    let mut steady = 0;
    loop {
        if let Some(status) = child.try_wait()? {
            if status.success() || file.is_file() {
                return Ok(());
            }
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                use std::io::Read;
                let _ = e.read_to_string(&mut err);
            }
            bail!("{status}: {}", err.trim().lines().last().unwrap_or(""));
        }
        // Written and no longer growing across a few looks (a slow disk
        // can land a PNG in more than one write): done.
        let len = std::fs::metadata(file)
            .map(|m| m.len())
            .ok()
            .filter(|&n| n > 0);
        if len.is_some() && len == last_len {
            steady += 1;
            if steady >= 3 {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(());
            }
        } else {
            steady = 0;
        }
        last_len = len;
        if started.elapsed() > cap {
            let _ = child.kill();
            let _ = child.wait();
            if file.is_file() {
                return Ok(());
            }
            bail!("took longer than {cap:?}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The page: dark terminal, monospace, the title as a band. Returns the
/// HTML and how many lines the body has (after the cap).
fn text_page(title: &str, text: &str) -> (String, usize) {
    fn esc(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }
    let mut lines: Vec<&str> = text.lines().collect();
    let mut cut = None;
    if lines.len() > TEXT_IMAGE_MAX_LINES {
        cut = Some(lines.len() - TEXT_IMAGE_MAX_LINES);
        lines.truncate(TEXT_IMAGE_MAX_LINES);
    }
    let mut body = esc(&lines.join("\n"));
    if let Some(n) = cut {
        body.push_str(&format!("\n… {n} more line(s) not shown"));
    }
    let n = lines.len() + usize::from(cut.is_some());
    let title_html = if title.trim().is_empty() {
        String::new()
    } else {
        format!("<div class=t>{}</div>", esc(title.trim()))
    };
    let html = format!(
        "<!doctype html><html><head><meta charset=utf-8><style>\
html,body{{margin:0;background:#1e1e1e}}\
body{{padding:14px 18px;font:14px/22px ui-monospace,SFMono-Regular,Menlo,Consolas,\"DejaVu Sans Mono\",monospace;color:#e6e6e6}}\
.t{{color:#9da5b4;border-bottom:1px solid #333;padding-bottom:8px;margin-bottom:10px;white-space:pre-wrap}}\
pre{{margin:0;white-space:pre-wrap;word-break:break-word}}\
</style></head><body>{title_html}<pre>{body}</pre></body></html>"
    );
    (html, n)
}

/// The Chrome the browser tool uses, if any.
fn chrome_binary() -> Option<PathBuf> {
    [
        "chromium",
        "google-chrome",
        "chromium-browser",
        "google-chrome-stable",
    ]
    .iter()
    .find_map(|name| which(std::ffi::OsStr::new(name)))
    .or_else(|| {
        let mac = Path::new("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome");
        mac.is_file().then(|| mac.to_path_buf())
    })
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
            // Text never reaches a display backend (`render_text`); the
            // root window is the harmless reading of it.
            Target::Screen | Target::Text => c.arg("root"),
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

pub(crate) fn display_hint() -> String {
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

pub(crate) fn which(program: &std::ffi::OsStr) -> Option<PathBuf> {
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
