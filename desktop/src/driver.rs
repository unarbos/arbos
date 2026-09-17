//! Driver: a control surface for the desktop window, so a program outside the
//! process can click every button, fill every field, move the mouse, read the
//! UI back, and take screenshots. It is how the app is pen-tested.
//!
//! Switched on by the environment. `ARBOS_DRIVER_SOCKET=/path.sock` names the
//! Unix socket to listen on; `ARBOS_DRIVER=1` picks one under the temp dir and
//! prints it to stderr. With neither set nothing here runs.
//!
//! Wire: newline-delimited JSON, one request per line, one reply per request.
//!
//! ```text
//! -> {"id": 1, "method": "click", "params": {"target": "composer-field"}}
//! <- {"id": 1, "ok": true, "result": {"x": 640.0, "y": 700.0, ...}}
//! <- {"id": 2, "ok": false, "error": "no element matches `nope`"}
//! ```
//!
//! Every reply is written after the window has drawn the frame that follows
//! the request, so what a reply reports is what is on screen.
//!
//! Positions are window points — logical pixels from the top-left of the
//! window's content, the same unit gpui lays out in. Screenshots are in
//! device pixels; `hello` reports the scale between the two.
//!
//! Methods: `hello`, `snapshot`, `find`, `state`, `move`, `down`, `up`,
//! `click`, `drag`, `scroll`, `key`, `type`, `fill`, `action`, `resize`,
//! `screenshot`, `quit`. See [`act`] for each one's parameters.

use crate::{
    model::{
        panel::PanelTab,
        project::Project,
        session::{ArtifactKind, ChatItem, ChatSession, Connection, ToolStatus},
        surface::{Bind, Surface},
    },
    view::{
        component::surface as board,
        root::{Arbos, Pane},
        settings::SettingsWindow,
    },
};
use anyhow::{Context as _, Result, anyhow, bail};
use bezel::gpui::{
    AnyWindowHandle, App, AppContext as _, Bounds, ElementProbe, Entity, Keystroke, Modifiers,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, PlatformInput, Point,
    ScrollDelta, ScrollWheelEvent, TouchPhase, Window, WindowHandle, point, px, size,
};
use futures::{StreamExt as _, channel::mpsc};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io::{BufRead as _, BufReader, Write as _},
    os::unix::{
        fs::PermissionsExt as _,
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
    sync::mpsc as sync_mpsc,
    thread,
};

/// The window title the screenshot looks for among AppKit's windows.
pub(crate) const WINDOW_TITLE: &str = "Arbos";

/// Where the socket goes, or `None` when the driver is switched off.
pub fn socket_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ARBOS_DRIVER_SOCKET") {
        return Some(PathBuf::from(path));
    }
    std::env::var_os("ARBOS_DRIVER")
        .filter(|flag| !flag.is_empty() && flag != "0")
        .map(|_| std::env::temp_dir().join(format!("arbos-desktop-{}.sock", std::process::id())))
}

/// One request from a connection, and where its reply goes.
struct Job {
    request: Value,
    reply: sync_mpsc::Sender<Value>,
}

/// Start listening. Returns at once; the socket lives until the app quits.
pub fn start(handle: WindowHandle<Arbos>, cx: &mut App) -> Result<PathBuf> {
    let path = socket_path().context("driver is not enabled")?;
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)
        .with_context(|| format!("bind driver socket {}", path.display()))?;
    // Whoever can open this socket can click and type in the app: owner only.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("chmod driver socket {}", path.display()))?;
    let (tx, mut rx) = mpsc::unbounded::<Job>();

    thread::Builder::new()
        .name("arbos-driver-accept".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let tx = tx.clone();
                let _ = thread::Builder::new()
                    .name("arbos-driver-conn".into())
                    .spawn(move || serve(stream, tx));
            }
        })
        .context("spawn driver accept thread")?;

    cx.spawn(async move |cx| {
        while let Some(job) = rx.next().await {
            let id = job.request.get("id").cloned().unwrap_or(Value::Null);
            let method = job
                .request
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let params = job.request.get("params").cloned().unwrap_or(Value::Null);
            let reply = job.reply;
            // `windows` and `quit` need no window of their own.
            if method == "windows" {
                let listed = cx.update(|cx| windows_json(handle, cx));
                let _ = reply.send(json!({ "id": id, "ok": true, "result": listed }));
                continue;
            }
            // Which window the request is for: the chat window unless the
            // params name another one (`"window": "settings"` or a window id).
            let wanted = params.get("window").cloned().unwrap_or(Value::Null);
            let target = match cx.update(|cx| resolve_window(handle, &wanted, cx)) {
                Ok(target) => target,
                Err(err) => {
                    let _ = reply.send(failure(id, &err));
                    continue;
                }
            };
            // The window is on the update stack inside this closure, so the
            // root comes from the view handed in, not from `handle.entity`.
            let step = cx.update_window(target, |view, window, cx| {
                let root = view.downcast::<Arbos>().ok();
                let acted = act(root.as_ref(), &method, &params, window, cx)?;
                Ok::<_, anyhow::Error>((root, acted))
            });
            let (root, acted) = match step {
                Ok(Ok(pair)) => pair,
                Ok(Err(err)) | Err(err) => {
                    let _ = reply.send(failure(id, &err));
                    continue;
                }
            };
            // Two frames: the callback runs at the start of a frame tick,
            // before the draw, so the first tick renders what the request
            // changed and the second reads it back.
            let reply_on_error = reply.clone();
            let id_on_error = id.clone();
            let quitting = method == "quit";
            // A request that closed its own window (CloseWindow on
            // Settings, Escape in it) has nothing left to settle on: the
            // request succeeded, and the reply says the window is gone
            // rather than failing.
            let still_open = cx.update(|cx| cx.windows().into_iter().any(|w| w == target));
            if !still_open {
                let mut result = acted;
                if let Some(obj) = result.as_object_mut() {
                    obj.insert("window_closed".into(), json!(true));
                }
                let _ = reply.send(json!({ "id": id, "ok": true, "result": result }));
                continue;
            }
            let settled = cx.update_window(target, |_, window, _| {
                window.refresh();
                window.on_next_frame(move |window, _| {
                    window.refresh();
                    window.on_next_frame(move |window, cx| {
                        let out = report(root.as_ref(), &method, acted, window, cx);
                        let _ = reply.send(match out {
                            Ok(result) => json!({ "id": id, "ok": true, "result": result }),
                            Err(err) => failure(id, &err),
                        });
                    });
                });
            });
            if let Err(err) = settled {
                let _ = reply_on_error.send(failure(id_on_error, &err));
                if quitting {
                    break;
                }
            }
        }
    })
    .detach();

    eprintln!("driver: listening on {}", path.display());
    Ok(path)
}

fn failure(id: Value, err: &anyhow::Error) -> Value {
    json!({ "id": id, "ok": false, "error": format!("{err:#}") })
}

/// What kind of window a handle is, by its root view type.
fn window_kind(window: &AnyWindowHandle) -> &'static str {
    if window.downcast::<Arbos>().is_some() {
        "main"
    } else if window.downcast::<SettingsWindow>().is_some() {
        "settings"
    } else {
        "other"
    }
}

fn window_id_json(window: &AnyWindowHandle) -> Value {
    json!(format!("{:?}", window.window_id()))
}

/// Every open window: kind, id, size, and whether it is the active one.
fn windows_json(main: WindowHandle<Arbos>, cx: &mut App) -> Value {
    let list: Vec<Value> = cx
        .windows()
        .into_iter()
        .map(|window| {
            let mut entry = json!({
                "kind": window_kind(&window),
                "id": window_id_json(&window),
                "main": window == main.into(),
            });
            let _ = cx.update_window(window, |_, w, _| {
                entry["width"] = json!(f32::from(w.viewport_size().width));
                entry["height"] = json!(f32::from(w.viewport_size().height));
                entry["active"] = json!(w.is_window_active());
            });
            entry
        })
        .collect();
    json!(list)
}

/// `"window"` param -> handle. Missing or `"main"` is the chat window;
/// `"settings"` is the settings window; anything else is a window id from
/// `windows`.
fn resolve_window(
    main: WindowHandle<Arbos>,
    wanted: &Value,
    cx: &mut App,
) -> Result<AnyWindowHandle> {
    match wanted.as_str() {
        None | Some("") | Some("main") => Ok(main.into()),
        Some(kind @ ("settings" | "other")) => cx
            .windows()
            .into_iter()
            .find(|window| window_kind(window) == kind)
            .with_context(|| format!("no {kind} window is open")),
        Some(id) => cx
            .windows()
            .into_iter()
            .find(|window| window_id_json(window) == json!(id))
            .with_context(|| format!("no window with id {id}")),
    }
}

/// One connection: read a line, hand it to the window, write the reply.
fn serve(stream: UnixStream, tx: mpsc::UnboundedSender<Job>) {
    let mut writer = match stream.try_clone() {
        Ok(writer) => writer,
        Err(_) => return,
    };
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(err) => {
                let reply = json!({ "id": null, "ok": false, "error": format!("bad json: {err}") });
                if write_line(&mut writer, &reply).is_err() {
                    break;
                }
                continue;
            }
        };
        let (reply_tx, reply_rx) = sync_mpsc::channel();
        if tx
            .unbounded_send(Job {
                request: request.clone(),
                reply: reply_tx,
            })
            .is_err()
        {
            break;
        }
        let Ok(mut reply) = reply_rx.recv() else {
            break;
        };
        if request.get("method").and_then(Value::as_str) == Some("screenshot") {
            reply = finish_screenshot(reply, &request);
        }
        if write_line(&mut writer, &reply).is_err() {
            break;
        }
    }
}

fn write_line(writer: &mut UnixStream, value: &Value) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    writer.write_all(&line)?;
    writer.flush()
}

/// The screenshot itself is taken off the main thread: `screencapture`
/// takes a few hundred milliseconds and the window must keep drawing.
fn finish_screenshot(reply: Value, request: &Value) -> Value {
    let id = reply.get("id").cloned().unwrap_or(Value::Null);
    let Some(window_id) = reply
        .get("result")
        .and_then(|result| result.get("window_id"))
        .and_then(Value::as_i64)
    else {
        return reply;
    };
    let path = request
        .get("params")
        .and_then(|params| params.get("path"))
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!(
                "arbos-shot-{}-{}.png",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0)
            ))
        });
    match capture_window(window_id, &path) {
        Ok(()) => {
            let mut result = reply.get("result").cloned().unwrap_or(json!({}));
            result["path"] = json!(path.display().to_string());
            json!({ "id": id, "ok": true, "result": result })
        }
        Err(err) => json!({ "id": id, "ok": false, "error": format!("{err:#}") }),
    }
}

/// `screencapture -l` grabs one window by its AppKit number, without its
/// shadow (`-o`) and without the camera sound (`-x`). It needs the Screen
/// Recording permission; macOS asks once, for whichever app launched us.
#[cfg(target_os = "macos")]
pub(crate) fn capture_window(window_id: i64, path: &std::path::Path) -> Result<()> {
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("create screenshot folder {}", dir.display()))?;
    }
    let status = std::process::Command::new("screencapture")
        .arg("-x")
        .arg("-o")
        .arg(format!("-l{window_id}"))
        .arg(path)
        .status()
        .context("run screencapture")?;
    if !status.success() {
        bail!("screencapture exited with {status}");
    }
    if std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0) == 0 {
        bail!("screencapture wrote nothing; is Screen Recording allowed?");
    }
    Ok(())
}

/// X11 (a test rig under Xvfb, a Linux desktop): grab the whole display
/// with ImageMagick's `import`, else `xwd` piped through `convert`. The
/// window is the only thing on an Xvfb screen, so the root is the window;
/// on a real desktop the caller crops if it must. `window_id` is unused.
#[cfg(not(target_os = "macos"))]
pub(crate) fn capture_window(_window_id: i64, path: &std::path::Path) -> Result<()> {
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("create screenshot folder {}", dir.display()))?;
    }
    let display =
        std::env::var("DISPLAY").context("DISPLAY is not set; no X display to capture")?;
    let via_import = std::process::Command::new("import")
        .args(["-display", &display, "-window", "root"])
        .arg(path)
        .status();
    let ok = match via_import {
        Ok(status) if status.success() => true,
        _ => {
            let xwd = std::process::Command::new("xwd")
                .args(["-root", "-silent", "-display", &display])
                .output()
                .context("run import or xwd (install imagemagick or x11-apps)")?;
            if !xwd.status.success() {
                bail!("xwd exited with {}", xwd.status);
            }
            let mut convert = std::process::Command::new("convert")
                .arg("xwd:-")
                .arg(path)
                .stdin(std::process::Stdio::piped())
                .spawn()
                .context("run convert (install imagemagick)")?;
            convert
                .stdin
                .take()
                .context("convert stdin")?
                .write_all(&xwd.stdout)?;
            convert.wait()?.success()
        }
    };
    if !ok || std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0) == 0 {
        bail!("capture of {display} wrote nothing");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Requests

/// Where a mouse action lands: an element by name, or a point.
#[derive(Deserialize, Default)]
struct Where {
    target: Option<String>,
    x: Option<f32>,
    y: Option<f32>,
}

#[derive(Deserialize, Default)]
struct Mods {
    #[serde(default)]
    shift: bool,
    #[serde(default)]
    ctrl: bool,
    #[serde(default)]
    alt: bool,
    #[serde(default)]
    cmd: bool,
    #[serde(default, rename = "fn")]
    fn_: bool,
}

impl From<Mods> for Modifiers {
    fn from(mods: Mods) -> Self {
        Modifiers {
            control: mods.ctrl,
            alt: mods.alt,
            shift: mods.shift,
            platform: mods.cmd,
            function: mods.fn_,
        }
    }
}

#[derive(Deserialize, Default)]
struct MouseParams {
    #[serde(flatten)]
    at: Where,
    button: Option<String>,
    count: Option<usize>,
    #[serde(default)]
    modifiers: Mods,
}

#[derive(Deserialize)]
struct DragParams {
    from: Where,
    to: Where,
    steps: Option<usize>,
    button: Option<String>,
    #[serde(default)]
    modifiers: Mods,
}

#[derive(Deserialize)]
struct ScrollParams {
    #[serde(flatten)]
    at: Where,
    #[serde(default)]
    dx: f32,
    #[serde(default)]
    dy: f32,
    #[serde(default)]
    lines: bool,
    #[serde(default)]
    modifiers: Mods,
}

#[derive(Deserialize)]
struct KeyParams {
    keys: String,
}

/// `fn`: hold or release the Fn key, as the native monitor would report it.
#[derive(Deserialize)]
struct FnParams {
    down: bool,
}

#[derive(Deserialize)]
struct TypeParams {
    text: String,
}

#[derive(Deserialize)]
struct FillParams {
    target: String,
    text: String,
}

#[derive(Deserialize)]
struct ActionParams {
    name: String,
    data: Option<Value>,
}

#[derive(Deserialize)]
struct ResizeParams {
    width: f32,
    height: f32,
}

#[derive(Deserialize)]
struct FindParams {
    target: String,
}

fn parse<T: for<'de> Deserialize<'de>>(params: &Value) -> Result<T> {
    let params = if params.is_null() {
        json!({})
    } else {
        params.clone()
    };
    serde_json::from_value(params).context("bad params")
}

/// Do what the request asks. Whatever it returns is the reply for methods
/// that only act; `report` replaces it for the ones that read.
fn act(
    root: Option<&Entity<Arbos>>,
    method: &str,
    params: &Value,
    window: &mut Window,
    cx: &mut App,
) -> Result<Value> {
    // Every request keeps the driven windows floating: Stage Manager would
    // otherwise swap them for strip thumbnails the moment the operator's own
    // app takes the stage, and captures and clicks would land on those. The
    // window this request targets also comes to the front: an occluded
    // window is not painted, and the reply waits for its next frame.
    crate::view::root::float_main_window();
    let extent = window.bounds().size;
    if let Some(number) = ns_window_number(f32::from(extent.width), f32::from(extent.height)) {
        order_front_regardless(number);
    }
    // And key: a person's click or chord always lands on the key window,
    // and app-level actions (⌘W) act on it.
    if !window.is_window_active() {
        window.activate_window();
    }
    match method {
        "hello" | "snapshot" | "state" => Ok(Value::Null),
        "find" => {
            let FindParams { target } = parse(params)?;
            let probe = find(window, &target)?;
            Ok(describe(&probe, window))
        }
        "move" => {
            let MouseParams { at, modifiers, .. } = parse(params)?;
            let position = locate(window, &at)?;
            mouse_move(position, None, modifiers.into(), window, cx);
            Ok(point_json(position))
        }
        "down" => {
            let MouseParams {
                at,
                button,
                count,
                modifiers,
            } = parse(params)?;
            let position = locate(window, &at)?;
            let modifiers: Modifiers = modifiers.into();
            let button = button_of(button.as_deref())?;
            mouse_move(position, None, modifiers, window, cx);
            mouse_down(position, button, count.unwrap_or(1), modifiers, window, cx);
            Ok(point_json(position))
        }
        "up" => {
            let MouseParams {
                at,
                button,
                count,
                modifiers,
            } = parse(params)?;
            let position = locate(window, &at)?;
            let button = button_of(button.as_deref())?;
            mouse_up(
                position,
                button,
                count.unwrap_or(1),
                modifiers.into(),
                window,
                cx,
            );
            Ok(point_json(position))
        }
        "click" => {
            let MouseParams {
                at,
                button,
                count,
                modifiers,
            } = parse(params)?;
            let probe = at
                .target
                .as_deref()
                .map(|target| find(window, target))
                .transpose()?;
            let position = match &probe {
                Some(probe) => probe.bounds.center(),
                None => locate(window, &at)?,
            };
            let modifiers: Modifiers = modifiers.into();
            let button = button_of(button.as_deref())?;
            let count = count.unwrap_or(1).max(1);
            mouse_move(position, None, modifiers, window, cx);
            for n in 1..=count {
                mouse_down(position, button, n, modifiers, window, cx);
                mouse_up(position, button, n, modifiers, window, cx);
            }
            let mut out = point_json(position);
            if let Some(probe) = probe {
                out["element"] = describe(&probe, window);
            }
            Ok(out)
        }
        "drag" => {
            let DragParams {
                from,
                to,
                steps,
                button,
                modifiers,
            } = parse(params)?;
            let start = locate(window, &from)?;
            let end = locate(window, &to)?;
            let modifiers: Modifiers = modifiers.into();
            let button = button_of(button.as_deref())?;
            let steps = steps.unwrap_or(8).max(1);
            mouse_move(start, None, modifiers, window, cx);
            mouse_down(start, button, 1, modifiers, window, cx);
            for step in 1..=steps {
                let t = step as f32 / steps as f32;
                let at = point(
                    start.x + (end.x - start.x) * t,
                    start.y + (end.y - start.y) * t,
                );
                mouse_move(at, Some(button), modifiers, window, cx);
            }
            mouse_up(end, button, 1, modifiers, window, cx);
            Ok(json!({ "from": point_json(start), "to": point_json(end) }))
        }
        "scroll" => {
            let ScrollParams {
                at,
                dx,
                dy,
                lines,
                modifiers,
            } = parse(params)?;
            let position = locate(window, &at)?;
            let delta = if lines {
                ScrollDelta::Lines(point(dx, dy))
            } else {
                ScrollDelta::Pixels(point(px(dx), px(dy)))
            };
            window.dispatch_event(
                PlatformInput::ScrollWheel(ScrollWheelEvent {
                    position,
                    delta,
                    modifiers: modifiers.into(),
                    touch_phase: TouchPhase::Moved,
                }),
                cx,
            );
            Ok(point_json(position))
        }
        "key" => {
            let KeyParams { keys } = parse(params)?;
            let mut handled = Vec::new();
            for chord in keys.split_whitespace() {
                let keystroke = Keystroke::parse(chord)
                    .map_err(|err| anyhow!("bad keystroke `{chord}`: {err:?}"))?;
                handled.push(window.dispatch_keystroke(keystroke, cx));
            }
            Ok(json!({ "handled": handled }))
        }
        "type" => {
            let TypeParams { text } = parse(params)?;
            type_text(&text, window, cx);
            Ok(json!({ "typed": text.chars().count() }))
        }
        "fill" => {
            let FillParams { target, text } = parse(params)?;
            let probe = find(window, &target)?;
            let position = probe.bounds.center();
            let modifiers = Modifiers::default();
            mouse_move(position, None, modifiers, window, cx);
            mouse_down(position, MouseButton::Left, 1, modifiers, window, cx);
            mouse_up(position, MouseButton::Left, 1, modifiers, window, cx);
            let select_all = Keystroke::parse("cmd-a").map_err(|err| anyhow!("{err:?}"))?;
            window.dispatch_keystroke(select_all, cx);
            if text.is_empty() {
                let delete = Keystroke::parse("backspace").map_err(|err| anyhow!("{err:?}"))?;
                window.dispatch_keystroke(delete, cx);
            } else {
                type_text(&text, window, cx);
            }
            Ok(json!({ "element": describe(&probe, window) }))
        }
        "action" => {
            let ActionParams { name, data } = parse(params)?;
            // ⌘W's handler acts on the key window through a nested window
            // update, which cannot run from inside this request's own update
            // of the target. Close the target here instead, with the menu's
            // semantics: the chat window takes Settings with it.
            if name == "arbos::CloseWindow" {
                if root.is_some() {
                    crate::kernel::shutdown_tunnels();
                    cx.defer(|cx| {
                        for other in cx.windows() {
                            if let Some(handle) =
                                other.downcast::<crate::view::settings::SettingsWindow>()
                            {
                                let _ = handle.update(cx, |_, window, _| window.remove_window());
                            }
                        }
                    });
                }
                window.remove_window();
                return Ok(json!({ "action": name, "window_closed": true }));
            }
            let action = cx
                .build_action(&name, data)
                .map_err(|err| anyhow!("build action `{name}`: {err:?}"))?;
            window.dispatch_action(action, cx);
            Ok(json!({ "action": name }))
        }
        "resize" => {
            let ResizeParams { width, height } = parse(params)?;
            window.resize(size(px(width), px(height)));
            Ok(json!({ "width": width, "height": height }))
        }
        "screenshot" => {
            // macOS hands back a tilted thumbnail for a window that is not
            // frontmost, so bring it forward; the capture runs two frames
            // later, once the reply settles.
            window.activate_window();
            let size = window.bounds().size;
            let window_id = ns_window_number(f32::from(size.width), f32::from(size.height))
                .context("find the window to capture")?;
            // Activation is a request the OS may refuse while the user is
            // busy in another app; raising the window without activating
            // is always allowed and is enough for the capture.
            order_front_regardless(window_id);
            Ok(json!({
                "window_id": window_id,
                "scale": window.scale_factor(),
                "width": f32::from(window.viewport_size().width),
                "height": f32::from(window.viewport_size().height),
            }))
        }
        // The hold-Fn dictation path, by the same calls the key monitor
        // makes: down starts a take into the composer, up ends it. Drivable
        // everywhere, so the UI pass can check the wiring; on a machine with
        // no dictation the notice it raises is the check.
        "fn" => {
            let FnParams { down } = parse(params)?;
            let root = root.context("no main window")?;
            root.update(cx, |this, cx| this.fn_key(down, cx));
            Ok(json!({ "down": down }))
        }
        "quit" => {
            cx.quit();
            Ok(json!({ "quit": true }))
        }
        "" => bail!("request has no `method`"),
        other => bail!("unknown method `{other}`"),
    }
}

/// Build the reply, one settled frame after the request.
fn report(
    root: Option<&Entity<Arbos>>,
    method: &str,
    acted: Value,
    window: &mut Window,
    cx: &mut App,
) -> Result<Value> {
    match method {
        "hello" => Ok(json!({
            "pid": std::process::id(),
            "version": env!("CARGO_PKG_VERSION"),
            "window": window_json(window),
            "windows": cx.windows().len(),
        })),
        "snapshot" => Ok(snapshot(root, window, cx)),
        "state" => Ok(state(root, window, cx)),
        _ => Ok(acted),
    }
}

// ---------------------------------------------------------------------------
// Input

fn button_of(name: Option<&str>) -> Result<MouseButton> {
    Ok(match name.unwrap_or("left") {
        "left" => MouseButton::Left,
        "right" => MouseButton::Right,
        "middle" => MouseButton::Middle,
        other => bail!("unknown mouse button `{other}`"),
    })
}

fn mouse_move(
    position: Point<Pixels>,
    pressed: Option<MouseButton>,
    modifiers: Modifiers,
    window: &mut Window,
    cx: &mut App,
) {
    window.dispatch_event(
        PlatformInput::MouseMove(MouseMoveEvent {
            position,
            pressed_button: pressed,
            modifiers,
        }),
        cx,
    );
}

fn mouse_down(
    position: Point<Pixels>,
    button: MouseButton,
    click_count: usize,
    modifiers: Modifiers,
    window: &mut Window,
    cx: &mut App,
) {
    window.dispatch_event(
        PlatformInput::MouseDown(MouseDownEvent {
            button,
            position,
            modifiers,
            click_count,
            first_mouse: false,
        }),
        cx,
    );
}

fn mouse_up(
    position: Point<Pixels>,
    button: MouseButton,
    click_count: usize,
    modifiers: Modifiers,
    window: &mut Window,
    cx: &mut App,
) {
    window.dispatch_event(
        PlatformInput::MouseUp(MouseUpEvent {
            button,
            position,
            modifiers,
            click_count,
        }),
        cx,
    );
}

/// Type as a keyboard would: one keystroke per character, so a field's own
/// bindings (`enter` to send, `escape` to dismiss) fire like they do for a
/// person. Newline is the `enter` key; tab is the `tab` key.
fn type_text(text: &str, window: &mut Window, cx: &mut App) {
    for ch in text.chars() {
        let keystroke = match ch {
            '\n' => Keystroke::parse("enter").ok(),
            '\t' => Keystroke::parse("tab").ok(),
            ' ' => Keystroke::parse("space").ok(),
            ch => Some(Keystroke {
                modifiers: Modifiers::default(),
                key: ch.to_lowercase().to_string(),
                key_char: Some(ch.to_string()),
            }),
        };
        if let Some(keystroke) = keystroke {
            window.dispatch_keystroke(keystroke, cx);
        }
    }
}

// ---------------------------------------------------------------------------
// Finding elements

/// Resolve a request's `target` / `x,y` to a window point.
fn locate(window: &Window, at: &Where) -> Result<Point<Pixels>> {
    if let Some(target) = &at.target {
        return Ok(find(window, target)?.bounds.center());
    }
    match (at.x, at.y) {
        (Some(x), Some(y)) => Ok(point(px(x), px(y))),
        _ => bail!("give either `target` or both `x` and `y`"),
    }
}

/// Whether `pattern` names the element at `path`. A pattern is a full path
/// (`a.b.c`), a tail of one (`c` or `b.c`), or either with `*` wildcards.
fn matches(pattern: &str, path: &str) -> bool {
    if pattern.contains('*') {
        return wild(pattern, path) || wild(&format!("*.{pattern}"), path);
    }
    path == pattern || path.ends_with(&format!(".{pattern}"))
}

/// `*` matches any run of characters, including none.
fn wild(pattern: &str, text: &str) -> bool {
    let mut parts = pattern.split('*');
    let Some(head) = parts.next() else {
        return true;
    };
    if !text.starts_with(head) {
        return false;
    }
    let mut rest = &text[head.len()..];
    let mut tail: Option<&str> = None;
    for part in parts {
        if let Some(prev) = tail {
            let Some(ix) = rest.find(prev) else {
                return false;
            };
            rest = &rest[ix + prev.len()..];
        }
        tail = Some(part);
    }
    match tail {
        Some(last) => rest.ends_with(last),
        None => true,
    }
}

/// Every element whose id matches, reachable ones first, topmost first.
fn candidates(window: &Window, pattern: &str) -> Vec<ElementProbe> {
    let mut found: Vec<(bool, ElementProbe)> = window
        .element_probes()
        .iter()
        .filter(|probe| matches(pattern, &probe.id.to_string()))
        .map(|probe| (reachable(probe, window), probe.clone()))
        .collect();
    // Stable: keeps prepaint order within each group, and reachable first.
    found.sort_by_key(|(hit, _)| !*hit);
    found.into_iter().map(|(_, probe)| probe).collect()
}

fn find(window: &Window, pattern: &str) -> Result<ElementProbe> {
    let mut found = candidates(window, pattern);
    if found.is_empty() {
        bail!("no element matches `{pattern}`");
    }
    Ok(found.swap_remove(0))
}

/// Whether a real click at the element's centre would reach it: it has a
/// hitbox, that hitbox is under the point, and nothing in front blocks it.
fn reachable(probe: &ElementProbe, window: &Window) -> bool {
    let Some(hitbox) = probe.hitbox else {
        return false;
    };
    let visible = probe.bounds.intersect(&probe.content_mask.bounds);
    if visible.is_empty() {
        return false;
    }
    window.hitboxes_at(visible.center()).contains(&hitbox)
}

fn describe(probe: &ElementProbe, window: &Window) -> Value {
    let visible = probe.bounds.intersect(&probe.content_mask.bounds);
    let path = probe.id.to_string();
    let name = path.rsplit('.').next().unwrap_or(&path).to_owned();
    json!({
        "id": name,
        "path": path,
        "x": f32::from(probe.bounds.origin.x),
        "y": f32::from(probe.bounds.origin.y),
        "w": f32::from(probe.bounds.size.width),
        "h": f32::from(probe.bounds.size.height),
        "cx": f32::from(probe.bounds.center().x),
        "cy": f32::from(probe.bounds.center().y),
        "visible": !visible.is_empty(),
        "interactive": probe.hitbox.is_some(),
        "reachable": reachable(probe, window),
    })
}

/// The last dictated take's clock, with the gateway's own numbers.
fn voice_latency(this: &Arbos) -> Value {
    let voice = crate::voice_ws::status();
    json!({
        "first_partial_ms": this.dictation.first_partial_ms,
        "release_to_send_ms": this.dictation.release_to_send_ms,
        "gateway_first_partial_ms": voice.first_partial_ms,
        "partial_age_ms": voice.partial_age_ms,
        "phase": voice.phase.map(|p| p.as_str()),
    })
}

fn point_json(position: Point<Pixels>) -> Value {
    json!({ "x": f32::from(position.x), "y": f32::from(position.y) })
}

fn bounds_json(bounds: Bounds<Pixels>) -> Value {
    json!({
        "x": f32::from(bounds.origin.x),
        "y": f32::from(bounds.origin.y),
        "w": f32::from(bounds.size.width),
        "h": f32::from(bounds.size.height),
    })
}

fn window_json(window: &Window) -> Value {
    let mouse = window.mouse_position();
    json!({
        "width": f32::from(window.viewport_size().width),
        "height": f32::from(window.viewport_size().height),
        "scale": window.scale_factor(),
        "active": window.is_window_active(),
        "bounds": bounds_json(window.bounds()),
        "mouse": point_json(mouse),
    })
}

/// Every identified element on screen, with where it is and whether a click
/// would reach it, plus the app's own account of itself.
fn snapshot(root: Option<&Entity<Arbos>>, window: &mut Window, cx: &mut App) -> Value {
    let elements: Vec<Value> = window
        .element_probes()
        .iter()
        .map(|probe| describe(probe, window))
        .collect();
    json!({
        "window": window_json(window),
        "elements": elements,
        "state": state(root, window, cx),
    })
}

// ---------------------------------------------------------------------------
// App state

/// What the app believes is going on, in words a test can assert on: which
/// pane shows, what is open, what the composer holds, what was said.
fn state(root: Option<&Entity<Arbos>>, window: &Window, cx: &App) -> Value {
    let Some(root) = root else {
        // Not the chat window (settings, for one): only the frame is known.
        return json!({ "window": "other", "focused": window.is_window_active() });
    };
    let this = root.read(cx);
    let workspace = this.workspace.read(cx);
    let composer = this.composer.read(cx);
    let field = composer.text_field();
    let composer_focused = this.composer_focus_handle(cx).is_focused(window);
    let projects: Vec<Value> = workspace
        .projects
        .iter()
        .enumerate()
        .map(|(ix, project)| {
            json!({
                "index": ix,
                "name": project.name(),
                "path": project.path.display().to_string(),
                "host": project.host,
                "active": workspace.active == Some(ix),
                // The tab's badge as tabs.rs draws it: a dot while a chat
                // asks or holds notifications nobody has looked at (#297).
                "tab_dot": project.sessions.iter().filter(|chat| !chat.closed).any(|chat| {
                    !chat.unseen.is_empty() || chat.plan_open().any(|n| n.do_kind == "ask")
                }),
                "unseen": project.sessions.iter().filter(|chat| !chat.closed).map(|chat| chat.unseen.len()).sum::<usize>(),
                "archive_open": project.archive_open,
                "focus": project.focus.map(|focus| json!({
                    "agent": focus.agent,
                    "surface": focus.surface.map(|id| id.0),
                })),
                "sessions": project.sessions.iter().map(|chat| session_json(Some(project), chat)).collect::<Vec<_>>(),
                "surfaces": project.surfaces.iter().map(surface_json).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "pane": pane_name(Some(this.pane)),
        "showing": pane_name(this.showing(cx)),
        // The side panel: whether it is out, how wide, its own tabs and which
        // of them is in front, plus whether its row is the one the tab chords
        // will move. `panel_open` stays under its old name — the parity loop
        // and the journeys assert on it.
        "panel_open": workspace.panel().is_some_and(|panel| panel.open),
        "panel": workspace.panel().map(|panel| json!({
            "open": panel.open,
            "width": panel.width(),
            "active": panel.active(),
            "focused": this.panel_focused(window, cx),
            "tabs": panel.tabs().iter().map(|tab| match tab {
                PanelTab::Project => json!({ "kind": "project" }),
                PanelTab::New(n) => json!({ "kind": "new", "id": n }),
                PanelTab::Surface(id) => {
                    let surface = workspace.active_project().and_then(|p| p.surface(*id));
                    json!({
                        "kind": "surface",
                        "id": id.0,
                        "title": surface.map(board::title),
                        "board_kind": surface.map(|s| s.board_kind.clone()),
                        "state": surface.and_then(board::state_word),
                    })
                }
            }).collect::<Vec<_>>(),
        })),
        "text_size": workspace.text_size,
        "bionic_reading": workspace.bionic_reading,
        // The rest of the Settings window's values, so a click on a control
        // there can be asserted on state and not recorded `unverified`
        // (rig audit R3, cycle 32).
        "appearance": format!("{:?}", workspace.appearance).to_ascii_lowercase(),
        "reduce_transparency": workspace.reduce_transparency,
        "cursor_blink": workspace.cursor_blink,
        "tint": { "hue": workspace.tint.hue, "chroma": workspace.tint.chroma },
        "watch_bounce": workspace.settings.watch_bounce,
        "update_channel": workspace.settings.update.channel,
        "notifications": {
            "notifier": crate::notify_os::NOTIFIER,
            "window_active": this.window_active,
            "touched": this.touched,
            "posted": this.notifications_posted.iter().map(|n| json!({
                "at": n.at,
                "title": n.title,
                "body": n.body,
                "error": n.error,
            })).collect::<Vec<_>>(),
        },
        "settings_open": cx.windows().iter().any(|w| w.downcast::<SettingsWindow>().is_some()),
        "opener_open": this.opener.read(cx).open,
        "search_open": this.chat_search.read(cx).is_open(),
        "feedback": {
            "sheet_open": this.feedback_sheet.read(cx).is_open,
            // The sentence he reads after Send. The one thing this feature
            // cannot get wrong is promising something it does not do, so the
            // promise itself is assertable rather than only photographable.
            "message": this.feedback_sheet.read(cx).message().map(|(ok, text)| json!({
                "ok": ok,
                "text": text,
            })),
            // What the sheet says about the trajectory when there is none. The
            // fault Jacob hit was invisible to the rig because this was not
            // here: three rows reading "nothing to send" look exactly like a
            // report that had nothing to attach.
            "unavailable": this.feedback_sheet.read(cx).unavailable(),
            "screenshot": {
                "attached": this.feedback_sheet.read(cx).shot_state().0,
                "whole_screen": this.feedback_sheet.read(cx).shot_state().1,
                "error": this.feedback_sheet.read(cx).shot_state().2,
            },
            "outbox": {
                "waiting": this.feedback_outbox.waiting,
                "sent_this_run": this.feedback_outbox.sent_this_run,
                "last_error": this.feedback_outbox.last_error,
                "drained": this.feedback_outbox.at.is_some(),
            },
        },
        "permissions": {
            "open": this.permissions_sheet.read(cx).is_open(),
            "seen": workspace.permissions_seen,
            "wants_attention": this.permission_center.read(cx).wants_attention(),
            "enabling_all": this.permission_center.read(cx).enabling_all,
            "rows": this.permission_center.read(cx).rows.iter().map(|row| json!({
                "permission": row.permission.title(),
                "status": match &row.status {
                    crate::permissions::Status::Granted => "granted",
                    crate::permissions::Status::NotAsked => "not_asked",
                    crate::permissions::Status::Denied => "denied",
                    crate::permissions::Status::Unavailable(_) => "unavailable",
                },
                "phase": match &row.phase {
                    crate::model::permission_center::Phase::Idle => "idle",
                    crate::model::permission_center::Phase::Requesting { .. } => "requesting",
                    crate::model::permission_center::Phase::Prompted { .. } => "prompted",
                    crate::model::permission_center::Phase::NeedsSettings => "needs_settings",
                },
            })).collect::<Vec<_>>(),
        },
        "menu_open": this.menu.is_some(),
        "renaming": this.renaming.is_some(),
        "composer": {
            "text": field.read(cx).content().to_string(),
            "focused": composer_focused,
            "recording": composer.is_recording(),
            // The take's live words (dictation partials), painted after the caret.
            "preview": composer.voice_preview(),
        },
        // The last dictated take's clock: Fn press to first partial, release
        // to send. The gateway's own numbers ride along.
        "voice_latency": voice_latency(this),
        // The call to the project in front, when one is live: what the
        // strip shows, so a test can assert on it without pixels.
        "call": this.call.as_ref().map(|call| {
            let voice = crate::voice_ws::status();
            json!({
                "active": true,
                "connecting": call.connecting,
                "session": call.session,
                "label": call.label,
                "phase": voice.phase.map(|p| p.as_str()),
                "muted": voice.muted,
                "mic_device": voice.mic_device,
                "mic_error": voice.mic_error,
                "speaker_device": voice.speaker_device,
                "work": {
                    "active": voice.work_active,
                    "agents": voice.work_agents,
                    "stale": voice.work_stale,
                    "sound": voice.work_sound,
                },
                "played_bytes": crate::voice_ws::counters().0,
                "level": voice.level,
                "partial": voice.text,
                "reply": voice.reply,
                "last_said": voice.last_said,
                "seconds": call.since.elapsed().as_secs(),
            })
        }),
        "active_project": workspace.active,
        "active_session": workspace.active_id(),
        "active_surface": workspace.active_surface().map(|surface| surface.id.0),
        "projects": projects,
    })
}

fn pane_name(pane: Option<Pane>) -> Value {
    match pane {
        Some(Pane::Chat) => json!("chat"),
        Some(Pane::Surface) => json!("surface"),
        Some(Pane::Project) => json!("project"),
        None => Value::Null,
    }
}

fn session_json(project: Option<&Project>, chat: &ChatSession) -> Value {
    json!({
        "id": chat.id,
        "title": chat.title,
        "name": chat.name,
        "agent": chat.entry.name,
        "agent_session": chat.agent_session,
        "model": chat.model,
        "parent": chat.parent,
        "readonly": chat.readonly,
        "agent_kind": chat.agent_kind,
        "unseen": chat.unseen.len(),
        "unseen_kinds": chat.unseen.iter().map(|n| n.kind.clone()).collect::<Vec<_>>(),
        "seen_through": chat.seen_through,
        "draft": chat.draft,
        "queued": chat.queue.len(),
        "held": chat.plan_queued(),
        "asks": chat.plan_open().filter(|n| n.do_kind == "ask").count(),
        "reconnect_attempt": chat.reconnect_attempt,
        "usage": chat.usage.map(|u| json!({"used": u.used, "size": u.size, "spent": u.spent, "last_cost": u.last_cost})),
        "connection": match chat.connection {
            Connection::Idle => "idle",
            Connection::Connecting => "connecting",
            Connection::Live(_) => "live",
            Connection::Reconnecting(_) => "reconnecting",
            Connection::Lost => "lost",
        },
        "streaming": chat.streaming,
        "waiting": chat.waiting,
        "quiet_secs": chat.quiet_for().as_secs(),
        "turn_open": chat.turn_open,
        "closed": chat.closed,
        "pills": project.map(|project| {
            let (working, prs) = crate::view::detail::pill_counts(project, chat);
            json!({ "working": working.len(), "prs": prs.len(), "pr_urls": prs })
        }),
        "permission": chat.permission.as_ref().map(|prompt| prompt.title.clone()),
        "questions": chat.questions.as_ref().map(|prompt| prompt.title.clone()),
        "items": chat.items.iter().map(item_json).collect::<Vec<_>>(),
    })
}

/// A transcript item as a small record. Text is cut at a few thousand
/// characters so a long chat does not make every reply slow.
fn item_json(item: &ChatItem) -> Value {
    const LIMIT: usize = 4000;
    fn cut(text: &str) -> String {
        if text.chars().count() <= LIMIT {
            text.to_owned()
        } else {
            let mut out: String = text.chars().take(LIMIT).collect();
            out.push('…');
            out
        }
    }
    match item {
        ChatItem::User(message) => json!({
            "kind": "user",
            "text": cut(&message.text),
            "channel": message.channel,
            "images": message.images.len(),
            "files": message.files.len(),
            "sent_at": message.sent_at,
            "feedback": message.feedback,
            "seq": message.seq,
            "reported": message.reported,
        }),
        ChatItem::From { who, text, .. } => json!({
            "kind": "from",
            "who": who,
            "text": cut(text),
        }),
        ChatItem::Agent(text) => json!({ "kind": "agent", "text": cut(text) }),
        ChatItem::Thinking { text, done, .. } => json!({
            "kind": "thinking",
            "text": cut(text),
            "done": done,
        }),
        ChatItem::Tool {
            id,
            label,
            status,
            output,
            child_session,
            ..
        } => json!({
            "kind": "tool",
            "id": id,
            "label": label,
            "status": match status {
                ToolStatus::Running => "running",
                ToolStatus::Success => "success",
                ToolStatus::Failure => "failure",
            },
            "output": cut(output),
            "child_session": child_session,
        }),
        ChatItem::Asked { question, answer } => json!({
            "kind": "asked",
            "question": cut(question),
            "answer": cut(answer),
        }),
        ChatItem::Notice { text, failed } => json!({
            "kind": "notice",
            "text": cut(text),
            "failed": failed,
        }),
        ChatItem::Wake { kind, secs, .. } => json!({ "kind": "wake", "wake": kind, "secs": secs }),
        ChatItem::Nudge(text) => json!({
            "kind": "nudge",
            "text": cut(text),
        }),
        ChatItem::Artifacts(files) => json!({
            "kind": "artifacts",
            "files": files.iter().map(|file| json!({
                "kind": match file.kind {
                    ArtifactKind::Image => "image",
                    ArtifactKind::Video => "video",
                },
                "path": file.path,
                "name": file.name,
                "label": file.caption,
                "thumb": file.thumb.is_some(),
            })).collect::<Vec<_>>(),
        }),
    }
}

fn surface_json(surface: &Surface) -> Value {
    let (bind, url, has_shot) = match &surface.bind {
        Bind::Terminal { id, cwd } => (json!({ "terminal": id, "cwd": cwd }), None, false),
        Bind::Browser { id, url, shot } => {
            (json!({ "browser": id }), Some(url.clone()), shot.is_some())
        }
        Bind::Process {
            id,
            log,
            live,
            done,
        } => (
            json!({
                "process": id,
                "log": log.display().to_string(),
                "live_bytes": live.len(),
                "done": done.map(|code| json!(code)),
            }),
            None,
            false,
        ),
        Bind::Url(url) => (json!({ "url": url }), Some(url.clone()), false),
        Bind::Path(path) => (json!({ "path": path.display().to_string() }), None, false),
        Bind::Empty => (json!({}), None, false),
    };
    json!({
        "id": surface.id.0,
        "owner": surface.owner,
        "kind": format!("{:?}", surface.kind).to_lowercase(),
        "title": surface.title,
        "board_kind": surface.board_kind,
        "key": surface.key,
        "url": url,
        "has_screenshot": has_shot,
        "bind": bind,
    })
}

// ---------------------------------------------------------------------------
// AppKit

/// The AppKit window number of the gpui window whose frame is `width` x
/// `height` points, for `screencapture -l`. Two windows of one size fall
/// back to the one titled "Arbos", then to the first visible window.
#[cfg(target_os = "macos")]
pub(crate) fn ns_window_number(width: f32, height: f32) -> Option<i64> {
    use objc::{class, msg_send, runtime::Object, sel, sel_impl};

    #[repr(C)]
    struct NsRect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }

    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let windows: *mut Object = msg_send![app, windows];
        let count: usize = msg_send![windows, count];
        let mut by_size = None;
        let mut by_title = None;
        let mut fallback = None;
        for i in 0..count {
            let ns_window: *mut Object = msg_send![windows, objectAtIndex: i];
            if ns_window.is_null() {
                continue;
            }
            let visible: bool = msg_send![ns_window, isVisible];
            if !visible {
                continue;
            }
            let number: i64 = msg_send![ns_window, windowNumber];
            if fallback.is_none() {
                fallback = Some(number);
            }
            let frame: NsRect = msg_send![ns_window, frame];
            if by_size.is_none()
                && (frame.w as f32 - width).abs() < 2.
                && (frame.h as f32 - height).abs() < 2.
            {
                by_size = Some(number);
            }
            let title: *mut Object = msg_send![ns_window, title];
            if title.is_null() {
                continue;
            }
            let utf8: *const std::os::raw::c_char = msg_send![title, UTF8String];
            if utf8.is_null() {
                continue;
            }
            let title = std::ffi::CStr::from_ptr(utf8).to_string_lossy();
            if title == WINDOW_TITLE && by_title.is_none() {
                by_title = Some(number);
            }
        }
        by_size.or(by_title).or(fallback)
    }
}

#[cfg(target_os = "macos")]
fn order_front_regardless(window_id: i64) {
    use objc::{class, msg_send, runtime::Object, sel, sel_impl};
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let ns_window: *mut Object = msg_send![app, windowWithWindowNumber: window_id];
        if !ns_window.is_null() {
            let _: () = msg_send![ns_window, orderFrontRegardless];
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn order_front_regardless(_window_id: i64) {}

/// No AppKit here: the capture path grabs the X display, so any id will do.
#[cfg(not(target_os = "macos"))]
pub(crate) fn ns_window_number(_width: f32, _height: f32) -> Option<i64> {
    Some(0)
}
